//! A basic VBA interpreter for the macros of spreadsheets: the language
//! (types, Dim/Const, If/Select, loops, Sub/Function with ByVal/ByRef,
//! On Error, With, arrays, Collection and Dictionary), VBA's usual
//! functions and the most used part of Excel's object model (Application,
//! Workbook, Worksheets, Range/Cells, fonts and fills, WorksheetFunction,
//! MsgBox and InputBox through [`Ui`]).
//!
//! What it changes in the workbook is one undo step. A macro stops after
//! [`Options::max_steps`] statements or [`Options::max_time`] of running
//! (time in dialogs doesn't count). UserForms show through
//! [`Ui::form_event`]. `Declare` (Windows API) gives a clear "not
//! supported" error except for a few timing functions acted out (see
//! `api`), and so does automating other applications.

pub mod access;
pub mod ast;
mod api;
mod builtins;
pub mod catalog;
mod compile;
mod consts;
pub mod debug;
pub mod error;
mod excel;
pub mod explain;
pub mod files;
pub mod forms;
mod form_write;
pub mod libraries;
mod interp;
mod lexer;
mod modules;
pub mod object;
pub mod suspicious;
mod parser;
pub mod udf;
pub mod value;
mod word;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use web_time::Duration;

use apex_sheet::{CellRef, Range, Workbook};

pub use ast::ModuleKind;
pub use error::Error;
pub use excel::books::{OpenBook, OpenDoc};
pub use files::DocFormat;
pub use interp::Owner;

use ast::{Arg, Expr, Module, ProcKind};
use interp::Interp;
use object::{Obj, RangeRef};
use value::Value;

/// The code of one module.
#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub kind: ModuleKind,
    pub code: String,
}

/// The modules of a VBA project, parsed.
#[derive(Clone, Debug)]
pub struct Project {
    pub modules: Vec<Module>,
}

/// The dialogs and outputs a macro uses.
pub trait Ui {
    /// MsgBox: returns the button pressed (vbOK = 1, vbCancel = 2, vbYes = 6, vbNo = 7...).
    fn msg_box(&mut self, prompt: &str, buttons: i32, title: &str) -> i32;
    /// InputBox: the text typed, or `None` when cancelled.
    fn input_box(&mut self, prompt: &str, title: &str, default: &str) -> Option<String>;
    /// Debug.Print.
    fn debug_print(&mut self, _text: &str) {}
    /// Application.StatusBar (None: back to normal).
    fn status(&mut self, _text: Option<&str>) {}
    /// Stopped in the debugger (with [`Options::debug`]), with the workbook
    /// as the macro left it so far: what to do now.
    fn pause(&mut self, _at: &debug::Pause, _wb: &Workbook) -> debug::DebugCmd {
        debug::DebugCmd::Continue
    }
    /// [`Ui::pause`] in a document's macro, with the document and its
    /// selection as the macro left them.
    fn pause_doc(&mut self, _at: &debug::Pause, _doc: &apex_doc::Document, _sel: apex_doc::Selection) -> debug::DebugCmd {
        debug::DebugCmd::Continue
    }
    /// A macro is about to change files in `folder` (write, delete, rename,
    /// make folders): whether it may, for the rest of the run.
    fn allow_file_changes(&mut self, _folder: &str) -> bool {
        false
    }
    /// GetOpenFilename, GetSaveAsFilename, FileDialog: the paths picked
    /// (None: cancelled); the macro may then use them.
    fn pick_files(&mut self, _pick: &files::FilePick) -> Option<Vec<String>> {
        None
    }
    /// Workbook.SaveAs, SaveCopyAs: write `wb` at `path` (already allowed)
    /// in `format`; CSV and tab-separated text take sheet `sheet`.
    fn save_workbook(&mut self, _wb: &Workbook, _path: &str, _format: files::BookFormat, _sheet: usize) -> Result<(), String> {
        Err("saving isn't available here".into())
    }
    /// Workbook.Save and SaveAs of a workbook the macro opened (not its
    /// own: without its password).
    fn save_opened(&mut self, wb: &Workbook, path: &str, format: files::BookFormat, sheet: usize) -> Result<(), String> {
        self.save_workbook(wb, path, format, sheet)
    }
    /// Document.SaveAs: write `doc` at `path` (already allowed) in `format`.
    fn save_document(&mut self, _doc: &apex_doc::Document, _path: &str, _format: files::DocFormat) -> Result<(), String> {
        Err("saving isn't available here".into())
    }
    /// Documents.Open: the document at `path` (already allowed), read.
    fn open_document(&mut self, _path: &str) -> Result<apex_doc::Document, String> {
        Err("opening other documents isn't available here".into())
    }
    /// Workbooks.Open: the workbook at `path` (already allowed), read.
    fn open_workbook(&mut self, _path: &str, _password: Option<&str>) -> Result<Workbook, String> {
        Err("opening other workbooks isn't available here".into())
    }
    /// Whether UserForms can be shown ([`Ui::form_event`]).
    fn can_show_forms(&self) -> bool {
        false
    }
    /// The forms shown (the last over the others; those not `enabled`
    /// wait for a modal one): what the user did in one, once there is
    /// something for the macro (a change, a click, the focus moving).
    /// None: the window went, and the forms with it.
    fn form_event(&mut self, _forms: &[Arc<forms::View>]) -> Option<forms::Event> {
        None
    }
    /// A form was hidden or unloaded: the forms still shown.
    fn forms_hidden(&mut self, _forms: &[Arc<forms::View>]) {}
}

#[derive(Clone, Debug)]
pub struct Options {
    pub active_sheet: usize,
    pub selection: Range,
    /// Workbook.Name and Workbook.Path.
    pub file_name: String,
    pub file_path: String,
    /// Application.UserName.
    pub user_name: String,
    /// Seconds from UTC of local time (Now, Date, Time).
    pub utc_offset: i64,
    pub max_steps: u64,
    pub max_time: Duration,
    /// Bytes of stack calls may use (deep recursion stops with "Out of
    /// stack space" before the thread's stack runs out).
    pub max_stack: usize,
    /// Set from another thread to stop the macro.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Run in the debugger: breakpoints, `Stop`, and errors no `On Error`
    /// handles pause it ([`Ui::pause`]).
    pub debug: Option<debug::Debugger>,
    /// Set from another thread to pause the macro in the debugger (Ctrl+Break);
    /// with no debugger, it stops.
    pub interrupt: Option<Arc<AtomicBool>>,
    /// Other workbooks' code `Application.Run "Book!Macro"` reaches: the
    /// personal macro workbook, add-ins.
    pub libraries: Vec<Library>,
    /// What macros may do with files beyond the workbook's folder.
    pub files: files::FileAccess,
    /// The designs of the UserForms, by module name.
    pub forms: Vec<(String, Arc<forms::Design>)>,
}

/// The code of another workbook (the personal macro workbook, an add-in)
/// that runs on the workbook of the macro: `name` is its file name.
#[derive(Clone, Debug)]
pub struct Library {
    pub name: String,
    /// Its modules (parsed when a macro calls into it).
    pub sources: Arc<Vec<Source>>,
}

impl Library {
    /// Whether `book` (`PERSONAL.XLSM`, `'personal'`) names it.
    pub fn is(&self, book: &str) -> bool {
        same_book(&self.name, book)
    }
}

/// Whether `book` (`Book1.xlsm`, `'book1'`) names the file `name`.
pub fn same_book(name: &str, book: &str) -> bool {
    let book = book.trim().trim_matches('\'');
    let stem = name.rsplit_once('.').map_or(name, |(a, _)| a);
    name.eq_ignore_ascii_case(book) || stem.eq_ignore_ascii_case(book)
}

impl Default for Options {
    fn default() -> Self {
        Options {
            active_sheet: 0,
            selection: Range::single(CellRef::new(0, 0)),
            file_name: "Book1".into(),
            file_path: String::new(),
            user_name: String::new(),
            utc_offset: 0,
            max_steps: 20_000_000,
            max_time: Duration::from_secs(60),
            max_stack: 1 << 20,
            cancel: None,
            debug: None,
            interrupt: None,
            libraries: Vec::new(),
            files: files::FileAccess::default(),
            forms: Vec::new(),
        }
    }
}

/// What running a macro did.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The error that stopped it, with its module and line.
    pub error: Option<Error>,
    pub active_sheet: usize,
    pub selection: Range,
    /// It changed the workbook (one undo step named `undo.macro`).
    pub changed: bool,
    /// Workbook.Save was called.
    pub save: bool,
    /// Application.EnableEvents at the end.
    pub events: bool,
    /// An event handler set its `Cancel` argument.
    pub cancel: bool,
    /// What it asked of the application for later (OnKey, OnTime).
    pub requests: Vec<Request>,
    /// Workbooks it opened and left open.
    pub books: Vec<OpenBook>,
}

/// Something a macro asks the application to do after it ends.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Application.OnKey: `procedure` runs on the key (None: the key does
    /// what it did again; `""`: nothing).
    OnKey { key: Key, procedure: Option<String> },
    /// Application.OnTime: run `procedure` at `at` (a date serial, local
    /// time), not after `latest`; `schedule` false takes such a one away.
    OnTime { at: f64, procedure: String, latest: Option<f64>, schedule: bool },
    /// Workbook.SaveAs wrote the workbook at `path`: it is that file now;
    /// `clean`: nothing changed after.
    SavedAs { path: String, clean: bool },
    /// Workbook.Close: close it (`save` Some: saving its changes or not;
    /// None: asking, if it has any).
    Close { save: Option<bool> },
    /// PrintOut or PrintPreview: the print dialog, for sheet `sheet`
    /// (None: the whole workbook). Nothing prints without the user.
    Print { sheet: Option<usize> },
}

impl Request {
    /// The same, its procedure named as one of `book` (`book!Proc`)
    /// unless it names its book already.
    pub fn in_book(self, book: &str) -> Request {
        let qualify = |p: String| if p.is_empty() || p.contains('!') { p } else { format!("{book}!{p}") };
        match self {
            Request::OnKey { key, procedure } => Request::OnKey { key, procedure: procedure.map(qualify) },
            Request::OnTime { at, procedure, latest, schedule } => Request::OnTime { at, procedure: qualify(procedure), latest, schedule },
            r @ (Request::SavedAs { .. } | Request::Close { .. } | Request::Print { .. }) => r,
        }
    }
}

/// A key with its modifiers, as OnKey and macro shortcuts name it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// A letter or digit (upper case), a sign, or the name of a key:
    /// ENTER, TAB, ESC, F1...F15, UP, DELETE...
    pub name: String,
}

impl Key {
    /// OnKey's notation: `^` Ctrl, `+` Shift, `%` Alt, then a character
    /// or `{NAME}` (`~` is Enter). None when it isn't a key.
    pub fn parse(s: &str) -> Option<Key> {
        let mut k = Key { ctrl: false, shift: false, alt: false, name: String::new() };
        let mut rest = s.trim();
        loop {
            match rest.chars().next()? {
                '^' if rest.len() > 1 => k.ctrl = true,
                '+' if rest.len() > 1 => k.shift = true,
                '%' if rest.len() > 1 => k.alt = true,
                _ => break,
            }
            rest = &rest[1..];
        }
        let name = if let Some(inner) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')).filter(|r| !r.is_empty()) {
            let up = inner.to_ascii_uppercase();
            let named = match up.as_str() {
                "BACKSPACE" | "BS" | "BKSP" => "BACKSPACE",
                "DELETE" | "DEL" => "DELETE",
                "ENTER" | "RETURN" => "ENTER",
                "ESCAPE" | "ESC" => "ESC",
                "BREAK" | "CAPSLOCK" | "CLEAR" | "HELP" | "NUMLOCK" | "SCROLLLOCK" => return None,
                "DOWN" | "UP" | "LEFT" | "RIGHT" | "END" | "HOME" | "INSERT" | "PGDN" | "PGUP" | "TAB" => up.as_str(),
                f if f.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=15).contains(&n)) => f,
                _ if inner.chars().count() == 1 => inner,
                _ => return None,
            };
            named.to_ascii_uppercase()
        } else {
            let mut cs = rest.chars();
            let c = cs.next()?;
            if cs.next().is_some() || c.is_control() {
                return None;
            }
            if c == '~' { "ENTER".to_string() } else { c.to_uppercase().collect() }
        };
        k.name = name;
        Some(k)
    }
}

/// The shortcuts of a module's macros (Macro Options, the recorder):
/// `Attribute Proc.VB_ProcData.VB_Invoke_Func = "q\n14"` is Ctrl+Q, an
/// upper case letter Ctrl+Shift.
pub fn macro_shortcuts(code: &str) -> Vec<(Key, String)> {
    code.lines()
        .filter_map(|l| {
            let l = l.trim();
            l.get(..10).filter(|h| h.eq_ignore_ascii_case("attribute "))?;
            let (lhs, rhs) = l[10..].split_once('=')?;
            let proc = lhs.trim().strip_suffix(".VB_ProcData.VB_Invoke_Func")?.trim();
            if proc.is_empty() || !proc.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            let c = rhs.trim().strip_prefix('"')?.chars().next()?;
            if !c.is_ascii_alphabetic() {
                return None;
            }
            Some((Key { ctrl: true, shift: c.is_ascii_uppercase(), alt: false, name: c.to_ascii_uppercase().to_string() }, proc.to_string()))
        })
        .collect()
}

/// Undo label (i18n key) of a macro's changes.
pub const UNDO_LABEL: &str = "undo.macro";

impl Project {
    /// Parses every module; the first syntax error stops it.
    pub fn parse(sources: &[Source]) -> Result<Project, Error> {
        // Tokens take tens of bytes a character: a project's code of a
        // few hundred KB compressed must not make gigabytes of them.
        const MAX_CODE: usize = 4 << 20;
        if sources.iter().map(|s| s.code.len()).sum::<usize>() > MAX_CODE {
            let big = sources.iter().max_by_key(|s| s.code.len()).map_or("", |s| s.name.as_str());
            return Err(Error::syntax(big, 0, "Project too large"));
        }
        let mut toks = Vec::new();
        for s in sources {
            toks.push(lexer::tokenize(&s.code, &s.name)?);
        }
        let modules = sources.iter().map(|s| format!("module:{}", s.name.to_ascii_lowercase()));
        let enums = std::rc::Rc::new(toks.iter().flat_map(|t| parser::enum_names(t)).chain(modules).collect());
        let mut modules = Vec::new();
        for (s, t) in sources.iter().zip(toks) {
            modules.push(parser::parse_tokens(&s.name, s.kind, t, std::rc::Rc::clone(&enums))?);
        }
        Ok(Project { modules })
    }

    /// Depuración > Compilar: errors found without running (names declared
    /// twice, missing labels, undeclared variables with `Option Explicit`).
    pub fn compile_errors(&self) -> Vec<Error> {
        compile::check(self)
    }

    /// Macros that can be run by name: public Subs without parameters
    /// (`Module.Sub` for those of sheet and workbook modules).
    pub fn macros(&self) -> Vec<String> {
        let mut out = Vec::new();
        for m in &self.modules {
            if !matches!(m.kind, ModuleKind::Standard | ModuleKind::Document) {
                continue;
            }
            for p in &m.procs {
                if p.kind == ProcKind::Sub && p.public && p.params.is_empty() {
                    out.push(if m.kind == ModuleKind::Standard { p.name.clone() } else { format!("{}.{}", m.name, p.name) });
                }
            }
        }
        out
    }

    /// What Run Macro can run asking for arguments: the public Subs with
    /// parameters and the public Functions of standard modules, with the
    /// names of their parameters (an optional one in brackets). Not those
    /// that take arrays or a ParamArray.
    pub fn with_arguments(&self) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        for m in self.modules.iter().filter(|m| m.kind == ModuleKind::Standard) {
            for p in &m.procs {
                let callable = p.public && ((p.kind == ProcKind::Sub && !p.params.is_empty()) || p.kind == ProcKind::Function);
                if callable && !p.params.iter().any(|a| a.array || a.paramarray) {
                    out.push((p.name.clone(), p.params.iter().map(|a| if a.optional { format!("[{}]", a.name) } else { a.name.clone() }).collect()));
                }
            }
        }
        out
    }

    /// A procedure by `Proc` or `Module.Proc`: (module, procedure) indexes.
    pub fn find(&self, name: &str) -> Option<(usize, usize)> {
        let (module, proc) = match name.rsplit_once('.') {
            Some((m, p)) => (Some(m), p),
            None => (None, name),
        };
        let proc = proc.trim();
        self.modules.iter().enumerate().filter(|(_, m)| module.is_none_or(|x| m.name.eq_ignore_ascii_case(x.trim()))).find_map(|(mi, m)| {
            m.procs.iter().position(|p| p.name.eq_ignore_ascii_case(proc) && p.kind != ProcKind::PropertyLet && (module.is_some() || m.kind == ModuleKind::Standard || p.public)).map(|pi| (mi, pi))
        })
    }

    /// Whether the workbook module (or a standard module's Auto_Open) has
    /// code to run when the file opens.
    pub fn has_open_handler(&self) -> bool {
        self.open_handler().is_some()
    }

    fn open_handler(&self) -> Option<(usize, usize)> {
        self.modules.iter().enumerate().find_map(|(mi, m)| {
            let name = match m.kind {
                ModuleKind::Document => "workbook_open",
                ModuleKind::Standard => "auto_open",
                _ => return None,
            };
            m.procs.iter().position(|p| p.name.eq_ignore_ascii_case(name)).map(|pi| (mi, pi))
        })
    }

    /// The handler of `event` (`Change`, `BeforeSave`...) of the workbook
    /// or of a sheet: `Workbook_<event>` or `Worksheet_<event>` in its
    /// module.
    pub fn handler(&self, wb: &Workbook, owner: Owner, event: &str) -> Option<(usize, usize)> {
        let name = match owner {
            Owner::Workbook => format!("workbook_{event}"),
            Owner::Sheet(_) => format!("worksheet_{event}"),
            Owner::None => return None,
        };
        self.modules.iter().enumerate().find_map(|(mi, m)| {
            let pi = m.procs.iter().position(|p| p.name.eq_ignore_ascii_case(&name) && p.kind == ProcKind::Sub)?;
            (interp::owner_for(m, wb) == owner).then_some((mi, pi))
        })
    }

    /// The names (lower case) of the procedures of document modules:
    /// what events may have a handler.
    pub fn event_names(&self) -> Vec<String> {
        self.modules.iter().filter(|m| m.kind == ModuleKind::Document).flat_map(|m| m.procs.iter().map(|p| p.name.to_ascii_lowercase())).collect()
    }

    /// Whether some sheet module handles Worksheet_Change.
    pub fn has_change_handler(&self) -> bool {
        self.modules.iter().any(|m| m.kind == ModuleKind::Document && m.procs.iter().any(|p| p.name.eq_ignore_ascii_case("worksheet_change")))
    }
}

/// Runs `f` on an interpreter and turns the result into an [`Outcome`].
fn run_with(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, f: impl FnOnce(&mut Interp) -> Result<(), Error>) -> Outcome {
    let mut it = match Interp::new(project, wb, ui, opts) {
        Ok(it) => it,
        Err(e) => return Outcome { error: Some(e), active_sheet: opts.active_sheet, selection: opts.selection, changed: false, save: false, events: true, cancel: false, requests: Vec::new(), books: Vec::new() },
    };
    // Modeless forms keep going once the macro is done.
    let res = f(&mut it).and_then(|_| it.form_loop(None));
    let error = match res {
        Err(e) if e.number == interp::END => None,
        Err(e) => Some(e),
        Ok(()) => None,
    };
    let undo = std::mem::take(&mut it.undo);
    let changed = !undo.is_empty();
    let (active, selection, save, events) = (it.active, it.selection, it.save, it.app.enable_events);
    let mut requests = std::mem::take(&mut it.requests);
    let books = it.take_books();
    if let Some((path, at)) = it.saved_as.take() {
        // Before a Close it asked for.
        let clean = at == undo.len();
        let i = requests.iter().position(|r| matches!(r, Request::Close { .. })).unwrap_or(requests.len());
        requests.insert(i, Request::SavedAs { path, clean });
    }
    drop(it);
    wb.push_undo(undo, UNDO_LABEL);
    let active = active.min(wb.sheets.len().saturating_sub(1));
    Outcome { error, active_sheet: active, selection, changed, save, events, cancel: false, requests, books }
}

/// Runs the macro `name` (`Proc` or `Module.Proc`).
pub fn run_macro(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, name: &str) -> Outcome {
    run_with(project, wb, ui, opts, |it| run_named_macro(it, project, name))
}

fn run_named_macro(it: &mut Interp, project: &Project, name: &str) -> Result<(), Error> {
    let (mi, pi) = project.find(name).ok_or_else(|| Error::app(format!("Cannot run the macro '{name}': it is not in this file")))?;
    let proc = project.modules[mi].procs[pi].clone();
    if !proc.params.iter().all(|p| p.optional || p.paramarray) {
        return Err(Error::app(format!("Cannot run the macro '{name}': it needs arguments")));
    }
    it.call_proc(mi, proc, &[]).map(|_| ())
}

/// Runs Sub or Function `name` with `args` as typed (a number, True or
/// False, else text; empty: an optional one left out). A Function's value
/// is shown in a MsgBox.
pub fn call_macro(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, name: &str, args: &[String]) -> Outcome {
    run_with(project, wb, ui, opts, |it| call_named(it, project, name, args))
}

fn call_named(it: &mut Interp, project: &Project, name: &str, args: &[String]) -> Result<(), Error> {
    let (mi, pi) = project.find(name).ok_or_else(|| Error::app(format!("Cannot run the macro '{name}': it is not in this file")))?;
    let proc = project.modules[mi].procs[pi].clone();
    if args.len() > proc.params.len() {
        return Err(Error::new(450, "Wrong number of arguments or invalid property assignment"));
    }
    let mut list = Vec::new();
    for (i, p) in proc.params.iter().enumerate() {
        let text = args.get(i).map_or("", |a| a.trim());
        if text.is_empty() && p.optional {
            list.push(Arg { name: None, expr: None });
            continue;
        }
        if text.is_empty() && args.get(i).is_none() {
            return Err(Error::new(449, "Argument not optional"));
        }
        let v = match text.to_ascii_lowercase().as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => value::parse_number(text).map_or_else(|| Value::str(args.get(i).cloned().unwrap_or_default()), Value::Double),
        };
        list.push(Arg { name: None, expr: Some(Expr::Lit(v)) });
    }
    // Trailing optional ones left out.
    while list.last().is_some_and(|a| a.expr.is_none()) {
        list.pop();
    }
    let function = proc.kind == ProcKind::Function;
    let r = it.call_proc(mi, proc, &list)?;
    if function {
        let text = match it.scalar(r) {
            Ok(v) => v.to_str().unwrap_or_else(|_| "?".into()),
            Err(_) => "?".into(),
        };
        let title = name.to_string();
        it.paused(|ui| ui.msg_box(&text, 64, &title));
    }
    Ok(())
}

/// A line of the Immediate window with no macro running: run in the
/// first standard module (or the first module). `?expr` prints a value.
pub fn immediate(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, text: &str) -> Outcome {
    run_with(project, wb, ui, opts, |it| immediate_in(it, project, text))
}

fn immediate_in(it: &mut Interp, project: &Project, text: &str) -> Result<(), Error> {
    let mi = project.modules.iter().position(|m| m.kind == ModuleKind::Standard).unwrap_or(0);
    if project.modules.is_empty() {
        return Err(Error::app("There are no modules"));
    }
    it.immediate_line(mi, text)
}

/// Workbook_Open (or Auto_Open), when the project has it.
pub fn workbook_open(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options) -> Option<Outcome> {
    let (mi, pi) = project.open_handler()?;
    Some(run_with(project, wb, ui, opts, |it| {
        let proc = project.modules[mi].procs[pi].clone();
        it.call_proc(mi, proc, &[]).map(|_| ())
    }))
}

/// Worksheet_Change of the module of `sheet`, with the changed cells as
/// Target, when the project has it.
pub fn worksheet_change(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, sheet: usize, target: Range) -> Option<Outcome> {
    let (mi, pi) = project.modules.iter().enumerate().find_map(|(mi, m)| {
        let pi = m.procs.iter().position(|p| p.name.eq_ignore_ascii_case("worksheet_change"))?;
        (interp::owner_for(m, wb) == interp::Owner::Sheet(sheet)).then_some((mi, pi))
    })?;
    Some(run_with(project, wb, ui, opts, |it| {
        let proc = project.modules[mi].procs[pi].clone();
        let arg = Arg { name: None, expr: Some(Expr::Lit(Value::Object(Obj::Range(RangeRef::new(sheet, target))))) };
        it.in_change = true;
        let res = it.call_proc(mi, proc, &[arg]).map(|_| ());
        it.in_change = false;
        res
    }))
}

/// What Excel passes to an event handler.
#[derive(Clone, Debug, PartialEq)]
pub enum EventArg {
    /// `Sh`: a worksheet.
    Sheet(usize),
    /// `Target`: cells of a sheet.
    Range(usize, Range),
    Bool(bool),
    /// `Cancel`: False; the handler may set it.
    Cancel,
    /// An object there is no model of (`Wn As Window`).
    Nothing,
    /// `Target As Hyperlink`: the link in a cell of a sheet.
    Hyperlink(usize, CellRef),
}

/// An event of the workbook (`Owner::Workbook`) or of a sheet, when the
/// project handles it: runs `Workbook_<event>` or `Worksheet_<event>`
/// with `args`. [`Outcome::cancel`] says whether it set `Cancel`.
pub fn fire_event(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, owner: Owner, event: &str, args: &[EventArg]) -> Option<Outcome> {
    let (mi, pi) = project.handler(wb, owner, event)?;
    if let EventArg::Sheet(i) | EventArg::Range(i, _) | EventArg::Hyperlink(i, _) = args.first()? {
        // A sheet that is no longer there.
        if *i >= wb.sheets.len() {
            return None;
        }
    }
    let mut cancel = false;
    let mut out = run_with(project, wb, ui, opts, |it| {
        let proc = project.modules[mi].procs[pi].clone();
        let values = args
            .iter()
            .map(|a| match a {
                EventArg::Sheet(i) => Value::Object(Obj::Sheet(*i)),
                EventArg::Range(i, r) => Value::Object(Obj::Range(RangeRef::new(*i, *r))),
                EventArg::Bool(b) => Value::Bool(*b),
                EventArg::Cancel => Value::Bool(false),
                EventArg::Nothing => Value::Nothing,
                EventArg::Hyperlink(i, c) => Value::Object(Obj::Part(Box::new(object::Part::Hyperlink(*i, *c)))),
            })
            .collect();
        it.in_change = event.eq_ignore_ascii_case("change") || event.eq_ignore_ascii_case("sheetchange");
        let back = it.call_event(mi, proc, values);
        it.in_change = false;
        let back = back?;
        cancel = args.iter().zip(&back).any(|(a, v)| *a == EventArg::Cancel && v.to_bool().unwrap_or(false));
        Ok(())
    });
    out.cancel = cancel;
    Some(out)
}

/// What running a document's macro did.
#[derive(Clone, Debug)]
pub struct DocOutcome {
    /// The error that stopped it, with its module and line.
    pub error: Option<Error>,
    /// It changed the document (the edits are in it, to make one undo
    /// step of).
    pub changed: bool,
    /// Document.Save was called.
    pub save: bool,
    /// Application.EnableEvents at the end.
    pub events: bool,
    /// An event handler set its `Cancel` argument.
    pub cancel: bool,
    /// What it asked of the application for later.
    pub requests: Vec<Request>,
    /// Where the Selection is.
    pub selection: apex_doc::Selection,
    /// Documents it opened or added and left open.
    pub docs: Vec<OpenDoc>,
}

/// Runs `f` on an interpreter for a document's macro: Word's objects, on
/// `doc` with the selection `sel`.
fn run_doc_with(project: &Project, doc: &mut apex_doc::Document, sel: apex_doc::Selection, ui: &mut dyn Ui, opts: &Options, f: impl FnOnce(&mut Interp) -> Result<(), Error>) -> DocOutcome {
    let revision = doc.revision;
    // The interpreter's workbook: none of its sheets has code, the
    // document's module is the "workbook" module.
    let mut wb = Workbook::new();
    wb.code_name = project.modules.iter().find(|m| m.kind == ModuleKind::Document).map(|m| m.name.clone());
    for s in &mut wb.sheets {
        s.code_name = Some("\u{1}".into());
    }
    let mut it = match Interp::with_book(project, interp::Book::Owned(Box::new(wb)), ui, opts) {
        Ok(it) => it,
        Err(e) => return DocOutcome { error: Some(e), changed: false, save: false, events: true, cancel: false, requests: Vec::new(), selection: sel, docs: Vec::new() },
    };
    it.word = Some(word::WordState::new(doc, sel));
    let res = f(&mut it).and_then(|_| it.form_loop(None));
    let error = match res {
        Err(e) if e.number == interp::END => None,
        Err(e) => Some(e),
        Ok(()) => None,
    };
    let (save, events) = (it.save, it.app.enable_events);
    let mut requests = std::mem::take(&mut it.requests);
    let saved_as = it.saved_as.take();
    let docs = it.take_docs();
    let mut w = it.word.take().expect("set above");
    let selection = w.selection();
    let end = w.doc.revision;
    drop(w);
    drop(it);
    if let Some((path, at)) = saved_as {
        let clean = at as u64 == end;
        let i = requests.iter().position(|r| matches!(r, Request::Close { .. })).unwrap_or(requests.len());
        requests.insert(i, Request::SavedAs { path, clean });
    }
    DocOutcome { error, changed: end != revision, save, events, cancel: false, requests, selection, docs }
}

/// Runs the macro `name` of a document (`Proc` or `Module.Proc`).
pub fn run_doc_macro(project: &Project, doc: &mut apex_doc::Document, sel: apex_doc::Selection, ui: &mut dyn Ui, opts: &Options, name: &str) -> DocOutcome {
    run_doc_with(project, doc, sel, ui, opts, |it| run_named_macro(it, project, name))
}

/// [`call_macro`] for a document.
pub fn call_doc_macro(project: &Project, doc: &mut apex_doc::Document, sel: apex_doc::Selection, ui: &mut dyn Ui, opts: &Options, name: &str, args: &[String]) -> DocOutcome {
    run_doc_with(project, doc, sel, ui, opts, |it| call_named(it, project, name, args))
}

/// [`immediate`] for a document.
pub fn doc_immediate(project: &Project, doc: &mut apex_doc::Document, sel: apex_doc::Selection, ui: &mut dyn Ui, opts: &Options, text: &str) -> DocOutcome {
    run_doc_with(project, doc, sel, ui, opts, |it| immediate_in(it, project, text))
}

/// What running a database's macro did.
#[derive(Clone, Debug)]
pub struct DbOutcome {
    /// The error that stopped it, with its module and line.
    pub error: Option<Error>,
    /// It changed rows of the database.
    pub changed: bool,
    /// What it asked of the forms on screen, in order.
    pub requests: Vec<access::DbRequest>,
}

/// Runs `f` on an interpreter for a database's macro: Access's objects on
/// `db`, with the forms open (`me` the one whose button ran it).
fn run_db_with(project: &Project, db: &mut dyn access::Database, forms: Vec<access::FormState>, me: Option<String>, ui: &mut dyn Ui, opts: &Options, f: impl FnOnce(&mut Interp) -> Result<(), Error>) -> DbOutcome {
    // No sheet has code: the modules are the database's.
    let mut wb = Workbook::new();
    for s in &mut wb.sheets {
        s.code_name = Some("\u{1}".into());
    }
    let mut it = match Interp::with_book(project, interp::Book::Owned(Box::new(wb)), ui, opts) {
        Ok(it) => it,
        Err(e) => return DbOutcome { error: Some(e), changed: false, requests: Vec::new() },
    };
    it.access = Some(access::AccessState::new(db, forms, me));
    let res = f(&mut it).and_then(|_| it.form_loop(None));
    let error = match res {
        Err(e) if e.number == interp::END => None,
        Err(e) => Some(e),
        Ok(()) => None,
    };
    let st = it.access.take().expect("set above");
    DbOutcome { error, changed: st.changed, requests: st.requests }
}

/// Runs the macro `name` of a database (`Proc` or `Module.Proc`).
pub fn run_db_macro(project: &Project, db: &mut dyn access::Database, forms: Vec<access::FormState>, me: Option<String>, ui: &mut dyn Ui, opts: &Options, name: &str) -> DbOutcome {
    run_db_with(project, db, forms, me, ui, opts, |it| run_named_macro(it, project, name))
}

/// [`call_macro`] for a database.
pub fn call_db_macro(project: &Project, db: &mut dyn access::Database, forms: Vec<access::FormState>, me: Option<String>, ui: &mut dyn Ui, opts: &Options, name: &str, args: &[String]) -> DbOutcome {
    run_db_with(project, db, forms, me, ui, opts, |it| call_named(it, project, name, args))
}

/// [`immediate`] for a database.
pub fn db_immediate(project: &Project, db: &mut dyn access::Database, forms: Vec<access::FormState>, ui: &mut dyn Ui, opts: &Options, text: &str) -> DbOutcome {
    run_db_with(project, db, forms, None, ui, opts, |it| immediate_in(it, project, text))
}

/// The handler of a document's event `Open`, `Close` or `New`:
/// `Document_<event>` in its module, else a standard module's
/// `Auto<event>` (`AutoOpen`).
pub fn doc_handler(project: &Project, event: &str) -> Option<(usize, usize)> {
    let doc = format!("document_{event}");
    let auto = format!("auto{event}");
    let find = |kind: ModuleKind, name: &str| project.modules.iter().enumerate().filter(|(_, m)| m.kind == kind).find_map(|(mi, m)| m.procs.iter().position(|p| p.kind == ProcKind::Sub && p.params.is_empty() && p.name.eq_ignore_ascii_case(name)).map(|pi| (mi, pi)));
    // The first document module is the document's (see run_doc_with).
    let first = project.modules.iter().position(|m| m.kind == ModuleKind::Document);
    find(ModuleKind::Document, &doc).filter(|(mi, _)| Some(*mi) == first).or_else(|| find(ModuleKind::Standard, &auto))
}

/// Runs the document's `event` handler ([`doc_handler`]), when it has one.
pub fn doc_event(project: &Project, doc: &mut apex_doc::Document, sel: apex_doc::Selection, ui: &mut dyn Ui, opts: &Options, event: &str) -> Option<DocOutcome> {
    let (mi, pi) = doc_handler(project, event)?;
    Some(run_doc_with(project, doc, sel, ui, opts, |it| {
        let proc = project.modules[mi].procs[pi].clone();
        it.call_proc(mi, proc, &[]).map(|_| ())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_names() {
        let p = Project::parse(&[
            Source { name: "Module1".into(), kind: ModuleKind::Standard, code: "Sub A()\nEnd Sub\nPrivate Sub B()\nEnd Sub\nSub C(x)\nEnd Sub\nFunction F()\nEnd Function\n".into() },
            Source { name: "Sheet1".into(), kind: ModuleKind::Document, code: "Sub Hello()\nEnd Sub\n".into() },
        ])
        .unwrap();
        assert_eq!(p.macros(), ["A", "Sheet1.Hello"]);
        assert_eq!(p.find("module1.a"), Some((0, 0)));
        assert_eq!(p.find("Sheet1.Hello"), Some((1, 0)));
        assert_eq!(p.find("Hello"), Some((1, 0)));
        assert!(p.find("Nope").is_none());
    }
}
