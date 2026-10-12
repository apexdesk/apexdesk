//! The evaluator: variables and scopes, procedure calls (ByVal/ByRef,
//! Optional, ParamArray), statements, `On Error` and the limits that keep
//! a runaway macro from hanging the application.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use apex_sheet::{Range, Workbook};

use crate::ast::*;
use crate::error::{Error, R};
use crate::object::{By, Obj, RangeRef};
use crate::value::{self, Array, VType, Value, compare};
use crate::{Options, Project, Ui};

/// A variable: its declared type (for arrays, of the elements) and value.
#[derive(Debug)]
pub struct Var {
    pub ty: VType,
    pub val: Value,
    pub konst: bool,
    pub array: bool,
    /// The return value of the function running (its name).
    pub ret: bool,
}

pub type Slot = Rc<RefCell<Var>>;

fn slot(ty: VType, val: Value) -> Slot {
    Rc::new(RefCell::new(Var { ty, val, konst: false, array: false, ret: false }))
}

/// What a statement leaves to the statements around it.
#[derive(Debug)]
pub enum Flow {
    Normal,
    Exit(ExitKind),
    GoTo(String),
    Resume(ResumeKind),
}

/// What a document module belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Owner {
    None,
    Workbook,
    Sheet(usize),
}

struct Frame {
    module: usize,
    proc: Rc<Proc>,
    locals: HashMap<String, Slot>,
    with: Vec<Value>,
    on_error: OnError,
    in_handler: bool,
    /// Top-level statement where the error being handled happened.
    err_pc: usize,
}

/// The Err object.
#[derive(Clone, Debug, Default)]
pub struct ErrState {
    pub number: i32,
    pub description: String,
    pub source: String,
    pub line: u32,
}

/// Settings of Application that macros change and read back.
#[derive(Clone, Debug)]
pub struct AppState {
    pub screen_updating: bool,
    pub display_alerts: bool,
    pub enable_events: bool,
    pub calculation: i32,
    pub status_bar: Option<String>,
}

/// Error number of `End`: stops the macro without an error.
pub const END: i32 = -1;

const MAX_DEPTH: usize = 10_000;

/// Where the stack is (it grows down on the platforms we build for, but
/// only the distance counts).
fn stack_here() -> usize {
    let x = 0u8;
    std::hint::black_box(&x) as *const u8 as usize
}

pub struct Interp<'a> {
    pub project: &'a Project,
    pub wb: &'a mut Workbook,
    pub ui: &'a mut dyn Ui,
    pub active: usize,
    pub selection: Range,
    /// Inverses of the changes made, for one undo step.
    pub undo: Vec<apex_sheet::Op>,
    pub owners: Vec<Owner>,
    module_vars: Vec<HashMap<String, Slot>>,
    statics: HashMap<(usize, String), HashMap<String, Slot>>,
    frames: Vec<Frame>,
    pub err: ErrState,
    pub app: AppState,
    pub save: bool,
    pub file_name: String,
    pub file_path: String,
    pub user_name: String,
    pub utc_offset: i64,
    pub rng: u64,
    steps: u64,
    max_steps: u64,
    stack_base: usize,
    max_stack: usize,
    max_time: Duration,
    started: Instant,
    /// Time spent in dialogs, which doesn't count against the limit.
    paused: Duration,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl<'a> Interp<'a> {
    pub fn new(project: &'a Project, wb: &'a mut Workbook, ui: &'a mut dyn Ui, opts: &Options) -> R<Interp<'a>> {
        value::set_locale(wb.locale);
        let owners = project.modules.iter().map(|m| owner_for(m, wb)).collect();
        let active = opts.active_sheet.min(wb.sheets.len().saturating_sub(1));
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
        let mut it = Interp {
            project,
            wb,
            ui,
            active,
            selection: opts.selection,
            undo: Vec::new(),
            owners,
            module_vars: vec![HashMap::new(); project.modules.len()],
            statics: HashMap::new(),
            frames: Vec::new(),
            err: ErrState::default(),
            app: AppState { screen_updating: true, display_alerts: true, enable_events: true, calculation: -4105, status_bar: None },
            save: false,
            file_name: opts.file_name.clone(),
            file_path: opts.file_path.clone(),
            user_name: opts.user_name.clone(),
            utc_offset: opts.utc_offset,
            rng: seed,
            steps: 0,
            max_steps: opts.max_steps,
            stack_base: stack_here(),
            max_stack: opts.max_stack,
            max_time: opts.max_time,
            started: Instant::now(),
            paused: Duration::ZERO,
            cancel: opts.cancel.clone(),
        };
        it.init_modules()?;
        Ok(it)
    }

    /// Module-level variables and constants.
    fn init_modules(&mut self) -> R<()> {
        let project = self.project;
        for (mi, m) in project.modules.iter().enumerate() {
            for (v, _) in &m.vars {
                let s = self.new_var(v, m.base)?;
                self.module_vars[mi].insert(v.name.clone(), s);
            }
        }
        // Constants may use earlier ones (and those of other modules).
        for (mi, m) in project.modules.iter().enumerate() {
            for (name, e, _) in &m.consts {
                self.frames.push(Frame { module: mi, proc: Rc::new(Proc { name: String::new(), kind: ProcKind::Sub, public: false, params: Vec::new(), ret: VType::Variant, body: Vec::new(), line: 0, is_static: false }), locals: HashMap::new(), with: Vec::new(), on_error: OnError::Off, in_handler: false, err_pc: 0 });
                let v = self.eval(e).map_err(|err| err.at(&m.name, 0));
                self.frames.pop();
                let s = slot(VType::Variant, v?);
                s.borrow_mut().konst = true;
                self.module_vars[mi].insert(name.clone(), s);
            }
        }
        Ok(())
    }

    // --- limits and errors -----------------------------------------------------

    pub fn tick(&mut self) -> R<()> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(Error::new(18, format!("The macro was stopped: it ran more than {} steps", self.max_steps)).fatal());
        }
        if self.steps % 1024 == 0 {
            if self.started.elapsed().saturating_sub(self.paused) > self.max_time {
                return Err(Error::new(18, format!("The macro was stopped: it ran for more than {} seconds", self.max_time.as_secs())).fatal());
            }
            if self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(Error::new(18, "The macro was stopped").fatal());
            }
        }
        Ok(())
    }

    /// Runs `f` (a dialog) without counting its time.
    pub fn paused<T>(&mut self, f: impl FnOnce(&mut dyn Ui) -> T) -> T {
        let t = Instant::now();
        let out = f(self.ui);
        self.paused += t.elapsed();
        out
    }

    fn set_err(&mut self, e: &Error) {
        self.err = ErrState { number: e.number, description: e.message.clone(), source: "VBAProject".into(), line: e.line };
    }

    pub fn module_name(&self) -> &str {
        self.frames.last().map_or("", |f| self.project.modules[f.module].name.as_str())
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a running procedure")
    }

    fn module(&self) -> usize {
        self.frames.last().map_or(0, |f| f.module)
    }

    pub fn compare_text(&self) -> bool {
        self.project.modules.get(self.module()).is_some_and(|m| m.compare_text)
    }

    /// The sheet unqualified `Range` and `Cells` refer to: the module's own
    /// sheet, else the active one.
    pub fn home_sheet(&self) -> usize {
        match self.owners.get(self.module()) {
            Some(Owner::Sheet(s)) if *s < self.wb.sheets.len() => *s,
            _ => self.active,
        }
    }

    // --- variables ---------------------------------------------------------------

    fn new_var(&mut self, v: &DimVar, base: i64) -> R<Slot> {
        if let Some(bounds) = &v.bounds {
            let mut b = Vec::new();
            for (lo, hi) in bounds {
                let lo = match lo {
                    Some(e) => self.eval_scalar(e)?.to_i64()?,
                    None => base,
                };
                b.push((lo, self.eval_scalar(hi)?.to_i64()?));
            }
            let arr = Array::new(b, v.ty)?;
            let s = slot(v.ty, Value::Array(Rc::new(arr)));
            s.borrow_mut().array = true;
            return Ok(s);
        }
        let val = match &v.new {
            Some(class) => self.new_object(class)?,
            None => v.ty.default_value(),
        };
        Ok(slot(v.ty, val))
    }

    /// The variable `name` in scope: local, module, then public ones.
    pub fn lookup(&self, name: &str) -> Option<Slot> {
        let f = self.frames.last()?;
        if let Some(s) = f.locals.get(name) {
            return Some(s.clone());
        }
        if let Some(s) = self.module_vars[f.module].get(name) {
            return Some(s.clone());
        }
        let project = self.project;
        for (mi, m) in project.modules.iter().enumerate() {
            if mi == f.module || m.kind != ModuleKind::Standard {
                continue;
            }
            let public = m.vars.iter().any(|(v, p)| *p && v.name == name) || m.consts.iter().any(|(n, _, p)| *p && n == name);
            if public {
                if let Some(s) = self.module_vars[mi].get(name) {
                    return Some(s.clone());
                }
            }
        }
        None
    }

    /// A variable or constant of module `mi`.
    pub(crate) fn module_var(&self, mi: usize, name: &str) -> Option<Slot> {
        self.module_vars.get(mi)?.get(name).cloned()
    }

    /// A procedure by name: the current module's, then public ones.
    fn find_proc(&self, name: &str) -> Option<(usize, Rc<Proc>)> {
        let cur = self.module();
        let project = self.project;
        if let Some(p) = project.modules.get(cur).and_then(|m| m.proc(name)) {
            return Some((cur, p.clone()));
        }
        project.modules.iter().enumerate().filter(|(_, m)| m.kind == ModuleKind::Standard).find_map(|(mi, m)| m.proc(name).filter(|p| p.public).map(|p| (mi, p.clone())))
    }

    /// Stores `v` in a variable (Let converts to its type; objects need Set).
    pub(crate) fn store(&mut self, s: &Slot, v: Value, set: bool) -> R<()> {
        let (ty, array, konst, cur_obj) = {
            let b = s.borrow();
            (b.ty, b.array, b.konst, matches!(b.val, Value::Object(Obj::Range(_))))
        };
        if konst {
            return Err(Error::new(0, "Assignment to constant not permitted"));
        }
        if set {
            if !v.is_object() {
                return Err(Error::object_required());
            }
            if ty != VType::Variant && ty != VType::Object {
                return Err(Error::type_mismatch());
            }
            s.borrow_mut().val = v;
            return Ok(());
        }
        // `rng = 5` on a Range variable sets its value.
        if cur_obj && ty == VType::Object {
            let target = s.borrow().val.clone();
            return self.set_default(target, v);
        }
        let v = match v {
            Value::Array(a) if array || ty == VType::Variant => Value::Array(a),
            Value::Array(_) => return Err(Error::type_mismatch()),
            Value::Object(_) | Value::Nothing if ty == VType::Object => return Err(Error::new(91, "Object variable not set (use Set)")),
            v @ (Value::Object(_) | Value::Nothing) => self.scalar(v)?.coerce(ty)?,
            v if array => return Err(Error::new(0, format!("Can't assign to array ({})", v.type_name()))),
            v => v.coerce(ty)?,
        };
        s.borrow_mut().val = v;
        Ok(())
    }

    /// A variable for `name` to assign to, created (Variant) when implicit
    /// declaration is allowed.
    fn target_slot(&mut self, name: &str) -> R<Slot> {
        if let Some(s) = self.lookup(name) {
            return Ok(s);
        }
        let project = self.project;
        if project.modules[self.module()].explicit {
            return Err(Error::new(0, format!("Variable not defined: {name}")));
        }
        let s = slot(VType::Variant, Value::Empty);
        self.frame().locals.insert(name.to_string(), s.clone());
        Ok(s)
    }

    // --- running procedures --------------------------------------------------------

    /// Calls a procedure with the caller's argument expressions.
    pub fn call_proc(&mut self, module: usize, proc: Rc<Proc>, args: &[Arg]) -> R<Value> {
        if self.frames.len() >= MAX_DEPTH || stack_here().abs_diff(self.stack_base) > self.max_stack {
            return Err(Error::new(28, "Out of stack space").fatal());
        }
        let params = &proc.params;
        let mut given: Vec<Option<&Expr>> = vec![None; params.len()];
        let mut extra: Vec<&Expr> = Vec::new();
        let mut pos = 0;
        for a in args {
            match &a.name {
                Some(n) => {
                    let i = params.iter().position(|p| &p.name == n).ok_or_else(|| Error::new(448, format!("Named argument not found: {n}")))?;
                    given[i] = a.expr.as_ref();
                }
                None => {
                    if pos < params.len() && !params[pos].paramarray {
                        given[pos] = a.expr.as_ref();
                        pos += 1;
                    } else if params.last().is_some_and(|p| p.paramarray) {
                        pos = pos.max(params.len() - 1);
                        if let Some(e) = &a.expr {
                            extra.push(e);
                        }
                    } else {
                        return Err(Error::new(450, "Wrong number of arguments or invalid property assignment"));
                    }
                }
            }
        }
        let mut locals = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            if p.paramarray {
                let mut items = Vec::new();
                for e in extra.iter().copied().chain(given[i]) {
                    let v = self.eval(e)?;
                    items.push(v);
                }
                let s = slot(VType::Variant, Value::Array(Rc::new(Array::from_vec(items, 0))));
                locals.insert(p.name.clone(), s);
                continue;
            }
            let s = match given[i] {
                Some(e) => {
                    let by_ref = if p.byval { None } else { self.ref_slot(e) };
                    match by_ref {
                        Some(s) => s,
                        None => {
                            let v = self.eval(e)?;
                            let v = match v {
                                v @ (Value::Object(_) | Value::Nothing) if matches!(p.ty, VType::Object | VType::Variant) => v,
                                Value::Array(a) => Value::Array(a),
                                v @ (Value::Object(_) | Value::Nothing) => self.scalar(v)?.coerce(p.ty)?,
                                v => v.coerce(p.ty)?,
                            };
                            let s = slot(p.ty, v);
                            s.borrow_mut().array = p.array;
                            s
                        }
                    }
                }
                None if p.optional => {
                    let v = match &p.default {
                        Some(d) => self.eval(d)?.coerce(p.ty)?,
                        None if p.ty == VType::Variant => Value::Missing,
                        None => p.ty.default_value(),
                    };
                    slot(p.ty, v)
                }
                None => return Err(Error::new(449, format!("Argument not optional: {}", p.name))),
            };
            locals.insert(p.name.clone(), s);
        }
        let lname = proc.name.to_lowercase();
        let ret = if matches!(proc.kind, ProcKind::Function | ProcKind::PropertyGet) {
            let s = slot(proc.ret, proc.ret.default_value());
            s.borrow_mut().ret = true;
            locals.insert(lname.clone(), s.clone());
            Some(s)
        } else {
            None
        };
        self.frames.push(Frame { module, proc: proc.clone(), locals, with: Vec::new(), on_error: OnError::Off, in_handler: false, err_pc: 0 });
        let res = self.run_body(&proc.body);
        self.frames.pop();
        res?;
        Ok(ret.map(|s| std::mem::take(&mut s.borrow_mut().val)).unwrap_or_default())
    }

    /// The variable an argument passes ByRef, when it is a plain variable.
    fn ref_slot(&self, e: &Expr) -> Option<Slot> {
        match e {
            Expr::Name(n) => self.lookup(n).filter(|s| !s.borrow().ret && !s.borrow().konst),
            _ => None,
        }
    }

    fn find_label(body: &[Line], label: &str) -> R<usize> {
        body.iter().position(|l| matches!(&l.stmt, Stmt::Label(x) if x == label)).ok_or_else(|| Error::new(0, format!("Label not defined: {label}")))
    }

    /// Runs a procedure body with its `On Error GoTo` handler and `Resume`.
    fn run_body(&mut self, body: &[Line]) -> R<()> {
        let mut pc = 0;
        loop {
            let res = self.exec_top(body, &mut pc);
            match res {
                Ok(Flow::Normal) | Ok(Flow::Exit(_)) => return Ok(()),
                Ok(Flow::GoTo(l)) => pc = Self::find_label(body, &l)?,
                Ok(Flow::Resume(k)) => {
                    let f = self.frame();
                    if !f.in_handler {
                        return Err(Error::new(20, "Resume without error"));
                    }
                    f.in_handler = false;
                    let at = f.err_pc;
                    self.err = ErrState::default();
                    pc = match k {
                        ResumeKind::Same => at,
                        ResumeKind::Next => at + 1,
                        ResumeKind::Label(l) => Self::find_label(body, &l)?,
                    };
                }
                Err(e) if e.fatal => return Err(e),
                Err(e) => {
                    let f = self.frame();
                    match f.on_error.clone() {
                        OnError::GoTo(l) if !f.in_handler => {
                            f.in_handler = true;
                            f.err_pc = pc;
                            self.set_err(&e);
                            pc = Self::find_label(body, &l).map_err(|x| x.at(&e.module, e.line))?;
                        }
                        _ => return Err(e),
                    }
                }
            }
        }
    }

    fn exec_top(&mut self, body: &[Line], pc: &mut usize) -> R<Flow> {
        while *pc < body.len() {
            let f = self.exec_line(&body[*pc])?;
            if !matches!(f, Flow::Normal) {
                return Ok(f);
            }
            *pc += 1;
        }
        Ok(Flow::Normal)
    }

    fn exec_block(&mut self, body: &[Line]) -> R<Flow> {
        for l in body {
            let f = self.exec_line(l)?;
            if !matches!(f, Flow::Normal) {
                return Ok(f);
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_line(&mut self, l: &Line) -> R<Flow> {
        self.tick().map_err(|e| e.at(self.module_name(), l.line))?;
        match self.exec(&l.stmt) {
            Ok(f) => Ok(f),
            Err(e) => {
                let e = e.at(self.module_name(), l.line);
                if !e.fatal && self.frame().on_error == OnError::ResumeNext {
                    self.set_err(&e);
                    Ok(Flow::Normal)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn exec(&mut self, stmt: &Stmt) -> R<Flow> {
        match stmt {
            Stmt::Dim { vars, is_static } => {
                let base = self.project.modules[self.module()].base;
                for v in vars {
                    if self.frame().locals.contains_key(&v.name) {
                        continue;
                    }
                    let s = if *is_static || self.frame().proc.is_static {
                        let key = (self.module(), self.frame().proc.name.to_lowercase());
                        let existing = self.statics.get(&key).and_then(|m| m.get(&v.name)).cloned();
                        match existing {
                            Some(s) => s,
                            None => {
                                let s = self.new_var(v, base)?;
                                self.statics.entry(key).or_default().insert(v.name.clone(), s.clone());
                                s
                            }
                        }
                    } else {
                        self.new_var(v, base)?
                    };
                    self.frame().locals.insert(v.name.clone(), s);
                }
            }
            Stmt::ReDim { preserve, vars } => {
                let base = self.project.modules[self.module()].base;
                for v in vars {
                    let mut b = Vec::new();
                    for (lo, hi) in v.bounds.as_deref().unwrap_or(&[]) {
                        let lo = match lo {
                            Some(e) => self.eval_scalar(e)?.to_i64()?,
                            None => base,
                        };
                        b.push((lo, self.eval_scalar(hi)?.to_i64()?));
                    }
                    let s = match self.lookup(&v.name) {
                        Some(s) => s,
                        None => {
                            let s = slot(v.ty, Value::Empty);
                            self.frame().locals.insert(v.name.clone(), s.clone());
                            s
                        }
                    };
                    let mut var = s.borrow_mut();
                    let ty = if v.ty != VType::Variant { v.ty } else { var.ty };
                    let arr = match (&var.val, preserve) {
                        (Value::Array(old), true) if !old.bounds.is_empty() => old.resized(b)?,
                        _ => Array::new(b, ty)?,
                    };
                    var.ty = ty;
                    var.array = true;
                    var.val = Value::Array(Rc::new(arr));
                }
            }
            Stmt::Const(list) => {
                for (n, e) in list {
                    let v = self.eval_scalar(e)?;
                    let s = slot(VType::Variant, v);
                    s.borrow_mut().konst = true;
                    self.frame().locals.insert(n.clone(), s);
                }
            }
            Stmt::Assign { target, value, set } => {
                let v = if *set { self.eval(value)? } else { self.eval_let(value)? };
                self.assign(target, v, *set)?;
            }
            Stmt::Call { target, args } => {
                self.call(target, args, true)?;
            }
            Stmt::If { branches, other } => {
                for (cond, body) in branches {
                    if self.truth(cond)? {
                        return self.exec_block(body);
                    }
                }
                if let Some(body) = other {
                    return self.exec_block(body);
                }
            }
            Stmt::Select { expr, cases, other } => {
                let v = self.eval_scalar(expr)?;
                let text = self.compare_text();
                for (tests, body) in cases {
                    let mut hit = false;
                    for t in tests {
                        hit = match t {
                            CaseTest::Value(e) => {
                                let x = self.eval_scalar(e)?;
                                compare(&v, &x, text)? == Some(std::cmp::Ordering::Equal)
                            }
                            CaseTest::Range(a, b) => {
                                let (a, b) = (self.eval_scalar(a)?, self.eval_scalar(b)?);
                                compare(&v, &a, text)?.is_some_and(|o| o.is_ge()) && compare(&v, &b, text)?.is_some_and(|o| o.is_le())
                            }
                            CaseTest::Is(op, e) => {
                                let x = self.eval_scalar(e)?;
                                matches!(self.binop(*op, v.clone(), x)?, Value::Bool(true))
                            }
                        };
                        if hit {
                            break;
                        }
                    }
                    if hit {
                        return self.exec_block(body);
                    }
                }
                if let Some(body) = other {
                    return self.exec_block(body);
                }
            }
            Stmt::For { var, from, to, step, body } => {
                let from = self.eval_scalar(from)?;
                let to = self.eval_scalar(to)?;
                let step = match step {
                    Some(s) => self.eval_scalar(s)?,
                    None => Value::Int(1),
                };
                let integral = value::is_integral(&from) && value::is_integral(&to) && value::is_integral(&step);
                let (to, step) = (to.to_f64()?, step.to_f64()?);
                self.assign(var, from, false)?;
                loop {
                    let cur = self.eval_scalar(var)?.to_f64()?;
                    if (step >= 0.0 && cur > to) || (step < 0.0 && cur < to) {
                        break;
                    }
                    match self.exec_block(body)? {
                        Flow::Normal => {}
                        Flow::Exit(ExitKind::For) => break,
                        f => return Ok(f),
                    }
                    self.tick()?;
                    let cur = self.eval_scalar(var)?.to_f64()? + step;
                    self.assign(var, value::num(cur, integral), false)?;
                }
            }
            Stmt::ForEach { var, group, body } => {
                let g = self.eval(group)?;
                let items = self.items_of(g)?;
                for item in items {
                    let item = item?;
                    let set = item.is_object();
                    self.assign(var, item, set)?;
                    match self.exec_block(body)? {
                        Flow::Normal => {}
                        Flow::Exit(ExitKind::For) => break,
                        f => return Ok(f),
                    }
                    self.tick()?;
                }
            }
            Stmt::Do { cond, body } => loop {
                if let Some(c) = cond.as_ref().filter(|c| c.pre) {
                    if self.truth(&c.expr)? == c.until {
                        break;
                    }
                }
                match self.exec_block(body)? {
                    Flow::Normal => {}
                    Flow::Exit(ExitKind::Do) => break,
                    f => return Ok(f),
                }
                self.tick()?;
                if let Some(c) = cond.as_ref().filter(|c| !c.pre) {
                    if self.truth(&c.expr)? == c.until {
                        break;
                    }
                }
            },
            Stmt::With { obj, body } => {
                let o = self.eval(obj)?;
                self.frame().with.push(o);
                let res = self.exec_block(body);
                self.frame().with.pop();
                return res;
            }
            Stmt::Exit(k) => return Ok(Flow::Exit(*k)),
            Stmt::OnError(mode) => {
                let f = self.frame();
                f.on_error = mode.clone();
                if *mode == OnError::Off || matches!(mode, OnError::ResumeNext) {
                    // Also ends a handler that didn't Resume.
                    f.in_handler = false;
                }
                self.err = ErrState::default();
            }
            Stmt::Resume(k) => return Ok(Flow::Resume(k.clone())),
            Stmt::GoTo(l) => return Ok(Flow::GoTo(l.clone())),
            Stmt::Label(_) => {}
            Stmt::Erase(list) => {
                for e in list {
                    let Expr::Name(n) = e else { return Err(Error::type_mismatch()) };
                    let s = self.lookup(n).ok_or_else(Error::type_mismatch)?;
                    let mut v = s.borrow_mut();
                    let ty = v.ty;
                    if let Value::Array(a) = &mut v.val {
                        let a = Rc::make_mut(a);
                        for x in &mut a.data {
                            *x = ty.default_value();
                        }
                    }
                }
            }
            Stmt::Print(parts, _) => {
                let mut out = Vec::new();
                for p in parts {
                    let v = self.eval_scalar(p)?;
                    out.push(if matches!(v, Value::Null) { "Null".to_string() } else { v.to_str()? });
                }
                let text = out.join(" ");
                self.ui.debug_print(&text);
            }
            Stmt::End => return Err(Error::new(END, "End").fatal()),
            Stmt::Unsupported(what) => return Err(Error::unsupported(what)),
        }
        Ok(Flow::Normal)
    }

    /// Items of a For Each: array elements, cells, sheets, collection items.
    fn items_of(&mut self, g: Value) -> R<Box<dyn Iterator<Item = R<Value>>>> {
        Ok(match g {
            Value::Array(a) => Box::new(a.data.clone().into_iter().map(Ok)),
            Value::Object(Obj::Range(r)) => {
                let rg = r.range;
                let sheet = r.sheet;
                match r.by {
                    By::Rows => Box::new((rg.start.row..=rg.end.row).map(move |row| Ok(Value::Object(Obj::Range(RangeRef { sheet, range: Range::new(apex_sheet::CellRef::new(row, rg.start.col), apex_sheet::CellRef::new(row, rg.end.col)), by: By::Rows }))))),
                    By::Cols => Box::new((rg.start.col..=rg.end.col).map(move |col| Ok(Value::Object(Obj::Range(RangeRef { sheet, range: Range::new(apex_sheet::CellRef::new(rg.start.row, col), apex_sheet::CellRef::new(rg.end.row, col)), by: By::Cols }))))),
                    By::Cells => {
                        let cols = rg.cols() as u64;
                        Box::new((0..rg.cell_count()).map(move |i| {
                            let c = apex_sheet::CellRef::new(rg.start.row + (i / cols) as u32, rg.start.col + (i % cols) as u32);
                            Ok(Value::Object(Obj::Range(RangeRef::new(sheet, Range::single(c)))))
                        }))
                    }
                }
            }
            Value::Object(Obj::Sheets) | Value::Object(Obj::Workbook) => Box::new((0..self.wb.sheets.len()).map(|i| Ok(Value::Object(Obj::Sheet(i))))),
            Value::Object(Obj::Collection(c)) => Box::new(c.borrow().items.iter().map(|x| Ok(x.1.clone())).collect::<Vec<_>>().into_iter()),
            Value::Object(Obj::Dictionary(d)) => Box::new(d.borrow().keys.clone().into_iter().map(Ok)),
            Value::Nothing => return Err(Error::not_set()),
            _ => return Err(Error::new(92, "For Each may only iterate over a collection object or an array")),
        })
    }

    // --- assignment ------------------------------------------------------------------

    pub fn assign(&mut self, target: &Expr, v: Value, set: bool) -> R<()> {
        match target {
            Expr::Name(n) => {
                let s = self.target_slot(n)?;
                self.store(&s, v, set)
            }
            Expr::Call(callee, args) => match &**callee {
                Expr::Name(n) if self.lookup(n).is_some_and(|s| !(s.borrow().ret && !args.is_empty())) => {
                    let s = self.lookup(n).ok_or_else(Error::subscript)?;
                    let is_array = matches!(s.borrow().val, Value::Array(_));
                    if is_array {
                        let idx = self.indexes(args)?;
                        let (ty, v) = {
                            let ty = s.borrow().ty;
                            let v = match v {
                                v @ (Value::Object(_) | Value::Nothing) if set => v,
                                v @ (Value::Object(_) | Value::Nothing) => self.scalar(v)?,
                                v => v,
                            };
                            (ty, v)
                        };
                        let v = if set { v } else { v.coerce(ty)? };
                        let mut b = s.borrow_mut();
                        let Value::Array(a) = &mut b.val else { return Err(Error::type_mismatch()) };
                        let a = Rc::make_mut(a);
                        let at = a.offset(&idx)?;
                        a.data[at] = v;
                        Ok(())
                    } else {
                        let o = s.borrow().val.clone();
                        if set { Err(Error::type_mismatch()) } else { self.set_member(o, "item", args, v) }
                    }
                }
                Expr::Name(n) => {
                    let obj = self.call_name(n, args, false)?;
                    self.set_default(obj, v)
                }
                Expr::Member(o, name) => {
                    let o = self.eval(o)?;
                    self.set_member(o, name, args, v)
                }
                Expr::WithMember(name) => {
                    let o = self.with_obj()?;
                    self.set_member(o, name, args, v)
                }
                other => {
                    let o = self.eval(other)?;
                    self.set_member(o, "item", args, v)
                }
            },
            Expr::Member(o, name) => {
                let o = self.eval(o)?;
                self.set_member(o, name, &[], v)
            }
            Expr::WithMember(name) => {
                let o = self.with_obj()?;
                self.set_member(o, name, &[], v)
            }
            _ => Err(Error::new(0, "Invalid assignment target")),
        }
    }

    fn indexes(&mut self, args: &[Arg]) -> R<Vec<i64>> {
        let mut idx = Vec::with_capacity(args.len());
        for a in args {
            let e = a.expr.as_ref().ok_or_else(Error::subscript)?;
            idx.push(self.eval_scalar(e)?.to_i64()?);
        }
        Ok(idx)
    }

    pub fn with_obj(&self) -> R<Value> {
        self.frames.last().and_then(|f| f.with.last().cloned()).ok_or_else(|| Error::new(0, "Invalid or unqualified reference"))
    }

    // --- expressions -----------------------------------------------------------------

    pub fn truth(&mut self, e: &Expr) -> R<bool> {
        match self.eval_scalar(e)? {
            Value::Null => Ok(false),
            v => v.to_bool(),
        }
    }

    /// An expression whose objects become their default value (a Range
    /// its Value).
    pub fn eval_scalar(&mut self, e: &Expr) -> R<Value> {
        let v = self.eval(e)?;
        self.scalar(v)
    }

    /// The right side of a Let: objects give their value.
    fn eval_let(&mut self, e: &Expr) -> R<Value> {
        let v = self.eval(e)?;
        match v {
            Value::Object(Obj::Collection(_) | Obj::Dictionary(_)) | Value::Nothing => Ok(v),
            v => self.scalar(v),
        }
    }

    pub fn scalar(&mut self, v: Value) -> R<Value> {
        match v {
            Value::Object(o) => self.default_value(o),
            Value::Nothing => Err(Error::not_set()),
            v => Ok(v),
        }
    }

    pub fn eval(&mut self, e: &Expr) -> R<Value> {
        if stack_here().abs_diff(self.stack_base) > self.max_stack {
            return Err(Error::new(28, "Out of stack space").fatal());
        }
        match e {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Name(n) => self.call_name(n, &[], false),
            Expr::Member(o, name) => {
                let o = self.eval(o)?;
                self.member(o, name, &[])
            }
            Expr::WithMember(name) => {
                let o = self.with_obj()?;
                self.member(o, name, &[])
            }
            Expr::Call(t, args) => self.call(t, args, false),
            Expr::Paren(e) => self.eval(e),
            Expr::Neg(e) => {
                let v = self.eval_scalar(e)?;
                Ok(match v {
                    Value::Null => Value::Null,
                    Value::Empty => Value::Int(0),
                    Value::Int(i) => value::num(-(i as f64), true),
                    Value::Long(i) => value::num(-(i as f64), true),
                    Value::Date(d) => Value::Date(-d),
                    Value::Bool(b) => Value::Int(if b { 1 } else { 0 }),
                    v => Value::Double(-v.to_f64()?),
                })
            }
            Expr::Not(e) => {
                let v = self.eval_scalar(e)?;
                Ok(match v {
                    Value::Null => Value::Null,
                    Value::Bool(b) => Value::Bool(!b),
                    v => value::num(!v.to_i64()? as f64, true),
                })
            }
            Expr::Bin(BinOp::Is, a, b) => {
                let (a, b) = (self.eval(a)?, self.eval(b)?);
                Ok(Value::Bool(match (a, b) {
                    (Value::Nothing, Value::Nothing) => true,
                    (Value::Object(x), Value::Object(y)) => x.same(&y),
                    (Value::Object(_), Value::Nothing) | (Value::Nothing, Value::Object(_)) => false,
                    _ => return Err(Error::object_required()),
                }))
            }
            Expr::Bin(op, a, b) => {
                let a = self.eval_scalar(a)?;
                let b = self.eval_scalar(b)?;
                self.binop(*op, a, b)
            }
            Expr::New(class) => self.new_object(class),
            Expr::TypeOf(e, class) => {
                let v = self.eval(e)?;
                let name = v.type_name().to_lowercase();
                Ok(Value::Bool(match &v {
                    Value::Object(_) => class == "object" || name == *class || class == "sheet" && name == "worksheet",
                    _ => false,
                }))
            }
            Expr::Me => match self.owners.get(self.module()) {
                Some(Owner::Sheet(s)) => Ok(Value::Object(Obj::Sheet(*s))),
                Some(Owner::Workbook) => Ok(Value::Object(Obj::Workbook)),
                _ => Err(Error::new(0, "Invalid use of Me keyword")),
            },
        }
    }

    pub fn binop(&mut self, op: BinOp, a: Value, b: Value) -> R<Value> {
        use BinOp::*;
        let text = self.compare_text();
        if matches!(op, Eq | Ne | Lt | Gt | Le | Ge) {
            let o = compare(&a, &b, text)?;
            let Some(o) = o else { return Ok(Value::Null) };
            return Ok(Value::Bool(match op {
                Eq => o.is_eq(),
                Ne => o.is_ne(),
                Lt => o.is_lt(),
                Gt => o.is_gt(),
                Le => o.is_le(),
                _ => o.is_ge(),
            }));
        }
        if matches!(a, Value::Null) || matches!(b, Value::Null) {
            return Ok(if op == Concat { Value::str(if let Value::Null = a { b.to_str().unwrap_or_default() } else { a.to_str().unwrap_or_default() }) } else { Value::Null });
        }
        let integral = value::is_integral(&a) && value::is_integral(&b);
        Ok(match op {
            Concat => {
                let (a, b) = (a.to_str()?, b.to_str()?);
                crate::value::string_room(a.len() + b.len())?;
                Value::str(format!("{a}{b}"))
            }
            Like => Value::Bool(value::like(&a.to_str()?, &b.to_str()?, text)),
            Add => match (&a, &b) {
                (Value::Str(x), Value::Str(y)) => {
                    crate::value::string_room(x.len() + y.len())?;
                    Value::str(format!("{x}{y}"))
                }
                (Value::Str(x), Value::Empty) | (Value::Empty, Value::Str(x)) => Value::Str(x.clone()),
                (Value::Date(x), y) | (y, Value::Date(x)) => Value::Date(x + y.to_f64()?),
                _ => value::num(a.to_f64()? + b.to_f64()?, integral),
            },
            Sub => match (&a, &b) {
                (Value::Date(x), Value::Date(y)) => Value::Double(x - y),
                (Value::Date(x), y) => Value::Date(x - y.to_f64()?),
                _ => value::num(a.to_f64()? - b.to_f64()?, integral),
            },
            Mul => value::num(a.to_f64()? * b.to_f64()?, integral),
            Div => {
                let d = b.to_f64()?;
                if d == 0.0 {
                    return Err(Error::div_zero());
                }
                Value::Double(a.to_f64()? / d)
            }
            IntDiv => {
                let d = b.to_i64()?;
                if d == 0 {
                    return Err(Error::div_zero());
                }
                value::num((a.to_i64()? / d) as f64, true)
            }
            Mod => {
                let d = b.to_i64()?;
                if d == 0 {
                    return Err(Error::div_zero());
                }
                value::num((a.to_i64()? % d) as f64, true)
            }
            Pow => {
                let r = a.to_f64()?.powf(b.to_f64()?);
                if r.is_nan() {
                    return Err(Error::invalid_call());
                }
                Value::Double(r)
            }
            And | Or | Xor | Eqv | Imp => {
                if let (Value::Bool(x), Value::Bool(y)) = (&a, &b) {
                    let (x, y) = (*x, *y);
                    Value::Bool(match op {
                        And => x && y,
                        Or => x || y,
                        Xor => x ^ y,
                        Eqv => x == y,
                        _ => !x || y,
                    })
                } else {
                    let (x, y) = (a.to_i64()?, b.to_i64()?);
                    value::num(match op {
                        And => x & y,
                        Or => x | y,
                        Xor => x ^ y,
                        Eqv => !(x ^ y),
                        _ => !x | y,
                    } as f64, true)
                }
            }
            Eq | Ne | Lt | Gt | Le | Ge | Is => unreachable!(),
        })
    }

    /// Calls (or reads) `target` with `args`; `stmt` when it is a statement.
    fn call(&mut self, target: &Expr, args: &[Arg], stmt: bool) -> R<Value> {
        match target {
            Expr::Name(n) => self.call_name(n, args, stmt),
            Expr::Member(o, name) => {
                let o = self.eval(o)?;
                self.member(o, name, args)
            }
            Expr::WithMember(name) => {
                let o = self.with_obj()?;
                self.member(o, name, args)
            }
            other => {
                let v = self.eval(other)?;
                if args.is_empty() && stmt {
                    return Ok(v);
                }
                self.index_value(v, args)
            }
        }
    }

    /// A name with arguments: a variable (indexed), a procedure, a VBA
    /// function or constant, or a global of Excel's object model.
    pub fn call_name(&mut self, n: &str, args: &[Arg], stmt: bool) -> R<Value> {
        if let Some(s) = self.lookup(n) {
            let is_ret = s.borrow().ret;
            if !(is_ret && (!args.is_empty() || stmt)) {
                let v = s.borrow().val.clone();
                if args.is_empty() {
                    return Ok(v);
                }
                return self.index_value(v, args);
            }
        }
        if let Some((m, p)) = self.find_proc(n) {
            return self.call_proc(m, p, args);
        }
        let project = self.project;
        if project.modules.iter().any(|m| m.declares.iter().any(|d| d == n)) {
            return Err(Error::unsupported(&format!("{n} (Declare, Windows API)")));
        }
        if let Some(mi) = project.modules.iter().position(|m| m.name.eq_ignore_ascii_case(n)) {
            match project.modules[mi].kind {
                ModuleKind::Form => return Err(Error::unsupported("UserForms")),
                ModuleKind::Class => return Err(Error::unsupported("class modules")),
                // Sheet1 (a code name), ThisWorkbook, Module1.
                _ if args.is_empty() => return Ok(Value::Object(self.module_object(mi))),
                _ => {}
            }
        }
        if args.is_empty()
            && let Some(i) = self.wb.sheets.iter().position(|s| s.code_name.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(n)))
        {
            return Ok(Value::Object(Obj::Sheet(i)));
        }
        if args.is_empty() {
            if let Some(v) = crate::consts::constant(n) {
                return Ok(v);
            }
        }
        if let Some(r) = self.builtin(n, args) {
            return r;
        }
        if let Some(r) = self.global(n, args) {
            return r;
        }
        if args.is_empty() && !stmt {
            if project.modules[self.module()].explicit {
                return Err(Error::new(0, format!("Variable not defined: {n}")));
            }
            let s = slot(VType::Variant, Value::Empty);
            self.frame().locals.insert(n.to_string(), s);
            return Ok(Value::Empty);
        }
        Err(Error::new(35, format!("Sub or Function not defined: {n}")))
    }

    /// `v(args)`: an array element or an object's default member.
    pub fn index_value(&mut self, v: Value, args: &[Arg]) -> R<Value> {
        match v {
            Value::Array(a) => {
                let idx = self.indexes(args)?;
                let at = a.offset(&idx)?;
                Ok(a.data[at].clone())
            }
            Value::Object(o) => self.member(Value::Object(o), "item", args),
            Value::Nothing => Err(Error::not_set()),
            _ => Err(Error::type_mismatch()),
        }
    }

    /// Arguments as they are (objects kept).
    pub fn arg_raw(&mut self, args: &[Arg]) -> R<Vec<Value>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(match &a.expr {
                Some(e) => self.eval(e)?,
                None => Value::Missing,
            });
        }
        Ok(out)
    }

    /// `New X` and CreateObject: VBA's collections; other classes are not
    /// supported.
    pub fn new_object(&mut self, class: &str) -> R<Value> {
        match class.to_lowercase().as_str() {
            "collection" | "vba.collection" => Ok(Value::Object(Obj::Collection(Default::default()))),
            "dictionary" | "scripting.dictionary" => Ok(Value::Object(Obj::Dictionary(Default::default()))),
            c => {
                if self.project.modules.iter().any(|m| m.kind == ModuleKind::Class && m.name.eq_ignore_ascii_case(c)) {
                    Err(Error::unsupported("class modules"))
                } else {
                    Err(Error::new(429, format!("ActiveX component can't create object: {c} (not supported in ApexDesk)")))
                }
            }
        }
    }

    /// Runs a procedure found by name ("Module.Proc" or "Proc") with values.
    pub fn run_named(&mut self, name: &str, values: Vec<Value>) -> R<Value> {
        let (module, proc) = self.project.find(name).ok_or_else(|| Error::app(format!("Cannot run the macro '{name}'")))?;
        let args: Vec<Arg> = values.into_iter().map(|v| Arg { name: None, expr: Some(Expr::Lit(v)) }).collect();
        // Literal arguments are evaluated in no frame: give them one.
        let proc = self.project.modules[module].procs[proc].clone();
        self.call_proc(module, proc, &args)
    }
}

/// Which sheet (or the workbook) a document module is the code of: by
/// name (Sheet1, Hoja1 → the first sheet; a sheet's own name) or by the
/// events it handles.
pub fn owner_for(m: &Module, wb: &Workbook) -> Owner {
    if m.kind != ModuleKind::Document {
        return Owner::None;
    }
    let lname = m.name.to_lowercase();
    if ["thisworkbook", "estelibro", "estelibrodetrabajo", "estelibro_de_trabajo", "diesearbeitsmappe", "ceclasseur", "questacartella_di_lavoro", "estapasta_de_trabalho", "estapastadetrabalho"].contains(&lname.as_str()) || m.procs.iter().any(|p| p.name.to_lowercase().starts_with("workbook_")) {
        return Owner::Workbook;
    }
    if let Some(i) = wb.sheets.iter().position(|s| s.code_name.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&m.name))) {
        return Owner::Sheet(i);
    }
    // Files with code names say which sheet each module is.
    if wb.sheets.iter().any(|s| s.code_name.is_some()) {
        return Owner::None;
    }
    if let Some(i) = wb.sheets.iter().position(|s| s.name.eq_ignore_ascii_case(&m.name)) {
        return Owner::Sheet(i);
    }
    let digits: String = m.name.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
    if let Ok(n) = digits.parse::<usize>() {
        if n >= 1 && n <= wb.sheets.len() {
            return Owner::Sheet(n - 1);
        }
    }
    Owner::Workbook
}
