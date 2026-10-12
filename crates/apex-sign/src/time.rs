//! Dates as Unix seconds (UTC): from calendar fields, ISO 8601 and PDF
//! date strings, and back.

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn unix(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || !(0..=24).contains(&h) || !(0..=59).contains(&mi) || !(0..=60).contains(&s) {
        return None;
    }
    Some(days(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// Year, month, day, hour, minute and second of `t`.
pub fn civil(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let (z, secs) = (t.div_euclid(86_400) + 719_468, t.rem_euclid(86_400));
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, secs / 3600, secs % 3600 / 60, secs % 60)
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// `2026-09-26T12:00:00Z`, with optional fractions and offset (none is
/// taken as UTC).
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let n = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse().ok() };
    let t = unix(n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13).unwrap_or(0), n(14, 16).unwrap_or(0), n(17, 19).unwrap_or(0))?;
    let mut tail = s.get(19..).unwrap_or("");
    if let Some(f) = tail.strip_prefix('.') {
        tail = f.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match tail.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if tail.starts_with('-') { -1 } else { 1 };
            let oh: i64 = tail.get(1..3)?.parse().ok()?;
            let om: i64 = tail.get(4..6).or(tail.get(3..5)).and_then(|m| m.parse().ok()).unwrap_or(0);
            sign * (oh * 3600 + om * 60)
        }
        _ => 0,
    };
    Some(t - offset)
}

/// Whether an ISO 8601 time names its zone (Z or an offset).
pub fn has_zone(s: &str) -> bool {
    let s = s.trim();
    s.ends_with('Z') || s.get(19..).is_some_and(|t| t.contains(['+', '-']))
}

/// A PDF date: `D:20260926120000+02'00'` (fields after the year optional).
pub fn parse_pdf(s: &str) -> Option<i64> {
    let s = s.trim().strip_prefix("D:").unwrap_or(s.trim());
    let n = |a: usize, b: usize, def: i64| -> i64 { s.get(a..b).and_then(|x| x.parse().ok()).unwrap_or(def) };
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let t = unix(y, n(4, 6, 1), n(6, 8, 1), n(8, 10, 0), n(10, 12, 0), n(12, 14, 0))?;
    let tail = s.get(14..).unwrap_or("");
    let offset = match tail.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if tail.starts_with('-') { -1 } else { 1 };
            let oh: i64 = tail.get(1..3).and_then(|x| x.parse().ok()).unwrap_or(0);
            let om: i64 = tail.get(4..6).and_then(|x| x.parse().ok()).unwrap_or(0);
            sign * (oh * 3600 + om * 60)
        }
        _ => 0,
    };
    Some(t - offset)
}

/// `D:20260926120000Z` for a PDF.
pub fn pdf_date(t: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(t);
    format!("D:{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}+00'00'")
}

/// `2026-09-26 12:00:00 UTC`.
pub fn format_utc(t: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(t);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for t in [0, 951_782_400, 1_790_000_000, 4_102_444_800 + 86_399] {
            let (y, mo, d, h, mi, s) = civil(t);
            assert_eq!(unix(y, mo, d, h, mi, s), Some(t));
        }
        assert_eq!(unix(2000, 2, 29, 0, 0, 0), Some(951_782_400));
    }

    #[test]
    fn strings() {
        let noon = unix(2026, 9, 26, 12, 0, 0);
        assert_eq!(parse_iso("2026-09-26T12:00:00Z"), noon);
        assert_eq!(parse_iso("2026-09-26T14:00:00.123+02:00"), noon);
        assert_eq!(parse_pdf("D:20260926140000+02'00'"), noon);
        assert_eq!(parse_pdf("D:20260926120000Z"), noon);
        assert_eq!(parse_pdf(&pdf_date(noon.unwrap())), noon);
    }
}
