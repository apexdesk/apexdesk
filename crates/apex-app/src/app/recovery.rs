//! Autosave and recovery: now and then, copies of the tabs with unsaved
//! changes go to the recovery folder (in the background); a normal exit
//! removes them, so the next start offers what a crash left behind.

use apex_fs::PathExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use web_time::{Duration, Instant, SystemTime};

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
    /// The encoding a text file is saved in, and whether it came from the Internet.
    encoding: Option<apex_io::Encoding>,
    downloaded: bool,
}

impl Found {
    /// Removes the copy (once restored).
    pub(super) fn discard(&self) {
        let _ = apex_fs::remove_file(&self.file);
        let _ = apex_fs::remove_file(&self.info);
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

/// Set for a window of its own made from a tab of another one: the
/// description of the copy of that tab it opens (see [`App::hand_off`]).
pub(super) const HANDOFF: &str = "APEXDESK_HANDOFF";

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
    if pid == apex_fs::process_id() {
        return true;
    }
    if cfg!(target_os = "linux") && !Path::new(&format!("/proc/{pid}")).fs_exists() {
        return false;
    }
    apex_fs::metadata(beat).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < HEARTBEAT * 3)
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
        let stem = format!("{}-{key}", apex_fs::process_id());
        Some((d.join(&stem), d.join(format!("{stem}.info"))))
    }

    /// Removes the copy of tab `key`.
    fn forget(&mut self, key: u64) {
        self.saved.remove(&key);
        if let Some((stem, info)) = self.paths(key) {
            for ext in ["docx", "docm", "xlsx", "xlsm", "pptx", "pptm", "pdf"] {
                let _ = apex_fs::remove_file(stem.with_extension(ext));
            }
            let _ = apex_fs::remove_file(info);
        }
    }

    /// Removes this session's copies (a normal exit).
    pub fn clear(&mut self) {
        let keys: Vec<u64> = self.saved.keys().copied().collect();
        // (The copies handed to new windows aren't this session's.)
        for k in keys {
            self.forget(k);
        }
        if let Some(d) = &self.dir {
            let _ = apex_fs::remove_file(d.join(format!("{}.alive", apex_fs::process_id())));
        }
    }
}

/// Copies left by processes that aren't running any more.
fn scan(dir: &Path) -> Vec<Found> {
    let Ok(rd) = apex_fs::read_dir(dir) else { return Vec::new() };
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
        match read_found(dir, &info) {
            Some(f) => out.push(f),
            // A description without its copy is dropped.
            None => {
                let _ = apex_fs::remove_file(&info);
            }
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.time));
    out
}

/// The copy described by `info` (in `dir`), when it's there.
fn read_found(dir: &Path, info: &Path) -> Option<Found> {
    let stem = info.file_stem()?.to_string_lossy().into_owned();
    let text = apex_fs::read_to_string(info).ok()?;
    let kv = read_info(&text);
    let file = kv.get("file").map(|f| dir.join(f)).filter(|f| f.fs_exists())?;
    let original = kv.get("path").filter(|p| !p.is_empty()).zip(kv.get("format").and_then(|f| Format::from_path(Path::new(&format!("x.{f}"))))).map(|(p, f)| (PathBuf::from(p), f));
    let name = kv.get("name").map_or_else(|| stem.clone(), |n| n.to_string());
    let time = apex_fs::metadata(&file).and_then(|m| m.modified()).ok();
    let encoding = kv.get("encoding").and_then(|e| apex_io::Encoding::ALL.into_iter().find(|x| x.name() == *e));
    let downloaded = kv.get("downloaded") == Some(&"1");
    Some(Found { file, info: info.to_path_buf(), original, name, time, keep: true, encoding, downloaded })
}

/// The job that writes a copy of tab `t` named `stem` (with the
/// extension its contents need, also returned); None for tabs whose
/// changes go to their file at once.
fn copy_job(t: &Tab, stem: &Path) -> Option<(PathBuf, Box<dyn FnOnce(&Path) -> bool + Send>)> {
    // Copies of encrypted files are encrypted too.
    let pw = t.password.clone();
    Some(match &t.view {
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
        View::Db(_) | View::Note(_) => return None,
        View::Pdf(v) => {
            // The file with the changes added, as saving writes it.
            let bytes = v.doc.to_bytes();
            (stem.with_extension("pdf"), Box::new(move |p: &Path| apex_fs::write(p, bytes).is_ok()))
        }
    })
}

/// The description of `file`, a copy of tab `t`.
fn describe(t: &Tab, file: &Path) -> String {
    format!(
        "file={}\nname={}\npath={}\nformat={}\nencoding={}\ndownloaded={}\n",
        file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        t.name().replace('\n', " "),
        t.path.as_ref().map(|p| p.to_string_lossy().replace('\n', " ")).unwrap_or_default(),
        t.format.map(|f| f.extension()).unwrap_or_default(),
        t.encoding.map(|e| e.name()).unwrap_or_default(),
        u8::from(t.downloaded()),
    )
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
            let _ = apex_fs::write(dir.join(format!("{}.alive", apex_fs::process_id())), b"");
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
            let Some((file, job)) = copy_job(t, &stem) else { continue };
            let desc = describe(t, &file);
            self.recovery.saved.insert(key, rev);
            let dir = dir.clone();
            // Written aside and renamed, so a crash while writing keeps the
            // previous copy.
            apex_fs::thread::spawn(move || {
                crate::settings::make_private_dir(&dir);
                let tmp = file.with_extension("tmp");
                if job(&tmp) && apex_fs::rename(&tmp, &file).is_ok() {
                    let _ = apex_fs::write(&info, desc);
                } else {
                    let _ = apex_fs::remove_file(&tmp);
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

    /// Tab `idx` changed its file or name: its copy is written again (the
    /// description says them).
    pub(super) fn copy_renamed(&mut self, idx: usize) {
        if let Some(key) = self.tabs.get(idx).map(Tab::backup_key) {
            self.recovery.saved.remove(&key);
        }
    }

    /// Writes a copy of tab `idx` as it is now, for a new window to open
    /// (with its changes, as a recovery copy); returns its description.
    /// The copy is left by this process, so if the new window never
    /// takes it, it is offered at the next start once this one ends.
    pub(super) fn hand_off(&mut self, idx: usize) -> Option<PathBuf> {
        let dir = self.recovery.dir.clone()?;
        if let Some(id) = self.tabs.get(idx).map(|t| t.id) {
            self.flush_macros(id);
        }
        self.hold(idx);
        let t = self.tabs.get(idx)?;
        if let Some(parent) = dir.parent() {
            crate::settings::make_private_dir(parent);
        }
        crate::settings::make_private_dir(&dir);
        let stem = dir.join(format!("{}-move{}", apex_fs::process_id(), t.id));
        let (file, job) = copy_job(t, &stem)?;
        let info = stem.with_extension("info");
        if !job(&file) || apex_fs::write(&info, describe(t, &file)).is_err() {
            let _ = apex_fs::remove_file(&file);
            return None;
        }
        Some(info)
    }

    /// Opens the copy a window handed over (its description `info`), as a
    /// changed tab, and removes it.
    pub(super) fn adopt_handoff(&mut self, info: &Path) {
        let Some(f) = info.parent().and_then(|d| read_found(d, info)) else { return };
        // Waiting for its password, the copy is removed once opened.
        if self.restore(&f) {
            f.discard();
        }
    }

    /// Opens a copy (encrypted ones with `password`); false when it
    /// couldn't, or when it waits for a password.
    pub(super) fn restore_with(&mut self, f: &Found, password: Option<&str>) -> bool {
        // PDF copies open in a PDF tab.
        let pdf = f.file.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        let opened = if pdf {
            apex_fs::read(&f.file).map_err(|e| apex_io::Error(e.to_string())).and_then(|d| crate::pdf_view::PdfView::open(d, password.map(str::to_string))).map(|mut v| {
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
                    t.encoding = f.encoding;
                    t.downloaded = f.downloaded;
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
                let _ = apex_fs::remove_file(&f.file);
                let _ = apex_fs::remove_file(&f.info);
            }
        }
        // The heartbeats of ended sessions.
        if let Some(d) = &self.recovery.dir {
            if let Ok(rd) = apex_fs::read_dir(d) {
                for e in rd.flatten() {
                    let p = e.path();
                    let pid = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
                    if p.extension().is_some_and(|x| x == "alive") && pid.is_some_and(|pid| !alive(pid, &p)) {
                        let _ = apex_fs::remove_file(p);
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
        let dir = apex_fs::temp_dir().join(format!("apexdesk-recovery-{}", apex_fs::process_id()));
        let _ = apex_fs::create_dir_all(&dir);
        // A session that ended (no such process) and this one.
        let dead = u32::MAX - 7;
        apex_fs::write(dir.join(format!("{dead}-3.docx")), b"x").unwrap();
        apex_fs::write(dir.join(format!("{dead}-3.info")), format!("file={dead}-3.docx\nname=Informe.docx\npath=/tmp/Informe.docx\nformat=docx\n")).unwrap();
        let me = apex_fs::process_id();
        apex_fs::write(dir.join(format!("{me}-1.xlsx")), b"x").unwrap();
        apex_fs::write(dir.join(format!("{me}-1.info")), format!("file={me}-1.xlsx\nname=Libro 1\npath=\nformat=\n")).unwrap();
        // A description without its copy is dropped.
        apex_fs::write(dir.join(format!("{dead}-4.info")), format!("file={dead}-4.docx\nname=x\n")).unwrap();
        let found = scan(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Informe.docx");
        assert_eq!(found[0].original, Some((PathBuf::from("/tmp/Informe.docx"), Format::Docx)));
        assert!(!dir.join(format!("{dead}-4.info")).fs_exists());
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
