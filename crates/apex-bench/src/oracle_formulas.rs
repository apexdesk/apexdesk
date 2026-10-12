//! Formulas against Excel: a workbook saved by Excel keeps the value Excel
//! computed for every formula. The formula cells are found in the file
//! itself (sheet XML of .xlsx, FORMULA records of .xls), the workbook is
//! opened with apex-io, recalculated with our engine, and each cell is
//! compared with Excel's value.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use apex_sheet::{CellRef, Content, ErrorKind, Value, Workbook};
use serde_json::{Value as Json, json};

use crate::oracle_zip::{Ev, Xml, Zip, attr, element_text, local, unescape};

/// Outcome of one formula cell.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Outcome {
    Match,
    /// Both are values (not errors) and they differ.
    Value,
    /// One side is an error, and the other isn't the same error.
    Error,
    /// A mismatch in a formula that uses a function we don't have.
    Unsupported,
    /// Excel has a formula here and we loaded none (not understood).
    Unparsed,
}

impl Outcome {
    pub const ALL: [Outcome; 5] = [Outcome::Match, Outcome::Value, Outcome::Error, Outcome::Unsupported, Outcome::Unparsed];

    pub fn name(self) -> &'static str {
        match self {
            Outcome::Match => "match",
            Outcome::Value => "value",
            Outcome::Error => "error",
            Outcome::Unsupported => "unsupported",
            Outcome::Unparsed => "unparsed",
        }
    }
}

/// A formula cell as Excel saved it.
pub struct RawCell {
    pub sheet: String,
    pub cell: CellRef,
    /// The formula text (.xlsx; the master's text for shared formulas).
    pub text: Option<String>,
    /// The value Excel computed and saved (Empty when there is none).
    pub cached: Value,
}

/// Functions whose value changes on every calculation: their cells aren't
/// compared.
const VOLATILE: &[&str] = &["NOW", "TODAY", "RAND", "RANDBETWEEN", "RANDARRAY", "INFO", "CELL"];

/// Function names called in a formula text (upper case, without the
/// `_xlfn.` family of prefixes), outside string literals.
pub fn functions(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            i += 1;
            while i < b.len() {
                if b[i] == b'"' {
                    if b.get(i + 1) == Some(&b'"') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == b'\'' {
            // Quoted sheet name.
            i += 1;
            while i < b.len() && b[i] != b'\'' {
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            if b.get(i) == Some(&b'(') && (s == 0 || !matches!(b[s - 1], b'!' | b'[')) {
                let mut name = text[s..i].to_ascii_uppercase();
                for p in ["_XLFN.", "_XLWS.", "_XLPM."] {
                    if let Some(r) = name.strip_prefix(p) {
                        name = r.to_string();
                    }
                }
                out.insert(name);
            }
            continue;
        }
        i += 1;
    }
    out
}

/// A rough family for a function, for the report.
pub fn category(f: &str) -> &'static str {
    const DATE: &[&str] = &["DATE", "DATEVALUE", "DAY", "DAYS", "DAYS360", "EDATE", "EOMONTH", "HOUR", "ISOWEEKNUM", "MINUTE", "MONTH", "NETWORKDAYS", "NETWORKDAYS.INTL", "NOW", "SECOND", "TIME", "TIMEVALUE", "TODAY", "WEEKDAY", "WEEKNUM", "WORKDAY", "WORKDAY.INTL", "YEAR", "YEARFRAC", "DATEDIF"];
    const LOOKUP: &[&str] = &["VLOOKUP", "HLOOKUP", "LOOKUP", "INDEX", "MATCH", "XLOOKUP", "XMATCH", "OFFSET", "INDIRECT", "CHOOSE", "ROW", "ROWS", "COLUMN", "COLUMNS", "ADDRESS", "TRANSPOSE", "HYPERLINK", "GETPIVOTDATA", "AREAS", "FILTER", "SORT", "SORTBY", "UNIQUE"];
    const TEXT: &[&str] = &["CONCATENATE", "CONCAT", "TEXTJOIN", "LEFT", "RIGHT", "MID", "LEN", "FIND", "SEARCH", "SUBSTITUTE", "REPLACE", "TRIM", "UPPER", "LOWER", "PROPER", "TEXT", "VALUE", "FIXED", "DOLLAR", "REPT", "EXACT", "CHAR", "CODE", "CLEAN", "T", "N", "LEFTB", "RIGHTB", "MIDB", "LENB", "TEXTBEFORE", "TEXTAFTER", "TEXTSPLIT", "NUMBERVALUE"];
    const LOGIC: &[&str] = &["IF", "IFS", "AND", "OR", "NOT", "XOR", "IFERROR", "IFNA", "SWITCH", "TRUE", "FALSE", "ISERROR", "ISERR", "ISNA", "ISBLANK", "ISNUMBER", "ISTEXT", "ISLOGICAL", "ISREF", "ISEVEN", "ISODD", "ISNONTEXT", "ERROR.TYPE", "NA", "TYPE", "ISFORMULA", "LET", "LAMBDA"];
    const AGG: &[&str] = &["SUM", "SUMIF", "SUMIFS", "SUMPRODUCT", "COUNT", "COUNTA", "COUNTBLANK", "COUNTIF", "COUNTIFS", "AVERAGE", "AVERAGEIF", "AVERAGEIFS", "AVERAGEA", "MIN", "MAX", "MINIFS", "MAXIFS", "MINA", "MAXA", "SUBTOTAL", "AGGREGATE", "PRODUCT", "SUMSQ", "DSUM", "DCOUNT", "DCOUNTA", "DAVERAGE", "DMAX", "DMIN", "DGET", "DPRODUCT"];
    const MATH: &[&str] = &["ROUND", "ROUNDUP", "ROUNDDOWN", "INT", "TRUNC", "ABS", "MOD", "POWER", "SQRT", "EXP", "LN", "LOG", "LOG10", "CEILING", "FLOOR", "MROUND", "SIGN", "PI", "RAND", "RANDBETWEEN", "EVEN", "ODD", "FACT", "COMBIN", "SIN", "COS", "TAN", "ATAN", "ATAN2", "ASIN", "ACOS", "DEGREES", "RADIANS", "CEILING.MATH", "FLOOR.MATH", "QUOTIENT", "GCD", "LCM", "MMULT"];
    const FIN: &[&str] = &["PMT", "IPMT", "PPMT", "NPV", "XNPV", "IRR", "XIRR", "MIRR", "FV", "PV", "RATE", "NPER", "SLN", "DB", "DDB", "SYD", "VDB", "EFFECT", "NOMINAL", "CUMIPMT", "CUMPRINC", "PRICE", "YIELD", "ACCRINT", "DURATION", "MDURATION", "COUPDAYS", "COUPNCD", "COUPPCD", "DISC", "TBILLPRICE"];
    const STATS: &[&str] = &["STDEV", "STDEVP", "STDEV.S", "STDEV.P", "VAR", "VARP", "VAR.S", "VAR.P", "MEDIAN", "MODE", "LARGE", "SMALL", "RANK", "RANK.EQ", "PERCENTILE", "QUARTILE", "CORREL", "SLOPE", "INTERCEPT", "FORECAST", "TREND", "GROWTH", "LINEST", "LOGEST", "NORMDIST", "NORMSDIST", "NORMINV", "NORMSINV", "NORM.DIST", "NORM.INV", "NORM.S.DIST", "NORM.S.INV", "TDIST", "TINV", "CHIDIST", "FDIST", "BINOMDIST", "POISSON", "EXPONDIST", "GAMMADIST", "LOGNORMDIST", "STANDARDIZE", "COVAR", "PEARSON", "RSQ", "STEYX", "AVEDEV", "DEVSQ", "GEOMEAN", "HARMEAN", "KURT", "SKEW", "FREQUENCY", "COUNTIF", "PERCENTRANK", "CONFIDENCE"];
    for (list, name) in [(DATE, "date"), (LOOKUP, "lookup"), (TEXT, "text"), (LOGIC, "logic/info"), (AGG, "aggregate"), (MATH, "math"), (FIN, "financial"), (STATS, "statistics")] {
        if list.contains(&f) {
            return name;
        }
    }
    if f == "(operators)" { "operators/references" } else { "other" }
}

/// Is `f` a function of our engine?
pub fn supported(f: &str) -> bool {
    apex_sheet::functions::Func::lookup(f).is_some()
}

/// Excel's value and ours agree (numbers within a relative 1e-9, an empty
/// result the same as Excel's 0 or "").
pub fn same(excel: &Value, ours: &Value) -> bool {
    let ours = match ours {
        Value::Array(m) => m.data.first().cloned().unwrap_or(Value::Empty),
        v => v.clone(),
    };
    match (excel, &ours) {
        (Value::Number(a), Value::Number(b)) => {
            let d = (a - b).abs();
            d <= 1e-10 || d <= 1e-9 * a.abs().max(b.abs())
        }
        (Value::Text(a), Value::Text(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Error(a), Value::Error(b)) => a == b,
        (Value::Number(n), Value::Empty) => *n == 0.0,
        (Value::Text(t), Value::Empty) => t.is_empty(),
        (Value::Empty, Value::Empty) => true,
        _ => false,
    }
}

pub fn show(v: &Value) -> String {
    match v {
        Value::Empty => "(empty)".into(),
        Value::Number(n) => format!("{n}"),
        Value::Text(t) => format!("\"{}\"", t.chars().take(60).collect::<String>()),
        Value::Bool(b) => if *b { "TRUE".into() } else { "FALSE".into() },
        Value::Error(e) => e.as_str().into(),
        Value::Array(m) => format!("{{array {}x{}}}", m.rows, m.cols),
        Value::Lambda(_) => "(lambda)".into(),
    }
}

// --- was it saved by Excel? --------------------------------------------------------------

/// The xlsx package's application and calculation mode ("auto", "manual"
/// or "fullCalcOnLoad"), or why it isn't a candidate. With manual
/// calculation Excel's values may be older than the inputs; with
/// fullCalcOnLoad the writer asked for a recalculation on opening.
fn xlsx_app(zip: &Zip) -> Result<(String, &'static str), String> {
    let app = zip.read_string("docProps/app.xml").and_then(|x| element_text(&x, "Application")).unwrap_or_default();
    if !(app.contains("Microsoft") && app.contains("Excel")) {
        return Err(format!("saved by '{app}'"));
    }
    let wb = zip.read_string("xl/workbook.xml").ok_or("no xl/workbook.xml")?;
    let mut calc = "auto";
    for ev in Xml::new(&wb) {
        if let Ev::Empty(n, a) | Ev::Start(n, a) = ev {
            if local(n) == "calcPr" {
                if attr(a, "calcMode").as_deref() == Some("manual") {
                    calc = "manual";
                }
                if matches!(attr(a, "fullCalcOnLoad").as_deref(), Some("1" | "true")) {
                    calc = "fullCalcOnLoad";
                }
            }
        }
    }
    Ok((app, calc))
}

/// Formula cells of every worksheet of an xlsx package.
pub fn xlsx_cells(zip: &Zip) -> Vec<RawCell> {
    let wb = zip.read_string("xl/workbook.xml").unwrap_or_default();
    let rels = zip.read_string("xl/_rels/workbook.xml.rels").unwrap_or_default();
    let mut targets = BTreeMap::new();
    for ev in Xml::new(&rels) {
        if let Ev::Empty(n, a) | Ev::Start(n, a) = ev {
            if local(n) == "Relationship" && attr(a, "Type").is_some_and(|t| t.ends_with("/worksheet")) {
                if let (Some(id), Some(t)) = (attr(a, "Id"), attr(a, "Target")) {
                    let path = match t.strip_prefix('/') {
                        Some(abs) => abs.to_string(),
                        None => format!("xl/{t}"),
                    };
                    targets.insert(id.into_owned(), path);
                }
            }
        }
    }
    let sst = shared_strings(zip);
    let mut out = Vec::new();
    for ev in Xml::new(&wb) {
        let (Ev::Empty(n, a) | Ev::Start(n, a)) = ev else { continue };
        if local(n) != "sheet" {
            continue;
        }
        let (Some(name), Some(id)) = (attr(a, "name"), attr(a, "r:id").or_else(|| attr(a, "id"))) else { continue };
        let Some(xml) = targets.get(id.as_ref()).and_then(|p| zip.read_string(p)) else { continue };
        sheet_cells(&xml, &name, &sst, &mut out);
    }
    out
}

/// The shared strings table (plain text of each item, phonetic runs left
/// out).
fn shared_strings(zip: &Zip) -> Vec<String> {
    let Some(xml) = zip.read_string("xl/sharedStrings.xml") else { return Vec::new() };
    let mut out = Vec::new();
    let mut skip = 0usize;
    let mut in_t = false;
    for ev in Xml::new(&xml) {
        match ev {
            Ev::Start(n, _) if local(n) == "si" => out.push(String::new()),
            Ev::Start(n, _) if local(n) == "rPh" => skip += 1,
            Ev::End(n) if local(n) == "rPh" => skip = skip.saturating_sub(1),
            Ev::Start(n, _) if local(n) == "t" => in_t = true,
            Ev::End(n) if local(n) == "t" => in_t = false,
            Ev::Text(t) if in_t && skip == 0 => {
                if let Some(s) = out.last_mut() {
                    s.push_str(&unescape(t));
                }
            }
            _ => {}
        }
    }
    out
}

/// The cached value of a cell from its type attribute and `<v>` text.
fn cached_value(t: &str, v: Option<&str>, sst: &[String]) -> Value {
    let Some(v) = v else { return Value::Empty };
    match t {
        "b" => Value::Bool(v.trim() == "1" || v.trim() == "true"),
        "e" => ErrorKind::parse(v.trim()).map(Value::Error).unwrap_or(Value::Empty),
        "str" | "inlineStr" => Value::Text(v.into()),
        "s" => v.trim().parse::<usize>().ok().and_then(|i| sst.get(i)).map(|s| Value::Text(s.as_str().into())).unwrap_or(Value::Empty),
        "n" | "" => v.trim().parse::<f64>().map(Value::Number).unwrap_or(Value::Empty),
        _ => Value::Empty,
    }
}

fn sheet_cells(xml: &str, sheet: &str, sst: &[String], out: &mut Vec<RawCell>) {
    let mut shared: BTreeMap<String, String> = BTreeMap::new();
    let mut cur: Option<CellRef> = None;
    let mut ty = String::new();
    let mut v: Option<String> = None;
    let mut f: Option<Option<String>> = None;
    let mut it = Xml::new(xml);
    while let Some(ev) = it.next() {
        match ev {
            Ev::Start(n, a) if local(n) == "c" => {
                cur = attr(a, "r").and_then(|r| CellRef::parse(&r));
                ty = attr(a, "t").map(|t| t.into_owned()).unwrap_or_default();
                f = None;
                v = None;
            }
            Ev::Start(n, _) if local(n) == "v" && cur.is_some() => {
                let mut text = String::new();
                for ev in it.by_ref() {
                    match ev {
                        Ev::Text(t) => text.push_str(&unescape(t)),
                        Ev::End(_) => break,
                        _ => {}
                    }
                }
                v = Some(text);
            }
            Ev::Start(n, a) if local(n) == "f" && cur.is_some() => {
                let mut text = String::new();
                for ev in it.by_ref() {
                    match ev {
                        Ev::Text(t) => text.push_str(&unescape(t)),
                        Ev::End(_) => break,
                        _ => {}
                    }
                }
                if attr(a, "t").as_deref() == Some("shared") {
                    if let Some(si) = attr(a, "si") {
                        if text.is_empty() {
                            f = Some(shared.get(si.as_ref()).cloned());
                            continue;
                        }
                        shared.insert(si.into_owned(), text.clone());
                    }
                }
                // What-if data tables have no formula text: name them as
                // Excel shows them ({=TABLE(...)}).
                if text.is_empty() && attr(a, "t").as_deref() == Some("dataTable") {
                    text = "TABLE()".into();
                }
                f = Some(Some(text));
            }
            Ev::Empty(n, a) if local(n) == "f" && cur.is_some() => {
                let si = attr(a, "si");
                f = Some(match si {
                    Some(s) => shared.get(s.as_ref()).cloned(),
                    None => Some(String::new()),
                });
                if attr(a, "t").as_deref() == Some("dataTable") {
                    f = Some(Some("TABLE()".into()));
                }
            }
            Ev::End(n) if local(n) == "c" => {
                if let (Some(c), Some(text)) = (cur.take(), f.take()) {
                    let cached = cached_value(&ty, v.as_deref(), sst);
                    out.push(RawCell { sheet: sheet.to_string(), cell: c, text, cached });
                }
            }
            _ => {}
        }
    }
}

// --- .xls --------------------------------------------------------------------------------

/// The records of a BIFF stream: (type, data, offset).
fn records(s: &[u8]) -> impl Iterator<Item = (u16, &[u8], usize)> {
    let mut p = 0usize;
    std::iter::from_fn(move || {
        let h = s.get(p..p + 4)?;
        let typ = u16::from_le_bytes([h[0], h[1]]);
        let len = u16::from_le_bytes([h[2], h[3]]) as usize;
        let data = s.get(p + 4..p + 4 + len)?;
        let at = p;
        p += 4 + len;
        Some((typ, data, at))
    })
}

/// The application name of an OLE file's SummaryInformation, if any.
fn summary_app(sum: &[u8]) -> Option<String> {
    let u32_at = |p: usize| sum.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let sec = u32_at(44)? as usize;
    let count = u32_at(sec + 4)? as usize;
    for k in 0..count.min(64) {
        let id = u32_at(sec + 8 + 8 * k)?;
        let off = u32_at(sec + 12 + 8 * k)? as usize;
        if id == 0x12 {
            let p = sec + off;
            if u32_at(p)? != 0x1E {
                return None;
            }
            let len = u32_at(p + 4)? as usize;
            let bytes = sum.get(p + 8..p + 8 + len)?;
            return Some(String::from_utf8_lossy(bytes).trim_end_matches('\0').trim().to_string());
        }
    }
    None
}

/// A BIFF error code.
fn xls_error(code: u8) -> Option<ErrorKind> {
    Some(match code {
        0x00 => ErrorKind::Null,
        0x07 => ErrorKind::Div0,
        0x0F => ErrorKind::Value,
        0x17 => ErrorKind::Ref,
        0x1D => ErrorKind::Name,
        0x24 => ErrorKind::Num,
        0x2A => ErrorKind::Na,
        _ => return None,
    })
}

/// The text of a STRING record (BIFF8: 16-bit length and a flags byte;
/// BIFF5: 16-bit length, 8-bit characters).
fn xls_string(d: &[u8], biff8: bool) -> String {
    let n = u16::from_le_bytes([d.first().copied().unwrap_or(0), d.get(1).copied().unwrap_or(0)]) as usize;
    if !biff8 {
        return d.get(2..).unwrap_or(&[]).iter().take(n).map(|&c| c as char).collect();
    }
    let high = d.get(2).is_some_and(|f| f & 1 != 0);
    let b = d.get(3..).unwrap_or(&[]);
    if high {
        let units: Vec<u16> = b.chunks_exact(2).take(n).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        b.iter().take(n).map(|&c| c as char).collect()
    }
}

/// Candidate check and formula cells of an .xls file.
fn xls_cells(data: &[u8]) -> Result<(String, &'static str, Vec<RawCell>), String> {
    let mut cf = cfb::CompoundFile::open(std::io::Cursor::new(data)).map_err(|e| format!("not an OLE file: {e}"))?;
    let mut app = String::new();
    if let Ok(mut s) = cf.open_stream("/\u{5}SummaryInformation") {
        let mut b = Vec::new();
        let _ = s.read_to_end(&mut b);
        app = summary_app(&b).unwrap_or_default();
    }
    if !app.is_empty() && !app.contains("Excel") {
        return Err(format!("saved by '{app}'"));
    }
    let mut wb = Vec::new();
    let name = if cf.exists("/Workbook") { "/Workbook" } else { "/Book" };
    cf.open_stream(name).map_err(|e| format!("no workbook stream: {e}"))?.read_to_end(&mut wb).map_err(|e| e.to_string())?;
    let mut sheets: BTreeMap<usize, String> = BTreeMap::new();
    let mut biff8 = true;
    let mut out: Vec<RawCell> = Vec::new();
    let mut current: Option<String> = None;
    let mut calc = "auto";
    // A string result comes in the STRING record after its FORMULA.
    let mut want_string = false;
    for (typ, d, at) in records(&wb) {
        if want_string && typ == 0x0207 {
            want_string = false;
            if let Some(last) = out.last_mut() {
                last.cached = Value::Text(xls_string(d, biff8).into());
            }
            continue;
        }
        if !matches!(typ, 0x003C | 0x04BC | 0x0221 | 0x0236) {
            want_string = false;
        }
        match typ {
            0x0809 => {
                let v = u16::from_le_bytes([d.first().copied().unwrap_or(0), d.get(1).copied().unwrap_or(0)]);
                let dt = u16::from_le_bytes([d.get(2).copied().unwrap_or(0), d.get(3).copied().unwrap_or(0)]);
                if dt == 0x0005 {
                    biff8 = v == 0x0600;
                }
                current = if dt == 0x0010 { sheets.get(&at).cloned() } else { None };
            }
            0x002F => return Err("encrypted".into()),
            0x0085 if d.len() >= 8 => {
                let pos = u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as usize;
                let cch = d[6] as usize;
                let name = if biff8 {
                    let high = d[7] & 1 != 0;
                    let b = &d[8..];
                    if high {
                        let units: Vec<u16> = b.chunks_exact(2).take(cch).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                        String::from_utf16_lossy(&units)
                    } else {
                        b.iter().take(cch).map(|&c| c as char).collect()
                    }
                } else {
                    d[7..].iter().take(cch).map(|&c| c as char).collect()
                };
                sheets.insert(pos, name);
            }
            0x000D if d.len() >= 2 && i16::from_le_bytes([d[0], d[1]]) == 0 => calc = "manual",
            0x0006 if d.len() >= 4 => {
                if let Some(sheet) = &current {
                    let row = u16::from_le_bytes([d[0], d[1]]) as u32;
                    let col = u16::from_le_bytes([d[2], d[3]]) as u32;
                    let cached = match d.get(6..14) {
                        Some(r) if r[6] == 0xFF && r[7] == 0xFF => match r[0] {
                            0 => {
                                want_string = true;
                                Value::Empty
                            }
                            1 => Value::Bool(r[2] != 0),
                            2 => xls_error(r[2]).map(Value::Error).unwrap_or(Value::Empty),
                            3 => Value::Text("".into()),
                            _ => Value::Empty,
                        },
                        Some(r) => Value::Number(f64::from_le_bytes(r.try_into().unwrap_or([0; 8]))),
                        None => Value::Empty,
                    };
                    // PtgTbl first: a cell of a what-if data table, named as
                    // Excel shows it (as in .xlsx).
                    let text = (d.get(22) == Some(&0x02)).then(|| "TABLE()".to_string());
                    out.push(RawCell { sheet: sheet.clone(), cell: CellRef::new(row, col), text, cached });
                }
            }
            _ => {}
        }
    }
    if app.is_empty() {
        app = "(no SummaryInformation)".into();
    }
    Ok((app, calc, out))
}

// --- the check ---------------------------------------------------------------------------

/// Per function: counts of each outcome.
pub type Tally = BTreeMap<String, [u64; 5]>;

/// Checks one file; the JSON object that `oracle` aggregates.
pub fn check(path: &Path) -> Json {
    let file = path.display().to_string();
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => return json!({"file": file, "status": "error", "reason": e.to_string()}),
    };
    let found = if data.starts_with(b"PK") {
        match Zip::new(&data) {
            Some(zip) => xlsx_app(&zip).map(|(app, calc)| (app, calc, xlsx_cells(&zip))),
            None => Err("bad zip".into()),
        }
    } else if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        xls_cells(&data)
    } else {
        Err("not xlsx or xls".into())
    };
    let (app, calc, raw) = match found {
        Ok(x) => x,
        Err(reason) => return json!({"file": file, "status": "skipped", "reason": reason}),
    };
    if raw.is_empty() {
        return json!({"file": file, "status": "skipped", "reason": "no formulas", "app": app});
    }
    let mut wb = match apex_io::open(path) {
        Ok((apex_io::Loaded::Sheet(wb), _)) => wb,
        Ok(_) => return json!({"file": file, "status": "error", "reason": "opened as a document", "app": app}),
        Err(e) => return json!({"file": file, "status": "error", "reason": format!("open: {e}"), "app": app}),
    };
    // Excel's values depend on the locale of the machine that computed
    // them (TEXT, DATEVALUE, VALUE...); these corpora come from US
    // machines, so we calculate in English whatever the system's locale.
    wb.locale = apex_sheet::Locale::EN;
    let (tally, cells, diffs, skipped) = compare(&mut wb, &raw);
    let total: u64 = cells.iter().sum();
    json!({"file": file, "status": "checked", "app": app, "calc": calc, "formulas": total, "cells": cells, "skipped_cells": skipped, "tally": tally, "diffs": diffs})
}

/// `apex-bench oracle-cells FILE SHEET!A1...`: for looking into a
/// difference, each cell's formula and value after loading (ours: the
/// workbook is recalculated on opening).
pub fn cells_main(args: &[String]) -> Result<(), String> {
    let [file, cells @ ..] = args else { return Err("usage: oracle-cells FILE SHEET!A1...".into()) };
    let mut wb = match apex_io::open(Path::new(file)) {
        Ok((apex_io::Loaded::Sheet(wb), _)) => wb,
        Ok(_) => return Err("not a spreadsheet".into()),
        Err(e) => return Err(format!("open: {e}")),
    };
    wb.locale = apex_sheet::Locale::EN;
    wb.recalc_all();
    for a in cells {
        let (s, c) = a.rsplit_once('!').unwrap_or(("", a));
        let s = s.trim_matches('\'');
        let si = if s.is_empty() { Some(0) } else { wb.sheets.iter().position(|x| x.name.eq_ignore_ascii_case(s)) };
        let cell = si.zip(CellRef::parse(c)).and_then(|(si, c)| wb.cell(si, c));
        match cell {
            Some(cell) => println!("{a}: ={}  ->  {}", cell.formula().map(|e| e.expr().to_canonical()).unwrap_or_default(), show(cell.value())),
            None => println!("{a}: (no cell)"),
        }
    }
    Ok(())
}

/// Formula cells with a volatile function and every formula that reads
/// them, directly or not: Excel's values there came from another moment
/// (or other random numbers).
fn volatile_cells(wb: &Workbook) -> BTreeSet<(usize, CellRef)> {
    use apex_sheet::functions::Func::*;
    let seeds = [Now, Today, Rand, RandBetween, RandArray, Info, CellInfo];
    let mut seen = BTreeSet::new();
    let mut stack = Vec::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        for (c, cell) in sh.iter() {
            if let Some(e) = cell.formula() {
                if seeds.iter().any(|f| e.expr().calls(*f)) && seen.insert((si, c)) {
                    stack.push((si, c));
                }
            }
        }
    }
    while let Some((si, c)) = stack.pop() {
        for d in wb.dependents(si, c) {
            if seen.insert(d) {
                stack.push(d);
            }
        }
    }
    seen
}

/// Compares Excel's values with ours after a full recalculation.
pub fn compare(wb: &mut Workbook, raw: &[RawCell]) -> (Tally, [u64; 5], Vec<Json>, BTreeMap<&'static str, u64>) {
    struct Before {
        excel: Value,
        text: Option<String>,
        parsed: bool,
    }
    let index: BTreeMap<String, usize> = wb.sheets.iter().enumerate().map(|(i, s)| (s.name.to_lowercase(), i)).collect();
    let volatile = volatile_cells(wb);
    let mut skipped: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut before = Vec::new();
    for rc in raw {
        let Some(&si) = index.get(&rc.sheet.to_lowercase()) else {
            *skipped.entry("sheet not loaded").or_default() += 1;
            continue;
        };
        let excel = rc.cached.clone();
        let (parsed, ours_text) = match wb.cell(si, rc.cell).map(|c| &c.content) {
            Some(Content::Formula { expr, .. }) => (true, Some(expr.expr().to_canonical())),
            // A cell of an array formula (.xls keeps a formula record in
            // each): its value comes from the formula of the anchor.
            Some(Content::Spill(_)) => {
                let anchor = wb.spill_anchor(si, rc.cell).and_then(|a| wb.cell(si, a)).and_then(|c| c.formula());
                (true, anchor.map(|e| e.expr().to_canonical()))
            }
            _ => (false, None),
        };
        if matches!(excel, Value::Empty) {
            *skipped.entry("no cached value").or_default() += 1;
            continue;
        }
        // `<f/>` with no text (files converted by other programs): there
        // is no formula to calculate, only the value kept.
        if !parsed && rc.text.as_deref().is_some_and(|t| t.trim().is_empty()) {
            *skipped.entry("no formula text").or_default() += 1;
            continue;
        }
        let text = ours_text.or_else(|| rc.text.clone());
        if volatile.contains(&(si, rc.cell)) || text.as_deref().is_some_and(|t| functions(t).iter().any(|f| VOLATILE.contains(&f.as_str()))) {
            *skipped.entry("volatile").or_default() += 1;
            continue;
        }
        before.push((si, rc, Before { excel, text, parsed }));
    }
    wb.recalc_all();
    let mut tally = Tally::new();
    let mut cells = [0u64; 5];
    let mut diffs = Vec::new();
    for (si, rc, b) in before {
        let ours = match wb.cell(si, rc.cell).map(|c| &c.content) {
            Some(Content::Formula { value, .. }) => value.clone(),
            Some(Content::Value(v) | Content::Spill(v)) => v.clone(),
            _ => Value::Empty,
        };
        let funcs = b.text.as_deref().map(functions).unwrap_or_default();
        let outcome = if !b.parsed {
            Outcome::Unparsed
        } else if same(&b.excel, &ours) {
            Outcome::Match
        } else if funcs.iter().any(|f| !supported(f)) {
            Outcome::Unsupported
        } else if matches!(b.excel, Value::Error(_)) || matches!(ours, Value::Error(_)) {
            Outcome::Error
        } else {
            Outcome::Value
        };
        cells[outcome as usize] += 1;
        let keys: Vec<String> = if funcs.is_empty() { vec!["(operators)".into()] } else { funcs.into_iter().collect() };
        for k in &keys {
            tally.entry(k.clone()).or_default()[outcome as usize] += 1;
        }
        // Up to 40 examples, at most 20 of each outcome (a file may have
        // many of one and few of another).
        if outcome != Outcome::Match && diffs.len() < 40 && cells[outcome as usize] <= 20 {
            diffs.push(json!({
                "sheet": wb.sheets[si].name,
                "cell": rc.cell.to_a1(),
                "formula": b.text.clone().unwrap_or_default(),
                "excel": show(&b.excel),
                "ours": show(&ours),
                "kind": outcome.name(),
            }));
        }
    }
    (tally, cells, diffs, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_functions() {
        let f = functions("=IF(A1>0,SUM('My Sheet (2)'!A1:A3),\"MAX(\")+_xlfn.CONCAT(B1)+Sheet1!A1+[1]Book!X(1)");
        assert_eq!(f.into_iter().collect::<Vec<_>>(), vec!["CONCAT", "IF", "SUM"]);
        assert_eq!(category("VLOOKUP"), "lookup");
        assert!(supported("SUM"));
        assert!(!supported("NOSUCHFUNCTION"));
    }

    #[test]
    fn compares_values() {
        assert!(same(&Value::Number(1.0), &Value::Number(1.0 + 1e-12)));
        assert!(!same(&Value::Number(1.0), &Value::Number(1.001)));
        assert!(same(&Value::Number(0.0), &Value::Empty));
        assert!(!same(&Value::Error(ErrorKind::Na), &Value::Error(ErrorKind::Value)));
    }

    /// A BIFF8 workbook stream with one sheet and two formula cells (a
    /// number and a string result), in an OLE file.
    #[test]
    fn reads_xls_formula_cells() {
        fn rec(out: &mut Vec<u8>, typ: u16, data: &[u8]) {
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&(data.len() as u16).to_le_bytes());
            out.extend_from_slice(data);
        }
        let bof = |dt: u16| [&0x0600u16.to_le_bytes()[..], &dt.to_le_bytes(), &[0; 12]].concat();
        let formula = |row: u16, col: u16, result: [u8; 8]| [&row.to_le_bytes()[..], &col.to_le_bytes(), &[0, 0], &result, &[0; 6], &[3, 0, 0x1E, 1, 0]].concat();
        let mut globals = Vec::new();
        rec(&mut globals, 0x0809, &bof(0x0005));
        let sheet_rec_len = 4 + 10; // BOUNDSHEET with the name "S1"
        let sheet_at = globals.len() + sheet_rec_len + 4; // after BOUNDSHEET and EOF
        let mut bs = (sheet_at as u32).to_le_bytes().to_vec();
        bs.extend_from_slice(&[0, 0, 2, 0, b'S', b'1']);
        rec(&mut globals, 0x0085, &bs);
        rec(&mut globals, 0x000A, &[]);
        assert_eq!(globals.len(), sheet_at);
        rec(&mut globals, 0x0809, &bof(0x0010));
        rec(&mut globals, 0x0006, &formula(0, 1, 2.5f64.to_le_bytes()));
        rec(&mut globals, 0x0006, &formula(2, 0, [0, 0, 0, 0, 0, 0, 0xFF, 0xFF]));
        rec(&mut globals, 0x0207, &[2, 0, 0, b'h', b'i']);
        // A cell of a data table (PtgTbl).
        rec(&mut globals, 0x0006, &[&formula(3, 0, 5f64.to_le_bytes())[..20], &[5, 0, 0x02, 3, 0, 0, 0]].concat());
        rec(&mut globals, 0x000A, &[]);
        let mut cf = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
        std::io::Write::write_all(&mut cf.create_stream("/Workbook").unwrap(), &globals).unwrap();
        let data = cf.into_inner().into_inner();
        let (app, calc, cells) = xls_cells(&data).unwrap();
        assert_eq!((app.as_str(), calc), ("(no SummaryInformation)", "auto"));
        assert_eq!(cells.len(), 3);
        assert_eq!((cells[0].text.as_deref(), cells[2].text.as_deref()), (None, Some("TABLE()")));
        assert_eq!((cells[0].sheet.as_str(), cells[0].cell.to_a1(), &cells[0].cached), ("S1", "B1".to_string(), &Value::Number(2.5)));
        assert_eq!(cells[1].cached, Value::Text("hi".into()));
    }

    /// A tiny xlsx "saved by Excel" with one right and one wrong cached value.
    #[test]
    fn checks_an_xlsx() {
        let ct = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
        let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
        let wbx = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets><calcPr calcId="191029"/></workbook>"#;
        let wbrels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
        let sheet = r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1"><f>A1*3</f><v>6</v></c><c r="C1"><f>SUM(A1,B1)</f><v>99</v></c><c r="D1"><f>TODAY()</f><v>45000</v></c><c r="E1"><f t="dataTable" ref="E1:E1" dt2D="0" dtr="1" r1="A1"/><v>5</v></c><c r="F1"><f>D1+1</f><v>45001</v></c><c r="G1" t="e"><f ca="1"/><v>#NAME?</v></c></row></sheetData></worksheet>"#;
        let app = r#"<?xml version="1.0"?><Properties><Application>Microsoft Excel</Application></Properties>"#;
        let bytes = crate::oracle_zip::build_zip(&[("[Content_Types].xml", ct), ("_rels/.rels", rels), ("xl/workbook.xml", wbx), ("xl/_rels/workbook.xml.rels", wbrels), ("xl/worksheets/sheet1.xml", sheet), ("docProps/app.xml", app)]);
        let dir = std::env::temp_dir().join(format!("apex-bench-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.xlsx");
        std::fs::write(&p, bytes).unwrap();
        let r = check(&p);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(r["status"], "checked", "{r}");
        assert_eq!(r["formulas"], 3);
        assert_eq!(r["tally"]["TABLE"][4], 1);
        // TODAY() and the formula that reads it.
        assert_eq!(r["skipped_cells"]["volatile"], 2);
        // A formula without its text.
        assert_eq!(r["skipped_cells"]["no formula text"], 1);
        assert_eq!(r["tally"]["(operators)"][0], 1);
        assert_eq!(r["tally"]["SUM"][1], 1);
        assert_eq!(r["diffs"][0]["cell"], "C1");
    }
}
