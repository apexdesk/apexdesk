//! The personal macro workbook (`personal.xlsm` in the settings folder)
//! and the installed add-ins (File > Options > Macros): macros of every
//! workbook may call theirs (`Application.Run "PERSONAL.XLSM!Macro"`),
//! and Run Macro lists them, to run on the active workbook. They are read
//! again when their files change. With macros off none is loaded, and
//! when only signed macros may run, only signed ones are.

use apex_fs::PathExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use web_time::{Duration, Instant, SystemTime};

use apex_vba::{Library, Project};

use super::super::{App, Notice};
use crate::i18n::tr;

/// The files are looked at again at most this often.
const RECHECK: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Libs {
    loaded: Vec<Loaded>,
    checked: Option<Instant>,
    /// The settings they were loaded under (on, signed only, files).
    under: Option<(bool, Option<Vec<String>>, Vec<PathBuf>)>,
    /// The modules of open workbooks' projects, read lately.
    open: Vec<(std::sync::Weak<Vec<u8>>, Arc<Vec<apex_vba::Source>>)>,
}

struct Loaded {
    path: PathBuf,
    /// The file's time and size when read.
    stamp: Option<(SystemTime, u64)>,
    lib: Option<Library>,
    /// Its public Subs without parameters.
    macros: Vec<String>,
}

/// The personal macro workbook.
pub(crate) fn personal_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("personal.xlsm"))
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = apex_fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The library in `path`, when its macros may run.
/// `signed_only`: the publishers trusted, when only signed macros run.
fn load(path: &Path, signed_only: Option<&[String]>) -> Option<(Library, Vec<String>)> {
    let (apex_io::Loaded::Sheet(wb), _) = apex_io::open(path).ok()? else { return None };
    let bin = wb.macros.project.as_ref()?;
    if signed_only.is_some_and(|p| !super::signed_book(&wb, p)) {
        return None;
    }
    let sources = super::sources(bin).ok()?;
    let macros = Project::parse(&sources).map(|p| p.macros()).unwrap_or_default();
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some((Library { name, sources: Arc::new(sources) }, macros))
}

impl App {
    /// The libraries macros may call now, read again when their files
    /// changed.
    pub(super) fn libraries(&mut self) -> Vec<Library> {
        self.refresh_libraries();
        self.macro_run.libs.loaded.iter().filter_map(|l| l.lib.clone()).collect()
    }

    /// What a macro of tab `id` may call with `Application.Run`: the other
    /// open workbooks whose macros are enabled, then the libraries.
    pub(super) fn run_targets(&mut self, id: u64) -> Vec<Library> {
        let books: Vec<(String, Arc<Vec<u8>>)> = self
            .tabs
            .iter()
            .filter(|t| t.id != id && super::allowed(&self.settings, &self.tabs, t.id) && self.macro_run.is_enabled(t.id) && !self.is_protected(t.id))
            .filter_map(|t| match &t.view {
                super::View::Sheet(v) => v.wb.macros.project.clone().map(|b| (t.name(), b)),
                _ => None,
            })
            .collect();
        let cache = &mut self.macro_run.libs.open;
        cache.retain(|(w, _)| w.strong_count() > 0);
        let mut out = Vec::new();
        for (name, bin) in books {
            let known = cache.iter().find(|(w, _)| w.upgrade().is_some_and(|b| Arc::ptr_eq(&b, &bin))).map(|(_, s)| s.clone());
            let sources = match known {
                Some(s) => s,
                None => {
                    let Ok(s) = super::sources(&bin) else { continue };
                    let s = Arc::new(s);
                    cache.push((Arc::downgrade(&bin), s.clone()));
                    s
                }
            };
            out.push(Library { name, sources });
        }
        out.extend(self.libraries());
        out
    }

    /// The macros of the libraries, as Run Macro lists them
    /// (`personal.xlsm!Macro`), but those of the file of tab `file`.
    pub(super) fn library_macros(&mut self, file: &str) -> Vec<String> {
        self.refresh_libraries();
        let libs = &self.macro_run.libs.loaded;
        libs.iter()
            .filter_map(|l| l.lib.as_ref().map(|lib| (lib, &l.macros)))
            .filter(|(lib, _)| !lib.is(file))
            .flat_map(|(lib, macros)| macros.iter().map(move |m| format!("{}!{m}", lib.name)))
            .collect()
    }

    /// Whether `name` is `Book!Macro` of a library (not tab `id`'s file,
    /// a workbook: libraries work on workbooks).
    pub(super) fn is_library_macro(&mut self, id: u64, name: &str) -> bool {
        let Some((book, _)) = name.rsplit_once('!') else { return false };
        let Some(t) = self.tabs.iter().find(|t| t.id == id).filter(|t| matches!(t.view, super::View::Sheet(_))) else { return false };
        let file = t.name();
        !apex_vba::same_book(&file, book) && self.libraries().iter().any(|l| l.is(book))
    }

    fn refresh_libraries(&mut self) {
        let s = &self.settings;
        let under = (s.macros_enabled, s.macros_signed_only.then(|| s.trusted_publishers.clone()), s.addins.clone());
        let libs = &mut self.macro_run.libs;
        let fresh = libs.under.as_ref() == Some(&under) && libs.checked.is_some_and(|t| t.elapsed() < RECHECK);
        if fresh {
            return;
        }
        if libs.under.as_ref() != Some(&under) {
            libs.loaded.clear();
        }
        libs.checked = Some(Instant::now());
        let (on, signed_only, addins) = under.clone();
        libs.under = Some(under);
        if !on {
            libs.loaded.clear();
            return;
        }
        let paths: Vec<PathBuf> = personal_path().into_iter().chain(addins).collect();
        libs.loaded.retain(|l| paths.contains(&l.path));
        for path in paths {
            let now = stamp(&path);
            if let Some(l) = libs.loaded.iter_mut().find(|l| l.path == path) {
                if l.stamp == now {
                    continue;
                }
                l.stamp = now;
                (l.lib, l.macros) = now.and_then(|_| load(&path, signed_only.as_deref())).map_or((None, Vec::new()), |(a, b)| (Some(a), b));
                continue;
            }
            let (lib, macros) = now.and_then(|_| load(&path, signed_only.as_deref())).map_or((None, Vec::new()), |(a, b)| (Some(a), b));
            libs.loaded.push(Loaded { path, stamp: now, lib, macros });
        }
        // Two files of the same name: the first one counts.
        let mut seen = Vec::new();
        for l in &mut libs.loaded {
            if let Some(lib) = &l.lib {
                if seen.iter().any(|n: &String| lib.is(n)) {
                    l.lib = None;
                    l.macros.clear();
                } else {
                    seen.push(lib.name.clone());
                }
            }
        }
    }

    /// Developer > Personal Macro Workbook: opens it, made first (with an
    /// empty module) when there is none.
    pub(in crate::app) fn open_personal_workbook(&mut self) {
        let Some(path) = personal_path() else { return };
        if !path.fs_exists()
            && let Err(e) = new_personal(&path)
        {
            self.notify(Notice::error(format!("{}: {e}", tr("macros.personal"))));
            return;
        }
        self.open_path(&path);
    }
}

/// A workbook with an empty project and one module, `Module1`.
pub(super) fn new_personal(path: &Path) -> Result<(), String> {
    let mut wb = apex_sheet::Workbook::new();
    let sheets: Vec<String> = (1..=wb.sheets.len()).map(|i| format!("Sheet{i}")).collect();
    let bin = apex_io::vba::new_workbook_project("ThisWorkbook", &sheets).map_err(|e| e.to_string())?;
    let mut p = apex_io::vba::read(&bin).map_err(|e| e.to_string())?;
    p.modules.push(apex_io::vba::VbaModule::standard("Module1"));
    let bin = p.write().map_err(|e| e.to_string())?;
    wb.code_name = Some("ThisWorkbook".into());
    for (sh, name) in wb.sheets.iter_mut().zip(sheets) {
        sh.code_name = Some(name);
    }
    wb.macros.project = Some(Arc::new(bin));
    if let Some(dir) = path.parent() {
        crate::settings::make_private_dir(dir);
    }
    apex_io::save_sheet(&wb, path, apex_io::Format::Xlsx).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_personal_workbook_has_a_module() {
        let dir = apex_fs::temp_dir().join(format!("apexdesk-personal-{}", apex_fs::process_id()));
        let path = dir.join("personal.xlsm");
        new_personal(&path).unwrap();
        let (lib, macros) = load(&path, None).expect("a library");
        assert_eq!(lib.name, "personal.xlsm");
        assert!(lib.sources.iter().any(|s| s.name == "Module1"));
        assert!(macros.is_empty());
        // Unsigned: not loaded when only signed macros may run.
        assert!(load(&path, Some(&[])).is_none());
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
