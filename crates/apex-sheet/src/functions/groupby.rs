//! Spreadsheet functions: summaries (GROUPBY, PIVOTBY, PERCENTOF,
//! GETPIVOTDATA) and TRIMRANGE.

use std::collections::HashMap;

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    match f {
        GroupBy | PivotBy | PercentOf | TrimRange => Some(ALL),
        GetPivotData => Some(0b10),
        _ => None,
    }
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    let need = |min: usize, max: usize| if a.len() < min || a.len() > max { Err(ErrorKind::Value) } else { Ok(()) };
    Some(match f {
        GroupBy => need(3, 8).and_then(|_| group_by(env, a)),
        PivotBy => need(4, 11).and_then(|_| pivot_by(env, a)),
        PercentOf => need(2, 2).and_then(|_| {
            let part = env.fold_numbers(&a[..1], 0.0, |x, y| x + y)?;
            let all = env.fold_numbers(&a[1..], 0.0, |x, y| x + y)?;
            if all == 0.0 { Err(ErrorKind::Div0) } else { Ok(num_ok(part / all)) }
        }),
        TrimRange => need(1, 3).and_then(|_| match trim_ref(env, a) {
            Some(r) => r.map(|(s, r)| env.ref_value(s, &r)),
            None => trim_array(env, a),
        }),
        GetPivotData => (|| {
            if a.len() < 2 || a.len() % 2 != 0 {
                return Err(ErrorKind::Value);
            }
            let field = env.text(&a[0])?;
            let (s, r) = env.range_arg(&a[1]).map_err(|_| ErrorKind::Ref)?;
            let mut pairs = Vec::with_capacity(a.len() / 2 - 1);
            for p in a[2..].chunks(2) {
                let name = env.text(&p[0])?;
                match env.scalar(&p[1]) {
                    Value::Error(k) => return Err(k),
                    v => pairs.push((name.to_string(), v)),
                }
            }
            env.ctx.pivot_value(s, r.start, &field, &pairs).ok_or(ErrorKind::Ref)
        })(),
        _ => return None,
    })
}

// --- TRIMRANGE ---------------------------------------------------------------

/// Which blank rows and columns to cut: bit 1 leading, bit 2 trailing.
fn trim_modes(env: &Env, a: &[Expr]) -> R<(u32, u32)> {
    let m = |i| -> R<u32> {
        let x = env.opt_num(a, i, 3.0)?.trunc();
        if (0.0..=3.0).contains(&x) { Ok(x as u32) } else { Err(ErrorKind::Value) }
    };
    Ok((m(1)?, m(2)?))
}

/// TRIMRANGE over a reference: the reference without its blank edges.
/// `None` when the argument is not a reference.
pub(super) fn trim_ref(env: &Env, a: &[Expr]) -> Option<R<(usize, Range)>> {
    let (s, r) = match env.reference(a.first()?)? {
        Ok(x) => x,
        Err(k) => return Some(Err(k)),
    };
    Some((|| {
        let (tr, tc) = trim_modes(env, a)?;
        let mut used: Option<Range> = None;
        crate::functions::scan(env.ctx, s, &r, &mut |c, _| {
            used = Some(match used {
                None => Range::single(c),
                Some(u) => Range::new(CellRef::new(u.start.row.min(c.row), u.start.col.min(c.col)), CellRef::new(u.end.row.max(c.row), u.end.col.max(c.col))),
            })
        });
        let u = used.ok_or(ErrorKind::Ref)?;
        let pick = |mode: u32, bit: u32, cut: u32, keep: u32| if mode & bit != 0 { cut } else { keep };
        Ok((
            s,
            Range::new(
                CellRef::new(pick(tr, 1, u.start.row, r.start.row), pick(tc, 1, u.start.col, r.start.col)),
                CellRef::new(pick(tr, 2, u.end.row, r.end.row), pick(tc, 2, u.end.col, r.end.col)),
            ),
        ))
    })())
}

fn trim_array(env: &Env, a: &[Expr]) -> R<Value> {
    let m = env.tab(&a[0])?.to_matrix()?;
    let (tr, tc) = trim_modes(env, a)?;
    let row_used = |r: usize| (0..m.cols).any(|c| !m.get(r, c).is_empty());
    let col_used = |c: usize| (0..m.rows).any(|r| !m.get(r, c).is_empty());
    let (Some(r0), Some(r1)) = ((0..m.rows).find(|&r| row_used(r)), (0..m.rows).rfind(|&r| row_used(r))) else { return Err(ErrorKind::Ref) };
    let (c0, c1) = ((0..m.cols).find(|&c| col_used(c)).unwrap_or(0), (0..m.cols).rfind(|&c| col_used(c)).unwrap_or(0));
    let (r0, r1) = (if tr & 1 != 0 { r0 } else { 0 }, if tr & 2 != 0 { r1 } else { m.rows - 1 });
    let (c0, c1) = (if tc & 1 != 0 { c0 } else { 0 }, if tc & 2 != 0 { c1 } else { m.cols - 1 });
    let data = (r0..=r1).flat_map(|r| (c0..=c1).map(move |c| (r, c))).map(|(r, c)| m.get(r, c).clone()).collect();
    Ok(Matrix::new(r1 - r0 + 1, c1 - c0 + 1, data).into_value())
}

// --- GROUPBY and PIVOTBY -------------------------------------------------------

/// Items of a field compare case-insensitively.
fn item_key(v: &Value) -> String {
    match v {
        Value::Text(t) => format!("t{}", t.to_lowercase()),
        Value::Number(n) => format!("n{}", n.to_bits()),
        Value::Bool(b) => format!("b{b}"),
        Value::Error(k) => format!("e{}", k.as_str()),
        _ => String::new(),
    }
}

/// Sort order of items: blanks last.
fn item_cmp(a: &Value, b: &Value) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => a.compare(b),
    }
}

/// A group of records: the items of the fields so far, and its subgroups.
struct Node {
    key: Vec<Value>,
    recs: Vec<usize>,
    kids: Vec<Node>,
}

fn group(fields: &Matrix, recs: Vec<usize>, key: Vec<Value>) -> Node {
    let lvl = key.len();
    if lvl == fields.cols {
        return Node { key, recs, kids: Vec::new() };
    }
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut parts: Vec<(Value, Vec<usize>)> = Vec::new();
    for &r in &recs {
        let v = fields.get(r, lvl);
        let i = *index.entry(item_key(v)).or_insert_with(|| {
            parts.push((v.clone(), Vec::new()));
            parts.len() - 1
        });
        parts[i].1.push(r);
    }
    let kids = parts
        .into_iter()
        .map(|(v, rs)| {
            let mut k = key.clone();
            k.push(v);
            group(fields, rs, k)
        })
        .collect();
    Node { key, recs, kids }
}

/// A sort key: a field column or a value column, and whether descending.
#[derive(Clone, Copy)]
enum By {
    Field(usize),
    Value(usize),
}

/// Compares groups at `level` (None: whole rows of a flat table).
fn node_cmp(a: &Node, b: &Node, level: Option<usize>, by: &[(By, bool)], val: &dyn Fn(&Node, usize) -> Value) -> Ordering {
    for &(k, desc) in by {
        let o = match k {
            By::Field(j) if level.is_none_or(|l| l == j) => item_cmp(&a.key[j], &b.key[j]),
            By::Field(_) => continue,
            By::Value(v) => item_cmp(&val(a, v), &val(b, v)),
        };
        let o = if desc { o.reverse() } else { o };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

fn sort_tree(n: &mut Node, by: &[(By, bool)], val: &dyn Fn(&Node, usize) -> Value) {
    let level = n.key.len();
    n.kids.sort_by(|a, b| node_cmp(a, b, Some(level), by, val));
    for k in &mut n.kids {
        sort_tree(k, by, val);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Item,
    Subtotal,
    Grand,
}

/// A row (or column) of the output.
struct Line {
    key: Vec<Value>,
    recs: Vec<usize>,
    /// Records of the enclosing group.
    parent: Vec<usize>,
    kind: Kind,
}

/// The lines of one axis: `depth` as in GROUPBY's total_depth.
struct Axis {
    fields: Matrix,
    depth: i64,
    by: Vec<(By, bool)>,
    table: bool,
}

impl Axis {
    fn lines(&self, recs: &[usize], val: &dyn Fn(&Node, usize) -> Value) -> Vec<Line> {
        let mut root = group(&self.fields, recs.to_vec(), Vec::new());
        let top = self.depth < 0;
        let mut out = Vec::new();
        let grand = |out: &mut Vec<Line>| out.push(Line { key: Vec::new(), recs: recs.to_vec(), parent: recs.to_vec(), kind: Kind::Grand });
        if self.depth.abs() >= 1 && top {
            grand(&mut out);
        }
        if self.table {
            let mut leaves = Vec::new();
            flatten(root, &mut leaves);
            leaves.sort_by(|a, b| node_cmp(a, b, None, &self.by, val));
            out.extend(leaves.into_iter().map(|n| Line { key: n.key, recs: n.recs, parent: recs.to_vec(), kind: Kind::Item }));
        } else {
            sort_tree(&mut root, &self.by, val);
            let subs = (self.depth.unsigned_abs() as usize).min(self.fields.cols);
            for k in &root.kids {
                emit(k, &root.recs, self.fields.cols, subs, top, &mut out);
            }
        }
        if self.depth.abs() >= 1 && !top {
            grand(&mut out);
        }
        out
    }

    fn fields(&self) -> usize {
        self.fields.cols
    }
}

fn flatten(n: Node, out: &mut Vec<Node>) {
    if n.kids.is_empty() {
        out.push(n);
    } else {
        n.kids.into_iter().for_each(|k| flatten(k, out));
    }
}

/// Lines of a group: subtotals for levels below `subs`, above or below.
fn emit(n: &Node, parent: &[usize], nf: usize, subs: usize, top: bool, out: &mut Vec<Line>) {
    if n.key.len() == nf {
        out.push(Line { key: n.key.clone(), recs: n.recs.clone(), parent: parent.to_vec(), kind: Kind::Item });
        return;
    }
    let sub = n.key.len() < subs;
    let line = || Line { key: n.key.clone(), recs: n.recs.clone(), parent: parent.to_vec(), kind: Kind::Subtotal };
    if sub && top {
        out.push(line());
    }
    for k in &n.kids {
        emit(k, &n.recs, nf, subs, top, out);
    }
    if sub && !top {
        out.push(line());
    }
}

/// The label cells of a line across its axis' fields.
fn labels(l: &Line, nf: usize) -> Vec<Value> {
    (0..nf)
        .map(|i| match l.kind {
            Kind::Grand if i == 0 => Value::text("Total"),
            _ => l.key.get(i).cloned().unwrap_or(Value::Empty),
        })
        .collect()
}

/// The function applied to groups; two-parameter ones (PERCENTOF) also get
/// the values their group is measured against.
struct Agg<'e, 'a> {
    env: &'e Env<'a>,
    l: Arc<Lambda>,
    two: bool,
    values: Matrix,
}

impl Agg<'_, '_> {
    fn new<'e, 'a>(env: &'e Env<'a>, e: &Expr, values: Matrix) -> R<Agg<'e, 'a>> {
        let l = env.lambda_arg(e)?;
        let two = match eta_func(&l) {
            Some(f) => f == Func::PercentOf,
            None => l.params.len() >= 2,
        };
        Ok(Agg { env, l, two, values })
    }

    fn column(&self, recs: &[usize], v: usize) -> Value {
        let data: Vec<Value> = recs.iter().map(|&r| self.values.get(r, v).clone()).collect();
        Value::Array(Arc::new(Matrix::new(data.len(), 1, data)))
    }

    fn apply(&self, recs: &[usize], all: &[usize], v: usize) -> Value {
        let mut args = vec![self.column(recs, v)];
        if self.two {
            args.push(self.column(all, v));
        }
        match self.env.apply(&self.l, &args) {
            Value::Array(m) if m.data.len() == 1 => m.data[0].clone(),
            Value::Array(_) | Value::Lambda(_) => Value::Error(ErrorKind::Calc),
            v => v,
        }
    }
}

fn opt_int(env: &Env, a: &[Expr], i: usize) -> R<Option<i64>> {
    match a.get(i) {
        None | Some(Expr::Missing) => Ok(None),
        Some(e) => Ok(Some(env.num(e)?.trunc() as i64)),
    }
}

fn matrix_arg(env: &Env, e: &Expr) -> R<Matrix> {
    env.tab(e)?.to_matrix()
}

fn drop_first_row(m: &Matrix) -> (Vec<Value>, Matrix) {
    let head = m.data[..m.cols].to_vec();
    (head, Matrix::new(m.rows - 1, m.cols, m.data[m.cols..].to_vec()))
}

/// field_headers: whether the data has headers and whether to show them
/// (missing: text above a number means headers, shown with several levels).
fn headers(env: &Env, a: &[Expr], i: usize, values: &Matrix, levels: bool) -> R<(bool, bool)> {
    Ok(match opt_int(env, a, i)? {
        None => {
            let has = values.rows > 1 && matches!(values.get(0, 0), Value::Text(_)) && matches!(values.get(1, 0), Value::Number(_));
            (has, has && levels)
        }
        Some(0) => (false, false),
        Some(1) => (true, false),
        Some(2) => (false, true),
        Some(3) => (true, true),
        _ => return Err(ErrorKind::Value),
    })
}

/// sort_order: column numbers (fields, then values), negative descending.
fn sort_keys(env: &Env, a: &[Expr], i: usize, nf: usize, nv: usize) -> R<Vec<(By, bool)>> {
    let Some(e) = a.get(i).filter(|e| !matches!(e, Expr::Missing)) else { return Ok((0..nf).map(|j| (By::Field(j), false)).collect()) };
    let m = matrix_arg(env, e)?;
    m.data
        .iter()
        .map(|v| {
            let k = v.to_number()?.trunc() as i64;
            let j = k.unsigned_abs() as usize;
            let by = match j {
                0 => return Err(ErrorKind::Value),
                j if j <= nf => By::Field(j - 1),
                j if j <= nf + nv => By::Value(j - nf - 1),
                _ => return Err(ErrorKind::Value),
            };
            Ok((by, k < 0))
        })
        .collect()
}

/// The records filter_array keeps.
fn records(env: &Env, a: &[Expr], i: usize, n: usize, header: bool) -> R<Vec<usize>> {
    let Some(e) = a.get(i).filter(|e| !matches!(e, Expr::Missing)) else { return Ok((0..n).collect()) };
    let m = matrix_arg(env, e)?;
    let skip = if m.data.len() == n + 1 && header {
        1
    } else if m.data.len() == n {
        0
    } else {
        return Err(ErrorKind::Value);
    };
    Ok((0..n).filter(|&r| matches!(m.data[r + skip].to_bool(), Ok(true))).collect())
}

fn check_depth(d: Option<i64>, nf: usize) -> R<i64> {
    match d {
        // Grand totals and, where possible, subtotals.
        None => Ok(nf.max(1) as i64),
        Some(d) if d.abs() <= 64 => Ok(d),
        _ => Err(ErrorKind::Value),
    }
}

fn names(head: Option<Vec<Value>>, n: usize, generated: &str) -> Vec<Value> {
    head.unwrap_or_else(|| (1..=n).map(|i| Value::text(format!("{generated} {i}"))).collect())
}

fn group_by(env: &Env, a: &[Expr]) -> R<Value> {
    let (mut rf, mut vals) = (matrix_arg(env, &a[0])?, matrix_arg(env, &a[1])?);
    if rf.rows != vals.rows {
        return Err(ErrorKind::Value);
    }
    let (has, show) = headers(env, a, 3, &vals, rf.cols > 1)?;
    let (mut rhead, mut vhead) = (None, None);
    if has {
        if rf.rows < 2 {
            return Err(ErrorKind::Calc);
        }
        let (h, m) = drop_first_row(&rf);
        (rhead, rf) = (Some(h), m);
        let (h, m) = drop_first_row(&vals);
        (vhead, vals) = (Some(h), m);
    }
    let (nf, nv) = (rf.cols, vals.cols);
    let table = match opt_int(env, a, 7)? {
        None | Some(0) => false,
        Some(1) => true,
        _ => return Err(ErrorKind::Value),
    };
    let depth = check_depth(opt_int(env, a, 4)?, if table { 1 } else { nf })?;
    let by = sort_keys(env, a, 5, nf, nv)?;
    let recs = records(env, a, 6, rf.rows, has)?;
    if recs.is_empty() {
        return Err(ErrorKind::Calc);
    }
    let agg = Agg::new(env, &a[2], vals)?;
    let val = |n: &Node, v: usize| agg.apply(&n.recs, &recs, v);
    let axis = Axis { fields: rf, depth, by, table };
    let lines = axis.lines(&recs, &val);
    let width = nf + nv;
    let mut data = Vec::with_capacity((lines.len() + 1) * width);
    if show {
        data.extend(names(rhead, nf, "Row Field"));
        data.extend(names(vhead, nv, "Value"));
    }
    for l in &lines {
        data.extend(labels(l, axis.fields()));
        data.extend((0..nv).map(|v| agg.apply(&l.recs, &recs, v)));
    }
    Ok(Matrix::new(data.len() / width, width, data).into_value())
}

fn pivot_by(env: &Env, a: &[Expr]) -> R<Value> {
    let (mut rf, mut cf, mut vals) = (matrix_arg(env, &a[0])?, matrix_arg(env, &a[1])?, matrix_arg(env, &a[2])?);
    if rf.rows != vals.rows || cf.rows != vals.rows {
        return Err(ErrorKind::Value);
    }
    let (has, show) = headers(env, a, 4, &vals, rf.cols > 1 || cf.cols > 1)?;
    let (mut rhead, mut vhead) = (None, None);
    if has {
        if rf.rows < 2 {
            return Err(ErrorKind::Calc);
        }
        let (h, m) = drop_first_row(&rf);
        (rhead, rf) = (Some(h), m);
        cf = drop_first_row(&cf).1;
        let (h, m) = drop_first_row(&vals);
        (vhead, vals) = (Some(h), m);
    }
    let (nr, nc, nv) = (rf.cols, cf.cols, vals.cols);
    let rdepth = check_depth(opt_int(env, a, 5)?, nr)?;
    let rby = sort_keys(env, a, 6, nr, nv)?;
    let cdepth = check_depth(opt_int(env, a, 7)?, nc)?;
    let cby = sort_keys(env, a, 8, nc, nv)?;
    let recs = records(env, a, 9, rf.rows, has)?;
    let rel = opt_int(env, a, 10)?.unwrap_or(0);
    if !(0..=4).contains(&rel) {
        return Err(ErrorKind::Value);
    }
    if recs.is_empty() {
        return Err(ErrorKind::Calc);
    }
    let agg = Agg::new(env, &a[3], vals)?;
    let val = |n: &Node, v: usize| agg.apply(&n.recs, &recs, v);
    let rows = Axis { fields: rf, depth: rdepth, by: rby, table: false }.lines(&recs, &val);
    let cols = Axis { fields: cf, depth: cdepth, by: cby, table: false }.lines(&recs, &val);
    let vrow = nv > 1 || show;
    let width = nr + cols.len() * nv;
    let mut data = Vec::new();
    // Column items, one header row per column field.
    let clabels: Vec<Vec<Value>> = cols.iter().map(|l| labels(l, nc)).collect();
    for i in 0..nc {
        data.extend(std::iter::repeat_n(Value::Empty, nr));
        for c in &clabels {
            data.push(c[i].clone());
            data.extend(std::iter::repeat_n(Value::Empty, nv - 1));
        }
    }
    if vrow {
        if show {
            data.extend(names(rhead, nr, "Row Field"));
        } else {
            data.extend(std::iter::repeat_n(Value::Empty, nr));
        }
        let vn = names(vhead, nv, "Value");
        for _ in &cols {
            data.extend(vn.iter().cloned());
        }
    }
    let inter = |x: &[usize], y: &[usize]| -> Vec<usize> {
        let mut inx = vec![false; agg.values.rows];
        x.iter().for_each(|&r| inx[r] = true);
        y.iter().copied().filter(|&r| inx[r]).collect()
    };
    for rl in &rows {
        data.extend(labels(rl, nr));
        for cl in &cols {
            let sub = inter(&rl.recs, &cl.recs);
            let all = match rel {
                0 => cl.recs.clone(),
                1 => rl.recs.clone(),
                2 => recs.clone(),
                3 => inter(&rl.recs, &cl.parent),
                _ => inter(&rl.parent, &cl.recs),
            };
            for v in 0..nv {
                data.push(if sub.is_empty() { Value::Empty } else { agg.apply(&sub, &all, v) });
            }
        }
    }
    Ok(Matrix::new(data.len() / width, width, data).into_value())
}

#[cfg(test)]
mod tests {
    use crate::{CellRef, Locale, Value, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let rows: [&[&str]; 7] = [
            &["Region", "Product", "Sales", "Units"],
            &["West", "Pen", "10", "1"],
            &["East", "Ink", "5", "2"],
            &["West", "Ink", "7", "3"],
            &["east", "Pen", "2", "4"],
            &["North", "Pen", "6", "5"],
            &["West", "Pen", "4", "6"],
        ];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                w.set_input(0, CellRef::new(r as u32, c as u32), v);
            }
        }
        w
    }

    fn text(w: &Workbook, f: &str) -> String {
        let s = |v: &Value| v.to_text().map(|t| t.to_string()).unwrap_or_else(|e| e.as_str().into());
        match w.eval_formula(0, f) {
            Value::Array(m) => m.data.chunks(m.cols).map(|r| r.iter().map(s).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join(";"),
            v => s(&v),
        }
    }

    /// A number as the tests' text shows it.
    fn n(x: f64) -> String {
        Value::Number(x).to_text().unwrap().to_string()
    }

    #[test]
    fn group_by() {
        let w = wb();
        assert_eq!(text(&w, "=GROUPBY(A2:A7,C2:C7,SUM)"), "East,7;North,6;West,21;Total,34");
        // Headers found (text over numbers) but shown only with several levels.
        assert_eq!(text(&w, "=GROUPBY(A1:A7,C1:C7,SUM)"), "East,7;North,6;West,21;Total,34");
        assert_eq!(text(&w, "=GROUPBY(A1:A7,C1:C7,SUM,3,0)"), "Region,Sales;East,7;North,6;West,21");
        assert_eq!(text(&w, "=GROUPBY(A2:A7,C2:D7,AVERAGE,0,-1,-2)"), format!("Total,{},3.5;West,7,{};North,6,5;East,3.5,3", n(17.0 / 3.0), n(10.0 / 3.0)));
        assert_eq!(text(&w, "=GROUPBY(A2:A7,C2:C7,LAMBDA(x,MAX(x)),0,0,-1)"), "West,10;North,6;East,5");
        assert_eq!(text(&w, "=GROUPBY(A2:A7,C2:C7,COUNT,,0,,C2:C7>4)"), "East,1;North,1;West,2");
        assert_eq!(text(&w, "=GROUPBY(A2:A7,C2:C7,PERCENTOF,,0)"), format!("East,{};North,{};West,{}", n(7.0 / 34.0), n(6.0 / 34.0), n(21.0 / 34.0)));
        // Two fields: subtotals and headers by default.
        assert_eq!(
            text(&w, "=GROUPBY(A1:B7,C1:C7,SUM)"),
            "Region,Product,Sales;East,Ink,5;East,Pen,2;East,,7;North,Pen,6;North,,6;West,Ink,7;West,Pen,14;West,,21;Total,,34"
        );
        assert_eq!(text(&w, "=GROUPBY(A2:B7,C2:C7,SUM,0,1,{2,-3},,1)"), "West,Ink,7;East,Ink,5;West,Pen,14;North,Pen,6;East,Pen,2;Total,,34");
        assert_eq!(text(&w, "=GROUPBY(A2:A7,C2:C9,SUM)"), "#VALUE!");
    }

    #[test]
    fn pivot_by() {
        let w = wb();
        assert_eq!(text(&w, "=PIVOTBY(A2:A7,B2:B7,C2:C7,SUM)"), ",Ink,Pen,Total;East,5,2,7;North,,6,6;West,7,14,21;Total,12,22,34");
        assert_eq!(text(&w, "=PIVOTBY(A2:A7,B2:B7,C2:C7,SUM,0,0,,0)"), ",Ink,Pen;East,5,2;North,,6;West,7,14");
        assert_eq!(text(&w, "=PIVOTBY(A2:A7,B2:B7,C2:C7,PERCENTOF,0,0,,0,,,2)"), format!(",Ink,Pen;East,{},{};North,,{};West,{},{}", n(5.0 / 34.0), n(2.0 / 34.0), n(6.0 / 34.0), n(7.0 / 34.0), n(14.0 / 34.0)));
        assert_eq!(text(&w, "=PIVOTBY(A1:A7,B1:B7,C1:D7,SUM,3,0,,0)"), ",Ink,,Pen,;Region,Sales,Units,Sales,Units;East,5,2,2,4;North,,,6,5;West,7,3,14,7");
    }

    #[test]
    fn percent_of_and_trim_range() {
        let mut w = wb();
        assert_eq!(text(&w, "=PERCENTOF(C2:C3,C2:C7)"), n(15.0 / 34.0));
        assert_eq!(text(&w, "=PERCENTOF(1,0)"), "#DIV/0!");
        assert_eq!(text(&w, "=ROWS(TRIMRANGE(A1:F20))&\"x\"&COLUMNS(TRIMRANGE(A1:F20))"), "7x4");
        assert_eq!(text(&w, "=SUM(TRIMRANGE(C:C))"), "34");
        assert_eq!(text(&w, "=TRIMRANGE({\"\",1;2,3},1,0)"), ",1;2,3");
        w.set_input(0, CellRef::new(9, 7), "x");
        assert_eq!(text(&w, "=ROWS(TRIMRANGE(A1:H20,2))&\"x\"&COLUMNS(TRIMRANGE(B1:H20,3,1))"), "10x7");
        assert_eq!(text(&w, "=ROWS(TRIMRANGE(A3:H20,1))"), "18");
        assert_eq!(text(&w, "=TRIMRANGE(J1:J5)"), "#REF!");
        // Function names as values elsewhere.
        assert_eq!(text(&w, "=BYROW(C2:D3,SUM)"), "11;7");
        assert_eq!(text(&w, "=REDUCE(0,C2:C7,SUM)"), "34");
        w.locale = Locale::ES;
        assert_eq!(text(&w, "=AGRUPARPOR(A2:A7;C2:C7;SUMA;0;0)"), "East,7;North,6;West,21");
        let e = crate::formula::parse("=AGRUPARPOR(A2:A7;C2:C7;SUMA)+SUMA(MAP(A1:A2;LAMBDA(suma;suma)))", &Locale::ES).unwrap();
        assert_eq!(e.to_xlsx(), "_xlfn.GROUPBY(A2:A7,C2:C7,_xleta.SUM)+SUM(_xlfn.MAP(A1:A2,_xlfn.LAMBDA(_xlpm.suma,_xlpm.suma)))");
    }
}
