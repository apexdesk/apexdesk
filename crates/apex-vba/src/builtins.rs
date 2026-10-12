//! VBA's functions: text, math, conversion, dates, arrays, information,
//! MsgBox and InputBox. Those that would reach outside the workbook
//! (files, Shell, other applications) give a "not supported" error.

use std::rc::Rc;

use apex_sheet::format::{self, NumFormat, date_to_serial, serial_to_date};

use crate::ast::Arg;
use crate::error::{Error, R};
use crate::interp::Interp;
use crate::value::{self, Array, VType, Value, number_text, parse_number};

/// The functions below (sorted: looked up before the arguments are evaluated).
const BUILTINS: &[&str] = &[
    "abs", "array", "asc", "ascw", "atn", "beep", "cbool", "cbyte", "ccur", "cdate", "cdbl", "cdec", "chdir", "choose", "chr", "chrw", "cint", "clng", "clnglng", "clngptr", "cos", "createobject", "csng", "cstr", "curdir", "cvar", "cverr", "date", "dateadd", "datediff", "datepart", "dateserial", "datevalue", "day", "dir", "doevents", "environ", "erl", "exp", "filecopy", "filedatetime", "filelen", "fix", "format", "formatcurrency", "formatdatetime", "formatnumber", "formatpercent", "freefile", "getobject", "getsetting", "hex", "hour", "iif", "inputbox", "instr", "instrrev", "int", "isarray", "isdate", "isempty", "iserror", "ismissing", "isnull", "isnumeric", "isobject", "join", "kill", "lbound", "lcase", "left", "len", "log", "ltrim", "mid", "minute", "mkdir", "month", "monthname", "msgbox", "now", "oct", "randomize", "replace", "rgb", "right", "rmdir", "rnd", "round", "rtrim", "savesetting", "second", "sendkeys", "sgn", "shell", "sin", "space", "split", "sqr", "strcomp", "strconv", "string", "strreverse", "switch", "tan", "time", "timer", "timeserial", "timevalue", "trim", "typename", "ubound", "ucase", "val", "vartype", "weekday", "weekdayname", "year",
];

fn days_in_month(y: i64, m: i64) -> i64 {
    (date_to_serial(y, m + 1, 1) - date_to_serial(y, m, 1)) as i64
}

/// Hours, minutes and seconds of a serial date.
fn hms(d: f64) -> (i64, i64, i64) {
    let secs = ((d - d.floor()) * 86_400.0).round() as i64 % 86_400;
    (secs / 3600, secs % 3600 / 60, secs % 60)
}

fn weekday(d: f64, first: i64) -> i64 {
    let w = (d.floor() as i64 + 6).rem_euclid(7) + 1;
    let first = if first < 1 { 1 } else { first };
    (w - first).rem_euclid(7) + 1
}

/// Val: the number at the start of the text ("12abc" is 12).
fn val(s: &str) -> f64 {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("&h") || lower.starts_with("&o") {
        let end = lower[2..].find(|c: char| !c.is_ascii_hexdigit()).map_or(lower.len(), |i| i + 2);
        return parse_number(&lower[..end]).unwrap_or(0.0);
    }
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && matches!(b[i], b'e' | b'E' | b'd' | b'D') {
        let mut j = i + 1;
        if j < b.len() && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    t[..i].replace(['d', 'D'], "e").parse().unwrap_or(0.0)
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// Position of `needle` in `hay` from char `start` (0-based), as chars.
fn find_from(hay: &[char], needle: &[char], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(hay.len()));
    }
    (start..hay.len().saturating_sub(needle.len() - 1)).find(|&i| hay[i..i + needle.len()] == *needle)
}

fn fold(s: &str, text: bool) -> Vec<char> {
    if text { s.to_lowercase().chars().collect() } else { s.chars().collect() }
}

/// A VBA format code in Excel's terms: `n` minutes, `AMPM`.
fn excel_code(code: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    for c in code.chars() {
        if c == '"' {
            quoted = !quoted;
        }
        out.push(if !quoted && (c == 'n' || c == 'N') { 'm' } else { c });
    }
    out.replace("AMPM", "AM/PM").replace("ampm", "am/pm")
}

impl Interp<'_> {
    pub fn builtin(&mut self, n: &str, args: &[Arg]) -> Option<R<Value>> {
        if BUILTINS.binary_search(&n).is_err() {
            return None;
        }
        Some(self.builtin_call(n, args))
    }

    /// Now in local time as a serial date.
    pub fn now(&self) -> f64 {
        format::now_serial() + self.utc_offset as f64 / 86_400.0
    }

    fn builtin_call(&mut self, n: &str, args: &[Arg]) -> R<Value> {
        let raw = self.arg_raw(args)?;
        // Functions that look at objects as such.
        match n {
            "isobject" => return Ok(Value::Bool(raw.first().is_some_and(Value::is_object))),
            "typename" => return Ok(Value::str(raw.first().map_or("Empty".into(), Value::type_name))),
            "vartype" => return Ok(Value::Int(raw.first().map_or(0, Value::var_type) as i16)),
            "ismissing" => return Ok(Value::Bool(matches!(raw.first(), Some(Value::Missing) | None))),
            "isarray" => return Ok(Value::Bool(matches!(raw.first(), Some(Value::Array(_))))),
            "array" => return Ok(Value::Array(Rc::new(Array::from_vec(raw, 0)))),
            "iif" => {
                let c = self.scalar(raw.first().cloned().unwrap_or_default())?;
                let c = !matches!(c, Value::Null) && c.to_bool()?;
                return Ok(raw.get(if c { 1 } else { 2 }).cloned().unwrap_or_default());
            }
            "choose" => {
                let i = self.scalar(raw.first().cloned().unwrap_or_default())?.to_i64()?;
                return Ok(if i >= 1 { raw.get(i as usize).cloned().unwrap_or(Value::Null) } else { Value::Null });
            }
            "createobject" | "getobject" => {
                let class = self.scalar(raw.first().cloned().unwrap_or_default())?.to_str()?;
                if n == "createobject" {
                    return self.new_object(&class);
                }
                return Err(Error::unsupported("GetObject"));
            }
            _ => {}
        }
        let mut a = Vec::with_capacity(raw.len());
        for v in raw {
            a.push(self.scalar(v)?);
        }
        let arg = |i: usize| a.get(i).cloned().filter(|v| !matches!(v, Value::Missing));
        let need = |i: usize| a.get(i).cloned().filter(|v| !matches!(v, Value::Missing)).ok_or_else(|| Error::new(449, "Argument not optional"));
        let s = |i: usize| -> R<String> { need(i)?.to_str() };
        let f = |i: usize| -> R<f64> { need(i)?.to_f64() };
        let int = |i: usize| -> R<i64> { need(i)?.to_i64() };
        let null_in = a.first().is_some_and(|v| matches!(v, Value::Null));
        let text = self.compare_text();
        let cmp_mode = |i: usize| -> R<bool> { Ok(match arg(i) { Some(v) => v.to_i64()? == 1, None => text }) };
        Ok(match n {
            // --- text
            "len" => {
                if null_in {
                    return Ok(Value::Null);
                }
                Value::Long(s(0)?.chars().count() as i32)
            }
            "left" | "right" => {
                if null_in {
                    return Ok(Value::Null);
                }
                let c = chars(&s(0)?);
                let k = int(1)?;
                if k < 0 {
                    return Err(Error::invalid_call());
                }
                let k = (k as usize).min(c.len());
                Value::str(if n == "left" { c[..k].iter().collect::<String>() } else { c[c.len() - k..].iter().collect() })
            }
            "mid" => {
                if null_in {
                    return Ok(Value::Null);
                }
                let c = chars(&s(0)?);
                let start = int(1)?;
                if start < 1 {
                    return Err(Error::invalid_call());
                }
                let from = ((start - 1) as usize).min(c.len());
                let len = match arg(2) {
                    Some(v) => v.to_i64()?.max(0) as usize,
                    None => c.len(),
                };
                Value::str(c[from..(from + len).min(c.len())].iter().collect::<String>())
            }
            "instr" => {
                // InStr([start,] s1, s2[, compare])
                let (start, i) = if a.len() >= 3 { (int(0)?, 1) } else { (1, 0) };
                if start < 1 {
                    return Err(Error::invalid_call());
                }
                let t = cmp_mode(i + 2)?;
                let (hay, needle) = (fold(&s(i)?, t), fold(&s(i + 1)?, t));
                if start as usize > hay.len() + 1 {
                    return Ok(Value::Long(0));
                }
                Value::Long(find_from(&hay, &needle, (start - 1) as usize).map_or(0, |p| p as i32 + 1))
            }
            "instrrev" => {
                let t = cmp_mode(3)?;
                let (hay, needle) = (fold(&s(0)?, t), fold(&s(1)?, t));
                let start = match arg(2) {
                    Some(v) if v.to_i64()? > 0 => (v.to_i64()? as usize).min(hay.len()),
                    _ => hay.len(),
                };
                if needle.is_empty() {
                    return Ok(Value::Long(start as i32));
                }
                let mut found = 0;
                let mut i = 0;
                while i + needle.len() <= start {
                    if hay[i..i + needle.len()] == *needle {
                        found = i + 1;
                    }
                    i += 1;
                }
                Value::Long(found as i32)
            }
            "ucase" => if null_in { Value::Null } else { Value::str(s(0)?.to_uppercase()) },
            "lcase" => if null_in { Value::Null } else { Value::str(s(0)?.to_lowercase()) },
            "trim" => if null_in { Value::Null } else { Value::str(s(0)?.trim_matches(' ')) },
            "ltrim" => if null_in { Value::Null } else { Value::str(s(0)?.trim_start_matches(' ')) },
            "rtrim" => if null_in { Value::Null } else { Value::str(s(0)?.trim_end_matches(' ')) },
            "space" => {
                let k = int(0)?.max(0) as usize;
                crate::value::string_room(k)?;
                Value::str(" ".repeat(k))
            }
            "string" => {
                let k = int(0)?.max(0) as usize;
                crate::value::string_room(k.saturating_mul(4))?;
                let c = match need(1)? {
                    Value::Str(x) => x.chars().next().ok_or_else(Error::invalid_call)?,
                    v => char::from_u32(v.to_i64()? as u32 % 256).unwrap_or(' '),
                };
                Value::str(std::iter::repeat_n(c, k).collect::<String>())
            }
            "strreverse" => Value::str(s(0)?.chars().rev().collect::<String>()),
            "strcomp" => {
                if null_in {
                    return Ok(Value::Null);
                }
                let t = cmp_mode(2)?;
                let (x, y) = (s(0)?, s(1)?);
                let o = if t { x.to_lowercase().cmp(&y.to_lowercase()) } else { x.cmp(&y) };
                Value::Int(o as i16)
            }
            "strconv" => {
                let x = s(0)?;
                match int(1)? {
                    1 => Value::str(x.to_uppercase()),
                    2 => Value::str(x.to_lowercase()),
                    3 => {
                        let mut out = String::new();
                        let mut start = true;
                        for c in x.chars() {
                            if start { out.extend(c.to_uppercase()) } else { out.extend(c.to_lowercase()) }
                            start = c.is_whitespace();
                        }
                        Value::str(out)
                    }
                    _ => Value::str(x),
                }
            }
            "replace" => {
                let x = chars(&s(0)?);
                let find = s(1)?;
                let repl = s(2)?;
                let start = match arg(3) {
                    Some(v) => v.to_i64()?,
                    None => 1,
                };
                if start < 1 {
                    return Err(Error::invalid_call());
                }
                let mut count = match arg(4) {
                    Some(v) => v.to_i64()?,
                    None => -1,
                };
                let t = cmp_mode(5)?;
                let x: Vec<char> = x.into_iter().skip(start as usize - 1).collect();
                if find.is_empty() {
                    return Ok(Value::str(x.into_iter().collect::<String>()));
                }
                let hay = if t { x.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect() } else { x.clone() };
                let needle = fold(&find, t);
                let mut out = String::new();
                let mut i = 0;
                while i < x.len() {
                    if count != 0 && i + needle.len() <= x.len() && hay[i..i + needle.len()] == *needle {
                        crate::value::string_room(out.len() + repl.len())?;
                        out.push_str(&repl);
                        i += needle.len();
                        count -= 1;
                    } else {
                        out.push(x[i]);
                        i += 1;
                    }
                }
                Value::str(out)
            }
            "split" => {
                let x = s(0)?;
                let delim = match arg(1) {
                    Some(v) => v.to_str()?,
                    None => " ".into(),
                };
                let limit = match arg(2) {
                    Some(v) => v.to_i64()?,
                    None => -1,
                };
                let parts: Vec<Value> = if x.is_empty() {
                    Vec::new()
                } else if delim.is_empty() {
                    vec![Value::str(x)]
                } else if limit > 0 {
                    x.splitn(limit as usize, delim.as_str()).map(Value::str).collect()
                } else {
                    x.split(delim.as_str()).map(Value::str).collect()
                };
                let mut arr = Array::from_vec(parts, 0);
                arr.ty = VType::String;
                Value::Array(Rc::new(arr))
            }
            "join" => {
                let Some(Value::Array(arr)) = arg(0) else { return Err(Error::type_mismatch()) };
                let delim = match arg(1) {
                    Some(v) => v.to_str()?,
                    None => " ".into(),
                };
                let mut out = String::new();
                for (i, v) in arr.data.iter().enumerate() {
                    let part = v.to_str()?;
                    crate::value::string_room(out.len() + part.len() + if i > 0 { delim.len() } else { 0 })?;
                    if i > 0 {
                        out.push_str(&delim);
                    }
                    out.push_str(&part);
                }
                Value::str(out)
            }
            "chr" | "chrw" => Value::str(char::from_u32(int(0)? as u32).ok_or_else(Error::invalid_call)?.to_string()),
            "asc" | "ascw" => Value::Long(s(0)?.chars().next().ok_or_else(Error::invalid_call)? as i32),
            "format" => {
                let v = need(0).unwrap_or(Value::Empty);
                let code = match arg(1) {
                    Some(c) => c.to_str()?,
                    None => String::new(),
                };
                Value::str(self.format(&v, &code)?)
            }
            "formatnumber" | "formatpercent" | "formatcurrency" => {
                let digits = match arg(1) {
                    Some(v) if v.to_i64()? >= 0 => v.to_i64()? as usize,
                    _ => 2,
                };
                value::string_room(digits)?;
                let dec = if digits > 0 { format!(".{}", "0".repeat(digits)) } else { String::new() };
                let code = match n {
                    "formatpercent" => format!("0{dec}%"),
                    "formatcurrency" => format!("\"{}\"#,##0{dec}", self.wb.locale.currency),
                    _ => format!("#,##0{dec}"),
                };
                Value::str(self.format(&need(0)?, &code)?)
            }
            "formatdatetime" => {
                let d = need(0)?.to_date()?;
                let code = match arg(1).map(|v| v.to_i64()).transpose()?.unwrap_or(0) {
                    1 => "dddd, mmmm d, yyyy".to_string(),
                    2 => self.wb.locale.short_date_code().to_string(),
                    3 => "hh:mm:ss".into(),
                    4 => "hh:mm".into(),
                    _ => return Ok(Value::str(value::date_text(d))),
                };
                Value::str(NumFormat::parse(&code).format_number(d, &self.wb.locale).0)
            }
            "cstr" => Value::str(s(0)?),
            "val" => value::num(val(&s(0)?), false),
            "hex" | "oct" => {
                let v = int(0)?;
                let v = if v < 0 { v as i32 as u32 as i64 } else { v };
                Value::str(if n == "hex" { format!("{v:X}") } else { format!("{v:o}") })
            }
            // --- math
            "abs" => match need(0)? {
                Value::Null => Value::Null,
                Value::Int(i) => value::num((i as f64).abs(), true),
                Value::Long(i) => value::num((i as f64).abs(), true),
                v => Value::Double(v.to_f64()?.abs()),
            },
            "int" | "fix" => {
                let v = need(0)?;
                let integral = value::is_integral(&v);
                let x = v.to_f64()?;
                let r = if n == "int" { x.floor() } else { x.trunc() };
                if integral { value::num(r, true) } else { Value::Double(r) }
            }
            "round" => {
                let x = f(0)?;
                let d = match arg(1) {
                    Some(v) => v.to_i64()?,
                    None => 0,
                };
                if d < 0 {
                    return Err(Error::invalid_call());
                }
                let p = 10f64.powi(d as i32);
                // Round half to even on the decimal representation.
                let scaled: f64 = format!("{:.9}", x * p).parse().unwrap_or(x * p);
                let r = scaled.round_ties_even() / p;
                if d == 0 && matches!(need(0)?, Value::Int(_) | Value::Long(_)) { value::num(r, true) } else { Value::Double(r) }
            }
            "sqr" => {
                let x = f(0)?;
                if x < 0.0 {
                    return Err(Error::invalid_call());
                }
                Value::Double(x.sqrt())
            }
            "exp" => Value::Double(f(0)?.exp()),
            "log" => {
                let x = f(0)?;
                if x <= 0.0 {
                    return Err(Error::invalid_call());
                }
                Value::Double(x.ln())
            }
            "sin" => Value::Double(f(0)?.sin()),
            "cos" => Value::Double(f(0)?.cos()),
            "tan" => Value::Double(f(0)?.tan()),
            "atn" => Value::Double(f(0)?.atan()),
            "sgn" => Value::Int(f(0)?.partial_cmp(&0.0).map_or(0, |o| o as i16)),
            "rnd" => {
                if arg(0).is_some_and(|v| v.to_f64().is_ok_and(|x| x < 0.0)) {
                    self.rng = (f(0)?.abs().to_bits()) | 1;
                }
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 7;
                self.rng ^= self.rng << 17;
                Value::Double((self.rng >> 11) as f64 / (1u64 << 53) as f64)
            }
            "randomize" => {
                self.rng = match arg(0) {
                    Some(v) => v.to_f64()?.to_bits() | 1,
                    None => (self.now().to_bits() ^ 0x9E37_79B9_7F4A_7C15) | 1,
                };
                Value::Empty
            }
            // --- conversion
            "cint" => need(0)?.coerce(VType::Integer)?,
            "clng" | "clngptr" => need(0)?.coerce(VType::Long)?,
            "clnglng" => need(0)?.coerce(VType::LongLong)?,
            "cdbl" | "csng" | "cdec" => need(0)?.coerce(VType::Double)?,
            "ccur" => need(0)?.coerce(VType::Currency)?,
            "cbyte" => need(0)?.coerce(VType::Byte)?,
            "cbool" => need(0)?.coerce(VType::Boolean)?,
            "cdate" | "datevalue" | "timevalue" => {
                let d = need(0)?.to_date()?;
                match n {
                    "datevalue" => Value::Date(d.floor()),
                    "timevalue" => Value::Date(d - d.floor()),
                    _ => Value::Date(d),
                }
            }
            "cvar" => need(0)?,
            "cverr" => Value::Error(int(0)? as i32),
            // --- information
            "isnumeric" => Value::Bool(match &a.first() {
                Some(Value::Str(x)) => parse_number(x).is_some(),
                Some(Value::Error(_) | Value::Null | Value::Array(_)) | None => false,
                Some(v) => v.is_numeric_type(),
            }),
            "isdate" => Value::Bool(match &a.first() {
                Some(Value::Date(_)) => true,
                Some(Value::Str(x)) => value::parse_date(x).is_some(),
                _ => false,
            }),
            "isempty" => Value::Bool(matches!(a.first(), Some(Value::Empty) | None)),
            "isnull" => Value::Bool(matches!(a.first(), Some(Value::Null))),
            "iserror" => Value::Bool(matches!(a.first(), Some(Value::Error(_)))),
            // --- arrays
            "lbound" | "ubound" => {
                let Some(Value::Array(arr)) = arg(0) else { return Err(Error::type_mismatch()) };
                let dim = match arg(1) {
                    Some(v) => v.to_i64()?,
                    None => 1,
                };
                if arr.bounds.is_empty() && !arr.data.is_empty() || dim < 1 {
                    return Err(Error::subscript());
                }
                let b = arr.bounds.get(dim as usize - 1).ok_or_else(Error::subscript)?;
                Value::Long(if n == "lbound" { b.0 } else { b.1 } as i32)
            }
            // --- dates
            "now" => Value::Date(self.now()),
            "date" => Value::Date(self.now().floor()),
            "time" => {
                let t = self.now();
                Value::Date(t - t.floor())
            }
            "timer" => {
                let t = self.now();
                Value::Double(((t - t.floor()) * 86_400.0 * 100.0).round() / 100.0)
            }
            "year" | "month" | "day" => {
                if null_in {
                    return Ok(Value::Null);
                }
                let (y, m, d) = serial_to_date(need(0)?.to_date()?);
                Value::Int(match n {
                    "year" => y as i16,
                    "month" => m as i16,
                    _ => d as i16,
                })
            }
            "hour" | "minute" | "second" => {
                let (h, m, s) = hms(need(0)?.to_date()?);
                Value::Int(match n {
                    "hour" => h,
                    "minute" => m,
                    _ => s,
                } as i16)
            }
            "weekday" => {
                let first = match arg(1) {
                    Some(v) => v.to_i64()?,
                    None => 1,
                };
                Value::Int(weekday(need(0)?.to_date()?, first) as i16)
            }
            "dateserial" => Value::Date(date_to_serial(int(0)?, int(1)?, int(2)?)),
            "timeserial" => Value::Date((int(0)? * 3600 + int(1)? * 60 + int(2)?) as f64 / 86_400.0),
            "monthname" => {
                let m = int(0)?;
                if !(1..=12).contains(&m) {
                    return Err(Error::invalid_call());
                }
                let code = if arg(1).map(|v| v.to_bool()).transpose()?.unwrap_or(false) { "mmm" } else { "mmmm" };
                Value::str(NumFormat::parse(code).format_number(date_to_serial(2001, m, 1), &self.wb.locale).0)
            }
            "weekdayname" => {
                let w = int(0)?;
                if !(1..=7).contains(&w) {
                    return Err(Error::invalid_call());
                }
                let first = match arg(2) {
                    Some(v) if v.to_i64()? > 0 => v.to_i64()?,
                    _ => 1,
                };
                let code = if arg(1).map(|v| v.to_bool()).transpose()?.unwrap_or(false) { "ddd" } else { "dddd" };
                // 2023-01-01 was a Sunday.
                let d = date_to_serial(2023, 1, 1) + ((w - 1 + first - 1) % 7) as f64;
                Value::str(NumFormat::parse(code).format_number(d, &self.wb.locale).0)
            }
            "dateadd" => {
                let iv = s(0)?.to_lowercase();
                let k = f(1)?;
                let d = need(2)?.to_date()?;
                Value::Date(match iv.as_str() {
                    "yyyy" | "q" | "m" => {
                        let months = match iv.as_str() {
                            "yyyy" => 12.0 * k,
                            "q" => 3.0 * k,
                            _ => k,
                        }
                        .trunc() as i64;
                        let (y, m, day) = serial_to_date(d);
                        let total = y * 12 + (m as i64 - 1) + months;
                        let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) + 1);
                        let day = (day as i64).min(days_in_month(ny, nm));
                        date_to_serial(ny, nm, day) + (d - d.floor())
                    }
                    "d" | "y" | "w" => d + k.trunc(),
                    "ww" => d + 7.0 * k.trunc(),
                    "h" => d + k.trunc() / 24.0,
                    "n" => d + k.trunc() / 1440.0,
                    "s" => d + k.trunc() / 86_400.0,
                    _ => return Err(Error::invalid_call()),
                })
            }
            "datediff" => {
                let iv = s(0)?.to_lowercase();
                let (d1, d2) = (need(1)?.to_date()?, need(2)?.to_date()?);
                let (y1, m1, _) = serial_to_date(d1);
                let (y2, m2, _) = serial_to_date(d2);
                let secs = |d: f64| (d * 86_400.0).round() as i64;
                Value::Long(match iv.as_str() {
                    "yyyy" => y2 - y1,
                    "q" => (y2 * 4 + (m2 as i64 - 1) / 3) - (y1 * 4 + (m1 as i64 - 1) / 3),
                    "m" => (y2 - y1) * 12 + m2 as i64 - m1 as i64,
                    "d" | "y" => d2.floor() as i64 - d1.floor() as i64,
                    "w" => (d2.floor() as i64 - d1.floor() as i64) / 7,
                    "ww" => ((d2.floor() as i64 + 6).div_euclid(7)) - ((d1.floor() as i64 + 6).div_euclid(7)),
                    "h" => secs(d2).div_euclid(3600) - secs(d1).div_euclid(3600),
                    "n" => secs(d2).div_euclid(60) - secs(d1).div_euclid(60),
                    "s" => secs(d2) - secs(d1),
                    _ => return Err(Error::invalid_call()),
                } as i32)
            }
            "datepart" => {
                let iv = s(0)?.to_lowercase();
                let d = need(1)?.to_date()?;
                let (y, m, day) = serial_to_date(d);
                let (h, mi, sec) = hms(d);
                Value::Long(match iv.as_str() {
                    "yyyy" => y,
                    "q" => (m as i64 - 1) / 3 + 1,
                    "m" => m as i64,
                    "d" => day as i64,
                    "y" => (d.floor() - date_to_serial(y, 1, 1)) as i64 + 1,
                    "w" => weekday(d, 1),
                    "ww" => ((d.floor() - date_to_serial(y, 1, 1)) as i64 + weekday(date_to_serial(y, 1, 1), 1) - 1) / 7 + 1,
                    "h" => h,
                    "n" => mi,
                    "s" => sec,
                    _ => return Err(Error::invalid_call()),
                } as i32)
            }
            // --- misc
            "switch" => {
                let mut i = 0;
                while i + 1 < a.len() {
                    if a[i].to_bool()? {
                        return Ok(a[i + 1].clone());
                    }
                    i += 2;
                }
                Value::Null
            }
            "rgb" => {
                let c = |i: usize| -> R<i64> { Ok(int(i)?.clamp(0, 255)) };
                Value::Long((c(0)? + c(1)? * 256 + c(2)? * 65_536) as i32)
            }
            "msgbox" => {
                let prompt = match a.first() {
                    Some(Value::Null) => "Null".to_string(),
                    _ => s(0)?,
                };
                let buttons = match arg(1) {
                    Some(v) => v.to_i64()? as i32,
                    None => 0,
                };
                let title = match arg(2) {
                    Some(v) => v.to_str()?,
                    None => String::new(),
                };
                let r = self.paused(|ui| ui.msg_box(&prompt, buttons, &title));
                Value::Int(r as i16)
            }
            "inputbox" => {
                let prompt = s(0)?;
                let title = match arg(1) {
                    Some(v) => v.to_str()?,
                    None => String::new(),
                };
                let default = match arg(2) {
                    Some(v) => v.to_str()?,
                    None => String::new(),
                };
                let r = self.paused(|ui| ui.input_box(&prompt, &title, &default));
                Value::str(r.unwrap_or_default())
            }
            "erl" => Value::Long(self.err.line as i32),
            "doevents" => Value::Int(0),
            "beep" => Value::Empty,
            "environ" => Value::str(""),
            "sendkeys" => return Err(Error::unsupported("SendKeys")),
            "shell" => return Err(Error::unsupported("Shell")),
            "getsetting" | "savesetting" => return Err(Error::unsupported("the registry (GetSetting/SaveSetting)")),
            _ => return Err(Error::unsupported("file access")),
        })
    }

    /// Format(value, code): named formats and Excel-style codes.
    pub fn format(&self, v: &Value, code: &str) -> R<String> {
        let loc = self.wb.locale;
        if code.is_empty() {
            return v.to_str();
        }
        let n = |v: &Value| -> Option<f64> {
            match v {
                Value::Str(s) => parse_number(s).or_else(|| value::parse_date(s)),
                Value::Null | Value::Missing | Value::Error(_) => None,
                v => v.to_f64().ok(),
            }
        };
        let named = match code.to_lowercase().as_str() {
            "general number" => return Ok(n(v).map(number_text).unwrap_or(v.to_str()?)),
            "general date" => return Ok(n(v).map(value::date_text).unwrap_or(v.to_str()?)),
            "currency" => format!("\"{}\"#,##0.00", loc.currency),
            "fixed" => "0.00".into(),
            "standard" => "#,##0.00".into(),
            "percent" => "0.00%".into(),
            "scientific" => "0.00E+00".into(),
            "long date" => "dddd, mmmm d, yyyy".into(),
            "medium date" => "dd-mmm-yy".into(),
            "short date" => loc.short_date_code().into(),
            "long time" => "hh:mm:ss".into(),
            "medium time" => "hh:mm AM/PM".into(),
            "short time" => "hh:mm".into(),
            "yes/no" | "true/false" | "on/off" => {
                let b = n(v).is_some_and(|x| x != 0.0);
                let (t, f) = match code.to_lowercase().as_str() {
                    "yes/no" => ("Yes", "No"),
                    "true/false" => ("True", "False"),
                    _ => ("On", "Off"),
                };
                return Ok(if b { t } else { f }.into());
            }
            ">" => return Ok(v.to_str()?.to_uppercase()),
            "<" => return Ok(v.to_str()?.to_lowercase()),
            _ => excel_code(code),
        };
        let fmt = NumFormat::parse(&named);
        Ok(match n(v) {
            Some(x) => fmt.format_number(x, &loc).0,
            None => fmt.format(&apex_sheet::Value::text(v.to_str()?), &loc).0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_sorted() {
        assert!(BUILTINS.windows(2).all(|w| w[0] < w[1]), "{:?}", BUILTINS.windows(2).find(|w| w[0] >= w[1]));
    }

    #[test]
    fn helpers() {
        assert_eq!(val("  12.5abc"), 12.5);
        assert_eq!(val("&HFF"), 255.0);
        assert_eq!(val("x"), 0.0);
        assert_eq!(weekday(date_to_serial(2024, 1, 1), 1), 2);
        assert_eq!(weekday(date_to_serial(2024, 1, 1), 2), 1);
        assert_eq!(excel_code("hh:nn \"n\""), "hh:mm \"n\"");
    }
}
