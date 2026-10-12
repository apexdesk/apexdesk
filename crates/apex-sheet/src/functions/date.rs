//! Spreadsheet functions: date.

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    match f {
        NetworkDays | Workday => Some(0b100),
        NetworkDaysIntl | WorkdayIntl => Some(0b1000),
        _ => None,
    }
}

/// Largest valid serial date (9999-12-31).
const MAX_SERIAL: f64 = 2_958_465.0;

fn need(a: &[Expr], min: usize, max: usize) -> R<()> {
    if a.len() < min || a.len() > max { Err(ErrorKind::Value) } else { Ok(()) }
}

pub(super) fn ymd(d: i64) -> (i64, u32, u32) {
    format::serial_to_date(d as f64)
}

pub(super) fn serial(y: i64, m: i64, d: i64) -> i64 {
    format::date_to_serial(y, m, d) as i64
}

pub(super) fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub(super) fn month_len(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Day of the week, Monday = 0.
fn wd(d: i64) -> usize {
    (d + 5).rem_euclid(7) as usize
}

/// A date argument: a number, or text that reads as a date or a number.
/// Time of day is dropped.
pub(super) fn date_arg(env: &Env, e: &Expr) -> R<i64> {
    let n = match env.scalar(e) {
        Value::Text(s) => match parse_when(&s, &env.locale) {
            Some((_, n)) => n,
            None => Value::Text(s).to_number()?,
        },
        Value::Bool(_) => return Err(ErrorKind::Value),
        v => v.to_number()?,
    };
    if !(0.0..MAX_SERIAL + 1.0).contains(&n) {
        return Err(ErrorKind::Num);
    }
    Ok(n.floor() as i64)
}

const MONTHS: [[&str; 12]; 2] = [
    ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"],
    ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
];

fn month_name(t: &str) -> Option<i64> {
    if t.chars().count() < 3 {
        return None;
    }
    MONTHS.iter().find_map(|names| names.iter().position(|n| n.starts_with(t)).map(|i| i as i64 + 1))
}

/// Text a formula takes as a number: a number as typed in `loc` ("1,234",
/// "10%", "$5"), a date or a time.
pub(crate) fn text_number(s: &str, loc: &Locale) -> Option<f64> {
    match format::parse_input(s, loc).value {
        Value::Number(n) => Some(n),
        _ => parse_when(s, loc).map(|(_, n)| n),
    }
}

/// Reads a date and/or time typed as text in the workbook locale. Returns
/// whether a date part was present and the serial number.
fn parse_when(s: &str, loc: &Locale) -> Option<(bool, f64)> {
    let p = format::parse_input(s, loc);
    if let (Value::Number(n), Some(code)) = (&p.value, &p.format) {
        if code.contains("yy") {
            return Some((true, *n));
        }
        if code.starts_with("hh") {
            return Some((false, *n));
        }
    }
    let mut t = s.trim().to_lowercase();
    // 12-hour clock suffix: "pm", "p.m.", "p. m."
    let mut half = None;
    for (suffix, pm) in [("a. m.", false), ("p. m.", true), ("a.m.", false), ("p.m.", true), ("am", false), ("pm", true)] {
        if let Some(r) = t.strip_suffix(suffix) {
            half = Some(pm);
            t = r.trim_end().to_string();
            break;
        }
    }
    let mut time = None;
    let mut date_toks: Vec<&str> = Vec::new();
    for tok in t.split_whitespace() {
        if tok.contains(':') {
            if time.is_some() {
                return None;
            }
            time = Some(parse_clock(tok, half)?);
        } else {
            date_toks.extend(tok.split(['/', '-', ',', '.']).filter(|x| !x.is_empty() && *x != "de" && *x != "del"));
        }
    }
    if time.is_none() && half.is_some() {
        // "3 pm"
        let [h] = date_toks.as_slice() else { return None };
        return Some((false, parse_clock(h, half)?));
    }
    if date_toks.is_empty() {
        return time.map(|t| (false, t));
    }
    let time = time.unwrap_or(0.0);
    let mut month = None;
    let mut nums: Vec<(i64, usize)> = Vec::new();
    for tok in &date_toks {
        if tok.chars().all(|c| c.is_ascii_digit()) {
            if tok.len() > 4 {
                return None;
            }
            nums.push((tok.parse().ok()?, tok.len()));
        } else if month.is_none() {
            month = Some(month_name(tok)?);
        } else {
            return None;
        }
    }
    let this_year = format::serial_to_date(format::now_serial()).0;
    let year = |(v, len): (i64, usize)| if len <= 2 { if v < 30 { 2000 + v } else { 1900 + v } } else { v };
    let (y, m, d) = match (month, nums.as_slice()) {
        (Some(m), [a, b]) if a.1 == 4 => (year(*a), m, b.0),
        (Some(m), [a, b]) => (year(*b), m, a.0),
        (Some(m), [a]) if a.1 == 4 || a.0 > 31 => (year(*a), m, 1),
        (Some(m), [a]) => (this_year, m, a.0),
        (None, [a, b]) if b.1 == 4 || b.0 > 31 => (year(*b), a.0, 1),
        (None, [a, b]) => match loc.date_order {
            format::DateOrder::Dmy => (this_year, b.0, a.0),
            _ => (this_year, a.0, b.0),
        },
        (None, [a, b, c]) if a.1 == 4 => (a.0, b.0, c.0),
        (None, [a, b, c]) => match loc.date_order {
            format::DateOrder::Dmy => (year(*c), b.0, a.0),
            format::DateOrder::Mdy => (year(*c), a.0, b.0),
            format::DateOrder::Ymd => (year(*a), b.0, c.0),
        },
        _ => return None,
    };
    if !(1900..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > month_len(y, m as u32) as i64 {
        return None;
    }
    Some((true, serial(y, m, d) as f64 + time))
}

/// "h:mm[:ss[.fff]]" as a fraction of a day; `half` is Some(pm) on a 12-hour clock.
fn parse_clock(s: &str, half: Option<bool>) -> Option<f64> {
    let p: Vec<&str> = s.split(':').collect();
    if p.len() > 3 || p.iter().any(|x| x.is_empty()) {
        return None;
    }
    let int = |x: &str| if x.len() <= 2 && x.chars().all(|c| c.is_ascii_digit()) { x.parse::<f64>().ok() } else { None };
    let mut h = int(p[0])?;
    let m = p.get(1).map_or(Some(0.0), |x| int(x))?;
    let sec = match p.get(2) {
        Some(x) if x.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') => x.replace(',', ".").parse::<f64>().ok()?,
        Some(_) => return None,
        None => 0.0,
    };
    if let Some(pm) = half {
        if !(1.0..=12.0).contains(&h) {
            return None;
        }
        h = h % 12.0 + if pm { 12.0 } else { 0.0 };
    }
    if h >= 24.0 || m >= 60.0 || sec >= 60.0 {
        return None;
    }
    Some((h * 3600.0 + m * 60.0 + sec) / 86_400.0)
}

/// Weekend days (Monday first) from a NETWORKDAYS.INTL/WORKDAY.INTL code
/// (1-7, 11-17) or a "0000011" mask.
fn weekend(env: &Env, a: &[Expr], i: usize) -> R<[bool; 7]> {
    let code = match a.get(i) {
        None | Some(Expr::Missing) => 1,
        Some(e) => match env.scalar(e) {
            Value::Text(s) => {
                let b = s.as_bytes();
                if b.len() != 7 || !b.iter().all(|c| *c == b'0' || *c == b'1') || b.iter().all(|c| *c == b'1') {
                    return Err(ErrorKind::Value);
                }
                return Ok(std::array::from_fn(|k| b[k] == b'1'));
            }
            Value::Bool(_) => return Err(ErrorKind::Value),
            v => v.to_number()?.trunc() as i64,
        },
    };
    let mut w = [false; 7];
    match code {
        1..=7 => {
            w[(code as usize + 4) % 7] = true;
            w[(code as usize + 5) % 7] = true;
        }
        11..=17 => w[(code as usize - 11 + 6) % 7] = true,
        _ => return Err(ErrorKind::Num),
    }
    Ok(w)
}

/// Holiday dates, sorted and without repeats.
fn holidays(env: &Env, a: &[Expr], i: usize) -> R<Vec<i64>> {
    let mut out = Vec::new();
    let Some(e) = a.get(i).filter(|e| !matches!(e, Expr::Missing)) else { return Ok(out) };
    let mut err = None;
    env.each(e, &mut |v, _| {
        let d = match v {
            Value::Empty => return,
            Value::Number(n) if (0.0..MAX_SERIAL + 1.0).contains(n) => Ok(n.floor() as i64),
            Value::Number(_) => Err(ErrorKind::Num),
            Value::Text(s) => parse_when(s, &env.locale).map(|(_, n)| n.floor() as i64).ok_or(ErrorKind::Value),
            Value::Error(k) => Err(*k),
            _ => Err(ErrorKind::Value),
        };
        match d {
            Ok(d) => out.push(d),
            Err(k) => {
                err.get_or_insert(k);
            }
        }
    });
    if let Some(k) = err {
        return Err(k);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Working days in `lo..=hi`.
fn count_work(lo: i64, hi: i64, wk: &[bool; 7], hol: &[i64]) -> i64 {
    let n = hi - lo + 1;
    let per_week = wk.iter().filter(|w| !**w).count() as i64;
    let mut c = n / 7 * per_week;
    c += (lo + n / 7 * 7..=hi).filter(|d| !wk[wd(*d)]).count() as i64;
    c - hol.iter().filter(|h| (lo..=hi).contains(*h) && !wk[wd(**h)]).count() as i64
}

fn workday(start: i64, days: f64, wk: &[bool; 7], hol: &[i64]) -> R<i64> {
    if days.abs() > 1e7 {
        return Err(ErrorKind::Num);
    }
    let days = days.trunc() as i64;
    if days == 0 {
        return Ok(start);
    }
    let step = days.signum();
    let per_week = wk.iter().filter(|w| !**w).count() as i64;
    let mut left = days.abs();
    // Jump whole weeks, then walk; holidays jumped over cost extra steps.
    let weeks = (left - 1) / per_week;
    let mut d = start + step * weeks * 7;
    left -= weeks * per_week;
    let (lo, hi) = if step > 0 { (start + 1, d) } else { (d, start - 1) };
    left += hol.iter().filter(|h| (lo..=hi).contains(*h) && !wk[wd(**h)]).count() as i64;
    while left > 0 {
        d += step;
        if !wk[wd(d)] && hol.binary_search(&d).is_err() {
            left -= 1;
        }
    }
    if !(0..=MAX_SERIAL as i64).contains(&d) {
        return Err(ErrorKind::Num);
    }
    Ok(d)
}

/// 30/360 day count. US (NASD) rules unless `euro`.
pub(super) fn days360(s: i64, e: i64, euro: bool) -> i64 {
    let ((y1, m1, mut d1), (y2, m2, mut d2)) = (ymd(s), ymd(e));
    if euro {
        d1 = d1.min(30);
        d2 = d2.min(30);
    } else {
        if d1 == 31 || (m1 == 2 && d1 == month_len(y1, 2)) {
            d1 = 30;
        }
        if d2 == 31 && d1 >= 30 {
            d2 = 30;
        }
    }
    (y2 - y1) * 360 + (m2 as i64 - m1 as i64) * 30 + d2 as i64 - d1 as i64
}

/// 30/360 US days as YEARFRAC basis 0 counts them.
pub(super) fn days360_us(s: i64, e: i64) -> i64 {
    let ((y1, m1, mut d1), (y2, m2, mut d2)) = (ymd(s), ymd(e));
    let last_feb = |y, m, d| m == 2 && d == month_len(y, 2);
    if d1 == 31 && d2 == 31 {
        d1 = 30;
        d2 = 30;
    } else if d1 == 31 {
        d1 = 30;
    } else if d1 == 30 && d2 == 31 {
        d2 = 30;
    } else if last_feb(y1, m1, d1) && last_feb(y2, m2, d2) {
        d1 = 30;
        d2 = 30;
    } else if last_feb(y1, m1, d1) {
        d1 = 30;
    }
    (y2 - y1) * 360 + (m2 as i64 - m1 as i64) * 30 + d2 as i64 - d1 as i64
}

/// Days from `s` to `e` counted with a day-count basis.
pub(super) fn basis_days(s: i64, e: i64, basis: i64) -> f64 {
    match basis {
        0 => days360_us(s, e) as f64,
        4 => days360(s, e, true) as f64,
        _ => (e - s) as f64,
    }
}

/// Days in the year for a day-count basis; actual/actual looks at the
/// years `s..=e` span (start <= end).
pub(super) fn year_days(s: i64, e: i64, basis: i64) -> f64 {
    match basis {
        1 => {
            let ((y1, m1, d1), (y2, m2, d2)) = (ymd(s), ymd(e));
            let within_year = y1 == y2 || (y2 == y1 + 1 && (m1 > m2 || (m1 == m2 && d1 >= d2)));
            if within_year {
                let feb29 = |y: i64| is_leap(y) && s < serial(y, 3, 1) && e >= serial(y, 3, 1);
                if (y1 == y2 && is_leap(y1)) || feb29(y1) || feb29(y2) || (m2 == 2 && d2 == 29) { 366.0 } else { 365.0 }
            } else {
                (serial(y2 + 1, 1, 1) - serial(y1, 1, 1)) as f64 / (y2 - y1 + 1) as f64
            }
        }
        3 => 365.0,
        _ => 360.0,
    }
}

/// YEARFRAC with Excel's day-count bases (start <= end).
pub(super) fn yearfrac(s: i64, e: i64, basis: i64) -> f64 {
    basis_days(s, e, basis) / year_days(s, e, basis)
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    eval(env, f, a).transpose()
}

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Option<Value>> {
    use Func::*;
    Ok(Some(match f {
        NetworkDays | NetworkDaysIntl => {
            let intl = f == NetworkDaysIntl;
            need(a, 2, if intl { 4 } else { 3 })?;
            let (s, e) = (date_arg(env, &a[0])?, date_arg(env, &a[1])?);
            let wk = if intl { weekend(env, a, 2)? } else { weekend(env, &[], 0)? };
            let hol = holidays(env, a, if intl { 3 } else { 2 })?;
            Value::Number(if s <= e { count_work(s, e, &wk, &hol) } else { -count_work(e, s, &wk, &hol) } as f64)
        }
        Workday | WorkdayIntl => {
            let intl = f == WorkdayIntl;
            need(a, 2, if intl { 4 } else { 3 })?;
            let s = date_arg(env, &a[0])?;
            let days = env.num(&a[1])?;
            let wk = if intl { weekend(env, a, 2)? } else { weekend(env, &[], 0)? };
            let hol = holidays(env, a, if intl { 3 } else { 2 })?;
            Value::Number(workday(s, days, &wk, &hol)? as f64)
        }
        DateDif => {
            need(a, 3, 3)?;
            let (s, e) = (date_arg(env, &a[0])?, date_arg(env, &a[1])?);
            let unit = env.text(&a[2])?.to_ascii_uppercase();
            if s > e {
                return Err(ErrorKind::Num);
            }
            let ((y1, m1, d1), (y2, m2, d2)) = (ymd(s), ymd(e));
            let months = (y2 - y1) * 12 + m2 as i64 - m1 as i64 - i64::from(d2 < d1);
            Value::Number(match unit.as_str() {
                "Y" => months / 12,
                "M" => months,
                "D" => e - s,
                "YM" => months % 12,
                // Excel counts from day d1 of the previous month, which can go negative.
                "MD" if d2 >= d1 => (d2 - d1) as i64,
                "MD" => e - serial(y2, m2 as i64 - 1, d1 as i64),
                "YD" => {
                    let anniv = serial(y2, m1 as i64, d1 as i64);
                    if anniv <= e { e - anniv } else { e - serial(y2 - 1, m1 as i64, d1 as i64) }
                }
                _ => return Err(ErrorKind::Num),
            } as f64)
        }
        WeekNum => {
            need(a, 1, 2)?;
            let d = date_arg(env, &a[0])?;
            let first = match env.opt_num(a, 1, 1.0)?.trunc() as i64 {
                1 | 17 => 6,
                2 | 11 => 0,
                t @ 12..=16 => (t - 11) as usize,
                21 => return Ok(Some(Value::Number(iso_week(d) as f64))),
                _ => return Err(ErrorKind::Num),
            };
            let jan1 = serial(ymd(d).0, 1, 1);
            let offset = (wd(jan1) as i64 - first as i64).rem_euclid(7);
            Value::Number(((d - jan1 + offset) / 7 + 1) as f64)
        }
        IsoWeekNum => {
            need(a, 1, 1)?;
            Value::Number(iso_week(date_arg(env, &a[0])?) as f64)
        }
        YearFrac => {
            need(a, 2, 3)?;
            let (s, e) = (date_arg(env, &a[0])?, date_arg(env, &a[1])?);
            let basis = env.opt_num(a, 2, 0.0)?.trunc();
            if !(0.0..=4.0).contains(&basis) {
                return Err(ErrorKind::Num);
            }
            Value::Number(yearfrac(s.min(e), s.max(e), basis as i64))
        }
        Time => {
            need(a, 3, 3)?;
            let (h, m, s) = (env.num(&a[0])?.trunc(), env.num(&a[1])?.trunc(), env.num(&a[2])?.trunc());
            if h.abs() > 32767.0 || m.abs() > 32767.0 || s.abs() > 32767.0 {
                return Err(ErrorKind::Num);
            }
            let total = h * 3600.0 + m * 60.0 + s;
            if total < 0.0 {
                return Err(ErrorKind::Num);
            }
            Value::Number(total.rem_euclid(86_400.0) / 86_400.0)
        }
        DateValue | TimeValue => {
            need(a, 1, 1)?;
            let s = match env.scalar(&a[0]) {
                Value::Text(s) => s,
                Value::Error(k) => return Err(k),
                _ => return Err(ErrorKind::Value),
            };
            let (has_date, n) = parse_when(&s, &env.locale).ok_or(ErrorKind::Value)?;
            if f == DateValue {
                if !has_date {
                    return Err(ErrorKind::Value);
                }
                Value::Number(n.floor())
            } else {
                Value::Number(n - n.floor())
            }
        }
        Days360 => {
            need(a, 2, 3)?;
            let (s, e) = (date_arg(env, &a[0])?, date_arg(env, &a[1])?);
            Value::Number(days360(s, e, env.opt_bool(a, 2, false)?) as f64)
        }
        _ => return Ok(None),
    }))
}

fn iso_week(d: i64) -> i64 {
    let thursday = d - wd(d) as i64 + 3;
    let jan1 = serial(ymd(thursday).0, 1, 1);
    (thursday - jan1) / 7 + 1
}

#[cfg(test)]
mod tests {
    use crate::{CellRef, ErrorKind, Locale, Value, Workbook};

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

    fn close(w: &Workbook, f: &str, want: f64) {
        let got = num(w, f);
        assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{f}: {got} != {want}");
    }

    fn err(w: &Workbook, f: &str, k: ErrorKind) {
        assert_eq!(w.eval_formula(0, f), Value::Error(k), "{f}");
    }

    #[test]
    fn network_days() {
        let mut w = wb();
        for (r, d) in ["2012-11-22", "2012-12-04", "2013-01-21"].iter().enumerate() {
            w.set_input(0, CellRef::new(r as u32, 0), d);
        }
        close(&w, "=NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1))", 110.0);
        close(&w, "=NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1),A1)", 109.0);
        close(&w, "=NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1),A1:A3)", 107.0);
        close(&w, "=NETWORKDAYS(DATE(2013,3,1),DATE(2012,10,1),A1:A3)", -107.0);
        close(&w, "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31))", 22.0);
        close(&w, "=NETWORKDAYS.INTL(DATE(2006,1,31),DATE(2006,1,1))", -22.0);
        close(&w, "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),7,{\"2006/1/2\",\"2006/1/16\"})", 22.0);
        close(&w, "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),\"0010001\",{\"2006/1/2\",\"2006/1/16\"})", 20.0);
        close(&w, "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31),11)", 26.0);
        err(&w, "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31),8)", ErrorKind::Num);
        err(&w, "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31),\"1111111\")", ErrorKind::Value);
        err(&w, "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,1,31),\"001\")", ErrorKind::Value);
        err(&w, "=NETWORKDAYS(1)", ErrorKind::Value);
        // Arrays in single-value arguments repeat the call.
        assert!(matches!(w.eval_formula(0, "=NETWORKDAYS(DATE(2006,1,1),{38748,38776})"), Value::Array(_)));
    }

    #[test]
    fn workdays() {
        let mut w = wb();
        for (r, d) in ["2008-11-26", "2008-12-04", "2009-01-21"].iter().enumerate() {
            w.set_input(0, CellRef::new(r as u32, 0), d);
        }
        close(&w, "=WORKDAY(DATE(2008,10,1),151)", 39933.0);
        close(&w, "=WORKDAY(DATE(2008,10,1),151,A1:A3)", 39938.0);
        close(&w, "=WORKDAY(DATE(2009,5,5),-151,A1:A3)", 39722.0);
        close(&w, "=WORKDAY(DATE(2008,10,4),0)", 39725.0);
        close(&w, "=WORKDAY(DATE(2008,10,4),1)", 39727.0);
        err(&w, "=WORKDAY.INTL(DATE(2012,1,1),30,0)", ErrorKind::Num);
        close(&w, "=WORKDAY.INTL(DATE(2012,1,1),90,11)", 41013.0);
        close(&w, "=WORKDAY.INTL(DATE(2012,1,1),30,17)", 40944.0);
        close(&w, "=WORKDAY.INTL(DATE(2012,1,1),30,\"0000011\")", num(&w, "=WORKDAY(DATE(2012,1,1),30)"));
        // Long spans agree with counting.
        close(&w, "=NETWORKDAYS(DATE(2008,10,1),WORKDAY(DATE(2008,10,1),5000,A1:A3),A1:A3)", 5001.0);
        err(&w, "=WORKDAY(DATE(2008,10,1),1E+9)", ErrorKind::Num);
    }

    #[test]
    fn datedif() {
        let w = wb();
        close(&w, "=DATEDIF(DATE(2001,1,1),DATE(2003,1,1),\"Y\")", 2.0);
        close(&w, "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"D\")", 440.0);
        close(&w, "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"YD\")", 75.0);
        close(&w, "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"M\")", 14.0);
        close(&w, "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"ym\")", 2.0);
        close(&w, "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"MD\")", 14.0);
        close(&w, "=DATEDIF(DATE(2020,1,15),DATE(2020,3,10),\"MD\")", 24.0);
        close(&w, "=DATEDIF(DATE(2020,1,31),DATE(2020,2,29),\"M\")", 0.0);
        close(&w, "=DATEDIF(\"2020-01-01\",\"2021-01-01\",\"Y\")", 1.0);
        err(&w, "=DATEDIF(DATE(2003,1,1),DATE(2001,1,1),\"Y\")", ErrorKind::Num);
        err(&w, "=DATEDIF(DATE(2001,1,1),DATE(2003,1,1),\"W\")", ErrorKind::Num);
    }

    #[test]
    fn weeks() {
        let w = wb();
        close(&w, "=WEEKNUM(DATE(2012,3,9))", 10.0);
        close(&w, "=WEEKNUM(DATE(2012,3,9),2)", 11.0);
        close(&w, "=WEEKNUM(DATE(2012,3,9),21)", 10.0);
        close(&w, "=ISOWEEKNUM(DATE(2012,3,9))", 10.0);
        // 2021-01-01 is a Friday: ISO week 53 of 2020.
        close(&w, "=ISOWEEKNUM(DATE(2021,1,1))", 53.0);
        close(&w, "=WEEKNUM(DATE(2021,1,1),16)", 1.0);
        close(&w, "=WEEKNUM(DATE(2021,1,2),16)", 2.0);
        close(&w, "=WEEKNUM(DATE(2021,1,2),15)", 1.0);
        close(&w, "=WEEKNUM(DATE(2021,12,31),1)", 53.0);
        err(&w, "=WEEKNUM(DATE(2021,1,2),3)", ErrorKind::Num);
        err(&w, "=WEEKNUM(-1)", ErrorKind::Num);
    }

    #[test]
    fn yearfrac_and_days360() {
        let w = wb();
        close(&w, "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30))", 0.580_555_555_555_555_6);
        close(&w, "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),1)", 0.576_502_732_240_437_2);
        close(&w, "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),2)", 211.0 / 360.0);
        close(&w, "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),3)", 0.578_082_191_780_821_9);
        close(&w, "=YEARFRAC(DATE(2012,7,30),DATE(2012,1,1),4)", 0.580_555_555_555_555_6);
        // Actual/actual across several years averages the year lengths.
        close(&w, "=YEARFRAC(DATE(2010,6,15),DATE(2013,2,1),1)", 962.0 / (1461.0 / 4.0));
        // 30/360 US: end of February counts as the 30th.
        close(&w, "=YEARFRAC(DATE(2011,2,28),DATE(2012,2,29))", 1.0);
        close(&w, "=YEARFRAC(DATE(2011,1,31),DATE(2011,3,31))", 60.0 / 360.0);
        err(&w, "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),5)", ErrorKind::Num);
        close(&w, "=DAYS360(DATE(2011,1,30),DATE(2011,12,31))", 330.0);
        close(&w, "=DAYS360(DATE(2011,1,30),DATE(2011,12,31),TRUE)", 330.0);
        close(&w, "=DAYS360(DATE(2011,1,1),DATE(2011,1,31))", 30.0);
        close(&w, "=DAYS360(DATE(2011,1,1),DATE(2011,1,31),TRUE)", 29.0);
        close(&w, "=DAYS360(DATE(2011,2,28),DATE(2011,3,31))", 30.0);
        close(&w, "=DAYS360(DATE(2011,12,31),DATE(2011,1,1))", -359.0);
    }

    #[test]
    fn times_and_text() {
        let mut w = wb();
        close(&w, "=TIME(12,0,0)", 0.5);
        close(&w, "=TIME(16,48,10)", (16.0 * 3600.0 + 48.0 * 60.0 + 10.0) / 86400.0);
        close(&w, "=TIME(27,0,0)", 0.125);
        close(&w, "=TIME(1,-30,0)", 1.0 / 48.0);
        err(&w, "=TIME(0,-1,0)", ErrorKind::Num);
        err(&w, "=TIME(40000,0,0)", ErrorKind::Num);
        close(&w, "=DATEVALUE(\"2008-08-22\")", 39682.0);
        close(&w, "=DATEVALUE(\"8/22/2008\")", 39682.0);
        close(&w, "=DATEVALUE(\"22-AUG-2008\")", 39682.0);
        close(&w, "=DATEVALUE(\"August 22, 2008\")", 39682.0);
        close(&w, "=DATEVALUE(\"2008-08-22 18:00\")", 39682.0);
        close(&w, "=TIMEVALUE(\"2:24 AM\")", 0.1);
        close(&w, "=TIMEVALUE(\"22-Aug-2011 6:35 AM\")", (6.0 * 60.0 + 35.0) / 1440.0);
        close(&w, "=TIMEVALUE(\"18:00\")", 0.75);
        close(&w, "=TIMEVALUE(\"12:00 AM\")", 0.0);
        err(&w, "=DATEVALUE(\"hello\")", ErrorKind::Value);
        err(&w, "=DATEVALUE(39682)", ErrorKind::Value);
        err(&w, "=DATEVALUE(\"2008-02-30\")", ErrorKind::Value);
        err(&w, "=TIMEVALUE(\"25:00\")", ErrorKind::Value);
        err(&w, "=TIMEVALUE(\" \")", ErrorKind::Value);
        let year = crate::format::serial_to_date(crate::format::now_serial()).0;
        close(&w, "=DATEVALUE(\"5-Jul\")", crate::format::date_to_serial(year, 7, 5));
        // Spanish workbook: day first, Spanish month names.
        w.locale = Locale::ES;
        close(&w, "=DATEVALUE(\"15/03/2023\")", 45000.0);
        close(&w, "=FECHANUMERO(\"15 de marzo de 2023\")", 45000.0);
        close(&w, "=DIAS.LAB(\"01/03/2023\";\"31/03/2023\")", 23.0);
        close(&w, "=HORANUMERO(\"15/03/2023 18:00\")", 0.75);
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        let w = wb();
        for f in [
            "=NETWORKDAYS(0,2958465)", "=NETWORKDAYS(1E+300,1)", "=WORKDAY(2958465,1)", "=WORKDAY(0,-1)", "=WORKDAY.INTL(1,1E+7,\"0111111\")",
            "=DATEDIF(0,2958465,\"MD\")", "=WEEKNUM(0)", "=ISOWEEKNUM(2958465)", "=YEARFRAC(0,2958465,1)", "=TIME(-32767,32767,-32767)",
            "=DATEVALUE(\"99999-1-1\")", "=TIMEVALUE(\"::\")", "=DATEVALUE(\"1 de 2 de\")", "=DAYS360(0,2958465)",
        ] {
            let _ = w.eval_formula(0, f);
        }
        close(&w, "=NETWORKDAYS(0,2958465)", num(&w, "=NETWORKDAYS.INTL(0,2958465,1)"));
    }
}
