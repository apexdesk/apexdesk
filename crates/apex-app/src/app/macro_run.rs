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

use apex_fs::PathExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use web_time::{Duration, Instant};

use apex_sheet::Workbook;
use apex_vba::{ModuleKind, Options, Outcome, Project, Source};
use eframe::egui::{self, RichText};

use super::{App, Notice, View};
use crate::i18n::{tr, trf};

mod db;
mod debug_ui;
mod events;
mod form_ui;
pub(crate) mod headless;
mod library;
mod recorder;
mod revocation;
mod schedule;
mod udf;

pub(crate) use library::personal_path;

pub(super) use debug_ui::{DebugConf, immediate_pane, watch_list};

/// What to run.
#[derive(Clone, Debug)]
enum Entry {
    /// A macro by name (`Sub` or `Module.Sub`).
    Macro(String),
    /// The same, paused at its first statement (F8).
    Step(String),
    /// A Sub or Function with arguments as typed (Run Macro).
    Call(String, Vec<String>),
    /// A line of the Immediate window, with no macro running.
    Immediate(String),
    /// Workbook_Open (or Auto_Open).
    Open,
    /// Document_New (or AutoNew): a document made from a template.
    New,
    /// Handlers of events of the workbook and its sheets, one after
    /// another (until one cancels or fails).
    Events(Vec<events::Event>),
}

/// A request from the macro's thread.
enum Msg {
    MsgBox { prompt: String, buttons: i32, title: String, reply: mpsc::Sender<i32> },
    InputBox { prompt: String, title: String, default: String, reply: mpsc::Sender<Option<String>> },
    Status(Option<String>),
    /// Debug.Print, and what the Immediate window prints.
    Print(String),
    /// Paused in the debugger, with the workbook or document so far.
    Pause { at: Box<apex_vba::debug::Pause>, wb: Shown, reply: mpsc::Sender<apex_vba::debug::DebugCmd> },
    Done(Box<Workbook>, Box<Outcome>),
    /// A document's macro ended, with the document as it left it.
    DocDone(Box<apex_doc::Document>, Box<apex_vba::DocOutcome>),
    /// A database's macro ended.
    DbDone(Box<apex_vba::DbOutcome>),
    /// The macro wants to change files in a folder: whether it may.
    AllowFiles { folder: String, reply: mpsc::Sender<bool> },
    /// GetOpenFilename, FileDialog...: the system's file dialog.
    PickFiles { pick: apex_vba::files::FilePick, reply: mpsc::Sender<Option<Vec<String>>> },
    /// UserForms shown: what the user does in them.
    Form { views: Vec<Arc<apex_vba::forms::View>>, reply: mpsc::Sender<Option<apex_vba::forms::Event>> },
    /// A form was hidden: those still shown.
    Forms(Vec<Arc<apex_vba::forms::View>>),
}

/// What a tab shows while its macro is paused (the macro's copy), or
/// what it had before (put back when the macro ends).
enum Shown {
    Book(Box<Workbook>),
    Doc(Box<apex_doc::Document>, apex_doc::Selection),
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
    /// The file dialog of a PickFiles is open.
    dialog_open: bool,
    status: Option<String>,
    started: Instant,
    /// Ctrl+Break: pause in the debugger.
    interrupt: Arc<AtomicBool>,
    /// Paused, waiting for the debugger.
    pause: Option<debug_ui::Stopped>,
    panel: debug_ui::Panel,
    /// When the last debugger command was sent.
    resumed: Instant,
    /// The tab's workbook or document before the first pause (the tab
    /// shows the macro's meanwhile); back in the tab when it ends.
    original: Option<Shown>,
    /// The modules' names and code, for the debugger.
    sources: Vec<(String, String)>,
    /// What remembers that this file's macros may change files (None:
    /// remembered already, or a file not saved).
    grant: Option<String>,
    /// What the user did that waits for the events.
    after: Option<events::After>,
    /// Names as the code writes them.
    names: HashMap<String, String>,
    /// A try (Run Macro > Try): the folders where it wanted to change files.
    trial: Option<Arc<Mutex<Vec<String>>>>,
    /// The UserForms shown last (drawn under a MsgBox a form's code shows).
    forms: Vec<Arc<apex_vba::forms::View>>,
    form: form_ui::State,
}

/// The Run Macro dialog.
struct Picker {
    tab: u64,
    names: Vec<String>,
    /// Subs with parameters and Functions, after `names` in the list.
    calls: Vec<(String, Vec<String>)>,
    selected: usize,
    /// A syntax error of the code (with module and line).
    error: Option<apex_vba::Error>,
    /// What the selected macro does (its index, the sentences).
    explained: Option<(usize, Vec<(String, bool)>)>,
}

/// The arguments of a Sub or Function to run.
struct ArgsDlg {
    tab: u64,
    name: String,
    params: Vec<String>,
    values: Vec<String>,
    /// Shown for the first time: the key that opened it doesn't count.
    fresh: bool,
}

/// Macros of a tab the user enabled (or a trusted location did).
struct Enabled {
    /// Application.EnableEvents as the last macro left it.
    events: bool,
    /// The project the handlers were looked for in (its address).
    project: usize,
    /// The procedures that may handle events (lower case).
    handlers: Vec<String>,
}

#[derive(Default)]
pub(super) struct MacroRun {
    job: Option<Job>,
    /// Waiting for the next frame (it needs the context).
    queue: Option<(u64, Entry)>,
    picker: Option<Picker>,
    args: Option<ArgsDlg>,
    /// "Enable the macros of this file?" and what runs after.
    consent: Option<(u64, Entry)>,
    /// A macro to run once Workbook_Open ends.
    after_open: Option<(u64, Entry)>,
    /// The error that stopped the last macro.
    failure: Option<(u64, apex_vba::Error)>,
    enabled: HashMap<u64, Enabled>,
    /// `Workbook::edits` already handled per tab.
    seen: HashMap<u64, u64>,
    /// Documents made from a template with macros: enabling them runs
    /// Document_New, not Document_Open.
    pub(super) made_new: HashSet<u64>,
    /// Events waiting for the macro running to end.
    events: VecDeque<(u64, Vec<events::Event>, Option<events::After>)>,
    /// Where the user was per tab (sheet, selection, sheets).
    places: HashMap<u64, events::Place>,
    /// The tab whose workbook was at the front.
    front: Option<u64>,
    /// Printing goes on: BeforePrint fired.
    printing: bool,
    /// Closing the window waits for BeforeClose of these tabs.
    quitting: Option<Vec<u64>>,
    /// They let it close.
    pub(super) quit_now: bool,
    /// Breakpoints, watches and the Immediate window per tab.
    pub(super) debug: HashMap<u64, DebugConf>,
    /// OnKey, macro shortcuts and OnTime.
    schedule: schedule::Schedule,
    /// The personal macro workbook and the add-ins.
    libs: library::Libs,
    /// The projects whose functions each tab's cells were given, its own
    /// then the libraries.
    udf: HashMap<u64, Vec<usize>>,
    /// Record Macro: its dialog, and the recording.
    pub(super) record: recorder::Record,
    /// Try asked for this macro of this tab: the next run of it is a try.
    trial: Option<(u64, String)>,
    /// A try that ended, shown to apply or discard.
    pub(super) report: Option<super::macro_trial::Trial>,
    /// A database form's button ran this macro of this tab: the form.
    db_form: Option<(u64, String, String)>,
}

impl MacroRun {
    /// Whether the macros of tab `id` were enabled.
    pub(super) fn is_enabled(&self, id: u64) -> bool {
        self.enabled.contains_key(&id)
    }

    /// Whether events fire for tab `id`, as a macro left it.
    pub(super) fn set_events(&mut self, id: u64, on: bool) {
        if let Some(en) = self.enabled.get_mut(&id) {
            en.events = on;
        }
    }

    /// Tab `id` holds other code now (its file was read again): its
    /// macros are asked about again.
    pub(super) fn forget(&mut self, id: u64) {
        self.enabled.remove(&id);
        self.schedule.forget(id);
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

/// The designs of the UserForms of a VBA project.
fn form_designs(bin: &[u8]) -> Vec<(String, Arc<apex_vba::forms::Design>)> {
    let Ok(p) = apex_io::vba::read(bin) else { return Vec::new() };
    p.modules
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m.kind, apex_io::vba::ModuleKind::Form))
        .filter_map(|(i, m)| Some((m.name.clone(), Arc::new(apex_vba::forms::Design::read(&p.form_streams(i)?)))))
        .collect()
}

/// Whether `path` is in one of the trusted folders (or below).
/// What "allow always for this file" remembers: the file's path and its
/// macros' code, hashed (other code or another file asks again).
fn grant_key(path: &Path, project: &[u8]) -> String {
    let path = apex_fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut data = path.to_string_lossy().into_owned().into_bytes();
    data.push(0);
    data.extend_from_slice(project);
    use sha2::Digest;
    sha2::Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn trusted(path: &Path, locations: &[PathBuf]) -> bool {
    let path = path.fs_canonicalize().unwrap_or_else(|_| path.to_path_buf());
    locations.iter().any(|l| {
        let l = l.fs_canonicalize().unwrap_or_else(|_| l.clone());
        !l.as_os_str().is_empty() && path.starts_with(&l)
    })
}

/// In the browser a macro runs on the page's only thread, where the
/// window can't show its questions: the browser's own dialogs ask them,
/// waiting for the answer. Breakpoints, forms and file pickers don't stop
/// it there.
#[cfg(target_arch = "wasm32")]
mod browser {
    fn text(prompt: &str, title: &str) -> String {
        if title.is_empty() { prompt.to_string() } else { format!("{title}\n\n{prompt}") }
    }

    pub fn confirm(text: &str) -> bool {
        web_sys::window().and_then(|w| w.confirm_with_message(text).ok()).unwrap_or(false)
    }

    /// MsgBox's answer: OK 1, Cancel 2, Abort 3, Retry 4, Yes 6, No 7.
    pub fn msg_box(prompt: &str, buttons: i32, title: &str) -> i32 {
        let t = text(prompt, title);
        let yes = |a, b| if confirm(&t) { a } else { b };
        match buttons & 7 {
            0 => {
                if let Some(w) = web_sys::window() {
                    let _ = w.alert_with_message(&t);
                }
                1
            }
            1 => yes(1, 2),
            2 => yes(4, 3),
            3 => yes(6, 2),
            4 => yes(6, 7),
            5 => yes(4, 2),
            _ => 1,
        }
    }

    pub fn input_box(prompt: &str, title: &str, default: &str) -> Option<String> {
        web_sys::window()?.prompt_with_message_and_default(&text(prompt, title), default).ok().flatten()
    }
}

/// The dialogs of a macro, shown by the UI thread.
struct ChannelUi {
    tx: mpsc::Sender<Msg>,
    ctx: egui::Context,
    /// The workbook's password and whether it came from the Internet:
    /// what SaveAs and SaveCopyAs write keeps them.
    password: Option<String>,
    downloaded: bool,
    /// A try: files aren't changed, the folders asked for are noted.
    trial: Option<Arc<Mutex<Vec<String>>>>,
}

impl apex_vba::Ui for ChannelUi {
    fn msg_box(&mut self, prompt: &str, buttons: i32, title: &str) -> i32 {
        #[cfg(target_arch = "wasm32")]
        return browser::msg_box(prompt, buttons, title);
        let (reply, rx) = mpsc::channel();
        if self.tx.send(Msg::MsgBox { prompt: prompt.to_string(), buttons, title: title.to_string(), reply }).is_err() {
            return 2;
        }
        self.ctx.request_repaint();
        rx.recv().unwrap_or(2)
    }

    fn input_box(&mut self, prompt: &str, title: &str, default: &str) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        return browser::input_box(prompt, title, default);
        let (reply, rx) = mpsc::channel();
        self.tx.send(Msg::InputBox { prompt: prompt.to_string(), title: title.to_string(), default: default.to_string(), reply }).ok()?;
        self.ctx.request_repaint();
        rx.recv().ok().flatten()
    }

    fn status(&mut self, text: Option<&str>) {
        let _ = self.tx.send(Msg::Status(text.map(str::to_string)));
        self.ctx.request_repaint();
    }

    fn debug_print(&mut self, text: &str) {
        let _ = self.tx.send(Msg::Print(text.to_string()));
        self.ctx.request_repaint();
    }

    fn allow_file_changes(&mut self, folder: &str) -> bool {
        if let Some(t) = &self.trial {
            if let Ok(mut f) = t.lock()
                && f.len() < 20
                && !f.iter().any(|x| x == folder)
            {
                f.push(folder.to_string());
            }
            return false;
        }
        #[cfg(target_arch = "wasm32")]
        return browser::confirm(&format!("{}\n\n{}", tr("macros.files_title"), trf("macros.files_ask", &[("folder", &folder)])));
        let (reply, rx) = mpsc::channel();
        if self.tx.send(Msg::AllowFiles { folder: folder.to_string(), reply }).is_err() {
            return false;
        }
        self.ctx.request_repaint();
        rx.recv().unwrap_or(false)
    }

    fn save_workbook(&mut self, wb: &Workbook, path: &str, format: apex_vba::files::BookFormat, sheet: usize) -> Result<(), String> {
        write_book(wb, path, format, sheet, self.password.as_deref(), self.downloaded)
    }

    fn save_opened(&mut self, wb: &Workbook, path: &str, format: apex_vba::files::BookFormat, sheet: usize) -> Result<(), String> {
        write_book(wb, path, format, sheet, None, self.downloaded)
    }

    fn save_document(&mut self, doc: &apex_doc::Document, path: &str, format: apex_vba::DocFormat) -> Result<(), String> {
        write_doc(doc, path, format, self.downloaded)
    }

    fn open_workbook(&mut self, path: &str, password: Option<&str>) -> Result<Workbook, String> {
        read_book(path, password)
    }

    fn open_document(&mut self, path: &str) -> Result<apex_doc::Document, String> {
        read_doc(path)
    }

    fn pick_files(&mut self, pick: &apex_vba::files::FilePick) -> Option<Vec<String>> {
        #[cfg(target_arch = "wasm32")]
        return None;
        let (reply, rx) = mpsc::channel();
        self.tx.send(Msg::PickFiles { pick: pick.clone(), reply }).ok()?;
        self.ctx.request_repaint();
        rx.recv().ok().flatten()
    }

    fn pause(&mut self, at: &apex_vba::debug::Pause, wb: &Workbook) -> apex_vba::debug::DebugCmd {
        #[cfg(target_arch = "wasm32")]
        return apex_vba::debug::DebugCmd::Continue;
        let (reply, rx) = mpsc::channel();
        if self.tx.send(Msg::Pause { at: Box::new(at.clone()), wb: Shown::Book(Box::new(wb.clone())), reply }).is_err() {
            return apex_vba::debug::DebugCmd::End;
        }
        self.ctx.request_repaint();
        // The window closed (or the tab): the macro ends.
        rx.recv().unwrap_or(apex_vba::debug::DebugCmd::End)
    }

    fn pause_doc(&mut self, at: &apex_vba::debug::Pause, doc: &apex_doc::Document, sel: apex_doc::Selection) -> apex_vba::debug::DebugCmd {
        #[cfg(target_arch = "wasm32")]
        return apex_vba::debug::DebugCmd::Continue;
        let (reply, rx) = mpsc::channel();
        if self.tx.send(Msg::Pause { at: Box::new(at.clone()), wb: Shown::Doc(Box::new(doc.clone()), sel), reply }).is_err() {
            return apex_vba::debug::DebugCmd::End;
        }
        self.ctx.request_repaint();
        rx.recv().unwrap_or(apex_vba::debug::DebugCmd::End)
    }

    fn can_show_forms(&self) -> bool {
        // A form waits for its events, which the browser's single thread
        // can't give it while the macro runs.
        cfg!(not(target_arch = "wasm32"))
    }

    fn form_event(&mut self, forms: &[Arc<apex_vba::forms::View>]) -> Option<apex_vba::forms::Event> {
        #[cfg(target_arch = "wasm32")]
        return None;
        let (reply, rx) = mpsc::channel();
        self.tx.send(Msg::Form { views: forms.to_vec(), reply }).ok()?;
        self.ctx.request_repaint();
        rx.recv().ok().flatten()
    }

    fn forms_hidden(&mut self, forms: &[Arc<apex_vba::forms::View>]) {
        let _ = self.tx.send(Msg::Forms(forms.to_vec()));
        self.ctx.request_repaint();
    }
}

/// Workbook.SaveAs and the like: `wb` written at `path` (already allowed)
/// in `format`, CSV and text with sheet `sheet`, encrypted with
/// `password`; marked as from the Internet when the macro's file is.
fn write_book(wb: &Workbook, path: &str, format: apex_vba::files::BookFormat, sheet: usize, password: Option<&str>, downloaded: bool) -> Result<(), String> {
    use apex_vba::files::BookFormat;
    let p = Path::new(path);
    let fmt = match format {
        BookFormat::Xlsx => apex_io::Format::Xlsx,
        BookFormat::Ods => apex_io::Format::Ods,
        BookFormat::Fods => apex_io::Format::Fods,
        BookFormat::Csv | BookFormat::Tsv => {
            // The active sheet, like Excel.
            let sep = (format == BookFormat::Tsv).then_some('\t');
            let text = apex_io::csv::write(wb, sheet.min(wb.sheets.len().saturating_sub(1)), sep);
            apex_io::write_atomic(p, text.as_bytes()).map_err(|e| e.to_string())?;
            if downloaded {
                apex_io::safe_path::mark_untrusted(p);
            }
            return Ok(());
        }
    };
    apex_io::save_sheet_with(wb, p, fmt, password).map_err(|e| e.to_string())?;
    if downloaded {
        apex_io::safe_path::mark_untrusted(p);
    }
    Ok(())
}

/// Document.SaveAs: `doc` written at `path` (already allowed) in
/// `format` (a .docm or template keeps the macros, a .docx drops them);
/// marked as from the Internet when the macro's file is.
fn write_doc(doc: &apex_doc::Document, path: &str, format: apex_vba::DocFormat, downloaded: bool) -> Result<(), String> {
    use apex_vba::DocFormat;
    let p = Path::new(path);
    let fmt = match format {
        DocFormat::Docx => apex_io::Format::Docx,
        DocFormat::Odt => apex_io::Format::Odt,
        DocFormat::Fodt => apex_io::Format::Fodt,
        DocFormat::Rtf => apex_io::Format::Rtf,
        DocFormat::Text => apex_io::Format::Text,
        DocFormat::Markdown => apex_io::Format::Markdown,
    };
    apex_io::save_doc(doc, p, fmt).map_err(|e| e.to_string())?;
    if downloaded {
        apex_io::safe_path::mark_untrusted(p);
    }
    Ok(())
}

/// Runs `entry` on a document (in the macro's thread).
fn run_doc(bin: &[u8], doc: &mut apex_doc::Document, sel: apex_doc::Selection, ui: &mut ChannelUi, opts: &Options, entry: Entry) -> apex_vba::DocOutcome {
    let none = |error| apex_vba::DocOutcome { error, changed: false, save: false, events: true, cancel: false, requests: Vec::new(), selection: sel, docs: Vec::new() };
    let project = match parse(bin) {
        Ok(p) => p,
        Err(e) => return none(Some(e)),
    };
    let with_forms;
    let opts = if project.modules.iter().any(|m| m.kind == ModuleKind::Form) {
        with_forms = Options { forms: form_designs(bin), ..opts.clone() };
        &with_forms
    } else {
        opts
    };
    let out = match entry {
        Entry::Macro(name) | Entry::Step(name) => Some(apex_vba::run_doc_macro(&project, doc, sel, ui, opts, &name)),
        Entry::Call(name, args) => Some(apex_vba::call_doc_macro(&project, doc, sel, ui, opts, &name, &args)),
        Entry::Immediate(line) => Some(apex_vba::doc_immediate(&project, doc, sel, ui, opts, &line)),
        Entry::Open => apex_vba::doc_event(&project, doc, sel, ui, opts, "Open"),
        Entry::New => apex_vba::doc_event(&project, doc, sel, ui, opts, "New"),
        // A document has no sheet events.
        Entry::Events(_) => None,
    };
    out.unwrap_or_else(|| none(None))
}

/// Workbooks.Open: the workbook at `path` (already allowed), read. Its
/// macros never run from there.
fn read_book(path: &str, password: Option<&str>) -> Result<Workbook, String> {
    match apex_io::open_with(Path::new(path), password) {
        Ok((apex_io::Loaded::Sheet(wb), _)) => {
            let mut wb = *wb;
            wb.enforce_protection = true;
            Ok(wb)
        }
        Ok(_) => Err(tr("mail.not_sheet").to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn read_doc(path: &str) -> Result<apex_doc::Document, String> {
    match apex_io::open_with(Path::new(path), None) {
        Ok((apex_io::Loaded::Doc(doc), _)) => Ok(*doc),
        Ok(_) => Err("it isn't a text document".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs `entry` on `wb` (in the macro's thread).
fn run(bin: Option<&[u8]>, wb: &mut Workbook, ui: &mut ChannelUi, opts: &Options, entry: Entry) -> Outcome {
    let none = |error| Outcome { error, active_sheet: opts.active_sheet, selection: opts.selection, changed: false, save: false, events: true, cancel: false, requests: Vec::new(), books: Vec::new() };
    // A macro of the personal workbook or an add-in, on this workbook.
    if let Some((lib, name)) = library_macro(&entry, opts) {
        let project = match Project::parse(&lib.sources) {
            Ok(p) => p,
            Err(e) => return none(Some(e)),
        };
        let mut out = match &entry {
            Entry::Call(_, args) => apex_vba::call_macro(&project, wb, ui, opts, name, args),
            _ => apex_vba::run_macro(&project, wb, ui, opts, name),
        };
        out.requests = out.requests.into_iter().map(|r| r.in_book(&lib.name)).collect();
        return out;
    }
    let Some(bin) = bin else { return none(None) };
    let project = match parse(bin) {
        Ok(p) => p,
        Err(e) => return none(Some(e)),
    };
    let with_forms;
    let opts = if project.modules.iter().any(|m| m.kind == ModuleKind::Form) {
        with_forms = Options { forms: form_designs(bin), ..opts.clone() };
        &with_forms
    } else {
        opts
    };
    // `ThisFile.xlsm!Macro`: one of its own.
    let own = |n: String| match n.rsplit_once('!') {
        Some((book, m)) if apex_vba::same_book(&opts.file_name, book) => m.to_string(),
        _ => n,
    };
    let entry = match entry {
        Entry::Macro(n) => Entry::Macro(own(n)),
        Entry::Call(n, args) => Entry::Call(own(n), args),
        e => e,
    };
    let out = match entry {
        Entry::Macro(name) | Entry::Step(name) => Some(apex_vba::run_macro(&project, wb, ui, opts, &name)),
        Entry::Call(name, args) => Some(apex_vba::call_macro(&project, wb, ui, opts, &name, &args)),
        Entry::Immediate(line) => Some(apex_vba::immediate(&project, wb, ui, opts, &line)),
        Entry::Open => apex_vba::workbook_open(&project, wb, ui, opts),
        // Workbooks have no New event.
        Entry::New => None,
        Entry::Events(events) => run_events(&project, wb, ui, opts, events),
    };
    out.unwrap_or_else(|| none(None))
}

/// The library and procedure a `Book!Macro` entry names, when the book is
/// one of the libraries (and not this file).
fn library_macro<'a>(entry: &'a Entry, opts: &'a Options) -> Option<(&'a apex_vba::Library, &'a str)> {
    let (Entry::Macro(n) | Entry::Call(n, _)) = entry else { return None };
    let (book, name) = n.rsplit_once('!')?;
    if apex_vba::same_book(&opts.file_name, book) {
        return None;
    }
    Some((opts.libraries.iter().find(|l| l.is(book))?, name))
}

/// The handlers of `events`, one after another: each starts where the
/// last left the sheet and selection; they stop at one that cancels or
/// fails or turns events off. What they did together.
fn run_events(project: &Project, wb: &mut Workbook, ui: &mut ChannelUi, opts: &Options, events: Vec<events::Event>) -> Option<Outcome> {
    let mut opts = opts.clone();
    let mut all: Option<Outcome> = None;
    for e in events {
        let Some(out) = apex_vba::fire_event(project, wb, ui, &opts, e.owner, e.name, &e.args) else { continue };
        opts.active_sheet = out.active_sheet;
        opts.selection = out.selection;
        let stop = out.error.is_some() || out.cancel || !out.events;
        all = Some(match all {
            Some(mut a) => {
                a.requests.extend(out.requests);
                a.books.extend(out.books);
                Outcome { changed: a.changed || out.changed, save: a.save || out.save, requests: a.requests, books: a.books, ..out }
            }
            None => out,
        });
        if stop {
            break;
        }
    }
    all
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

/// The macro-runnable project of a tab's workbook or document.
fn macro_project(view: &View) -> Option<&Arc<Vec<u8>>> {
    match view {
        View::Sheet(v) => v.wb.macros.project.as_ref(),
        View::Doc(v) => v.doc.macros.project.as_ref(),
        View::Db(v) => v.macros.project.as_ref(),
        View::Slides(_) => None,
        View::Pdf(_) | View::Note(_) => None,
    }
}

/// Whether the macros of tab `id` may run at all: macros are on and, when
/// only signed ones may run, its project is signed by a trusted
/// certificate.
pub(super) fn allowed(settings: &crate::settings::Settings, tabs: &[super::Tab], id: u64) -> bool {
    if !settings.macros_enabled {
        return false;
    }
    !settings.macros_signed_only || tabs.iter().find(|t| t.id == id).is_some_and(|t| signed(&t.view, &settings.trusted_publishers))
}

/// The project carries an intact signature of a trusted certificate or
/// publisher, made for its code as it is now: from its own streams or, in
/// OOXML files, from the signature parts next to it.
fn signed(view: &View, publishers: &[String]) -> bool {
    match view {
        View::Sheet(v) => signed_book(&v.wb, publishers),
        View::Doc(v) => signed_macros(&v.doc.macros, publishers),
        _ => false,
    }
}

/// The signatures of a workbook's project (streams and OOXML parts).
fn signers(wb: &Workbook) -> Vec<apex_sign::vba::VbaSigner> {
    macro_signers(&wb.macros)
}

/// The signatures of a file's project (streams and OOXML parts).
fn macro_signers(macros: &apex_doc::Macros) -> Vec<apex_sign::vba::VbaSigner> {
    let Some(bin) = macros.project.as_ref() else { return Vec::new() };
    let parts = macros.parts.iter().filter(|p| p.rel_type.contains("vbaProjectSignature")).map(|p| p.data.to_vec());
    apex_io::vba::signatures(bin).into_iter().chain(parts).filter_map(|s| apex_sign::vba::check(&s, bin)).collect()
}

/// The certificate of a sound signature of tab `id`'s project whose
/// publisher isn't trusted yet: one the user may choose to trust (never
/// when the organization names the publishers).
pub(super) fn untrusted_publisher(settings: &crate::settings::Settings, tabs: &[super::Tab], id: u64) -> Option<apex_sign::x509::Cert> {
    if crate::policy::locked("TrustedPublishers") {
        return None;
    }
    let all = match &tabs.iter().find(|t| t.id == id)?.view {
        View::Sheet(v) => signers(&v.wb),
        View::Doc(v) => macro_signers(&v.doc.macros),
        _ => return None,
    };
    if all.iter().any(|s| s.is_trusted_with(&settings.trusted_publishers, false)) {
        return None;
    }
    all.into_iter().find(|s| s.is_sound()).map(|s| s.cert)
}

/// [`signed`] for a workbook.
fn signed_book(wb: &Workbook, publishers: &[String]) -> bool {
    signed_macros(&wb.macros, publishers)
}

/// [`signed`] for a file's project.
fn signed_macros(macros: &apex_doc::Macros, publishers: &[String]) -> bool {
    use std::sync::Weak;
    thread_local! {
        /// Projects checked lately (checking reads the whole project
        /// again, and events ask every frame), with the roots trusted then.
        static SEEN: std::cell::RefCell<Vec<(Weak<Vec<u8>>, (usize, u64), bool)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let Some(bin) = macros.project.as_ref() else { return false };
    let only_listed = crate::policy::locked("TrustedPublishers");
    let roots = (apex_sign::trust::roots_added(), {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (publishers, only_listed, revocation::generation()).hash(&mut h);
        h.finish()
    });
    if let Some(ok) = SEEN.with(|s| s.borrow().iter().find(|(w, r, _)| *r == roots && w.upgrade().is_some_and(|b| Arc::ptr_eq(&b, bin))).map(|e| e.2)) {
        return ok;
    }
    let ok = macro_signers(macros).iter().any(|s| s.is_trusted_with(publishers, only_listed) && !revocation::known_revoked(&s.cert));
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

/// The system's dialog for what a macro asks to pick.
fn picker(pick: &apex_vba::files::FilePick) -> crate::file_dialog::FileDialog {
    let mut dlg = crate::file_dialog::FileDialog::new();
    if !pick.title.is_empty() {
        dlg = dlg.set_title(pick.title.clone());
    }
    for (name, patterns) in &pick.filters {
        // `*.txt`: the extension; `*.*` is every file (no filter).
        let exts: Vec<String> = patterns.iter().filter_map(|p| p.strip_prefix("*.")).filter(|e| !e.is_empty() && !e.contains(['*', '?'])).map(String::from).collect();
        if !exts.is_empty() {
            dlg = dlg.add_filter(name.clone(), &exts);
        }
    }
    let initial = Path::new(&pick.initial);
    if initial.is_absolute() {
        if initial.fs_is_dir() {
            dlg = dlg.set_directory(initial);
        } else if let Some(dir) = initial.parent() {
            dlg = dlg.set_directory(dir);
        }
    }
    if !initial.fs_is_dir()
        && let Some(name) = initial.file_name()
    {
        dlg = dlg.set_file_name(name.to_string_lossy());
    }
    dlg
}

fn file_kind(pick: &apex_vba::files::FilePick) -> crate::file_dialog::Kind {
    use apex_vba::files::PickKind;
    use crate::file_dialog::Kind;
    match pick.kind {
        PickKind::Folder => Kind::Folder,
        PickKind::Save => Kind::Save,
        PickKind::Open if pick.multi => Kind::PickMany,
        PickKind::Open => Kind::Pick,
    }
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
        if !self.revocation_allows(id) {
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.hold(idx);
        if let View::Doc(v) = &self.tabs[idx].view {
            let Some(bin) = v.doc.macros.project.clone() else { return };
            let project = parse(&bin).ok();
            self.macro_run.enabled.insert(id, Enabled { events: true, project: Arc::as_ptr(&bin) as usize, handlers: Vec::new() });
            let (event, entry) = if self.macro_run.made_new.remove(&id) { ("New", Entry::New) } else { ("Open", Entry::Open) };
            if project.is_some_and(|p| apex_vba::doc_handler(&p, event).is_some()) {
                self.macro_run.queue = Some((id, entry));
            }
            return;
        }
        let View::Sheet(v) = &self.tabs[idx].view else { return };
        let Some(bin) = v.wb.macros.project.clone() else { return };
        self.macro_run.seen.insert(id, v.wb.edits);
        let project = parse(&bin).ok();
        let handlers = project.as_ref().map(Project::event_names).unwrap_or_default();
        self.macro_run.enabled.insert(id, Enabled { events: true, project: Arc::as_ptr(&bin) as usize, handlers });
        self.events_settled(id);
        if project.is_some_and(|p| p.has_open_handler()) {
            self.macro_run.queue = Some((id, Entry::Open));
        }
    }

    /// Under "signed only", before macros signed by an authority are
    /// enabled: whether its certificate wasn't revoked (see `revocation`).
    /// Publishers trusted by name and self-signed certificates aren't
    /// asked about.
    fn revocation_allows(&mut self, id: u64) -> bool {
        let mode = self.settings.macro_revocation.as_str();
        if !self.settings.macros_signed_only || mode == "off" {
            return true;
        }
        let Some(macros) = self.tabs.iter().find(|t| t.id == id).and_then(|t| match &t.view {
            View::Sheet(v) => Some(&v.wb.macros),
            View::Doc(v) => Some(&v.doc.macros),
            _ => None,
        }) else {
            return true;
        };
        let signers = macro_signers(macros);
        if signers.iter().any(|s| s.is_trusted_with(&self.settings.trusted_publishers, true)) {
            return true;
        }
        let Some(s) = signers.iter().find(|s| s.is_trusted() && !s.cert.self_issued()) else { return true };
        match revocation::verdict(&s.chain) {
            revocation::Verdict::Good => true,
            revocation::Verdict::Revoked => {
                self.notify(Notice::error(trf("macros.revoked", &[("name", &s.cert.subject.display())])));
                false
            }
            revocation::Verdict::NoAnswer if mode == "require" => {
                self.notify(Notice::error(tr("macros.revocation_required")));
                false
            }
            revocation::Verdict::NoAnswer => {
                self.notify(Notice::info(tr("macros.revocation_unchecked")));
                true
            }
        }
    }

    /// Developer > Macros (Alt+F8): the macros of the active workbook.
    pub(super) fn open_run_macro(&mut self) {
        let active = self.active;
        let Some(id) = self.tabs.get(active).map(|t| t.id) else { return };
        self.flush_macros(id);
        self.hold(active);
        let Some(t) = self.tabs.get(active) else { return };
        if !matches!(t.view, View::Sheet(_) | View::Doc(_) | View::Db(_)) {
            self.notify(Notice::info(tr("macros.sheets_only")));
            return;
        }
        let sheet = matches!(t.view, View::Sheet(_));
        let (mut names, calls, error) = match macro_project(&t.view).map(|b| parse(b)) {
            Some(Ok(p)) => (p.macros(), p.with_arguments(), None),
            Some(Err(e)) => (Vec::new(), Vec::new(), Some(e)),
            None => (Vec::new(), Vec::new(), None),
        };
        let file = t.name();
        // The personal workbook's and add-ins' macros work on workbooks.
        if sheet {
            names.extend(self.library_macros(&file));
        }
        self.macro_run.picker = Some(Picker { tab: id, names, calls, selected: 0, error, explained: None });
    }

    /// Runs macro `name` of tab `id` (asking first when needed).
    pub(super) fn run_macro_named(&mut self, id: u64, name: String) {
        self.request_run(id, Entry::Macro(name));
    }

    /// Runs macro `name` of tab `id` paused at its first statement (F8).
    pub(super) fn step_macro_named(&mut self, id: u64, name: String) {
        self.request_run(id, Entry::Step(name));
    }

    /// A line of the Immediate window of tab `id`, with no macro running.
    pub(super) fn run_immediate(&mut self, id: u64, line: String) {
        self.request_run(id, Entry::Immediate(line));
    }

    /// An AI assistant's run_macro: only a macro of the file itself, whose
    /// macros the user enabled; it starts (its dialogs show as usual) and
    /// isn't waited for.
    pub(super) fn mcp_run_macro(&mut self, id: u64, name: &str, args: Vec<String>) -> Result<(), String> {
        if self.is_protected(id) {
            return Err("the file is in Protected View: its macros don't run".into());
        }
        if name.contains('!') {
            return Err("only the file's own macros can be run".into());
        }
        if !(allowed(&self.settings, &self.tabs, id) && self.macro_run.is_enabled(id)) {
            return Err("this file's macros aren't enabled: the user must enable them in ApexDesk first (the macro bar)".into());
        }
        if self.macro_run.job.is_some() || self.macro_run.queue.is_some() || self.macro_run.consent.is_some() {
            return Err("a macro is already running".into());
        }
        self.macro_run.queue = Some((id, if args.is_empty() { Entry::Macro(name.to_string()) } else { Entry::Call(name.to_string(), args) }));
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn macro_run_take_queue_for_test(&mut self) {
        self.macro_run.queue = None;
    }

    fn request_run(&mut self, id: u64, entry: Entry) {
        if self.is_protected(id) {
            self.notify(Notice::info(tr("protected.macros")));
            return;
        }
        // A macro of the personal workbook or an add-in: the user's own
        // (or installed) code, loaded only when macros may run; it doesn't
        // run this file's code.
        let library = match &entry {
            Entry::Macro(n) | Entry::Call(n, _) => self.is_library_macro(id, n),
            _ => false,
        };
        if library || (allowed(&self.settings, &self.tabs, id) && self.macro_run.is_enabled(id)) {
            self.macro_run.queue = Some((id, entry));
        } else {
            self.macro_run.consent = Some((id, entry));
        }
    }

    /// Starts a queued macro, handles what a running one asks, fires
    /// Worksheet_Change after the user's edits and shows the dialogs.
    pub(super) fn macro_frame(&mut self, ctx: &egui::Context) {
        if self.macro_run.job.is_none() {
            self.sync_udfs();
            // A form button clicked.
            let asked = match self.tabs.get_mut(self.active).map(|t| (t.id, &mut t.view)) {
                Some((id, View::Sheet(v))) => v.run_macro.take().map(|m| (id, m)),
                Some((id, View::Db(v))) => v.run_macro.take().map(|(name, form)| {
                    // The form is the macro's Me.
                    self.macro_run.db_form = Some((id, name.clone(), form));
                    (id, name)
                }),
                _ => None,
            };
            if let Some((id, name)) = asked {
                self.run_macro_named(id, name);
            }
            self.sheet_events(ctx);
            self.macro_timers(ctx);
            if let Some((id, entry)) = self.macro_run.queue.take() {
                self.start_macro(ctx, id, entry, None);
            } else if self.macro_run.consent.is_none()
                && let Some((id, entry, after)) = self.next_events()
            {
                self.start_macro(ctx, id, entry, after);
            }
        }
        self.poll_macro();
        self.macro_job_ui(ctx);
        self.run_macro_dialog(ctx);
        self.macro_args_dialog(ctx);
        self.macro_shortcut_dialog(ctx);
        self.record_frame(ctx);
        self.consent_dialog(ctx);
        self.trust_publisher_dialog(ctx);
        self.macro_error_dialog(ctx);
        self.trial_dialog(ctx);
    }

    fn start_macro(&mut self, ctx: &egui::Context, id: u64, entry: Entry, after: Option<events::After>) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        if matches!(self.tabs[idx].view, View::Doc(_)) {
            return self.start_doc_macro(ctx, idx, entry);
        }
        if matches!(self.tabs[idx].view, View::Db(_)) {
            return self.start_db_macro(ctx, idx, entry);
        }
        if let Entry::Macro(n) = &entry {
            self.record_run(id, n);
        }
        self.flush_macros(id);
        self.hold(idx);
        let file_name = self.tabs[idx].name();
        let file_path = self.tabs[idx].path.as_ref().and_then(|p| p.parent()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let libraries = self.run_targets(id);
        // A workbook in a trusted location reaches files outside its folder.
        let trusted_book = self.tabs[idx].path.as_deref().is_some_and(|p| trusted(p, &self.settings.trusted_locations));
        let (password, downloaded) = (self.tabs[idx].password.clone(), self.tabs[idx].downloaded());
        let View::Sheet(v) = &mut self.tabs[idx].view else { return };
        let bin = v.wb.macros.project.clone();
        let grant = self.tabs[idx].path.as_deref().zip(bin.as_deref()).map(|(p, b)| grant_key(p, b));
        let granted = grant.as_ref().is_some_and(|g| self.settings.macro_file_grants.contains(g));
        let mut grant = grant.filter(|_| !granted);
        let View::Sheet(v) = &mut self.tabs[idx].view else { return };
        let (sheet, selection) = v.macro_context();
        let mut wb = v.wb.clone();
        // The recorder writes what the user does, not what macros do.
        wb.recorder = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let interrupt = Arc::new(AtomicBool::new(false));
        let conf = self.macro_run.debug.entry(id).or_default();
        let debug = apex_vba::debug::Debugger { breakpoints: conf.breakpoints.clone(), watches: conf.watches.clone(), step: matches!(entry, Entry::Step(_)) };
        let mut opts = Options {
            active_sheet: sheet,
            selection,
            file_name,
            file_path,
            user_name: crate::doc_view::user_name(),
            utc_offset: crate::common::local_utc_offset(),
            // The thread has 64 MiB of stack.
            max_stack: 48 << 20,
            cancel: Some(cancel.clone()),
            interrupt: Some(interrupt.clone()),
            debug: Some(debug),
            libraries,
            files: apex_vba::files::FileAccess { trusted: trusted_book, write: granted.then_some(true) },
            ..Options::default()
        };
        let trial = self.macro_run.trial.take_if(|(t, n)| *t == id && matches!(&entry, Entry::Macro(m) if m == n)).map(|_| Arc::new(Mutex::new(Vec::new())));
        if trial.is_some() {
            // It asks about each folder (and is refused), and doesn't
            // pause: the tab would show its workbook.
            opts.files.write = None;
            opts.debug = None;
            grant = None;
        }
        if library_macro(&entry, &opts).is_some() {
            // The breakpoints are in this file's code, and what it may do
            // with files is for this file's code.
            opts.debug = None;
            opts.files.write = None;
            grant = None;
        } else if bin.is_none() {
            return;
        }
        let mods = bin.as_deref().and_then(|b| sources(b).ok()).map(|s| s.into_iter().map(|s| (s.name, s.code)).collect::<Vec<_>>()).unwrap_or_default();
        let names = debug_ui::casing(&mods);
        let name = match &entry {
            Entry::Macro(n) | Entry::Step(n) | Entry::Call(n, _) => n.clone(),
            Entry::Immediate(_) => tr("debug.immediate").to_string(),
            Entry::Open | Entry::New => "Workbook_Open".to_string(),
            Entry::Events(ev) => ev.first().map(|e| e.name).unwrap_or_default().to_string(),
        };
        let (tx, rx) = mpsc::channel();
        let mut ui = ChannelUi { tx: tx.clone(), ctx: ctx.clone(), password, downloaded, trial: trial.clone() };
        let spawned = apex_fs::thread::Builder::new().name("apex-macro".into()).stack_size(64 << 20).spawn(move || {
            let mut wb = wb;
            let out = run(bin.as_deref().map(Vec::as_slice), &mut wb, &mut ui, &opts, entry);
            let _ = tx.send(Msg::Done(Box::new(wb), Box::new(out)));
            ui.ctx.request_repaint();
        });
        if spawned.is_err() {
            self.notify(Notice::error(tr("macros.crashed")));
            if let Some(a) = after {
                self.event_cancelled(id, a);
            }
            return;
        }
        self.macro_run.failure = None;
        let now = Instant::now();
        self.macro_run.job = Some(Job { tab: id, name, rx, cancel, ask: None, input: String::new(), focus_input: false, dialog_open: false, status: None, started: now, interrupt, pause: None, panel: debug_ui::Panel::default(), resumed: now, original: None, sources: mods, grant, names, after, trial, forms: Vec::new(), form: form_ui::State::default() });
    }

    /// [`Self::start_macro`] for a document: Word's objects on a copy of
    /// it (no libraries: they are spreadsheets' macros).
    fn start_doc_macro(&mut self, ctx: &egui::Context, idx: usize, entry: Entry) {
        let id = self.tabs[idx].id;
        self.flush_macros(id);
        self.hold(idx);
        let file_name = self.tabs[idx].name();
        let file_path = self.tabs[idx].path.as_ref().and_then(|p| p.parent()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let trusted_doc = self.tabs[idx].path.as_deref().is_some_and(|p| trusted(p, &self.settings.trusted_locations));
        let downloaded = self.tabs[idx].downloaded();
        let path = self.tabs[idx].path.clone();
        let View::Doc(v) = &mut self.tabs[idx].view else { return };
        let Some(bin) = v.doc.macros.project.clone() else { return };
        let grant = path.as_deref().map(|p| grant_key(p, &bin));
        let granted = grant.as_ref().is_some_and(|g| self.settings.macro_file_grants.contains(g));
        let mut grant = grant.filter(|_| !granted);
        let sel = v.macro_context();
        let mut doc = v.doc.clone();
        // The macro's edits are one undo step: its copy keeps none.
        doc.trim_undo(0);
        let cancel = Arc::new(AtomicBool::new(false));
        let interrupt = Arc::new(AtomicBool::new(false));
        let conf = self.macro_run.debug.entry(id).or_default();
        let debug = apex_vba::debug::Debugger { breakpoints: conf.breakpoints.clone(), watches: conf.watches.clone(), step: matches!(entry, Entry::Step(_)) };
        let mut opts = Options {
            file_name,
            file_path,
            user_name: crate::doc_view::user_name(),
            utc_offset: crate::common::local_utc_offset(),
            // The thread has 64 MiB of stack.
            max_stack: 48 << 20,
            cancel: Some(cancel.clone()),
            interrupt: Some(interrupt.clone()),
            debug: Some(debug),
            files: apex_vba::files::FileAccess { trusted: trusted_doc, write: granted.then_some(true) },
            ..Options::default()
        };
        let trial = self.macro_run.trial.take_if(|(t, n)| *t == id && matches!(&entry, Entry::Macro(m) if m == n)).map(|_| Arc::new(Mutex::new(Vec::new())));
        if trial.is_some() {
            // As for a workbook's: no file changes, no pauses.
            opts.files.write = None;
            opts.debug = None;
            grant = None;
        }
        let mods = sources(&bin).map(|s| s.into_iter().map(|s| (s.name, s.code)).collect::<Vec<_>>()).unwrap_or_default();
        let names = debug_ui::casing(&mods);
        let name = match &entry {
            Entry::Macro(n) | Entry::Step(n) | Entry::Call(n, _) => n.clone(),
            Entry::Immediate(_) => tr("debug.immediate").to_string(),
            Entry::Open => "Document_Open".to_string(),
            Entry::New => "Document_New".to_string(),
            Entry::Events(_) => return,
        };
        let (tx, rx) = mpsc::channel();
        let mut ui = ChannelUi { tx: tx.clone(), ctx: ctx.clone(), password: None, downloaded, trial: trial.clone() };
        let spawned = apex_fs::thread::Builder::new().name("apex-macro".into()).stack_size(64 << 20).spawn(move || {
            let mut doc = doc;
            let out = run_doc(&bin, &mut doc, sel, &mut ui, &opts, entry);
            let _ = tx.send(Msg::DocDone(Box::new(doc), Box::new(out)));
            ui.ctx.request_repaint();
        });
        if spawned.is_err() {
            self.notify(Notice::error(tr("macros.crashed")));
            return;
        }
        self.macro_run.failure = None;
        let now = Instant::now();
        self.macro_run.job = Some(Job { tab: id, name, rx, cancel, ask: None, input: String::new(), focus_input: false, dialog_open: false, status: None, started: now, interrupt, pause: None, panel: debug_ui::Panel::default(), resumed: now, original: None, sources: mods, grant, names, after: None, trial, forms: Vec::new(), form: form_ui::State::default() });
    }

    /// Takes what the macro's thread sent.
    fn poll_macro(&mut self) {
        loop {
            let Some(job) = self.macro_run.job.as_mut().filter(|j| j.ask.is_none() && j.pause.as_ref().is_none_or(|p| p.sent)) else { return };
            match job.rx.try_recv() {
                Ok(Msg::Status(s)) => job.status = s,
                Ok(Msg::Forms(views)) => job.forms = views,
                Ok(Msg::Print(text)) => {
                    let tab = job.tab;
                    self.macro_run.debug.entry(tab).or_default().print(&text);
                }
                Ok(Msg::Pause { at, wb, reply }) => {
                    let tab = job.tab;
                    let (sheet, sel) = (at.sheet, at.selection);
                    job.pause = Some(debug_ui::Stopped { at, reply, debugging: false, sent: false });
                    // The tab shows the workbook as the macro left it.
                    let keep = job.original.is_none();
                    let Some(idx) = self.tabs.iter().position(|t| t.id == tab) else { continue };
                    self.hold(idx);
                    let old = match (&mut self.tabs[idx].view, wb) {
                        (View::Sheet(v), Shown::Book(wb)) => {
                            let old = std::mem::replace(&mut v.wb, *wb);
                            v.macro_done(None, sheet, sel);
                            Some(Shown::Book(Box::new(old)))
                        }
                        (View::Doc(v), Shown::Doc(doc, sel)) => {
                            let (old, old_sel) = v.show_macro_doc(*doc, sel);
                            Some(Shown::Doc(Box::new(old), old_sel))
                        }
                        _ => None,
                    };
                    if keep && let Some(job) = self.macro_run.job.as_mut() {
                        job.original = old;
                    }
                }
                Ok(Msg::DbDone(out)) => {
                    let Some(j) = self.macro_run.job.take() else { return };
                    self.finish_db_macro(j.tab, *out);
                    return;
                }
                Ok(Msg::DocDone(doc, out)) => {
                    let job = self.macro_run.job.take();
                    let Some(j) = job else { return };
                    self.restore_original(j.tab, j.original);
                    match j.trial {
                        Some(files) => self.finish_doc_trial(j.tab, j.name, *doc, *out, &files),
                        None => self.finish_doc_macro(j.tab, *doc, *out),
                    }
                    return;
                }
                Ok(Msg::Done(wb, out)) => {
                    let job = self.macro_run.job.take();
                    if let Some(j) = job.as_ref().filter(|j| j.trial.is_some()) {
                        let (tab, name) = (j.tab, j.name.clone());
                        let files = j.trial.clone().unwrap_or_default();
                        self.restore_original(tab, job.and_then(|j| j.original));
                        self.finish_trial(tab, name, *wb, *out, &files);
                        return;
                    }
                    let (tab, original, after) = job.map_or((0, None, None), |j| (j.tab, j.original, j.after));
                    self.restore_original(tab, original);
                    let go = out.error.is_none() && !out.cancel;
                    self.finish_macro(tab, Some((*wb, *out)));
                    match after {
                        Some(a) if go => self.event_after(tab, a),
                        Some(a) => self.event_cancelled(tab, a),
                        None => {}
                    }
                    return;
                }
                Ok(m) => {
                    if let Msg::Form { views, .. } = &m {
                        job.forms = views.clone();
                        job.form.reset();
                    }
                    if let Msg::InputBox { default, .. } = &m {
                        job.input = default.clone();
                        job.focus_input = true;
                    }
                    job.ask = Some(m);
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let job = self.macro_run.job.take();
                    let (tab, original, after) = job.map_or((0, None, None), |j| (j.tab, j.original, j.after));
                    self.restore_original(tab, original);
                    self.finish_macro(tab, None);
                    if let Some(a) = after {
                        self.event_cancelled(tab, a);
                    }
                    return;
                }
            }
        }
    }

    /// The tab's workbook as it was before the macro paused (it showed the
    /// macro's while paused).
    fn restore_original(&mut self, id: u64, original: Option<Shown>) {
        let Some(shown) = original else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.hold(idx);
        match (&mut self.tabs[idx].view, shown) {
            (View::Sheet(v), Shown::Book(wb)) => v.wb = *wb,
            (View::Doc(v), Shown::Doc(doc, sel)) => {
                let (copy, _) = v.show_macro_doc(*doc, sel);
                // Nothing the view worked out for the copy is taken for it.
                v.doc.revision = v.doc.revision.max(copy.revision) + 1;
            }
            _ => {}
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
        }
        self.events_settled(id);
        if let Some(en) = self.macro_run.enabled.get_mut(&id) {
            en.events = out.events;
        }
        // Workbook.Close last: after SaveAs and Save.
        let (close, requests): (Vec<_>, Vec<_>) = out.requests.into_iter().partition(|r| matches!(r, apex_vba::Request::Close { .. }));
        self.macro_requests(id, requests);
        if let Some(e) = out.error {
            self.macro_run.failure = Some((id, e));
        }
        // Workbook.Save (no BeforeSave: a handler that saves would fire
        // itself again).
        if out.save
            && let Some(idx) = self.tabs.iter().position(|t| t.id == id)
        {
            self.save_then(idx, false, |_, _| {});
        }
        for r in close {
            self.book_request(id, r);
        }
        self.books_left_open(out.books);
    }

    /// Puts the document the macro changed in its tab, as one undo step.
    fn finish_doc_macro(&mut self, id: u64, doc: apex_doc::Document, out: apex_vba::DocOutcome) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.hold(idx);
        if let View::Doc(v) = &mut self.tabs[idx].view {
            v.macro_done(out.changed.then_some(doc), out.selection);
        }
        if let Some(en) = self.macro_run.enabled.get_mut(&id) {
            en.events = out.events;
        }
        // Document.Close last: after SaveAs and Save.
        let (close, requests): (Vec<_>, Vec<_>) = out.requests.into_iter().partition(|r| matches!(r, apex_vba::Request::Close { .. }));
        self.macro_requests(id, requests);
        if let Some(e) = out.error {
            self.macro_run.failure = Some((id, e));
        }
        if out.save
            && let Some(idx) = self.tabs.iter().position(|t| t.id == id)
        {
            self.save_then(idx, false, |_, _| {});
        }
        for r in close {
            self.book_request(id, r);
        }
        self.docs_left_open(out.docs);
    }

    /// Documents a macro opened or added and left open, each in a tab:
    /// one it didn't change opened from its file like any other.
    fn docs_left_open(&mut self, docs: Vec<apex_vba::OpenDoc>) {
        for d in docs {
            let path = d.path.map(PathBuf::from);
            match path {
                Some(p) if !d.doc.modified => self.open_path(&p),
                p => {
                    let format = p.as_deref().and_then(apex_io::Format::from_path);
                    let macros = d.doc.macros.has_code();
                    self.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(d.doc))), p.clone(), format);
                    self.left_open(p.as_deref(), macros);
                }
            }
        }
    }

    /// Workbooks a macro opened and left open, each in a tab: one it
    /// didn't change opened from its file like any other.
    fn books_left_open(&mut self, books: Vec<apex_vba::OpenBook>) {
        for b in books {
            let path = b.path.map(PathBuf::from);
            match path {
                Some(p) if !b.wb.modified => self.open_path(&p),
                p => {
                    let format = p.as_deref().and_then(apex_io::Format::from_path);
                    let macros = b.wb.macros.has_code();
                    self.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(b.wb))), p.clone(), format);
                    self.left_open(p.as_deref(), macros);
                }
            }
        }
    }

    /// The tab just made of a file a macro changed, as when the user opens
    /// it: a downloaded one in Protected View, its own macros waiting.
    fn left_open(&mut self, path: Option<&Path>, macros: bool) {
        if path.is_some_and(|p| self.should_protect(p)) {
            self.protect_last();
        }
        if macros && let Some(id) = self.tabs.last().map(|t| t.id) {
            self.macros.note(id);
        }
    }

    /// MsgBox, InputBox, or the Stop button of a running macro.
    fn macro_job_ui(&mut self, ctx: &egui::Context) {
        if self.debugger_ui(ctx) {
            return;
        }
        let Some(job) = &mut self.macro_run.job else { return };
        // The forms stay under what their code shows.
        if !job.forms.is_empty() && !matches!(job.ask, Some(Msg::Form { .. })) {
            form_ui::show(ctx, &job.forms, &mut job.form, false);
        }
        let modal = egui::Modal::new(egui::Id::new("macro_job"));
        match &job.ask {
            Some(Msg::Form { views, reply }) => {
                if let Some(ev) = form_ui::show(ctx, views, &mut job.form, true) {
                    let _ = reply.send(Some(ev));
                    job.ask = None;
                    job.resumed = Instant::now();
                }
            }
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
            Some(Msg::AllowFiles { folder, reply }) => {
                let (mut answer, mut remember) = (None, false);
                let m = modal.show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    ui.set_max_width(520.0);
                    ui.heading(tr("macros.files_title"));
                    ui.add_space(8.0);
                    ui.label(trf("macros.files_ask", &[("folder", folder)]));
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("macros.files_deny")).clicked() {
                            answer = Some(false);
                        }
                        if ui.button(tr("macros.files_allow")).clicked() {
                            answer = Some(true);
                        }
                        if job.grant.is_some() && ui.button(tr("macros.files_always")).on_hover_text(tr("macros.files_always_tip")).clicked() {
                            answer = Some(true);
                            remember = true;
                        }
                    });
                });
                // No key allows it: only the button.
                if answer.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
                if let Some(a) = answer {
                    let _ = reply.send(a);
                    job.ask = None;
                }
                if remember && let Some(g) = job.grant.take() {
                    let grants = &mut self.settings.macro_file_grants;
                    if grants.len() >= crate::settings::MAX_GRANTS {
                        grants.remove(0);
                    }
                    grants.push(g);
                    self.settings.save();
                }
            }
            Some(Msg::PickFiles { pick, reply }) => {
                if !job.dialog_open {
                    job.dialog_open = true;
                    let reply = reply.clone();
                    picker(pick).ask(file_kind(pick), move |app: &mut App, paths: Option<Vec<std::path::PathBuf>>| {
                        let _ = reply.send(paths.map(|v| v.into_iter().map(|p| p.to_string_lossy().into_owned()).collect()));
                        if let Some(job) = app.macro_run.job.as_mut()
                            && matches!(job.ask, Some(Msg::PickFiles { .. }))
                        {
                            job.ask = None;
                            job.dialog_open = false;
                        }
                    });
                }
                // The workbook waits for the dialog.
                modal.show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("macros.picking"));
                    });
                });
            }
            _ => {
                // Blocks the window while it runs; says so after a moment
                // (most macros end before).
                let shown = job.started.elapsed().min(job.resumed.elapsed()) > Duration::from_millis(400);
                let modal = if shown { modal } else { modal.backdrop_color(egui::Color32::TRANSPARENT).frame(egui::Frame::NONE) };
                let (name, status) = (job.name.clone(), job.status.clone());
                let (mut stop, mut pause) = (false, false);
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
                        pause = ui.button(tr("debug.break")).on_hover_text(tr("debug.break_tip")).clicked();
                    });
                });
                // Ctrl+Break (Esc) stops it, like in Excel.
                if stop || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                // Excel's Ctrl+Break: the debugger at the next statement.
                if pause {
                    job.interrupt.store(true, Ordering::Relaxed);
                }
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    /// The arguments of a Sub or Function picked in Run Macro.
    fn macro_args_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.macro_run.args else { return };
        let (mut ok, mut cancel) = (false, false);
        let m = egui::Modal::new(egui::Id::new("macro_args")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(&d.name);
            ui.add_space(6.0);
            egui::Grid::new("macro_args_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                for (i, (p, v)) in d.params.iter().zip(d.values.iter_mut()).enumerate() {
                    ui.label(p);
                    let r = ui.add(egui::TextEdit::singleline(v).desired_width(240.0));
                    if i == 0 && ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                    ui.end_row();
                }
            });
            ui.label(RichText::new(tr("macros.args_tip")).weak());
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel = ui.button(tr("dialog.cancel")).clicked();
                ok = ui.button(RichText::new(tr("macros.run")).strong()).clicked();
            });
        });
        if m.is_top_modal && !std::mem::take(&mut d.fresh) {
            ok |= ctx.input(|i| i.key_pressed(egui::Key::Enter));
            cancel |= ctx.input(|i| i.key_pressed(egui::Key::Escape));
        }
        if !(ok || cancel) {
            return;
        }
        let Some(d) = self.macro_run.args.take() else { return };
        if ok {
            self.request_run(d.tab, Entry::Call(d.name, d.values));
        }
    }

    /// Developer > Macros: pick one and run it.
    /// Reads what the macro selected in Run Macro does, once per selection.
    fn explain_selected(&mut self) {
        let Some(p) = &self.macro_run.picker else { return };
        if p.error.is_some() || p.explained.as_ref().is_some_and(|(i, _)| *i == p.selected) {
            return;
        }
        let name = p.names.get(p.selected).cloned().or_else(|| p.selected.checked_sub(p.names.len()).and_then(|i| p.calls.get(i)).map(|c| c.0.clone()));
        let project = self.tabs.iter().find(|t| t.id == p.tab).and_then(|t| macro_project(&t.view)).and_then(|b| apex_io::vba::read(b).ok());
        let lines = match (name, project) {
            // A library's macro isn't this workbook's code.
            (Some(n), Some(project)) if !n.contains('!') => {
                let modules: Vec<(&str, &str)> = project.modules.iter().map(|m| (m.name.as_str(), m.code.as_str())).collect();
                super::macro_explain::lines(&apex_vba::explain::explain(&modules, &n))
            }
            _ => Vec::new(),
        };
        let selected = p.selected;
        if let Some(p) = &mut self.macro_run.picker {
            p.explained = Some((selected, lines));
        }
    }

    fn run_macro_dialog(&mut self, ctx: &egui::Context) {
        self.explain_selected();
        let Some(p) = &mut self.macro_run.picker else { return };
        if !self.tabs.iter().any(|t| t.id == p.tab) {
            self.macro_run.picker = None;
            return;
        }
        let enabled = allowed(&self.settings, &self.tabs, p.tab);
        let why = if self.settings.macros_enabled { tr("macros.unsigned") } else { tr("macros.disabled") };
        let (mut run, mut edit, mut close, mut options, mut shortcut, mut try_it) = (false, false, false, false, false, false);
        let library = p.names.get(p.selected).is_some_and(|n| n.contains('!'));
        let m = egui::Modal::new(egui::Id::new("run_macro")).show(ctx, |ui| {
            ui.set_width(560.0f32.min(crate::widgets::dialog_room(ctx).x));
            ui.heading(tr("macros.run_macro"));
            ui.add_space(6.0);
            if let Some(e) = &p.error {
                ui.label(RichText::new(trf("macros.error_at", &[("module", &e.module), ("line", &e.line)])).strong());
                ui.label(RichText::new(e.to_string()).color(ui.visuals().error_fg_color));
            } else if p.names.is_empty() && p.calls.is_empty() {
                ui.label(tr("macros.no_macros"));
            } else {
                ui.label(tr("macros.run_list"));
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        let calls = p.calls.iter().map(|(n, a)| format!("{n}({})", a.join(", ")));
                        for (i, n) in p.names.iter().cloned().chain(calls).enumerate() {
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
                if let Some((_, lines)) = p.explained.as_ref().filter(|(_, l)| !l.is_empty()) {
                    ui.add_space(6.0);
                    super::macro_explain::show(ui, lines);
                }
            }
            if !enabled {
                ui.add_space(6.0);
                ui.label(RichText::new(why).color(ui.visuals().warn_fg_color));

            }
            ui.add_space(10.0);
            // On a phone the buttons go on more rows.
            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("{}…", tr("macros.settings"))).clicked() {
                    options = true;
                }
                let named = p.error.is_none() && p.names.get(p.selected).is_some_and(|n| !n.contains('!'));
                shortcut = ui.add_enabled(named, egui::Button::new(format!("{}…", tr("macros.options")))).clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true), |ui| {
                    close = ui.button(tr("dialog.close")).clicked();
                    if ui.button(tr("macros.edit")).clicked() {
                        edit = true;
                    }
                    let can = (enabled || library) && p.selected < p.names.len() + p.calls.len();
                    if ui.add_enabled(can, egui::Button::new(RichText::new(tr("macros.run")).strong())).clicked() {
                        run = true;
                    }
                    let named = p.error.is_none() && p.names.get(p.selected).is_some_and(|n| !n.contains('!'));
                    if ui.add_enabled(enabled && named, egui::Button::new(tr("macros.trial"))).on_hover_text(tr("macros.trial_tip")).clicked() {
                        try_it = true;
                    }
                });
            });
        });
        if m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        let count = p.names.len() + p.calls.len();
        if m.is_top_modal && count > 0 {
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                p.selected = (p.selected + 1).min(count - 1);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                p.selected = p.selected.saturating_sub(1);
            }
        }
        let library = p.names.get(p.selected).is_some_and(|n| n.contains('!'));
        if m.is_top_modal && p.selected < p.names.len() + p.calls.len() && (enabled || library) && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            run = true;
        }
        run &= enabled || library;
        let tab = p.tab;
        let name = p.names.get(p.selected).cloned();
        let call = p.selected.checked_sub(p.names.len()).and_then(|i| p.calls.get(i)).cloned();
        if try_it && let Some(name) = name.clone() {
            self.macro_run.picker = None;
            self.macro_run.trial = Some((tab, name.clone()));
            self.run_macro_named(tab, name);
            return;
        }
        if run || edit || close || options || shortcut {
            self.macro_run.picker = None;
        }
        if shortcut && let Some(name) = name.clone() {
            self.open_macro_shortcut(tab, name);
        }
        if options {
            self.open_macro_options();
        }
        if edit {
            self.open_macros();
        }
        if run && let Some(name) = name {
            self.run_macro_named(tab, name);
        } else if run && let Some((name, params)) = call {
            let values = vec![String::new(); params.len()];
            self.macro_run.args = Some(ArgsDlg { tab, name, params, values, fresh: true });
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
        let danger = if enabled { super::macros_ui::danger_text(&self.macro_signals(id)) } else { None };
        let (mut choice, mut options) = (None, false);
        let m = egui::Modal::new(egui::Id::new("macro_consent")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("macros.consent_title"));
            ui.add_space(8.0);
            ui.label(if enabled { trf("macros.consent", &[("file", &file)]) } else { why.to_string() });
            if let Some(d) = &danger {
                ui.add_space(8.0);
                ui.label(RichText::new(d).strong().color(ui.visuals().error_fg_color));
            }

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
            if let Some(key) = unsupported_hint(e) {
                ui.add_space(6.0);
                ui.label(RichText::new(tr(key)).color(ui.visuals().weak_text_color()));
            }
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

/// What to do instead, when the macro used something ApexDesk doesn't
/// run (the error says what).
fn unsupported_hint(e: &apex_vba::Error) -> Option<&'static str> {
    if !matches!(e.number, 445 | 429) {
        return None;
    }
    let m = e.message.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| m.contains(w));
    Some(if e.number == 429 || has(&["shell", "declare", "getobject"]) {
        "macros.alt_programs"
    } else if has(&["sendkeys"]) {
        "macros.alt_sendkeys"
    } else if has(&["registry"]) {
        "macros.alt_registry"
    } else if has(&["userforms"]) {
        "macros.alt_userforms"
    } else if has(&["querytables", "external data"]) {
        "macros.alt_query"
    } else if has(&["worksheet.sort", "worksheet.autofilter"]) {
        "macros.alt_sort"
    } else if has(&["buttons", "checkboxes", "dropdowns", "optionbuttons", "listboxes", "oleobjects", "drawingobjects"]) {
        "macros.alt_controls"
    } else if has(&["application.quit"]) {
        "macros.alt_quit"
    } else if has(&["commandbars", "application.vbe"]) {
        "macros.alt_menus"
    } else if m.contains('(') {
        // The message says what to do already.
        return None;
    } else if has(&["workbooks"]) {
        "macros.alt_workbooks"
    } else {
        "macros.alt_generic"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document's AutoOpen runs on a copy, which comes back as one undo
    /// step, also when it fails.
    #[test]
    fn a_documents_macros_run_as_one_undo_step() {
        let ctx = egui::Context::default();
        let settings = crate::settings::Settings { macros_enabled: true, ..Default::default() };
        let mut app = App::blank(&ctx, settings, None, Instant::now());
        let mut doc = apex_doc::Document::from_plain_text("Hola");
        let mut p = apex_io::vba::read(&apex_io::vba::new_document_project()).unwrap();
        let mut m = apex_io::vba::VbaModule::standard("Module1");
        m.code = "Sub AutoOpen()\n    ActiveDocument.Content.InsertAfter \" mundo\"\n    ActiveDocument.Paragraphs(1).Range.Font.Bold = True\nEnd Sub\n\nSub Boom()\n    ActiveDocument.Content.InsertBefore \"X\"\n    Err.Raise 5\nEnd Sub\n".into();
        p.modules.push(m);
        doc.macros.set_project(p.write().unwrap());
        app.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(doc))), None, None);
        let id = app.tabs[0].id;
        let text = |app: &App| match &app.tabs[0].view {
            View::Doc(v) => v.doc.shared().to_plain_text(),
            _ => unreachable!(),
        };
        let wait = |app: &mut App| {
            let t = Instant::now();
            while app.macro_run.job.is_some() {
                assert!(t.elapsed() < Duration::from_secs(20), "the macro never ended");
                apex_fs::thread::sleep(Duration::from_millis(5));
                app.poll_macro();
            }
        };
        app.enable_macros(id);
        let (tab, entry) = app.macro_run.queue.take().expect("AutoOpen runs");
        assert!(matches!(entry, Entry::Open));
        app.start_macro(&ctx, tab, entry, None);
        wait(&mut app);
        assert!(app.macro_run.failure.is_none());
        assert_eq!(text(&app).trim_end(), "Hola mundo");
        let View::Doc(v) = &mut app.tabs[0].view else { unreachable!() };
        assert!(v.doc.modified);
        v.undo();
        assert_eq!(text(&app).trim_end(), "Hola");
        // A failing macro stops in the debugger; ended there, it keeps
        // what it did until then (undoable too).
        app.run_macro_named(id, "Boom".into());
        let (tab, entry) = app.macro_run.queue.take().expect("enabled: no question");
        app.start_macro(&ctx, tab, entry, None);
        assert!(doc_wait(&mut app));
        assert_eq!(text(&app).trim_end(), "XHola");
        let stop = app.macro_run.job.as_mut().unwrap().pause.as_mut().unwrap();
        assert!(matches!(&stop.at.reason, apex_vba::debug::Reason::Error(e) if e.number == 5));
        stop.reply.send(apex_vba::debug::DebugCmd::End).unwrap();
        stop.sent = true;
        wait(&mut app);
        assert!(app.macro_run.failure.is_none());
        assert_eq!(text(&app).trim_end(), "XHola");
        let View::Doc(v) = &mut app.tabs[0].view else { unreachable!() };
        v.undo();
        assert_eq!(text(&app).trim_end(), "Hola");
    }

    /// An app with a document whose Module1 has `code`, macros enabled.
    fn doc_app(code: &str) -> (egui::Context, App, u64) {
        doc_app_new(code, false)
    }

    /// `made_new`: a document made from a .dotm template.
    fn doc_app_new(code: &str, made_new: bool) -> (egui::Context, App, u64) {
        let ctx = egui::Context::default();
        let settings = crate::settings::Settings { macros_enabled: true, ..Default::default() };
        let mut app = App::blank(&ctx, settings, None, Instant::now());
        let mut doc = apex_doc::Document::from_plain_text("Hola");
        let mut p = apex_io::vba::read(&apex_io::vba::new_document_project()).unwrap();
        let mut m = apex_io::vba::VbaModule::standard("Module1");
        m.code = code.into();
        p.modules.push(m);
        doc.macros.set_project(p.write().unwrap());
        app.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(doc))), None, None);
        let id = app.tabs[0].id;
        if made_new {
            app.macro_run.made_new.insert(id);
        }
        app.enable_macros(id);
        (ctx, app, id)
    }

    fn doc_text(app: &App) -> String {
        match &app.tabs[0].view {
            View::Doc(v) => v.doc.shared().to_plain_text().trim_end().to_string(),
            _ => unreachable!(),
        }
    }

    /// Polls until the macro ends (false) or pauses (true).
    fn doc_wait(app: &mut App) -> bool {
        let t = Instant::now();
        while app.macro_run.job.is_some() {
            assert!(t.elapsed() < Duration::from_secs(20), "the macro never ended");
            app.poll_macro();
            if app.macro_run.job.as_ref().is_some_and(|j| j.pause.as_ref().is_some_and(|p| !p.sent)) {
                return true;
            }
            apex_fs::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_documents_macro_in_the_debugger() {
        let (ctx, mut app, id) = doc_app("Sub Main()\n    ActiveDocument.Content.InsertAfter \" uno\"\n    ActiveDocument.Content.InsertAfter \" dos\"\nEnd Sub\n");
        app.macro_run.debug.entry(id).or_default().toggle("Module1", 3);
        app.start_macro(&ctx, id, Entry::Macro("Main".into()), None);
        assert!(doc_wait(&mut app));
        // The tab shows the document as the macro left it so far.
        assert_eq!(doc_text(&app), "Hola uno");
        let job = app.macro_run.job.as_mut().unwrap();
        let stop = job.pause.as_mut().unwrap();
        assert_eq!((stop.at.module.as_str(), stop.at.line), ("Module1", 3));
        stop.reply.send(apex_vba::debug::DebugCmd::Continue).unwrap();
        stop.sent = true;
        assert!(!doc_wait(&mut app));
        assert_eq!(doc_text(&app), "Hola uno dos");
        // One undo step, back to the document as it was.
        let View::Doc(v) = &mut app.tabs[0].view else { unreachable!() };
        v.undo();
        assert_eq!(doc_text(&app), "Hola");
        // Stepping from the start, ending it there: nothing changes.
        app.step_macro_named(id, "Main".into());
        let (tab, entry) = app.macro_run.queue.take().expect("enabled: no question");
        app.start_macro(&ctx, tab, entry, None);
        assert!(doc_wait(&mut app));
        let stop = app.macro_run.job.as_mut().unwrap().pause.as_mut().unwrap();
        assert_eq!(stop.at.line, 2);
        stop.reply.send(apex_vba::debug::DebugCmd::End).unwrap();
        stop.sent = true;
        assert!(!doc_wait(&mut app));
        assert_eq!(doc_text(&app), "Hola");
        let View::Doc(v) = &app.tabs[0].view else { unreachable!() };
        assert!(v.doc.can_redo());
    }

    #[test]
    fn documents_a_macro_leaves_open() {
        let dir = apex_fs::temp_dir().join(format!("apex-doc-open-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let dir = apex_vba::files::canonical(&dir).unwrap();
        apex_fs::write(dir.join("Other.docx"), apex_io::docx::write(&apex_doc::Document::from_plain_text("Otro"))).unwrap();
        let (ctx, mut app, id) = doc_app("Sub Main()\n    Documents.Add.Content.Text = \"Nuevo\"\n    Set d = Documents.Open(ThisDocument.Path & \"/Other.docx\")\n    d.Content.InsertAfter \" cambiado\"\n    ThisDocument.Content.InsertAfter \" propio\"\nEnd Sub\n");
        app.tabs[0].path = Some(dir.join("Doc1.docm"));
        app.start_macro(&ctx, id, Entry::Macro("Main".into()), None);
        assert!(!doc_wait(&mut app));
        assert!(app.macro_run.failure.is_none(), "{:?}", app.macro_run.failure.as_ref().map(|f| f.1.to_string()));
        assert_eq!(doc_text(&app), "Hola propio");
        let texts: Vec<(String, Option<PathBuf>)> = app.tabs[1..].iter().map(|t| match &t.view {
            View::Doc(v) => (v.doc.shared().to_plain_text().trim_end().to_string(), t.path.clone()),
            _ => unreachable!(),
        }).collect();
        assert_eq!(texts, [("Nuevo".to_string(), None), ("Otro cambiado".to_string(), Some(dir.join("Other.docx")))]);
        // Its file wasn't written.
        assert_eq!(apex_io::docx::read(&apex_fs::read(dir.join("Other.docx")).unwrap()).unwrap().to_plain_text().trim_end(), "Otro");
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_template_makes_a_new_document() {
        let code = "Sub AutoNew()\n    ActiveDocument.Content.InsertAfter \" nuevo\"\nEnd Sub\nSub AutoOpen()\n    ActiveDocument.Content.InsertAfter \" abierto\"\nEnd Sub\n";
        for (made_new, text) in [(true, "Hola nuevo"), (false, "Hola abierto")] {
            let (ctx, mut app, _) = doc_app_new(code, made_new);
            let (tab, entry) = app.macro_run.queue.take().expect("an event to run");
            assert_eq!(matches!(entry, Entry::New), made_new);
            app.start_macro(&ctx, tab, entry, None);
            assert!(!doc_wait(&mut app));
            assert_eq!(doc_text(&app), text);
        }
    }

    #[test]
    fn trying_a_documents_macro() {
        let dir = apex_fs::temp_dir().join(format!("apex-doc-try-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let dir = apex_vba::files::canonical(&dir).unwrap();
        let (ctx, mut app, id) = doc_app("Sub Main()\n    ActiveDocument.Content.InsertAfter \" mundo\"\n    ActiveDocument.SaveAs2 ActiveDocument.Path & \"/copy.docx\"\nEnd Sub\n");
        app.tabs[0].path = Some(dir.join("Doc1.docm"));
        app.macro_run.trial = Some((id, "Main".into()));
        app.start_macro(&ctx, id, Entry::Macro("Main".into()), None);
        assert!(!doc_wait(&mut app));
        // Nothing changed yet; the report lists the paragraph and the
        // folder it wanted to write in.
        assert_eq!(doc_text(&app), "Hola");
        let t = app.macro_run.report.as_ref().expect("a report");
        assert_eq!(t.changes.paras, [(1, "Hola".to_string(), "Hola mundo".to_string())]);
        assert_eq!(t.files, [dir.display().to_string()]);
        assert!(!dir.join("copy.docx").fs_exists());
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn hints_for_what_isnt_supported() {
        let hint = |e: apex_vba::Error| unsupported_hint(&e);
        assert_eq!(hint(apex_vba::Error::unsupported("Shell")), Some("macros.alt_programs"));
        assert_eq!(hint(apex_vba::Error::new(429, "ActiveX component can't create object: Outlook.Application")), Some("macros.alt_programs"));
        assert_eq!(hint(apex_vba::Error::unsupported("Application.workbooks")), Some("macros.alt_workbooks"));
        assert_eq!(hint(apex_vba::Error::unsupported("Worksheet.querytables")), Some("macros.alt_query"));
        assert_eq!(hint(apex_vba::Error::unsupported("SaveAs with a Password (set it in File > Protect)")), None);
        assert_eq!(hint(apex_vba::Error::unsupported("Chart.export")), Some("macros.alt_generic"));
        assert_eq!(hint(apex_vba::Error::new(9, "Subscript out of range")), None);
    }

    #[test]
    fn a_try_changes_no_file() {
        use apex_vba::Ui;
        let (tx, rx) = mpsc::channel();
        let noted = Arc::new(Mutex::new(Vec::new()));
        let mut ui = ChannelUi { tx, ctx: egui::Context::default(), password: None, downloaded: false, trial: Some(noted.clone()) };
        assert!(!ui.allow_file_changes("/tmp/x"));
        assert!(!ui.allow_file_changes("/tmp/x"));
        // Nobody was asked.
        assert!(rx.try_recv().is_err());
        assert_eq!(*noted.lock().unwrap(), ["/tmp/x"]);
    }

    #[test]
    fn file_grants_follow_the_code() {
        let p = Path::new("/nonexistent/book.xlsm");
        let a = grant_key(p, b"code");
        assert_eq!(a.len(), 64);
        assert_eq!(a, grant_key(p, b"code"));
        assert_ne!(a, grant_key(p, b"code2"));
        assert_ne!(a, grant_key(Path::new("/nonexistent/other.xlsm"), b"code"));
    }

    #[test]
    fn trusted_folders() {
        let base = apex_fs::temp_dir().join("apex_trust_test");
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
