//! Spreadsheet functions of Google Sheets that Excel doesn't have:
//! ARRAYFORMULA, SPLIT, JOIN, FLATTEN, COUNTUNIQUE, ARRAY_CONSTRAIN,
//! SORTN, the operator functions (ADD, EQ...), ISEMAIL, ISURL and the
//! TO_ conversions. QUERY is in `gquery`.

use super::*;
use crate::formula::{BinOp, UnOp};

/// Functions that only Google Sheets knows: an xlsx keeps them inside
/// `__xludf.DUMMYFUNCTION` with their last value, as Google writes them.
pub fn is_google(f: Func) -> bool {
    use Func::*;
    matches!(
        f,
        Query | ArrayFormula | Split | Join | Flatten | CountUnique | ArrayConstrain | SortN | GAdd | GMinus | GMultiply | GDivide | GPow | GEq | GNe | GGt | GGte | GLt | GLte | GUMinus | GUPlus | GUnaryPercent | IsEmail | IsUrl | ToText | ToPercent | ToDollars | ToDate | ToPureNumber
    )
}

/// The name Google Sheets wraps its own functions in when it writes xlsx.
const DUMMY: &str = "__xludf.DUMMYFUNCTION";

/// What a formula that Google Sheets wrote in an xlsx stands for.
#[derive(Debug, PartialEq)]
pub enum FromGoogle {
    /// The formula inside, calculated again.
    Formula(Expr),
    /// Uses something not known here (IMPORTRANGE, GOOGLEFINANCE...): the
    /// formula as written, which gives the last value.
    Fixed(Expr),
    /// A cell of the array the formula above or to the left spills.
    Spilled,
}

/// `__xludf.DUMMYFUNCTION("...")`, alone or inside `IFERROR(..., last
/// value)`: the text inside and whether it has the IFERROR.
fn dummy_inner(e: &Expr) -> Option<(&str, bool)> {
    match e {
        Expr::Call { func: Some(Func::IfError), args, .. } if args.len() == 2 => match &args[0] {
            Expr::Call { name, args, .. } if name.eq_ignore_ascii_case(DUMMY) => match args.as_slice() {
                [Expr::Text(t)] => Some((&**t, true)),
                _ => None,
            },
            _ => None,
        },
        Expr::Call { name, args, .. } if name.eq_ignore_ascii_case(DUMMY) => match args.as_slice() {
            [Expr::Text(t)] => Some((&**t, false)),
            _ => None,
        },
        _ => None,
    }
}

/// A value as a formula constant.
fn literal(v: &Value) -> Expr {
    match v {
        Value::Number(n) => Expr::Number(*n),
        Value::Text(t) => Expr::Text(t.clone()),
        Value::Bool(b) => Expr::Bool(*b),
        Value::Error(k) => Expr::Error(*k),
        _ => Expr::Text("".into()),
    }
}

/// Reads a formula written by Google Sheets (`cached` is its last value);
/// None for any other formula.
pub fn from_google(e: &Expr, cached: &Value) -> Option<FromGoogle> {
    let (inner, wrapped) = dummy_inner(e)?;
    if inner == "\"COMPUTED_VALUE\"" {
        return Some(FromGoogle::Spilled);
    }
    if let Ok(x) = crate::formula::parse_canonical(inner) {
        let mut unknown = false;
        x.visit_unknown_calls(&mut |_| unknown = true);
        if !unknown {
            return Some(FromGoogle::Formula(x));
        }
    }
    Some(FromGoogle::Fixed(if wrapped { e.clone() } else { Expr::Call { name: Func::IfError.name().into(), func: Some(Func::IfError), args: vec![e.clone(), literal(cached)] } }))
}

/// Whether a formula calls a function only Google Sheets has.
pub fn uses_google(e: &Expr) -> bool {
    match e {
        Expr::Call { func, args, .. } => func.is_some_and(is_google) || args.iter().any(uses_google),
        Expr::Unary(_, x) | Expr::Percent(x) | Expr::Paren(x) => uses_google(x),
        Expr::Binary(_, l, r) => uses_google(l) || uses_google(r),
        Expr::Array(rows) => rows.iter().flatten().any(uses_google),
        _ => false,
    }
}

fn wrap(inner: &str, cached: &Value) -> String {
    format!("IFERROR({DUMMY}(\"{}\"),{})", inner.replace('"', "\"\""), literal(cached).to_xlsx())
}

/// The xlsx text of a formula with Google functions (`formula` without
/// `=`), as Google Sheets writes it: Excel shows the last value.
pub fn to_google(formula: &str, cached: &Value) -> String {
    wrap(formula, cached)
}

/// The xlsx formula of a cell that such a formula spills into.
pub fn spilled_to_google(v: &Value) -> String {
    wrap("\"COMPUTED_VALUE\"", v)
}

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    match f {
        Query | ArrayFormula | Join | Flatten | CountUnique | SortN => Some(ALL),
        ArrayConstrain => Some(0b1),
        _ => None,
    }
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    if !is_google(f) {
        return None;
    }
    let n = a.len();
    let need = |min: usize, max: usize| if n < min || n > max { Err(ErrorKind::Value) } else { Ok(()) };
    let bin = |op: BinOp| need(2, 2).map(|_| env.eval(&Expr::Binary(op, Box::new(a[0].clone()), Box::new(a[1].clone()))));
    Some(match f {
        Query => need(1, 3).and_then(|_| super::gquery::query(env, a)),
        // Every formula spills here, so ARRAYFORMULA only passes its value on.
        ArrayFormula => need(1, 1).map(|_| env.eval(&a[0])),
        Split => need(2, 4).and_then(|_| split(env, a)),
        Join => need(2, 255).and_then(|_| {
            let sep = env.text(&a[0])?;
            let mut out: Vec<Arc<str>> = Vec::new();
            // Too long a result is #VALUE!, found before joining.
            let mut len = 0usize;
            for e in &a[1..] {
                let t = env.tab(e)?;
                for i in 0..t.len() {
                    let s = t.nth(i).to_text()?;
                    len = len.saturating_add(s.len() + sep.len());
                    if len > super::MAX_TEXT_BYTES + sep.len() {
                        return Err(ErrorKind::Value);
                    }
                    out.push(s);
                }
            }
            Ok(Value::text(out.join(&*sep)))
        }),
        Flatten => need(1, 255).and_then(|_| {
            let mut out = Vec::new();
            for e in a {
                let t = env.tab(e)?;
                if out.len().saturating_add(t.len()) > MAX_ARRAY {
                    return Err(ErrorKind::Num);
                }
                for i in 0..t.len() {
                    out.push(t.nth(i));
                }
            }
            Ok(Matrix::new(out.len(), 1, out).into_value())
        }),
        CountUnique => need(1, 255).and_then(|_| {
            let mut seen = std::collections::HashSet::new();
            for e in a {
                let t = env.tab(e)?;
                for i in 0..t.len() {
                    let v = t.nth(i);
                    if !matches!(v, Value::Empty) {
                        seen.insert(key(std::slice::from_ref(&v)));
                    }
                }
            }
            Ok(Value::Number(seen.len() as f64))
        }),
        ArrayConstrain => need(3, 3).and_then(|_| {
            let t = env.tab(&a[0])?;
            let rows = env.opt_num(a, 1, 0.0)?.trunc();
            let cols = env.opt_num(a, 2, 0.0)?.trunc();
            if rows < 1.0 || cols < 1.0 {
                return Err(ErrorKind::Value);
            }
            let (rows, cols) = ((rows as usize).min(t.rows()), (cols as usize).min(t.cols()));
            let data = (0..rows).flat_map(|r| (0..cols).map(move |c| (r, c))).map(|(r, c)| t.get(r, c)).collect();
            Ok(Matrix::new(rows, cols, data).into_value())
        }),
        SortN => sort_n(env, a),
        GAdd => bin(BinOp::Add),
        GMinus => bin(BinOp::Sub),
        GMultiply => bin(BinOp::Mul),
        GDivide => bin(BinOp::Div),
        GPow => bin(BinOp::Pow),
        GEq => bin(BinOp::Eq),
        GNe => bin(BinOp::Ne),
        GGt => bin(BinOp::Gt),
        GGte => bin(BinOp::Ge),
        GLt => bin(BinOp::Lt),
        GLte => bin(BinOp::Le),
        GUMinus => need(1, 1).map(|_| env.eval(&Expr::Unary(UnOp::Neg, Box::new(a[0].clone())))),
        GUPlus => need(1, 1).map(|_| env.eval(&a[0])),
        GUnaryPercent => need(1, 1).map(|_| env.eval(&Expr::Percent(Box::new(a[0].clone())))),
        IsEmail => need(1, 1).map(|_| {
            let t = env.scalar(&a[0]).to_text().unwrap_or_default();
            Value::Bool(is_email(&t))
        }),
        IsUrl => need(1, 1).map(|_| {
            let t = env.scalar(&a[0]).to_text().unwrap_or_default();
            Value::Bool(is_url(&t))
        }),
        ToText => need(1, 1).and_then(|_| env.scalar(&a[0]).to_text().map(Value::Text)),
        // The number itself: the format they give a cell isn't kept.
        ToPercent | ToDollars | ToDate | ToPureNumber => need(1, 1).map(|_| match env.scalar(&a[0]) {
            Value::Number(x) => Value::Number(x),
            Value::Bool(b) => Value::Number(b as u8 as f64),
            Value::Empty => Value::Number(0.0),
            v => v,
        }),
        _ => return None,
    })
}

fn is_email(s: &str) -> bool {
    let Some((user, host)) = s.split_once('@') else { return false };
    let ok = |c: char| !c.is_whitespace() && c != '@';
    !user.is_empty() && user.chars().all(ok) && host.chars().all(ok) && host.rsplit_once('.').is_some_and(|(name, tld)| !name.is_empty() && tld.len() >= 2 && tld.chars().all(|c| c.is_alphabetic()))
}

fn is_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    let rest = ["http://", "https://", "ftp://"].iter().find_map(|p| lower.strip_prefix(p)).unwrap_or(&lower);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
    !s.contains(char::is_whitespace) && host.rsplit_once('.').is_some_and(|(name, tld)| !name.is_empty() && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())) && host.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '-'))
}

/// A piece of SPLIT: numbers become numbers, as in Google Sheets.
fn piece(s: &str) -> Value {
    match s.trim().parse::<f64>() {
        Ok(x) if x.is_finite() && !s.trim().is_empty() => Value::Number(x),
        _ => Value::text(s),
    }
}

fn split_one(text: &str, delim: &str, each: bool, remove_empty: bool) -> Vec<Value> {
    let parts: Vec<&str> = if delim.is_empty() {
        vec![text]
    } else if each {
        text.split(|c: char| delim.contains(c)).collect()
    } else {
        text.split(delim).collect()
    };
    parts.into_iter().filter(|p| !remove_empty || !p.is_empty()).map(piece).collect()
}

/// SPLIT(text, delimiter, [split_by_each], [remove_empty_text]): a row of
/// pieces, a row for each text of a range.
fn split(env: &Env, a: &[Expr]) -> R<Value> {
    let delim = env.text(&a[1])?;
    let each = env.opt_bool(a, 2, true)?;
    let remove_empty = env.opt_bool(a, 3, true)?;
    let t = env.tab(&a[0])?;
    let mut rows = Vec::with_capacity(t.len());
    // Pieces in all, bounded as they're made (not once all are there).
    let mut pieces = 0usize;
    for i in 0..t.len() {
        let row = match t.nth(i) {
            Value::Error(k) => vec![Value::Error(k)],
            v => split_one(&v.to_text()?, &delim, each, remove_empty),
        };
        pieces += row.len().max(1);
        if pieces > MAX_ARRAY {
            return Err(ErrorKind::Num);
        }
        rows.push(row);
    }
    let w = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    if rows.len() * w > MAX_ARRAY {
        return Err(ErrorKind::Num);
    }
    let h = rows.len();
    let data = rows.into_iter().flat_map(|mut r| {
        r.resize(w, Value::Empty);
        r
    });
    Ok(Matrix::new(h, w, data.collect()).into_value())
}

/// A key equal for equal values (to find them in a hash table).
pub(super) fn key(vs: &[Value]) -> String {
    vs.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join("\u{1}")
}

/// SORTN(range, [n], [ties_mode], [sort_column, is_ascending]...): the
/// first n rows once sorted. Ties mode 0 cuts at n, 1 keeps the rows tied
/// with the last one, 2 drops repeated rows first and 3 keeps the rows
/// of the first n different ones.
fn sort_n(env: &Env, a: &[Expr]) -> R<Value> {
    if a.is_empty() || (a.len() > 3 && a.len() % 2 == 0) {
        return Err(ErrorKind::Value);
    }
    let m = env.tab(&a[0])?.to_matrix()?;
    let n = env.opt_num(a, 1, 1.0)?.trunc();
    let mode = env.opt_num(a, 2, 0.0)?.trunc() as i64;
    if n < 0.0 || !(0..=3).contains(&mode) {
        return Err(ErrorKind::Value);
    }
    let n = n as usize;
    let mut keys = Vec::new();
    for p in a.get(3..).unwrap_or(&[]).chunks(2) {
        let c = env.num(&p[0])?.trunc();
        if c < 1.0 || c as usize > m.cols {
            return Err(ErrorKind::Value);
        }
        keys.push((c as usize - 1, env.boolean(&p[1])?));
    }
    let row = |r: usize| &m.data[r * m.cols..(r + 1) * m.cols];
    let mut order: Vec<usize> = (0..m.rows).collect();
    let by_keys = |x: usize, y: usize| {
        for &(c, asc) in &keys {
            let o = m.get(x, c).compare(m.get(y, c));
            if o != std::cmp::Ordering::Equal {
                return if asc { o } else { o.reverse() };
            }
        }
        std::cmp::Ordering::Equal
    };
    order.sort_by(|&x, &y| by_keys(x, y));
    let keep: Vec<usize> = match mode {
        0 => order.into_iter().take(n).collect(),
        1 => {
            let mut out: Vec<usize> = order.iter().copied().take(n).collect();
            if let Some(&last) = out.last() {
                out.extend(order[n..].iter().copied().take_while(|&r| by_keys(r, last) == std::cmp::Ordering::Equal));
            }
            out
        }
        // Rows seen are looked up by key (a scan of them all would make
        // it quadratic).
        2 => {
            let mut out: Vec<usize> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for r in order {
                if out.len() == n {
                    break;
                }
                if seen.insert(key(row(r))) {
                    out.push(r);
                }
            }
            out
        }
        _ => {
            let mut distinct = std::collections::HashSet::new();
            let mut out = Vec::new();
            for r in order {
                let k = key(row(r));
                if !distinct.contains(&k) {
                    if distinct.len() == n {
                        continue;
                    }
                    distinct.insert(k);
                }
                out.push(r);
            }
            out
        }
    };
    if keep.is_empty() {
        return Err(ErrorKind::Na);
    }
    let data = keep.iter().flat_map(|&r| row(r).iter().cloned()).collect();
    Ok(Matrix::new(keep.len(), m.cols, data).into_value())
}

#[cfg(test)]
mod tests {
    use crate::{CellRef, Locale, Value, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let rows: [&[&str]; 6] = [&["Name", "Team", "Score"], &["Ann", "Red", "10"], &["Bob", "Blue", "7"], &["Cid", "Red", "7"], &["Dee", "Blue", "3"], &["Eve", "Red", ""]];
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

    #[test]
    fn google_functions() {
        let w = wb();
        assert_eq!(text(&w, "=SPLIT(\"a,b;;3\",\",;\")"), "a,b,3");
        assert_eq!(w.eval_formula(0, "=INDEX(SPLIT(\"a,b,3\",\",\"),3)"), Value::Number(3.0));
        assert_eq!(text(&w, "=SPLIT(\"a--b-c\",\"--\",FALSE)"), "a,b-c");
        assert_eq!(text(&w, "=SPLIT(\"a,,b\",\",\",TRUE,FALSE)"), "a,,b");
        assert_eq!(text(&w, "=SPLIT(A2:A3&\" x\",\" \")"), "Ann,x;Bob,x");
        assert_eq!(text(&w, "=JOIN(\"-\",A2:A4,\"z\")"), "Ann-Bob-Cid-z");
        assert_eq!(text(&w, "=FLATTEN(A2:B3)"), "Ann;Red;Bob;Blue");
        assert_eq!(text(&w, "=COUNTUNIQUE(B2:B6,C2:C6)"), "5");
        assert_eq!(text(&w, "=ARRAY_CONSTRAIN(A1:C6,2,2)"), "Name,Team;Ann,Red");
        assert_eq!(text(&w, "=ARRAYFORMULA(C2:C4*2)"), "20;14;14");
        assert_eq!(text(&w, "=ADD(1,2)&MINUS(5,2)&MULTIPLY(2,3)&DIVIDE(8,4)&POW(2,3)"), "33628");
        assert_eq!(text(&w, "=EQ(1,1)&NE(1,1)&GT(2,1)&GTE(1,1)&LT(2,1)&LTE(1,2)"), "TRUEFALSETRUETRUEFALSETRUE");
        assert_eq!(text(&w, "=UMINUS(3)&UPLUS(3)&UNARY_PERCENT(50)"), "-330.5");
        assert_eq!(text(&w, "=ISEMAIL(\"a.b@x.org\")&ISEMAIL(\"a@b\")&ISURL(\"https://x.com/a\")&ISURL(\"x com\")"), "TRUEFALSETRUEFALSE");
        assert_eq!(text(&w, "=TO_TEXT(12)&TO_PURE_NUMBER(0.5)"), "120.5");
        assert_eq!(text(&w, "=REGEXMATCH(\"abc\",\"b\")"), "TRUE");
    }

    #[test]
    fn from_and_to_google() {
        use super::{FromGoogle, from_google, to_google, uses_google};
        use crate::formula::parse_canonical;
        let e = parse_canonical("IFERROR(__xludf.DUMMYFUNCTION(\"QUERY(A1:C6,\"\"select A\"\")\"),\"Name\")").unwrap();
        let Some(FromGoogle::Formula(x)) = from_google(&e, &Value::text("Name")) else { panic!() };
        assert!(uses_google(&x));
        assert_eq!(x.to_xlsx(), "QUERY(A1:C6,\"select A\")");
        assert_eq!(to_google(&x.to_xlsx(), &Value::text("Name")), "IFERROR(__xludf.DUMMYFUNCTION(\"QUERY(A1:C6,\"\"select A\"\")\"),\"Name\")");
        let e = parse_canonical("IFERROR(__xludf.DUMMYFUNCTION(\"\"\"COMPUTED_VALUE\"\"\"),3)").unwrap();
        assert_eq!(from_google(&e, &Value::Number(3.0)), Some(FromGoogle::Spilled));
        // Unknown here: the last value stays.
        let e = parse_canonical("__xludf.DUMMYFUNCTION(\"GOOGLEFINANCE(\"\"GOOG\"\")\")").unwrap();
        let Some(FromGoogle::Fixed(x)) = from_google(&e, &Value::Number(150.0)) else { panic!() };
        let w = wb();
        assert_eq!(w.eval_formula(0, &format!("={}", x.to_xlsx())), Value::Number(150.0));
        assert_eq!(from_google(&parse_canonical("SUM(1)").unwrap(), &Value::Empty), None);
    }

    #[test]
    fn sortn() {
        let w = wb();
        assert_eq!(text(&w, "=SORTN(A2:C5,2,0,3,FALSE)"), "Ann,Red,10;Bob,Blue,7");
        assert_eq!(text(&w, "=SORTN(A2:C5,2,1,3,FALSE)"), "Ann,Red,10;Bob,Blue,7;Cid,Red,7");
        assert_eq!(text(&w, "=SORTN(C2:C5,2,2,1,FALSE)"), "10;7");
        assert_eq!(text(&w, "=SORTN(C2:C5,2,3,1,FALSE)"), "10;7;7");
    }
}
