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

mod class;
mod debugger;
mod udt;
mod userform;

pub(crate) use udt::WithObj;

/// A variable: its declared type (for arrays, of the elements) and value.
#[derive(Debug)]
pub struct Var {
    pub ty: VType,
    pub val: Value,
    pub konst: bool,
    pub array: bool,
    /// The return value of the function running (its name).
    pub ret: bool,
    /// The user-defined type it holds (its elements', for an array).
    pub udt: Option<Rc<str>>,
    /// `String * n`.
    pub fixed: Option<u32>,
}

pub type Slot = Rc<RefCell<Var>>;

fn slot(ty: VType, val: Value) -> Slot {
    Rc::new(RefCell::new(Var { ty, val, konst: false, array: false, ret: false, udt: None, fixed: None }))
}

/// What a statement leaves to the statements around it.
#[derive(Debug)]
pub enum Flow {
    Normal,
    Exit(ExitKind),
    GoTo(String),
    /// `Return` of a GoSub.
    Return,
    Resume(ResumeKind),
    /// The debugger moved the next statement to this line.
    Jump(u32),
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
    with: Vec<WithObj>,
    on_error: OnError,
    in_handler: bool,
    /// Top-level statement where the error being handled happened.
    err_pc: usize,
    /// Line of the statement running.
    line: u32,
    /// The object a class module's procedure runs for.
    me: Option<Rc<crate::object::Instance>>,
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

/// The workbook a macro works on: its own, or only to read (a function
/// called by a formula, which may not change cells).
pub enum Book<'a> {
    Mut(&'a mut Workbook),
    Ref(&'a Workbook),
    /// Another workbook the macro opened.
    Owned(Box<Workbook>),
}

impl std::ops::Deref for Book<'_> {
    type Target = Workbook;
    fn deref(&self) -> &Workbook {
        match self {
            Book::Mut(w) => w,
            Book::Ref(w) => w,
            Book::Owned(w) => w,
        }
    }
}

impl Book<'_> {
    /// The same workbook, for a shorter while.
    pub fn reborrow(&mut self) -> Book<'_> {
        match self {
            Book::Mut(w) => Book::Mut(w),
            Book::Ref(w) => Book::Ref(w),
            Book::Owned(w) => Book::Mut(w),
        }
    }
}

pub struct Interp<'a> {
    pub project: &'a Project,
    pub wb: Book<'a>,
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
    /// OnKey and OnTime, for the application after the macro.
    pub requests: Vec<crate::Request>,
    /// A Change handler runs: what it writes doesn't run one again (no
    /// endless loop when it forgets EnableEvents).
    pub(crate) in_change: bool,
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
    pub(crate) paused: Duration,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Other workbooks' code (Application.Run "Book!Macro").
    pub libraries: Vec<crate::Library>,
    /// The cell whose formula called the function running
    /// (Application.Caller).
    pub caller: Option<crate::object::RangeRef>,
    /// ActiveWindow.SplitRow and SplitColumn set, for FreezePanes.
    pub split: Option<(u32, u32)>,
    /// What Copy or Cut (true) without a destination put aside.
    pub clip: Option<(RangeRef, bool)>,
    dbg: Option<Box<debugger::State>>,
    /// The object the next procedure called runs for (a method call).
    next_me: Option<Rc<crate::object::Instance>>,
    /// The WithEvents variables, for RaiseEvent.
    sinks: Vec<class::Sink>,
    /// Every object of a class made, weakly (see `let_go_instances`).
    instances: Vec<std::rc::Weak<crate::object::Instance>>,
    prune_at: usize,
    /// The lists Range.AdvancedFilter filtered in place, for ShowAllData.
    pub(crate) filtered: Vec<(usize, apex_sheet::Range)>,
    /// What Worksheet.Sort holds, by sheet: SortFields, SetRange, Header.
    pub(crate) sorts: HashMap<usize, crate::object::SortState>,
    /// The chart selected or added last: ActiveChart.
    pub(crate) active_chart: Option<(usize, u32)>,
    /// Files the macro opened, and what it may reach.
    pub(crate) files: crate::files::Files,
    pub(crate) dialogs_shown: u32,
    /// Workbook.SaveAs: the path and how many changes there were then.
    pub(crate) saved_as: Option<(String, usize)>,
    interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The UserForms loaded, and the designs of the form modules.
    pub(crate) forms: userform::Forms,
    pub(crate) form_designs: Vec<(String, std::sync::Arc<crate::forms::Design>)>,
    /// Other workbooks opened (None once closed: numbers aren't reused).
    pub(crate) books: Vec<Option<crate::excel::books::BookState<'a>>>,
    /// The one unqualified `Range`, `ActiveSheet`... refer to (None: the
    /// macro's own).
    pub(crate) active_book: Option<usize>,
    /// The one swapped in (None: the macro's own).
    pub(crate) current_book: Option<usize>,
    /// The macro's own workbook's file while another one is swapped in.
    pub(crate) outer_book: Option<std::path::PathBuf>,
    /// The workbook `clip` is in (None: the macro's own).
    pub(crate) clip_in: Option<usize>,
    /// Cells copied in another workbook, for a paste in the one swapped in.
    pub(crate) carried: Option<apex_sheet::Clip>,
    /// The last Range.Find, for FindNext.
    pub(crate) find_with: Option<crate::excel::FindWith>,
    /// The document of a Word macro (None: a workbook's).
    pub(crate) word: Option<crate::word::WordState<'a>>,
}

impl<'a> Interp<'a> {
    pub fn new(project: &'a Project, wb: &'a mut Workbook, ui: &'a mut dyn Ui, opts: &Options) -> R<Interp<'a>> {
        Self::with_book(project, Book::Mut(wb), ui, opts)
    }

    pub fn with_book(project: &'a Project, wb: Book<'a>, ui: &'a mut dyn Ui, opts: &Options) -> R<Interp<'a>> {
        value::set_locale(wb.locale);
        let owners = project.modules.iter().map(|m| owner_for(m, &wb)).collect();
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
            requests: Vec::new(),
            in_change: false,
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
            libraries: opts.libraries.clone(),
            caller: None,
            split: None,
            clip: None,
            dbg: None,
            next_me: None,
            instances: Vec::new(),
            sinks: Vec::new(),
            prune_at: 1024,
            filtered: Vec::new(),
            sorts: HashMap::new(),
            active_chart: None,
            files: crate::files::Files::new(&opts.file_path, &opts.file_name, &opts.files),
            dialogs_shown: 0,
            saved_as: None,
            interrupt: opts.interrupt.clone(),
            forms: userform::Forms::default(),
            form_designs: opts.forms.clone(),
            books: Vec::new(),
            active_book: None,
            current_book: None,
            outer_book: None,
            clip_in: None,
            carried: None,
            find_with: None,
            word: None,
        };
        it.init_modules()?;
        if let Some(d) = &opts.debug {
            it.dbg = Some(Box::new(debugger::State::new(project, d)));
        }
        Ok(it)
    }

    /// Module-level variables and constants.
    fn init_modules(&mut self) -> R<()> {
        let project = self.project;
        for (mi, m) in project.modules.iter().enumerate() {
            // A class's variables are each object's (see new_instance).
            if m.kind == ModuleKind::Class {
                continue;
            }
            for (v, _) in &m.vars {
                let s = self.new_var(v, m.base)?;
                if v.withevents {
                    self.listen(&s, None, mi, &v.name);
                }
                self.module_vars[mi].insert(v.name.clone(), s);
            }
        }
        // Constants may use earlier ones (and those of other modules).
        for (mi, m) in project.modules.iter().enumerate() {
            for (name, e, _) in &m.consts {
                self.frames.push(Frame { module: mi, proc: Rc::new(Proc { name: String::new(), kind: ProcKind::Sub, public: false, params: Vec::new(), ret: VType::Variant, ret_name: None, body: Vec::new(), line: 0, is_static: false }), locals: HashMap::new(), with: Vec::new(), on_error: OnError::Off, in_handler: false, err_pc: 0, line: 0, me: None });
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
        // Often enough that steps on huge strings or arrays (tens of
        // milliseconds each) can't overrun the time by much.
        if self.steps % 64 == 0 {
            if self.started.elapsed().saturating_sub(self.paused) > self.max_time {
                return Err(Error::new(18, format!("The macro was stopped: it ran for more than {} seconds", self.max_time.as_secs())).fatal());
            }
            if self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(Error::new(18, "The macro was stopped").fatal());
            }
            if self.interrupt.as_ref().is_some_and(|c| c.swap(false, std::sync::atomic::Ordering::Relaxed)) && !self.interrupt() {
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

    pub(crate) fn module(&self) -> usize {
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

    fn bounds_of(&mut self, bounds: &[(Option<Expr>, Expr)], base: i64) -> R<Vec<(i64, i64)>> {
        let mut b = Vec::with_capacity(bounds.len());
        for (lo, hi) in bounds {
            let lo = match lo {
                Some(e) => self.eval_scalar(e)?.to_i64()?,
                None => base,
            };
            b.push((lo, self.eval_scalar(hi)?.to_i64()?));
        }
        Ok(b)
    }

    /// What a new element of a variable declared `v` holds when it isn't
    /// its type's default: a record, a fixed-length string. With the
    /// user-defined type's name.
    pub(crate) fn first_value(&mut self, ty_name: Option<&str>, fixed: Option<u32>) -> R<(Option<Value>, Option<Rc<str>>)> {
        let rec = match ty_name {
            Some(n) => self.new_record(n)?,
            None => None,
        };
        let udt = rec.as_ref().and(ty_name).map(Rc::from);
        Ok((rec.or_else(|| fixed.map(udt::blank_fixed)), udt))
    }

    fn new_var(&mut self, v: &DimVar, base: i64) -> R<Slot> {
        let (one, udt) = match &v.new {
            Some(_) => (None, None),
            None => self.first_value(v.ty_name.as_deref(), v.fixed)?,
        };
        let ty = if udt.is_some() { VType::Variant } else { v.ty };
        let s = if let Some(bounds) = &v.bounds {
            let b = self.bounds_of(bounds, base)?;
            let mut arr = Array::new(b, ty)?;
            if let Some(one) = &one {
                arr.data.fill(one.clone());
            }
            let s = slot(ty, Value::Array(Rc::new(arr)));
            s.borrow_mut().array = true;
            s
        } else {
            let val = match (&v.new, one) {
                (Some(class), _) => self.new_object(class)?,
                (None, Some(one)) => one,
                (None, None) => ty.default_value(),
            };
            slot(ty, val)
        };
        {
            let mut b = s.borrow_mut();
            b.udt = udt;
            b.fixed = v.fixed;
        }
        Ok(s)
    }

    /// The variable `name` in scope: local, module, then public ones.
    pub fn lookup(&self, name: &str) -> Option<Slot> {
        let f = self.frames.last()?;
        if let Some(s) = f.locals.get(name) {
            return Some(s.clone());
        }
        if let Some(me) = &f.me
            && let Some(s) = me.vars.borrow().get(name)
        {
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
        let (ty, array, konst, cur_obj, udt, fixed) = {
            let b = s.borrow();
            (b.ty, b.array, b.konst, matches!(b.val, Value::Object(Obj::Range(_))), b.udt.clone(), b.fixed)
        };
        if konst {
            return Err(Error::new(0, "Assignment to constant not permitted"));
        }
        if !array && (udt.is_some() || fixed.is_some()) {
            let v = self.fit(ty, fixed, udt.as_deref(), v, set)?;
            s.borrow_mut().val = v;
            return Ok(());
        }
        if set {
            if !v.is_object() {
                return Err(Error::object_required());
            }
            if ty != VType::Variant && ty != VType::Object {
                return Err(Error::type_mismatch());
            }
            let old = std::mem::replace(&mut s.borrow_mut().val, v);
            return self.terminate(&old, 1);
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
        // Taken before the arguments, which may call other procedures; a
        // class's procedure calling another of it keeps its Me.
        let me = self.next_me.take().or_else(|| self.frames.last().filter(|f| f.module == module).and_then(|f| f.me.clone()));
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
            let (one, udt) = self.first_value(proc.ret_name.as_deref(), None)?;
            let ty = if udt.is_some() { VType::Variant } else { proc.ret };
            let s = slot(ty, one.unwrap_or_else(|| ty.default_value()));
            {
                let mut b = s.borrow_mut();
                b.ret = true;
                b.udt = udt;
            }
            locals.insert(lname.clone(), s.clone());
            Some(s)
        } else {
            None
        };
        self.frames.push(Frame { module, proc: proc.clone(), locals, with: Vec::new(), on_error: OnError::Off, in_handler: false, err_pc: 0, line: 0, me });
        let res = self.run_body(&proc.body);
        let f = self.frames.pop();
        let out = ret.map(|s| std::mem::take(&mut s.borrow_mut().val)).unwrap_or_default();
        res?;
        if let Some(f) = f {
            self.terminate_locals(f.locals)?;
        }
        Ok(out)
    }

    /// Calls an event handler with the values Excel passes; returns them
    /// as the handler left them (`Cancel = True`).
    pub fn call_event(&mut self, module: usize, proc: Rc<Proc>, values: Vec<Value>) -> R<Vec<Value>> {
        if self.frames.len() >= MAX_DEPTH || stack_here().abs_diff(self.stack_base) > self.max_stack {
            return Err(Error::new(28, "Out of stack space").fatal());
        }
        if proc.params.len() != values.len() || proc.params.iter().any(|p| p.paramarray) {
            return Err(Error::app(format!("Procedure declaration does not match description of event: {}", proc.name)).at(&self.project.modules[module].name, proc.line));
        }
        let mut locals = HashMap::new();
        let mut slots = Vec::new();
        for (p, v) in proc.params.iter().zip(values) {
            let v = match v {
                v @ (Value::Object(_) | Value::Nothing) if matches!(p.ty, VType::Object | VType::Variant) => v,
                v @ (Value::Object(_) | Value::Nothing) => self.scalar(v)?.coerce(p.ty)?,
                v => v.coerce(p.ty)?,
            };
            let s = slot(p.ty, v);
            slots.push(s.clone());
            locals.insert(p.name.clone(), s);
        }
        self.frames.push(Frame { module, proc: proc.clone(), locals, with: Vec::new(), on_error: OnError::Off, in_handler: false, err_pc: 0, line: 0, me: None });
        let res = self.run_body(&proc.body);
        self.frames.pop();
        res?;
        Ok(slots.iter().map(|s| s.borrow().val.clone()).collect())
    }

    /// Runs an event procedure of a form with `me` and its arguments
    /// already made.
    pub(crate) fn run_handler(&mut self, module: usize, proc: Rc<Proc>, locals: HashMap<String, Slot>, me: Option<Rc<crate::object::Instance>>) -> R<()> {
        if self.frames.len() >= MAX_DEPTH || stack_here().abs_diff(self.stack_base) > self.max_stack {
            return Err(Error::new(28, "Out of stack space").fatal());
        }
        self.frames.push(Frame { module, proc: proc.clone(), locals, with: Vec::new(), on_error: OnError::Off, in_handler: false, err_pc: 0, line: 0, me });
        let res = self.run_body(&proc.body);
        let f = self.frames.pop();
        res?;
        if let Some(f) = f {
            self.terminate_locals(f.locals)?;
        }
        Ok(())
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
                Ok(Flow::Return) => {
                    let line = body.get(pc).map_or(0, |l| l.line);
                    return Err(Error::new(3, "Return without GoSub").at(self.module_name(), line));
                }
                Ok(Flow::Jump(line)) => pc = body.iter().position(|l| l.line == line).ok_or_else(|| Error::new(0, "Cannot move the next statement there"))?,
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
            match self.exec_line(&body[*pc])? {
                Flow::Normal => *pc += 1,
                // A line of this block: the debugger moved there.
                Flow::Jump(line) if body.iter().any(|l| l.line == line) => *pc = body.iter().position(|l| l.line == line).unwrap_or(0),
                f => return Ok(f),
            }
        }
        Ok(Flow::Normal)
    }

    /// GoSub: the procedure from `label` until `Return`, then on after
    /// the GoSub. Falling off the end ends the procedure, like VBA.
    fn gosub(&mut self, label: &str) -> R<Flow> {
        if stack_here().abs_diff(self.stack_base) > self.max_stack {
            return Err(Error::new(28, "Out of stack space").fatal());
        }
        let proc = self.frame().proc.clone();
        let mut pc = Self::find_label(&proc.body, label)?;
        match self.exec_top(&proc.body, &mut pc)? {
            Flow::Return => Ok(Flow::Normal),
            Flow::Normal => Ok(Flow::Exit(ExitKind::Sub)),
            f => Ok(f),
        }
    }

    fn exec_block(&mut self, body: &[Line]) -> R<Flow> {
        let mut pc = 0;
        self.exec_top(body, &mut pc)
    }

    fn exec_line(&mut self, l: &Line) -> R<Flow> {
        self.tick().map_err(|e| e.at(self.module_name(), l.line))?;
        self.frame().line = l.line;
        if self.dbg.is_some() {
            if let Some(f) = self.before_line(l)? {
                return Ok(f);
            }
        }
        loop {
            match self.exec(&l.stmt) {
                Ok(f) => return Ok(f),
                Err(e) => {
                    let e = e.at(self.module_name(), l.line);
                    if !e.fatal && self.frame().on_error == OnError::ResumeNext {
                        self.set_err(&e);
                        return Ok(Flow::Normal);
                    }
                    if !e.fatal && self.debugging() && !self.handled() {
                        // Stopped at the error: continuing runs it again.
                        match self.pause(debugger::Reason::Error(e), l.line)? {
                            None => continue,
                            Some(f) => return Ok(f),
                        }
                    }
                    return Err(e);
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
                    let b = self.bounds_of(v.bounds.as_deref().unwrap_or(&[]), base)?;
                    if !v.fields.is_empty() {
                        self.redim_field(v, b, *preserve)?;
                        continue;
                    }
                    let s = match self.lookup(&v.name) {
                        Some(s) => s,
                        None => {
                            let s = slot(v.ty, Value::Empty);
                            self.frame().locals.insert(v.name.clone(), s.clone());
                            s
                        }
                    };
                    // The elements a record or a fixed-length string, new
                    // ones too.
                    let (ty_name, fixed) = {
                        let var = s.borrow();
                        (v.ty_name.clone().or_else(|| var.udt.as_deref().map(str::to_string)), v.fixed.or(var.fixed))
                    };
                    let (one, udt) = self.first_value(ty_name.as_deref(), fixed)?;
                    let mut var = s.borrow_mut();
                    let ty = if udt.is_some() { VType::Variant } else if v.ty != VType::Variant { v.ty } else { var.ty };
                    let mut arr = match (&var.val, preserve) {
                        (Value::Array(old), true) if !old.bounds.is_empty() => {
                            let mut a = old.resized(b)?;
                            if let Some(one) = &one {
                                // Elements of records or fixed strings are
                                // never Empty: these are the new ones.
                                for x in a.data.iter_mut().filter(|x| matches!(x, Value::Empty)) {
                                    *x = one.clone();
                                }
                            }
                            a
                        }
                        _ => {
                            let mut a = Array::new(b, ty)?;
                            if let Some(one) = &one {
                                a.data.fill(one.clone());
                            }
                            a
                        }
                    };
                    arr.ty = ty;
                    var.ty = ty;
                    var.array = true;
                    var.udt = udt;
                    var.fixed = fixed;
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
                let o = self.with_of(obj)?;
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
            Stmt::GoSub(l) => return self.gosub(l),
            Stmt::RaiseEvent(name, args) => self.raise_event(name, args)?,
            Stmt::Return => return Ok(Flow::Return),
            Stmt::Align { target, value, right } => {
                let cur = self.eval_scalar(target)?.to_str()?;
                let v = self.eval_scalar(value)?.to_str()?;
                let len = cur.chars().count();
                let have = v.chars().count().min(len);
                let pad = " ".repeat(len - have);
                let v: String = v.chars().take(len).collect();
                let out = if *right { pad + &v } else { v + &pad };
                self.assign(target, Value::str(out), false)?;
            }
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
            Stmt::Print(items) => {
                // Like `Print #` (a `;` or `,` at the end doesn't hold the line).
                let (text, _, _) = self.print_line(items, 0)?;
                self.ui.debug_print(&text.replace("\r\n", "\n"));
            }
            Stmt::End => return Err(Error::new(END, "End").fatal()),
            Stmt::Stop => {
                if !self.debugging() {
                    return Err(Error::new(END, "End").fatal());
                }
                let line = self.frame().line;
                if let Some(f) = self.pause(debugger::Reason::Stop, line)? {
                    return Ok(f);
                }
            }
            Stmt::Assert(e) => {
                if self.debugging() && !self.truth(e)? {
                    let line = self.frame().line;
                    if let Some(f) = self.pause(debugger::Reason::Assert, line)? {
                        return Ok(f);
                    }
                }
            }
            Stmt::File(f) => self.file_stmt(f)?,
            Stmt::Unsupported(what) => return Err(Error::unsupported(what)),
        }
        Ok(Flow::Normal)
    }

    /// Items of a For Each: array elements, cells, sheets, collection items.
    pub(crate) fn items_of(&mut self, g: Value) -> R<Box<dyn Iterator<Item = R<Value>>>> {
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
            Value::Object(Obj::Multi(items)) => {
                let mut all = Vec::with_capacity(items.len());
                for o in items.iter() {
                    all.push(self.items_of(Value::Object(o.clone()))?);
                }
                Box::new(all.into_iter().flatten())
            }
            Value::Object(Obj::Part(p)) if let Some(items) = self.part_items(&p) => Box::new(items.into_iter().map(Ok)),
            Value::Object(Obj::In(b, o)) => return self.items_in(b, *o),
            Value::Object(Obj::Word(o)) => Box::new(self.word_items(*o)?.into_iter().map(Ok)),
            Value::Object(Obj::Workbook) if self.word.is_some() => Box::new(self.word_items(crate::word::WObj::Doc)?.into_iter().map(Ok)),
            Value::Object(Obj::Workbooks) => {
                let mut all = vec![Ok(Value::Object(Obj::Workbook))];
                all.extend((0..self.books.len()).filter(|&b| self.books[b].is_some()).map(|b| Ok(Value::Object(Obj::In(b, Box::new(Obj::Workbook))))));
                Box::new(all.into_iter())
            }
            Value::Object(Obj::Sheets) | Value::Object(Obj::Workbook) => Box::new((0..self.wb.sheets.len()).map(|i| Ok(Value::Object(Obj::Sheet(i))))),
            Value::Object(Obj::Collection(c)) => Box::new(c.borrow().items.iter().map(|x| Ok(x.1.clone())).collect::<Vec<_>>().into_iter()),
            Value::Object(Obj::Dictionary(d)) => Box::new(d.borrow().keys.clone().into_iter().map(Ok)),
            Value::Object(Obj::Controls(i, k, pages)) => Box::new(self.controls_items(&i, k, pages)?.into_iter().map(Ok)),
            Value::Object(Obj::Instance(i)) if i.form.is_some() => Box::new(self.controls_items(&i, None, false)?.into_iter().map(Ok)),
            Value::Nothing => return Err(Error::not_set()),
            _ => return Err(Error::new(92, "For Each may only iterate over a collection object or an array")),
        })
    }

    // --- assignment ------------------------------------------------------------------

    pub fn assign(&mut self, target: &Expr, v: Value, set: bool) -> R<()> {
        // Into a record or an array of them (or of fixed-length strings).
        if matches!(target, Expr::Member(..) | Expr::Call(..) | Expr::WithMember(_))
            && let Some((s, steps)) = self.path(target)?
            && !steps.is_empty()
        {
            return self.put(&s, &steps, v, set);
        }
        if let Expr::Call(f, args) = target
            && let Expr::Name(n) = &**f
            && (n == "mid" || n == "mid$")
            && self.lookup(n).is_none()
        {
            return self.mid_stmt(args, v);
        }
        match target {
            Expr::Name(n) => {
                // A control or property of the form whose code runs.
                if self.lookup(n).is_none()
                    && let Some(me) = self.form_me()
                    && self.form_knows(&me, n)
                {
                    return self.form_set(me, n, v);
                }
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

    /// `Mid(s, start[, length]) = text`: characters of `s` replaced in
    /// place, its length kept.
    fn mid_stmt(&mut self, args: &[Arg], v: Value) -> R<()> {
        let [s, start, rest @ ..] = args else { return Err(Error::new(450, "Wrong number of arguments or invalid property assignment")) };
        let (Some(se), Some(start)) = (&s.expr, &start.expr) else { return Err(Error::new(5, "Invalid procedure call or argument")) };
        if rest.len() > 1 {
            return Err(Error::new(450, "Wrong number of arguments or invalid property assignment"));
        }
        let cur: Vec<char> = self.eval_scalar(se)?.to_str()?.chars().collect();
        let start = self.eval_scalar(start)?.to_i64()?;
        let len = match rest.first().and_then(|a| a.expr.as_ref()) {
            Some(e) => Some(self.eval_scalar(e)?.to_i64()?),
            None => None,
        };
        if start < 1 || start as usize > cur.len() || len.is_some_and(|l| l < 0) {
            return Err(Error::new(5, "Invalid procedure call or argument"));
        }
        let at = start as usize - 1;
        let new: Vec<char> = self.scalar(v)?.to_str()?.chars().collect();
        let n = new.len().min(cur.len() - at).min(len.map_or(usize::MAX, |l| l as usize));
        let mut out = cur;
        out[at..at + n].copy_from_slice(&new[..n]);
        self.assign(se, Value::str(out.into_iter().collect::<String>()), false)
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
        let w = self.frames.last().and_then(|f| f.with.last()).ok_or_else(|| Error::new(0, "Invalid or unqualified reference"))?;
        udt::with_value(w)
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
                    Value::Object(Obj::Instance(i)) => class == "object" || self.instance_is(i, class) || i.form.is_some() && class.trim_start_matches("msforms.") == "userform",
                    Value::Object(Obj::Control(..)) => matches!(class.trim_start_matches("msforms.").as_ref(), "object" | "control") || name == class.trim_start_matches("msforms."),
                    Value::Object(_) => class == "object" || name == *class || class == "sheet" && name == "worksheet",
                    _ => false,
                }))
            }
            Expr::Me if self.frames.last().is_some_and(|f| f.me.is_some()) => Ok(Value::Object(Obj::Instance(self.frames.last().and_then(|f| f.me.clone()).ok_or_else(Error::not_set)?))),
            Expr::Me if self.word.is_some() && self.owners.get(self.module()) == Some(&Owner::Workbook) => Ok(crate::word::wo(crate::word::WObj::Doc)),
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
                // Too long refused before copying (a copy is what takes long).
                if let (Value::Str(x), Value::Str(y)) = (&a, &b) {
                    crate::value::string_room(x.len() + y.len())?;
                }
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

    /// A function of `Declare`: the harmless ones acted out (see
    /// [`crate::api`]), the rest refused.
    fn windows_api(&mut self, d: &Declare, args: &[Arg]) -> R<Value> {
        if !crate::api::harmless(&d.lib, &d.api) {
            return Err(Error::unsupported(&format!("{} (Declare, Windows API)", d.name)));
        }
        let ms = crate::api::uptime() * 1000.0;
        Ok(match d.api.as_str() {
            // Like Application.Wait: time isn't spent.
            "sleep" | "messagebeep" => Value::Long(1),
            "gettickcount" | "timegettime" => Value::Long(ms as u32 as i32),
            "gettickcount64" => Value::Double(ms.floor()),
            _ => {
                let v = if d.api == "queryperformancefrequency" { crate::api::FREQUENCY } else { (ms * crate::api::FREQUENCY / 1000.0).floor() };
                if let Some(e) = args.first().and_then(|a| a.expr.as_ref()) {
                    self.assign(e, Value::Double(v), false)?;
                }
                Value::Long(1)
            }
        })
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
        // A control of the form whose code runs.
        if let Some(me) = self.form_me() {
            let control = me.form.as_ref().is_some_and(|v| v.borrow().find(n).is_some());
            if control {
                return self.form_member(me, n, args);
            }
        }
        if let Some((m, p)) = self.find_proc(n) {
            return self.call_proc(m, p, args);
        }
        // The form's own members (Hide, Caption...) by themselves.
        if let Some(me) = self.form_me()
            && (args.is_empty() || n != "left")
            && self.form_knows(&me, n)
        {
            return self.form_member(me, n, args);
        }
        if (n == "load" || n == "unload") && args.len() == 1 {
            return self.load_statement(n == "unload", args);
        }
        let project = self.project;
        if let Some(d) = project.modules.iter().flat_map(|m| &m.declares).find(|d| d.name == n) {
            return self.windows_api(d, args);
        }
        if let Some(mi) = project.modules.iter().position(|m| m.name.eq_ignore_ascii_case(n)) {
            match project.modules[mi].kind {
                ModuleKind::Form => {
                    let inst = Value::Object(Obj::Instance(self.form_default(mi)?));
                    return if args.is_empty() { Ok(inst) } else { self.index_value(inst, args) };
                }
                ModuleKind::Class => return Err(Error::new(0, format!("Invalid use of the class name {}: make an object with New", project.modules[mi].name))),
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
            "filesystemobject" | "scripting.filesystemobject" => self.new_fso(),
            c => {
                if let Some(mi) = self.project.modules.iter().position(|m| matches!(m.kind, ModuleKind::Class | ModuleKind::Form) && m.name.eq_ignore_ascii_case(c)) {
                    self.new_instance(mi)
                } else {
                    Err(Error::new(429, format!("ActiveX component can't create object: {c} (not supported in ApexDesk)")))
                }
            }
        }
    }

    /// Runs a procedure found by name ("Module.Proc" or "Proc") with values.
    /// Macro `name` of another workbook's code `lib`, on this workbook:
    /// an interpreter of its own, within what is left of this one's
    /// limits; its changes join this macro's.
    /// The workbook, to change: not in a function called by a formula.
    pub fn wb_mut(&mut self) -> R<&mut Workbook> {
        match &mut self.wb {
            Book::Mut(w) => Ok(w),
            Book::Owned(w) => Ok(w),
            Book::Ref(_) => Err(Error::new(1004, "A function called from a cell can't change the workbook")),
        }
    }

    pub fn run_library(&mut self, lib: &crate::Library, name: &str, values: Vec<Value>) -> R<Value> {
        let used = self.stack_base.abs_diff(stack_here());
        // What this one may still use (it can't call back into another).
        let opts = crate::Options {
            active_sheet: self.active,
            selection: self.selection,
            file_name: self.file_name.clone(),
            file_path: self.file_path.clone(),
            user_name: self.user_name.clone(),
            utc_offset: self.utc_offset,
            max_steps: self.max_steps.saturating_sub(self.steps),
            max_time: self.max_time.saturating_sub(self.started.elapsed().saturating_sub(self.paused)),
            max_stack: self.max_stack.saturating_sub(used + (64 << 10)),
            cancel: self.cancel.clone(),
            debug: None,
            interrupt: self.interrupt.clone(),
            libraries: Vec::new(),
            files: self.files.access(),
            forms: Vec::new(),
        };
        let project = crate::Project::parse(&lib.sources)?;
        let (mi, pi) = project.find(name).ok_or_else(|| Error::app(format!("Cannot run the macro '{}!{name}'", lib.name)))?;
        let mut it = Interp::with_book(&project, self.wb.reborrow(), &mut *self.ui, &opts)?;
        it.app = self.app.clone();
        let args: Vec<Arg> = values.into_iter().map(|v| Arg { name: None, expr: Some(Expr::Lit(v)) }).collect();
        let proc = project.modules[mi].procs[pi].clone();
        let r = it.call_proc(mi, proc, &args);
        let (undo, active, selection, save, app, requests, steps) = (std::mem::take(&mut it.undo), it.active, it.selection, it.save, it.app.clone(), std::mem::take(&mut it.requests), it.steps);
        drop(it);
        self.undo.extend(undo);
        self.active = active;
        self.selection = selection;
        self.save |= save;
        self.app = app;
        let room = 1000usize.saturating_sub(self.requests.len());
        self.requests.extend(requests.into_iter().take(room).map(|r| r.in_book(&lib.name)));
        self.steps += steps;
        r
    }

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
    if wb.code_name.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&m.name)) || ["thisworkbook", "estelibro", "estelibrodetrabajo", "estelibro_de_trabajo", "diesearbeitsmappe", "ceclasseur", "questacartella_di_lavoro", "estapasta_de_trabalho", "estapastadetrabalho"].contains(&lname.as_str()) || m.procs.iter().any(|p| p.name.to_lowercase().starts_with("workbook_")) {
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

impl Drop for Interp<'_> {
    fn drop(&mut self) {
        self.let_go_instances();
    }
}
