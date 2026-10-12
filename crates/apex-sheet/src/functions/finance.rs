//! Spreadsheet functions: finance.

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    match f {
        Irr | Mirr => Some(0b1),
        Xirr => Some(0b11),
        Xnpv => Some(0b110),
        FvSchedule => Some(0b10),
        _ => None,
    }
}

fn need(a: &[Expr], min: usize, max: usize) -> R<()> {
    if a.len() < min || a.len() > max { Err(ErrorKind::Value) } else { Ok(()) }
}

/// The `type` argument: payments at the start of each period when nonzero.
fn due(env: &Env, a: &[Expr], i: usize) -> R<bool> {
    Ok(env.opt_num(a, i, 0.0)? != 0.0)
}

fn fv(rate: f64, nper: f64, pmt: f64, pv: f64, due: bool) -> f64 {
    if rate == 0.0 {
        return -(pv + pmt * nper);
    }
    let q = (1.0 + rate).powf(nper);
    let t = if due { 1.0 + rate } else { 1.0 };
    -(pv * q + pmt * t * (q - 1.0) / rate)
}

fn pmt(rate: f64, nper: f64, pv: f64, fv: f64, due: bool) -> f64 {
    if rate == 0.0 {
        return -(pv + fv) / nper;
    }
    let q = (1.0 + rate).powf(nper);
    let t = if due { 1.0 + rate } else { 1.0 };
    -(rate * (pv * q + fv)) / (t * (q - 1.0))
}

fn ipmt(rate: f64, per: f64, nper: f64, pv: f64, fv_: f64, due: bool) -> f64 {
    let p = pmt(rate, nper, pv, fv_, due);
    let interest = match (per == 1.0, due) {
        (true, true) => 0.0,
        (true, false) => -pv,
        (false, true) => fv(rate, per - 2.0, p, pv, true) - p,
        (false, false) => fv(rate, per - 1.0, p, pv, false),
    };
    interest * rate
}

/// Finds a rate where `f` (value, derivative) is zero: Newton from
/// `guess`, then bisection on the sign change nearest to the guess.
pub(super) fn solve(f: impl Fn(f64) -> (f64, f64), guess: f64) -> Option<f64> {
    let mut x = guess;
    for _ in 0..100 {
        let (y, dy) = f(x);
        if !y.is_finite() || !dy.is_finite() || dy == 0.0 {
            break;
        }
        if y == 0.0 {
            return Some(x);
        }
        let mut nx = x - y / dy;
        if nx <= -1.0 {
            nx = (x - 1.0) / 2.0;
        }
        if (nx - x).abs() <= 1e-12 * nx.abs().max(1.0) {
            // Creeping towards -1 also takes tiny steps: check for a sign change.
            let h = 1e-9 * nx.abs().max(1.0);
            let (lo, hi) = (f(nx - h).0, f(nx + h).0);
            if f(nx).0 == 0.0 || (lo < 0.0) != (hi < 0.0) {
                return Some(nx);
            }
            break;
        }
        x = nx;
    }
    const GRID: [f64; 34] = [
        -0.9999, -0.999, -0.99, -0.95, -0.9, -0.8, -0.7, -0.6, -0.5, -0.4, -0.3, -0.2, -0.1, -0.05, 0.0, 0.02, 0.05, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0, 20.0, 50.0, 100.0, 1000.0,
    ];
    let mut best: Option<(f64, f64, f64)> = None;
    for w in GRID.windows(2) {
        let (lo, hi) = (w[0], w[1]);
        let (flo, fhi) = (f(lo).0, f(hi).0);
        if flo.is_finite() && fhi.is_finite() && (flo < 0.0) != (fhi < 0.0) {
            let dist = if guess < lo { lo - guess } else if guess > hi { guess - hi } else { 0.0 };
            if best.is_none_or(|b| dist < b.2) {
                best = Some((lo, hi, dist));
            }
        }
    }
    let (mut lo, mut hi, _) = best?;
    let neg_lo = f(lo).0 < 0.0;
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if mid == lo || mid == hi {
            break;
        }
        if (f(mid).0 < 0.0) == neg_lo {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

/// DDB depreciation for one (possibly fractional) period.
fn ddb(cost: f64, salvage: f64, life: f64, period: f64, factor: f64) -> f64 {
    let rate = factor / life;
    let (rate, old) = if rate >= 1.0 { (1.0, if period == 1.0 { cost } else { 0.0 }) } else { (rate, cost * (1.0 - rate).powf(period - 1.0)) };
    let new = cost * (1.0 - rate).powf(period);
    (if new < salvage { old - salvage } else { old - new }).max(0.0)
}

/// Declining balance over the first `period` periods, switching to
/// straight line when that gives more (LibreOffice's algorithm, as Excel).
fn inter_vdb(cost: f64, salvage: f64, life: f64, life1: f64, period: f64, factor: f64) -> f64 {
    let end = period.ceil();
    let mut rest = cost - salvage;
    let (mut total, mut sln, mut straight) = (0.0, 0.0, false);
    for i in 1..=end as u64 {
        let mut term = if straight {
            sln
        } else {
            let d = ddb(cost, salvage, life, i as f64, factor);
            sln = rest / (life1 - (i - 1) as f64);
            if sln > d {
                straight = true;
                sln
            } else {
                rest -= d;
                d
            }
        };
        if i as f64 == end {
            term *= period + 1.0 - end;
        }
        total += term;
    }
    total
}

fn vdb(cost: f64, salvage: f64, life: f64, start: f64, end: f64, factor: f64, no_switch: bool) -> f64 {
    let (istart, iend) = (start.floor(), end.ceil());
    if no_switch {
        let mut total = 0.0;
        for i in istart as u64 + 1..=iend as u64 {
            let mut term = ddb(cost, salvage, life, i as f64, factor);
            if i == istart as u64 + 1 {
                term *= end.min(istart + 1.0) - start;
            } else if i == iend as u64 {
                term *= end + 1.0 - iend;
            }
            total += term;
        }
        return total;
    }
    // Partial first and last periods are taken out of whole-period amounts.
    let mut part = 0.0;
    if start != istart {
        let value = cost - inter_vdb(cost, salvage, life, life, istart, factor);
        part += (start - istart) * inter_vdb(value, salvage, life, life - istart, 1.0, factor);
    }
    if end != iend {
        let value = cost - inter_vdb(cost, salvage, life, life, iend - 1.0, factor);
        part += (iend - end) * inter_vdb(value, salvage, life, life - (iend - 1.0), 1.0, factor);
    }
    let cost = cost - inter_vdb(cost, salvage, life, life, istart, factor);
    inter_vdb(cost, salvage, life, life - istart, iend - istart, factor) - part
}

/// Values and dates for XNPV/XIRR, checked like Excel.
fn cash_flows(env: &Env, values: &Expr, dates: &Expr) -> R<(Vec<f64>, Vec<f64>)> {
    let (vt, dt) = (env.tab(values)?, env.tab(dates)?);
    if vt.len() != dt.len() || vt.len() == 0 {
        return Err(ErrorKind::Num);
    }
    let mut v = Vec::with_capacity(vt.len());
    let mut d = Vec::with_capacity(vt.len());
    for i in 0..vt.len() {
        v.push(match vt.nth(i) {
            Value::Number(n) => n,
            Value::Error(k) => return Err(k),
            _ => return Err(ErrorKind::Value),
        });
        d.push(match dt.nth(i) {
            Value::Number(n) if n >= 0.0 => n.floor(),
            Value::Number(_) => return Err(ErrorKind::Num),
            Value::Error(k) => return Err(k),
            _ => return Err(ErrorKind::Value),
        });
    }
    // Excel lets a date before the first one pass when nothing is paid
    // then (schedules whose later years wrapped round to 1900s dates).
    if d.iter().zip(&v).any(|(x, n)| *x < d[0] && *n != 0.0) {
        return Err(ErrorKind::Num);
    }
    Ok((v, d))
}

fn has_both_signs(v: &[f64]) -> bool {
    v.iter().any(|x| *x > 0.0) && v.iter().any(|x| *x < 0.0)
}

/// Fractional-dollar conversions: the fraction digits of `x` read as
/// numerator over `from`, rewritten over `to`.
fn dollar(x: f64, fraction: f64, to_decimal: bool) -> R<Value> {
    let f = fraction.trunc();
    if f < 0.0 {
        return Err(ErrorKind::Num);
    }
    if f == 0.0 {
        return Err(ErrorKind::Div0);
    }
    let scale = 10f64.powf(f.log10().ceil());
    let (int, frac) = (x.trunc(), x - x.trunc());
    Ok(num_ok(int + if to_decimal { frac * scale / f } else { frac * f / scale }))
}

/// The fixed rate of a currency to the euro, and the decimals its amounts
/// are rounded to.
fn euro_rate(code: &str) -> R<(f64, i32)> {
    Ok(match code.trim().to_ascii_uppercase().as_str() {
        "EUR" => (1.0, 2),
        "BEF" => (40.3399, 0),
        "LUF" => (40.3399, 0),
        "DEM" => (1.95583, 2),
        "ESP" => (166.386, 0),
        "FRF" => (6.55957, 2),
        "IEP" => (0.787564, 2),
        "ITL" => (1936.27, 0),
        "NLG" => (2.20371, 2),
        "ATS" => (13.7603, 2),
        "PTE" => (200.482, 1),
        "FIM" => (5.94573, 2),
        "GRD" => (340.75, 0),
        "SIT" => (239.64, 2),
        "CYP" => (0.585274, 2),
        "MTL" => (0.4293, 2),
        "SKK" => (30.126, 2),
        "EEK" => (15.6466, 2),
        "LVL" => (0.702804, 2),
        "LTL" => (3.4528, 2),
        "HRK" => (7.5345, 2),
        _ => return Err(ErrorKind::Value),
    })
}

/// `x` rounded half away from zero to `d` decimals.
fn round_to(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    let r = (x * m).abs();
    // A hair over, so 0.5 written in binary as 0.4999… still goes up.
    (r + 0.5 + r * 1e-15).floor().copysign(x) / m
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    eval(env, f, a).transpose()
}

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Option<Value>> {
    use Func::*;
    let num = |i: usize| env.num(&a[i]);
    Ok(Some(match f {
        Rate => {
            need(a, 3, 6)?;
            let (n, p, pv) = (num(0)?, num(1)?, num(2)?);
            let (fv_, t) = (env.opt_num(a, 3, 0.0)?, if due(env, a, 4)? { 1.0 } else { 0.0 });
            let guess = env.opt_num(a, 5, 0.1)?;
            if n <= 0.0 {
                return Err(ErrorKind::Num);
            }
            let f = |r: f64| {
                if r.abs() < 1e-10 {
                    return (pv + p * n + fv_, pv * n + p * (t * n + n * (n - 1.0) / 2.0));
                }
                let q = (1.0 + r).powf(n);
                let dq = n * (1.0 + r).powf(n - 1.0);
                let k = (1.0 + r * t) * (q - 1.0) / r;
                let dk = t * (q - 1.0) / r + (1.0 + r * t) * (dq * r - (q - 1.0)) / (r * r);
                (pv * q + p * k + fv_, pv * dq + p * dk)
            };
            num_ok(solve(f, guess).ok_or(ErrorKind::Num)?)
        }
        Nper => {
            need(a, 3, 5)?;
            let (rate, p, pv) = (num(0)?, num(1)?, num(2)?);
            let (fv_, due) = (env.opt_num(a, 3, 0.0)?, due(env, a, 4)?);
            if rate == 0.0 {
                if p == 0.0 {
                    return Err(ErrorKind::Num);
                }
                return Ok(Some(num_ok(-(pv + fv_) / p)));
            }
            let z = p * (if due { 1.0 + rate } else { 1.0 }) / rate;
            let x = (z - fv_) / (pv + z);
            if x <= 0.0 || rate <= -1.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(x.ln() / rate.ln_1p())
        }
        Irr => {
            need(a, 1, 2)?;
            let mut v = Vec::new();
            env.numbers(&a[..1], &mut v)?;
            let guess = env.opt_num(a, 1, 0.1)?;
            if !has_both_signs(&v) {
                return Err(ErrorKind::Num);
            }
            let f = |r: f64| {
                let (mut y, mut dy, mut disc) = (0.0, 0.0, 1.0);
                for (i, x) in v.iter().enumerate() {
                    y += x * disc;
                    disc /= 1.0 + r;
                    dy -= i as f64 * x * disc;
                }
                (y, dy)
            };
            num_ok(solve(f, guess).ok_or(ErrorKind::Num)?)
        }
        Xnpv => {
            need(a, 3, 3)?;
            let rate = num(0)?;
            let (v, d) = cash_flows(env, &a[1], &a[2])?;
            if rate <= -1.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(v.iter().zip(&d).map(|(x, t)| x / (1.0 + rate).powf((t - d[0]) / 365.0)).sum())
        }
        Xirr => {
            need(a, 2, 3)?;
            let (v, d) = cash_flows(env, &a[0], &a[1])?;
            let guess = env.opt_num(a, 2, 0.1)?;
            if !has_both_signs(&v) || guess <= -1.0 {
                return Err(ErrorKind::Num);
            }
            let f = |r: f64| {
                let (mut y, mut dy) = (0.0, 0.0);
                for (x, t) in v.iter().zip(&d) {
                    let e = (t - d[0]) / 365.0;
                    let disc = (1.0 + r).powf(-e);
                    y += x * disc;
                    dy -= e * x * disc / (1.0 + r);
                }
                (y, dy)
            };
            num_ok(solve(f, guess).ok_or(ErrorKind::Num)?)
        }
        Ipmt | Ppmt => {
            need(a, 4, 6)?;
            let (rate, per, nper, pv) = (num(0)?, num(1)?, num(2)?, num(3)?);
            let (fv_, due) = (env.opt_num(a, 4, 0.0)?, due(env, a, 5)?);
            if per < 1.0 || per > nper {
                return Err(ErrorKind::Num);
            }
            let i = ipmt(rate, per, nper, pv, fv_, due);
            num_ok(if f == Ipmt { i } else { pmt(rate, nper, pv, fv_, due) - i })
        }
        CumIpmt | CumPrinc => {
            need(a, 6, 6)?;
            let (rate, nper, pv) = (num(0)?, num(1)?, num(2)?);
            let (start, end, t) = (num(3)?.trunc(), num(4)?.trunc(), num(5)?);
            if rate <= 0.0 || nper <= 0.0 || pv <= 0.0 || start < 1.0 || end < start || end > nper || (t != 0.0 && t != 1.0) {
                return Err(ErrorKind::Num);
            }
            let due = t == 1.0;
            let p = pmt(rate, nper, pv, 0.0, due);
            // Balance after payment k; the principal paid is its change.
            let bal = |k: f64| if k == 0.0 { pv } else { -fv(rate, k, p, pv, due) / if due { 1.0 + rate } else { 1.0 } };
            let princ = bal(end) - bal(start - 1.0);
            num_ok(if f == CumPrinc { princ } else { p * (end - start + 1.0) - princ })
        }
        IsPmt => {
            need(a, 4, 4)?;
            let (rate, per, nper, pv) = (num(0)?, num(1)?, num(2)?, num(3)?);
            if nper == 0.0 {
                return Err(ErrorKind::Div0);
            }
            num_ok(pv * rate * (per / nper - 1.0))
        }
        Sln => {
            need(a, 3, 3)?;
            let (cost, salvage, life) = (num(0)?, num(1)?, num(2)?);
            if life == 0.0 {
                return Err(ErrorKind::Div0);
            }
            num_ok((cost - salvage) / life)
        }
        Syd => {
            need(a, 4, 4)?;
            let (cost, salvage, life, per) = (num(0)?, num(1)?, num(2)?, num(3)?);
            if life <= 0.0 || per <= 0.0 || per > life {
                return Err(ErrorKind::Num);
            }
            num_ok((cost - salvage) * (life - per + 1.0) * 2.0 / (life * (life + 1.0)))
        }
        Db => {
            need(a, 4, 5)?;
            let (cost, salvage, life, period) = (num(0)?, num(1)?, num(2)?, num(3)?.trunc());
            let month = env.opt_num(a, 4, 12.0)?.trunc();
            let last = if month < 12.0 { life + 1.0 } else { life };
            if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period < 1.0 || !(1.0..=12.0).contains(&month) || period > last {
                return Err(ErrorKind::Num);
            }
            if cost == 0.0 {
                return Ok(Some(Value::Number(0.0)));
            }
            // Excel rounds the rate to three decimals.
            let rate = ((1.0 - (salvage / cost).powf(1.0 / life)) * 1000.0).round() / 1000.0;
            let first = cost * rate * month / 12.0;
            if period == 1.0 {
                return Ok(Some(num_ok(first)));
            }
            let before = (cost - first) * (1.0 - rate).powf(period - 2.0);
            let dep = before * rate;
            num_ok(if period > life { dep * (12.0 - month) / 12.0 } else { dep })
        }
        Ddb => {
            need(a, 4, 5)?;
            let (cost, salvage, life, period) = (num(0)?, num(1)?, num(2)?, num(3)?);
            let factor = env.opt_num(a, 4, 2.0)?;
            if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period <= 0.0 || factor <= 0.0 || period > life {
                return Err(ErrorKind::Num);
            }
            num_ok(ddb(cost, salvage, life, period, factor))
        }
        Vdb => {
            need(a, 5, 7)?;
            let (cost, salvage, life, start, end) = (num(0)?, num(1)?, num(2)?, num(3)?, num(4)?);
            let factor = env.opt_num(a, 5, 2.0)?;
            let no_switch = env.opt_bool(a, 6, false)?;
            if cost < 0.0 || salvage < 0.0 || salvage > cost || life <= 0.0 || start < 0.0 || end < start || end > life || factor <= 0.0 {
                return Err(ErrorKind::Num);
            }
            // The schedule is walked period by period.
            if end > 1e7 {
                return Err(ErrorKind::Num);
            }
            num_ok(vdb(cost, salvage, life, start, end, factor, no_switch))
        }
        Mirr => {
            need(a, 3, 3)?;
            let mut v = Vec::new();
            env.numbers(&a[..1], &mut v)?;
            let (fin, re) = (num(1)?, num(2)?);
            let n = v.len() as f64;
            let (mut pos, mut neg) = (0.0, 0.0);
            for (i, x) in v.iter().enumerate() {
                if *x > 0.0 {
                    pos += x / (1.0 + re).powi(i as i32);
                } else {
                    neg += x / (1.0 + fin).powi(i as i32);
                }
            }
            if pos == 0.0 || neg == 0.0 {
                return Err(ErrorKind::Div0);
            }
            num_ok((-pos * (1.0 + re).powf(n - 1.0) / neg).powf(1.0 / (n - 1.0)) - 1.0)
        }
        Effect | Nominal => {
            need(a, 2, 2)?;
            let (r, n) = (num(0)?, num(1)?.trunc());
            if r <= 0.0 || n < 1.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(if f == Effect { (1.0 + r / n).powf(n) - 1.0 } else { n * ((1.0 + r).powf(1.0 / n) - 1.0) })
        }
        Rri => {
            need(a, 3, 3)?;
            let (n, pv, fv_) = (num(0)?, num(1)?, num(2)?);
            if n <= 0.0 || pv == 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok((fv_ / pv).powf(1.0 / n) - 1.0)
        }
        PDuration => {
            need(a, 3, 3)?;
            let (rate, pv, fv_) = (num(0)?, num(1)?, num(2)?);
            if rate <= 0.0 || pv <= 0.0 || fv_ <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok((fv_.ln() - pv.ln()) / rate.ln_1p())
        }
        FvSchedule => {
            need(a, 2, 2)?;
            let mut x = num(0)?;
            let mut err = None;
            env.each(&a[1], &mut |v, _| match v {
                Value::Number(r) => x *= 1.0 + r,
                Value::Empty => {}
                Value::Error(k) => {
                    err.get_or_insert(*k);
                }
                _ => {
                    err.get_or_insert(ErrorKind::Value);
                }
            });
            if let Some(k) = err {
                return Err(k);
            }
            num_ok(x)
        }
        DollarDe | DollarFr => {
            need(a, 2, 2)?;
            dollar(num(0)?, num(1)?, f == DollarDe)?
        }
        EuroConvert => {
            need(a, 3, 5)?;
            let x = num(0)?;
            let (from, to) = (euro_rate(&env.text(&a[1])?)?, euro_rate(&env.text(&a[2])?)?);
            let full = env.opt_bool(a, 3, false)?;
            let mut euros = x / from.0;
            if a.len() > 4 && !matches!(a[4], Expr::Missing) {
                let t = num(4)?.trunc();
                if t < 3.0 {
                    return Err(ErrorKind::Value);
                }
                if from.0 != 1.0 && to.0 != 1.0 {
                    euros = round_to(euros, t as i32);
                }
            }
            let y = euros * to.0;
            num_ok(if full { y } else { round_to(y, to.1) })
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use crate::{CellRef, ErrorKind, Locale, Value, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        w
    }

    /// Checks against Excel's result, to `tol` relative.
    fn close(w: &Workbook, f: &str, want: f64, tol: f64) {
        match w.eval_formula(0, f) {
            Value::Number(got) => assert!((got - want).abs() <= tol * want.abs().max(1e-3), "{f}: {got} != {want}"),
            v => panic!("{f} gave {v:?}"),
        }
    }

    #[test]
    fn euroconvert_follows_the_fixed_rates_and_rounding() {
        let w = wb();
        // Excel's examples.
        close(&w, "=EUROCONVERT(1,\"FRF\",\"DEM\",TRUE,3)", 0.297_286_16, 1e-9);
        close(&w, "=EUROCONVERT(1,\"FRF\",\"DEM\",FALSE,3)", 0.30, 1e-12);
        close(&w, "=EUROCONVERT(1,\"FRF\",\"EUR\")", 0.15, 1e-12);
        close(&w, "=EUROCONVERT(1,\"FRF\",\"EUR\",TRUE,3)", 0.152_449, 1e-6);
        close(&w, "=EUROCONVERT(1,\"EUR\",\"esp\")", 166.0, 1e-12);
        assert_eq!(w.eval_formula(0, "=EUROCONVERT(1,\"XYZ\",\"EUR\")"), Value::Error(ErrorKind::Value));
        assert_eq!(w.eval_formula(0, "=EUROCONVERT(1,\"FRF\",\"DEM\",TRUE,2)"), Value::Error(ErrorKind::Value));
    }

    fn err(w: &Workbook, f: &str, k: ErrorKind) {
        assert_eq!(w.eval_formula(0, f), Value::Error(k), "{f}");
    }

    #[test]
    fn loans() {
        let w = wb();
        close(&w, "=RATE(48,-200,8000)", 0.007_701_472_488_201_65, 1e-9);
        close(&w, "=RATE(48,-200,8000)*12", 0.092_417_669_858_42, 1e-9);
        close(&w, "=RATE(10,0,-100,200)", 2f64.powf(0.1) - 1.0, 1e-9);
        close(&w, "=RATE(12,-100,1200)", 0.0, 1.0);
        err(&w, "=RATE(0,-200,8000)", ErrorKind::Num);
        err(&w, "=RATE(10,100,100)", ErrorKind::Num);
        close(&w, "=NPER(0.01,-100,-1000,10000,1)", 59.673_865_674_294_6, 1e-9);
        close(&w, "=NPER(0.01,-100,-1000,10000)", 60.082_122_853_761_1, 1e-9);
        close(&w, "=NPER(0.01,-100,-1000)", -9.578_594_039_813_17, 1e-9);
        close(&w, "=NPER(0,-100,1000)", 10.0, 1e-12);
        err(&w, "=NPER(0.01,-100,20000)", ErrorKind::Num);
        close(&w, "=IPMT(0.1/12,1,36,8000)", -66.666_666_666_666_7, 1e-9);
        close(&w, "=IPMT(0.1,3,3,8000)", -292.447_129_909_366, 1e-9);
        close(&w, "=IPMT(0.1,1,3,8000,0,1)", 0.0, 1e-9);
        err(&w, "=IPMT(0.1,4,3,8000)", ErrorKind::Num);
        close(&w, "=PPMT(0.1/12,1,24,2000)", -75.623_186_008_366_7, 1e-9);
        close(&w, "=PPMT(0.08,10,10,200000)", -27_598.053_462_421_4, 1e-9);
        // Interest plus principal is the payment, also for payments in advance.
        close(&w, "=IPMT(0.05,4,10,1000,0,1)+PPMT(0.05,4,10,1000,0,1)", -123.337_690_443_292, 1e-9);
        close(&w, "=IPMT(0.05,2,10,1000,0,1)", -43.833_115_477_835_4, 1e-9);
        close(&w, "=CUMIPMT(0.09/12,360,125000,13,24,0)", -11_135.232_130_750_5, 1e-9);
        close(&w, "=CUMIPMT(0.09/12,360,125000,1,1,0)", -937.5, 1e-9);
        close(&w, "=CUMPRINC(0.09/12,360,125000,13,24,0)", -934.107_123_420_868, 1e-9);
        close(&w, "=CUMPRINC(0.09/12,360,125000,1,1,0)", -68.278_271_180_977_5, 1e-9);
        // Payments in advance: the sums match period by period IPMT.
        close(&w, "=CUMIPMT(0.05,10,1000,2,4,1)", -119.374_898_751_274, 1e-9);
        close(&w, "=CUMIPMT(0.05,10,1000,1,4,1)+CUMPRINC(0.05,10,1000,1,4,1)", 4.0 * -123.337_690_443_292, 1e-9);
        err(&w, "=CUMIPMT(0.05,10,1000,2,4,2)", ErrorKind::Num);
        err(&w, "=CUMPRINC(0.05,10,1000,5,4,0)", ErrorKind::Num);
        close(&w, "=ISPMT(0.1/12,1,36,8000000)", -64_814.814_814_814_8, 1e-9);
        close(&w, "=ISPMT(0.1,1,3,8000000)", -533_333.333_333_333, 1e-9);
        err(&w, "=ISPMT(0.1,1,0,8000000)", ErrorKind::Div0);
    }

    #[test]
    fn cash_flows() {
        let mut w = wb();
        for (r, v) in ["-70000", "12000", "15000", "18000", "21000", "26000"].iter().enumerate() {
            w.set_input(0, CellRef::new(r as u32, 0), v);
        }
        close(&w, "=IRR(A1:A5)", -0.021_244_848_272_975_5, 1e-8);
        close(&w, "=IRR(A1:A6)", 0.086_630_948_036_531_3, 1e-8);
        close(&w, "=IRR(A1:A3,-0.1)", -0.443_506_941_334_192, 1e-8);
        close(&w, "=IRR({-100,230,-132},0.5)", 0.2, 1e-8);
        close(&w, "=IRR({-100,230,-132},0.05)", 0.1, 1e-8);
        close(&w, "=IRR({-1000,10,10,10,10,10,1100},100)", 0.024_120_619_756_840_76, 1e-8);
        err(&w, "=IRR({1,2,3})", ErrorKind::Num);
        let vals = ["-10000", "2750", "4250", "3250", "2750"];
        let dates = ["2008-01-01", "2008-03-01", "2008-10-30", "2009-02-15", "2009-04-01"];
        for r in 0..5 {
            w.set_input(0, CellRef::new(r, 1), vals[r as usize]);
            w.set_input(0, CellRef::new(r, 2), dates[r as usize]);
        }
        close(&w, "=XNPV(0.09,B1:B5,C1:C5)", 2_086.647_602_031_33, 1e-9);
        close(&w, "=XIRR(B1:B5,C1:C5)", 0.373_362_535_4, 1e-8);
        close(&w, "=XIRR(B1:B5,C1:C5,-0.5)", 0.373_362_535_4, 1e-8);
        err(&w, "=XIRR(B1:B5,C1:C4)", ErrorKind::Num);
        err(&w, "=XNPV(0.09,B1:B5,C5:C9)", ErrorKind::Value);
        close(&w, "=NPV(0.1,-10000,3000,4200,6800)", 1_188.443_412_335_67, 1e-9);
        for (r, v) in ["-120000", "39000", "30000", "21000", "37000", "46000"].iter().enumerate() {
            w.set_input(0, CellRef::new(r as u32, 3), v);
        }
        close(&w, "=MIRR(D1:D6,0.1,0.12)", 0.126_094_130_0, 1e-8);
        close(&w, "=MIRR(D1:D4,0.1,0.12)", -0.048_044_655_249_980_1, 1e-8);
        close(&w, "=MIRR(D1:D6,0.1,0.14)", 0.134_759_111_3, 1e-8);
        err(&w, "=MIRR(D2:D6,0.1,0.12)", ErrorKind::Div0);
        close(&w, "=FVSCHEDULE(1,{0.09,0.11,0.1})", 1.33089, 1e-12);
        err(&w, "=FVSCHEDULE(1,{0.09,\"x\"})", ErrorKind::Value);
        // A date before the first with nothing paid then is let through.
        for r in 0..5 {
            w.set_input(0, CellRef::new(r, 4), vals[r as usize]);
            w.set_input(0, CellRef::new(r, 5), dates[r as usize]);
        }
        w.set_input(0, CellRef::new(5, 4), "0");
        w.set_input(0, CellRef::new(5, 5), "1930-05-31");
        close(&w, "=XIRR(E1:E6,F1:F6)", 0.373_362_535_4, 1e-8);
        w.set_input(0, CellRef::new(5, 4), "5");
        err(&w, "=XIRR(E1:E6,F1:F6)", ErrorKind::Num);
    }

    #[test]
    fn depreciation() {
        let w = wb();
        close(&w, "=SLN(30000,7500,10)", 2250.0, 1e-12);
        err(&w, "=SLN(30000,7500,0)", ErrorKind::Div0);
        close(&w, "=SYD(30000,7500,10,1)", 4_090.909_090_909_09, 1e-9);
        close(&w, "=SYD(30000,7500,10,10)", 409.090_909_090_909, 1e-9);
        err(&w, "=SYD(30000,7500,10,11)", ErrorKind::Num);
        let db = [186_083.333_333_333, 259_639.416_666_667, 176_814.442_75, 120_410.635_512_75, 81_999.642_784_182_7, 55_841.756_736_028_5, 15_845.098_473_848_1];
        for (i, want) in db.iter().enumerate() {
            close(&w, &format!("=DB(1000000,100000,6,{},7)", i + 1), *want, 1e-9);
        }
        err(&w, "=DB(1000000,100000,6,8,7)", ErrorKind::Num);
        err(&w, "=DB(1000000,100000,6,7)", ErrorKind::Num);
        err(&w, "=DB(1000000,100000,6,1,13)", ErrorKind::Num);
        close(&w, "=DDB(2400,300,10*365,1)", 1.315_068_493_150_68, 1e-9);
        close(&w, "=DDB(2400,300,10*12,1,2)", 40.0, 1e-9);
        close(&w, "=DDB(2400,300,10,1,2)", 480.0, 1e-9);
        close(&w, "=DDB(2400,300,10,2,1.5)", 306.0, 1e-9);
        close(&w, "=DDB(2400,300,10,10)", 22.122_547_2, 1e-9);
        err(&w, "=DDB(2400,300,10,11)", ErrorKind::Num);
        close(&w, "=VDB(2400,300,10*365,0,1)", 1.315_068_493_150_68, 1e-9);
        close(&w, "=VDB(2400,300,10*12,0,1)", 40.0, 1e-9);
        close(&w, "=VDB(2400,300,10,0,1)", 480.0, 1e-9);
        close(&w, "=VDB(2400,300,10*12,6,18)", 396.306_053_264_775, 1e-9);
        close(&w, "=VDB(2400,300,10*12,6,18,1.5)", 311.808_936_658_78, 1e-9);
        close(&w, "=VDB(2400,300,10,0,0.875,1.5)", 315.0, 1e-9);
        // The whole life adds up to cost minus salvage (switching to straight line).
        close(&w, "=VDB(2400,300,10,0,10)", 2100.0, 1e-9);
        close(&w, "=VDB(2400,300,10,0,10,2,TRUE)", 2100.0, 1e-9);
        close(&w, "=VDB(2400,300,10,7,10,2,TRUE)", 2400.0 * 0.8f64.powi(7) - 300.0, 1e-9);
        err(&w, "=VDB(2400,300,10,5,4)", ErrorKind::Num);
    }

    #[test]
    fn rates_and_dollars() {
        let w = wb();
        close(&w, "=EFFECT(0.0525,4)", 0.053_542_667_370_758_1, 1e-9);
        close(&w, "=NOMINAL(0.053543,4)", 0.052_500_319_868_356_1, 1e-9);
        err(&w, "=EFFECT(0.0525,0)", ErrorKind::Num);
        err(&w, "=NOMINAL(-0.1,4)", ErrorKind::Num);
        close(&w, "=RRI(96,10000,11000)", 0.000_993_307_376_291_330_3, 1e-9);
        err(&w, "=RRI(0,10000,11000)", ErrorKind::Num);
        close(&w, "=PDURATION(0.025,2000,2200)", 3.859_866_162_622_65, 1e-9);
        close(&w, "=PDURATION(0.025/12,1000,1200)", 87.605_476_419_083_3, 1e-9);
        err(&w, "=PDURATION(0,1000,1200)", ErrorKind::Num);
        close(&w, "=DOLLARDE(1.02,16)", 1.125, 1e-12);
        close(&w, "=DOLLARDE(1.1,32)", 1.3125, 1e-12);
        close(&w, "=DOLLARFR(1.125,16)", 1.02, 1e-12);
        close(&w, "=DOLLARFR(1.125,32)", 1.04, 1e-12);
        close(&w, "=DOLLARDE(-1.02,16)", -1.125, 1e-12);
        err(&w, "=DOLLARDE(1.02,0)", ErrorKind::Div0);
        err(&w, "=DOLLARFR(1.02,-1)", ErrorKind::Num);
        // Single-value arguments given arrays repeat the call.
        assert!(matches!(w.eval_formula(0, "=EFFECT({0.05,0.06},4)"), Value::Array(_)));
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        let w = wb();
        for f in [
            "=RATE(1E+300,-1,1)", "=NPER(-1,1,1)", "=IRR({-1E+300,1E+300})", "=XIRR({-1,1},{0,1E+300})", "=DB(1,1,1E+300,1E+300,1)",
            "=DDB(1E+300,0,1E-300,1)", "=VDB(1,0,1E+300,0,1E+300)", "=VDB(1,0,1,0,1)", "=MIRR({1},0,0)", "=DOLLARDE(1E+300,1E+300)",
            "=CUMIPMT(1E-300,1E+300,1,1,1E+300,0)", "=FVSCHEDULE(1,{})", "=IPMT(-1,1,1,1)", "=RRI(1,-1,1)",
        ] {
            let _ = w.eval_formula(0, f);
        }
    }
}
