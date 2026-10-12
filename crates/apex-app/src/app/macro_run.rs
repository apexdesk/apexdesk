//! Running VBA macros of spreadsheets: the Run Macro list, the editor's
//! Run button, Workbook_Open and Worksheet_Change, the security questions
//! and the dialogs a running macro shows (MsgBox, InputBox and Stop).
//!
//! Running macros is off by default (File > Options > Macros). When on, a
//! file's macros run only after the user enables them for that file (the
//! message bar, or a question before the first run), except files in a
//! trusted location, whose Workbook_Open runs when they open. A macro runs
//! in a thread of its own on a copy of the workbook, which replaces the
//! tab's one when it ends: its changes are one undo step, and a macro that
//! fails or is stopped keeps what it did until then (also undoable).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use apex_sheet::{Range, Workbook};
use apex_vba::{ModuleKind, Options, Outcome, Project, Source};
use eframe::egui::{self, RichText};

use super::{App, Notice, View};
use crate::i18n::{tr, trf};

/// What to run.
#[derive(Clone, Debug)]
enum Entry {
    /// A macro by name (`Sub` or `Module.Sub`).
    Macro(String),
    /// Workbook_Open (or Auto_Open).
    Open,
    /// Worksheet_Change of a sheet, with the changed cells.
    Change(usize, Range),
}

/// A request from the macro's thread.
enum Msg {
    MsgBox { prompt: String, buttons: i32, title: String, reply: mpsc::Sender<i32> },
    InputBox { prompt: String, title: String, default: String, reply: mpsc::Sender<Option<String>> },
    Status(Option<String>),
    Done(Box<Workbook>, Box<Outcome>),
}

struct Job {
    tab: u64,
    name: String,
    rx: mpsc::Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    /// The MsgBox or InputBox being shown.
    ask: Option<Msg>,
    input: String,
    focus_input: bool,
    status: Option<String>,
    started: Instant,
}

/// The Run Macro dialog.
struct Picker {
    tab: u64,
    names: Vec<String>,
    selected: usize,
    /// A syntax error of the code (with module and line).
    error: Option<apex_vba::Error>,
}

/// Macros of a tab the user enabled (or a trusted location did).
struct Enabled {
    /// Application.EnableEvents as the last macro left it.
    events: bool,
    /// The project the handlers were looked for in (its address).
    project: usize,
    change: bool,
}

#[derive(Default)]
pub(super) struct MacroRun {
    job: Option<Job>,
    /// Waiting for the next frame (it needs the context).
    queue: Option<(u64, Entry)>,
    picker: Option<Picker>,
    /// "Enable the macros of this file?" and what runs after.
    consent: Option<(u64, Entry)>,
    /// A macro to run once Workbook_Open ends.
    after_open: Option<(u64, Entry)>,
    /// The error that stopped the last macro.
    failure: Option<(u64, apex_vba::Error)>,
    enabled: HashMap<u64, Enabled>,
    /// `Workbook::edits` already handled per tab.
    seen: HashMap<u64, u64>,
}

impl MacroRun {
    /// Whether the macros of tab `id` were enabled.
    pub(super) fn is_enabled(&self, id: u64) -> bool {
        self.enabled.contains_key(&id)
    }

    /// Tab `id` holds other code now (its file was read again): its
    /// macros are asked about again.
    pub(super) fn forget(&mut self, id: u64) {
        self.enabled.remove(&id);
    }
}

/// The modules of a VBA project (the `vbaProject.bin` bytes).
fn sources(bin: &[u8]) -> Result<Vec<Source>, apex_vba::Error> {
    let p = apex_io::vba::read(bin).map_err(|e| apex_vba::Error::app(e.to_string()))?;
    Ok(p.modules
        .into_iter()
        .map(|m| {
            let kind = match m.kind {
                apex_io::vba::ModuleKind::Standard => ModuleKind::Standard,
                apex_io::vba::ModuleKind::Class => ModuleKind::Class,
                apex_io::vba::ModuleKind::Document => ModuleKind::Document,
                apex_io::vba::ModuleKind::Form => ModuleKind::Form,
            };
            Source { name: m.name, kind, code: m.code }
        })
        .collect())
}

fn parse(bin: &[u8]) -> Result<Project, apex_vba::Error> {
    Project::parse(&sources(bin)?)
}

/// Whether `path` is in one of the trusted folders (or below).
pub(super) fn trusted(path: &Path, locations: &[PathBuf]) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    locations.iter().any(|l| {
        let l = l.canonicalize().unwrap_or_else(|_| l.clone());
        !l.as_os_str().is_empty() && path.starts_with(&l)
    })
}

/// The dialogs of a macro, shown by the UI thread.
struct ChannelUi {
    tx: mpsc::Sender<Msg>,
    ctx: egui::Context,
}

impl apex_vba::Ui for ChannelUi {
    fn msg_box(&mut self, prompt: &str, buttons: i32, title: &str) -> i32 {
        let (reply, rx) = mpsc::channel();
        if self.tx.send(Msg::MsgBox { prompt: prompt.to_string(), buttons, title: title.to_string(), reply }).is_err() {
            return 2;
        }
        self.ctx.request_repaint();
        rx.recv().unwrap_or(2)
    }

    fn input_box(&mut self, prompt: &str, title: &str, default: &str) -> Option<String> {
        let (reply, rx) = mpsc::channel();
        self.tx.send(Msg::InputBox { prompt: prompt.to_string(), title: title.to_string(), default: default.to_string(), reply }).ok()?;
        self.ctx.request_repaint();
        rx.recv().ok().flatten()
    }

    fn status(&mut self, text: Option<&str>) {
        let _ = self.tx.send(Msg::Status(text.map(str::to_string)));
        self.ctx.request_repaint();
    }
}

/// Runs `entry` on `wb` (in the macro's thread).
fn run(bin: &[u8], wb: &mut Workbook, ui: &mut ChannelUi, opts: &Options, entry: Entry) -> Outcome {
    let none = |error| Outcome { error, active_sheet: opts.active_sheet, selection: opts.selection, changed: false, save: false, events: true };
    let project = match parse(bin) {
        Ok(p) => p,
        Err(e) => return none(Some(e)),
    };
    let out = match entry {
        Entry::Macro(name) => Some(apex_vba::run_macro(&project, wb, ui, opts, &name)),
        Entry::Open => apex_vba::workbook_open(&project, wb, ui, opts),
        Entry::Change(sheet, target) => apex_vba::worksheet_change(&project, wb, ui, opts, sheet, target),
    };
    out.unwrap_or_else(|| none(None))
}

/// The buttons of a MsgBox (`vbOKOnly`... `vbRetryCancel`): i18n key and
/// the value it returns.
fn msg_buttons(buttons: i32) -> &'static [(&'static str, i32)] {
    match buttons & 7 {
        1 => &[("dialog.ok", 1), ("dialog.cancel", 2)],
        2 => &[("macros.b_abort", 3), ("macros.b_retry", 4), ("macros.b_ignore", 5)],
        3 => &[("macros.b_yes", 6), ("macros.b_no", 7), ("dialog.cancel", 2)],
        4 => &[("macros.b_yes", 6), ("macros.b_no", 7)],
        5 => &[("macros.b_retry", 4), ("dialog.cancel", 2)],
        _ => &[("dialog.ok", 1)],
    }
}

/// The macro-runnable project of tab `idx`'s workbook.
fn sheet_project(view: &View) -> Option<&Arc<Vec<u8>>> {
    match view {
        View::Sheet(v) => v.wb.macros.project.as_ref(),
        View::Doc(_) | View::Slides(_) => None,
        View::Pdf(_) | View::Db(_) | View::Note(_) => None,
    }
}

/// Whether the macros of tab `id` may run at all: macros are on and, when
/// only signed ones may run, its project is signed by a trusted
/// certificate.
pub(super) fn allowed(settings: &crate::settings::Settings, tabs: &[super::Tab], id: u64) -> bool {
    if !settings.macros_enabled {
        return false;
    }
    !settings.macros_signed_only || tabs.iter().find(|t| t.id == id).is_some_and(|t| signed(&t.view))
}

/// The project carries an intact signature of a trusted certificate,
/// made for its code as it is now: from its own streams or, in OOXML
/// files, from the signature parts next to it.
fn signed(view: &View) -> bool {
    use std::sync::Weak;
    thread_local! {
        /// Projects checked lately (checking reads the whole project
        /// again, and events ask every frame), with the roots trusted then.
        static SEEN: std::cell::RefCell<Vec<(Weak<Vec<u8>>, usize, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let View::Sheet(v) = view else { return false };
    let Some(bin) = v.wb.macros.project.as_ref() else { return false };
    let roots = apex_sign::trust::roots_added();
    if let Some(ok) = SEEN.with(|s| s.borrow().iter().find(|(w, r, _)| *r == roots && w.upgrade().is_some_and(|b| Arc::ptr_eq(&b, bin))).map(|e| e.2)) {
        return ok;
    }
    let parts = v.wb.macros.parts.iter().filter(|p| p.rel_type.contains("vbaProjectSignature")).map(|p| p.data.to_vec());
    let ok = apex_io::vba::signatures(bin).into_iter().chain(parts).any(|s| apex_sign::vba::check(&s, bin).is_some_and(|s| s.is_trusted()));
    SEEN.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|(w, ..)| w.strong_count() > 0);
        if s.len() >= 16 {
            s.remove(0);
        }
        s.push((Arc::downgrade(bin), roots, ok));
    });
    ok
}

impl App {
    /// A file with macros opened in tab `id`: in a trusted location (and
    /// with macros allowed) they are enabled and Workbook_Open runs.
    pub(super) fn macros_opened(&mut self, id: u64, path: &Path) {
        if allowed(&self.settings, &self.tabs, id) && trusted(path, &self.settings.trusted_locations) {
            self.enable_macros(id);
        }
    }

    /// The user enabled the macros of tab `id`: events fire from now on
    /// and Workbook_Open runs.
    pub(super) fn enable_macros(&mut self, id: u64) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.hold(idx);
        let View::Sheet(v) = &self.tabs[idx].view else { return };
        let Some(bin) = v.wb.macros.project.clone() else { return };
        self.macro_run.seen.insert(id, v.wb.edits);
        let project = parse(&bin).ok();
        let change = project.as_ref().is_some_and(Project::has_change_handler);
        self.macro_run.enabled.insert(id, Enabled { events: true, project: Arc::as_ptr(&bin) as usize, change });
        if project.is_some_and(|p| p.has_open_handler()) {
            self.macro_run.queue = Some((id, Entry::Open));
        }
    }

    /// Developer > Macros (Alt+F8): the macros of the active workbook.
    pub(super) fn open_run_macro(&mut self) {
        let active = self.active;
        let Some(id) = self.tabs.get(active).map(|t| t.id) else { return };
        self.flush_macros(id);
        self.hold(active);
        let Some(t) = self.tabs.get(active) else { return };
        if matches!(t.view, View::Doc(_)) {
            self.notify(Notice::info(tr("macros.sheets_only")));
            return;
        }
        let (names, error) = match sheet_project(&t.view).map(|b| parse(b)) {
            Some(Ok(p)) => (p.macros(), None),
            Some(Err(e)) => (Vec::new(), Some(e)),
            None => (Vec::new(), None),
        };
        self.macro_run.picker = Some(Picker { tab: id, names, selected: 0, error });
    }

    /// Runs macro `name` of tab `id` (asking first when needed).
    pub(super) fn run_macro_named(&mut self, id: u64, name: String) {
        self.request_run(id, Entry::Macro(name));
    }

    fn request_run(&mut self, id: u64, entry: Entry) {
        if self.is_protected(id) {
            self.notify(Notice::info(tr("protected.macros")));
            return;
        }
        if allowed(&self.settings, &self.tabs, id) && self.macro_run.is_enabled(id) {
            self.macro_run.queue = Some((id, entry));
        } else {
            self.macro_run.consent = Some((id, entry));
        }
    }

    /// Starts a queued macro, handles what a running one asks, fires
    /// Worksheet_Change after the user's edits and shows the dialogs.
    pub(super) fn macro_frame(&mut self, ctx: &egui::Context) {
        if self.macro_run.job.is_none() {
            // A form button clicked.
            let asked = match self.tabs.get_mut(self.active).map(|t| (t.id, &mut t.view)) {
                Some((id, View::Sheet(v))) => v.run_macro.take().map(|m| (id, m)),
                _ => None,
            };
            if let Some((id, name)) = asked {
                self.run_macro_named(id, name);
            }
            self.change_events();
            if let Some((id, entry)) = self.macro_run.queue.take() {
                self.start_macro(ctx, id, entry);
            }
        }
        self.poll_macro();
        self.macro_job_ui(ctx);
        self.run_macro_dialog(ctx);
        self.consent_dialog(ctx);
        self.macro_error_dialog(ctx);
    }

    /// Worksheet_Change of the active workbook after an edit of the user.
    fn change_events(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let id = t.id;
        // Macros turned off (or left unsigned) since: no more events.
        if !allowed(&self.settings, &self.tabs, id) {
            return;
        }
        let View::Sheet(v) = &t.view else { return };
        let Some(en) = self.macro_run.enabled.get_mut(&id) else { return };
        let edits = v.wb.edits;
        let seen = self.macro_run.seen.insert(id, edits);
        if seen.is_none_or(|s| s == edits) || !en.events {
            return;
        }
        // The code may have changed in the editor: look again.
        let Some(bin) = v.wb.macros.project.as_ref() else { return };
        if Arc::as_ptr(bin) as usize != en.project {
            en.project = Arc::as_ptr(bin) as usize;
            en.change = parse(bin).is_ok_and(|p| p.has_change_handler());
        }
        if !en.change {
            return;
        }
        if let Some((sheet, target)) = v.wb.last_edit() {
            self.macro_run.queue = Some((id, Entry::Change(sheet, target)));
        }
    }

    fn start_macro(&mut self, ctx: &egui::Context, id: u64, entry: Entry) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.flush_macros(id);
        self.hold(idx);
        let file_name = self.tabs[idx].name();
        let file_path = self.tabs[idx].path.as_ref().and_then(|p| p.parent()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let View::Sheet(v) = &mut self.tabs[idx].view else { return };
        let Some(bin) = v.wb.macros.project.clone() else { return };
        let (sheet, selection) = v.macro_context();
        let wb = v.wb.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let opts = Options {
            active_sheet: sheet,
            selection,
            file_name,
            file_path,
            user_name: crate::doc_view::user_name(),
            utc_offset: crate::common::local_utc_offset(),
            // The thread has 64 MiB of stack.
            max_stack: 48 << 20,
            cancel: Some(cancel.clone()),
            ..Options::default()
        };
        let name = match &entry {
            Entry::Macro(n) => n.clone(),
            Entry::Open => "Workbook_Open".to_string(),
            Entry::Change(..) => "Worksheet_Change".to_string(),
        };
        let (tx, rx) = mpsc::channel();
        let mut ui = ChannelUi { tx: tx.clone(), ctx: ctx.clone() };
        let spawned = std::thread::Builder::new().name("apex-macro".into()).stack_size(64 << 20).spawn(move || {
            let mut wb = wb;
            let out = run(&bin, &mut wb, &mut ui, &opts, entry);
            let _ = tx.send(Msg::Done(Box::new(wb), Box::new(out)));
            ui.ctx.request_repaint();
        });
        if spawned.is_err() {
            self.notify(Notice::error(tr("macros.crashed")));
            return;
        }
        self.macro_run.failure = None;
        self.macro_run.job = Some(Job { tab: id, name, rx, cancel, ask: None, input: String::new(), focus_input: false, status: None, started: Instant::now() });
    }

    /// Takes what the macro's thread sent.
    fn poll_macro(&mut self) {
        loop {
            let Some(job) = self.macro_run.job.as_mut().filter(|j| j.ask.is_none()) else { return };
            match job.rx.try_recv() {
                Ok(Msg::Status(s)) => job.status = s,
                Ok(Msg::Done(wb, out)) => {
                    let job = self.macro_run.job.take();
                    self.finish_macro(job.map_or(0, |j| j.tab), Some((*wb, *out)));
                    return;
                }
                Ok(m) => {
                    if let Msg::InputBox { default, .. } = &m {
                        job.input = default.clone();
                        job.focus_input = true;
                    }
                    job.ask = Some(m);
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let job = self.macro_run.job.take();
                    self.finish_macro(job.map_or(0, |j| j.tab), None);
                    return;
                }
            }
        }
    }

    /// Puts the workbook the macro changed in its tab.
    fn finish_macro(&mut self, id: u64, done: Option<(Workbook, Outcome)>) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        let Some((wb, out)) = done else {
            // It panicked: the tab keeps the workbook as it was.
            self.notify(Notice::error(tr("macros.crashed")));
            return;
        };
        self.hold(idx);
        if let View::Sheet(v) = &mut self.tabs[idx].view {
            v.macro_done(out.changed.then_some(wb), out.active_sheet, out.selection);
            self.macro_run.seen.insert(id, v.wb.edits);
        }
        if let Some(en) = self.macro_run.enabled.get_mut(&id) {
            en.events = out.events;
        }
        if let Some(e) = out.error {
            self.macro_run.failure = Some((id, e));
        }
        if out.save {
            self.save(idx, false);
        }
    }

    /// MsgBox, InputBox, or the Stop button of a running macro.
    fn macro_job_ui(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.macro_run.job else { return };
        let modal = egui::Modal::new(egui::Id::new("macro_job"));
        match &job.ask {
            Some(Msg::MsgBox { prompt, buttons, title, reply }) => {
                let mut answer = None;
                let choices = msg_buttons(*buttons);
                let m = modal.show(ctx, |ui| {
                    ui.set_min_width(320.0);
                    ui.set_max_width(520.0);
                    ui.heading(if title.is_empty() { "ApexDesk" } else { title.as_str() });
                    ui.add_space(8.0);
                    ui.label(prompt.as_str());
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (key, value) in choices.iter().rev() {
                            if ui.button(tr(key)).clicked() {
                                answer = Some(*value);
                            }
                        }
                    });
                });
                if answer.is_none() && m.is_top_modal {
                    if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        answer = choices.first().map(|c| c.1);
                    } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                        answer = Some(choices.iter().find(|c| c.1 == 2).map_or(choices[0].1, |c| c.1));
                    }
                }
                if let Some(a) = answer {
                    let _ = reply.send(a);
                    job.ask = None;
                }
            }
            Some(Msg::InputBox { prompt, title, reply, .. }) => {
                let mut answer = None;
                let m = modal.show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    ui.set_max_width(520.0);
                    ui.heading(if title.is_empty() { "ApexDesk" } else { title.as_str() });
                    ui.add_space(8.0);
                    ui.label(prompt.as_str());
                    let r = ui.add(egui::TextEdit::singleline(&mut job.input).desired_width(f32::INFINITY));
                    if std::mem::take(&mut job.focus_input) {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        answer = Some(Some(job.input.clone()));
                    }
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("dialog.cancel")).clicked() {
                            answer = Some(None);
                        }
                        if ui.button(tr("dialog.ok")).clicked() {
                            answer = Some(Some(job.input.clone()));
                        }
                    });
                });
                if answer.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(None);
                }
                if let Some(a) = answer {
                    let _ = reply.send(a);
                    job.ask = None;
                }
            }
            _ => {
                // Blocks the window while it runs; says so after a moment
                // (most macros end before).
                let shown = job.started.elapsed() > Duration::from_millis(400);
                let modal = if shown { modal } else { modal.backdrop_color(egui::Color32::TRANSPARENT).frame(egui::Frame::NONE) };
                let (name, status) = (job.name.clone(), job.status.clone());
                let mut stop = false;
                modal.show(ctx, |ui| {
                    if !shown {
                        return;
                    }
                    ui.set_min_width(320.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(trf("macros.running", &[("name", &name)]));
                    });
                    if let Some(s) = &status {
                        ui.label(RichText::new(s).weak());
                    }
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        stop = ui.button(tr("macros.stop")).clicked();
                    });
                });
                // Ctrl+Break (Esc) stops it, like in Excel.
                if stop || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    /// Developer > Macros: pick one and run it.
    fn run_macro_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.macro_run.picker else { return };
        if !self.tabs.iter().any(|t| t.id == p.tab) {
            self.macro_run.picker = None;
            return;
        }
        let enabled = allowed(&self.settings, &self.tabs, p.tab);
        let why = if self.settings.macros_enabled { tr("macros.unsigned") } else { tr("macros.disabled") };
        let (mut run, mut edit, mut close, mut options) = (false, false, false, false);
        let m = egui::Modal::new(egui::Id::new("run_macro")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(tr("macros.run_macro"));
            ui.add_space(6.0);
            if let Some(e) = &p.error {
                ui.label(RichText::new(trf("macros.error_at", &[("module", &e.module), ("line", &e.line)])).strong());
                ui.label(RichText::new(e.to_string()).color(ui.visuals().error_fg_color));
            } else if p.names.is_empty() {
                ui.label(tr("macros.no_macros"));
            } else {
                ui.label(tr("macros.run_list"));
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for (i, n) in p.names.iter().enumerate() {
                            let r = ui.selectable_label(i == p.selected, n);
                            if r.clicked() {
                                p.selected = i;
                            }
                            if r.double_clicked() {
                                run = true;
                            }
                        }
                    });
                });
            }
            if !enabled {
                ui.add_space(6.0);
                ui.label(RichText::new(why).color(ui.visuals().warn_fg_color));

            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(format!("{}…", tr("macros.settings"))).clicked() {
                    options = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button(tr("dialog.close")).clicked();
                    if ui.button(tr("macros.edit")).clicked() {
                        edit = true;
                    }
                    let can = enabled && !p.names.is_empty();
                    if ui.add_enabled(can, egui::Button::new(RichText::new(tr("macros.run")).strong())).clicked() {
                        run = true;
                    }
                });
            });
        });
        if m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if m.is_top_modal && !p.names.is_empty() && enabled && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            run = true;
        }
        run &= enabled;
        let tab = p.tab;
        let name = p.names.get(p.selected).cloned();
        if run || edit || close || options {
            self.macro_run.picker = None;
        }
        if options {
            self.open_macro_options();
        }
        if edit {
            self.open_macros();
        }
        if run && let Some(name) = name {
            self.run_macro_named(tab, name);
        }
    }

    /// "Enable the macros of this file?", or where to allow macros.
    fn consent_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, _)) = &self.macro_run.consent else { return };
        let id = *id;
        let Some(file) = self.tabs.iter().find(|t| t.id == id).map(|t| t.name()) else {
            self.macro_run.consent = None;
            return;
        };
        let enabled = allowed(&self.settings, &self.tabs, id);
        let why = if self.settings.macros_enabled { tr("macros.unsigned") } else { tr("macros.disabled") };
        let (mut choice, mut options) = (None, false);
        let m = egui::Modal::new(egui::Id::new("macro_consent")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("macros.consent_title"));
            ui.add_space(8.0);
            ui.label(if enabled { trf("macros.consent", &[("file", &file)]) } else { why.to_string() });

            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.cancel")).clicked() {
                    choice = Some(false);
                }
                if enabled {
                    if ui.button(RichText::new(tr("macros.consent_ok")).strong()).clicked() {
                        choice = Some(true);
                    }
                } else if ui.button(format!("{}…", tr("macros.settings"))).clicked() {
                    choice = Some(false);
                    options = true;
                }
            });
        });
        if choice.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(false);
        }
        let Some(yes) = choice else { return };
        let Some((id, entry)) = self.macro_run.consent.take() else { return };
        if options {
            self.open_macro_options();
        }
        if yes && enabled {
            self.enable_macros(id);
            // Workbook_Open (queued when enabling) goes first, then the macro.
            match self.macro_run.queue.take() {
                Some(open) => {
                    self.macro_run.queue = Some(open);
                    self.macro_run.after_open = Some((id, entry));
                }
                None => self.macro_run.queue = Some((id, entry)),
            }
        }
    }

    /// The error that stopped a macro: its module, line and message.
    fn macro_error_dialog(&mut self, ctx: &egui::Context) {
        if self.macro_run.job.is_some() {
            return;
        }
        // Workbook_Open ended (it's no longer queued): then the macro.
        if self.macro_run.queue.is_none()
            && let Some(next) = self.macro_run.after_open.take()
            && self.macro_run.failure.is_none()
        {
            self.macro_run.queue = Some(next);
            return;
        }
        let Some((id, e)) = &self.macro_run.failure else { return };
        let (id, module, line) = (*id, e.module.clone(), e.line);
        let (mut close, mut show) = (false, false);
        let m = egui::Modal::new(egui::Id::new("macro_error")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.set_max_width(520.0);
            ui.heading(tr("macros.error_title"));
            ui.add_space(8.0);
            if !module.is_empty() {
                ui.label(RichText::new(trf("macros.error_at", &[("module", &module), ("line", &line)])).strong());
            }
            ui.label(e.to_string());
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = ui.button(tr("dialog.close")).clicked();
                if !module.is_empty() && ui.button(tr("macros.show_code")).clicked() {
                    show = true;
                }
            });
        });
        if m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
            close = true;
        }
        if close || show {
            self.macro_run.failure = None;
        }
        if show && let Some(idx) = self.tabs.iter().position(|t| t.id == id) {
            self.active = idx;
            self.show_macro_code(&module, line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_folders() {
        let base = std::env::temp_dir().join("apex_trust_test");
        let locs = vec![base.join("ok")];
        assert!(trusted(&base.join("ok").join("sub").join("a.xlsm"), &locs));
        assert!(trusted(&base.join("ok").join("a.xlsm"), &locs));
        assert!(!trusted(&base.join("okay").join("a.xlsm"), &locs));
        assert!(!trusted(&base.join("a.xlsm"), &locs));
        assert!(!trusted(&base.join("a.xlsm"), &[PathBuf::new()]));
    }

    #[test]
    fn message_box_buttons() {
        assert_eq!(msg_buttons(0), &[("dialog.ok", 1)]);
        assert_eq!(msg_buttons(4 + 32).iter().map(|b| b.1).collect::<Vec<_>>(), [6, 7]);
        assert_eq!(msg_buttons(3).len(), 3);
    }
}
