//! Evaluates DAX measures in a filter context: filters on columns (or
//! whole tables) of the model flow from the "one" side of a relationship
//! to the "many" side, CALCULATE changes them, FILTER and the X functions
//! iterate rows (a row context), and a measure used inside a row context
//! filters by that row (context transition).

use std::cell::RefCell;
use std::sync::Arc;

use crate::analysis::Lang;
use crate::analysis::pivot::{self, ItemFilter, MeasureEval};
use crate::value::{ErrorKind, Value};
use crate::workbook::HashMap;

use super::dax::{self, BinOp, DaxError, DaxErrorKind, Expr, Func};
use super::data::{ModelData, key};
use super::Measure;

/// Rows of `table` allowed by a filter; `col` is the filtered column (None:
/// a filter on whole rows, as from FILTER or a row context).
#[derive(Clone, Debug)]
pub struct Filter {
    pub table: usize,
    pub col: Option<usize>,
    pub mask: Arc<Vec<bool>>,
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    pub filters: Vec<Filter>,
    /// Row contexts, innermost last: (table, row).
    rows: Vec<(usize, usize)>,
}

impl Context {
    pub fn new(filters: Vec<Filter>) -> Context {
        Context { filters, rows: Vec::new() }
    }

    fn with_row(&self, t: usize, r: usize) -> Context {
        let mut c = self.clone();
        c.rows.push((t, r));
        c
    }

    fn remove(&mut self, t: usize, col: Option<usize>) {
        self.filters.retain(|f| f.table != t || (col.is_some() && f.col != col));
    }
}

/// The rows a table expression yields.
enum Rows {
    /// Whole rows of a table.
    Table(usize, Vec<usize>),
    /// Distinct values of one column (a row standing for each).
    Column(usize, usize, Vec<usize>),
}

impl Rows {
    fn table(&self) -> usize {
        match self {
            Rows::Table(t, _) | Rows::Column(t, _, _) => *t,
        }
    }

    fn rows(&self) -> &[usize] {
        match self {
            Rows::Table(_, r) | Rows::Column(_, _, r) => r,
        }
    }
}

/// Measures parsed against a model: (name, expression or its error).
pub type Compiled = Vec<(String, Result<Expr, DaxError>)>;

/// Compiles every measure of the model.
pub fn compile_all(data: &ModelData, measures: &[Measure]) -> Compiled {
    measures.iter().map(|m| (m.name.clone(), compile(&m.formula, data, measures))).collect()
}

/// Evaluates expressions and measures over a model.
pub struct Engine<'a> {
    pub data: &'a ModelData,
    measures: &'a Compiled,
    /// Measures being evaluated (cycles).
    stack: RefCell<Vec<usize>>,
}

/// Parses and resolves a formula against the model and its measures.
pub fn compile(formula: &str, data: &ModelData, measures: &[Measure]) -> Result<Expr, DaxError> {
    let is_measure = |n: &str| measures.iter().any(|m| m.name.eq_ignore_ascii_case(n.trim()));
    let e = dax::parse(formula, &is_measure)?;
    dax::resolve(&e, data, &is_measure, None)
}

/// The first measure of `name` that leads back to itself, if any.
pub fn find_cycle(name: &str, measures: &[Measure], data: &ModelData) -> Option<String> {
    let is_measure = |n: &str| measures.iter().any(|m| m.name.eq_ignore_ascii_case(n.trim()));
    let refs = |n: &str| -> Vec<String> {
        let Some(m) = measures.iter().find(|m| m.name.eq_ignore_ascii_case(n)) else { return Vec::new() };
        let mut out = Vec::new();
        if let Ok(e) = dax::parse(&m.formula, &is_measure) {
            e.measures(&mut out);
        }
        let _ = data;
        out
    };
    let mut seen: Vec<String> = Vec::new();
    let mut todo = refs(name);
    while let Some(n) = todo.pop() {
        if n.eq_ignore_ascii_case(name) {
            return Some(name.to_string());
        }
        if seen.iter().any(|s| s.eq_ignore_ascii_case(&n)) {
            continue;
        }
        todo.extend(refs(&n));
        seen.push(n);
    }
    None
}

fn num(v: &Value) -> Result<Option<f64>, ErrorKind> {
    match v {
        Value::Empty => Ok(None),
        Value::Number(n) => Ok(Some(*n)),
        Value::Bool(b) => Ok(Some(*b as u8 as f64)),
        Value::Text(t) if t.is_empty() => Ok(None),
        Value::Text(t) => t.trim().parse::<f64>().map(Some).map_err(|_| ErrorKind::Value),
        Value::Error(e) => Err(*e),
        _ => Err(ErrorKind::Value),
    }
}

fn truth(v: &Value) -> Result<bool, ErrorKind> {
    match v {
        Value::Empty => Ok(false),
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => Ok(*n != 0.0),
        Value::Error(e) => Err(*e),
        _ => Err(ErrorKind::Value),
    }
}

fn number(x: f64) -> Value {
    if x.is_finite() { Value::Number(x) } else { Value::Error(ErrorKind::Num) }
}

/// DAX comparison: blank equals 0, "" and FALSE; text ignores case.
fn compare(a: &Value, b: &Value) -> Result<std::cmp::Ordering, ErrorKind> {
    use Value::*;
    if let (Error(e), _) | (_, Error(e)) = (a, b) {
        return Err(*e);
    }
    let text = |v: &Value| -> Option<String> {
        match v {
            Text(t) => Some(t.to_string()),
            Empty => Some(String::new()),
            _ => None,
        }
    };
    match (a, b) {
        (Text(_), _) | (_, Text(_)) => match (text(a), text(b)) {
            (Some(x), Some(y)) => Ok(crate::value::cmp_text_ci(&x, &y)),
            // A number against text compares its text.
            _ => {
                let s = |v: &Value| v.to_text().map(|t| t.to_string()).unwrap_or_default();
                Ok(crate::value::cmp_text_ci(&s(a), &s(b)))
            }
        },
        _ => {
            let x = num(a)?.unwrap_or(0.0);
            let y = num(b)?.unwrap_or(0.0);
            Ok(x.total_cmp(&y))
        }
    }
}

impl<'a> Engine<'a> {
    pub fn new(data: &'a ModelData, measures: &'a Compiled) -> Engine<'a> {
        Engine { data, measures, stack: RefCell::new(Vec::new()) }
    }

    fn measure_index(&self, name: &str) -> Option<usize> {
        self.measures.iter().position(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
    }

    /// The parse error of a measure, if it has one.
    pub fn error(&self, name: &str) -> Option<&DaxError> {
        self.measures.get(self.measure_index(name)?)?.1.as_ref().err()
    }

    /// Evaluates measure `name` in `ctx`.
    pub fn measure(&self, name: &str, ctx: &Context) -> Value {
        match self.measure_index(name) {
            Some(i) => self.eval_measure(i, ctx),
            None => Value::Error(ErrorKind::Name),
        }
    }

    fn eval_measure(&self, i: usize, ctx: &Context) -> Value {
        if self.stack.borrow().contains(&i) {
            return Value::Error(ErrorKind::Circular);
        }
        let Ok(e) = &self.measures[i].1 else { return Value::Error(ErrorKind::Name) };
        self.stack.borrow_mut().push(i);
        // A measure in a row context filters by that row.
        let v = if ctx.rows.is_empty() { self.eval(e, ctx) } else { self.eval(e, &self.transition(ctx)) };
        self.stack.borrow_mut().pop();
        v
    }

    /// Evaluates an expression compiled with [`compile`].
    pub fn eval(&self, e: &Expr, ctx: &Context) -> Value {
        match e {
            Expr::Num(n) => Value::Number(*n),
            Expr::Str(s) => Value::text(s.as_str()),
            Expr::Measure(m) => self.measure(m, ctx),
            Expr::Col { table, col } => {
                let Some((t, c)) = self.col(table.as_deref(), col) else { return Value::Error(ErrorKind::Ref) };
                match ctx.rows.iter().rev().find(|(rt, _)| *rt == t) {
                    Some((_, r)) => self.data.value(t, c, *r).clone(),
                    None => Value::Error(ErrorKind::Value),
                }
            }
            Expr::Table(_) => Value::Error(ErrorKind::Value),
            Expr::Neg(a) => match num(&self.eval(a, ctx)) {
                Ok(Some(x)) => Value::Number(-x),
                Ok(None) => Value::Empty,
                Err(k) => Value::Error(k),
            },
            Expr::Bin(op, a, b) => self.binary(*op, a, b, ctx),
            Expr::Call(f, args) => self.call(*f, args, ctx),
        }
    }

    fn col(&self, table: Option<&str>, col: &str) -> Option<(usize, usize)> {
        let t = self.data.table(table?)?;
        Some((t, self.data.column(t, col)?))
    }

    fn binary(&self, op: BinOp, a: &Expr, b: &Expr, ctx: &Context) -> Value {
        if matches!(op, BinOp::And | BinOp::Or) {
            let x = match truth(&self.eval(a, ctx)) {
                Ok(x) => x,
                Err(k) => return Value::Error(k),
            };
            if (op == BinOp::And && !x) || (op == BinOp::Or && x) {
                return Value::Bool(x);
            }
            return match truth(&self.eval(b, ctx)) {
                Ok(y) => Value::Bool(y),
                Err(k) => Value::Error(k),
            };
        }
        let (x, y) = (self.eval(a, ctx), self.eval(b, ctx));
        use std::cmp::Ordering::*;
        let cmp = |ok: &dyn Fn(std::cmp::Ordering) -> bool| match compare(&x, &y) {
            Ok(o) => Value::Bool(ok(o)),
            Err(k) => Value::Error(k),
        };
        match op {
            BinOp::Eq => cmp(&|o| o == Equal),
            BinOp::Ne => cmp(&|o| o != Equal),
            BinOp::Lt => cmp(&|o| o == Less),
            BinOp::Le => cmp(&|o| o != Greater),
            BinOp::Gt => cmp(&|o| o == Greater),
            BinOp::Ge => cmp(&|o| o != Less),
            BinOp::Cat => {
                let s = |v: &Value| match v {
                    Value::Number(n) => Ok(crate::format::general_number(*n, '.')),
                    Value::Error(e) => Err(*e),
                    v => v.to_text().map(|t| t.to_string()),
                };
                match (s(&x), s(&y)) {
                    (Ok(p), Ok(q)) => Value::text(p + &q),
                    (Err(k), _) | (_, Err(k)) => Value::Error(k),
                }
            }
            _ => {
                let (p, q) = match (num(&x), num(&y)) {
                    (Ok(p), Ok(q)) => (p, q),
                    (Err(k), _) | (_, Err(k)) => return Value::Error(k),
                };
                match op {
                    BinOp::Add | BinOp::Sub => match (p, q) {
                        (None, None) => Value::Empty,
                        _ => {
                            let (p, q) = (p.unwrap_or(0.0), q.unwrap_or(0.0));
                            number(if op == BinOp::Add { p + q } else { p - q })
                        }
                    },
                    BinOp::Mul => match (p, q) {
                        (Some(p), Some(q)) => number(p * q),
                        _ => Value::Empty,
                    },
                    BinOp::Div => match (p, q) {
                        (None, _) => Value::Empty,
                        (Some(_), None) => Value::Error(ErrorKind::Div0),
                        (Some(_), Some(q)) if q == 0.0 => Value::Error(ErrorKind::Div0),
                        (Some(p), Some(q)) => number(p / q),
                    },
                    _ => number(p.unwrap_or(0.0).powf(q.unwrap_or(0.0))),
                }
            }
        }
    }

    /// Whether any filter reaches table `t` (on it or on a table it relates to).
    fn filtered(&self, t: usize, ctx: &Context, depth: usize) -> bool {
        if ctx.filters.iter().any(|f| f.table == t) {
            return true;
        }
        depth < self.data.tables.len() && self.data.links.iter().any(|l| l.from == t && self.filtered(l.to, ctx, depth + 1))
    }

    /// The rows of `t` the filter context lets through.
    pub fn visible(&self, t: usize, ctx: &Context) -> Vec<bool> {
        self.visible_at(t, ctx, 0)
    }

    fn visible_at(&self, t: usize, ctx: &Context, depth: usize) -> Vec<bool> {
        let mut m = vec![true; self.data.rows(t)];
        for f in ctx.filters.iter().filter(|f| f.table == t) {
            for (x, y) in m.iter_mut().zip(f.mask.iter()) {
                *x &= *y;
            }
        }
        if depth < self.data.tables.len() {
            for l in self.data.links.iter().filter(|l| l.from == t) {
                if !self.filtered(l.to, ctx, depth + 1) {
                    continue;
                }
                let dm = self.visible_at(l.to, ctx, depth + 1);
                for (r, x) in m.iter_mut().enumerate() {
                    *x &= l.map.get(r).copied().flatten().is_some_and(|k| dm[k as usize]);
                }
            }
        }
        m
    }

    fn visible_rows(&self, t: usize, ctx: &Context) -> Vec<usize> {
        self.visible(t, ctx).iter().enumerate().filter(|(_, v)| **v).map(|(i, _)| i).collect()
    }

    /// Context transition: every row context becomes a filter on its row.
    fn transition(&self, ctx: &Context) -> Context {
        let mut c = Context { filters: ctx.filters.clone(), rows: Vec::new() };
        for &(t, r) in &ctx.rows {
            c.remove(t, None);
            let mut mask = vec![false; self.data.rows(t)];
            if let Some(x) = mask.get_mut(r) {
                *x = true;
            }
            c.filters.push(Filter { table: t, col: None, mask: Arc::new(mask) });
        }
        c
    }

    /// One row per distinct value of column `c` among `rows`.
    fn distinct(&self, t: usize, c: usize, rows: impl Iterator<Item = usize>) -> Vec<usize> {
        let mut seen = std::collections::HashSet::new();
        let mut blank = false;
        rows.filter(|r| match key(self.data.value(t, c, *r)) {
            Some(k) => seen.insert(k),
            None => !std::mem::replace(&mut blank, true),
        })
        .collect()
    }

    fn table(&self, e: &Expr, ctx: &Context) -> Option<Rows> {
        match e {
            Expr::Table(n) => {
                let t = self.data.table(n)?;
                Some(Rows::Table(t, self.visible_rows(t, ctx)))
            }
            Expr::Call(Func::All | Func::Values, a) => {
                let all = matches!(e, Expr::Call(Func::All, _));
                match &a[0] {
                    Expr::Col { table, col } => {
                        let (t, c) = self.col(table.as_deref(), col)?;
                        let rows = if all { self.distinct(t, c, 0..self.data.rows(t)) } else { self.distinct(t, c, self.visible_rows(t, ctx).into_iter()) };
                        Some(Rows::Column(t, c, rows))
                    }
                    Expr::Table(n) if all => {
                        let t = self.data.table(n)?;
                        Some(Rows::Table(t, (0..self.data.rows(t)).collect()))
                    }
                    other => self.table(other, ctx),
                }
            }
            Expr::Call(Func::Filter, a) => {
                let src = self.table(&a[0], ctx)?;
                let t = src.table();
                let keep: Vec<usize> = src.rows().iter().copied().filter(|r| truth(&self.eval(&a[1], &ctx.with_row(t, *r))).unwrap_or(false)).collect();
                Some(match src {
                    Rows::Table(..) => Rows::Table(t, keep),
                    Rows::Column(_, c, _) => Rows::Column(t, c, keep),
                })
            }
            _ => None,
        }
    }

    /// The values of a column argument over the visible rows.
    fn column_values(&self, e: &Expr, ctx: &Context) -> Option<(usize, usize, Vec<usize>)> {
        let Expr::Col { table, col } = e else { return None };
        let (t, c) = self.col(table.as_deref(), col)?;
        Some((t, c, self.visible_rows(t, ctx)))
    }

    fn call(&self, f: Func, a: &[Expr], ctx: &Context) -> Value {
        let err = |k| Value::Error(k);
        match f {
            Func::True => Value::Bool(true),
            Func::False => Value::Bool(false),
            Func::Blank => Value::Empty,
            Func::Sum | Func::Average | Func::Count | Func::CountA | Func::CountBlank | Func::DistinctCount | Func::Min | Func::Max if a.len() == 1 => {
                let Some((t, c, rows)) = self.column_values(&a[0], ctx) else { return err(ErrorKind::Ref) };
                let vals = rows.iter().map(|r| self.data.value(t, c, *r));
                match f {
                    Func::Count | Func::CountA => Value::Number(vals.filter(|v| !v.is_empty() && !matches!(v, Value::Text(s) if s.is_empty())).count() as f64),
                    Func::CountBlank => Value::Number(vals.filter(|v| v.is_empty() || matches!(v, Value::Text(s) if s.is_empty())).count() as f64),
                    Func::DistinctCount => Value::Number(self.distinct(t, c, rows.iter().copied()).len() as f64),
                    _ => {
                        let mut xs = Vec::new();
                        for v in vals {
                            match v {
                                Value::Number(n) => xs.push(*n),
                                Value::Error(e) => return err(*e),
                                _ => {}
                            }
                        }
                        if xs.is_empty() {
                            return Value::Empty;
                        }
                        match f {
                            Func::Sum => number(xs.iter().sum()),
                            Func::Average => number(xs.iter().sum::<f64>() / xs.len() as f64),
                            Func::Min => number(xs.iter().copied().fold(f64::INFINITY, f64::min)),
                            _ => number(xs.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
                        }
                    }
                }
            }
            Func::Sum | Func::Average | Func::Count | Func::CountA | Func::CountBlank | Func::DistinctCount => err(ErrorKind::Value),
            Func::Min | Func::Max => {
                let (x, y) = (self.eval(&a[0], ctx), self.eval(&a[1], ctx));
                match (num(&x), num(&y)) {
                    (Ok(None), Ok(None)) => Value::Empty,
                    (Ok(p), Ok(q)) => {
                        let (p, q) = (p.unwrap_or(0.0), q.unwrap_or(0.0));
                        Value::Number(if f == Func::Min { p.min(q) } else { p.max(q) })
                    }
                    (Err(k), _) | (_, Err(k)) => err(k),
                }
            }
            Func::CountRows => match self.table(&a[0], ctx) {
                Some(r) if r.rows().is_empty() => Value::Empty,
                Some(r) => Value::Number(r.rows().len() as f64),
                None => err(ErrorKind::Ref),
            },
            Func::SumX | Func::AverageX | Func::MinX | Func::MaxX | Func::CountX => {
                let Some(src) = self.table(&a[0], ctx) else { return err(ErrorKind::Ref) };
                let t = src.table();
                let mut xs = Vec::new();
                for r in src.rows() {
                    let v = self.eval(&a[1], &ctx.with_row(t, *r));
                    match num(&v) {
                        Ok(Some(x)) => xs.push(x),
                        Ok(None) => {}
                        Err(_) if f == Func::CountX && matches!(v, Value::Text(_)) => xs.push(0.0),
                        Err(k) => return err(k),
                    }
                }
                if f == Func::CountX {
                    return Value::Number(xs.len() as f64);
                }
                if xs.is_empty() {
                    return Value::Empty;
                }
                match f {
                    Func::SumX => number(xs.iter().sum()),
                    Func::AverageX => number(xs.iter().sum::<f64>() / xs.len() as f64),
                    Func::MinX => number(xs.iter().copied().fold(f64::INFINITY, f64::min)),
                    _ => number(xs.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
                }
            }
            Func::Divide => {
                let (x, y) = (self.eval(&a[0], ctx), self.eval(&a[1], ctx));
                match (num(&x), num(&y)) {
                    (Ok(_), Ok(None | Some(0.0))) => a.get(2).map_or(Value::Empty, |alt| self.eval(alt, ctx)),
                    (Ok(None), Ok(_)) => Value::Empty,
                    (Ok(Some(p)), Ok(Some(q))) => number(p / q),
                    (Err(k), _) | (_, Err(k)) => err(k),
                }
            }
            Func::Related => {
                let Expr::Col { table, col } = &a[0] else { return err(ErrorKind::Ref) };
                let Some((t, c)) = self.col(table.as_deref(), col) else { return err(ErrorKind::Ref) };
                for &(rt, r) in ctx.rows.iter().rev() {
                    if rt == t {
                        return self.data.value(t, c, r).clone();
                    }
                    if self.data.path(rt, t).is_some() {
                        return self.data.related_row(rt, r, t).map_or(Value::Empty, |x| self.data.value(t, c, x).clone());
                    }
                }
                err(ErrorKind::Value)
            }
            Func::Calculate => self.calculate(a, ctx),
            Func::Filter | Func::All | Func::Values => err(ErrorKind::Value),
            Func::If => match truth(&self.eval(&a[0], ctx)) {
                Ok(true) => self.eval(&a[1], ctx),
                Ok(false) => a.get(2).map_or(Value::Empty, |x| self.eval(x, ctx)),
                Err(k) => err(k),
            },
            Func::And | Func::Or => self.binary(if f == Func::And { BinOp::And } else { BinOp::Or }, &a[0], &a[1], ctx),
            Func::Not => match truth(&self.eval(&a[0], ctx)) {
                Ok(b) => Value::Bool(!b),
                Err(k) => err(k),
            },
            Func::IsBlank => Value::Bool(matches!(self.eval(&a[0], ctx), Value::Empty)),
            Func::Abs => match num(&self.eval(&a[0], ctx)) {
                Ok(Some(x)) => Value::Number(x.abs()),
                Ok(None) => Value::Empty,
                Err(k) => err(k),
            },
            Func::Round => match (num(&self.eval(&a[0], ctx)), num(&self.eval(&a[1], ctx))) {
                (Ok(x), Ok(d)) => {
                    let (x, d) = (x.unwrap_or(0.0), d.unwrap_or(0.0).trunc() as i32);
                    let p = 10f64.powi(d.abs());
                    number(if d >= 0 { (x * p).round() / p } else { (x / p).round() * p })
                }
                (Err(k), _) | (_, Err(k)) => err(k),
            },
        }
    }

    /// CALCULATE: its filters (evaluated in the outer context) replace the
    /// filters on the same columns or tables after context transition.
    fn calculate(&self, a: &[Expr], ctx: &Context) -> Value {
        let mut inner = self.transition(ctx);
        let mut adds: Vec<Filter> = Vec::new();
        for f in &a[1..] {
            match f {
                // ALL as a filter removes filters.
                Expr::Call(Func::All, x) => match &x[0] {
                    Expr::Table(n) => {
                        // The expanded table: the related "one" sides too.
                        if let Some(t) = self.data.table(n) {
                            for u in self.data.reachable(t) {
                                inner.remove(u, None);
                            }
                        }
                    }
                    Expr::Col { table, col } => {
                        if let Some((t, c)) = self.col(table.as_deref(), col) {
                            inner.remove(t, Some(c));
                        }
                    }
                    _ => {}
                },
                e if e.is_table() => {
                    let Some(rows) = self.table(e, ctx) else { return Value::Error(ErrorKind::Ref) };
                    let t = rows.table();
                    match rows {
                        Rows::Table(_, rs) => {
                            let mut mask = vec![false; self.data.rows(t)];
                            for r in rs {
                                mask[r] = true;
                            }
                            adds.push(Filter { table: t, col: None, mask: Arc::new(mask) });
                        }
                        Rows::Column(_, c, rs) => {
                            let keys: std::collections::HashSet<_> = rs.iter().map(|r| key(self.data.value(t, c, *r))).collect();
                            let mask = (0..self.data.rows(t)).map(|r| keys.contains(&key(self.data.value(t, c, r)))).collect();
                            adds.push(Filter { table: t, col: Some(c), mask: Arc::new(mask) });
                        }
                    }
                }
                // A boolean filter: the rows of its column's table where
                // it holds, whatever the other filters.
                e => {
                    let mut cols = Vec::new();
                    e.columns(&mut cols);
                    let Some((t, c)) = cols.first().and_then(|(t, c)| self.col(Some(t), c)) else { return Value::Error(ErrorKind::Value) };
                    let mut memo: HashMap<Option<super::data::Key>, bool> = HashMap::default();
                    let mask = (0..self.data.rows(t))
                        .map(|r| {
                            let k = key(self.data.value(t, c, r));
                            *memo.entry(k).or_insert_with(|| truth(&self.eval(e, &ctx.with_row(t, r))).unwrap_or(false))
                        })
                        .collect();
                    adds.push(Filter { table: t, col: Some(c), mask: Arc::new(mask) });
                }
            }
        }
        for f in &adds {
            inner.remove(f.table, f.col);
        }
        inner.filters.extend(adds);
        self.eval(&a[0], &inner)
    }
}

/// Measures of a pivot table built on the data model: evaluates each in
/// the filter context of a cell (its row and column items, the report
/// filters and slicers).
pub struct PivotMeasures {
    pub data: Arc<ModelData>,
    compiled: Compiled,
    /// The measures the table shows (value columns from `MEASURE_BASE`).
    pub used: Vec<String>,
    /// The model column behind each source column: (table, column).
    pub cols: Vec<Option<(usize, usize)>>,
    pub lang: Lang,
    /// Item texts of each row of a table column under a grouping, keyed by
    /// source column and grouping.
    texts: RefCell<HashMap<String, Arc<Vec<String>>>>,
}

impl std::fmt::Debug for PivotMeasures {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("PivotMeasures").field("used", &self.used).finish()
    }
}

impl PivotMeasures {
    pub fn new(data: Arc<ModelData>, measures: &[Measure], used: Vec<String>, cols: Vec<Option<(usize, usize)>>, lang: Lang) -> PivotMeasures {
        let compiled = compile_all(&data, measures);
        PivotMeasures { data, compiled, used, cols, lang, texts: RefCell::new(HashMap::default()) }
    }

    fn filter(&self, f: &ItemFilter) -> Option<Filter> {
        let (t, c) = (*self.cols.get(f.col as usize)?)?;
        let id = format!("{}:{:?}", f.col, f.group);
        let texts = self.texts.borrow().get(&id).cloned();
        let texts = match texts {
            Some(x) => x,
            None => {
                let v: Vec<String> = self.data.tables[t].cols[c].iter().map(|v| pivot::item_text(v, f.group, self.lang).to_lowercase()).collect();
                let v = Arc::new(v);
                self.texts.borrow_mut().insert(id, v.clone());
                v
            }
        };
        let mask = texts.iter().map(|s| f.items.contains(s)).collect();
        Some(Filter { table: t, col: Some(c), mask: Arc::new(mask) })
    }
}

impl MeasureEval for PivotMeasures {
    fn eval(&self, k: usize, filters: &[ItemFilter]) -> Option<f64> {
        let name = self.used.get(k)?;
        let engine = Engine::new(&self.data, &self.compiled);
        let ctx = Context::new(filters.iter().filter_map(|f| self.filter(f)).collect());
        match engine.measure(name, &ctx) {
            Value::Empty => None,
            Value::Number(n) => Some(n),
            Value::Bool(b) => Some(b as u8 as f64),
            Value::Text(t) => t.trim().parse().ok(),
            _ => Some(f64::NAN),
        }
    }
}

/// A formula's problem as the measure dialog reports it: parse and name
/// errors, then cycles through other measures.
pub fn check_measure(name: &str, formula: &str, data: &ModelData, measures: &[Measure]) -> Result<Expr, DaxError> {
    let mut all: Vec<Measure> = measures.iter().filter(|m| !m.name.eq_ignore_ascii_case(name)).cloned().collect();
    all.push(Measure { name: name.to_string(), formula: formula.to_string(), ..Default::default() });
    let e = compile(formula, data, &all)?;
    if let Some(c) = find_cycle(name, &all, data) {
        return Err(DaxError { kind: DaxErrorKind::Circular, arg: c, pos: 0 });
    }
    Ok(e)
}
