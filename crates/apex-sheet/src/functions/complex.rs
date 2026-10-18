//! Spreadsheet functions: complex numbers and Bessel functions.
//!
//! Complex numbers are text such as "3+4i" or "-2.5j", always with "." as
//! the decimal separator. The formulas follow LibreOffice's (as Excel).

use std::f64::consts::{FRAC_2_PI, LOG2_E, LOG10_E, PI};

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    match f {
        ImSum | ImProduct => Some(ALL),
        _ => None,
    }
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    Some(match f {
        Complex => complex(env, a),
        ImAbs | Imaginary | ImArgument | ImReal => part(env, f, a),
        ImConjugate | ImCos | ImCosh | ImCot | ImCsc | ImCsch | ImExp | ImLn | ImLog10 | ImLog2 | ImSec | ImSech | ImSin | ImSinh | ImSqrt | ImTan => unary(env, f, a),
        ImDiv | ImSub | ImPower => binary(env, f, a),
        ImSum | ImProduct => fold(env, f, a),
        BesselI | BesselJ | BesselK | BesselY => bessel(env, f, a),
        _ => return None,
    })
}

fn need(a: &[Expr], min: usize, max: usize) -> R<()> {
    if a.len() < min || a.len() > max { Err(ErrorKind::Value) } else { Ok(()) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct C {
    r: f64,
    i: f64,
}

impl C {
    fn abs(self) -> f64 {
        self.r.hypot(self.i)
    }

    fn is_zero(self) -> bool {
        self.r == 0.0 && self.i == 0.0
    }

    fn mul(self, z: C) -> C {
        C { r: self.r * z.r - self.i * z.i, i: self.r * z.i + self.i * z.r }
    }

    fn div(self, z: C) -> R<C> {
        if z.is_zero() {
            return Err(ErrorKind::Num);
        }
        let f = 1.0 / (z.r * z.r + z.i * z.i);
        Ok(C { r: (self.r * z.r + self.i * z.i) * f, i: (z.r * self.i - self.r * z.i) * f })
    }

    fn ln(self) -> R<C> {
        if self.is_zero() {
            return Err(ErrorKind::Num);
        }
        Ok(C { r: self.abs().ln(), i: self.i.atan2(self.r) })
    }

    fn pow(self, n: f64) -> R<C> {
        if self.is_zero() {
            return if n > 0.0 { Ok(self) } else { Err(ErrorKind::Num) };
        }
        let phi = self.i.atan2(self.r) * n;
        let p = self.abs().powf(n);
        Ok(C { r: phi.cos() * p, i: phi.sin() * p })
    }
}

/// Parses "a", "bi", "a+bi", "i", "-j"...: the number and its suffix.
fn parse(s: &str) -> Option<(C, Option<char>)> {
    let b = s.as_bytes();
    if b.is_empty() {
        return Some((C { r: 0.0, i: 0.0 }, None));
    }
    // A signed number from `p`: (value if any digits, sign, end).
    let number = |p: usize| -> (Option<f64>, f64, usize) {
        let mut e = p;
        let sign = match b.get(e) {
            Some(b'+') => {
                e += 1;
                1.0
            }
            Some(b'-') => {
                e += 1;
                -1.0
            }
            _ => 1.0,
        };
        let start = e;
        let mut digits = false;
        while e < b.len() && b[e].is_ascii_digit() {
            e += 1;
            digits = true;
        }
        if b.get(e) == Some(&b'.') {
            e += 1;
            while e < b.len() && b[e].is_ascii_digit() {
                e += 1;
                digits = true;
            }
        }
        if !digits {
            return (None, sign, start);
        }
        if matches!(b.get(e), Some(b'e' | b'E')) {
            let mut x = e + 1;
            if matches!(b.get(x), Some(b'+' | b'-')) {
                x += 1;
            }
            if b.get(x).is_some_and(u8::is_ascii_digit) {
                while x < b.len() && b[x].is_ascii_digit() {
                    x += 1;
                }
                e = x;
            }
        }
        (s[p..e].parse().ok(), sign, e)
    };
    let suffix = |p: usize| match &b[p..] {
        [c @ (b'i' | b'j')] => Some(*c as char),
        _ => None,
    };
    let (x, sign, e) = number(0);
    if e == b.len() {
        return Some((C { r: x?, i: 0.0 }, None));
    }
    if let Some(c) = suffix(e) {
        return Some((C { r: 0.0, i: x.unwrap_or(sign) }, Some(c)));
    }
    let x = x?;
    if !matches!(b[e], b'+' | b'-') {
        return None;
    }
    let (y, sign, e) = number(e);
    Some((C { r: x, i: y.unwrap_or(sign) }, Some(suffix(e)?)))
}

/// A complex argument: a number or complex text.
fn from_value(v: &Value) -> R<(C, Option<char>)> {
    match v {
        Value::Number(n) => Ok((C { r: *n, i: 0.0 }, None)),
        Value::Empty => Ok((C { r: 0.0, i: 0.0 }, None)),
        Value::Text(t) => parse(t).ok_or(ErrorKind::Num),
        Value::Error(k) => Err(*k),
        _ => Err(ErrorKind::Value),
    }
}

fn arg(env: &Env, e: &Expr) -> R<(C, Option<char>)> {
    from_value(&env.scalar(e))
}

/// The suffix of a result: arguments mixing "i" and "j" are an error.
fn join(a: Option<char>, b: Option<char>) -> R<Option<char>> {
    match (a, b) {
        (Some(x), Some(y)) if x != y => Err(ErrorKind::Value),
        _ => Ok(a.or(b)),
    }
}

/// Up to 15 significant digits, with an exponent outside 1E-5..1E15
/// (C's "%.15g", as Excel and LibreOffice write them).
fn fmt(x: f64) -> String {
    let x = x + 0.0;
    if x == 0.0 {
        return "0".into();
    }
    let trim = |s: String| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    let s = format!("{x:.14e}");
    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
    let e: i32 = e.parse().unwrap_or(0);
    if (-4..15).contains(&e) {
        trim(format!("{:.*}", (14 - e) as usize, x))
    } else {
        format!("{}E{}{:02}", trim(m.to_string()), if e < 0 { '-' } else { '+' }, e.abs())
    }
}

fn text(z: C, sfx: Option<char>) -> R<Value> {
    if !z.r.is_finite() || !z.i.is_finite() {
        return Err(ErrorKind::Num);
    }
    let mut s = String::new();
    if z.i == 0.0 || z.r != 0.0 {
        s.push_str(&fmt(z.r));
    }
    if z.i != 0.0 {
        let im = fmt(z.i);
        let im = match im.as_str() {
            "1" => "",
            "-1" => "-",
            _ => &im,
        };
        if z.r != 0.0 && !im.starts_with('-') {
            s.push('+');
        }
        s.push_str(im);
        s.push(sfx.unwrap_or('i'));
    }
    Ok(Value::text(s))
}

fn complex(env: &Env, a: &[Expr]) -> R<Value> {
    need(a, 2, 3)?;
    let (r, i) = (env.num(&a[0])?, env.num(&a[1])?);
    let sfx = match a.get(2) {
        None | Some(Expr::Missing) => 'i',
        Some(e) => match &*env.text(e)? {
            "" | "i" => 'i',
            "j" => 'j',
            _ => return Err(ErrorKind::Value),
        },
    };
    text(C { r, i }, Some(sfx))
}

fn part(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    need(a, 1, 1)?;
    let (z, _) = arg(env, &a[0])?;
    Ok(num_ok(match f {
        Func::ImAbs => z.abs(),
        Func::Imaginary => z.i,
        Func::ImReal => z.r,
        _ => {
            if z.is_zero() {
                return Err(ErrorKind::Div0);
            }
            z.i.atan2(z.r)
        }
    }))
}

fn unary(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use Func::*;
    need(a, 1, 1)?;
    let (z, sfx) = arg(env, &a[0])?;
    let C { r, i } = z;
    let c = |r: f64, i: f64| C { r, i };
    // Real arguments use the real function (as LibreOffice does).
    let real = i == 0.0;
    let nonzero = |z: C| if z.is_zero() { Err(ErrorKind::Num) } else { Ok(()) };
    // (a + bi) * s, where s underflows (and a or b overflow) far away.
    let scaled = |s: f64, a: f64, b: f64| if s == 0.0 { c(0.0, 0.0) } else { c(a * s, b * s) };
    let z = match f {
        ImConjugate => c(r, -i),
        ImExp => c(r.exp() * i.cos(), r.exp() * i.sin()),
        ImLn => z.ln()?,
        ImLog10 => {
            let l = z.ln()?;
            c(l.r * LOG10_E, l.i * LOG10_E)
        }
        ImLog2 => {
            let l = z.ln()?;
            c(l.r * LOG2_E, l.i * LOG2_E)
        }
        ImSqrt if z.is_zero() => z,
        ImSqrt => {
            // The larger part first, the other from it (no cancellation).
            let t = ((z.abs() + r.abs()) / 2.0).sqrt();
            if r >= 0.0 { c(t, i / (2.0 * t)) } else { c(i.abs() / (2.0 * t), t.copysign(i)) }
        }
        ImSin => c(r.sin() * i.cosh(), r.cos() * i.sinh()),
        ImCos => c(r.cos() * i.cosh(), -(r.sin() * i.sinh())),
        ImSinh => c(r.sinh() * i.cos(), r.cosh() * i.sin()),
        ImCosh => c(r.cosh() * i.cos(), r.sinh() * i.sin()),
        ImTan if real => c(r.tan(), 0.0),
        ImTan => {
            let s = 1.0 / ((2.0 * r).cos() + (2.0 * i).cosh());
            c((2.0 * r).sin() * s, (2.0 * i).sinh() * s)
        }
        ImCot if real => {
            nonzero(z)?;
            c(1.0 / r.tan(), 0.0)
        }
        ImCot => {
            let s = 1.0 / ((2.0 * i).cosh() - (2.0 * r).cos());
            c((2.0 * r).sin() * s, -((2.0 * i).sinh() * s))
        }
        ImSec if real => c(1.0 / r.cos(), 0.0),
        ImSec => {
            let s = 1.0 / ((2.0 * i).cosh() + (2.0 * r).cos());
            scaled(s, 2.0 * r.cos() * i.cosh(), 2.0 * r.sin() * i.sinh())
        }
        ImCsc if real => {
            nonzero(z)?;
            c(1.0 / r.sin(), 0.0)
        }
        ImCsc => {
            let s = 1.0 / ((2.0 * i).cosh() - (2.0 * r).cos());
            scaled(s, 2.0 * r.sin() * i.cosh(), -2.0 * r.cos() * i.sinh())
        }
        ImSech if real => c(1.0 / r.cosh(), 0.0),
        ImSech => {
            let s = 1.0 / ((2.0 * r).cosh() + (2.0 * i).cos());
            scaled(s, 2.0 * r.cosh() * i.cos(), -2.0 * r.sinh() * i.sin())
        }
        ImCsch if real => {
            nonzero(z)?;
            c(1.0 / r.sinh(), 0.0)
        }
        _ => {
            let s = 1.0 / ((2.0 * r).cosh() - (2.0 * i).cos());
            scaled(s, 2.0 * r.sinh() * i.cos(), -2.0 * r.cosh() * i.sin())
        }
    };
    text(z, sfx)
}

fn binary(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    need(a, 2, 2)?;
    let (x, sx) = arg(env, &a[0])?;
    if f == Func::ImPower {
        return text(x.pow(env.num(&a[1])?)?, sx);
    }
    let (y, sy) = arg(env, &a[1])?;
    let sfx = join(sx, sy)?;
    let z = if f == Func::ImDiv { x.div(y)? } else { C { r: x.r - y.r, i: x.i - y.i } };
    text(z, sfx)
}

/// IMSUM and IMPRODUCT over all the values of their arguments (empty
/// cells skipped).
fn fold(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    if a.is_empty() {
        return Err(ErrorKind::Value);
    }
    let mut acc: Option<C> = None;
    let mut sfx = None;
    for e in a {
        let mut res = Ok(());
        env.each(e, &mut |v, _| {
            if res.is_err() || matches!(v, Value::Empty) {
                return;
            }
            res = from_value(v).and_then(|(z, s)| {
                sfx = join(sfx, s)?;
                acc = Some(match acc {
                    None => z,
                    Some(p) if f == Func::ImSum => C { r: p.r + z.r, i: p.i + z.i },
                    Some(p) => p.mul(z),
                });
                Ok(())
            });
        });
        res?;
    }
    text(acc.unwrap_or(C { r: 0.0, i: 0.0 }), sfx)
}

// ---------------------------------------------------------------------------
// Bessel functions

fn bessel(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    need(a, 2, 2)?;
    let (x, n) = (env.num(&a[0])?, env.num(&a[1])?.trunc());
    if n < 0.0 || ((f == Func::BesselK || f == Func::BesselY) && x <= 0.0) {
        return Err(ErrorKind::Num);
    }
    // Beyond this the answer is 0 or overflows anyway.
    let n = n.min(1e6) as u32;
    Ok(num_ok(match f {
        Func::BesselI => bessel_i(x, n),
        Func::BesselJ => bessel_j(x, n),
        Func::BesselK => bessel_k(x, n),
        _ => bessel_y(x, n),
    }))
}

/// I_n(x) from its power series.
fn bessel_i(x: f64, n: u32) -> f64 {
    let h = x / 2.0;
    let mut t = 1.0;
    for k in 1..=n {
        t = t / k as f64 * h;
    }
    let mut s = t;
    let mut k = 1.0;
    while t != 0.0 && k < 2000.0 {
        t = t * h / k * h / (k + n as f64);
        s += t;
        if t.abs() <= s.abs() * 1e-15 {
            break;
        }
        k += 1.0;
    }
    s
}

/// J_0..J_m of x > 0 by Miller's backward recurrence: returns J_n, J_0,
/// J_1 and the sums of the Neumann series for Y_0 and Y_1:
/// Σ (-1)^k J_2k / k and Σ (-1)^k (2k+1) / (k (k+1)) J_2k+1.
fn miller(x: f64, n: u32) -> (f64, f64, f64, f64, f64) {
    let top = (n as f64).max(x);
    let m = ((top + 30.0 + (160.0 * top).sqrt()) as u64 / 2 * 2).max(2);
    let (mut next, mut cur) = (0.0, 1e-300);
    let (mut jn, mut j1, mut norm, mut s0, mut s1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut k = m;
    while k > 0 {
        // cur = j_k: fold it in, then step down to j_{k-1}.
        if k == n as u64 {
            jn = cur;
        }
        if k == 1 {
            j1 = cur;
        }
        let h = k / 2;
        let sign = if h % 2 == 0 { 1.0 } else { -1.0 };
        if k % 2 == 0 {
            norm += 2.0 * cur;
            s0 += sign * cur / h as f64;
        } else if h > 0 {
            s1 += sign * (2 * h + 1) as f64 / (h * (h + 1)) as f64 * cur;
        }
        let prev = 2.0 * k as f64 / x * cur - next;
        next = cur;
        cur = prev;
        k -= 1;
        if cur.abs() > 1e250 {
            for v in [&mut next, &mut cur, &mut jn, &mut j1, &mut norm, &mut s0, &mut s1] {
                *v *= 1e-250;
            }
        }
    }
    let j0 = cur;
    if n == 0 {
        jn = j0;
    }
    let norm = norm + j0;
    (jn / norm, j0 / norm, j1 / norm, s0 / norm, s1 / norm)
}

fn bessel_j(x: f64, n: u32) -> f64 {
    if x == 0.0 {
        return if n == 0 { 1.0 } else { 0.0 };
    }
    let sign = if x < 0.0 && n % 2 == 1 { -1.0 } else { 1.0 };
    let x = x.abs();
    if x > 1e7 {
        return sign * (FRAC_2_PI / x).sqrt() * (x - n as f64 * PI / 2.0 - PI / 4.0).cos();
    }
    sign * miller(x, n).0
}

fn bessel_y(x: f64, n: u32) -> f64 {
    let (y0, y1) = if x > 1e7 {
        let s = (FRAC_2_PI / x).sqrt();
        (s * (x - PI / 4.0).sin(), -s * (x - PI / 4.0).cos())
    } else {
        let (_, j0, j1, s0, s1) = miller(x, 0);
        let l = (x / 2.0).ln() + 0.577_215_664_901_532_9;
        (FRAC_2_PI * (l * j0 - 2.0 * s0), FRAC_2_PI * (-j0 / x + (l - 1.0) * j1 - s1))
    };
    recur(x, n, y0, y1, -1.0)
}

/// Upward recurrence from orders 0 and 1: f_k+1 = 2k/x f_k + sign f_k-1.
fn recur(x: f64, n: u32, f0: f64, f1: f64, sign: f64) -> f64 {
    if n == 0 {
        return f0;
    }
    let (mut a, mut b) = (f0, f1);
    for k in 1..n {
        (a, b) = (b, 2.0 * k as f64 / x * b + sign * a);
        if !b.is_finite() {
            break;
        }
    }
    b
}

/// K_n(x) from the polynomial approximations of Abramowitz and Stegun
/// 9.8.5-9.8.8 (as LibreOffice and Excel).
fn bessel_k(x: f64, n: u32) -> f64 {
    let poly = |y: f64, c: &[f64]| c.iter().rev().fold(0.0, |s, c| s * y + c);
    let (k0, k1) = if x <= 2.0 {
        let y = x * x / 4.0;
        let l = (x / 2.0).ln();
        (
            -l * bessel_i(x, 0) + poly(y, &[-0.577_215_66, 0.422_784_20, 0.230_697_56, 0.348_859_0e-1, 0.262_698e-2, 0.107_50e-3, 0.74e-5]),
            l * bessel_i(x, 1) + poly(y, &[1.0, 0.154_431_44, -0.672_785_79, -0.181_568_97, -0.191_940_2e-1, -0.110_404e-2, -0.468_6e-4]) / x,
        )
    } else {
        let y = 2.0 / x;
        let e = (-x).exp() / x.sqrt();
        (
            e * poly(y, &[1.253_314_14, -0.783_235_8e-1, 0.218_956_8e-1, -0.106_244_6e-1, 0.587_872e-2, -0.251_540e-2, 0.532_08e-3]),
            e * poly(y, &[1.253_314_14, 0.234_986_19, -0.365_562_0e-1, 0.150_426_8e-1, -0.780_353e-2, 0.325_614e-2, -0.682_45e-3]),
        )
    };
    recur(x, n, k0, k1, 1.0)
}

#[cfg(test)]
mod tests {
    use crate::{ErrorKind, Locale, Value, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        w
    }

    fn text(w: &Workbook, f: &str, want: &str) {
        assert_eq!(w.eval_formula(0, f), Value::text(want), "{f}");
    }

    /// Checks against LibreOffice's result, to `tol` relative.
    fn close(w: &Workbook, f: &str, want: f64, tol: f64) {
        match w.eval_formula(0, f) {
            Value::Number(got) => assert!((got - want).abs() <= tol * want.abs().max(1e-300), "{f}: {got} != {want}"),
            v => panic!("{f} gave {v:?}"),
        }
    }

    fn err(w: &Workbook, f: &str, k: ErrorKind) {
        assert_eq!(w.eval_formula(0, f), Value::Error(k), "{f}");
    }

    #[test]
    fn format_and_parse() {
        let w = wb();
        text(&w, "=COMPLEX(3,4)", "3+4i");
        text(&w, "=COMPLEX(3,-4,\"j\")", "3-4j");
        text(&w, "=COMPLEX(0,1)", "i");
        text(&w, "=COMPLEX(0,-1)", "-i");
        text(&w, "=COMPLEX(1,1)", "1+i");
        text(&w, "=COMPLEX(0,0)", "0");
        text(&w, "=COMPLEX(-2.5,0)", "-2.5");
        text(&w, "=COMPLEX(1E+20,1E-20)", "1E+20+1E-20i");
        text(&w, "=COMPLEX(0.0001,123456789012345678)", "0.0001+1.23456789012346E+17i");
        text(&w, "=COMPLEX(0.00001,-1/3)", "1E-05-0.333333333333333i");
        err(&w, "=COMPLEX(1,2,\"k\")", ErrorKind::Value);
        err(&w, "=COMPLEX(1,2,\"I\")", ErrorKind::Value);
        close(&w, "=IMREAL(\"6-9i\")", 6.0, 0.0);
        close(&w, "=IMAGINARY(\"0-j\")", -1.0, 0.0);
        close(&w, "=IMAGINARY(\"-j\")", -1.0, 0.0);
        close(&w, "=IMAGINARY(\"+i\")", 1.0, 0.0);
        close(&w, "=IMAGINARY(\"2.5e1i\")", 25.0, 0.0);
        close(&w, "=IMREAL(\"1.5E-1+2i\")", 0.15, 0.0);
        close(&w, "=IMAGINARY(4)", 0.0, 0.0);
        close(&w, "=IMABS(\"5+12i\")", 13.0, 0.0);
        close(&w, "=IMARGUMENT(\"3+4i\")", 0.927_295_218_001_612, 1e-15);
        err(&w, "=IMARGUMENT(0)", ErrorKind::Div0);
        for bad in ["\"abc\"", "\"1+2\"", "\"1+2k\"", "\"i2\"", "\" 1+i\"", "\"1++2i\"", "\"1+2ii\"", "\"+\"", "\"1+2I\""] {
            err(&w, &format!("=IMREAL({bad})"), ErrorKind::Num);
        }
        err(&w, "=IMREAL(TRUE)", ErrorKind::Value);
        text(&w, "=IMCONJUGATE(\"3+4j\")", "3-4j");
        text(&w, "=IMCONJUGATE(5)", "5");
        err(&w, "=IMSUM(\"1+i\",\"1+j\")", ErrorKind::Value);
        text(&w, "=IMSUM(\"1+j\",2)", "3+j");
    }

    /// Results of LibreOffice 26.2 (and of Excel's documentation).
    #[test]
    fn arithmetic() {
        let mut w = wb();
        text(&w, "=IMSUM(\"3+4i\",\"5-3i\")", "8+i");
        text(&w, "=IMSUB(\"13+4i\",\"5+3i\")", "8+i");
        text(&w, "=IMPRODUCT(\"3+4i\",\"5-3i\")", "27+11i");
        text(&w, "=IMPRODUCT(\"1+2i\",30)", "30+60i");
        text(&w, "=IMDIV(\"-238+240i\",\"10+24i\")", "5+12i");
        err(&w, "=IMDIV(1,0)", ErrorKind::Num);
        text(&w, "=IMPOWER(\"2+3i\",3)", "-46+9.00000000000001i");
        text(&w, "=IMSQRT(\"-4\")", "2i");
        text(&w, "=IMSQRT(\"1+i\")", "1.09868411346781+0.455089860562227i");
        text(&w, "=IMEXP(\"1+i\")", "1.46869393991589+2.28735528717884i");
        text(&w, "=IMLN(\"3+4i\")", "1.6094379124341+0.927295218001612i");
        text(&w, "=IMLOG10(\"3+4i\")", "0.698970004336019+0.402719196273373i");
        text(&w, "=IMLOG2(\"3+4i\")", "2.32192809488736+1.33780421245098i");
        err(&w, "=IMLN(0)", ErrorKind::Num);
        text(&w, "=IMSUM({\"1+i\",\"2\";\"3i\",\"\"})", "3+4i");
        err(&w, "=IMSUM({\"1+i\",\"3j\"})", ErrorKind::Value);
        w.set_input(0, crate::CellRef::new(0, 0), "1+i");
        w.set_input(0, crate::CellRef::new(1, 0), "2-i");
        w.set_input(0, crate::CellRef::new(3, 0), "3");
        text(&w, "=IMSUM(A1:A4)", "6");
        text(&w, "=IMPRODUCT(A1:A4,\"i\")", "-3+9i");
    }

    #[test]
    fn trigonometry() {
        let w = wb();
        text(&w, "=IMSIN(\"4+3i\")", "-7.61923172032141-6.548120040911i");
        text(&w, "=IMCOS(\"1+i\")", "0.833730025131149-0.988897705762865i");
        text(&w, "=IMTAN(\"4+3i\")", "0.00490825806749606+1.00070953606723i");
        text(&w, "=IMCOT(\"4+3i\")", "0.00490118239430447-0.999266927805902i");
        // Excel's documented result (LibreOffice: -0.065294027857947...).
        text(&w, "=IMSEC(\"4+3i\")", "-0.0652940278579471-0.0752249603027732i");
        text(&w, "=IMCSC(\"4+3i\")", "-0.0754898329158637+0.0648774713706355i");
        text(&w, "=IMSINH(\"4+3i\")", "-27.0168132580039+3.85373803791938i");
        text(&w, "=IMCOSH(\"4+3i\")", "-27.0349456030742+3.85115333481178i");
        text(&w, "=IMSECH(\"4+3i\")", "-0.0362534969158689-0.00516434460775318i");
        text(&w, "=IMCSCH(\"4+3i\")", "-0.036275889628626-0.0051744731840194i");
        err(&w, "=IMCSC(0)", ErrorKind::Num);
        err(&w, "=IMCOT(0)", ErrorKind::Num);
        err(&w, "=IMCSCH(0)", ErrorKind::Num);
    }

    #[test]
    fn bessel() {
        let w = wb();
        close(&w, "=BESSELJ(1.9,2)", 0.329_925_727_692_387, 1e-12);
        close(&w, "=BESSELK(1.5,1)", 0.277_387_803_602_702, 1e-12);
        close(&w, "=BESSELY(2.5,1)", 0.145_918_137_966_786, 1e-12);
        close(&w, "=BESSELI(1.5,1)", 0.981_666_428_577_908, 1e-12);
        err(&w, "=BESSELJ(1,-1)", ErrorKind::Num);
        err(&w, "=BESSELK(0,1)", ErrorKind::Num);
        err(&w, "=BESSELY(-1,1)", ErrorKind::Num);
        err(&w, "=BESSELI(\"a\",1)", ErrorKind::Value);
    }

    #[test]
    fn file_names() {
        let e = crate::formula::parse_canonical("IMSUM(IMCOSH(A1),IMTAN(1))&ECMA.CEILING(2.5,1)&BETADIST(0.5,2,3)&LOGNORMDIST(4,3.5,1.2)").unwrap();
        let x = e.to_xlsx();
        assert_eq!(x, "IMSUM(_xlfn.IMCOSH(A1),_xlfn.IMTAN(1))&_xlfn.ECMA.CEILING(2.5,1)&BETADIST(0.5,2,3)&LOGNORMDIST(4,3.5,1.2)");
        assert_eq!(crate::formula::parse_canonical(&x).unwrap(), e);
        assert_eq!(e.to_odf(), "of:=IMSUM(IMCOSH([.A1]);IMTAN(1))&_xlfn.ECMA.CEILING(2.5;1)&BETADIST(0.5;2;3)&LOGNORMDIST(4;3.5;1.2)");
        let mut w = wb();
        w.locale = Locale::ES;
        // "." inside complex numbers whatever the locale, as Excel.
        text(&w, "=IM.SUM(\"1.5+i\";COMPLEJO(1;2,5))", "2.5+3.5i");
        err(&w, "=IM.REAL(\"1,5+i\")", ErrorKind::Num);
    }
}

