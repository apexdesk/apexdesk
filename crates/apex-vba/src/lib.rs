//! A basic VBA interpreter for the macros of spreadsheets: the language
//! (types, Dim/Const, If/Select, loops, Sub/Function with ByVal/ByRef,
//! On Error, With, arrays, Collection and Dictionary), VBA's usual
//! functions and the most used part of Excel's object model (Application,
//! Workbook, Worksheets, Range/Cells, fonts and fills, WorksheetFunction,
//! MsgBox and InputBox through [`Ui`]).
//!
//! What it changes in the workbook is one undo step. A macro stops after
//! [`Options::max_steps`] statements or [`Options::max_time`] of running
//! (time in dialogs doesn't count). UserForms, `Declare` (Windows API)
//! and automating other applications give a clear "not supported" error.

pub mod ast;
mod builtins;
mod consts;
pub mod error;
mod excel;
mod interp;
mod lexer;
mod modules;
pub mod object;
mod parser;
pub mod value;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use apex_sheet::{CellRef, Range, Workbook};

pub use ast::ModuleKind;
pub use error::Error;

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
        let mut modules = Vec::new();
        for s in sources {
            modules.push(parser::parse_module(&s.name, s.kind, &s.code)?);
        }
        Ok(Project { modules })
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

    /// Whether some sheet module handles Worksheet_Change.
    pub fn has_change_handler(&self) -> bool {
        self.modules.iter().any(|m| m.kind == ModuleKind::Document && m.procs.iter().any(|p| p.name.eq_ignore_ascii_case("worksheet_change")))
    }
}

/// Runs `f` on an interpreter and turns the result into an [`Outcome`].
fn run_with(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, f: impl FnOnce(&mut Interp) -> Result<(), Error>) -> Outcome {
    let mut it = match Interp::new(project, wb, ui, opts) {
        Ok(it) => it,
        Err(e) => return Outcome { error: Some(e), active_sheet: opts.active_sheet, selection: opts.selection, changed: false, save: false, events: true },
    };
    let res = f(&mut it);
    let error = match res {
        Err(e) if e.number == interp::END => None,
        Err(e) => Some(e),
        Ok(()) => None,
    };
    let undo = std::mem::take(&mut it.undo);
    let changed = !undo.is_empty();
    let (active, selection, save, events) = (it.active, it.selection, it.save, it.app.enable_events);
    drop(it);
    wb.push_undo(undo, UNDO_LABEL);
    let active = active.min(wb.sheets.len().saturating_sub(1));
    Outcome { error, active_sheet: active, selection, changed, save, events }
}

/// Runs the macro `name` (`Proc` or `Module.Proc`).
pub fn run_macro(project: &Project, wb: &mut Workbook, ui: &mut dyn Ui, opts: &Options, name: &str) -> Outcome {
    run_with(project, wb, ui, opts, |it| {
        let (mi, pi) = project.find(name).ok_or_else(|| Error::app(format!("Cannot run the macro '{name}': it is not in this workbook")))?;
        let proc = project.modules[mi].procs[pi].clone();
        if !proc.params.iter().all(|p| p.optional || p.paramarray) {
            return Err(Error::app(format!("Cannot run the macro '{name}': it needs arguments")));
        }
        it.call_proc(mi, proc, &[]).map(|_| ())
    })
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
        it.call_proc(mi, proc, &[arg]).map(|_| ())
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
