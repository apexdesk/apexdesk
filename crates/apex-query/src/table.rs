//! The in-memory table queries work on: named columns with a type, and
//! rows of values. Also the conversions from text (type detection) and to
//! the cells of a workbook.

use std::cmp::Ordering;

use apex_sheet::format::{date_to_serial, serial_to_date};
use apex_sheet::query::ColType;
use apex_sheet::{QueryResult, Value};

/// A value of a query table.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Null,
    Num(f64),
    Text(String),
    Bool(bool),
    /// A date (and time) as a spreadsheet serial number.
    Date(f64),
}

impl Val {
    pub fn text(s: impl Into<String>) -> Val {
        Val::Text(s.into())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Val::Null) || matches!(self, Val::Text(t) if t.is_empty())
    }

    /// The value as text (dates in ISO form).
    pub fn to_text(&self) -> String {
        match self {
            Val::Null => String::new(),
            Val::Num(n) => fmt_num(*n),
            Val::Text(t) => t.clone(),
            Val::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Val::Date(d) => fmt_date(*d),
        }
    }

    /// The value as a number, if it is one or reads as one.
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Val::Num(n) | Val::Date(n) => Some(*n),
            Val::Bool(b) => Some(*b as u8 as f64),
            Val::Text(t) => parse_num(t.trim(), '.').or_else(|| parse_num(t.trim(), ',')),
            Val::Null => None,
        }
    }

    /// Orders values for sorting and comparing: nulls first, then numbers
    /// and dates, text (case-insensitive), and booleans.
    pub fn compare(&self, o: &Val) -> Ordering {
        fn rank(v: &Val) -> u8 {
            match v {
                Val::Null => 0,
                Val::Num(_) | Val::Date(_) => 1,
                Val::Text(t) if t.is_empty() => 0,
                Val::Text(_) => 2,
                Val::Bool(_) => 3,
            }
        }
        match (self, o) {
            (Val::Num(a) | Val::Date(a), Val::Num(b) | Val::Date(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Val::Text(a), Val::Text(b)) => a.to_lowercase().cmp(&b.to_lowercase()),
            (Val::Bool(a), Val::Bool(b)) => a.cmp(b),
            _ => rank(self).cmp(&rank(o)),
        }
    }

    /// A key that groups equal values (case-sensitive for text, as keys of
    /// joins and groups are).
    pub fn key(&self) -> String {
        match self {
            Val::Null => "\u{0}".into(),
            Val::Text(t) if t.is_empty() => "\u{0}".into(),
            Val::Num(n) | Val::Date(n) => format!("n{n}"),
            Val::Text(t) => format!("t{t}"),
            Val::Bool(b) => format!("b{b}"),
        }
    }
}

/// A number without a useless ".0".
pub fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// `2024-03-05` or `2024-03-05 14:30:00`.
pub fn fmt_date(d: f64) -> String {
    let (y, m, day) = serial_to_date(d);
    let secs = ((d - d.floor()) * 86400.0).round() as i64;
    if secs == 0 {
        format!("{y:04}-{m:02}-{day:02}")
    } else {
        format!("{y:04}-{m:02}-{day:02} {:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    }
}

/// A plain number with `dec` as decimal separator (the other one may
/// group thousands only when `dec` is ',').
pub fn parse_num(s: &str, dec: char) -> Option<f64> {
    if s.is_empty() || s.len() > 40 {
        return None;
    }
    let s = s.strip_suffix('%').map_or((s, 1.0), |x| (x.trim_end(), 0.01));
    let (body, scale) = s;
    let t: String = if dec == ',' {
        if body.matches(',').count() > 1 {
            return None;
        }
        body.chars().filter(|&c| c != '.').map(|c| if c == ',' { '.' } else { c }).collect()
    } else {
        if body.contains(',') {
            return None;
        }
        body.to_string()
    };
    let ok = t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')) && t.chars().any(|c| c.is_ascii_digit());
    if !ok || t.starts_with(['e', 'E']) {
        return None;
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n * scale)
}

/// ISO dates and times: `2024-03-05`, `2024-03-05T14:30`, `2024-03-05
/// 14:30:15.5Z` (the time zone is dropped).
pub fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.len() < 8 || s.len() > 35 {
        return None;
    }
    let (date, time) = match s.find(['T', ' ']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let mut p = date.split('-');
    let (y, m, d) = (p.next()?, p.next()?, p.next()?);
    if p.next().is_some() || y.len() != 4 || m.is_empty() || m.len() > 2 || d.is_empty() || d.len() > 2 {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut serial = date_to_serial(y, m, d);
    if let Some(t) = time {
        // Drop a time zone: the values are shown as written.
        let t = t.trim_end_matches('Z');
        let t = match t.rfind(['+', '-']) {
            Some(i) if i >= 5 => &t[..i],
            _ => t,
        };
        let mut q = t.split(':');
        let h: f64 = q.next()?.parse().ok()?;
        let mi: f64 = q.next().unwrap_or("0").parse().ok()?;
        let sec: f64 = q.next().unwrap_or("0").parse().ok()?;
        if h >= 24.0 || mi >= 60.0 || sec >= 61.0 {
            return None;
        }
        serial += (h * 3600.0 + mi * 60.0 + sec) / 86400.0;
    }
    Some(serial)
}

/// Converts a value to a column type; values that don't fit become null
/// (text that isn't a number, for example).
pub fn convert(v: &Val, ty: ColType) -> Val {
    if v.is_null() {
        return Val::Null;
    }
    match ty {
        ColType::Any => v.clone(),
        ColType::Text => Val::Text(v.to_text()),
        ColType::Number => v.as_num().map_or(Val::Null, Val::Num),
        ColType::Integer => v.as_num().map_or(Val::Null, |n| Val::Num(n.round())),
        ColType::Bool => match v {
            Val::Bool(b) => Val::Bool(*b),
            Val::Num(n) => Val::Bool(*n != 0.0),
            Val::Text(t) => match t.trim().to_lowercase().as_str() {
                "true" | "yes" | "1" | "t" | "y" | "verdadero" | "sí" | "si" | "vrai" | "wahr" | "vero" | "verdadeiro" => Val::Bool(true),
                "false" | "no" | "0" | "f" | "n" | "falso" | "faux" | "falsch" | "não" | "nao" => Val::Bool(false),
                _ => Val::Null,
            },
            _ => Val::Null,
        },
        ColType::Date => match v {
            Val::Date(d) | Val::Num(d) => Val::Date(d.floor()),
            Val::Text(t) => parse_date(t).map_or(Val::Null, |d| Val::Date(d.floor())),
            _ => Val::Null,
        },
        ColType::DateTime => match v {
            Val::Date(d) | Val::Num(d) => Val::Date(*d),
            Val::Text(t) => parse_date(t).map_or(Val::Null, Val::Date),
            _ => Val::Null,
        },
    }
}

/// A table of a query.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataTable {
    pub columns: Vec<String>,
    pub types: Vec<ColType>,
    pub rows: Vec<Vec<Val>>,
}

impl DataTable {
    pub fn new(columns: Vec<String>) -> DataTable {
        let types = vec![ColType::Any; columns.len()];
        DataTable { columns, types, rows: Vec::new() }
    }

    /// A table from rows of text: the first row names the columns when
    /// `header`; types are detected.
    pub fn from_text_rows(mut rows: Vec<Vec<String>>, header: bool) -> DataTable {
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        let names: Vec<String> = if header && !rows.is_empty() { rows.remove(0) } else { Vec::new() };
        let columns = unique_names((0..width).map(|i| names.get(i).map(|s| s.trim().to_string()).unwrap_or_default()).collect());
        // Rows filled out to the widest: as many as the cells allowed.
        rows.truncate(crate::steps::MAX_CELLS / width.max(1));
        let mut t = DataTable::new(columns);
        t.rows = rows.into_iter().map(|r| (0..width).map(|i| r.get(i).map_or(Val::Null, |s| if s.is_empty() { Val::Null } else { Val::Text(s.clone()) })).collect()).collect();
        t.detect_types();
        t
    }

    pub fn width(&self) -> usize {
        self.columns.len()
    }

    /// The index of a column (exact name first, then ignoring case).
    pub fn col(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name).or_else(|| self.columns.iter().position(|c| c.eq_ignore_ascii_case(name)))
    }

    /// [`col`](Self::col), or an error naming the column.
    pub fn need(&self, name: &str) -> Result<usize, crate::Error> {
        self.col(name).ok_or_else(|| crate::Error::MissingColumn(name.to_string()))
    }

    /// Makes every row as wide as the columns.
    pub fn square(&mut self) {
        let w = self.columns.len();
        for r in &mut self.rows {
            r.resize(w, Val::Null);
        }
        self.types.resize(w, ColType::Any);
    }

    /// Turns text columns whose values all read as numbers, dates or
    /// booleans into those.
    pub fn detect_types(&mut self) {
        for c in 0..self.width() {
            let texts: Vec<&str> = self.rows.iter().filter_map(|r| match r.get(c) {
                Some(Val::Text(t)) if !t.trim().is_empty() => Some(t.as_str()),
                _ => None,
            }).collect();
            if texts.is_empty() || self.rows.iter().any(|r| !matches!(r.get(c), Some(Val::Text(_) | Val::Null) | None)) {
                continue;
            }
            // Codes with leading zeros ("007", "08001") stay text.
            let zeros = texts.iter().any(|t| {
                let t = t.trim().trim_start_matches('-');
                t.len() > 1 && t.starts_with('0') && !t.starts_with("0.") && !t.starts_with("0,")
            });
            let conv: Option<Box<dyn Fn(&str) -> Option<Val>>> = if !zeros && texts.iter().all(|t| parse_num(t.trim(), '.').is_some()) {
                Some(Box::new(|t: &str| parse_num(t.trim(), '.').map(Val::Num)))
            } else if !zeros && texts.iter().all(|t| parse_num(t.trim(), ',').is_some()) {
                Some(Box::new(|t: &str| parse_num(t.trim(), ',').map(Val::Num)))
            } else if texts.iter().all(|t| parse_date(t).is_some()) {
                Some(Box::new(|t: &str| parse_date(t).map(Val::Date)))
            } else if texts.iter().all(|t| t.trim().eq_ignore_ascii_case("true") || t.trim().eq_ignore_ascii_case("false")) {
                Some(Box::new(|t: &str| Some(Val::Bool(t.trim().eq_ignore_ascii_case("true")))))
            } else {
                None
            };
            let Some(conv) = conv else { continue };
            for r in &mut self.rows {
                if let Some(v) = r.get_mut(c) {
                    if let Val::Text(t) = v {
                        *v = if t.trim().is_empty() { Val::Null } else { conv(t).unwrap_or(Val::Null) };
                    }
                }
            }
        }
    }

    /// The first `n` rows (a preview).
    pub fn head(&self, n: usize) -> DataTable {
        DataTable { columns: self.columns.clone(), types: self.types.clone(), rows: self.rows.iter().take(n).cloned().collect() }
    }

    /// The table as cells: values, and a date format for date columns.
    pub fn to_result(&self) -> QueryResult {
        let formats = (0..self.width())
            .map(|c| {
                let ty = self.types.get(c).copied().unwrap_or(ColType::Any);
                let dates: Vec<f64> = self.rows.iter().filter_map(|r| if let Some(Val::Date(d)) = r.get(c) { Some(*d) } else { None }).collect();
                match ty {
                    ColType::Date => Some("yyyy-mm-dd".to_string()),
                    ColType::DateTime => Some("yyyy-mm-dd hh:mm:ss".to_string()),
                    _ if dates.is_empty() => None,
                    _ if dates.iter().all(|d| d.fract() == 0.0) => Some("yyyy-mm-dd".to_string()),
                    _ => Some("yyyy-mm-dd hh:mm:ss".to_string()),
                }
            })
            .collect();
        let rows = self.rows.iter().map(|r| r.iter().map(to_value).collect()).collect();
        QueryResult { names: self.columns.clone(), rows, formats }
    }
}

/// A cell value for a table value.
pub fn to_value(v: &Val) -> Value {
    match v {
        Val::Null => Value::Empty,
        Val::Num(n) | Val::Date(n) => Value::Number(*n),
        Val::Text(t) if t.is_empty() => Value::Empty,
        Val::Text(t) => Value::text(t.as_str()),
        Val::Bool(b) => Value::Bool(*b),
    }
}

/// A table value for a cell value (`date`: the cell has a date format).
pub fn from_value(v: &Value, date: bool) -> Val {
    match v {
        Value::Empty => Val::Null,
        Value::Number(n) if date => Val::Date(*n),
        Value::Number(n) => Val::Num(*n),
        Value::Text(t) => Val::Text(t.to_string()),
        Value::Bool(b) => Val::Bool(*b),
        Value::Error(e) => Val::Text(e.as_str().to_string()),
        Value::Array(_) | Value::Lambda(_) => Val::Null,
    }
}

/// Column names made unique and never empty (`Column1`, `Name2`).
pub fn unique_names(names: Vec<String>) -> Vec<String> {
    let mut unique = apex_sheet::table::UniqueNames::default();
    names.into_iter().enumerate().map(|(i, n)| unique.add(if n.trim().is_empty() { format!("Column{}", i + 1) } else { n })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_dates() {
        assert_eq!(parse_num("1234.5", '.'), Some(1234.5));
        assert_eq!(parse_num("1.234,5", ','), Some(1234.5));
        assert_eq!(parse_num("1,5", '.'), None);
        assert_eq!(parse_num("12%", '.'), Some(0.12));
        assert_eq!(parse_num("abc", '.'), None);
        assert_eq!(parse_num("e5", '.'), None);
        assert_eq!(parse_date("2024-01-01"), Some(45292.0));
        assert_eq!(parse_date("2024-01-01T12:00:00Z"), Some(45292.5));
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(fmt_date(45292.5), "2024-01-01 12:00:00");
    }

    #[test]
    fn detects_types() {
        let rows = vec![
            vec!["id".into(), "price".into(), "day".into(), "zip".into(), "ok".into()],
            vec!["1".into(), "2,5".into(), "2024-01-02".into(), "08001".into(), "true".into()],
            vec!["2".into(), "".into(), "2024-01-03".into(), "28001".into(), "FALSE".into()],
        ];
        let t = DataTable::from_text_rows(rows, true);
        assert_eq!(t.columns, ["id", "price", "day", "zip", "ok"]);
        assert_eq!(t.rows[0], vec![Val::Num(1.0), Val::Num(2.5), Val::Date(45293.0), Val::text("08001"), Val::Bool(true)]);
        assert_eq!(t.rows[1][1], Val::Null);
        assert_eq!(t.to_result().formats[2].as_deref(), Some("yyyy-mm-dd"));
    }
}
