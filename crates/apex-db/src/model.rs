//! What a database holds, whatever it was read from: tables with their
//! columns, rows and indexes, relations between tables and saved queries.

use std::fmt::Write as _;

/// The type of a column, as Access names them (other formats map to the
/// nearest one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColType {
    Bool,
    Byte,
    Int16,
    Int32,
    Int64,
    /// Fixed point with four decimals.
    Money,
    Single,
    Double,
    DateTime,
    /// Up to 255 bytes.
    Binary,
    /// Up to 255 characters.
    Text,
    /// Long text.
    Memo,
    /// OLE objects and long binary data.
    Ole,
    Guid,
    /// Exact decimal numbers with a precision and a scale.
    Decimal,
    /// Files attached to the row (Access 2007+).
    Attachment,
    /// Several values in one field (Access 2007+ multi-value lists).
    MultiValue,
}

impl ColType {
    /// Stable name (saved in files and shown in the designer by key).
    pub fn key(self) -> &'static str {
        match self {
            ColType::Bool => "bool",
            ColType::Byte => "byte",
            ColType::Int16 => "int16",
            ColType::Int32 => "int32",
            ColType::Int64 => "int64",
            ColType::Money => "money",
            ColType::Single => "single",
            ColType::Double => "double",
            ColType::DateTime => "datetime",
            ColType::Binary => "binary",
            ColType::Text => "text",
            ColType::Memo => "memo",
            ColType::Ole => "ole",
            ColType::Guid => "guid",
            ColType::Decimal => "decimal",
            ColType::Attachment => "attachment",
            ColType::MultiValue => "multivalue",
        }
    }

    pub fn is_number(self) -> bool {
        matches!(self, ColType::Byte | ColType::Int16 | ColType::Int32 | ColType::Int64 | ColType::Money | ColType::Single | ColType::Double | ColType::Decimal)
    }

    pub fn is_integer(self) -> bool {
        matches!(self, ColType::Byte | ColType::Int16 | ColType::Int32 | ColType::Int64)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub ty: ColType,
    /// Characters of text columns, bytes of binary ones.
    pub size: u32,
    pub precision: u8,
    pub scale: u8,
    pub required: bool,
    /// Numbered automatically (Access AutoNumber).
    pub auto: bool,
    /// A memo holding hyperlinks.
    pub hyperlink: bool,
    /// The designer's properties, as expressions in Access syntax: default
    /// value, validation rule and its message, description, and the
    /// expression of calculated columns.
    pub default: String,
    pub validation: String,
    pub validation_text: String,
    pub description: String,
    pub expression: String,
}

impl Column {
    pub fn new(name: impl Into<String>, ty: ColType) -> Column {
        Column { name: name.into(), ty, size: 0, precision: 0, scale: 0, required: false, auto: false, hyperlink: false, default: String::new(), validation: String::new(), validation_text: String::new(), description: String::new(), expression: String::new() }
    }
}

/// A file attached to a row.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub data: Vec<u8>,
}

/// One value of a row.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    /// Money in ten-thousandths.
    Money(i64),
    /// An exact decimal number, as text ("-12.50").
    Decimal(String),
    Text(String),
    /// Date and time as days since 30 December 1899 (OLE automation dates,
    /// what Access and spreadsheets use).
    Date(f64),
    Blob(Vec<u8>),
    Guid(String),
    Attachments(Vec<Attachment>),
    /// The values of a multi-value field.
    List(Vec<Value>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The value as a number, when it is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Int(i) => Some(*i as f64),
            Value::Num(n) | Value::Date(n) => Some(*n),
            Value::Money(m) => Some(*m as f64 / 10000.0),
            Value::Decimal(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// The value as text, as it is written in CSV exports (dates in ISO
    /// form, booleans as 1/0 like mdb-export does).
    pub fn to_text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => if *b { "1".into() } else { "0".into() },
            Value::Int(i) => i.to_string(),
            Value::Num(n) => fmt_num(*n),
            Value::Money(m) => fmt_money(*m),
            Value::Decimal(s) | Value::Text(s) | Value::Guid(s) => s.clone(),
            Value::Date(d) => fmt_date(*d),
            Value::Blob(b) => format!("({} bytes)", b.len()),
            Value::Attachments(a) => a.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join("; "),
            Value::List(l) => l.iter().map(Value::to_text).collect::<Vec<_>>().join("; "),
        }
    }
}

/// A number without a trailing ".0" and without exponent for usual sizes.
pub fn fmt_num(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Money in ten-thousandths as a decimal number ("12.5", "-0.0001").
pub fn fmt_money(m: i64) -> String {
    let neg = m < 0;
    let a = m.unsigned_abs();
    let (int, frac) = (a / 10000, a % 10000);
    let mut s = if neg { "-".to_string() } else { String::new() };
    let _ = write!(s, "{int}");
    if frac != 0 {
        let f = format!("{frac:04}");
        let _ = write!(s, ".{}", f.trim_end_matches('0'));
    }
    s
}

/// Days since 1899-12-30 to (year, month, day).
pub fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    // Days from 1970-01-01, then the civil-from-days algorithm.
    let z = days - 25569 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day) to days since 1899-12-30.
pub fn ymd_to_days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468 + 25569
}

/// An OLE date as "2024-03-01" or "2024-03-01 13:45:00".
pub fn fmt_date(d: f64) -> String {
    if !d.is_finite() {
        return String::new();
    }
    // OLE dates before 1899-12-30 count the time forward from the day.
    let day = d.trunc() as i64;
    let frac = (d - d.trunc()).abs();
    let mut secs = (frac * 86400.0).round() as i64;
    let mut day = day;
    if secs >= 86400 {
        secs -= 86400;
        day += if d < 0.0 { -1 } else { 1 };
    }
    let (y, m, dd) = days_to_ymd(day);
    if secs == 0 {
        format!("{y:04}-{m:02}-{dd:02}")
    } else {
        format!("{y:04}-{m:02}-{dd:02} {:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    }
}

/// Parses "2024-03-01", "2024-03-01 13:45[:00]" or "2024-03-01T13:45:00"
/// to an OLE date.
pub fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date, time) = match s.find([' ', 'T']) {
        Some(i) => (&s[..i], Some(s[i + 1..].trim())),
        None => (s, None),
    };
    let mut it = date.split(['-', '/']);
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) || date.len() < 8 {
        return None;
    }
    let mut v = ymd_to_days(y, m, d) as f64;
    if let Some(t) = time.filter(|t| !t.is_empty()) {
        let t = t.trim_end_matches('Z');
        let mut p = t.split(':');
        let h: f64 = p.next()?.parse().ok()?;
        let mi: f64 = p.next().unwrap_or("0").parse().ok()?;
        let se: f64 = p.next().unwrap_or("0").parse().ok()?;
        let frac = (h * 3600.0 + mi * 60.0 + se) / 86400.0;
        // OLE dates before 1899-12-30 count the time forward from the day.
        v += if v < 0.0 { -frac } else { frac };
    }
    Some(v)
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Index {
    pub name: String,
    /// Column names, and whether each goes up.
    pub columns: Vec<(String, bool)>,
    pub primary: bool,
    pub unique: bool,
    /// The index of a relation's foreign key (Access keeps one per relation).
    pub foreign: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub indexes: Vec<Index>,
    /// Where a linked table's data lives (another file or a connection).
    pub linked: Option<String>,
}

impl Table {
    pub fn new(name: impl Into<String>, columns: Vec<Column>) -> Table {
        Table { name: name.into(), columns, rows: Vec::new(), indexes: Vec::new(), linked: None }
    }

    pub fn col(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The primary key's columns.
    pub fn primary_key(&self) -> Vec<String> {
        self.indexes.iter().find(|i| i.primary).map(|i| i.columns.iter().map(|c| c.0.clone()).collect()).unwrap_or_default()
    }
}

/// A relation: rows of `table` point with `columns` to the row of
/// `ref_table` with the same `ref_columns` (usually its key).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Relation {
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    /// Referential integrity is enforced.
    pub enforce: bool,
    pub cascade_update: bool,
    pub cascade_delete: bool,
    pub one_to_one: bool,
}

/// What a saved query does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryKind {
    Select,
    MakeTable,
    Append,
    Update,
    Delete,
    Crosstab,
    DataDefinition,
    PassThrough,
    Union,
    Unknown,
}

/// A saved query and its SQL text (in the dialect of the program that
/// saved it).
#[derive(Clone, Debug, PartialEq)]
pub struct SavedQuery {
    pub name: String,
    pub kind: QueryKind,
    pub sql: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Database {
    pub tables: Vec<Table>,
    pub relations: Vec<Relation>,
    pub queries: Vec<SavedQuery>,
    /// Things that could not be read (said once to the user).
    pub warnings: Vec<String>,
}

impl Database {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(fmt_date(0.0), "1899-12-30");
        assert_eq!(fmt_date(45352.5), "2024-03-01 12:00:00");
        assert_eq!(parse_date("2024-03-01 12:00"), Some(45352.5));
        assert_eq!(ymd_to_days(1900, 1, 1), 2);
        assert_eq!(days_to_ymd(ymd_to_days(1582, 10, 15)), (1582, 10, 15));
        assert_eq!(parse_date("2024-13-01"), None);
    }

    #[test]
    fn money() {
        assert_eq!(fmt_money(125000), "12.5");
        assert_eq!(fmt_money(-1), "-0.0001");
        assert_eq!(fmt_money(30000), "3");
    }
}
