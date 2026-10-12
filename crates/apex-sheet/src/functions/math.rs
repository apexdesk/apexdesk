//! Spreadsheet functions: math.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    Some(match f {
        SumSq | SumX2My2 | SumX2Py2 | SumXMy2 | Gcd | Lcm | Multinomial => ALL,
        Aggregate => !0b11,
        SeriesSum => 0b1000,
        MMult => 0b11,
        MInverse | MDeterm => 0b1,
        _ => return None,
    })
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    eval(env, f, a).transpose()
}

/// Largest integer a double holds exactly; Excel's limit for GCD, BASE...
const EXACT: f64 = 9_007_199_254_740_992.0;

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Option<Value>> {
    use ErrorKind::{Div0, Num};
    use Func::*;
    let n = a.len();
    let need = |min: usize, max: usize| if n < min || n > max { Err(ErrorKind::Value) } else { Ok(()) };
    Ok(Some(match f {
        SumSq => num_ok(env.fold_numbers(a, 0.0, |s, x| s + x * x)?),
        SumX2My2 | SumX2Py2 | SumXMy2 => {
            need(2, 2)?;
            let (x, y) = (env.tab(&a[0])?, env.tab(&a[1])?);
            if x.len() != y.len() {
                return Err(ErrorKind::Na);
            }
            let mut s = 0.0;
            for i in 0..x.len() {
                match (x.nth(i), y.nth(i)) {
                    (Value::Error(k), _) | (_, Value::Error(k)) => return Err(k),
                    (Value::Number(p), Value::Number(q)) => {
                        s += match f {
                            SumX2My2 => p * p - q * q,
                            SumX2Py2 => p * p + q * q,
                            _ => (p - q) * (p - q),
                        }
                    }
                    // Pairs with text, logicals or blanks are left out.
                    _ => {}
                }
            }
            num_ok(s)
        }
        Gcd | Lcm => {
            let mut v = Vec::new();
            env.numbers(a, &mut v)?;
            if v.is_empty() {
                return Err(ErrorKind::Value);
            }
            let mut acc = if f == Gcd { 0.0 } else { 1.0 };
            for x in v {
                let x = x.trunc();
                if !(0.0..EXACT).contains(&x) {
                    return Err(Num);
                }
                acc = if f == Gcd {
                    gcd(acc as u64, x as u64) as f64
                } else if x == 0.0 || acc == 0.0 {
                    0.0
                } else {
                    acc / gcd(acc as u64, x as u64) as f64 * x
                };
                if acc >= EXACT {
                    return Err(Num);
                }
            }
            Value::Number(acc)
        }
        Fact => {
            let x = env.num1(a)?.trunc();
            if !(0.0..=170.0).contains(&x) {
                return Err(Num);
            }
            Value::Number((2..=x as u32).fold(1.0, |p, i| p * i as f64))
        }
        FactDouble => {
            let x = env.num1(a)?.trunc();
            if !(0.0..=300.0).contains(&x) {
                return Err(Num);
            }
            num_ok((1..=x as u32).rev().step_by(2).fold(1.0, |p, i| p * i as f64))
        }
        Combin | Combina | Permut | Permutationa => {
            need(2, 2)?;
            let (x, k) = (env.num(&a[0])?.trunc(), env.num(&a[1])?.trunc());
            if x < 0.0 || k < 0.0 {
                return Err(Num);
            }
            num_ok(match f {
                Combin => combin(x, k)?,
                Combina if x == 0.0 && k == 0.0 => 1.0,
                Combina => combin(x + k - 1.0, k)?,
                Permut => {
                    if k > x {
                        return Err(Num);
                    }
                    let mut p: f64 = 1.0;
                    let mut i = 0.0;
                    // Stops early: the product overflows long before k gets big.
                    while i < k && p.is_finite() {
                        p *= x - i;
                        i += 1.0;
                    }
                    p
                }
                _ => x.powf(k),
            })
        }
        Multinomial => {
            let mut v = Vec::new();
            env.numbers(a, &mut v)?;
            let (mut sum, mut r) = (0.0, 1.0);
            for x in v {
                let x = x.trunc();
                if x < 0.0 {
                    return Err(Num);
                }
                sum += x;
                r *= combin(sum, x)?;
                if !r.is_finite() {
                    return Err(Num);
                }
            }
            Value::Number(r)
        }
        Sin | Cos | Tan | Asin | Acos | Atan | Sinh | Cosh | Tanh | Asinh | Acosh | Atanh | Cot | Coth | Csc | Csch | Sec | Sech | Acot | Acoth | Degrees | Radians | SqrtPi => num_ok(unary(f, env.num1(a)?)?),
        Atan2 => {
            need(2, 2)?;
            let (x, y) = (env.num(&a[0])?, env.num(&a[1])?);
            if x == 0.0 && y == 0.0 {
                return Err(Div0);
            }
            Value::Number(y.atan2(x))
        }
        Roman => {
            need(1, 2)?;
            let x = env.num(&a[0])?.trunc();
            let form = match a.get(1) {
                None | Some(Expr::Missing) => 0.0,
                Some(e) => match env.scalar(e) {
                    Value::Bool(b) => if b { 0.0 } else { 4.0 },
                    v => v.to_number()?.trunc(),
                },
            };
            if !(0.0..=3999.0).contains(&x) || !(0.0..=4.0).contains(&form) {
                return Err(ErrorKind::Value);
            }
            Value::text(roman(x as u32, form as u32))
        }
        Arabic => Value::Number(arabic(&env.text1(a)?).ok_or(ErrorKind::Value)?),
        Base => {
            need(2, 3)?;
            let x = env.num(&a[0])?.trunc();
            let radix = env.num(&a[1])?.trunc();
            let min = env.opt_num(a, 2, 0.0)?.trunc();
            if !(0.0..EXACT).contains(&x) || !(2.0..=36.0).contains(&radix) || !(0.0..=255.0).contains(&min) {
                return Err(Num);
            }
            let (mut x, radix) = (x as u64, radix as u64);
            let mut digits = Vec::new();
            while x > 0 {
                digits.push(std::char::from_digit((x % radix) as u32, radix as u32).unwrap_or('0').to_ascii_uppercase());
                x /= radix;
            }
            while digits.len() < (min as usize).max(1) {
                digits.push('0');
            }
            Value::text(digits.iter().rev().collect::<String>())
        }
        Decimal => {
            need(2, 2)?;
            let s = env.text(&a[0])?;
            let radix = env.num(&a[1])?.trunc();
            if !(2.0..=36.0).contains(&radix) || s.chars().count() > 255 {
                return Err(Num);
            }
            let mut x = 0.0;
            for c in s.chars() {
                x = x * radix + c.to_digit(radix as u32).ok_or(Num)? as f64;
            }
            num_ok(x)
        }
        MRound => {
            need(2, 2)?;
            let (x, m) = (env.num(&a[0])?, env.num(&a[1])?);
            if m == 0.0 {
                return Ok(Some(Value::Number(0.0)));
            }
            if x * m < 0.0 {
                return Err(Num);
            }
            // f64::round is half away from zero, like Excel.
            num_ok(sig15(sig15(x / m).round() * m))
        }
        Even | Odd => {
            let x = env.num1(a)?;
            let mut v = sig15(x.abs()).ceil();
            let even = v % 2.0 == 0.0;
            if (f == Even) != even || (f == Odd && v == 0.0) {
                v += 1.0;
            }
            num_ok(if x < 0.0 { -v } else { v })
        }
        Quotient => {
            need(2, 2)?;
            let (x, d) = (env.num(&a[0])?, env.num(&a[1])?);
            if d == 0.0 {
                return Err(Div0);
            }
            num_ok(sig15(x / d).trunc() + 0.0)
        }
        CeilingMath | FloorMath | CeilingPrecise | FloorPrecise => {
            let precise = matches!(f, CeilingPrecise | FloorPrecise);
            need(1, if precise { 2 } else { 3 })?;
            let x = env.num(&a[0])?;
            let s = env.opt_num(a, 1, 1.0)?.abs();
            // A mode only matters for negative numbers: it flips the direction.
            let flip = !precise && x < 0.0 && env.opt_num(a, 2, 0.0)? != 0.0;
            if s == 0.0 {
                return Ok(Some(Value::Number(0.0)));
            }
            let q = sig15(x / s);
            let up = matches!(f, CeilingMath | CeilingPrecise) != flip;
            num_ok(sig15(if up { q.ceil() } else { q.floor() } * s))
        }
        Aggregate => {
            if n < 3 {
                return Err(ErrorKind::Value);
            }
            let func = env.num(&a[0])?.trunc();
            let opt = match &a[1] {
                Expr::Missing => 0.0,
                e => env.num(e)?.trunc(),
            };
            if !(1.0..=19.0).contains(&func) || !(0.0..=7.0).contains(&opt) {
                return Err(ErrorKind::Value);
            }
            let (func, opt) = (func as u32, opt as u32);
            if func >= 14 && n != 4 {
                return Err(ErrorKind::Value);
            }
            let (v, filled) = agg_values(env, if func >= 14 { &a[2..3] } else { &a[2..] }, opt)?;
            let k = if func >= 14 { env.num(&a[3])? } else { 0.0 };
            aggregate(func, v, filled, k)?
        }
        SeriesSum => {
            need(4, 4)?;
            let (x, p, m) = (env.num(&a[0])?, env.num(&a[1])?, env.num(&a[2])?);
            let t = env.tab(&a[3])?;
            let mut s = 0.0;
            for i in 0..t.len() {
                let c = match t.nth(i) {
                    Value::Number(c) => c,
                    Value::Error(k) => return Err(k),
                    _ => return Err(ErrorKind::Value),
                };
                s += c * x.powf(p + i as f64 * m);
            }
            num_ok(s)
        }
        MMult => {
            need(2, 2)?;
            let (x, y) = (num_matrix(env, &a[0])?, num_matrix(env, &a[1])?);
            if x.1 != y.0 {
                return Err(ErrorKind::Value);
            }
            let (rows, cols, inner) = (x.0, y.1, x.1);
            if rows * cols > MAX_ARRAY || rows.saturating_mul(cols).saturating_mul(inner) > MAX_PRODUCTS {
                return Err(Num);
            }
            let mut data = Vec::with_capacity(rows * cols);
            for r in 0..rows {
                for c in 0..cols {
                    data.push(num_ok((0..inner).map(|i| x.2[r * inner + i] * y.2[i * cols + c]).sum()));
                }
            }
            Matrix::new(rows, cols, data).into_value()
        }
        MInverse | MDeterm => {
            need(1, 1)?;
            let (rows, cols, m) = num_matrix(env, &a[0])?;
            if rows != cols {
                return Err(ErrorKind::Value);
            }
            if rows.saturating_mul(rows).saturating_mul(rows) > MAX_PRODUCTS {
                return Err(Num);
            }
            if f == MDeterm {
                num_ok(determinant(rows, m))
            } else {
                let inv = inverse(rows, m).ok_or(Num)?;
                Matrix::new(rows, rows, inv.into_iter().map(num_ok).collect()).into_value()
            }
        }
        MUnit => {
            let k = env.num1(a)?.trunc();
            if k < 1.0 {
                return Err(ErrorKind::Value);
            }
            if k * k > MAX_ARRAY as f64 {
                return Err(Num);
            }
            let k = k as usize;
            Matrix::new(k, k, (0..k * k).map(|i| Value::Number(if i / k == i % k { 1.0 } else { 0.0 })).collect()).into_value()
        }
        _ => return Ok(None),
    }))
}

/// Rounds to the 15 significant digits Excel works with, so quotients such
/// as 1.3/0.2 (6.4999...) round the way they read.
fn sig15(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x + 0.0;
    }
    format!("{:.14e}", x).parse::<f64>().unwrap_or(x) + 0.0
}

fn gcd(mut x: u64, mut y: u64) -> u64 {
    while y != 0 {
        (x, y) = (y, x % y);
    }
    x
}

fn combin(n: f64, k: f64) -> R<f64> {
    if n < 0.0 || k < 0.0 || k > n {
        return Err(ErrorKind::Num);
    }
    let k = k.min(n - k);
    let (mut r, mut i) = (1.0, 0.0);
    // C(n, k) >= 2^k here, so this overflows (and stops) before k passes ~1030.
    while i < k {
        r = r * (n - i) / (i + 1.0);
        if !r.is_finite() {
            return Err(ErrorKind::Num);
        }
        i += 1.0;
    }
    Ok(r.round())
}

fn unary(f: Func, x: f64) -> R<f64> {
    use ErrorKind::{Div0, Num};
    use Func::*;
    // Excel refuses circular functions of huge arguments (2^27 and up).
    if matches!(f, Sin | Cos | Tan | Cot | Csc | Sec | Coth | Csch | Sech) && x.abs() >= 134_217_728.0 {
        return Err(Num);
    }
    Ok(match f {
        Sin => x.sin(),
        Cos => x.cos(),
        Tan => x.tan(),
        Asin | Acos if x.abs() > 1.0 => return Err(Num),
        Asin => x.asin(),
        Acos => x.acos(),
        Atan => x.atan(),
        Sinh => x.sinh(),
        Cosh => x.cosh(),
        Tanh => x.tanh(),
        Asinh => x.asinh(),
        Acosh if x < 1.0 => return Err(Num),
        Acosh => x.acosh(),
        Atanh if x.abs() >= 1.0 => return Err(Num),
        Atanh => x.atanh(),
        Cot | Coth | Csc | Csch if x == 0.0 => return Err(Div0),
        Cot => 1.0 / x.tan(),
        Coth => 1.0 / x.tanh(),
        Csc => 1.0 / x.sin(),
        Csch => 1.0 / x.sinh(),
        Sec => 1.0 / x.cos(),
        Sech => 1.0 / x.cosh(),
        Acot => FRAC_PI_2 - x.atan(),
        Acoth if x.abs() <= 1.0 => return Err(Num),
        Acoth => 0.5 * ((x + 1.0) / (x - 1.0)).ln(),
        Degrees => x.to_degrees(),
        Radians => x.to_radians(),
        SqrtPi if x < 0.0 => return Err(Num),
        SqrtPi => (x * PI).sqrt(),
        _ => return Err(ErrorKind::Value),
    })
}

/// Excel's ROMAN: `mode` 0 is classic, 1 to 4 allow ever longer
/// subtractions (499 -> CDXCIX, LDVLIV, XDIX, VDIV, ID).
fn roman(mut n: u32, mode: u32) -> String {
    const CH: [char; 7] = ['M', 'D', 'C', 'L', 'X', 'V', 'I'];
    const VAL: [u32; 7] = [1000, 500, 100, 50, 10, 5, 1];
    let mut s = String::new();
    for i in 0..4 {
        let mut idx = 2 * i;
        let digit = n / VAL[idx];
        if digit % 5 == 4 {
            // n < 4000, so a 4 or 9 never shows up at the thousands.
            let idx2 = if digit == 4 { idx - 1 } else { idx - 2 };
            let mut steps = 0;
            while steps < mode && idx < 6 {
                steps += 1;
                if VAL[idx2] - VAL[idx + 1] <= n {
                    idx += 1;
                } else {
                    steps = mode;
                }
            }
            s.push(CH[idx]);
            s.push(CH[idx2]);
            n = (n + VAL[idx]).saturating_sub(VAL[idx2]);
        } else {
            if digit > 4 {
                s.push(CH[idx - 1]);
            }
            s.extend(std::iter::repeat_n(CH[idx], (digit % 5) as usize));
            n %= VAL[idx];
        }
    }
    s
}

fn arabic(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.chars().count() > 255 {
        return None;
    }
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let vals = s
        .chars()
        .map(|c| match c.to_ascii_uppercase() {
            'I' => Some(1.0),
            'V' => Some(5.0),
            'X' => Some(10.0),
            'L' => Some(50.0),
            'C' => Some(100.0),
            'D' => Some(500.0),
            'M' => Some(1000.0),
            _ => None,
        })
        .collect::<Option<Vec<f64>>>()?;
    // A digit before a larger one subtracts (IX, MCM...).
    let x: f64 = vals.iter().enumerate().map(|(i, &v)| if vals.get(i + 1).is_some_and(|&w| w > v) { -v } else { v }).sum();
    Some(if neg { -x } else { x })
}

/// Most multiplications a matrix function makes (n³ for n×n): about a
/// tenth of a second, as a file's formulas all run when it opens.
const MAX_PRODUCTS: usize = 100_000_000;

/// An array or range of numbers (anything else is #VALUE!) with its size.
fn num_matrix(env: &Env, e: &Expr) -> R<(usize, usize, Vec<f64>)> {
    let t = env.tab(e)?;
    if t.len() > MAX_ARRAY {
        return Err(ErrorKind::Num);
    }
    let m = t.to_matrix()?;
    let data = m
        .data
        .iter()
        .map(|v| match v {
            Value::Number(x) => Ok(*x),
            Value::Error(k) => Err(*k),
            _ => Err(ErrorKind::Value),
        })
        .collect::<R<Vec<f64>>>()?;
    Ok((m.rows, m.cols, data))
}

/// LU decomposition with partial pivoting, like Excel (so a singular
/// matrix may give a tiny non-zero result through rounding).
fn determinant(n: usize, mut m: Vec<f64>) -> f64 {
    let mut det = 1.0;
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| m[i * n + c].abs().total_cmp(&m[j * n + c].abs())).unwrap_or(c);
        if m[p * n + c] == 0.0 {
            return 0.0;
        }
        if p != c {
            for k in 0..n {
                m.swap(p * n + k, c * n + k);
            }
            det = -det;
        }
        let piv = m[c * n + c];
        det *= piv;
        for r in c + 1..n {
            let factor = m[r * n + c] / piv;
            for k in c..n {
                m[r * n + k] -= factor * m[c * n + k];
            }
        }
    }
    det
}

/// Gauss-Jordan elimination; `None` for a singular matrix.
fn inverse(n: usize, mut m: Vec<f64>) -> Option<Vec<f64>> {
    let mut inv: Vec<f64> = (0..n * n).map(|i| if i / n == i % n { 1.0 } else { 0.0 }).collect();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| m[i * n + c].abs().total_cmp(&m[j * n + c].abs()))?;
        if m[p * n + c] == 0.0 {
            return None;
        }
        for k in 0..n {
            m.swap(p * n + k, c * n + k);
            inv.swap(p * n + k, c * n + k);
        }
        let piv = m[c * n + c];
        for k in 0..n {
            m[c * n + k] /= piv;
            inv[c * n + k] /= piv;
        }
        for r in (0..n).filter(|&r| r != c) {
            let factor = m[r * n + c];
            if factor != 0.0 {
                for k in 0..n {
                    m[r * n + k] -= factor * m[c * n + k];
                    inv[r * n + k] -= factor * inv[c * n + k];
                }
            }
        }
    }
    Some(inv)
}

/// Whether a cell holds a SUBTOTAL or AGGREGATE formula (AGGREGATE skips them).
fn nested(env: &Env, s: usize, c: CellRef) -> bool {
    env.ctx.is_subtotal(s, c)
        || env.ctx.formula_text(s, c).is_some_and(|t| {
            let t = t.to_uppercase();
            t.contains("AGGREGATE(") || t.contains("AGREGAR(")
        })
}

/// The values AGGREGATE works on, in reading order, and how many non-empty
/// values there are (for COUNTA). `opt` bits: 1 hidden rows, 2 errors;
/// options 0 to 3 also skip nested SUBTOTAL and AGGREGATE results.
fn agg_values(env: &Env, args: &[Expr], opt: u32) -> R<(Vec<f64>, usize)> {
    let (skip_hidden, skip_errors, skip_nested) = (opt & 1 != 0, opt & 2 != 0, opt < 4);
    let (mut nums, mut filled, mut err) = (Vec::new(), 0usize, None);
    let mut take = |v: &Value, direct: bool| match v {
        Value::Empty => {}
        Value::Error(k) => {
            if !skip_errors {
                err.get_or_insert(*k);
            }
        }
        Value::Number(x) => {
            filled += 1;
            nums.push(*x);
        }
        v => {
            filled += 1;
            if direct {
                match v.to_number() {
                    Ok(x) => nums.push(x),
                    Err(k) if !skip_errors => {
                        err.get_or_insert(k);
                    }
                    Err(_) => {}
                }
            }
        }
    };
    for e in args {
        match env.arg(e) {
            Arg::R(s, r) => {
                let mut cells = Vec::new();
                crate::functions::scan(env.ctx, s, &r, &mut |c, v| cells.push((c, v.clone())));
                cells.sort_by_key(|(c, _)| (c.row, c.col));
                for (c, v) in cells {
                    if (skip_hidden && env.ctx.row_hidden(s, c.row) != Hidden::No) || (skip_nested && !matches!(v, Value::Empty) && nested(env, s, c)) {
                        continue;
                    }
                    take(&v, false);
                }
            }
            Arg::A(m) => m.data.iter().for_each(|v| take(v, false)),
            Arg::V(v) => take(&v, true),
        }
    }
    match err {
        Some(k) => Err(k),
        None => Ok((nums, filled)),
    }
}

fn aggregate(func: u32, mut v: Vec<f64>, filled: usize, k: f64) -> R<Value> {
    use ErrorKind::{Div0, Num};
    let len = v.len() as f64;
    let sum: f64 = v.iter().sum();
    let var = |ddof: f64| -> R<f64> {
        if len - ddof < 1.0 {
            return Err(Div0);
        }
        let mean = sum / len;
        Ok(v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (len - ddof))
    };
    let r = match func {
        1 if v.is_empty() => return Err(Div0),
        1 => sum / len,
        2 => len,
        3 => filled as f64,
        4 => v.iter().cloned().reduce(f64::max).unwrap_or(0.0),
        5 => v.iter().cloned().reduce(f64::min).unwrap_or(0.0),
        6 if v.is_empty() => 0.0,
        6 => v.iter().product(),
        7 => var(1.0)?.sqrt(),
        8 => var(0.0)?.sqrt(),
        9 => sum,
        10 => var(1.0)?,
        11 => var(0.0)?,
        13 => mode(&v).ok_or(ErrorKind::Na)?,
        _ => {
            v.sort_by(f64::total_cmp);
            match func {
                12 => percentile(&v, 0.5, false)?,
                14 | 15 => {
                    let k = k.ceil();
                    if k < 1.0 || k > len {
                        return Err(Num);
                    }
                    v[if func == 15 { k as usize - 1 } else { v.len() - k as usize }]
                }
                16 | 18 => percentile(&v, k, func == 18)?,
                _ => {
                    let q = k.trunc();
                    let ok = if func == 17 { (0.0..=4.0).contains(&q) } else { (1.0..=3.0).contains(&q) };
                    if !ok {
                        return Err(Num);
                    }
                    percentile(&v, q / 4.0, func == 19)?
                }
            }
        }
    };
    Ok(num_ok(r))
}

/// PERCENTILE.INC, or PERCENTILE.EXC with `exc`, of sorted numbers.
fn percentile(v: &[f64], p: f64, exc: bool) -> R<f64> {
    let n = v.len() as f64;
    let pos = if exc { p * (n + 1.0) - 1.0 } else { p * (n - 1.0) };
    let bad = if exc { p <= 0.0 || p >= 1.0 || pos < 0.0 || pos > n - 1.0 } else { !(0.0..=1.0).contains(&p) };
    if v.is_empty() || bad || !pos.is_finite() {
        return Err(ErrorKind::Num);
    }
    let i = pos.floor() as usize;
    let frac = pos - pos.floor();
    Ok(match v.get(i + 1) {
        Some(next) if frac > 0.0 => v[i] + frac * (next - v[i]),
        _ => v[i],
    })
}

/// MODE.SNGL: the most repeated number, the first one seen on ties.
fn mode(v: &[f64]) -> Option<f64> {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for x in v {
        *counts.entry((x + 0.0).to_bits()).or_default() += 1;
    }
    let mut best: Option<(f64, usize)> = None;
    for &x in v {
        let c = counts[&(x + 0.0).to_bits()];
        if c > 1 && best.is_none_or(|(_, b)| c > b) {
            best = Some((x, c));
        }
    }
    best.map(|(x, _)| x)
}

#[cfg(test)]
mod tests {
    use crate::format::Locale;
    use crate::{CellRef, Value, Workbook};

    fn book(cells: &[(u32, u32, &str)]) -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        for &(r, c, t) in cells {
            wb.set_input(0, CellRef::new(r, c), t);
        }
        wb
    }

    fn num(wb: &Workbook, f: &str) -> f64 {
        match wb.eval_formula(0, f) {
            Value::Number(x) => x,
            v => panic!("{f} gave {v:?}"),
        }
    }

    fn close(wb: &Workbook, f: &str, want: f64) {
        let got = num(wb, f);
        assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{f}: got {got}, want {want}");
    }

    fn err(wb: &Workbook, f: &str) -> String {
        match wb.eval_formula(0, f) {
            Value::Error(k) => k.as_str().to_string(),
            v => panic!("{f} gave {v:?}"),
        }
    }

    fn text(wb: &Workbook, f: &str) -> String {
        match wb.eval_formula(0, f) {
            Value::Text(s) => s.to_string(),
            v => panic!("{f} gave {v:?}"),
        }
    }

    fn array(wb: &Workbook, f: &str) -> (usize, usize, Vec<f64>) {
        match wb.eval_formula(0, f) {
            Value::Array(m) => (m.rows, m.cols, m.data.iter().map(|v| v.as_number().unwrap_or(f64::NAN)).collect()),
            v => panic!("{f} gave {v:?}"),
        }
    }

    #[test]
    fn sums_of_squares() {
        let wb = book(&[(0, 0, "2"), (1, 0, "3"), (2, 0, "9"), (0, 1, "6"), (1, 1, "5"), (2, 1, "11"), (3, 0, "x")]);
        close(&wb, "=SUMSQ(3,4)", 25.0);
        close(&wb, "=SUMSQ(A1:A4)", 94.0);
        close(&wb, "=SUMX2MY2({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", -55.0);
        close(&wb, "=SUMX2PY2({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", 521.0);
        close(&wb, "=SUMXMY2({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", 79.0);
        close(&wb, "=SUMXMY2(A1:A3,B1:B3)", 24.0);
        assert_eq!(err(&wb, "=SUMXMY2({1,2},{1,2,3})"), "#N/A");
    }

    #[test]
    fn integers_and_counting() {
        let wb = book(&[]);
        close(&wb, "=GCD(5,2)", 1.0);
        close(&wb, "=GCD(24,36)", 12.0);
        close(&wb, "=GCD(7.9,1)", 1.0);
        close(&wb, "=GCD(5,0)", 5.0);
        assert_eq!(err(&wb, "=GCD(-1,2)"), "#NUM!");
        close(&wb, "=LCM(5,2)", 10.0);
        close(&wb, "=LCM(24,36)", 72.0);
        close(&wb, "=LCM({3,4},6)", 12.0);
        close(&wb, "=FACT(5)", 120.0);
        close(&wb, "=FACT(1.9)", 1.0);
        close(&wb, "=FACT(0)", 1.0);
        assert_eq!(err(&wb, "=FACT(-1)"), "#NUM!");
        assert_eq!(err(&wb, "=FACT(171)"), "#NUM!");
        close(&wb, "=FACTDOUBLE(6)", 48.0);
        close(&wb, "=FACTDOUBLE(7)", 105.0);
        close(&wb, "=COMBIN(8,2)", 28.0);
        close(&wb, "=COMBIN(60,30)", 118264581564861424.0);
        assert_eq!(err(&wb, "=COMBIN(2,3)"), "#NUM!");
        assert_eq!(err(&wb, "=COMBIN(1E+300,5E+299)"), "#NUM!");
        close(&wb, "=COMBINA(4,3)", 20.0);
        close(&wb, "=COMBINA(10,3)", 220.0);
        close(&wb, "=PERMUT(100,3)", 970200.0);
        close(&wb, "=PERMUT(3,2)", 6.0);
        assert_eq!(err(&wb, "=PERMUT(1E+9,1E+9)"), "#NUM!");
        close(&wb, "=PERMUTATIONA(3,2)", 9.0);
        close(&wb, "=PERMUTATIONA(2,2)", 4.0);
        close(&wb, "=MULTINOMIAL(2,3,4)", 1260.0);
        assert_eq!(err(&wb, "=MULTINOMIAL(1,-1)"), "#NUM!");
    }

    #[test]
    fn trigonometry() {
        let wb = book(&[]);
        close(&wb, "=SIN(PI()/2)", 1.0);
        close(&wb, "=SIN(RADIANS(30))", 0.5);
        close(&wb, "=COS(1.047)", 0.500171075);
        close(&wb, "=TAN(0.785)", 0.99920399);
        close(&wb, "=ASIN(-0.5)", -0.523598776);
        close(&wb, "=ACOS(-0.5)", 2.094395102);
        close(&wb, "=ATAN(1)", 0.785398163);
        close(&wb, "=ATAN2(1,1)", 0.785398163);
        close(&wb, "=ATAN2(-1,-1)", -2.35619449);
        assert_eq!(err(&wb, "=ATAN2(0,0)"), "#DIV/0!");
        close(&wb, "=SINH(1)", 1.175201194);
        close(&wb, "=COSH(4)", 27.30823284);
        close(&wb, "=TANH(-2)", -0.96402758);
        close(&wb, "=ASINH(-2.5)", -1.647231146);
        close(&wb, "=ACOSH(10)", 2.993222846);
        close(&wb, "=ATANH(0.761594155955765)", 1.0);
        close(&wb, "=COT(30)", -0.156119952);
        close(&wb, "=COTH(2)", 1.037314721);
        close(&wb, "=CSC(15)", 1.537780562);
        close(&wb, "=CSCH(1.5)", 0.469642441);
        close(&wb, "=SEC(45)", 1.903594407);
        close(&wb, "=SECH(45)", 5.72504e-20);
        close(&wb, "=ACOT(2)", 0.463647609);
        close(&wb, "=ACOTH(6)", 0.168236118);
        close(&wb, "=DEGREES(PI())", 180.0);
        close(&wb, "=RADIANS(270)", 4.71238898);
        assert_eq!(err(&wb, "=ASIN(2)"), "#NUM!");
        assert_eq!(err(&wb, "=ACOSH(0.5)"), "#NUM!");
        assert_eq!(err(&wb, "=ATANH(1)"), "#NUM!");
        assert_eq!(err(&wb, "=ACOTH(0.5)"), "#NUM!");
        assert_eq!(err(&wb, "=COT(0)"), "#DIV/0!");
        assert_eq!(err(&wb, "=CSC(0)"), "#DIV/0!");
        assert_eq!(err(&wb, "=SIN(2^27)"), "#NUM!");
        assert_eq!(err(&wb, "=SINH(1000)"), "#NUM!");
        assert_eq!(err(&wb, "=SIN()"), "#VALUE!");
        // Single-value functions repeat over arrays.
        assert_eq!(array(&wb, "=DEGREES({0,3.14159265358979})").2.len(), 2);
    }

    #[test]
    fn numerals_and_bases() {
        let wb = book(&[]);
        for (form, want) in [(0, "CDXCIX"), (1, "LDVLIV"), (2, "XDIX"), (3, "VDIV"), (4, "ID")] {
            assert_eq!(text(&wb, &format!("=ROMAN(499,{form})")), want);
        }
        assert_eq!(text(&wb, "=ROMAN(499)"), "CDXCIX");
        assert_eq!(text(&wb, "=ROMAN(499,TRUE)"), "CDXCIX");
        assert_eq!(text(&wb, "=ROMAN(499,FALSE)"), "ID");
        assert_eq!(text(&wb, "=ROMAN(1999)"), "MCMXCIX");
        assert_eq!(text(&wb, "=ROMAN(3999)"), "MMMCMXCIX");
        assert_eq!(text(&wb, "=ROMAN(0)"), "");
        assert_eq!(err(&wb, "=ROMAN(4000)"), "#VALUE!");
        assert_eq!(err(&wb, "=ROMAN(-1)"), "#VALUE!");
        assert_eq!(err(&wb, "=ROMAN(10,5)"), "#VALUE!");
        close(&wb, "=ARABIC(\"LVII\")", 57.0);
        close(&wb, "=ARABIC(\"mcmxii\")", 1912.0);
        close(&wb, "=ARABIC(\"-MMXI\")", -2011.0);
        close(&wb, "=ARABIC(\"\")", 0.0);
        close(&wb, "=ARABIC(ROMAN(1999,4))", 1999.0);
        assert_eq!(err(&wb, "=ARABIC(\"ABC\")"), "#VALUE!");
        assert_eq!(text(&wb, "=BASE(7,2)"), "111");
        assert_eq!(text(&wb, "=BASE(100,16)"), "64");
        assert_eq!(text(&wb, "=BASE(15,2,10)"), "0000001111");
        assert_eq!(text(&wb, "=BASE(0,2)"), "0");
        assert_eq!(err(&wb, "=BASE(-1,2)"), "#NUM!");
        assert_eq!(err(&wb, "=BASE(10,37)"), "#NUM!");
        close(&wb, "=DECIMAL(\"FF\",16)", 255.0);
        close(&wb, "=DECIMAL(111,2)", 7.0);
        close(&wb, "=DECIMAL(\"zap\",36)", 45745.0);
        assert_eq!(err(&wb, "=DECIMAL(\"12\",2)"), "#NUM!");
    }

    #[test]
    fn rounding() {
        let wb = book(&[]);
        close(&wb, "=MROUND(10,3)", 9.0);
        close(&wb, "=MROUND(-10,-3)", -9.0);
        close(&wb, "=MROUND(1.3,0.2)", 1.4);
        close(&wb, "=MROUND(5,0)", 0.0);
        assert_eq!(err(&wb, "=MROUND(5,-2)"), "#NUM!");
        close(&wb, "=EVEN(1.5)", 2.0);
        close(&wb, "=EVEN(3)", 4.0);
        close(&wb, "=EVEN(2)", 2.0);
        close(&wb, "=EVEN(-1)", -2.0);
        close(&wb, "=EVEN(0)", 0.0);
        close(&wb, "=ODD(1.5)", 3.0);
        close(&wb, "=ODD(3)", 3.0);
        close(&wb, "=ODD(2)", 3.0);
        close(&wb, "=ODD(-1)", -1.0);
        close(&wb, "=ODD(-2)", -3.0);
        close(&wb, "=ODD(0)", 1.0);
        close(&wb, "=QUOTIENT(5,2)", 2.0);
        close(&wb, "=QUOTIENT(4.5,3.1)", 1.0);
        close(&wb, "=QUOTIENT(-10,3)", -3.0);
        assert_eq!(err(&wb, "=QUOTIENT(1,0)"), "#DIV/0!");
        close(&wb, "=SQRTPI(1)", 1.772453851);
        close(&wb, "=SQRTPI(2)", 2.506628275);
        assert_eq!(err(&wb, "=SQRTPI(-1)"), "#NUM!");
        close(&wb, "=CEILING.MATH(24.3,5)", 25.0);
        close(&wb, "=CEILING.MATH(6.7)", 7.0);
        close(&wb, "=CEILING.MATH(-8.1,2)", -8.0);
        close(&wb, "=CEILING.MATH(-5.5,2,-1)", -6.0);
        close(&wb, "=CEILING.MATH(0.3,0.1)", 0.3);
        close(&wb, "=FLOOR.MATH(24.3,5)", 20.0);
        close(&wb, "=FLOOR.MATH(6.7)", 6.0);
        close(&wb, "=FLOOR.MATH(-8.1,2)", -10.0);
        close(&wb, "=FLOOR.MATH(-5.5,2,-1)", -4.0);
        close(&wb, "=CEILING.PRECISE(4.3)", 5.0);
        close(&wb, "=CEILING.PRECISE(-4.3)", -4.0);
        close(&wb, "=CEILING.PRECISE(4.3,2)", 6.0);
        close(&wb, "=CEILING.PRECISE(4.3,-2)", 6.0);
        close(&wb, "=CEILING.PRECISE(-4.3,2)", -4.0);
        close(&wb, "=ISO.CEILING(-4.3,-2)", -4.0);
        close(&wb, "=FLOOR.PRECISE(-3.2,-1)", -4.0);
        close(&wb, "=FLOOR.PRECISE(3.2,1)", 3.0);
        close(&wb, "=FLOOR.PRECISE(-3.2,1)", -4.0);
        close(&wb, "=FLOOR.PRECISE(3.2,-1)", 3.0);
        close(&wb, "=FLOOR.PRECISE(3.2)", 3.0);
        close(&wb, "=FLOOR.PRECISE(5,0)", 0.0);
    }

    #[test]
    fn aggregate() {
        // Microsoft's example data, with an error and a nested SUBTOTAL.
        let data = ["=1/0", "72", "30", "=SQRT(-1)", "31", "96", "32", "81", "33", "53", "34"];
        let col_b = ["82", "65", "95", "63", "53", "71", "55", "83", "100", "91", "89"];
        let mut cells: Vec<(u32, u32, &str)> = Vec::new();
        for (i, (x, y)) in data.iter().zip(col_b.iter()).enumerate() {
            cells.push((i as u32, 0, x));
            cells.push((i as u32, 1, y));
        }
        let mut wb = book(&cells);
        close(&wb, "=AGGREGATE(4,6,A1:A11)", 96.0);
        close(&wb, "=AGGREGATE(14,6,A1:A11,3)", 72.0);
        assert_eq!(err(&wb, "=AGGREGATE(15,6,A1:A11)"), "#VALUE!");
        close(&wb, "=AGGREGATE(12,6,A1:A11,B1:B11)", 68.0);
        assert_eq!(err(&wb, "=AGGREGATE(9,4,A1:A11)"), "#DIV/0!");
        assert_eq!(err(&wb, "=AGGREGATE(20,6,A1:A11)"), "#VALUE!");
        assert_eq!(err(&wb, "=AGGREGATE(9,8,A1:A11)"), "#VALUE!");
        // Every function over B1:B11, checked against Excel.
        let want = [
            (1, 77.0),
            (2, 11.0),
            (3, 11.0),
            (4, 100.0),
            (5, 53.0),
            (6, 443810694208907025000.0),
            (7, 16.401219466856727),
            (8, 15.637949179654427),
            (9, 847.0),
            (10, 269.0),
            (11, 244.54545454545453),
            (12, 82.0),
        ];
        for (func, w) in want {
            close(&wb, &format!("=AGGREGATE({func},0,B1:B11)"), w);
        }
        close(&wb, "=AGGREGATE(14,0,B1:B11,2)", 95.0);
        close(&wb, "=AGGREGATE(15,0,B1:B11,2)", 55.0);
        close(&wb, "=AGGREGATE(16,0,B1:B11,0.3)", 65.0);
        close(&wb, "=AGGREGATE(17,0,B1:B11,1)", 64.0);
        close(&wb, "=AGGREGATE(18,0,B1:B11,0.3)", 64.2);
        close(&wb, "=AGGREGATE(19,0,B1:B11,3)", 91.0);
        assert_eq!(err(&wb, "=AGGREGATE(13,0,B1:B11)"), "#N/A");
        close(&wb, "=AGGREGATE(13,0,{1,2,2,3,3})", 2.0);
        assert_eq!(err(&wb, "=AGGREGATE(17,0,B1:B11,5)"), "#NUM!");
        // Array form with errors produced inside the formula.
        close(&wb, "=AGGREGATE(14,6,B1:B11/(B1:B11>90),1)", 100.0);
        close(&wb, "=AGGREGATE(15,6,B1:B11/(B1:B11>90),1)", 91.0);
        // Hidden rows count unless the option skips them.
        wb.sheets[0].row_heights.insert(8, 0.0);
        close(&wb, "=AGGREGATE(4,5,B1:B11)", 95.0);
        close(&wb, "=AGGREGATE(4,4,B1:B11)", 100.0);
        close(&wb, "=AGGREGATE(2,7,A1:A11)", 8.0);
        // Nested SUBTOTAL and AGGREGATE results are skipped by options 0-3.
        wb.set_input(0, CellRef::new(11, 1), "=SUBTOTAL(9,B1:B2)");
        wb.set_input(0, CellRef::new(12, 1), "=AGGREGATE(9,0,B1:B2)");
        close(&wb, "=AGGREGATE(9,0,B1:B13)", 847.0);
        close(&wb, "=AGGREGATE(9,4,B1:B13)", 847.0 + 2.0 * 147.0);
    }

    #[test]
    fn matrices() {
        let wb = book(&[]);
        let (r, c, d) = array(&wb, "=MMULT({1,3;7,2},{2,0;0,2})");
        assert_eq!((r, c), (2, 2));
        assert_eq!(d, vec![2.0, 6.0, 14.0, 4.0]);
        close(&wb, "=MMULT({1,2,3},{4;5;6})", 32.0);
        assert_eq!(err(&wb, "=MMULT({1,2},{1,2})"), "#VALUE!");
        assert_eq!(err(&wb, "=MMULT({1,\"a\"},{1;2})"), "#VALUE!");
        let (_, _, d) = array(&wb, "=MINVERSE({4,-1;2,0})");
        for (g, w) in d.iter().zip([0.0, 0.5, -1.0, 2.0]) {
            assert!((g - w).abs() < 1e-12);
        }
        let (_, _, d) = array(&wb, "=MINVERSE({1,2,1;3,4,-1;0,2,0})");
        for (g, w) in d.iter().zip([0.25, 0.25, -0.75, 0.0, 0.0, 0.5, 0.75, -0.25, -0.25]) {
            assert!((g - w).abs() < 1e-12);
        }
        assert_eq!(err(&wb, "=MINVERSE({1,2;2,4})"), "#NUM!");
        assert_eq!(err(&wb, "=MMULT(SEQUENCE(2000,2000),SEQUENCE(2000,2000))"), "#NUM!");
        assert_eq!(err(&wb, "=MDETERM(SEQUENCE(2000,2000))"), "#NUM!");
        assert_eq!(err(&wb, "=MINVERSE({1,2,3})"), "#VALUE!");
        close(&wb, "=MDETERM({1,3,8,5;1,3,6,1;1,1,1,0;7,3,10,2})", 88.0);
        close(&wb, "=MDETERM({3,6,1;1,1,0;3,10,2})", 1.0);
        close(&wb, "=MDETERM({3,6;1,1})", -3.0);
        close(&wb, "=MDETERM({1,2;2,4})", 0.0);
        let (r, c, d) = array(&wb, "=MUNIT(3)");
        assert_eq!((r, c), (3, 3));
        assert_eq!(d, vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        close(&wb, "=MUNIT(1)", 1.0);
        assert_eq!(err(&wb, "=MUNIT(0)"), "#VALUE!");
        assert_eq!(err(&wb, "=MUNIT(1E+10)"), "#NUM!");
        close(&wb, "=SUM(MMULT(MUNIT(2),{1,2;3,4}))", 10.0);
    }

    #[test]
    fn series() {
        let wb = book(&[(0, 0, "1"), (1, 0, "-0.5"), (2, 0, "0.041666667"), (3, 0, "-0.001388889")]);
        close(&wb, "=SERIESSUM(PI()/4,0,2,A1:A4)", 0.707103215);
        close(&wb, "=SERIESSUM(2,1,1,{1,1,1})", 14.0);
        assert_eq!(err(&wb, "=SERIESSUM(2,1,1,{1,\"a\"})"), "#VALUE!");
    }
}
