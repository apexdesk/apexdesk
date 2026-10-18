//! Spreadsheet functions: securities (bonds, discounted securities,
//! Treasury bills) and French depreciation, as Excel computes them.

use super::date::{basis_days, date_arg, days360, is_leap, month_len, serial, year_days, yearfrac, ymd};
use super::*;

pub(super) fn array_params(_: Func) -> Option<u64> {
    None
}

fn need(a: &[Expr], min: usize, max: usize) -> R<()> {
    if a.len() < min || a.len() > max { Err(ErrorKind::Value) } else { Ok(()) }
}

/// Coupons a year: 1, 2 or 4.
fn freq(env: &Env, e: &Expr) -> R<f64> {
    let f = env.num(e)?.trunc();
    if f == 1.0 || f == 2.0 || f == 4.0 { Ok(f) } else { Err(ErrorKind::Num) }
}

/// Day-count basis 0-4 (0 when omitted).
fn basis(env: &Env, a: &[Expr], i: usize) -> R<i64> {
    let b = env.opt_num(a, i, 0.0)?.trunc();
    if (0.0..=4.0).contains(&b) { Ok(b as i64) } else { Err(ErrorKind::Num) }
}

fn is_eom(d: i64) -> bool {
    let (y, m, day) = ymd(d);
    day == month_len(y, m)
}

/// `d` moved by `months`, landing on the month's last day when `eom`.
fn add_months(d: i64, months: i64, eom: bool) -> i64 {
    let (y, m, day) = ymd(d);
    let t = y * 12 + m as i64 - 1 + months;
    let (ny, nm) = (t.div_euclid(12), t.rem_euclid(12) as u32 + 1);
    let len = month_len(ny, nm);
    serial(ny, nm as i64, if eom { len } else { day.min(len) } as i64)
}

/// Index `k` of the last date `at(k) <= d` in a schedule going forward
/// with `k` (`at(k0) <= d` for the returned `k >= k0`).
fn last_before(at: impl Fn(i64) -> i64, d: i64, k0: i64, guess: i64) -> i64 {
    let mut k = guess.max(k0);
    while k > k0 && at(k) > d {
        k -= 1;
    }
    while at(k + 1) <= d {
        k += 1;
    }
    k
}

fn months_between(s: i64, e: i64) -> i64 {
    let ((y1, m1, _), (y2, m2, _)) = (ymd(s), ymd(e));
    (y2 - y1) * 12 + m2 as i64 - m1 as i64
}

/// 30/360 US between two coupon dates (both ends adjusted).
fn days360_both(s: i64, e: i64) -> i64 {
    let ((y1, m1, mut d1), (y2, m2, mut d2)) = (ymd(s), ymd(e));
    if d2 == 31 || (m2 == 2 && d2 == month_len(y2, 2)) {
        d2 = 30;
    }
    if d1 == 31 || (m1 == 2 && d1 == month_len(y1, 2)) {
        d1 = 30;
    }
    (y2 - y1) * 360 + (m2 as i64 - m1 as i64) * 30 + d2 as i64 - d1 as i64
}

/// The coupon period around a settlement date: coupons fall every
/// `12 / f` months back from maturity (on month ends if maturity is one).
struct Coupons {
    pcd: i64,
    ncd: i64,
    /// Coupons left to maturity.
    n: f64,
    /// Days in the period, from its start to settlement, and to its end.
    e: f64,
    a: f64,
    dsc: f64,
}

fn coupons(s: i64, m: i64, f: f64, basis: i64) -> Coupons {
    let step = 12 / f as i64;
    let eom = is_eom(m);
    let at = |k: i64| add_months(m, k * step, eom);
    let k = last_before(at, s, i64::MIN, -(months_between(s, m) / step) - 1);
    let (pcd, ncd) = (at(k), at(k + 1));
    let e = match basis {
        1 => (ncd - pcd) as f64,
        3 => 365.0 / f,
        _ => 360.0 / f,
    };
    let a = basis_days(pcd, s, basis);
    let dsc = match basis {
        0 => days360_both(pcd, ncd) as f64 - a,
        4 => days360(s, ncd, true) as f64,
        _ => (ncd - s) as f64,
    };
    Coupons { pcd, ncd, n: -k as f64, e, a, dsc }
}

fn price(c: &Coupons, f: f64, rate: f64, yld: f64, red: f64) -> f64 {
    let cpn = 100.0 * rate / f;
    let (x, t) = (1.0 + yld / f, c.dsc / c.e);
    let accrued = cpn * c.a / c.e;
    if c.n == 1.0 {
        return (red + cpn) / (1.0 + t * yld / f) - accrued;
    }
    red / x.powf(c.n - 1.0 + t) + cpn / x.powf(t) * discounted(x, c.n) - accrued
}

/// `n` payments of 1, the first undiscounted, discounted by `x` a period.
fn discounted(x: f64, n: f64) -> f64 {
    if (x - 1.0).abs() < 1e-12 { n } else { (1.0 - x.powf(-n)) / (1.0 - 1.0 / x) }
}

/// Macaulay duration in years.
fn duration(c: &Coupons, f: f64, rate: f64, yld: f64) -> f64 {
    let cpn = 100.0 * rate / f;
    let (x, t0) = (1.0 + yld / f, (c.e - c.a) / c.e);
    let (mut num, mut den) = (0.0, 0.0);
    for k in 1..=c.n as i64 {
        let t = k as f64 - 1.0 + t0;
        let v = cpn / x.powf(t);
        num += v * t;
        den += v;
    }
    let t = c.n - 1.0 + t0;
    let v = 100.0 / x.powf(t);
    (num + v * t) / (den + v) / f
}

/// The yield at which `price` gives `pr`.
fn yield_for(price: impl Fn(f64) -> f64, pr: f64, guess: f64) -> R<f64> {
    let g = |y: f64| {
        let h = 1e-7 * y.abs().max(1.0);
        (price(y) - pr, (price(y + h) - price(y - h)) / (2.0 * h))
    };
    finance::solve(g, guess).ok_or(ErrorKind::Num)
}

/// Coupon periods accrued for ACCRINT; coupons fall every `12 / f` months
/// from `fi`. Past `fi`, `from_issue` false (calc_method FALSE) counts the
/// days from the period before `fi` as a single span, as Excel does.
fn accrint(issue: i64, fi: i64, s: i64, f: f64, basis: i64, from_issue: bool) -> f64 {
    let from_issue = from_issue || s <= fi;
    let step = 12 / f as i64;
    let eom = is_eom(fi);
    let at = |k: i64| add_months(fi, k * step, eom);
    let mut k = if s > fi && from_issue { last_before(at, s, 0, months_between(fi, s) / step) } else { -1 };
    let (pcd, ncd) = (at(k), at(k + 1));
    let e = match basis {
        1 => (ncd - pcd) as f64,
        3 => 365.0 / f,
        _ => 360.0 / f,
    };
    let mut acc = basis_days(issue.max(pcd), s, basis) / e;
    // Earlier periods back to issue: whole ones count as one coupon
    // (none when counting from the first interest date).
    let mut late = pcd;
    while late > issue {
        k -= 1;
        let early = at(k);
        acc += if issue <= early {
            if from_issue { 1.0 } else { 0.0 }
        } else {
            let nl = match basis {
                0 => days360_both(early, late) as f64,
                3 => 365.0 / f,
                _ => basis_days(early, late, basis),
            };
            basis_days(issue, late, basis) / nl
        };
        late = early;
    }
    acc
}

/// ODDFPRICE: price with an odd (short or long) first period.
#[allow(clippy::too_many_arguments)]
fn odd_first(s: i64, m: i64, issue: i64, fc: i64, f: f64, basis: i64, rate: f64, yld: f64, red: f64) -> f64 {
    let c = coupons(s, fc, f, basis);
    let (e, n) = (c.e, coupons(fc, m, f, basis).n);
    let (cpn, x) = (100.0 * rate / f, 1.0 + yld / f);
    let regular = |t: f64| cpn / x.powf(1.0 + t) * discounted(x, n);
    let dfc = basis_days(issue, fc, basis);
    if dfc < e {
        let t = basis_days(s, fc, basis) / e;
        let a = basis_days(issue, s, basis);
        return red / x.powf(n + t) + cpn * dfc / e / x.powf(t) + regular(t) - cpn * a / e;
    }
    // Long first period: quasi-coupon periods back from the first coupon.
    let step = 12 / f as i64;
    let eom = is_eom(fc);
    let (mut dc, mut acc, mut late, mut k) = (0.0, 0.0, fc, 0);
    while late > issue {
        k += 1;
        let early = add_months(fc, -k * step, eom);
        let nl = if basis == 1 { (late - early) as f64 } else { e };
        dc += basis_days(issue.max(early), late, basis) / nl;
        let (from, to) = (issue.max(early), s.min(late));
        if from < to {
            acc += basis_days(from, to, basis) / nl;
        }
        late = early;
    }
    let dsc = if basis == 2 || basis == 3 { (c.ncd - s) as f64 } else { e - c.a };
    let t = c.n - 1.0 + dsc / e;
    red / x.powf(n + t) + cpn * dc / x.powf(t) + regular(t) - cpn * acc
}

/// Sums over the quasi-coupon periods of an odd last period (from `li` to
/// maturity): days in it, accrued before settlement and left after it,
/// each over the period's normal length.
fn odd_last(s: i64, m: i64, li: i64, f: f64, basis: i64) -> (f64, f64, f64) {
    let step = 12 / f as i64;
    let eom = is_eom(li);
    let (mut dc, mut acc, mut dsc, mut early, mut k) = (0.0, 0.0, 0.0, li, 0);
    while early < m {
        k += 1;
        let late = add_months(li, k * step, eom);
        let nl = match basis {
            1 => (late - early) as f64,
            3 => 365.0 / f,
            _ => 360.0 / f,
        };
        dc += basis_days(early, late.min(m), basis) / nl;
        if early < s {
            acc += basis_days(early, s.min(late), basis) / nl;
        }
        if late > s {
            dsc += basis_days(s.max(early), late.min(m), basis) / nl;
        }
        early = late;
    }
    (dc, acc, dsc)
}

/// First-period depreciation for AMORLINC/AMORDEGRC (capped at the
/// depreciable amount) and whether it covers part of a year.
fn amor_first(cost: f64, bought: i64, first: i64, salvage: f64, rate: f64, basis: i64) -> (f64, bool) {
    // Excel counts a purchase on 29 February from the 28th.
    let fix = |d: i64| {
        let (y, m, day) = ymd(d);
        if (basis == 1 || basis == 3) && is_leap(y) && m == 2 && day >= 28 { serial(y, 2, 28) } else { d }
    };
    let t = basis_days(fix(bought), fix(first), basis) * rate * cost / year_days(bought, first, basis);
    let (d, part) = if t == 0.0 { (cost * rate, false) } else { (t, true) };
    (d.min(cost - salvage), part)
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    eval(env, f, a).transpose()
}

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Option<Value>> {
    use Func::*;
    let num = |i: usize| env.num(&a[i]);
    let date = |i: usize| date_arg(env, &a[i]);
    Ok(Some(match f {
        AccrInt => {
            need(a, 6, 8)?;
            let (issue, fi, s) = (date(0)?, date(1)?, date(2)?);
            let (rate, par, fr) = (num(3)?, env.opt_num(a, 4, 1000.0)?, freq(env, &a[5])?);
            let (b, from_issue) = (basis(env, a, 6)?, env.opt_bool(a, 7, true)?);
            if rate <= 0.0 || par <= 0.0 || issue >= s {
                return Err(ErrorKind::Num);
            }
            num_ok(par * rate / fr * accrint(issue, fi, s, fr, b, from_issue))
        }
        AccrIntM => {
            need(a, 4, 5)?;
            let (issue, s, rate, par, b) = (date(0)?, date(1)?, num(2)?, env.opt_num(a, 3, 1000.0)?, basis(env, a, 4)?);
            if rate <= 0.0 || par <= 0.0 || issue >= s {
                return Err(ErrorKind::Num);
            }
            num_ok(par * rate * yearfrac(issue, s, b))
        }
        AmorDegrc | AmorLinc => {
            need(a, 6, 7)?;
            let (cost, bought, first, salvage) = (num(0)?, date(1)?, date(2)?, num(3)?);
            let (per, rate, b) = (num(4)?.trunc(), num(5)?, basis(env, a, 6)?);
            if b == 2 || cost < 0.0 || salvage < 0.0 || salvage > cost || per < 0.0 || rate <= 0.0 || first < bought {
                return Err(ErrorKind::Num);
            }
            let life = 1.0 / rate;
            if f == AmorLinc {
                if cost == salvage || per > life.ceil() {
                    return Ok(Some(Value::Number(0.0)));
                }
                let (d0, _) = amor_first(cost, bought, first, salvage, rate, b);
                if per == 0.0 {
                    return Ok(Some(num_ok(d0)));
                }
                let full = cost * rate;
                return Ok(Some(num_ok(full.min((cost - salvage - d0 - (per - 1.0) * full).max(0.0)))));
            }
            // The rate grows by a coefficient set by the asset's life.
            let coef = match life {
                l if (3.0..=4.0).contains(&l) => 1.5,
                l if (5.0..=6.0).contains(&l) => 2.0,
                l if l > 6.0 => 2.5,
                l if l == 1.0 || l == 2.0 => 1.0,
                _ => return Err(ErrorKind::Num),
            };
            let mut r = rate * coef;
            let (d0, part) = amor_first(cost, bought, first, salvage, r, b);
            let d0 = d0.round();
            if per == 0.0 {
                return Ok(Some(Value::Number(d0)));
            }
            // Declining balance; the period before last takes half of what
            // is left and the last one the rest. Only the result is rounded.
            let periods = if part { life + 1.0 } else { life };
            let (mut left, mut dep) = (cost - d0, 0.0);
            for p in 1..=per.min(1e6) as u64 {
                let d = if (periods - p as f64 - 1.0 - 2.0).abs() < 1e-9 {
                    r = 1.0;
                    left * 0.5
                } else {
                    r * left
                };
                dep = if left < salvage { 0.0 } else { d };
                left -= dep;
            }
            num_ok(dep.round())
        }
        CoupDayBs | CoupDays | CoupDaysNc | CoupNcd | CoupNum | CoupPcd => {
            need(a, 3, 4)?;
            let (s, m) = (date(0)?, date(1)?);
            let (fr, b) = (freq(env, &a[2])?, basis(env, a, 3)?);
            if s >= m {
                return Err(ErrorKind::Num);
            }
            let c = coupons(s, m, fr, b);
            Value::Number(match f {
                CoupDayBs => c.a,
                CoupDays => c.e,
                CoupDaysNc => c.dsc,
                CoupNcd => c.ncd as f64,
                CoupPcd => c.pcd as f64,
                _ => c.n,
            })
        }
        Disc | IntRate | Received | PriceDisc | YieldDisc => {
            need(a, 4, 5)?;
            let (s, m) = (date(0)?, date(1)?);
            let (x, y, b) = (num(2)?, num(3)?, basis(env, a, 4)?);
            if s >= m || x <= 0.0 || y <= 0.0 {
                return Err(ErrorKind::Num);
            }
            let yf = yearfrac(s, m, b);
            num_ok(match f {
                // (price, redemption)
                Disc => (y - x) / y / yf,
                YieldDisc => (y - x) / x / yf,
                // (investment, redemption)
                IntRate => (y - x) / x / yf,
                // (investment, discount)
                Received => {
                    let d = 1.0 - y * yf;
                    if d <= 0.0 {
                        return Err(ErrorKind::Num);
                    }
                    x / d
                }
                // (discount, redemption)
                _ => y * (1.0 - x * yf),
            })
        }
        PriceMat | YieldMat => {
            need(a, 5, 6)?;
            let (s, m) = (date(0)?, date(1)?);
            let (issue, rate, x, b) = (date(2)?, num(3)?, num(4)?, basis(env, a, 5)?);
            if s >= m || issue > s || rate < 0.0 || x < 0.0 || (f == YieldMat && x == 0.0) {
                return Err(ErrorKind::Num);
            }
            let yd = year_days(issue, s, b);
            let (dim, a_, dsm) = (basis_days(issue, m, b) / yd, basis_days(issue, s, b) / yd, basis_days(s, m, b) / yd);
            num_ok(if f == PriceMat {
                (100.0 + dim * rate * 100.0) / (1.0 + dsm * x) - a_ * rate * 100.0
            } else {
                let paid = x / 100.0 + a_ * rate;
                (1.0 + dim * rate - paid) / paid / dsm
            })
        }
        TBillEq | TBillPrice | TBillYield => {
            need(a, 3, 3)?;
            let (s, m) = (date(0)?, date(1)?);
            let x = num(2)?;
            if s >= m || m > add_months(s, 12, false) || x <= 0.0 {
                return Err(ErrorKind::Num);
            }
            let dsm = (m - s) as f64;
            num_ok(match f {
                TBillPrice => {
                    let p = 100.0 * (1.0 - x * dsm / 360.0);
                    if p <= 0.0 {
                        return Err(ErrorKind::Num);
                    }
                    p
                }
                TBillYield => (100.0 - x) / x * 360.0 / dsm,
                _ if dsm <= 182.0 => 365.0 * x / (360.0 - x * dsm),
                // Over half a year: the bond-equivalent yield of a
                // semiannual bond (Excel counts a 366-day bill as 365).
                _ => {
                    let p = 1.0 - x * dsm / 360.0;
                    let t = dsm.min(365.0) / 365.0;
                    let d = t * t - (2.0 * t - 1.0) * (1.0 - 1.0 / p);
                    if p <= 0.0 || d < 0.0 {
                        return Err(ErrorKind::Num);
                    }
                    2.0 * (d.sqrt() - t) / (2.0 * t - 1.0)
                }
            })
        }
        Price | Yield => {
            need(a, 6, 7)?;
            let (s, m) = (date(0)?, date(1)?);
            let (rate, x, red, fr, b) = (num(2)?, num(3)?, num(4)?, freq(env, &a[5])?, basis(env, a, 6)?);
            if s >= m || rate < 0.0 || x < 0.0 || red <= 0.0 || (f == Yield && x == 0.0) {
                return Err(ErrorKind::Num);
            }
            let c = coupons(s, m, fr, b);
            if f == Price {
                return Ok(Some(num_ok(price(&c, fr, rate, x, red))));
            }
            if c.n <= 1.0 {
                let cpn = 100.0 * rate / fr;
                let paid = x + cpn * c.a / c.e;
                return Ok(Some(num_ok((red + cpn - paid) / paid * fr * c.e / c.dsc)));
            }
            num_ok(yield_for(|y| price(&c, fr, rate, y, red), x, 0.1)?)
        }
        Duration | MDuration => {
            need(a, 5, 6)?;
            let (s, m) = (date(0)?, date(1)?);
            let (rate, yld, fr, b) = (num(2)?, num(3)?, freq(env, &a[4])?, basis(env, a, 5)?);
            if s >= m || rate < 0.0 || yld < 0.0 {
                return Err(ErrorKind::Num);
            }
            let d = duration(&coupons(s, m, fr, b), fr, rate, yld);
            num_ok(if f == Duration { d } else { d / (1.0 + yld / fr) })
        }
        OddFPrice | OddFYield => {
            need(a, 8, 9)?;
            let (s, m) = (date(0)?, date(1)?);
            let (issue, fc, rate, x, red) = (date(2)?, date(3)?, num(4)?, num(5)?, num(6)?);
            let (fr, b) = (freq(env, &a[7])?, basis(env, a, 8)?);
            if !(m > fc && fc > s && s > issue) || rate < 0.0 || x < 0.0 || red <= 0.0 || (f == OddFYield && x == 0.0) {
                return Err(ErrorKind::Num);
            }
            let p = |y: f64| odd_first(s, m, issue, fc, fr, b, rate, y, red);
            num_ok(if f == OddFPrice { p(x) } else { yield_for(p, x, 0.1)? })
        }
        OddLPrice | OddLYield => {
            need(a, 7, 8)?;
            let (s, m) = (date(0)?, date(1)?);
            let (li, rate, x, red) = (date(2)?, num(3)?, num(4)?, num(5)?);
            let (fr, b) = (freq(env, &a[6])?, basis(env, a, 7)?);
            if !(m > s && s > li) || rate < 0.0 || x < 0.0 || red <= 0.0 || (f == OddLYield && x == 0.0) {
                return Err(ErrorKind::Num);
            }
            let (dc, acc, dsc) = odd_last(s, m, li, fr, b);
            let cpn = 100.0 * rate / fr;
            num_ok(if f == OddLPrice {
                (red + dc * cpn) / (1.0 + dsc * x / fr) - acc * cpn
            } else {
                let paid = x + acc * cpn;
                (red + dc * cpn - paid) / paid * fr / dsc
            })
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use crate::{ErrorKind, Locale, Value, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        w
    }

    fn num(w: &Workbook, f: &str) -> f64 {
        match w.eval_formula(0, f) {
            Value::Number(n) => n,
            v => panic!("{f} gave {v:?}"),
        }
    }

    /// Checks a result to `tol` relative.
    fn close(w: &Workbook, f: &str, want: f64, tol: f64) {
        let got = num(w, f);
        assert!((got - want).abs() <= tol * want.abs().max(1e-3), "{f}: {got} != {want}");
    }

    fn err(w: &Workbook, f: &str, k: ErrorKind) {
        assert_eq!(w.eval_formula(0, f), Value::Error(k), "{f}");
    }

    #[test]
    fn coupons() {
        let w = wb();
        // Excel's examples.
        let args = "DATE(2011,1,25),DATE(2011,11,15),2,1";
        for (f, want) in [("COUPDAYBS", 71.0), ("COUPDAYS", 181.0), ("COUPDAYSNC", 110.0), ("COUPNCD", 40678.0), ("COUPNUM", 2.0), ("COUPPCD", 40497.0)] {
            close(&w, &format!("={f}({args})"), want, 0.0);
        }
        // Month-end maturities keep coupons on month ends (checked with LibreOffice).
        let cases = [
            ("DATE(2014,8,29),DATE(2019,5,31),4,4", [89.0, 90.0, 1.0, 41882.0, 20.0, 41790.0]),
            ("DATE(2013,5,31),DATE(2021,8,31),1,4", [270.0, 360.0, 90.0, 41517.0, 9.0, 41152.0]),
            ("DATE(2006,4,15),DATE(2017,8,31),4,2", [46.0, 90.0, 46.0, 38868.0, 46.0, 38776.0]),
            ("DATE(2010,12,1),DATE(2028,5,31),2,2", [1.0, 180.0, 181.0, 40694.0, 35.0, 40512.0]),
        ];
        for (args, want) in cases {
            for (f, want) in ["COUPDAYBS", "COUPDAYS", "COUPDAYSNC", "COUPNCD", "COUPNUM", "COUPPCD"].iter().zip(want) {
                close(&w, &format!("={f}({args})"), want, 0.0);
            }
        }
        // 30/360 European counts to the next coupon directly.
        close(&w, "=COUPDAYSNC(DATE(2014,3,28),DATE(2017,5,31),4,4)", 62.0, 0.0);
        err(&w, "=COUPNUM(DATE(2011,11,15),DATE(2011,11,15),2,1)", ErrorKind::Num);
        err(&w, "=COUPNUM(DATE(2011,1,25),DATE(2011,11,15),3,1)", ErrorKind::Num);
        err(&w, "=COUPNUM(DATE(2011,1,25),DATE(2011,11,15),2,5)", ErrorKind::Num);
        err(&w, "=COUPNUM(\"x\",DATE(2011,11,15),2,1)", ErrorKind::Value);
    }

    #[test]
    fn bonds() {
        let w = wb();
        close(&w, "=PRICE(DATE(2008,2,15),DATE(2017,11,15),0.0575,0.065,100,2,0)", 94.634_361_621_322_1, 1e-10);
        close(&w, "=YIELD(DATE(2008,2,15),DATE(2016,11,15),0.0575,95.04287,100,2,0)", 0.065_000_006_880_755_2, 1e-9);
        close(&w, "=DURATION(DATE(2018,7,1),DATE(2048,1,1),0.08,0.09,2,1)", 10.919_145_3, 1e-8);
        close(&w, "=MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)", 5.735_67, 1e-6);
        // One coupon left: simple interest over the rest of the period.
        close(&w, "=PRICE(DATE(2010,9,1),DATE(2010,10,4),0.05,0.06,100,2,0)", 102.5 / (1.0 + 33.0 / 180.0 * 0.03) - 2.5 * 147.0 / 180.0, 1e-12);
        for b in 0..5 {
            for (s, m) in [("DATE(2008,2,15)", "DATE(2017,11,15)"), ("DATE(2010,9,1)", "DATE(2010,10,4)"), ("DATE(2012,2,29)", "DATE(2030,8,31)")] {
                let p = num(&w, &format!("=PRICE({s},{m},0.05,0.07,100,4,{b})"));
                close(&w, &format!("=YIELD({s},{m},0.05,{p},100,4,{b})"), 0.07, 1e-9);
            }
        }
        err(&w, "=PRICE(DATE(2008,2,15),DATE(2017,11,15),-0.0575,0.065,100,2,0)", ErrorKind::Num);
        err(&w, "=PRICE(DATE(2008,2,15),DATE(2017,11,15),0.0575,0.065,0,2,0)", ErrorKind::Num);
        err(&w, "=YIELD(DATE(2008,2,15),DATE(2016,11,15),0.0575,0,100,2,0)", ErrorKind::Num);
        err(&w, "=DURATION(DATE(2018,7,1),DATE(2018,7,1),0.08,0.09,2,1)", ErrorKind::Num);
    }

    #[test]
    fn accrued_interest() {
        let w = wb();
        close(&w, "=ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0)", 16.666_666_666_666_7, 1e-12);
        close(&w, "=ACCRINT(DATE(2008,3,5),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0,FALSE)", 15.555_555_555_555_6, 1e-12);
        close(&w, "=ACCRINT(DATE(2008,4,5),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0,TRUE)", 7.222_222_222_222_22, 1e-12);
        // Settled after the first interest date: FALSE counts one span of days.
        close(&w, "=ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,9,10),0.1,1000,2,0,TRUE)", 52.777_777_777_777_8, 1e-12);
        close(&w, "=ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,9,10),0.1,1000,2,0,FALSE)", 52.5, 1e-12);
        close(&w, "=ACCRINT(DATE(2002,9,28),DATE(2005,6,24),DATE(2004,11,28),0.1,1000,4,0)", 216.666_666_666_667, 1e-12);
        close(&w, "=ACCRINT(DATE(2002,9,28),DATE(2005,6,24),DATE(2004,11,28),0.1,,4,0)", 216.666_666_666_667, 1e-12);
        close(&w, "=ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1,1000,3)", 20.547_945_205_479_5, 1e-12);
        err(&w, "=ACCRINT(DATE(2008,5,1),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0)", ErrorKind::Num);
        err(&w, "=ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,5,1),0,1000,2,0)", ErrorKind::Num);
        err(&w, "=ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1,-1,3)", ErrorKind::Num);
    }

    #[test]
    fn discounted_and_at_maturity() {
        let w = wb();
        close(&w, "=DISC(DATE(2018,1,25),DATE(2018,6,15),97.975,100,1)", 0.052_420_212_765_957_7, 1e-10);
        close(&w, "=INTRATE(DATE(2008,2,15),DATE(2008,5,15),1000000,1014420,2)", 0.057_68, 1e-10);
        close(&w, "=RECEIVED(DATE(2008,2,15),DATE(2008,5,15),1000000,0.0575,2)", 1_014_584.654_407_1, 1e-10);
        close(&w, "=PRICEDISC(DATE(2008,2,16),DATE(2008,3,1),0.0525,100,2)", 99.795_833_333_333_3, 1e-12);
        close(&w, "=YIELDDISC(DATE(2008,2,16),DATE(2008,3,1),99.795,100,2)", 0.052_822_571_986_860_1, 1e-10);
        close(&w, "=PRICEMAT(DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),0.061,0.061,0)", 99.984_498_875_557, 1e-10);
        close(&w, "=YIELDMAT(DATE(2008,3,15),DATE(2008,11,3),DATE(2007,11,8),0.0625,100.0123,0)", 0.060_954_333_691_538_6, 1e-10);
        err(&w, "=DISC(DATE(2018,6,15),DATE(2018,6,15),97.975,100,1)", ErrorKind::Num);
        err(&w, "=RECEIVED(DATE(2008,2,15),DATE(2008,5,15),1000000,0,2)", ErrorKind::Num);
        err(&w, "=PRICEMAT(DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),-0.061,0.061,0)", ErrorKind::Num);
    }

    #[test]
    fn treasury_bills() {
        let w = wb();
        close(&w, "=TBILLPRICE(DATE(2008,3,31),DATE(2008,6,1),0.09)", 98.45, 1e-12);
        close(&w, "=TBILLYIELD(DATE(2008,3,31),DATE(2008,6,1),98.45)", 0.091_416_962_925_342_3, 1e-10);
        close(&w, "=TBILLEQ(DATE(2008,3,31),DATE(2008,6,1),0.0914)", 0.094_151_493_565_943, 1e-10);
        // Over half a year: a semiannual bond's yield.
        let (t, p): (f64, f64) = (300.0 / 365.0, 1.0 - 0.09 * 300.0 / 360.0);
        close(&w, "=TBILLEQ(DATE(2008,1,1),DATE(2008,1,1)+300,0.09)", 2.0 * ((t * t - (2.0 * t - 1.0) * (1.0 - 1.0 / p)).sqrt() - t) / (2.0 * t - 1.0), 1e-12);
        err(&w, "=TBILLPRICE(DATE(2008,3,31),DATE(2009,4,1),0.09)", ErrorKind::Num);
        err(&w, "=TBILLYIELD(DATE(2008,3,31),DATE(2008,3,31),98.45)", ErrorKind::Num);
        err(&w, "=TBILLEQ(DATE(2008,3,31),DATE(2008,6,1),0)", ErrorKind::Num);
    }

    #[test]
    fn odd_periods() {
        let w = wb();
        close(&w, "=ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),0.0785,0.0625,100,2,1)", 113.597_717, 1e-8);
        close(&w, "=ODDFYIELD(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),0.0575,84.5,100,2,0)", 0.0772, 1e-3);
        close(&w, "=ODDLPRICE(DATE(2008,2,7),DATE(2008,6,15),DATE(2007,10,15),0.0375,0.0405,100,2,0)", 99.878_286_014_721_3, 1e-10);
        close(&w, "=ODDLYIELD(DATE(2008,4,20),DATE(2008,6,15),DATE(2007,12,24),0.0375,99.875,100,2,0)", 0.045_192_235_629_169, 1e-10);
        // Checked with LibreOffice (its actual/actual differs).
        close(&w, "=ODDLPRICE(DATE(2009,8,28),DATE(2009,9,13),DATE(2008,12,29),0.0375,0.0405,100,2,2)", 99.982_161_276_369_2, 1e-10);
        close(&w, "=ODDLYIELD(DATE(2008,11,7),DATE(2009,11,26),DATE(2008,10,31),0.0375,99.875,100,1,3)", 0.038_708_698_467_222_9, 1e-9);
        close(&w, "=ODDLPRICE(DATE(2009,5,2),DATE(2010,4,13),DATE(2008,12,31),0.0375,0.0405,100,1,4)", 99.679_380_745_725_4, 1e-10);
        // A first period of regular length prices like a regular bond
        // (with actual days over 360 or 365 it is not regular).
        for b in [0, 1, 4] {
            let p = num(&w, &format!("=PRICE(DATE(2008,11,11),DATE(2021,3,1),0.0785,0.0625,100,2,{b})"));
            close(&w, &format!("=ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,9,1),DATE(2009,3,1),0.0785,0.0625,100,2,{b})"), p, 1e-9);
        }
        // Long first period: the yield gives the price back.
        for b in 0..5 {
            let p = num(&w, &format!("=ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2007,10,15),DATE(2009,9,1),0.0785,0.0625,100,2,{b})"));
            close(&w, &format!("=ODDFYIELD(DATE(2008,11,11),DATE(2021,3,1),DATE(2007,10,15),DATE(2009,9,1),0.0785,{p},100,2,{b})"), 0.0625, 1e-9);
        }
        err(&w, "=ODDFPRICE(DATE(2008,10,1),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),0.0785,0.0625,100,2,1)", ErrorKind::Num);
        err(&w, "=ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),0.0785,-0.0625,100,2,1)", ErrorKind::Num);
        err(&w, "=ODDLPRICE(DATE(2007,10,1),DATE(2008,6,15),DATE(2007,10,15),0.0375,0.0405,100,2,0)", ErrorKind::Num);
    }

    #[test]
    fn french_depreciation() {
        let w = wb();
        close(&w, "=AMORDEGRC(2400,DATE(2008,8,19),DATE(2008,12,31),300,1,0.15,1)", 776.0, 0.0);
        close(&w, "=AMORDEGRC(2800,DATE(2003,9,4),DATE(2006,12,31),200,1,0.02,1)", 117.0, 0.0);
        // Excel's schedules: half the rest in the period before last, all of it in the last.
        let table = [(0, [4146.0, 5945.0, 4954.0, 4954.0, 0.0]), (1, [4139.0, 5948.0, 4957.0, 4957.0, 0.0]), (3, [4151.0, 5943.0, 4953.0, 4953.0, 0.0]), (4, [4125.0, 5953.0, 4961.0, 4961.0, 0.0])];
        for (b, want) in table {
            for (p, want) in want.iter().enumerate() {
                close(&w, &format!("=AMORDEGRC(20000,DATE(2020,6,12),DATE(2020,12,31),4000,{p},0.25,{b})"), *want, 0.0);
            }
        }
        for (p, want) in [2000.0, 3200.0, 1920.0, 1440.0].iter().enumerate() {
            close(&w, &format!("=AMORDEGRC(10000,DATE(2019,6,30),DATE(2019,12,31),1000,{p},0.2,4)"), *want, 0.0);
        }
        close(&w, "=AMORLINC(2400,DATE(2008,8,19),DATE(2008,12,31),300,1,0.15,1)", 360.0, 1e-12);
        close(&w, "=AMORLINC(2400,DATE(2008,8,19),DATE(2008,12,31),300,0,0.15,1)", 2400.0 * 0.15 * 134.0 / 366.0, 1e-12);
        close(&w, "=AMORLINC(2400,DATE(2008,8,19),DATE(2008,12,31),300,6,0.15,1)", 2100.0 - 1800.0 - 2400.0 * 0.15 * 134.0 / 366.0, 1e-12);
        close(&w, "=AMORLINC(2400,DATE(2008,8,19),DATE(2008,12,31),300,7,0.15,1)", 0.0, 0.0);
        // Bought on the first period's date: a whole first period.
        close(&w, "=AMORLINC(2400,DATE(2008,12,31),DATE(2008,12,31),300,0,0.15,1)", 360.0, 1e-12);
        // Lives between 4 and 5 years have no coefficient; basis 2 is not allowed.
        err(&w, "=AMORDEGRC(2400,DATE(2008,8,19),DATE(2008,12,31),300,1,0.22,1)", ErrorKind::Num);
        err(&w, "=AMORDEGRC(2400,DATE(2008,8,19),DATE(2008,12,31),300,1,0.15,2)", ErrorKind::Num);
        err(&w, "=AMORLINC(2400,DATE(2008,8,19),DATE(2008,12,31),3000,1,0.15,1)", ErrorKind::Num);
    }

    #[test]
    fn spanish_names() {
        let mut w = wb();
        w.locale = Locale::ES;
        close(&w, "=CUPON.NUM(FECHA(2011;1;25);FECHA(2011;11;15);2;1)", 2.0, 0.0);
        close(&w, "=PRECIO(FECHA(2008;2;15);FECHA(2017;11;15);0,0575;0,065;100;2;0)", 94.634_361_621_322_1, 1e-10);
        close(&w, "=LETRA.DE.TES.PRECIO(FECHA(2008;3;31);FECHA(2008;6;1);0,09)", 98.45, 1e-12);
        close(&w, "=DURACION(FECHA(2018;7;1);FECHA(2048;1;1);0,08;0,09;2;1)", 10.919_145_3, 1e-8);
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        let w = wb();
        for f in [
            "=PRICE(0,2958465,0.05,0.05,100,4,1)", "=YIELD(0,2958465,0.05,1E+300,100,4,1)", "=DURATION(0,2958465,1E+300,1E+300,4,0)",
            "=ACCRINT(0,2958465,2958465,0.1,1000,4,1)", "=ACCRINT(0,1,2958465,0.1,1000,4,1,FALSE)", "=COUPNUM(0,2958465,4,1)",
            "=ODDFPRICE(3,2958465,1,2,0.05,0.05,100,4,1)", "=ODDFYIELD(3,2958465,1,2958464,0.05,1E-300,100,4,1)", "=ODDLYIELD(2958464,2958465,0,0.05,100,100,4,1)",
            "=AMORDEGRC(1E+300,0,2958465,0,1E+300,1E-300,1)", "=AMORLINC(1,0,2958465,0,1E+300,1E-300,0)", "=TBILLEQ(1,2,1E+300)", "=YIELDMAT(1,2958465,0,1E+300,1E-300,1)",
        ] {
            let _ = w.eval_formula(0, f);
        }
    }
}

