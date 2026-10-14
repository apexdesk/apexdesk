//! Autosave and recovery: now and then, copies of the tabs with unsaved
//! changes go to the recovery folder (in the background); a normal exit
//! removes them, so the next start offers what a crash left behind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use apex_io::{Format, Loaded};
use eframe::egui::{self, RichText};

use super::{App, Tab, View};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

/// How often the heartbeat of a running ApexDesk is written; a process
/// whose heartbeat is older than a few of these has ended.
const HEARTBEAT: Duration = Duration::from_secs(60);

/// A copy left by a session that ended without closing.
#[derive(Clone)]
pub(super) struct Found {
    file: PathBuf,
    info: PathBuf,
    /// The file it was a copy of, and its format.
    original: Option<(PathBuf, Format)>,
    pub(super) name: String,
    time: Option<SystemTime>,
    keep: bool,
}

impl Found {
    /// Removes the copy (once restored).
    pub(super) fn discard(&self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_file(&self.info);
    }
}

pub(super) struct Recovery {
    dir: Option<PathBuf>,
    last: Instant,
    beat: Option<Instant>,
    /// Copies are wanted now, not at the next interval (Android: the app
    /// went to the background, where the system may end it unasked).
    pub due: bool,
    /// Per tab: the change count of the copy written.
    saved: HashMap<u64, u64>,
    /// Copies of an earlier session, offered at start.
    pub found: Vec<Found>,
}

fn dir() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("recovery"))
}

/// Interval between copies: the setting, or `APEXDESK_AUTOSAVE_SECS`
/// (for checks).
fn interval(minutes: u32) -> Option<Duration> {
    if let Some(s) = std::env::var("APEXDESK_AUTOSAVE_SECS").ok().and_then(|v| v.parse::<u64>().ok()) {
        return Some(Duration::from_secs(s.max(1)));
    }
    (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
}

/// Whether process `pid` (which wrote heartbeat `beat`) is still running.
fn alive(pid: u32, beat: &Path) -> bool {
    if pid == std::process::id() {
        return true;
    }
    if cfg!(target_os = "linux") && !Path::new(&format!("/proc/{pid}")).exists() {
        return false;
    }
    std::fs::metadata(beat).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < HEARTBEAT * 3)
}

/// Key and value lines of a copy's description.
fn read_info(text: &str) -> HashMap<&str, &str> {
    text.lines().filter_map(|l| l.split_once('=')).collect()
}

impl Recovery {
    pub fn new() -> Recovery {
        let dir = dir();
        let found = dir.as_deref().map(scan).unwrap_or_default();
        Recovery { dir, last: Instant::now(), beat: None, due: false, saved: HashMap::new(), found }
    }

    fn paths(&self, key: u64) -> Option<(PathBuf, PathBuf)> {
        let d = self.dir.as_ref()?;
        let stem = format!("{}-{key}", std::process::id());
        Some((d.join(&stem), d.join(format!("{stem}.info"))))
    }

    /// Removes the copy of tab `key`.
    fn forget(&mut self, key: u64) {
        self.saved.remove(&key);
        if let Some((stem, info)) = self.paths(key) {
            for ext in ["docx", "docm", "xlsx", "xlsm", "pptx", "pptm", "pdf"] {
                let _ = std::fs::remove_file(stem.with_extension(ext));
            }
            let _ = std::fs::remove_file(info);
        }
    }

    /// Removes this session's copies (a normal exit).
    pub fn clear(&mut self) {
        let keys: Vec<u64> = self.saved.keys().copied().collect();
        for k in keys {
            self.forget(k);
        }
        if let Some(d) = &self.dir {
            let _ = std::fs::remove_file(d.join(format!("{}.alive", std::process::id())));
        }
    }
}

/// Copies left by processes that aren't running any more.
fn scan(dir: &Path) -> Vec<Found> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let info = e.path();
        if info.extension().is_none_or(|x| x != "info") {
            continue;
        }
        let Some(stem) = info.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
        let Some(pid) = stem.split('-').next().and_then(|p| p.parse::<u32>().ok()) else { continue };
        if alive(pid, &dir.join(format!("{pid}.alive"))) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&info) else { continue };
        let kv = read_info(&text);
        let Some(file) = kv.get("file").map(|f| dir.join(f)).filter(|f| f.exists()) else {
            let _ = std::fs::remove_file(&info);
            continue;
        };
        let original = kv.get("path").filter(|p| !p.is_empty()).zip(kv.get("format").and_then(|f| Format::from_path(Path::new(&format!("x.{f}"))))).map(|(p, f)| (PathBuf::from(p), f));
        let name = kv.get("name").map_or_else(|| stem.clone(), |n| n.to_string());
        let time = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        out.push(Found { file, info, original, name, time, keep: true });
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.time));
    out
}

/// "5 min ago" style age of a copy.
fn age(t: Option<SystemTime>) -> String {
    let secs = t.and_then(|t| t.elapsed().ok()).map_or(0, |d| d.as_secs());
    match secs {
        0..=59 => tr("recovery.just_now").to_string(),
        60..=3599 => trf("recovery.minutes_ago", &[("n", &(secs / 60))]),
        3600..=86_399 => trf("recovery.hours_ago", &[("n", &(secs / 3600))]),
        _ => trf("recovery.days_ago", &[("n", &(secs / 86_400))]),
    }
}

impl Tab {
    /// The key of the copy: linked windows share one.
    fn backup_key(&self) -> u64 {
        self.link.unwrap_or(self.id)
    }
}

impl App {
    /// Writes copies of the changed tabs when it's time, and removes those
    /// of tabs saved or closed since.
    pub(super) fn autosave(&mut self, ctx: &egui::Context) {
        let Some(every) = interval(self.settings.autosave_minutes) else { return };
        ctx.request_repaint_after(every.min(HEARTBEAT));
        let Some(dir) = self.recovery.dir.clone() else { return };
        if self.recovery.beat.is_none_or(|b| b.elapsed() >= HEARTBEAT) {
            if let Some(parent) = dir.parent() {
                crate::settings::make_private_dir(parent);
            }
            crate::settings::make_private_dir(&dir);
            let _ = std::fs::write(dir.join(format!("{}.alive", std::process::id())), b"");
            self.recovery.beat = Some(Instant::now());
        }
        if !self.recovery.due && self.recovery.last.elapsed() < every {
            return;
        }
        self.recovery.due = false;
        self.recovery.last = Instant::now();
        // Code typed in the macro editor goes into the copy too.
        if let Some(tab) = self.macros.editor.as_ref().map(|e| e.tab) {
            self.flush_macros(tab);
        }
        let mut live = Vec::new();
        for t in &self.tabs {
            // The other windows of a workbook show a placeholder.
            if t.link.is_some() && !t.holds || !t.view.modified() {
                continue;
            }
            let key = t.backup_key();
            live.push(key);
            let rev = match &t.view {
                View::Sheet(v) => v.wb.revision,
                View::Doc(v) => v.backup_revision(),
                View::Slides(v) => v.pres.revision,
                View::Pdf(v) => v.doc.revision,
                // Changes go to the database file at once.
                View::Db(_) | View::Note(_) => continue,
            };
            if self.recovery.saved.get(&key) == Some(&rev) {
                continue;
            }
            let Some((stem, info)) = self.recovery.paths(key) else { continue };
            // Copies of encrypted files are encrypted too.
            let pw = t.password.clone();
            let (file, job): (PathBuf, Box<dyn FnOnce(&Path) -> bool + Send>) = match &t.view {
                View::Sheet(v) => {
                    let wb = v.wb.clone();
                    // Macro-enabled when there are macros, so they are kept.
                    let ext = if wb.macros.project.is_some() { "xlsm" } else { "xlsx" };
                    (stem.with_extension(ext), Box::new(move |p: &Path| apex_io::save_sheet_with(&wb, p, Format::Xlsx, pw.as_deref()).is_ok()))
                }
                View::Doc(v) => {
                    let doc = v.backup_doc();
                    let ext = if doc.macros.project.is_some() { "docm" } else { "docx" };
                    (stem.with_extension(ext), Box::new(move |p: &Path| apex_io::save_doc_with(&doc, p, Format::Docx, pw.as_deref()).is_ok()))
                }
                View::Slides(v) => {
                    let deck = v.pres.deck.clone();
                    let ext = if deck.macros.project.is_some() { "pptm" } else { "pptx" };
                    (stem.with_extension(ext), Box::new(move |p: &Path| apex_io::save_slides_with(&deck, p, Format::Pptx, pw.as_deref()).is_ok()))
                }
                View::Db(_) | View::Note(_) => continue,
                View::Pdf(v) => {
                    // The file with the changes added, as saving writes it.
                    let bytes = v.doc.to_bytes();
                    (stem.with_extension("pdf"), Box::new(move |p: &Path| std::fs::write(p, bytes).is_ok()))
                }
            };
            let desc = format!(
                "file={}\nname={}\npath={}\nformat={}\n",
                file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                t.name().replace('\n', " "),
                t.path.as_ref().map(|p| p.to_string_lossy().replace('\n', " ")).unwrap_or_default(),
                t.format.map(|f| f.extension()).unwrap_or_default()
            );
            self.recovery.saved.insert(key, rev);
            let dir = dir.clone();
            // Written aside and renamed, so a crash while writing keeps the
            // previous copy.
            std::thread::spawn(move || {
                crate::settings::make_private_dir(&dir);
                let tmp = file.with_extension("tmp");
                if job(&tmp) && std::fs::rename(&tmp, &file).is_ok() {
                    let _ = std::fs::write(&info, desc);
                } else {
                    let _ = std::fs::remove_file(&tmp);
                }
            });
        }
        let gone: Vec<u64> = self.recovery.saved.keys().copied().filter(|k| !live.contains(k)).collect();
        for k in gone {
            self.recovery.forget(k);
        }
    }

    /// Opens a copy as a changed tab of the file it came from.
    fn restore(&mut self, f: &Found) -> bool {
        self.restore_with(f, None)
    }

    /// Opens a copy (encrypted ones with `password`); false when it
    /// couldn't, or when it waits for a password.
    pub(super) fn restore_with(&mut self, f: &Found, password: Option<&str>) -> bool {
        // PDF copies open in a PDF tab.
        let pdf = f.file.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        let opened = if pdf {
            std::fs::read(&f.file).map_err(|e| apex_io::Error(e.to_string())).and_then(|d| crate::pdf_view::PdfView::open(d, password.map(str::to_string))).map(|mut v| {
                v.doc.mark_unsaved();
                Err(View::Pdf(Box::new(v)))
            })
        } else {
            apex_io::open_with(&f.file, password).map(|(l, _)| Ok(l))
        };
        match opened {
            Ok(loaded) => {
                let view = match loaded {
                    Err(v) => v,
                    Ok(loaded) => match loaded {
                    Loaded::Sheet(mut wb) => {
                        wb.modified = true;
                        View::Sheet(Box::new(crate::sheet_view::SheetView::new(*wb)))
                    }
                    Loaded::Doc(mut doc) => {
                        doc.modified = true;
                        View::Doc(Box::new(crate::doc_view::DocView::new(*doc)))
                    }
                    Loaded::Slides(d) => {
                        let mut v = crate::slides_view::SlidesView::new(*d);
                        v.pres.modified = true;
                        View::Slides(Box::new(v))
                    }
                    },
                };
                let (path, format) = f.original.clone().map_or((None, None), |(p, fm)| (Some(p), Some(fm)));
                let untitled = path.is_none();
                self.push_tab(view, path, format);
                if let Some(t) = self.tabs.last_mut() {
                    if untitled {
                        t.untitled.clone_from(&f.name);
                    }
                    t.password = password.map(str::to_string);
                }
                // A source code file shows as code again.
                if let Some((p, _)) = &f.original {
                    self.note_source(self.tabs.len() - 1, p, false);
                }
                true
            }
            Err(e) if e.needs_password() || e.wrong_password() => {
                self.ask_password(f.file.clone(), Some(f.clone()), e.wrong_password());
                false
            }
            Err(e) => {
                self.notify(super::Notice::error(trf("error.open", &[("file", &f.name), ("error", &e)])));
                false
            }
        }
    }

    /// The documents recovered after a crash: open them or discard them.
    pub(super) fn recovery_dialog(&mut self, ctx: &egui::Context) {
        if self.recovery.found.is_empty() {
            return;
        }
        let found = &mut self.recovery.found;
        let res = widgets::dialog(ctx, "recovery", tr("recovery.title"), 460.0, |ui| {
            ui.label(tr("recovery.intro"));
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for f in found.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut f.keep, RichText::new(&f.name).strong());
                        ui.label(RichText::new(age(f.time)).weak());
                    });
                    if let Some((p, _)) = &f.original {
                        ui.indent(&f.file, |ui| ui.label(RichText::new(p.to_string_lossy()).small().weak()));
                    }
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new(tr("recovery.hint")).weak());
        });
        if res == DialogResult::None {
            return;
        }
        let found = std::mem::take(&mut self.recovery.found);
        if res == DialogResult::Cancel {
            // Offered again next time.
            return;
        }
        for f in &found {
            // Unchecked copies are discarded.
            if !f.keep || self.restore(f) {
                let _ = std::fs::remove_file(&f.file);
                let _ = std::fs::remove_file(&f.info);
            }
        }
        // The heartbeats of ended sessions.
        if let Some(d) = &self.recovery.dir {
            if let Ok(rd) = std::fs::read_dir(d) {
                for e in rd.flatten() {
                    let p = e.path();
                    let pid = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
                    if p.extension().is_some_and(|x| x == "alive") && pid.is_some_and(|pid| !alive(pid, &p)) {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_of_ended_sessions() {
        let dir = std::env::temp_dir().join(format!("apexdesk-recovery-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // A session that ended (no such process) and this one.
        let dead = u32::MAX - 7;
        std::fs::write(dir.join(format!("{dead}-3.docx")), b"x").unwrap();
        std::fs::write(dir.join(format!("{dead}-3.info")), format!("file={dead}-3.docx\nname=Informe.docx\npath=/tmp/Informe.docx\nformat=docx\n")).unwrap();
        let me = std::process::id();
        std::fs::write(dir.join(format!("{me}-1.xlsx")), b"x").unwrap();
        std::fs::write(dir.join(format!("{me}-1.info")), format!("file={me}-1.xlsx\nname=Libro 1\npath=\nformat=\n")).unwrap();
        // A description without its copy is dropped.
        std::fs::write(dir.join(format!("{dead}-4.info")), format!("file={dead}-4.docx\nname=x\n")).unwrap();
        let found = scan(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Informe.docx");
        assert_eq!(found[0].original, Some((PathBuf::from("/tmp/Informe.docx"), Format::Docx)));
        assert!(!dir.join(format!("{dead}-4.info")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
