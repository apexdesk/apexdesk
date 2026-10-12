//! Spreadsheet functions: lookup and reference, dynamic arrays and lambdas.

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    Some(match f {
        Lookup => 0b110,
        XMatch => 0b10,
        Indirect | Address | Hyperlink | Switch | Sequence | RandArray => return None,
        Offset | ChooseCols | ChooseRows | Take | Drop | Filter | Sort | SortBy | Unique | VStack | HStack | Transpose | Expand | ToCol | ToRow | WrapRows | WrapCols
        | Areas | FormulaText | Map | Reduce | Scan | ByRow | ByCol | MakeArray | IsOmitted => ALL,
        _ => return None,
    })
}

fn matrix(rows: usize, cols: usize, data: Vec<Value>) -> R<Value> {
    if rows == 0 || cols == 0 {
        return Err(ErrorKind::Calc);
    }
    Ok(Matrix::new(rows, cols, data).into_value())
}

/// A size argument for building arrays.
fn size(x: f64) -> R<usize> {
    if x < 0.0 {
        return Err(ErrorKind::Value);
    }
    let n = x.trunc() as usize;
    if n == 0 {
        return Err(ErrorKind::Calc);
    }
    if n > MAX_ARRAY {
        return Err(ErrorKind::Num);
    }
    Ok(n)
}

fn transpose(m: &Matrix) -> Matrix {
    let mut data = Vec::with_capacity(m.data.len());
    for c in 0..m.cols {
        for r in 0..m.rows {
            data.push(m.get(r, c).clone());
        }
    }
    Matrix::new(m.cols, m.rows, data)
}

fn rows_of(m: &Matrix) -> impl Iterator<Item = &[Value]> {
    m.data.chunks(m.cols.max(1))
}

/// Picks rows (by 0-based index) of a matrix.
fn pick_rows(m: &Matrix, idx: &[usize]) -> Matrix {
    let data = idx.iter().flat_map(|&r| m.data[r * m.cols..(r + 1) * m.cols].iter().cloned()).collect();
    Matrix::new(idx.len(), m.cols, data)
}

/// 1-based, negative from the end; `None` when out of range.
fn index_from(i: f64, len: usize) -> Option<usize> {
    let i = i.trunc() as i64;
    match i {
        1.. if i as usize <= len => Some(i as usize - 1),
        ..=-1 if (-i) as usize <= len => Some((len as i64 + i) as usize),
        _ => None,
    }
}

/// Ordering for SORT and SORTBY: numbers, text, logical values, errors.
fn sort_cmp(a: &Value, b: &Value) -> Ordering {
    a.compare(b)
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    if !handles(f) {
        return None;
    }
    let n = a.len();
    let need = |min: usize, max: usize| if n < min || n > max { Err(ErrorKind::Value) } else { Ok(()) };
    let m = |e: &Expr| env.tab(e).and_then(|t| t.to_matrix());
    Some((|| -> R<Value> {
        Ok(match f {
            Sequence => {
                need(1, 4)?;
                let rows = size(env.num(&a[0])?)?;
                let cols = size(env.opt_num(a, 1, 1.0)?)?;
                let (start, step) = (env.opt_num(a, 2, 1.0)?, env.opt_num(a, 3, 1.0)?);
                if rows * cols > MAX_ARRAY {
                    return Err(ErrorKind::Num);
                }
                return matrix(rows, cols, (0..rows * cols).map(|i| Value::Number(start + step * i as f64)).collect());
            }
            RandArray => {
                need(0, 5)?;
                let rows = size(env.opt_num(a, 0, 1.0)?)?;
                let cols = size(env.opt_num(a, 1, 1.0)?)?;
                let (lo, hi) = (env.opt_num(a, 2, 0.0)?, env.opt_num(a, 3, 1.0)?);
                let int = env.opt_bool(a, 4, false)?;
                if hi < lo || rows * cols > MAX_ARRAY {
                    return Err(if hi < lo { ErrorKind::Value } else { ErrorKind::Num });
                }
                let data = (0..rows * cols)
                    .map(|_| {
                        Value::Number(if int {
                            let (lo, hi) = (lo.ceil(), hi.floor());
                            (lo + (rand01() * (hi - lo + 1.0)).floor()).min(hi)
                        } else {
                            lo + rand01() * (hi - lo)
                        })
                    })
                    .collect();
                return matrix(rows, cols, data);
            }
            Offset | Indirect => {
                let (s, r) = env.ref_call(f, a).ok_or(ErrorKind::Value)??;
                env.ref_value(s, &r)
            }
            Lookup => {
                need(2, 3)?;
                let key = env.scalar(&a[0]);
                let look = env.tab(&a[1])?;
                // Array form: search the longer side's first line, return the last.
                let (len, get_key, get_res): (usize, Box<dyn Fn(usize) -> Value>, Box<dyn Fn(usize) -> Value>) = if n == 3 {
                    let res = env.tab(&a[2])?;
                    if res.rows() != 1 && res.cols() != 1 {
                        return Err(ErrorKind::Na);
                    }
                    let look_len = look.len();
                    (look_len, Box::new(move |i| look.nth(i)), Box::new(move |i| if i < res.len() { res.nth(i) } else { Value::Error(ErrorKind::Na) }))
                } else if look.cols() > look.rows() {
                    let (w, h) = (look.cols(), look.rows());
                    let t = Arc::new(look.to_matrix()?);
                    let t2 = t.clone();
                    (w, Box::new(move |i| t.get(0, i).clone()), Box::new(move |i| t2.get(h - 1, i).clone()))
                } else {
                    let (h, w) = (look.rows(), look.cols());
                    let t = Arc::new(look.to_matrix()?);
                    let t2 = t.clone();
                    (h, Box::new(move |i| t.get(i, 0).clone()), Box::new(move |i| t2.get(i, w - 1).clone()))
                };
                let pos = lookup_pos(len as u32, |i| get_key(i as usize), &key, 1, &env.locale)?;
                // An empty cell stays empty for what uses it (as VLOOKUP).
                get_res(pos as usize)
            }
            XMatch => {
                need(2, 4)?;
                let key = env.scalar(&a[0]);
                let t = env.tab(&a[1])?;
                let mode = env.opt_num(a, 2, 0.0)? as i32;
                let search = env.opt_num(a, 3, 1.0)? as i32;
                if t.rows() != 1 && t.cols() != 1 {
                    return Err(ErrorKind::Value);
                }
                if !(-1..=2).contains(&mode) || ![-2, -1, 1, 2].contains(&search) {
                    return Err(ErrorKind::Value);
                }
                match xmatch(t.len(), |i| t.nth(i), &key, mode, search, &env.locale) {
                    Some(i) => Value::Number(i as f64 + 1.0),
                    None => return Err(ErrorKind::Na),
                }
            }
            Address => {
                need(2, 5)?;
                let row = env.num(&a[0])?.trunc();
                let col = env.num(&a[1])?.trunc();
                let abs = env.opt_num(a, 2, 1.0)?.trunc() as i32;
                let a1 = env.opt_bool(a, 3, true)?;
                if row < 1.0 || col < 1.0 || row > MAX_ROWS as f64 || col > MAX_COLS as f64 || !(1..=4).contains(&abs) {
                    return Err(ErrorKind::Value);
                }
                let (row_abs, col_abs) = (abs == 1 || abs == 2, abs == 1 || abs == 3);
                let mut out = String::new();
                if let Some(e) = a.get(4).filter(|e| !matches!(e, Expr::Missing)) {
                    let sheet = env.text(e)?;
                    if !sheet.is_empty() {
                        if sheet.chars().all(|c| c.is_alphanumeric() || c == '_') {
                            out.push_str(&sheet);
                        } else {
                            out.push_str(&format!("'{}'", sheet.replace('\'', "''")));
                        }
                        out.push('!');
                    }
                }
                if a1 {
                    if col_abs {
                        out.push('$');
                    }
                    out.push_str(&crate::addr::col_name(col as u32 - 1));
                    if row_abs {
                        out.push('$');
                    }
                    out.push_str(&format!("{}", row as u32));
                } else {
                    let part = |abs: bool, v: f64| if abs { format!("{}", v as u32) } else { format!("[{}]", v as u32) };
                    out.push_str(&format!("R{}C{}", part(row_abs, row), part(col_abs, col)));
                }
                Value::text(out)
            }
            ChooseCols | ChooseRows => {
                if n < 2 {
                    return Err(ErrorKind::Value);
                }
                let src = m(&a[0])?;
                let src = if f == ChooseCols { transpose(&src) } else { src };
                let mut idx = Vec::new();
                for e in &a[1..] {
                    let mut err = None;
                    env.each(e, &mut |v, _| match v.to_number() {
                        Ok(x) => match index_from(x, src.rows) {
                            Some(i) => idx.push(i),
                            None => err = Some(ErrorKind::Value),
                        },
                        Err(k) => err = Some(k),
                    });
                    if let Some(k) = err {
                        return Err(k);
                    }
                }
                let out = pick_rows(&src, &idx);
                let out = if f == ChooseCols { transpose(&out) } else { out };
                return matrix(out.rows, out.cols, out.data);
            }
            Take | Drop => {
                need(2, 3)?;
                let src = m(&a[0])?;
                let pick = |len: usize, e: Option<&Expr>| -> R<std::ops::Range<usize>> {
                    let k = match e {
                        None | Some(Expr::Missing) => return Ok(0..len),
                        Some(e) => env.num(e)?.trunc(),
                    };
                    let k = k.clamp(-(len as f64), len as f64) as i64;
                    let len = len as i64;
                    let (lo, hi) = match (f, k >= 0) {
                        (Take, true) => (0, k),
                        (Take, false) => (len + k, len),
                        (_, true) => (k, len),
                        (_, false) => (0, len + k),
                    };
                    Ok(lo as usize..hi as usize)
                };
                let rows = pick(src.rows, a.get(1))?;
                let cols = pick(src.cols, a.get(2))?;
                let data = rows.clone().flat_map(|r| cols.clone().map(move |c| (r, c))).map(|(r, c)| src.get(r, c).clone()).collect();
                return matrix(rows.len(), cols.len(), data);
            }
            Expand => {
                need(2, 4)?;
                let src = m(&a[0])?;
                let rows = match a.get(1) {
                    Some(Expr::Missing) | None => src.rows,
                    Some(e) => env.num(e)?.trunc().max(0.0) as usize,
                };
                let cols = match a.get(2) {
                    Some(Expr::Missing) | None => src.cols,
                    Some(e) => env.num(e)?.trunc().max(0.0) as usize,
                };
                if rows < src.rows || cols < src.cols {
                    return Err(ErrorKind::Value);
                }
                if rows.checked_mul(cols).is_none_or(|n| n > MAX_ARRAY) {
                    return Err(ErrorKind::Num);
                }
                let pad = match a.get(3) {
                    Some(Expr::Missing) | None => Value::Error(ErrorKind::Na),
                    Some(e) => env.scalar(e),
                };
                let data = (0..rows).flat_map(|r| (0..cols).map(move |c| (r, c))).map(|(r, c)| if r < src.rows && c < src.cols { src.get(r, c).clone() } else { pad.clone() }).collect();
                return matrix(rows, cols, data);
            }
            ToCol | ToRow => {
                need(1, 3)?;
                let src = m(&a[0])?;
                let ignore = env.opt_num(a, 1, 0.0)? as i32;
                if !(0..=3).contains(&ignore) {
                    return Err(ErrorKind::Value);
                }
                let by_col = env.opt_bool(a, 2, false)?;
                let src = if by_col { transpose(&src) } else { src };
                let data: Vec<Value> = src
                    .data
                    .into_iter()
                    .filter(|v| match v {
                        Value::Empty => ignore & 1 == 0,
                        Value::Error(_) => ignore & 2 == 0,
                        _ => true,
                    })
                    .collect();
                let len = data.len();
                return if f == ToCol { matrix(len, 1, data) } else { matrix(1, len, data) };
            }
            WrapRows | WrapCols => {
                need(2, 3)?;
                let src = m(&a[0])?;
                if src.rows != 1 && src.cols != 1 {
                    return Err(ErrorKind::Value);
                }
                let w = env.num(&a[1])?.trunc();
                if w < 1.0 {
                    return Err(ErrorKind::Num);
                }
                let w = (w as usize).min(src.data.len().max(1));
                let pad = match a.get(2) {
                    Some(Expr::Missing) | None => Value::Error(ErrorKind::Na),
                    Some(e) => env.scalar(e),
                };
                let h = src.data.len().div_ceil(w);
                let mut data = src.data;
                data.resize(h * w, pad);
                let out = Matrix::new(h, w, data);
                let out = if f == WrapCols { transpose(&out) } else { out };
                return matrix(out.rows, out.cols, out.data);
            }
            Transpose => {
                need(1, 1)?;
                let t = transpose(&m(&a[0])?);
                return matrix(t.rows, t.cols, t.data);
            }
            VStack | HStack => {
                if n == 0 {
                    return Err(ErrorKind::Value);
                }
                let parts = a.iter().map(|e| m(e).map(|x| if f == HStack { transpose(&x) } else { x })).collect::<R<Vec<_>>>()?;
                let cols = parts.iter().map(|p| p.cols).max().unwrap_or(0);
                // Short rows are padded out to the widest: the size first.
                if parts.iter().map(|p| p.rows).sum::<usize>().saturating_mul(cols) > MAX_ARRAY {
                    return Err(ErrorKind::Num);
                }
                let mut data = Vec::new();
                let mut rows = 0;
                for p in &parts {
                    for row in rows_of(p) {
                        data.extend(row.iter().cloned());
                        data.extend(std::iter::repeat_n(Value::Error(ErrorKind::Na), cols - row.len()));
                        rows += 1;
                    }
                }
                if rows * cols > MAX_ARRAY {
                    return Err(ErrorKind::Num);
                }
                let out = Matrix::new(rows, cols, data);
                let out = if f == HStack { transpose(&out) } else { out };
                return matrix(out.rows, out.cols, out.data);
            }
            Filter => {
                need(2, 3)?;
                let src = m(&a[0])?;
                let inc = m(&a[1])?;
                let by_rows = inc.cols == 1 && inc.rows == src.rows;
                let by_cols = inc.rows == 1 && inc.cols == src.cols;
                if !by_rows && !by_cols {
                    return Err(ErrorKind::Value);
                }
                let mut keep = Vec::new();
                for (i, v) in inc.data.iter().enumerate() {
                    let yes = match v {
                        Value::Error(k) => return Err(*k),
                        Value::Empty => false,
                        Value::Text(_) => return Err(ErrorKind::Value),
                        v => v.to_bool()?,
                    };
                    if yes {
                        keep.push(i);
                    }
                }
                if keep.is_empty() {
                    return match a.get(2) {
                        Some(Expr::Missing) | None => Err(ErrorKind::Calc),
                        Some(e) => Ok(env.eval(e)),
                    };
                }
                let out = if by_rows { pick_rows(&src, &keep) } else { transpose(&pick_rows(&transpose(&src), &keep)) };
                return matrix(out.rows, out.cols, out.data);
            }
            Sort => {
                need(1, 4)?;
                let src = m(&a[0])?;
                let by_col = env.opt_bool(a, 3, false)?;
                let src = if by_col { transpose(&src) } else { src };
                let list = |i: usize, d: f64| -> R<Vec<f64>> {
                    match a.get(i) {
                        None | Some(Expr::Missing) => Ok(vec![d]),
                        Some(e) => {
                            let mut v = Vec::new();
                            let mut err = None;
                            env.each(e, &mut |x, _| match x.to_number() {
                                Ok(x) => v.push(x),
                                Err(k) => err = Some(k),
                            });
                            err.map_or(Ok(v), Err)
                        }
                    }
                };
                let idx = list(1, 1.0)?;
                let ord = list(2, 1.0)?;
                let mut keys = Vec::new();
                for (i, k) in idx.iter().enumerate() {
                    let k = k.trunc() as usize;
                    let o = ord.get(i).or(ord.first()).copied().unwrap_or(1.0);
                    if k < 1 || k > src.cols || (o != 1.0 && o != -1.0) {
                        return Err(ErrorKind::Value);
                    }
                    keys.push((k - 1, o < 0.0));
                }
                let mut order: Vec<usize> = (0..src.rows).collect();
                order.sort_by(|&x, &y| {
                    for &(k, desc) in &keys {
                        let o = sort_cmp(src.get(x, k), src.get(y, k));
                        let o = if desc { o.reverse() } else { o };
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    Ordering::Equal
                });
                let out = pick_rows(&src, &order);
                let out = if by_col { transpose(&out) } else { out };
                return matrix(out.rows, out.cols, out.data);
            }
            SortBy => {
                if n < 2 {
                    return Err(ErrorKind::Value);
                }
                let src = m(&a[0])?;
                let mut keys: Vec<(Matrix, bool)> = Vec::new();
                let mut by_col = None;
                let mut i = 1;
                while i < n {
                    let k = m(&a[i])?;
                    let desc = match a.get(i + 1) {
                        None | Some(Expr::Missing) => false,
                        Some(e) => match env.num(e)? {
                            1.0 => false,
                            -1.0 => true,
                            _ => return Err(ErrorKind::Value),
                        },
                    };
                    let cols_key = k.rows == 1 && k.cols == src.cols && !(k.cols == 1 && src.rows == 1 && src.cols == 1);
                    let rows_key = k.cols == 1 && k.rows == src.rows;
                    let this = if rows_key { false } else if cols_key { true } else { return Err(ErrorKind::Value) };
                    if by_col.is_some_and(|b| b != this) {
                        return Err(ErrorKind::Value);
                    }
                    by_col = Some(this);
                    keys.push((k, desc));
                    i += 2;
                }
                let by_col = by_col.unwrap_or(false);
                let src = if by_col { transpose(&src) } else { src };
                let mut order: Vec<usize> = (0..src.rows).collect();
                order.sort_by(|&x, &y| {
                    for (k, desc) in &keys {
                        let o = sort_cmp(&k.data[x], &k.data[y]);
                        let o = if *desc { o.reverse() } else { o };
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    Ordering::Equal
                });
                let out = pick_rows(&src, &order);
                let out = if by_col { transpose(&out) } else { out };
                return matrix(out.rows, out.cols, out.data);
            }
            Unique => {
                need(1, 3)?;
                let src = m(&a[0])?;
                let by_col = env.opt_bool(a, 1, false)?;
                let once = env.opt_bool(a, 2, false)?;
                let src = if by_col { transpose(&src) } else { src };
                let same = |x: &[Value], y: &[Value]| x.iter().zip(y).all(|(p, q)| std::mem::discriminant(p) == std::mem::discriminant(q) && p.compare(q) == Ordering::Equal);
                // First occurrence of each distinct row, with its count.
                let mut firsts: Vec<(usize, usize)> = Vec::new();
                let mut index: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
                for (r, row) in rows_of(&src).enumerate() {
                    let key: String = row.iter().map(|v| format!("{:?}", v).to_lowercase()).collect::<Vec<_>>().join("\u{1}");
                    let bucket = index.entry(key).or_default();
                    match bucket.iter().find(|&&i| same(&src.data[firsts[i].0 * src.cols..(firsts[i].0 + 1) * src.cols], row)) {
                        Some(&i) => firsts[i].1 += 1,
                        None => {
                            bucket.push(firsts.len());
                            firsts.push((r, 1));
                        }
                    }
                }
                let keep: Vec<usize> = firsts.iter().filter(|(_, c)| !once || *c == 1).map(|(r, _)| *r).collect();
                if keep.is_empty() {
                    return Err(ErrorKind::Calc);
                }
                let out = pick_rows(&src, &keep);
                let out = if by_col { transpose(&out) } else { out };
                return matrix(out.rows, out.cols, out.data);
            }
            Areas => {
                need(1, 1)?;
                env.range_arg(&a[0])?;
                Value::Number(1.0)
            }
            FormulaText => {
                need(1, 1)?;
                let (s, r) = env.range_arg(&a[0])?;
                Value::text(env.ctx.formula_text(s, r.start).ok_or(ErrorKind::Na)?)
            }
            Hyperlink => {
                need(1, 2)?;
                match a.get(1).filter(|e| !matches!(e, Expr::Missing)) {
                    Some(e) => env.scalar(e),
                    None => Value::Text(env.text(&a[0])?),
                }
            }
            Switch => {
                if n < 3 {
                    return Err(ErrorKind::Value);
                }
                let v = env.scalar(&a[0]);
                if let Value::Error(k) = v {
                    return Err(k);
                }
                let pairs = &a[1..];
                for p in pairs.chunks(2) {
                    if p.len() == 2 {
                        let w = env.scalar(&p[0]);
                        if std::mem::discriminant(&w) == std::mem::discriminant(&v) && w.compare(&v) == Ordering::Equal {
                            return Ok(env.eval(&p[1]));
                        }
                    } else {
                        return Ok(env.eval(&p[0]));
                    }
                }
                return Err(ErrorKind::Na);
            }
            IsOmitted => {
                need(1, 1)?;
                match &a[0] {
                    Expr::Name(x) => Value::Bool(matches!(env.lookup_name(x), Some(Bound::Omitted))),
                    _ => Value::Bool(false),
                }
            }
            Map => {
                if n < 2 {
                    return Err(ErrorKind::Value);
                }
                let l = env.lambda_arg(&a[n - 1])?;
                let arrays = a[..n - 1].iter().map(&m).collect::<R<Vec<_>>>()?;
                let rows = arrays.iter().map(|x| x.rows).max().unwrap_or(1);
                let cols = arrays.iter().map(|x| x.cols).max().unwrap_or(1);
                if rows.saturating_mul(cols) > MAX_ARRAY {
                    return Err(ErrorKind::Num);
                }
                let mut data = Vec::with_capacity(rows * cols);
                let mut text = 0;
                for r in 0..rows {
                    for c in 0..cols {
                        let args: Vec<Value> = arrays.iter().map(|x| x.at(r, c)).collect();
                        let v = single(env.apply(&l, &args));
                        if !text_fits(&v, &mut text) {
                            return Err(ErrorKind::Num);
                        }
                        data.push(v);
                    }
                }
                return matrix(rows, cols, data);
            }
            Reduce | Scan => {
                need(2, 3)?;
                let (init, arr, lam) = if n == 3 { (env.scalar(&a[0]), &a[1], &a[2]) } else { (Value::Empty, &a[0], &a[1]) };
                let l = env.lambda_arg(lam)?;
                let src = m(arr)?;
                let mut acc = init;
                let mut out = Vec::with_capacity(src.data.len());
                let mut text = 0;
                for v in &src.data {
                    acc = env.apply(&l, &[acc, v.clone()]);
                    if f == Scan {
                        let v = single(acc.clone());
                        if !text_fits(&v, &mut text) {
                            return Err(ErrorKind::Num);
                        }
                        out.push(v);
                    }
                }
                return if f == Scan { matrix(src.rows, src.cols, out) } else { Ok(acc) };
            }
            ByRow | ByCol => {
                need(2, 2)?;
                let src = m(&a[0])?;
                let l = env.lambda_arg(&a[1])?;
                let src = if f == ByCol { transpose(&src) } else { src };
                let mut text = 0;
                let mut data = Vec::new();
                for row in rows_of(&src) {
                    let v = single(env.apply(&l, &[Matrix::new(1, row.len(), row.to_vec()).into_value()]));
                    if !text_fits(&v, &mut text) {
                        return Err(ErrorKind::Num);
                    }
                    data.push(v);
                }
                let len = data.len();
                return if f == ByRow { matrix(len, 1, data) } else { matrix(1, len, data) };
            }
            MakeArray => {
                need(3, 3)?;
                let rows = size(env.num(&a[0])?)?;
                let cols = size(env.num(&a[1])?)?;
                if rows * cols > MAX_ARRAY {
                    return Err(ErrorKind::Num);
                }
                let l = env.lambda_arg(&a[2])?;
                let mut data = Vec::with_capacity(rows * cols);
                let mut text = 0;
                for r in 0..rows {
                    for c in 0..cols {
                        let v = single(env.apply(&l, &[Value::Number(r as f64 + 1.0), Value::Number(c as f64 + 1.0)]));
                        if !text_fits(&v, &mut text) {
                            return Err(ErrorKind::Num);
                        }
                        data.push(v);
                    }
                }
                return matrix(rows, cols, data);
            }
            _ => unreachable!("handles() lists the functions above"),
        })
    })())
}

/// Whether this module implements `f`.
fn handles(f: Func) -> bool {
    use Func::*;
    matches!(
        f,
        Sequence | RandArray | Offset | Indirect | Lookup | XMatch | Address | ChooseCols | ChooseRows | Take | Drop | Expand | ToCol | ToRow | WrapRows | WrapCols | Transpose | VStack | HStack | Filter | Sort | SortBy | Unique | Areas | FormulaText | Hyperlink | Switch | IsOmitted | Map | Reduce | Scan | ByRow | ByCol | MakeArray
    )
}

/// A lambda result stored in an array: nested arrays can't be.
fn single(v: Value) -> Value {
    match v {
        Value::Array(_) | Value::Lambda(_) => Value::Error(ErrorKind::Calc),
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use crate::{CellRef, Workbook};
    use crate::value::Value;

    fn wb() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = crate::format::Locale::EN;
        for (i, (name, n)) in [("b", 3), ("a", 1), ("c", 2), ("a", 5)].iter().enumerate() {
            wb.set_input(0, CellRef::new(i as u32, 0), name);
            wb.set_input(0, CellRef::new(i as u32, 1), &n.to_string());
        }
        wb
    }

    fn text(wb: &Workbook, f: &str) -> String {
        match wb.eval_formula(0, f) {
            Value::Array(m) => m.data.iter().map(|v| v.to_text().map(|t| t.to_string()).unwrap_or_else(|e| e.as_str().into())).collect::<Vec<_>>().join(","),
            v => v.to_text().map(|t| t.to_string()).unwrap_or_else(|e| e.as_str().into()),
        }
    }

    #[test]
    fn dynamic_arrays() {
        let wb = wb();
        assert_eq!(text(&wb, "=SEQUENCE(2,3)"), "1,2,3,4,5,6");
        assert_eq!(text(&wb, "=SORT(A1:A4)"), "a,a,b,c");
        assert_eq!(text(&wb, "=SORT(A1:B4,2,-1)"), "a,5,b,3,c,2,a,1");
        assert_eq!(text(&wb, "=SORTBY(A1:A4,B1:B4)"), "a,c,b,a");
        assert_eq!(text(&wb, "=UNIQUE(A1:A4)"), "b,a,c");
        assert_eq!(text(&wb, "=UNIQUE(A1:A4,,TRUE)"), "b,c");
        assert_eq!(text(&wb, "=FILTER(A1:A4,B1:B4>2)"), "b,a");
        assert_eq!(text(&wb, "=FILTER(A1:A4,B1:B4>9)"), "#CALC!");
        assert_eq!(text(&wb, "=FILTER(A1:A4,B1:B4>9,\"none\")"), "none");
        assert_eq!(text(&wb, "=TRANSPOSE({1,2;3,4})"), "1,3,2,4");
        assert_eq!(text(&wb, "=VSTACK({1,2},{3})"), "1,2,3,#N/A");
        assert_eq!(text(&wb, "=HSTACK({1;2},{3;4})"), "1,3,2,4");
        assert_eq!(text(&wb, "=TAKE(B1:B4,2)"), "3,1");
        assert_eq!(text(&wb, "=TAKE(B1:B4,-1)"), "5");
        assert_eq!(text(&wb, "=DROP(B1:B4,3)"), "5");
        assert_eq!(text(&wb, "=CHOOSECOLS(A1:B4,-1)"), "3,1,2,5");
        assert_eq!(text(&wb, "=CHOOSEROWS(A1:B4,{1,4})"), "b,3,a,5");
        assert_eq!(text(&wb, "=EXPAND({1,2},2,3,0)"), "1,2,0,0,0,0");
        // Sizes whose product wraps, and ranges past the largest array.
        assert_eq!(text(&wb, "=EXPAND(A1,4294967296,4294967296)"), "#NUM!");
        assert_eq!(text(&wb, "=ROWS(SEQUENCE(65536)+SEQUENCE(1,65536))"), "#NUM!");
        assert_eq!(text(&wb, "=LEN(TEXTJOIN(\"\",0,EXPAND(\"\",2000,2000,REPT(\"a\",32767))))"), "#VALUE!");
        // A whole-sheet OFFSET is cut to the used area (two columns).
        assert_eq!(text(&wb, "=ROWS(TRANSPOSE(OFFSET(A1,0,0,1048576,16384)))"), "2");
        assert_eq!(text(&wb, "=TOCOL({1,2;3,4},0,TRUE)"), "1,3,2,4");
        assert_eq!(text(&wb, "=WRAPROWS({1,2,3,4,5},2,0)"), "1,2,3,4,5,0");
        assert_eq!(text(&wb, "=WRAPCOLS({1,2,3},2)"), "1,3,2,#N/A");
        assert_eq!(text(&wb, "=SUM(SEQUENCE(10))"), "55");
        assert_eq!(text(&wb, "=ROWS(RANDARRAY(4,2))"), "4");
    }

    #[test]
    fn lookups_and_references() {
        let wb = wb();
        assert_eq!(text(&wb, "=LOOKUP(4,{1,3,5},{\"x\",\"y\",\"z\"})"), "y");
        assert_eq!(text(&wb, "=LOOKUP(4,{1,2;3,4;5,6})"), "4");
        assert_eq!(text(&wb, "=XMATCH(\"c\",A1:A4)"), "3");
        assert_eq!(text(&wb, "=XMATCH(4,B1:B4,1)"), "4");
        assert_eq!(text(&wb, "=XMATCH(\"a\",A1:A4,0,-1)"), "4");
        assert_eq!(text(&wb, "=XLOOKUP(\"c\",A1:A4,B1:B4)"), "2");
        assert_eq!(text(&wb, "=XLOOKUP(\"z\",A1:A4,B1:B4,\"no\")"), "no");
        assert_eq!(text(&wb, "=ADDRESS(2,3)"), "$C$2");
        assert_eq!(text(&wb, "=ADDRESS(2,3,4)"), "C2");
        assert_eq!(text(&wb, "=ADDRESS(2,3,1,FALSE,\"My Sheet\")"), "'My Sheet'!R2C3");
        assert_eq!(text(&wb, "=SUM(OFFSET(A1,1,1,2,1))"), "3");
        assert_eq!(text(&wb, "=INDIRECT(\"B\"&4)"), "5");
        assert_eq!(text(&wb, "=SUM(INDIRECT(\"R1C2:R2C2\",FALSE))"), "4");
        assert_eq!(text(&wb, "=SUM(INDEX(A1:B4,0,2))"), "11");
        assert_eq!(text(&wb, "=INDEX(A1:B4,{1,2},2)"), "3,1");
        assert_eq!(text(&wb, "=SWITCH(2,1,\"one\",2,\"two\",\"other\")"), "two");
        assert_eq!(text(&wb, "=SWITCH(9,1,\"one\",\"other\")"), "other");
        assert_eq!(text(&wb, "=AREAS(A1:B2)"), "1");
    }

    #[test]
    fn lambdas() {
        let wb = wb();
        assert_eq!(text(&wb, "=LET(x,2,y,x*3,x+y)"), "8");
        assert_eq!(text(&wb, "=LET(r,B1:B4,SUMIF(r,\">2\"))"), "8");
        assert_eq!(text(&wb, "=MAP(B1:B4,LAMBDA(v,v*2))"), "6,2,4,10");
        assert_eq!(text(&wb, "=REDUCE(0,B1:B4,LAMBDA(a,v,a+v))"), "11");
        assert_eq!(text(&wb, "=SCAN(,{1,2,3},LAMBDA(a,v,a+v))"), "1,3,6");
        assert_eq!(text(&wb, "=BYROW({1,2;3,4},LAMBDA(r,SUM(r)))"), "3,7");
        assert_eq!(text(&wb, "=BYCOL({1,2;3,4},LAMBDA(c,MAX(c)))"), "3,4");
        assert_eq!(text(&wb, "=MAKEARRAY(2,2,LAMBDA(r,c,r*10+c))"), "11,12,21,22");
        assert_eq!(text(&wb, "=LET(f,LAMBDA(x,y,IF(ISOMITTED(y),x,x+y)),f(1)+f(1,2))"), "4");
        assert_eq!(text(&wb, "=LET(k,10,f,LAMBDA(x,x+k),f(1))"), "11");
        assert_eq!(text(&wb, "=MAP({1,2},LAMBDA(x,{1,2}))"), "#CALC!,#CALC!");
    }

    #[test]
    fn broadcasting() {
        let wb = wb();
        assert_eq!(text(&wb, "=B1:B4*2"), "6,2,4,10");
        assert_eq!(text(&wb, "={1,2}+{10;20}"), "11,12,21,22");
        assert_eq!(text(&wb, "=LEN(A1:A2&\"xy\")"), "3,3");
        assert_eq!(text(&wb, "=IF(B1:B4>2,\"hi\",\"lo\")"), "hi,lo,lo,hi");
        assert_eq!(text(&wb, "=SUM(IF(B1:B4>2,B1:B4))"), "8");
        assert_eq!(text(&wb, "=IFERROR(1/(B1:B2-1),\"x\")"), "0.5,x");
        assert_eq!(text(&wb, "=VLOOKUP({\"a\",\"c\"},A1:B4,2,FALSE)"), "1,2");
        assert_eq!(text(&wb, "=ROUND({1.26,2.5},1)"), "1.3,2.5");
        assert_eq!(text(&wb, "=@B1:B4"), "3");
        assert_eq!(text(&wb, "=SUMPRODUCT((A1:A4=\"a\")*B1:B4)"), "6");
    }
}
