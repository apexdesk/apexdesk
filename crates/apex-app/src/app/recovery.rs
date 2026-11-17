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
use crate::widgets;

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

    #[cfg(test)]
    pub(super) fn file(&self) -> &Path {
        &self.file
    }

    #[cfg(test)]
    pub(super) fn set_keep(&mut self, keep: bool) {
        self.keep = keep;
    }
}

#[cfg(test)]
pub(super) fn scan_for_test(dir: &Path) -> Vec<Found> {
    scan(dir)
}

pub(super) struct Recovery {
    dir: Option<PathBuf>,
    last: Instant,
    beat: Option<Instant>,
    /// Copies are wanted now, not at the next interval (Android: the app
    /// went to the background, where the system may end it unasked).
    pub due: bool,
    /// The window ends without asking about its changes (the system
    /// asked it to): the copies stay for the next start.
    pub keep: bool,
    /// Per tab: the change count of the copy written.
    saved: HashMap<u64, u64>,
    /// Copies being written (tab and change count), and where the writers
    /// say how it went.
    writing: HashMap<u64, u64>,
    done: (std::sync::mpsc::Sender<(u64, u64, bool)>, std::sync::mpsc::Receiver<(u64, u64, bool)>),
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

/// This run's name in the recovery folder: the process id and when it
/// started. A process id alone is used again by a later process (in a
/// sandbox ApexDesk gets the same small one each time), which would take
/// the copies of a session that crashed for its own.
fn session() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let t = SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        format!("{}_{t:x}", apex_fs::process_id())
    })
}

/// The session a recovery file belongs to (`pid_token-…`, or the
/// `pid-…` of earlier versions) and its process id.
fn session_of(stem: &str) -> Option<(&str, u32)> {
    let s = stem.split('-').next()?;
    let pid = s.split('_').next()?.parse().ok()?;
    Some((s, pid))
}

/// Whether session `s` of process `pid` (which writes heartbeat `beat`)
/// is still running.
fn alive(s: &str, pid: u32, beat: &Path) -> bool {
    if s == session() {
        return true;
    }
    // Another session with this process id has ended (or runs in another
    // sandbox, where its heartbeat says so).
    if cfg!(target_os = "linux") && pid != apex_fs::process_id() && !Path::new(&format!("/proc/{pid}")).fs_exists() {
        return false;
    }
    apex_fs::metadata(beat).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < HEARTBEAT * 3)
}

/// Removes the heartbeats and half-written copies of sessions that ended.
fn sweep(dir: &Path) {
    let Ok(rd) = apex_fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "alive" && x != "tmp") {
            continue;
        }
        let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
        let Some((s, pid)) = session_of(&stem) else { continue };
        if !alive(s, pid, &dir.join(format!("{s}.alive"))) {
            let _ = apex_fs::remove_file(&p);
        }
    }
}

/// Key and value lines of a copy's description.
fn read_info(text: &str) -> HashMap<&str, &str> {
    text.lines().filter_map(|l| l.split_once('=')).collect()
}

impl Recovery {
    pub fn new() -> Recovery {
        let dir = dir();
        let found = dir.as_deref().map(scan).unwrap_or_default();
        if let Some(d) = &dir {
            sweep(d);
        }
        Recovery { dir, last: Instant::now(), beat: None, due: false, keep: false, saved: HashMap::new(), writing: HashMap::new(), done: std::sync::mpsc::channel(), found }
    }

    fn paths(&self, key: u64) -> Option<(PathBuf, PathBuf)> {
        let d = self.dir.as_ref()?;
        let stem = format!("{}-{key}", session());
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

    /// Notes the copies written since the last call: only a copy that
    /// reached the disk counts as kept.
    fn collect(&mut self) {
        while let Ok((key, rev, ok)) = self.done.1.try_recv() {
            if self.writing.get(&key) == Some(&rev) {
                self.writing.remove(&key);
            }
            if ok {
                self.saved.insert(key, rev);
            }
        }
    }

    /// Removes this session's copies (a normal exit), also those still
    /// being written.
    pub fn clear(&mut self) {
        self.saved.clear();
        self.writing.clear();
        let Some(d) = &self.dir else { return };
        let mine = format!("{}-", session());
        let Ok(rd) = apex_fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // (The copies handed to new windows aren't this session's.)
            if name.strip_prefix(&mine).is_some_and(|rest| !rest.starts_with("move")) {
                let _ = apex_fs::remove_file(e.path());
            }
        }
        self.end_heartbeat();
    }

    /// This session's heartbeat goes: it isn't running any more.
    fn end_heartbeat(&self) {
        if let Some(d) = &self.dir {
            let _ = apex_fs::remove_file(d.join(format!("{}.alive", session())));
        }
    }
}

/// Writes copy `file` (described by `desc` in `info`) with `job`: aside
/// first and then renamed, so a crash while writing keeps the previous
/// copy. Whether it was written.
fn write_copy(dir: &Path, file: &Path, info: &Path, desc: &str, job: Box<dyn FnOnce(&Path) -> bool + Send>) -> bool {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    crate::settings::make_private_dir(dir);
    // A name of its own: an emergency copy may be written while the
    // periodic one is.
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = file.with_extension(format!("{n}.tmp"));
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&tmp))).unwrap_or(false);
    if ok && apex_fs::rename(&tmp, file).is_ok() && apex_fs::write(info, desc).is_ok() {
        return true;
    }
    let _ = apex_fs::remove_file(&tmp);
    false
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
        let Some((s, pid)) = session_of(&stem) else { continue };
        if alive(s, pid, &dir.join(format!("{s}.alive"))) {
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

/// The change count a copy of `view` is taken at (None: its changes go to
/// its file at once).
fn backup_revision(view: &View) -> Option<u64> {
    Some(match view {
        View::Sheet(v) => v.wb.revision,
        View::Doc(v) => v.backup_revision(),
        View::Slides(v) => v.pres.revision,
        View::Pdf(v) => v.doc.revision,
        View::Db(_) | View::Note(_) => return None,
    })
}

/// What the user answered in the recovery dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Answer {
    /// Open the checked copies; the others stay for next time.
    Recover,
    /// Delete the checked copies; the others stay for next time.
    Discard,
    /// Keep them all for next time.
    Later,
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
            let _ = apex_fs::write(dir.join(format!("{}.alive", session())), b"");
            self.recovery.beat = Some(Instant::now());
        }
        self.recovery.collect();
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
            let Some(rev) = backup_revision(&t.view) else { continue };
            // Kept already, or on its way (a failed copy is tried again).
            if self.recovery.saved.get(&key) == Some(&rev) || self.recovery.writing.contains_key(&key) {
                continue;
            }
            let Some((stem, info)) = self.recovery.paths(key) else { continue };
            let Some((file, job)) = copy_job(t, &stem) else { continue };
            let desc = describe(t, &file);
            self.recovery.writing.insert(key, rev);
            let dir = dir.clone();
            let done = self.recovery.done.0.clone();
            apex_fs::thread::spawn(move || {
                let ok = write_copy(&dir, &file, &info, &desc, job);
                let _ = done.send((key, rev, ok));
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

    /// The window goes away without the user having answered for its
    /// changes (the session ends, the display is gone, a signal or a
    /// crash): copies of the changed tabs are written now, waiting a few
    /// seconds at most, and left for the next start to offer. `flush`
    /// takes the macro editor's code too (not after a crash, where the
    /// less code runs the better).
    pub(super) fn keep_copies_now(&mut self, flush: bool) {
        let Some(dir) = self.recovery.dir.clone().filter(|_| interval(self.settings.autosave_minutes).is_some()) else {
            self.recovery.end_heartbeat();
            return;
        };
        if flush && let Some(tab) = self.macros.editor.as_ref().map(|e| e.tab) {
            self.flush_macros(tab);
        }
        self.recovery.collect();
        if let Some(parent) = dir.parent() {
            crate::settings::make_private_dir(parent);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let mut live = Vec::new();
        let mut started = 0;
        for t in &self.tabs {
            if t.link.is_some() && !t.holds || !t.view.modified() {
                continue;
            }
            let key = t.backup_key();
            live.push(key);
            let Some(rev) = backup_revision(&t.view) else { continue };
            if self.recovery.saved.get(&key) == Some(&rev) {
                continue;
            }
            let Some((stem, info)) = self.recovery.paths(key) else { continue };
            // A crash may have left the model half changed: what can't be
            // copied is left out.
            let Ok(Some((file, job, desc))) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| copy_job(t, &stem).map(|(f, j)| {
                let d = describe(t, &f);
                (f, j, d)
            }))) else {
                continue;
            };
            let (dir, tx) = (dir.clone(), tx.clone());
            apex_fs::thread::spawn(move || {
                let _ = tx.send(write_copy(&dir, &file, &info, &desc, job));
            });
            started += 1;
        }
        let gone: Vec<u64> = self.recovery.saved.keys().copied().filter(|k| !live.contains(k)).collect();
        for k in gone {
            self.recovery.forget(k);
        }
        let until = Instant::now() + Duration::from_secs(5);
        for _ in 0..started {
            if rx.recv_timeout(until.saturating_duration_since(Instant::now())).is_err() {
                break;
            }
        }
        self.recovery.end_heartbeat();
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
        let stem = dir.join(format!("{}-move{}", session(), t.id));
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
        let any = found.iter().any(|f| f.keep);
        let mut answer = None;
        let m = egui::Modal::new(egui::Id::new("recovery")).show(ctx, |ui| {
            ui.set_width(460.0f32.min(widgets::dialog_room(ctx).x));
            ui.heading(tr("recovery.title"));
            ui.add_space(6.0);
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
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("recovery.later")).clicked() {
                    answer = Some(Answer::Later);
                }
                if ui.add_enabled(any, egui::Button::new(tr("recovery.discard"))).clicked() {
                    answer = Some(Answer::Discard);
                }
                if ui.add_enabled(any, egui::Button::new(RichText::new(tr("recovery.recover")).strong())).clicked() {
                    answer = Some(Answer::Recover);
                }
            });
        });
        if answer.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(Answer::Later);
        }
        if let Some(a) = answer {
            self.answer_recovery(a);
        }
    }

    /// Does what the recovery dialog was answered: only the copies the
    /// user picked are opened or deleted; the rest are offered again at
    /// the next start.
    pub(super) fn answer_recovery(&mut self, answer: Answer) {
        let found = std::mem::take(&mut self.recovery.found);
        for f in found.iter().filter(|f| f.keep) {
            match answer {
                Answer::Recover => {
                    // Waiting for its password, the copy is removed once opened.
                    if self.restore(f) {
                        f.discard();
                    }
                }
                Answer::Discard => f.discard(),
                Answer::Later => {}
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
        let me = session();
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

    /// A crashed session whose process id this one got again (in a
    /// sandbox the ids are small and repeat) is not taken for this one:
    /// its copies are offered, by the old names and the new.
    #[test]
    fn same_process_id_other_session() {
        let dir = apex_fs::temp_dir().join(format!("apexdesk-recovery-pid-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&dir);
        apex_fs::create_dir_all(&dir).unwrap();
        let pid = apex_fs::process_id();
        for (stem, name) in [(format!("{pid}-2"), "Old.docx"), (format!("{pid}_1-3"), "Earlier.docx"), (format!("{}-4", session()), "Mine.docx")] {
            apex_fs::write(dir.join(format!("{stem}.docx")), b"x").unwrap();
            apex_fs::write(dir.join(format!("{stem}.info")), format!("file={stem}.docx\nname={name}\n")).unwrap();
        }
        let mut names: Vec<String> = scan(&dir).into_iter().map(|f| f.name).collect();
        names.sort();
        assert_eq!(names, ["Earlier.docx", "Old.docx"]);
        let _ = apex_fs::remove_dir_all(&dir);
    }

    /// At start the heartbeats and half-written copies of ended sessions
    /// go; this session's stay.
    #[test]
    fn ended_heartbeats_are_swept() {
        let dir = apex_fs::temp_dir().join(format!("apexdesk-recovery-sweep-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&dir);
        apex_fs::create_dir_all(&dir).unwrap();
        let dead = u32::MAX - 7;
        let names = [format!("{dead}.alive"), format!("{dead}_5.alive"), format!("{dead}_5-1.3.tmp"), format!("{}.alive", session()), format!("{dead}-2.docx")];
        for n in &names {
            apex_fs::write(dir.join(n), b"").unwrap();
        }
        sweep(&dir);
        let left: Vec<bool> = names.iter().map(|n| dir.join(n).fs_exists()).collect();
        assert_eq!(left, [false, false, false, true, true]);
        let _ = apex_fs::remove_dir_all(&dir);
    }

    /// A copy counts as kept only once it is on disk: a write that failed
    /// leaves the tab to be copied again.
    #[test]
    fn copy_kept_only_when_written() {
        let dir = apex_fs::temp_dir().join(format!("apexdesk-recovery-write-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&dir);
        apex_fs::create_dir_all(&dir).unwrap();
        let (file, info) = (dir.join("s-1.docx"), dir.join("s-1.info"));
        assert!(!write_copy(&dir, &file, &info, "file=s-1.docx\n", Box::new(|_| false)));
        assert!(!write_copy(&dir, &file, &info, "file=s-1.docx\n", Box::new(|_| panic!("broken model"))));
        assert!(!file.fs_exists() && !info.fs_exists());
        assert_eq!(apex_fs::read_dir(&dir).unwrap().count(), 0, "a half-written copy was left");
        assert!(write_copy(&dir, &file, &info, "file=s-1.docx\n", Box::new(|p| apex_fs::write(p, b"x").is_ok())));
        assert!(file.fs_exists() && info.fs_exists());
        let mut r = Recovery::new();
        r.writing.insert(1, 7);
        r.writing.insert(2, 7);
        r.done.0.send((1, 7, false)).unwrap();
        r.done.0.send((2, 7, true)).unwrap();
        r.collect();
        assert!(r.writing.is_empty());
        assert_eq!(r.saved.get(&1), None);
        assert_eq!(r.saved.get(&2), Some(&7));
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
