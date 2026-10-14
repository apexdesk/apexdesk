//! Debugging a macro: breakpoints, stepping, watches and the Immediate
//! window. The interpreter stops before a statement and asks [`crate::Ui::pause`]
//! what to do, with what the debugger windows show ([`Pause`]).

use crate::error::Error;

/// What to stop at, given in [`crate::Options::debug`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Debugger {
    pub breakpoints: Vec<Breakpoint>,
    pub watches: Vec<Watch>,
    /// Stop at the first statement (F8 to start).
    pub step: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Breakpoint {
    pub module: String,
    /// 1-based, as the editor numbers lines.
    pub line: u32,
    /// Stops only when this expression is True.
    pub condition: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchKind {
    /// Only shown.
    Watch,
    /// Stops when it becomes True.
    BreakWhenTrue,
    /// Stops when its value changes.
    BreakWhenChanged,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Watch {
    pub expr: String,
    pub kind: WatchKind,
}

/// Why the macro stopped.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    Breakpoint,
    Step,
    Stop,
    /// `Debug.Assert` with a False expression.
    Assert,
    /// The watch at this index.
    Watch(usize),
    /// An error no `On Error` handles; continuing runs the statement again.
    Error(Error),
}

/// A variable (or an element, item or watch) as the Locals window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct VarView {
    pub name: String,
    /// VBA's type name (`Long`, `String`, `Variant()`, `Range`...), empty
    /// for a watch that can't be evaluated here.
    pub ty: String,
    /// As VBA shows it: strings in quotes.
    pub value: String,
    /// Array elements, Collection and Dictionary items.
    pub children: Vec<VarView>,
}

/// A call on the stack, innermost first.
#[derive(Clone, Debug, PartialEq)]
pub struct StackEntry {
    pub module: String,
    pub proc: String,
    pub line: u32,
}

/// The macro, stopped before a statement.
#[derive(Clone, Debug, PartialEq)]
pub struct Pause {
    pub module: String,
    pub line: u32,
    /// The active sheet and the selection, as the macro left them.
    pub sheet: usize,
    pub selection: apex_sheet::Range,
    pub reason: Reason,
    /// The procedure's variables, by name (names are lower case: VBA
    /// isn't case sensitive; the code says how they are written).
    pub locals: Vec<VarView>,
    /// The module's variables.
    pub module_vars: Vec<VarView>,
    pub stack: Vec<StackEntry>,
    /// The watches' values, in their order.
    pub watches: Vec<VarView>,
}

/// What to do from a pause.
#[derive(Clone, Debug, PartialEq)]
pub enum DebugCmd {
    /// F5: run to the next breakpoint.
    Continue,
    /// F8: the next statement, into calls.
    StepInto,
    /// Shift+F8: the next statement of this procedure.
    StepOver,
    /// Ctrl+Shift+F8: back in the caller.
    StepOut,
    /// Ctrl+F8: run to this line.
    RunTo { module: String, line: u32 },
    /// Ctrl+F9: run this line of the procedure next (one of the blocks the
    /// current statement is in); refused (the pause comes again) elsewhere.
    SetNext(u32),
    /// Reset: stops the macro (what it did stays, one undo step).
    End,
    /// The Immediate window: a statement, or `?expr` to print a value.
    /// Its output and errors come through [`crate::Ui::debug_print`].
    Eval(String),
    Breakpoints(Vec<Breakpoint>),
    Watches(Vec<Watch>),
}
