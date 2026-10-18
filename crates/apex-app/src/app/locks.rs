//! Files open elsewhere: the lock files written next to the open files
//! (see `lockfile`), the question asked when someone else has a file open
//! (read-only, a copy, anyway, or tell me when it's free), the read-only
//! mode, and the warning when a file changed on disk since it was opened.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use web_time::{Duration, Instant, SystemTime};

use apex_io::Loaded;
use eframe::egui::{self, RichText};

use super::{App, Notice, View};
use crate::i18n::{tr, trf};
use crate::lockfile::{self, Lock};

/// How often a file waited for is checked.
const POLL: Duration = Duration::from_secs(3);

/// How a file locked by someone else is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OpenMode {
    Normal,
    ReadOnly,
    /// As a new, untitled file.
    Copy,
    /// For editing, leaving the other's lock alone.
    Anyway,
}

/// Modification time and size of a file, to see whether someone changed it.
type Stamp = (Option<SystemTime>, u64);

fn stamp(p: &Path) -> Option<Stamp> {
    apex_fs::metadata(p).ok().map(|m| (m.modified().ok(), m.len()))
}

/// An open file.
struct Entry {
    lock: Option<Lock>,
    /// No lock of ours (opened read-only, or anyway).
    skip: bool,
    tried: bool,
    read_only: bool,
    /// As it was when opened or last saved.
    disk: Option<Stamp>,
}

struct Prompt {
    path: PathBuf,
    password: Option<String>,
    found: lockfile::Found,
}

#[derive(Default)]
pub(super) struct Locks {
    files: HashMap<PathBuf, Entry>,
    prompt: Option<Prompt>,
    /// The answer for a file being opened (kept while its password is asked).
    pending: Option<(PathBuf, OpenMode)>,
    /// Files to tell about when their lock goes, and when to look next.
    watch: Vec<(PathBuf, Instant)>,
    available: Option<PathBuf>,
    /// Tab whose file changed on disk before saving over it.
    conflict: Option<u64>,
    /// Tab allowed to save over such a file once.
    overwrite: Option<u64>,
    /// The lock files setting as last applied.
    enabled: Option<bool>,
}

/// When the lock was taken, in the interface's date and time formats.
fn when(f: &lockfile::Found) -> String {
    // LibreOffice's "26.09.2026 14:03", else the lock's time.
    let parsed = (|| {
        let (date, time) = f.owner.time.trim().split_once(' ')?;
        let mut d = date.split('.').map(|x| x.parse::<i64>().ok());
        let (day, month, year) = (d.next()??, d.next()??, d.next()??);
        let (h, m) = time.split_once(':')?;
        Some((year, month as u32, day as u32, h.parse().ok()?, m.parse().ok()?))
    })();
    match parsed.or_else(|| f.since.map(crate::common::local_time)) {
        Some(t) => {
            let (d, t) = crate::common::date_time_of(t);
            format!("{d} {t}")
        }
        None => String::new(),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub(super) fn view_of(loaded: Loaded) -> View {
    match loaded {
        Loaded::Sheet(wb) => View::Sheet(Box::new(crate::sheet_view::SheetView::new(*wb))),
        Loaded::Doc(doc) => View::Doc(Box::new(crate::doc_view::DocView::new(*doc))),
        Loaded::Slides(d) => View::Slides(Box::new(crate::slides_view::SlidesView::new(*d))),
    }
}

impl View {
    pub(super) fn read_only(&self) -> bool {
        match self {
            View::Sheet(v) => v.wb.read_only,
            View::Doc(v) => v.read_only,
            View::Slides(v) => v.read_only,
            View::Pdf(v) => v.read_only,
            View::Db(v) => v.read_only,
            View::Note(v) => v.read_only,
        }
    }

    pub(super) fn set_read_only(&mut self, on: bool) {
        match self {
            View::Sheet(v) => v.wb.read_only = on,
            View::Doc(v) => v.read_only = on,
            View::Slides(v) => v.read_only = on,
            View::Pdf(v) => v.read_only = on,
            View::Db(v) => v.read_only = on,
            View::Note(v) => v.read_only = on,
        }
    }
}

impl App {
    fn lock_owner(&self) -> lockfile::Owner {
        lockfile::me(&crate::doc_view::user_name())
    }

    /// Before opening `path`: how to open it, or None after asking the user
    /// because someone else has it open.
    pub(super) fn lock_gate(&mut self, path: &Path, password: &Option<String>) -> Option<OpenMode> {
        if let Some((p, m)) = self.locks.pending.take() {
            if p == path {
                return Some(m);
            }
        }
        if !self.settings.lock_files || apex_io::is_template(path) {
            return Some(OpenMode::Normal);
        }
        match lockfile::check(path, &self.lock_owner()) {
            lockfile::State::Other(found) => {
                self.locks.prompt = Some(Prompt { path: path.to_path_buf(), password: password.clone(), found });
                None
            }
            _ => Some(OpenMode::Normal),
        }
    }

    /// Keeps the answer while the password of `path` is asked.
    pub(super) fn keep_open_mode(&mut self, path: &Path, mode: OpenMode) {
        if mode != OpenMode::Normal {
            self.locks.pending = Some((path.to_path_buf(), mode));
        }
    }

    /// A file just opened in the last tab.
    pub(super) fn opened(&mut self, path: &Path, mode: OpenMode) {
        // An e-mail message is only read (exported, printed): no lock.
        let mode = if apex_io::Format::from_path(path) == Some(apex_io::Format::Mail) { OpenMode::ReadOnly } else { mode };
        let read_only = mode == OpenMode::ReadOnly;
        self.locks.files.insert(path.to_path_buf(), Entry { lock: None, skip: mode != OpenMode::Normal, tried: false, read_only, disk: stamp(path) });
        if read_only && let Some(t) = self.tabs.last_mut() {
            t.view.set_read_only(true);
        }
    }

    /// A file shown only in part (past `apex_doc::MAX_PARAGRAPHS`): saving
    /// it over itself would lose the rest, so it opens read-only; saved
    /// elsewhere it is the user's own again.
    pub(super) fn opened_in_part(&mut self, path: &Path) {
        if let Some(e) = self.locks.files.get_mut(path) {
            e.read_only = true;
        }
        let n = apex_doc::MAX_PARAGRAPHS.to_string();
        self.notify(Notice::error(trf("notice.truncated", &[("n", &n)])));
    }

    /// A file opened read-only (as its author recommends) is edited after
    /// all: it takes its lock like any other.
    pub(super) fn edit_after_all(&mut self, path: &Path) {
        if let Some(e) = self.locks.files.get_mut(path) {
            e.read_only = false;
            e.skip = false;
            e.tried = false;
        }
    }

    /// Opened as a copy: an untitled tab named after the file.
    pub(super) fn opened_copy(&mut self, path: &Path) {
        if let Some(t) = self.tabs.last_mut() {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            t.untitled = trf("lock.copy_name", &[("name", &stem)]);
        }
    }

    /// Before saving tab `idx` over `path`: false (and asks what to do) when
    /// someone changed the file since it was opened or saved.
    pub(super) fn may_overwrite(&mut self, idx: usize, path: &Path) -> bool {
        let id = self.tabs[idx].id;
        if self.tabs[idx].path.as_deref() != Some(path) || self.locks.overwrite.take() == Some(id) {
            return true;
        }
        let Some(e) = self.locks.files.get(path) else { return true };
        let now = stamp(path);
        // A file removed meanwhile is simply written again.
        if e.disk.is_none() || now.is_none() || now == e.disk {
            return true;
        }
        self.locks.conflict = Some(id);
        false
    }

    /// Whether the file at `path` is as it was when opened or last saved
    /// here (true when it isn't followed).
    pub(super) fn unchanged_on_disk(&self, path: &Path) -> bool {
        self.locks.files.get(path).is_none_or(|e| e.disk.is_some() && stamp(path) == e.disk)
    }

    /// Tab `idx` was saved to `path`.
    pub(super) fn saved_file(&mut self, idx: usize, path: &Path) {
        let e = self.locks.files.entry(path.to_path_buf()).or_insert(Entry { lock: None, skip: false, tried: false, read_only: false, disk: None });
        e.disk = stamp(path);
        // A read-only file saved elsewhere is the user's own.
        e.read_only = false;
        if let Some(t) = self.tabs.get_mut(idx) {
            t.view.set_read_only(false);
        }
    }

    /// Takes and releases locks as tabs open, close and change files, and
    /// applies the read-only mode to the views; call once a frame.
    pub(super) fn sync_locks(&mut self) {
        let on = self.settings.lock_files;
        if self.locks.enabled != Some(on) {
            self.locks.enabled = Some(on);
            for e in self.locks.files.values_mut() {
                e.tried = false;
                if !on {
                    e.lock = None;
                }
            }
        }
        let paths: Vec<PathBuf> = self.tabs.iter().filter_map(|t| t.path.clone()).collect();
        self.locks.files.retain(|p, _| paths.contains(p));
        let tabs = &self.tabs;
        self.protected.tabs.retain(|id| tabs.iter().any(|t| t.id == *id));
        let mut owner = None;
        for t in &mut self.tabs {
            let Some(p) = &t.path else { continue };
            let e = self.locks.files.entry(p.clone()).or_insert_with(|| Entry { lock: None, skip: false, tried: false, read_only: false, disk: stamp(p) });
            // Protected View: read-only, and no lock taken until editing.
            let protected = self.protected.tabs.contains(&t.id);
            if t.view.read_only() != (e.read_only || protected) {
                t.view.set_read_only(e.read_only || protected);
            }
            if on && !protected && !e.tried && !e.skip && e.lock.is_none() {
                e.tried = true;
                if lockfile::lockable(p) {
                    let me = owner.get_or_insert_with(|| lockfile::me(&crate::doc_view::user_name()));
                    // A folder that can't be written simply gets no lock.
                    e.lock = lockfile::acquire(p, me).ok();
                }
            }
        }
    }

    /// Removes this session's locks (on exit).
    pub(super) fn release_locks(&mut self) {
        self.locks.files.clear();
    }

    pub(super) fn lock_dialogs(&mut self, ctx: &egui::Context) {
        self.locked_dialog(ctx);
        self.watch_locks(ctx);
        self.available_dialog(ctx);
        self.conflict_dialog(ctx);
    }

    fn locked_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.locks.prompt else { return };
        let name = file_name(&p.path);
        let who = p.found.owner.display_name().to_string();
        let since = when(&p.found);
        let mut choice = None;
        egui::Modal::new(egui::Id::new("file_locked")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading(tr("lock.title"));
            ui.add_space(6.0);
            let body = match (who.is_empty(), since.is_empty()) {
                (true, _) => trf("lock.body_unknown", &[("file", &name)]),
                (false, true) => trf("lock.body_user", &[("file", &name), ("user", &who)]),
                (false, false) => trf("lock.body", &[("file", &name), ("user", &who), ("since", &since)]),
            };
            ui.label(body);
            ui.add_space(4.0);
            ui.label(RichText::new(tr("lock.hint")).weak());
            ui.add_space(10.0);
            for (mode, key, tip) in [
                (Some(OpenMode::ReadOnly), "lock.read_only_open", "lock.read_only_tip"),
                (Some(OpenMode::Copy), "lock.copy", "lock.copy_tip"),
                (None, "lock.notify", "lock.notify_tip"),
                (Some(OpenMode::Anyway), "lock.anyway", "lock.anyway_tip"),
            ] {
                let b = egui::Button::new(tr(key)).min_size(egui::vec2(ui.available_width(), 0.0));
                if ui.add(b).on_hover_text(tr(tip)).clicked() {
                    choice = Some((mode, true));
                }
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("unsaved.cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    choice = Some((None, false));
                }
            });
        });
        let Some((mode, go)) = choice else { return };
        let Some(p) = self.locks.prompt.take() else { return };
        if !go {
            return;
        }
        // Notify: open read-only now, and say when it's free.
        let mode = mode.unwrap_or_else(|| {
            self.locks.watch.push((p.path.clone(), Instant::now() + POLL));
            OpenMode::ReadOnly
        });
        self.locks.pending = Some((p.path.clone(), mode));
        self.open_path_with(&p.path, p.password);
    }

    fn watch_locks(&mut self, ctx: &egui::Context) {
        if self.locks.watch.is_empty() {
            return;
        }
        ctx.request_repaint_after(POLL);
        let now = Instant::now();
        let me = self.lock_owner();
        let mut freed = Vec::new();
        self.locks.watch.retain_mut(|(p, next)| {
            if *next > now {
                return true;
            }
            *next = now + POLL;
            let free = !matches!(lockfile::check(p, &me), lockfile::State::Other(_));
            if free {
                freed.push(p.clone());
            }
            !free
        });
        // Only files still open read-only are offered.
        for p in freed {
            let open_ro = self.tabs.iter().any(|t| t.path.as_deref() == Some(p.as_path()) && t.view.read_only());
            if open_ro && self.locks.available.is_none() {
                self.notify(Notice::info(trf("lock.available_notice", &[("file", &file_name(&p))])));
                self.locks.available = Some(p);
            }
        }
    }

    fn available_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.locks.available else { return };
        let name = file_name(p);
        let mut choice = None;
        egui::Modal::new(egui::Id::new("file_available")).show(ctx, |ui| {
            ui.set_max_width(400.0);
            ui.heading(tr("lock.available_title"));
            ui.add_space(6.0);
            ui.label(trf("lock.available_body", &[("file", &name)]));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(tr("lock.edit")).strong()).clicked() {
                    choice = Some(true);
                }
                if ui.button(tr("dialog.close")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    choice = Some(false);
                }
            });
        });
        let Some(edit) = choice else { return };
        let Some(p) = self.locks.available.take() else { return };
        if edit {
            self.reopen_for_editing(&p);
        }
    }

    /// Opens a read-only tab's file again, for editing (it may have
    /// changed while someone else had it).
    fn reopen_for_editing(&mut self, path: &Path) {
        let Some(idx) = self.tabs.iter().position(|t| t.path.as_deref() == Some(path)) else { return };
        if matches!(lockfile::check(path, &self.lock_owner()), lockfile::State::Other(_)) {
            self.notify(Notice::error(trf("lock.taken_again", &[("file", &file_name(path))])));
            return;
        }
        // Windows of a workbook share the view: those just lose read-only.
        if self.tabs[idx].link.is_none() {
            let pw = self.tabs[idx].password.clone();
            match apex_io::open_with(path, pw.as_deref()) {
                Ok((loaded, _)) => {
                    self.tabs[idx].view = view_of(loaded);
                    let id = self.tabs[idx].id;
                    self.macro_run.forget(id);
                }
                Err(e) => {
                    self.notify(Notice::error(trf("error.open", &[("file", &file_name(path)), ("error", &e)])));
                    return;
                }
            }
        }
        if let Some(e) = self.locks.files.get_mut(path) {
            e.read_only = false;
            e.skip = false;
            e.tried = false;
            e.disk = stamp(path);
        }
        self.notify(Notice::info(tr("lock.now_editing")));
    }

    fn conflict_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.locks.conflict else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else {
            self.locks.conflict = None;
            return;
        };
        let name = self.tabs[idx].name();
        let mut choice = None;
        egui::Modal::new(egui::Id::new("file_changed")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(tr("lock.changed_title"));
            ui.add_space(6.0);
            ui.label(trf("lock.changed_body", &[("file", &name)]));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(tr("lock.overwrite")).clicked() {
                    choice = Some(0);
                }
                if ui.button(RichText::new(tr("lock.save_as")).strong()).clicked() {
                    choice = Some(1);
                }
                if ui.button(tr("unsaved.cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    choice = Some(2);
                }
            });
        });
        let Some(c) = choice else { return };
        self.locks.conflict = None;
        match c {
            0 => {
                self.locks.overwrite = Some(id);
                self.save(idx, false);
            }
            1 => {
                self.save(idx, true);
            }
            _ => {}
        }
    }

    /// The read-only mark of the status bar.
    pub(super) fn read_only_badge(ui: &mut egui::Ui) {
        ui.separator();
        ui.label(RichText::new(tr("lock.read_only")).strong().color(crate::theme::ACCENT)).on_hover_text(tr("lock.read_only_hint"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_times() {
        let f = lockfile::Found { owner: lockfile::Owner { time: "05.03.2026 09:07".into(), ..Default::default() }, since: None, office: false };
        let s = when(&f);
        assert!(s.contains("2026") && s.contains("09:07") || s.contains("9:07"), "{s}");
        let f = lockfile::Found { owner: Default::default(), since: None, office: true };
        assert_eq!(when(&f), "");
    }
}
