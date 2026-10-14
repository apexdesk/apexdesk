//! User-defined types (`Type ... End Type`). A variable of one holds a
//! record by value: copied on assignment, its fields read like members and
//! written through the variable (`p.x = 1`, `pts(2).name = "a"`).

use std::rc::Rc;

use super::{Interp, Slot};
use crate::ast::{Arg, Expr, ModuleKind, TypeDef};
use crate::error::{Error, R};
use crate::value::{Array, Field, Record, VType, Value, fixed_len};

/// Types that hold themselves (directly or not) stop at this depth.
const MAX_NESTING: usize = 32;

/// A step from a variable into what it holds: a field (with the indexes
/// when the field is an array, or the arguments for an object it holds)
/// or the indexes of an array variable.
#[derive(Clone, Debug)]
pub(crate) enum Step {
    Field(Rc<str>, Vec<Value>),
    Index(Vec<Value>),
}

/// The object of a With block: a value, or a record reached through a
/// variable, read again at each use so `.x = 1: .y = .x` sees the change.
#[derive(Clone, Debug)]
pub(crate) enum WithObj {
    Value(Value),
    Place(Slot, Vec<Step>),
}

/// What a place takes: a variable's or a field's type.
struct Kind {
    ty: VType,
    fixed: Option<u32>,
    udt: Option<Rc<str>>,
    /// The whole of an array (assigning one array to it).
    array: bool,
}

/// Where a walk ended: the value, or an object met with the steps left.
enum Walk {
    Done(Value, Kind),
    Object(Value, usize),
}

/// A new `String * len`: null characters, like VBA.
pub(crate) fn blank_fixed(len: u32) -> Value {
    Value::str("\0".repeat(len as usize))
}

fn index_of(args: &[Value]) -> R<Vec<i64>> {
    args.iter().map(|v| v.to_i64()).collect()
}

/// Follows `steps` from `root` without changing anything.
fn walk(root: &Value, root_kind: Kind, steps: &[Step]) -> R<Walk> {
    let mut cur = root.clone();
    let mut kind = root_kind;
    for (k, st) in steps.iter().enumerate() {
        if matches!(cur, Value::Object(_) | Value::Nothing) {
            return Ok(Walk::Object(cur, k));
        }
        match (&cur, st) {
            (Value::Record(r), Step::Field(name, args)) => {
                let f = r.field(name)?;
                let next = match (&f.val, args.is_empty()) {
                    (v, true) => {
                        kind = Kind { ty: f.ty, fixed: f.fixed, udt: f.udt.clone(), array: matches!(v, Value::Array(_)) };
                        v.clone()
                    }
                    (Value::Array(a), false) => {
                        kind = Kind { ty: f.ty, fixed: f.fixed, udt: f.udt.clone(), array: false };
                        a.data[a.offset(&index_of(args)?)?].clone()
                    }
                    (v @ Value::Object(_), false) => return Ok(Walk::Object(v.clone(), k)),
                    _ => return Err(Error::type_mismatch()),
                };
                cur = next;
            }
            (Value::Array(a), Step::Index(args)) => {
                let next = a.data[a.offset(&index_of(args)?)?].clone();
                kind.array = false;
                cur = next;
            }
            (Value::Record(_), Step::Index(_)) => return Err(Error::type_mismatch()),
            _ => return Err(Error::new(424, "Object required")),
        }
    }
    Ok(Walk::Done(cur, kind))
}

impl<'a> Interp<'a> {
    /// The user-defined type `name` seen from the running module: its
    /// own, then the public ones of standard modules.
    pub(crate) fn find_type(&self, name: &str) -> Option<&'a TypeDef> {
        let project = self.project;
        let same = |t: &&TypeDef| t.name.eq_ignore_ascii_case(name);
        project.modules.get(self.module()).and_then(|m| m.types.iter().find(same)).or_else(|| project.modules.iter().filter(|m| m.kind == ModuleKind::Standard).flat_map(|m| &m.types).filter(|t| t.public).find(same))
    }

    /// A new value of the user-defined type `name`; None when it isn't one.
    pub(crate) fn new_record(&mut self, name: &str) -> R<Option<Value>> {
        self.record_of(name, 0)
    }

    fn record_of(&mut self, name: &str, depth: usize) -> R<Option<Value>> {
        let Some(def) = self.find_type(name) else { return Ok(None) };
        if depth > MAX_NESTING {
            return Err(Error::new(0, format!("User-defined type contains itself: {}", def.name)));
        }
        let base = self.project.modules[self.module()].base;
        let mut fields = Vec::with_capacity(def.fields.len());
        for f in &def.fields {
            let rec = match &f.ty_name {
                Some(n) => self.record_of(n, depth + 1)?,
                None => None,
            };
            let udt: Option<Rc<str>> = rec.as_ref().and(f.ty_name.as_deref()).map(Rc::from);
            let ty = if udt.is_some() { VType::Variant } else { f.ty };
            let one = rec.or_else(|| f.fixed.map(blank_fixed));
            let val = match &f.bounds {
                Some(b) => {
                    let b = self.bounds_of(b, base)?;
                    let mut a = Array::new(b, ty)?;
                    if let Some(one) = &one {
                        a.data.fill(one.clone());
                    }
                    Value::Array(Rc::new(a))
                }
                None => one.unwrap_or_else(|| ty.default_value()),
            };
            fields.push(Field { name: f.name.as_str().into(), ty, fixed: f.fixed, udt, val });
        }
        Ok(Some(Value::Record(Rc::new(Record { name: def.name.as_str().into(), fields }))))
    }

    /// `v` as what a place of `ty` takes (Let, or Set when `set`).
    pub(crate) fn fit(&mut self, ty: VType, fixed: Option<u32>, udt: Option<&str>, v: Value, set: bool) -> R<Value> {
        if let Some(n) = udt {
            return match v {
                Value::Record(r) if !set && r.name.eq_ignore_ascii_case(n) => Ok(Value::Record(r)),
                _ => Err(Error::type_mismatch()),
            };
        }
        if set {
            if !v.is_object() {
                return Err(Error::object_required());
            }
            if ty != VType::Variant && ty != VType::Object {
                return Err(Error::type_mismatch());
            }
            return Ok(v);
        }
        let v = match v {
            Value::Object(_) | Value::Nothing if ty == VType::Object => return Err(Error::new(91, "Object variable not set (use Set)")),
            v @ (Value::Object(_) | Value::Nothing) => self.scalar(v)?,
            Value::Array(_) if ty != VType::Variant => return Err(Error::type_mismatch()),
            v => v,
        };
        let v = v.coerce(ty)?;
        Ok(match (fixed, v) {
            (Some(n), Value::Str(s)) => fixed_len(&s, n),
            (_, v) => v,
        })
    }

    /// A field of a record value: `p.x`, `p.list(2)`.
    pub(crate) fn record_member(&mut self, r: &Record, name: &str, args: &[Arg]) -> R<Value> {
        let f = r.field(name)?;
        if args.is_empty() {
            return Ok(f.val.clone());
        }
        match &f.val {
            Value::Array(a) => {
                let idx = self.indexes(args)?;
                Ok(a.data[a.offset(&idx)?].clone())
            }
            v @ Value::Object(_) => {
                let v = v.clone();
                self.member(v, "item", args)
            }
            _ => Err(Error::type_mismatch()),
        }
    }

    /// Whether `s` holds what is written through steps (a record, an array
    /// of them or of fixed-length strings).
    fn holder(s: &Slot) -> bool {
        let b = s.borrow();
        b.udt.is_some() || b.fixed.is_some() || matches!(b.val, Value::Record(_))
    }

    fn arg_values(&mut self, args: &[Arg]) -> R<Vec<Value>> {
        args.iter()
            .map(|a| match &a.expr {
                Some(e) => self.eval_scalar(e),
                None => Ok(Value::Missing),
            })
            .collect()
    }

    /// The variable `e` goes into and the steps from it, when it is a
    /// record's part; the indexes are worked out here, once.
    pub(crate) fn path(&mut self, e: &Expr) -> R<Option<(Slot, Vec<Step>)>> {
        Ok(match e {
            Expr::Name(n) => self.lookup(n).filter(Self::holder).map(|s| (s, Vec::new())),
            Expr::Member(b, f) => self.path(b)?.map(|(s, mut st)| {
                st.push(Step::Field(f.as_str().into(), Vec::new()));
                (s, st)
            }),
            Expr::WithMember(f) => self.with_place().map(|(s, mut st)| {
                st.push(Step::Field(f.as_str().into(), Vec::new()));
                (s, st)
            }),
            Expr::Call(c, args) => match &**c {
                // A function's own name with arguments calls it again.
                Expr::Name(n) => match self.lookup(n).filter(|s| Self::holder(s) && !s.borrow().ret) {
                    Some(s) => {
                        let a = self.arg_values(args)?;
                        Some((s, vec![Step::Index(a)]))
                    }
                    None => None,
                },
                Expr::Member(b, f) => match self.path(b)? {
                    Some((s, mut st)) => {
                        st.push(Step::Field(f.as_str().into(), self.arg_values(args)?));
                        Some((s, st))
                    }
                    None => None,
                },
                Expr::WithMember(f) => match self.with_place() {
                    Some((s, mut st)) => {
                        st.push(Step::Field(f.as_str().into(), self.arg_values(args)?));
                        Some((s, st))
                    }
                    None => None,
                },
                _ => None,
            },
            _ => None,
        })
    }

    fn with_place(&self) -> Option<(Slot, Vec<Step>)> {
        match self.frames.last()?.with.last()? {
            WithObj::Place(s, st) => Some((s.clone(), st.clone())),
            WithObj::Value(_) => None,
        }
    }

    fn slot_kind(s: &Slot) -> Kind {
        let b = s.borrow();
        Kind { ty: b.ty, fixed: b.fixed, udt: b.udt.clone(), array: false }
    }

    /// The value at `steps` from `s` (objects met on the way asked for
    /// the rest as members).
    pub(crate) fn value_at(&mut self, s: &Slot, steps: &[Step]) -> R<Value> {
        let root = s.borrow().val.clone();
        match walk(&root, Self::slot_kind(s), steps)? {
            Walk::Done(v, _) => Ok(v),
            Walk::Object(mut o, k) => {
                for st in &steps[k..] {
                    let (name, args) = step_member(st);
                    o = self.member(o, name, &args)?;
                }
                Ok(o)
            }
        }
    }

    /// Assigns `v` at `steps` from `s`: the variable's copy of the record
    /// changes, or the object met on the way is given the rest.
    pub(crate) fn put(&mut self, s: &Slot, steps: &[Step], v: Value, set: bool) -> R<()> {
        let root = s.borrow().val.clone();
        let kind = match walk(&root, Self::slot_kind(s), steps)? {
            Walk::Done(_, kind) => kind,
            Walk::Object(mut o, k) => {
                let last = steps.len() - 1;
                for st in &steps[k..last] {
                    let (name, args) = step_member(st);
                    o = self.member(o, name, &args)?;
                }
                let (name, args) = step_member(&steps[last]);
                return self.set_member(o, name, &args, v);
            }
        };
        let v = if kind.array {
            match v {
                Value::Array(a) if kind.ty == VType::Variant || a.ty == kind.ty => Value::Array(a),
                _ => return Err(Error::new(0, "Can't assign to array")),
            }
        } else {
            self.fit(kind.ty, kind.fixed, kind.udt.as_deref(), v, set)?
        };
        let mut b = s.borrow_mut();
        let mut cur: &mut Value = &mut b.val;
        for st in steps {
            cur = match (cur, st) {
                (Value::Record(r), Step::Field(name, args)) => {
                    let f = Rc::make_mut(r).field_mut(name)?;
                    if args.is_empty() {
                        &mut f.val
                    } else {
                        let Value::Array(a) = &mut f.val else { return Err(Error::type_mismatch()) };
                        let a = Rc::make_mut(a);
                        let at = a.offset(&index_of(args)?)?;
                        &mut a.data[at]
                    }
                }
                (Value::Array(a), Step::Index(args)) => {
                    let a = Rc::make_mut(a);
                    let at = a.offset(&index_of(args)?)?;
                    &mut a.data[at]
                }
                _ => return Err(Error::type_mismatch()),
            };
        }
        *cur = v;
        Ok(())
    }

    /// The object for a With block.
    pub(crate) fn with_of(&mut self, e: &Expr) -> R<WithObj> {
        match self.path(e)? {
            Some((s, st)) if !st.is_empty() => {
                let v = self.value_at(&s, &st)?;
                Ok(if matches!(v, Value::Record(_)) { WithObj::Place(s, st) } else { WithObj::Value(v) })
            }
            Some((s, _)) if matches!(s.borrow().val, Value::Record(_)) => Ok(WithObj::Place(s, Vec::new())),
            Some((s, _)) => Ok(WithObj::Value(s.borrow().val.clone())),
            None => Ok(WithObj::Value(self.eval(e)?)),
        }
    }
}

/// A step as a member call on an object.
fn step_member(st: &Step) -> (&str, Vec<Arg>) {
    let (name, args) = match st {
        Step::Field(n, a) => (&**n, a),
        Step::Index(a) => ("item", a),
    };
    (name, args.iter().map(|v| Arg { name: None, expr: (!matches!(v, Value::Missing)).then(|| Expr::Lit(v.clone())) }).collect())
}

/// The current value of a With block's object.
pub(crate) fn with_value(w: &WithObj) -> R<Value> {
    match w {
        WithObj::Value(v) => Ok(v.clone()),
        WithObj::Place(s, st) => match walk(&s.borrow().val.clone(), Interp::slot_kind(s), st)? {
            Walk::Done(v, _) => Ok(v),
            Walk::Object(..) => Err(Error::type_mismatch()),
        },
    }
}

impl Interp<'_> {
    /// `ReDim [Preserve] p.list(bounds)`: a field of a record held by a variable.
    pub(crate) fn redim_field(&mut self, v: &crate::ast::DimVar, bounds: Vec<(i64, i64)>, preserve: bool) -> R<()> {
        let target = v.fields.iter().fold(Expr::Name(v.name.clone()), |e, f| Expr::Member(Box::new(e), f.clone()));
        let Some((s, steps)) = self.path(&target)?.filter(|(_, st)| !st.is_empty()) else {
            return Err(Error::new(0, format!("ReDim: {} isn't a field of a user-defined type", v.fields.join("."))));
        };
        let root = s.borrow().val.clone();
        let (cur, kind) = match walk(&root, Self::slot_kind(&s), &steps)? {
            Walk::Done(cur, kind) => (cur, kind),
            Walk::Object(..) => return Err(Error::type_mismatch()),
        };
        let (one, _) = self.first_value(kind.udt.as_deref(), kind.fixed)?;
        let mut arr = match (&cur, preserve) {
            (Value::Array(old), true) if !old.bounds.is_empty() => old.resized(bounds)?,
            _ => Array::new(bounds, kind.ty)?,
        };
        if let Some(one) = &one {
            for x in arr.data.iter_mut().filter(|x| matches!(x, Value::Empty)) {
                *x = one.clone();
            }
        }
        let ty = kind.ty;
        arr.ty = ty;
        self.put(&s, &steps, Value::Array(Rc::new(arr)), false)
    }
}
