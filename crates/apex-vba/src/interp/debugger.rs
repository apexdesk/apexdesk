//! The debugger in the interpreter: where to stop, what a pause shows and
//! what the commands from it do.

use std::collections::HashMap;
use std::time::Instant;

use super::{END, Flow, Interp, Slot};
use crate::ast::{Expr, Line, ModuleKind, OnError, Stmt};
pub use crate::debug::Reason;
use crate::debug::{Breakpoint, DebugCmd, Debugger, Pause, StackEntry, VarView, Watch, WatchKind};
use crate::error::{Error, R};
use crate::object::Obj;
use crate::value::Value;
use crate::{Project, parser};

/// Elements of an array (and items of a collection) a pause shows.
const MAX_CHILDREN: usize = 1000;

/// Where stepping stops next.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Run,
    Into,
    /// At a statement of this depth of calls or less.
    Over(usize),
    /// At a statement of less depth.
    Out(usize),
    /// At this module (lower case) and line.
    To(String, u32),
}

pub struct State {
    breakpoints: Vec<(Breakpoint, Option<R<Expr>>)>,
    /// With the value last seen, for those that stop when it changes.
    watches: Vec<(Watch, R<Expr>, Option<String>)>,
    step: Step,
    /// Evaluating for the debugger (a condition, a watch, the Immediate
    /// window): no pauses then.
    busy: bool,
}

fn condition(b: &Breakpoint) -> Option<R<Expr>> {
    let c = b.condition.as_deref().map(str::trim).filter(|c| !c.is_empty())?;
    Some(parser::parse_expression("Condition", c))
}

fn watch(w: &Watch) -> (Watch, R<Expr>, Option<String>) {
    (w.clone(), parser::parse_expression("Watch", &w.expr), None)
}

impl State {
    pub fn new(_project: &Project, d: &Debugger) -> State {
        State {
            breakpoints: d.breakpoints.iter().map(|b| (b.clone(), condition(b))).collect(),
            watches: d.watches.iter().map(watch).collect(),
            step: if d.step { Step::Into } else { Step::Run },
            busy: false,
        }
    }
}

/// An error as the Immediate window prints it.
fn error_text(e: &Error) -> String {
    if e.number == 0 { e.message.clone() } else { format!("Error {}: {}", e.number, e.message) }
}

/// A value as the debugger windows show it: strings in quotes.
fn shown(v: &Value) -> String {
    match v {
        Value::Str(s) => {
            let mut t: String = s.chars().take(500).collect();
            if t.len() < s.len() {
                t.push('…');
            }
            format!("\"{t}\"")
        }
        Value::Empty => "Empty".into(),
        Value::Null => "Null".into(),
        Value::Nothing => "Nothing".into(),
        Value::Missing => "Missing".into(),
        Value::Array(a) if a.bounds.is_empty() => "()".into(),
        Value::Array(a) => format!("({})", a.bounds.iter().map(|(lo, hi)| format!("{lo} To {hi}")).collect::<Vec<_>>().join(", ")),
        Value::Object(Obj::Range(r)) => r.range.to_a1(),
        Value::Object(Obj::Collection(c)) => format!("Count = {}", c.borrow().items.len()),
        Value::Object(Obj::Dictionary(d)) => format!("Count = {}", d.borrow().items.len()),
        Value::Object(o) => o.type_name().into(),
        v => v.to_str().unwrap_or_default(),
    }
}

fn view(name: String, v: &Value, depth: usize) -> VarView {
    let mut children = Vec::new();
    if depth < 4 {
        match v {
            Value::Array(a) if !a.bounds.is_empty() => {
                let mut idx: Vec<i64> = a.bounds.iter().map(|b| b.0).collect();
                for x in a.data.iter().take(MAX_CHILDREN) {
                    let at = idx.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
                    children.push(view(format!("({at})"), x, depth + 1));
                    for d in (0..idx.len()).rev() {
                        idx[d] += 1;
                        if idx[d] <= a.bounds[d].1 {
                            break;
                        }
                        idx[d] = a.bounds[d].0;
                    }
                }
            }
            Value::Object(Obj::Collection(c)) => {
                for (i, (_, x)) in c.borrow().items.iter().take(MAX_CHILDREN).enumerate() {
                    children.push(view(format!("Item {}", i + 1), x, depth + 1));
                }
            }
            Value::Object(Obj::Dictionary(d)) => {
                let d = d.borrow();
                for (k, x) in d.keys.iter().zip(&d.items).take(MAX_CHILDREN) {
                    children.push(view(shown(k), x, depth + 1));
                }
            }
            _ => {}
        }
    }
    VarView { name, ty: v.type_name(), value: shown(v), children }
}

fn views(vars: &HashMap<String, Slot>) -> Vec<VarView> {
    let mut out: Vec<VarView> = vars.iter().map(|(n, s)| view(n.clone(), &s.borrow().val, 0)).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The blocks from `body` down to the one with the statement at `line`
/// (the lines the next statement may move to).
fn blocks_to<'b>(body: &'b [Line], line: u32, out: &mut Vec<&'b [Line]>) -> bool {
    out.push(body);
    for l in body {
        if l.line == line {
            return true;
        }
        for b in inner_blocks(&l.stmt) {
            if blocks_to(b, line, out) {
                return true;
            }
        }
    }
    out.pop();
    false
}

fn inner_blocks(s: &Stmt) -> Vec<&[Line]> {
    match s {
        Stmt::If { branches, other } => branches.iter().map(|(_, b)| b.as_slice()).chain(other.as_deref()).collect(),
        Stmt::Select { cases, other, .. } => cases.iter().map(|(_, b)| b.as_slice()).chain(other.as_deref()).collect(),
        Stmt::For { body, .. } | Stmt::ForEach { body, .. } | Stmt::Do { body, .. } | Stmt::With { body, .. } => vec![body.as_slice()],
        _ => Vec::new(),
    }
}

impl Interp<'_> {
    /// Debugging, and not evaluating for the debugger.
    pub(super) fn debugging(&self) -> bool {
        self.dbg.as_ref().is_some_and(|d| !d.busy)
    }

    /// Some `On Error` of the calls running catches an error.
    pub(super) fn handled(&self) -> bool {
        self.frames.iter().any(|f| f.on_error == OnError::ResumeNext || matches!(f.on_error, OnError::GoTo(_)) && !f.in_handler)
    }

    /// Runs `f` for the debugger: no pauses, and Err and the line of the
    /// statement running stay as they were.
    fn quiet<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let (err, line) = (self.err.clone(), self.frames.last().map_or(0, |f| f.line));
        if let Some(d) = self.dbg.as_mut() {
            d.busy = true;
        }
        let res = f(self);
        if let Some(d) = self.dbg.as_mut() {
            d.busy = false;
        }
        self.err = err;
        if let Some(f) = self.frames.last_mut() {
            f.line = line;
        }
        res
    }

    /// Before the statement `l`: stops when stepping, at a breakpoint or a
    /// watch. A flow when the next statement was moved.
    pub(super) fn before_line(&mut self, l: &Line) -> R<Option<Flow>> {
        let Some(d) = self.dbg.as_ref() else { return Ok(None) };
        if d.busy {
            return Ok(None);
        }
        let depth = self.frames.len();
        let module = self.module_name().to_lowercase();
        // VBA doesn't stop at declarations.
        let quiet = matches!(l.stmt, Stmt::Dim { .. } | Stmt::Const(_) | Stmt::Label(_));
        let step = !quiet
            && match &d.step {
                Step::Run => false,
                Step::Into => true,
                Step::Over(n) => depth <= *n,
                Step::Out(n) => depth < *n,
                Step::To(m, line) => *m == module && *line == l.line,
            };
        let conditions: Vec<Option<Expr>> = d
            .breakpoints
            .iter()
            .filter(|(b, _)| b.line == l.line && b.module.to_lowercase() == module)
            .map(|(_, c)| match c {
                Some(Ok(e)) => Some(e.clone()),
                // An unconditional breakpoint, or one whose condition
                // isn't valid (it shows the error when it stops).
                _ => None,
            })
            .collect();
        let mut reason = step.then_some(Reason::Step);
        for c in conditions {
            if reason.is_some() {
                break;
            }
            let hit = match c {
                None => true,
                Some(e) => match self.quiet(|it| it.truth(&e)) {
                    Ok(t) => t,
                    Err(e) if e.fatal => return Err(e),
                    Err(_) => false,
                },
            };
            if hit {
                reason = Some(Reason::Breakpoint);
            }
        }
        if let Some(i) = self.watch_hit()? {
            reason = reason.or(Some(Reason::Watch(i)));
        }
        match reason {
            Some(r) => self.pause(r, l.line),
            None => Ok(None),
        }
    }

    /// Evaluates the watches that stop the macro; the first that does.
    fn watch_hit(&mut self) -> R<Option<usize>> {
        let Some(d) = self.dbg.as_ref() else { return Ok(None) };
        let list: Vec<(usize, WatchKind, Expr)> = d.watches.iter().enumerate().filter(|(_, (w, _, _))| w.kind != WatchKind::Watch).filter_map(|(i, (w, e, _))| Some((i, w.kind, e.as_ref().ok()?.clone()))).collect();
        let mut hit = None;
        for (i, kind, e) in list {
            let now = match kind {
                WatchKind::BreakWhenTrue => match self.quiet(|it| it.truth(&e)) {
                    Ok(t) => Some(if t { "True" } else { "False" }.to_string()),
                    Err(e) if e.fatal => return Err(e),
                    Err(_) => Some("False".to_string()),
                },
                _ => match self.quiet(|it| it.eval(&e)) {
                    Ok(v) => Some(shown(&v)),
                    Err(e) if e.fatal => return Err(e),
                    // Out of its context: not a change.
                    Err(_) => None,
                },
            };
            let Some(now) = now else { continue };
            let Some(d) = self.dbg.as_mut() else { break };
            let last = d.watches[i].2.replace(now.clone());
            let stop = match kind {
                WatchKind::BreakWhenTrue => now == "True" && last.as_deref() != Some("True"),
                _ => last.is_some_and(|l| l != now),
            };
            if stop && hit.is_none() {
                hit = Some(i);
            }
        }
        Ok(hit)
    }

    /// What the debugger windows show.
    fn snapshot(&mut self, reason: Reason, line: u32) -> R<Pause> {
        let f = self.frames.last().expect("a running procedure");
        let module = self.project.modules[f.module].name.clone();
        let locals = views(&f.locals);
        let module_vars = views(&self.module_vars[f.module]);
        let stack = self
            .frames
            .iter()
            .rev()
            .filter(|f| !f.proc.name.is_empty())
            .map(|f| StackEntry { module: self.project.modules[f.module].name.clone(), proc: f.proc.name.clone(), line: f.line })
            .collect();
        let exprs: Vec<(String, R<Expr>)> = self.dbg.as_ref().map_or(Vec::new(), |d| d.watches.iter().map(|(w, e, _)| (w.expr.clone(), e.clone())).collect());
        let mut watches = Vec::new();
        for (text, e) in exprs {
            let v = match e {
                Ok(e) => self.quiet(|it| it.eval(&e)),
                Err(e) => Err(e),
            };
            watches.push(match v {
                Ok(v) => view(text, &v, 0),
                Err(e) if e.fatal && e.number == END => return Err(e),
                Err(e) => VarView { name: text, ty: String::new(), value: error_text(&e), children: Vec::new() },
            });
        }
        Ok(Pause { module, line, sheet: self.active, selection: self.selection, reason, locals, module_vars, stack, watches })
    }

    /// Stops before the statement at `line` until the debugger says go on:
    /// `None` runs the statement, a flow moves to another.
    pub(super) fn pause(&mut self, reason: Reason, line: u32) -> R<Option<Flow>> {
        loop {
            let p = self.snapshot(reason.clone(), line)?;
            let t0 = Instant::now();
            let cmd = match self.word.as_mut() {
                Some(w) => {
                    let sel = w.selection();
                    self.ui.pause_doc(&p, &w.doc, sel)
                }
                None => self.ui.pause(&p, &self.wb),
            };
            self.paused += t0.elapsed();
            let depth = self.frames.len();
            let Some(d) = self.dbg.as_mut() else { return Ok(None) };
            match cmd {
                DebugCmd::Continue => d.step = Step::Run,
                DebugCmd::StepInto => d.step = Step::Into,
                DebugCmd::StepOver => d.step = Step::Over(depth),
                DebugCmd::StepOut => d.step = Step::Out(depth),
                DebugCmd::RunTo { module, line } => d.step = Step::To(module.to_lowercase(), line),
                DebugCmd::SetNext(to) => {
                    if self.can_move(line, to) {
                        if let Some(d) = self.dbg.as_mut() {
                            d.step = Step::Into;
                        }
                        return Ok(Some(Flow::Jump(to)));
                    }
                    continue;
                }
                DebugCmd::End => return Err(Error::new(END, "End").fatal()),
                DebugCmd::Eval(text) => {
                    self.immediate(&text)?;
                    continue;
                }
                DebugCmd::Breakpoints(b) => {
                    d.breakpoints = b.iter().map(|b| (b.clone(), condition(b))).collect();
                    continue;
                }
                DebugCmd::Watches(w) => {
                    d.watches = w.iter().map(watch).collect();
                    continue;
                }
            }
            return Ok(None);
        }
    }

    /// The next statement may move from `from` to `to`: a line of a block
    /// the current statement is in, of the same procedure.
    fn can_move(&self, from: u32, to: u32) -> bool {
        let Some(f) = self.frames.last() else { return false };
        let mut chain = Vec::new();
        blocks_to(&f.proc.body, from, &mut chain) && chain.iter().any(|b| b.iter().any(|l| l.line == to && !matches!(l.stmt, Stmt::Dim { .. } | Stmt::Const(_))))
    }

    /// A line of the Immediate window with no macro running, in the
    /// context of module `mi`.
    pub fn immediate_line(&mut self, mi: usize, text: &str) -> R<()> {
        let proc = std::rc::Rc::new(crate::ast::Proc { name: "Immediate".into(), kind: crate::ast::ProcKind::Sub, public: false, params: Vec::new(), ret: crate::value::VType::Variant, ret_name: None, body: Vec::new(), line: 0, is_static: false });
        self.frames.push(super::Frame { module: mi, proc, locals: HashMap::new(), with: Vec::new(), on_error: OnError::Off, in_handler: false, err_pc: 0, line: 0, me: None });
        let res = self.immediate(text);
        self.frames.pop();
        res
    }

    /// Ctrl+Break: pause at the next statement.
    pub(super) fn interrupt(&mut self) -> bool {
        match self.dbg.as_mut() {
            Some(d) if !d.busy => {
                d.step = Step::Into;
                true
            }
            _ => false,
        }
    }

    /// A line of the Immediate window, run in the procedure stopped:
    /// `?expr` (or `Print expr`) prints the value.
    fn immediate(&mut self, text: &str) -> R<()> {
        let t = text.trim();
        if t.is_empty() {
            return Ok(());
        }
        let code = match t.strip_prefix('?') {
            Some(e) => format!("Debug.Print {e}"),
            None if t.len() > 6 && t[..6].eq_ignore_ascii_case("print ") => format!("Debug.Print {}", &t[6..]),
            None => t.to_string(),
        };
        // One line: no new procedures or blocks spanning lines.
        let code = code.replace(['\r', '\n'], " ");
        let m = match parser::parse_module("Immediate", ModuleKind::Standard, &format!("Sub Immediate()\n{code}\nEnd Sub\n")) {
            Ok(m) if m.procs.len() == 1 => m,
            Ok(_) => return Ok(()),
            Err(e) => {
                self.ui.debug_print(&error_text(&e));
                return Ok(());
            }
        };
        let body = m.procs[0].body.clone();
        let res = self.quiet(|it| {
            let mut pc = 0;
            it.exec_top(&body, &mut pc).map(|_| ())
        });
        match res {
            Err(e) if e.fatal => Err(e),
            Err(e) => {
                self.ui.debug_print(&error_text(&e));
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }
}
