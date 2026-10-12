//! Spreadsheet functions: database (DSUM, DCOUNT...) over a range with a
//! header row, filtered by a criteria range.

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    handles(f).then_some(0b101)
}

fn handles(f: Func) -> bool {
    use Func::*;
    matches!(f, DAverage | DCount | DCountA | DGet | DMax | DMin | DProduct | DStdev | DStdevP | DSum | DVar | DVarP)
}

/// One condition of a criteria row.
enum Cond {
    /// A database column and what its values must match.
    Col(usize, Criteria),
    /// A computed criterion: a formula written for the first record,
    /// evaluated for each one (sheet, cell and formula).
    Formula(usize, CellRef, Expr),
    Fixed(bool),
}

/// Header text as matched against field names.
fn key(v: &Value) -> String {
    v.to_text().map(|t| t.trim().to_lowercase()).unwrap_or_default()
}

fn column(db: &Tab, name: &Value) -> Option<usize> {
    let k = key(name);
    if k.is_empty() {
        return None;
    }
    (0..db.cols()).find(|&c| key(&db.get(0, c)) == k)
}

/// Criteria as database criteria read them: plain text matches the
/// values that start with it.
fn criterion(v: Value, loc: &Locale) -> Criteria {
    match &v {
        Value::Text(t) if !t.starts_with(['<', '>', '=']) && matches!(format::parse_input(t, loc).value, Value::Text(_)) => Criteria::new(Value::text(format!("{t}*")), loc),
        _ => Criteria::new(v, loc),
    }
}

/// The criteria range: rows of conditions (the rows are alternatives).
fn criteria(env: &Env, db: &Tab, e: &Expr) -> R<Vec<Vec<Cond>>> {
    let c = env.tab(e)?;
    if c.rows() < 2 {
        return Ok(vec![Vec::new()]);
    }
    let mut out = Vec::with_capacity(c.rows() - 1);
    for r in 1..c.rows() {
        let mut row = Vec::new();
        for j in 0..c.cols() {
            let v = c.get(r, j);
            if v.is_empty() || matches!(&v, Value::Text(t) if t.is_empty()) {
                continue;
            }
            if let Some(col) = column(db, &c.get(0, j)) {
                row.push(Cond::Col(col, criterion(v, &env.locale)));
                continue;
            }
            let formula = match &c {
                Tab::R(ctx, s, rg) => {
                    let at = CellRef::new(rg.start.row + r as u32, rg.start.col + j as u32);
                    ctx.formula_text(*s, at).and_then(|t| crate::formula::parse(&t, &env.locale).ok()).map(|x| (*s, at, x))
                }
                Tab::A(_) => None,
            };
            row.push(match formula {
                Some((s, at, x)) => Cond::Formula(s, at, x),
                None => Cond::Fixed(v == Value::Bool(true)),
            });
        }
        out.push(row);
    }
    Ok(out)
}

fn keeps(env: &Env, db: &Tab, crit: &[Vec<Cond>], r: usize) -> bool {
    crit.iter().any(|row| {
        row.iter().all(|c| match c {
            Cond::Col(col, k) => k.matches(&db.get(r, *col)),
            Cond::Formula(s, at, x) => {
                let mut x = x.clone();
                x.offset(r as i64 - 1, 0);
                let e = Env::new(env.ctx, *s, *at, env.locale);
                matches!(e.eval(&x).to_bool(), Ok(true))
            }
            Cond::Fixed(b) => *b,
        })
    })
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    if !handles(f) {
        return None;
    }
    Some((|| {
        if a.len() != 3 {
            return Err(ErrorKind::Value);
        }
        let db = env.tab(&a[0])?;
        let field = match &a[1] {
            Expr::Missing if matches!(f, DCount | DCountA) => None,
            e => Some(match env.scalar(e) {
                Value::Error(k) => return Err(k),
                Value::Number(n) if n >= 1.0 && (n as usize) <= db.cols() => n as usize - 1,
                v @ Value::Text(_) => column(&db, &v).ok_or(ErrorKind::Value)?,
                _ => return Err(ErrorKind::Value),
            }),
        };
        let crit = criteria(env, &db, &a[2])?;
        let recs: Vec<usize> = (1..db.rows()).filter(|&r| keeps(env, &db, &crit, r)).collect();
        let Some(col) = field else { return Ok(Value::Number(recs.len() as f64)) };
        let vals = recs.iter().map(|&r| db.get(r, col));
        match f {
            DCount => return Ok(Value::Number(vals.filter(|v| matches!(v, Value::Number(_))).count() as f64)),
            DCountA => return Ok(Value::Number(vals.filter(|v| !v.is_empty()).count() as f64)),
            DGet => {
                return match recs.len() {
                    0 => Err(ErrorKind::Value),
                    1 => Ok(match db.get(recs[0], col) {
                        Value::Empty => Value::Number(0.0),
                        v => v,
                    }),
                    _ => Err(ErrorKind::Num),
                };
            }
            _ => {}
        }
        let mut x = Vec::new();
        for v in vals {
            match v {
                Value::Number(n) => x.push(n),
                Value::Error(k) => return Err(k),
                _ => {}
            }
        }
        let n = x.len() as f64;
        let mean = x.iter().sum::<f64>() / n;
        let ss = x.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
        let need = |min: usize| if x.len() < min { Err(ErrorKind::Div0) } else { Ok(()) };
        Ok(num_ok(match f {
            DSum => x.iter().sum(),
            DAverage => {
                need(1)?;
                mean
            }
            DMax => x.iter().copied().reduce(f64::max).unwrap_or(0.0),
            DMin => x.iter().copied().reduce(f64::min).unwrap_or(0.0),
            DProduct if x.is_empty() => 0.0,
            DProduct => x.iter().product(),
            DStdev | DVar => {
                need(2)?;
                let v = ss / (n - 1.0);
                if f == DStdev { v.sqrt() } else { v }
            }
            _ => {
                need(1)?;
                let v = ss / n;
                if f == DStdevP { v.sqrt() } else { v }
            }
        }))
    })())
}

#[cfg(test)]
mod tests {
    use crate::{CellRef, ErrorKind, Locale, Value, Workbook};

    /// Microsoft's DAVERAGE/DSUM example: criteria in A1:F3, database in A5:E11.
    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let rows: [&[&str]; 11] = [
            &["Tree", "Height", "Age", "Yield", "Profit", "Height"],
            &["=\"=Apple\"", ">10", "", "", "", "<16"],
            &["=\"=Pear\""],
            &[],
            &["Tree", "Height", "Age", "Yield", "Profit"],
            &["Apple", "18", "20", "14", "105"],
            &["Pear", "12", "12", "10", "96"],
            &["Cherry", "13", "14", "9", "105"],
            &["Apple", "14", "15", "10", "75"],
            &["Pear", "9", "8", "8", "76.8"],
            &["Apple", "8", "9", "6", "45"],
        ];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                if !v.is_empty() {
                    w.set_input(0, CellRef::new(r as u32, c as u32), v);
                }
            }
        }
        w
    }

    fn close(w: &Workbook, f: &str, want: f64) {
        match w.eval_formula(0, f) {
            Value::Number(got) => assert!((got - want).abs() < 1e-9 * want.abs().max(1.0), "{f}: {got} != {want}"),
            v => panic!("{f} gave {v:?}"),
        }
    }

    #[test]
    fn microsoft_example() {
        // Results as Excel's documentation and LibreOffice give them.
        let w = wb();
        close(&w, "=DCOUNT(A5:E11,\"Age\",A1:F2)", 1.0);
        close(&w, "=DCOUNTA(A5:E11,\"Profit\",A1:F2)", 1.0);
        close(&w, "=DMAX(A5:E11,\"Profit\",A1:A3)", 105.0);
        close(&w, "=DMIN(A5:E11,\"Profit\",A1:B2)", 75.0);
        close(&w, "=DSUM(A5:E11,\"Profit\",A1:A2)", 225.0);
        close(&w, "=DSUM(A5:E11,\"Profit\",A1:F2)", 75.0);
        close(&w, "=DPRODUCT(A5:E11,\"Yield\",A1:F2)", 10.0);
        close(&w, "=DAVERAGE(A5:E11,\"Yield\",A1:B2)", 12.0);
        close(&w, "=DAVERAGE(A5:E11,3,A5:E11)", 13.0);
        close(&w, "=DSTDEV(A5:E11,\"Yield\",A1:A3)", 2.966_479_394_838_27);
        close(&w, "=DSTDEVP(A5:E11,\"Yield\",A1:A3)", 2.653_299_832_284_32);
        close(&w, "=DVAR(A5:E11,\"Yield\",A1:A3)", 8.8);
        close(&w, "=DVARP(A5:E11,\"Yield\",A1:A3)", 7.04);
        close(&w, "=DCOUNTA(A5:E11,\"Profit\",A1:A3)", 5.0);
        close(&w, "=DCOUNT(A5:E11,,A1:A3)", 5.0);
        assert_eq!(w.eval_formula(0, "=DGET(A5:E11,\"Yield\",A1:A3)"), Value::Error(ErrorKind::Num));
        assert_eq!(w.eval_formula(0, "=DGET(A5:E11,\"Yield\",{\"Tree\";\"Kiwi\"})"), Value::Error(ErrorKind::Value));
        close(&w, "=DGET(A5:E11,\"yield\",{\"Tree\";\"Cherry\"})", 9.0);
        assert_eq!(w.eval_formula(0, "=DSUM(A5:E11,\"Nope\",A1:A2)"), Value::Error(ErrorKind::Value));
        assert_eq!(w.eval_formula(0, "=DSUM(A5:E11,,A1:A2)"), Value::Error(ErrorKind::Value));
    }

    #[test]
    fn criteria_forms() {
        let w = wb();
        // Plain text is a prefix match; wildcards; "<>"; numbers; Spanish name.
        close(&w, "=DSUM(A5:E11,5,{\"Tree\";\"Ch\"})", 105.0);
        close(&w, "=DSUM(A5:E11,5,{\"Tree\";\"?ear\"})", 172.8);
        close(&w, "=DCOUNT(A5:E11,5,{\"Tree\";\"<>Apple\"})", 3.0);
        close(&w, "=DCOUNT(A5:E11,5,{\"Age\";12})", 1.0);
        close(&w, "=DSUM(A5:E11,5,{\"Tree\",\"Age\";\"Apple\",\">10\";\"Pear\",\"\"})", 352.8);
        close(&w, "=DSUM(A5:E11,5,{\"Tree\"})", 502.8);
        close(&w, "=DMAX(A5:E11,5,{\"Tree\";\"Kiwi\"})", 0.0);
        let mut w = w;
        // A computed criterion: header not in the database.
        w.set_input(0, CellRef::new(0, 8), "Big");
        w.set_input(0, CellRef::new(1, 8), "=C6>AVERAGE($C$6:$C$11)");
        close(&w, "=DCOUNT(A5:E11,\"Age\",I1:I2)", 3.0);
        close(&w, "=DSUM(A5:E11,\"Profit\",I1:I2)", 285.0);
        w.locale = Locale::ES;
        close(&w, "=BDSUMA(A5:E11;\"Profit\";A1:A2)", 225.0);
    }
}
