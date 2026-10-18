//! Class modules: `New Class` makes an object with its own copy of the
//! module's variables; its public procedures are its methods, Property
//! Get/Let/Set its properties and its public variables its fields.
//! `Implements IShape` lets it answer for `IShape_Area` as `Area`.
//! Class_Initialize runs when it is made; Class_Terminate when the last
//! variable holding it lets it go (Set to Nothing, or the procedure ends).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::{Interp, Slot, stack_here};
use crate::ast::{Arg, Expr, Module, Proc, ProcKind};
use crate::error::{Error, R};
use crate::object::{Instance, Obj};
use crate::value::Value;

/// The procedure `name` of a class module: a method or Property Get
/// (`assign`: a Property Let/Set), public ones only from outside;
/// else the one an interface it implements gives (`IShape_Area`).
fn method<'p>(m: &'p Module, name: &str, assign: bool, outside: bool) -> Option<&'p Rc<Proc>> {
    let fits = |p: &&Rc<Proc>| (p.kind == ProcKind::PropertyLet) == assign;
    m.procs.iter().filter(fits).find(|p| p.name.eq_ignore_ascii_case(name) && (p.public || !outside)).or_else(|| {
        m.implements.iter().find_map(|i| {
            let full = format!("{i}_{name}");
            m.procs.iter().filter(fits).find(|p| p.name.eq_ignore_ascii_case(&full))
        })
    })
}

/// A WithEvents variable: the events of the object it holds run
/// `var_Event` of `module`, for `owner` when that is a class's object.
pub(crate) struct Sink {
    slot: std::rc::Weak<RefCell<super::Var>>,
    owner: Option<std::rc::Weak<Instance>>,
    module: usize,
    var: String,
}

impl Interp<'_> {
    /// A new object of the class module `mi`.
    pub(crate) fn new_instance(&mut self, mi: usize) -> R<Value> {
        let inst = self.make_instance(mi)?;
        self.initialize(&inst)?;
        Ok(Value::Object(Obj::Instance(inst)))
    }

    /// A new object of module `mi`, before its Initialize.
    pub(crate) fn make_instance(&mut self, mi: usize) -> R<Rc<Instance>> {
        // `Dim x As New Same` among its own variables would never end.
        if stack_here().abs_diff(self.stack_base) > self.max_stack {
            return Err(Error::new(28, "Out of stack space").fatal());
        }
        let project = self.project;
        let m = &project.modules[mi];
        let mut vars = HashMap::with_capacity(m.vars.len());
        for (v, _) in &m.vars {
            vars.insert(v.name.clone(), self.new_var(v, m.base)?);
        }
        let inst = Rc::new(Instance { module: mi, class: m.name.as_str().into(), vars: RefCell::new(vars), form: self.form_view(mi).map(RefCell::new) });
        self.track(&inst);
        for (v, _) in m.vars.iter().filter(|(v, _)| v.withevents) {
            let s = inst.vars.borrow().get(&v.name).cloned();
            if let Some(s) = s {
                self.listen(&s, Some(&inst), mi, &v.name);
            }
        }
        Ok(inst)
    }

    /// Keeps a weak hold of every object made, to break the cycles they
    /// may form when the macro ends.
    fn track(&mut self, inst: &Rc<Instance>) {
        if self.instances.len() >= self.prune_at {
            self.instances.retain(|w| w.strong_count() > 0);
            self.prune_at = (self.instances.len() * 2).max(1024);
        }
        self.instances.push(Rc::downgrade(inst));
    }

    /// Registers a WithEvents variable.
    pub(crate) fn listen(&mut self, s: &Slot, owner: Option<&Rc<Instance>>, module: usize, var: &str) {
        if self.sinks.len() >= 1024 && self.sinks.len().is_power_of_two() {
            self.sinks.retain(|k| k.slot.strong_count() > 0);
        }
        self.sinks.push(Sink { slot: Rc::downgrade(s), owner: owner.map(Rc::downgrade), module, var: var.to_string() });
    }

    /// `RaiseEvent name(args)`: the handlers of the WithEvents variables
    /// holding this object, with the arguments as written (ByRef ones
    /// come back, like `Cancel`).
    pub(crate) fn raise_event(&mut self, name: &str, args: &[Arg]) -> R<()> {
        let Some(me) = self.frames.last().and_then(|f| f.me.clone()) else {
            return Err(Error::new(0, "RaiseEvent is only valid in a class module"));
        };
        let holds = |k: &Sink| k.slot.upgrade().is_some_and(|s| matches!(&s.borrow().val, Value::Object(Obj::Instance(i)) if Rc::ptr_eq(i, &me)));
        let found: Vec<(Option<Option<Rc<Instance>>>, usize, String)> = self.sinks.iter().filter(|k| holds(k)).map(|k| (k.owner.as_ref().map(|w| w.upgrade()), k.module, format!("{}_{name}", k.var))).collect();
        let project = self.project;
        for (owner, module, handler) in found {
            let Some(p) = project.modules[module].procs.iter().find(|p| p.kind == ProcKind::Sub && p.name.eq_ignore_ascii_case(&handler)) else { continue };
            match owner {
                Some(Some(o)) => {
                    self.call_method(o, p.clone(), args)?;
                }
                // Its object is gone.
                Some(None) => {}
                None => {
                    self.call_proc(module, p.clone(), args)?;
                }
            }
        }
        Ok(())
    }

    /// Runs a procedure of a class with `inst` as Me.
    pub(crate) fn call_method(&mut self, inst: Rc<Instance>, p: Rc<Proc>, args: &[Arg]) -> R<Value> {
        let mi = inst.module;
        self.next_me = Some(inst);
        self.call_proc(mi, p, args)
    }

    /// The public variable `name` of an object.
    fn field_slot(&self, inst: &Instance, name: &str) -> Option<Slot> {
        let m = &self.project.modules[inst.module];
        if !m.vars.iter().any(|(v, public)| *public && v.name.eq_ignore_ascii_case(name)) {
            return None;
        }
        inst.vars.borrow().get(&name.to_lowercase()).cloned()
    }

    /// `obj.name(args)`: a method, a Property Get or a public variable.
    pub(crate) fn instance_member(&mut self, inst: Rc<Instance>, name: &str, args: &[Arg]) -> R<Value> {
        let project = self.project;
        let m = &project.modules[inst.module];
        if let Some(p) = method(m, name, false, true) {
            return self.call_method(inst, p.clone(), args);
        }
        if let Some(s) = self.field_slot(&inst, name) {
            let v = s.borrow().val.clone();
            return if args.is_empty() { Ok(v) } else { self.index_value(v, args) };
        }
        if inst.form.is_some() {
            return self.form_member(inst, name, args);
        }
        Err(Error::no_member(name))
    }

    /// `obj.name(args) = v`: a Property Let/Set or a public variable.
    pub(crate) fn instance_set(&mut self, inst: Rc<Instance>, name: &str, args: &[Arg], v: Value) -> R<()> {
        let project = self.project;
        let m = &project.modules[inst.module];
        if let Some(p) = method(m, name, true, true) {
            let mut all = args.to_vec();
            all.push(Arg { name: None, expr: Some(Expr::Lit(v)) });
            return self.call_method(inst, p.clone(), &all).map(|_| ());
        }
        if args.is_empty()
            && let Some(s) = self.field_slot(&inst, name)
        {
            let set = v.is_object();
            return self.store(&s, v, set);
        }
        if inst.form.is_some() {
            return if args.is_empty() { self.form_set(inst, name, v) } else { Err(Error::no_member(name)) };
        }
        Err(Error::no_member(name))
    }

    /// Whether an object is of class `class` or implements it (TypeOf).
    pub(crate) fn instance_is(&self, inst: &Instance, class: &str) -> bool {
        inst.class.eq_ignore_ascii_case(class) || self.project.modules[inst.module].implements.iter().any(|i| i.eq_ignore_ascii_case(class))
    }

    /// Class_Terminate of `v` when nothing else holds it (`held`: the
    /// references of the variable letting it go).
    pub(crate) fn terminate(&mut self, v: &Value, held: usize) -> R<()> {
        let Value::Object(Obj::Instance(i)) = v else { return Ok(()) };
        if Rc::strong_count(i) != held {
            return Ok(());
        }
        if i.form.is_some() {
            let i = i.clone();
            return self.unload_form(&i, None).map(|_| ());
        }
        let project = self.project;
        match project.modules[i.module].proc("class_terminate") {
            Some(p) => self.call_method(i.clone(), p.clone(), &[]).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Class_Terminate of the objects only the locals of an ended
    /// procedure held.
    pub(crate) fn terminate_locals(&mut self, locals: HashMap<String, Slot>) -> R<()> {
        let mut res = Ok(());
        for (_, s) in locals {
            if Rc::strong_count(&s) != 1 || s.borrow().ret {
                continue;
            }
            let v = std::mem::take(&mut s.borrow_mut().val);
            let r = self.terminate(&v, 1);
            if res.is_ok() {
                res = r;
            }
        }
        res
    }

    /// Breaks the cycles objects may form (a list whose last node points
    /// at the first), so they are freed.
    pub(crate) fn let_go_instances(&mut self) {
        let alive: Vec<Rc<Instance>> = self.instances.drain(..).filter_map(|w| w.upgrade()).collect();
        for i in &alive {
            if let Ok(mut vars) = i.vars.try_borrow_mut() {
                let taken: Vec<Slot> = vars.drain().map(|(_, s)| s).collect();
                drop(vars);
                drop(taken);
            }
        }
    }
}
