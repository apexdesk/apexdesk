//! Locale, number format codes (Excel-compatible subset), dates and
//! parsing of user input typed into a cell.

use crate::value::Value;

mod date_orders;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOrder {
    Dmy,
    Mdy,
    Ymd,
}

impl DateOrder {
    /// Gregorian date order from Unicode CLDR. Accept POSIX and BCP 47
    /// spellings, resolve aliases and likely scripts/regions with ICU.
    /// Calendar extensions do not change Excel's Gregorian date system.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let tag = tag.split(['.', '@']).next().unwrap_or_default().replace('_', "-");
        if tag.eq_ignore_ascii_case("c") || tag.eq_ignore_ascii_case("posix") {
            return Some(Self::Mdy);
        }
        let lookup = |tag: &str| {
            let tag = tag.to_ascii_lowercase();
            date_orders::ORDERS.binary_search_by_key(&tag.as_str(), |&(name, _)| name)
                .ok().map(|i| date_orders::ORDERS[i].1)
        };
        let mut locale: icu_locale::Locale = tag.parse().ok()?;
        icu_locale::LocaleCanonicalizer::new_extended().canonicalize(&mut locale);
        if let Some(value) = locale.extensions.unicode.keywords.get(&icu_locale::extensions::unicode::key!("rg"))
            && let Some(region) = value.to_string().strip_suffix("zzzz").and_then(|s| s.parse().ok())
        {
            locale.id.region = Some(region);
        }
        if let Some(order) = lookup(&locale.id.to_string()) {
            return Some(order);
        }
        locale.id.variants = Default::default();
        icu_locale::LocaleExpander::new_extended().maximize(&mut locale.id);
        let language = locale.id.language;
        // Try the inferred script (e.g. zh-HK -> zh-Hant-HK), then the
        // regional form (en-Latn-AU -> en-AU), then the language default.
        let mut candidates = vec![locale.id.to_string()];
        if let Some(script) = locale.id.script {
            candidates.push(format!("{language}-{script}"));
        }
        if let Some(region) = locale.id.region {
            candidates.push(format!("{language}-{region}"));
        }
        candidates.push(language.to_string());
        candidates.iter().find_map(|tag| lookup(tag))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locale {
    pub decimal: char,
    pub thousands: char,
    /// Function argument separator shown in the formula bar.
    pub arg_sep: char,
    pub date_order: DateOrder,
    pub date_sep: char,
    pub currency: &'static str,
    pub currency_after: bool,
}

impl Locale {
    pub const EN: Locale = Locale {
        decimal: '.',
        thousands: ',',
        arg_sep: ',',
        date_order: DateOrder::Mdy,
        date_sep: '/',
        currency: "$",
        currency_after: false,
    };
    pub const ES: Locale = Locale {
        decimal: ',',
        thousands: '.',
        arg_sep: ';',
        date_order: DateOrder::Dmy,
        date_sep: '/',
        currency: "€",
        currency_after: true,
    };

    /// Number and date categories are independent: LC_ALL overrides both,
    /// otherwise LC_NUMERIC and LC_TIME each fall back to LANG.
    pub fn from_env() -> Locale {
        let category = |name| ["LC_ALL", name, "LANG"].iter()
            .filter_map(|k| std::env::var(k).ok()).find(|v| !v.is_empty()).unwrap_or_default();
        let mut locale = Self::from_tag(&category("LC_NUMERIC"));
        locale.date_order = DateOrder::from_tag(&category("LC_TIME")).unwrap_or(Self::EN.date_order);
        locale
    }

    pub fn from_tag(tag: &str) -> Locale {
        let t = tag.to_ascii_lowercase();
        let comma_langs = ["es", "fr", "de", "it", "pt", "nl", "ca", "gl", "eu", "ru", "pl", "tr", "sv", "da", "nb", "fi", "cs"];
        let mut locale = if comma_langs.iter().any(|l| t.starts_with(l)) {
            Locale::ES
        } else {
            Locale::EN
        };
        locale.date_order = DateOrder::from_tag(tag).unwrap_or(locale.date_order);
        locale
    }

    pub fn short_date_code(&self) -> &'static str {
        match self.date_order {
            DateOrder::Dmy => "dd/mm/yyyy",
            DateOrder::Mdy => "mm/dd/yyyy",
            DateOrder::Ymd => "yyyy-mm-dd",
        }
    }
}

impl Default for Locale {
    fn default() -> Self {
        Locale::EN
    }
}

// ---------------------------------------------------------------------------
// Dates: serial numbers count days since 1899-12-30 (Excel 1900 system).

const EPOCH_OFFSET: i64 = 25569; // days from 1899-12-30 to 1970-01-01

/// Days since 1970-01-01 -> (y, m, d). Howard Hinnant's algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Serial date for a calendar date. Months/days out of range roll over like Excel.
pub fn date_to_serial(y: i64, m: i64, d: i64) -> f64 {
    let total_m = y * 12 + (m - 1);
    let (yy, mm) = (total_m.div_euclid(12), total_m.rem_euclid(12) + 1);
    (days_from_civil(yy, mm as u32, 1) + d - 1 + EPOCH_OFFSET) as f64
}

pub fn serial_to_date(serial: f64) -> (i64, u32, u32) {
    civil_from_days(serial.floor() as i64 - EPOCH_OFFSET)
}

/// Serial date for a calendar date in Excel's 1900 system, which counts
/// a 29 February 1900: serials before 1 March 1900 are one less than
/// `date_to_serial` (1 is 1 January 1900, 60 the missing 29 February).
pub fn excel_serial(y: i64, m: i64, d: i64) -> f64 {
    let s = date_to_serial(y, m, d);
    if date_to_serial(y, m, 1) < 61.0 { s - 1.0 } else { s }
}

/// The calendar date of a serial in Excel's 1900 system: 0 is
/// 0 January 1900 and 60 is 29 February 1900.
pub fn excel_date(serial: f64) -> (i64, u32, u32) {
    match serial.floor() {
        d if !(0.0..61.0).contains(&d) => serial_to_date(d),
        0.0 => (1900, 1, 0),
        60.0 => (1900, 2, 29),
        d => serial_to_date(d + 1.0),
    }
}

/// Current local-ish time as a serial number (UTC; timezone databases are
/// deliberately not bundled).
pub fn now_serial() -> f64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    secs / 86_400.0 + EPOCH_OFFSET as f64
}

const MONTHS_ES: [&str; 12] = [
    "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre",
];
const MONTHS_EN: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];
const DAYS_ES: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
const DAYS_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

// ---------------------------------------------------------------------------
// General number formatting.

/// Excel-like "General": up to ~11 significant characters.
pub fn general_number(n: f64, decimal: char) -> String {
    if !n.is_finite() {
        return "#NUM!".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    let a = n.abs();
    let s = if a.fract() == 0.0 && a < 1e15 {
        format!("{}", n as i64)
    } else if !(1e-9..1e11).contains(&a) {
        let s = format!("{:.5E}", n);
        // "1.23450E11" -> "1.2345E+11"
        let (mant, exp) = s.split_once('E').unwrap_or((&s, "0"));
        let mant = if mant.contains('.') { mant.trim_end_matches('0').trim_end_matches('.') } else { mant };
        let e: i32 = exp.parse().unwrap_or(0);
        format!("{}E{}{:02}", mant, if e < 0 { '-' } else { '+' }, e.abs())
    } else {
        let int_digits = if a >= 1.0 { a.log10().floor() as i32 + 1 } else { 0 };
        let decimals = (10 - int_digits).clamp(0, 10) as usize;
        let s = format!("{:.*}", decimals, n);
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if decimal != '.' { s.replace('.', &decimal.to_string()) } else { s }
}

/// Full-precision representation for editing (no thousands separators).
pub fn edit_number(n: f64, decimal: char) -> String {
    let s = if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{}", n) };
    if decimal != '.' { s.replace('.', &decimal.to_string()) } else { s }
}

// ---------------------------------------------------------------------------
// Format codes.

#[derive(Clone, Debug, PartialEq)]
enum DatePart {
    Year2,
    Year4,
    Month,
    Month2,
    MonthShort,
    MonthLong,
    Day,
    Day2,
    DayShort,
    DayLong,
    Hour,
    Hour2,
    Minute,
    Minute2,
    Second,
    Second2,
    AmPm,
    /// Elapsed time, `[h]`, `[mm]` or `[ss]`: the unit and the width.
    Elapsed(char, usize),
    /// Fractions of a second after the seconds, `.00`: the digits.
    Frac(usize),
}

/// A piece of the number run of a format: digit placeholders with the
/// text written between them (`000"-"0000`).
#[derive(Clone, Debug, PartialEq)]
enum Pat {
    Digit(char),
    Dot,
    Lit(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Lit(String),
    /// Numeric placeholder block: the whole `#,##0.00` run.
    Number,
    Date(DatePart),
    Text,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct Section {
    toks: Vec<Tok>,
    int_zeros: usize,
    dec_zeros: usize,
    dec_hashes: usize,
    thousands: bool,
    percent: bool,
    scientific: bool,
    is_date: bool,
    color: Option<u32>,
    general: bool,
    condition: Option<(String, f64)>,
    /// The number run, when it has text between its digits.
    pat: Vec<Pat>,
    /// Commas after the digits: divide by 1000 for each.
    scale: i32,
}

/// Marks the fill point of a formatted text (see `NumFormat::format_marked`).
pub const FILL: char = '\u{1F}';

fn unmark(mut r: (String, Option<u32>)) -> (String, Option<u32>) {
    if r.0.contains(FILL) {
        r.0.retain(|c| c != FILL);
    }
    r
}

/// Excel's accounting format: the symbol and the number at opposite
/// sides of the cell, negatives in parentheses when the symbol goes first
/// (as in the US), and a dash for zero. An empty symbol gives none.
pub fn accounting_code(symbol: &str, after: bool, decimals: usize) -> String {
    let dec = if decimals > 0 { format!(".{}", "0".repeat(decimals)) } else { String::new() };
    let q = "?".repeat(decimals);
    let sym = if symbol.is_empty() { String::new() } else { format!("\"{symbol}\"") };
    if after || symbol.is_empty() {
        let s = if symbol.is_empty() { String::new() } else { format!(" {sym}") };
        format!("_-* #,##0{dec}{s}_-;-* #,##0{dec}{s}_-;_-* \"-\"{q}{s}_-;_-@_-")
    } else {
        format!("_({sym}* #,##0{dec}_);_({sym}* (#,##0{dec});_({sym}* \"-\"{q}_);_(@_)")
    }
}

/// A parsed number format code, e.g. `#,##0.00 "€"` or `dd/mm/yyyy`.
#[derive(Clone, Debug, PartialEq)]
pub struct NumFormat {
    general: bool,
    sections: Vec<Section>,
}

impl NumFormat {
    pub fn general() -> Self {
        NumFormat { general: true, sections: Vec::new() }
    }

    pub fn is_general(&self) -> bool {
        self.general
    }

    pub fn is_date(&self) -> bool {
        self.sections.first().is_some_and(|s| s.is_date)
    }

    pub fn is_percent(&self) -> bool {
        self.sections.first().is_some_and(|s| s.percent)
    }

    pub fn has_time(&self) -> bool {
        self.sections.first().is_some_and(|s| {
            s.toks.iter().any(|t| matches!(t, Tok::Date(DatePart::Hour | DatePart::Hour2 | DatePart::Minute | DatePart::Minute2)))
        })
    }

    pub fn has_date_part(&self) -> bool {
        self.sections.first().is_some_and(|s| {
            s.toks.iter().any(|t| matches!(t, Tok::Date(DatePart::Day | DatePart::Day2 | DatePart::Year4 | DatePart::Year2 | DatePart::Month | DatePart::Month2 | DatePart::MonthShort | DatePart::MonthLong)))
        })
    }

    pub fn parse(code: &str) -> Self {
        let code = code.trim();
        if code.is_empty() || code.eq_ignore_ascii_case("general") || code.eq_ignore_ascii_case("estándar") {
            return Self::general();
        }
        let sections = split_sections(code).into_iter().map(|s| parse_section(&s)).collect();
        NumFormat { general: false, sections }
    }

    /// Formats a value; returns the text and an optional RGB color.
    pub fn format(&self, v: &Value, loc: &Locale) -> (String, Option<u32>) {
        unmark(self.format_marked(v, loc))
    }

    pub fn format_number(&self, n: f64, loc: &Locale) -> (String, Option<u32>) {
        unmark(self.format_number_marked(n, loc))
    }

    /// The number as shown, for Precision as displayed: rounded to the
    /// decimals of its section. General, dates, scientific and sections
    /// without digits keep it whole.
    pub fn as_shown(&self, n: f64) -> f64 {
        if self.general || self.sections.is_empty() || !n.is_finite() {
            return n;
        }
        let sec = self.pick_section(n).0;
        if sec.is_date || sec.general || sec.scientific || !sec.toks.contains(&Tok::Number) {
            return n;
        }
        let dec = (sec.dec_zeros + sec.dec_hashes) as i32 + if sec.percent { 2 } else { 0 } - 3 * sec.scale;
        round_to(n, dec)
    }

    /// Whether the format fills the cell (`*` in the code).
    pub fn has_fill(&self) -> bool {
        self.sections.iter().any(|s| s.toks.iter().any(|t| matches!(t, Tok::Lit(l) if l.contains(FILL))))
    }

    /// Like `format`, with `FILL` where the cell is padded to its width.
    pub fn format_marked(&self, v: &Value, loc: &Locale) -> (String, Option<u32>) {
        match v {
            Value::Empty => (String::new(), None),
            Value::Text(s) => {
                if let Some(sec) = self.sections.get(3).or_else(|| self.sections.iter().find(|s| s.toks.contains(&Tok::Text))) {
                    let mut out = String::new();
                    // A code of thousands of `@` would repeat the text as often.
                    const MAX_SHOWN: usize = 1 << 20;
                    for t in &sec.toks {
                        match t {
                            _ if out.len() > MAX_SHOWN => break,
                            Tok::Lit(l) => out.push_str(l),
                            Tok::Text => out.push_str(s),
                            _ => {}
                        }
                    }
                    (out, sec.color)
                } else {
                    (s.to_string(), None)
                }
            }
            Value::Bool(b) => {
                let t = match (loc.decimal == ',', *b) {
                    (true, true) => "VERDADERO",
                    (true, false) => "FALSO",
                    (false, true) => "TRUE",
                    (false, false) => "FALSE",
                };
                (t.to_string(), None)
            }
            Value::Error(e) => (e.as_str().to_string(), None),
            Value::Array(_) | Value::Lambda(_) => (crate::value::ErrorKind::Value.as_str().to_string(), None),
            Value::Number(n) => self.format_number_marked(*n, loc),
        }
    }

    pub fn format_number_marked(&self, n: f64, loc: &Locale) -> (String, Option<u32>) {
        if self.general || self.sections.is_empty() {
            return (general_number(n, loc.decimal), None);
        }
        let (sec, value, negative_handled) = self.pick_section(n);
        let neg = value < 0.0 && !negative_handled;
        let mut out = String::new();
        if neg {
            out.push('-');
        }
        let v = value.abs();
        if sec.is_date {
            format_date(sec, v, loc, &mut out);
        } else {
            let mut num = if sec.percent { v * 100.0 } else { v } / 1000f64.powi(sec.scale);
            let body = if sec.general {
                general_number(num, loc.decimal)
            } else if sec.scientific {
                format_scientific(num, sec, loc)
            } else {
                let dec = sec.dec_zeros + sec.dec_hashes;
                num = round_to(num, dec as i32);
                if sec.pat.iter().any(|p| matches!(p, Pat::Lit(_))) { format_pattern(num, sec, loc) } else { format_fixed(num, sec, loc) }
            };
            for t in &sec.toks {
                match t {
                    Tok::Lit(l) => out.push_str(l),
                    Tok::Number => out.push_str(&body),
                    Tok::Text => {}
                    Tok::Date(_) => {}
                }
            }
        }
        (out, sec.color)
    }

    fn pick_section(&self, n: f64) -> (&Section, f64, bool) {
        let secs = &self.sections;
        if secs.iter().any(|s| s.condition.is_some()) {
            for s in secs {
                if let Some((op, t)) = &s.condition {
                    let ok = match op.as_str() {
                        "<" => n < *t,
                        "<=" => n <= *t,
                        ">" => n > *t,
                        ">=" => n >= *t,
                        "=" => n == *t,
                        "<>" => n != *t,
                        _ => false,
                    };
                    if ok {
                        return (s, n, true);
                    }
                }
            }
            let last = secs.iter().rev().find(|s| s.condition.is_none()).unwrap_or(&secs[0]);
            return (last, n, false);
        }
        match secs.len() {
            1 => (&secs[0], n, false),
            _ if n > 0.0 => (&secs[0], n, false),
            _ if n < 0.0 => (&secs[1], -n, true),
            2 => (&secs[0], n, false),
            _ => (&secs[2], n, true),
        }
    }
}

fn round_to(n: f64, dec: i32) -> f64 {
    let p = 10f64.powi(dec);
    let r = (n * p).round() / p;
    if r.is_finite() { r } else { n }
}

fn split_sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut in_q = false;
    let mut in_b = false;
    let mut esc = false;
    for c in code.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
            continue;
        }
        match c {
            '\\' if !in_q => esc = true,
            '"' => in_q = !in_q,
            '[' if !in_q => in_b = true,
            ']' if !in_q => in_b = false,
            ';' if !in_q && !in_b => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().unwrap().push(c);
    }
    out
}

fn color_by_name(n: &str) -> Option<u32> {
    Some(match n.to_ascii_lowercase().as_str() {
        "red" | "rojo" => 0xC00000,
        "blue" | "azul" => 0x0000FF,
        "green" | "verde" => 0x008000,
        "black" | "negro" => 0x000000,
        "white" | "blanco" => 0xFFFFFF,
        "yellow" | "amarillo" => 0xFFFF00,
        "magenta" => 0xFF00FF,
        "cyan" | "cian" => 0x00FFFF,
        _ => return None,
    })
}

fn parse_section(s: &str) -> Section {
    let mut sec = Section::default();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut lit = String::new();
    let mut in_number = false;
    let mut after_dot = false;
    // Placeholders seen: '?' only, and any other digit placeholder.
    let (mut q_marks, mut digits) = (0usize, false);
    let flush = |lit: &mut String, toks: &mut Vec<Tok>| {
        if !lit.is_empty() {
            toks.push(Tok::Lit(std::mem::take(lit)));
        }
    };
    if s.trim().eq_ignore_ascii_case("general") {
        sec.general = true;
        sec.toks.push(Tok::Number);
        return sec;
    }
    // Pre-scan: date/time codes contain y/m/d/h/s outside quotes and brackets.
    {
        let (mut q, mut b, mut esc) = (false, false, false);
        let mut bracket = String::new();
        for &c in &chars {
            if esc {
                esc = false;
                continue;
            }
            match c {
                '\\' if !q => esc = true,
                '"' => q = !q,
                '[' if !q => {
                    b = true;
                    bracket.clear();
                }
                ']' if !q && b => {
                    b = false;
                    if bracket.chars().all(|c| "hHmMsS".contains(c)) && !bracket.is_empty() {
                        sec.is_date = true;
                    }
                }
                _ if b => bracket.push(c),
                _ if !q && "yYdDhHsSmM".contains(c) => sec.is_date = true,
                _ => {}
            }
        }
    }
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                let inside = in_number && placeholder_later(&chars, i);
                in_number = inside;
                let mut l = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    l.push(chars[i]);
                    i += 1;
                }
                if inside { sec.pat.push(Pat::Lit(l)) } else { lit.push_str(&l) }
            }
            '\\' => {
                let inside = in_number && placeholder_later(&chars, i);
                in_number = inside;
                if let Some(&n) = chars.get(i + 1) {
                    if inside { sec.pat.push(Pat::Lit(n.to_string())) } else { lit.push(n) }
                    i += 1;
                }
            }
            '_' => {
                in_number = false;
                lit.push(' ');
                i += 1;
            }
            '*' => {
                // Repeat the next character to fill the cell: marked, and
                // drawn as a gap between the two sides (accounting formats).
                in_number = false;
                if !lit.contains(FILL) && !sec.toks.iter().any(|t| matches!(t, Tok::Lit(l) if l.contains(FILL))) {
                    lit.push(FILL);
                }
                i += 1;
            }
            '[' => {
                let end = chars[i..].iter().position(|&c| c == ']').map(|p| i + p).unwrap_or(chars.len());
                let inner: String = chars[i + 1..end.min(chars.len())].iter().collect();
                if let Some(col) = color_by_name(&inner) {
                    sec.color = Some(col);
                } else if let Some(rest) = inner.strip_prefix('$') {
                    let sym = rest.split('-').next().unwrap_or("");
                    lit.push_str(sym);
                } else if inner.starts_with(['<', '>', '=']) {
                    let op_len = inner.chars().take_while(|c| "<>=".contains(*c)).count();
                    let (op, val) = inner.split_at(op_len);
                    if let Ok(v) = val.trim().parse::<f64>() {
                        sec.condition = Some((op.to_string(), v));
                    }
                } else if sec.is_date && !inner.is_empty() && inner.chars().all(|c| "hHmMsS".contains(c)) {
                    // Elapsed time: [h], [mm], [ss].
                    flush(&mut lit, &mut sec.toks);
                    let unit = inner.chars().next().unwrap_or('h').to_ascii_lowercase();
                    sec.toks.push(Tok::Date(DatePart::Elapsed(unit, inner.chars().count())));
                }
                i = end;
            }
            '@' => {
                flush(&mut lit, &mut sec.toks);
                sec.toks.push(Tok::Text);
            }
            '0' | '#' | '?' | '.' | ',' if !sec.is_date && (c != '.' && c != ',' || in_number || next_is_placeholder(&chars, i)) => {
                if !in_number {
                    flush(&mut lit, &mut sec.toks);
                    sec.toks.push(Tok::Number);
                    in_number = true;
                    after_dot = false;
                }
                match c {
                    '.' => {
                        after_dot = true;
                        sec.pat.push(Pat::Dot);
                    }
                    ',' => {
                        if next_is_placeholder(&chars, i) {
                            sec.thousands |= !after_dot;
                        } else {
                            // After the digits: thousands (`#,##0,`).
                            sec.scale += 1;
                        }
                    }
                    '0' | '?' => {
                        sec.pat.push(Pat::Digit(c));
                        if c == '?' {
                            q_marks += 1;
                        } else {
                            digits = true;
                        }
                        if after_dot {
                            sec.dec_zeros += 1
                        } else {
                            sec.int_zeros += 1
                        }
                    }
                    _ => {
                        sec.pat.push(Pat::Digit(c));
                        digits = true;
                        if after_dot {
                            sec.dec_hashes += 1
                        }
                    }
                }
            }
            'E' | 'e' if in_number && matches!(chars.get(i + 1), Some('+') | Some('-')) => {
                sec.scientific = true;
                i += 2;
                while i < chars.len() && matches!(chars[i], '0' | '#') {
                    i += 1;
                }
                continue;
            }
            '%' => {
                in_number = false;
                sec.percent = true;
                lit.push('%');
            }
            '.' if sec.is_date && chars.get(i + 1) == Some(&'0') && lit.is_empty() && matches!(sec.toks.last(), Some(Tok::Date(DatePart::Second | DatePart::Second2 | DatePart::Elapsed('s', _)))) => {
                let n = chars[i + 1..].iter().take_while(|&&c| c == '0').count();
                sec.toks.push(Tok::Date(DatePart::Frac(n.min(3))));
                i += n + 1;
                continue;
            }
            _ if sec.is_date && "yYmMdDhHsSaA".contains(c) => {
                in_number = false;
                let lc = c.to_ascii_lowercase();
                if lc == 'a' {
                    let rest: String = chars[i..].iter().take(5).collect::<String>().to_ascii_uppercase();
                    if rest.starts_with("AM/PM") {
                        flush(&mut lit, &mut sec.toks);
                        sec.toks.push(Tok::Date(DatePart::AmPm));
                        i += 5;
                        continue;
                    }
                    lit.push(c);
                    i += 1;
                    continue;
                }
                let mut n = 0;
                while i + n < chars.len() && chars[i + n].to_ascii_lowercase() == lc {
                    n += 1;
                }
                flush(&mut lit, &mut sec.toks);
                let part = match lc {
                    'y' => {
                        if n <= 2 {
                            DatePart::Year2
                        } else {
                            DatePart::Year4
                        }
                    }
                    'd' => match n {
                        1 => DatePart::Day,
                        2 => DatePart::Day2,
                        3 => DatePart::DayShort,
                        _ => DatePart::DayLong,
                    },
                    'h' => {
                        if n == 1 {
                            DatePart::Hour
                        } else {
                            DatePart::Hour2
                        }
                    }
                    's' => {
                        if n == 1 {
                            DatePart::Second
                        } else {
                            DatePart::Second2
                        }
                    }
                    _ => {
                        // 'm': minutes if after an hour or before seconds.
                        let prev_is_hour = sec.toks.iter().rev().find_map(|t| match t {
                            Tok::Date(p) => Some(matches!(p, DatePart::Hour | DatePart::Hour2 | DatePart::Elapsed('h', _))),
                            _ => None,
                        }) == Some(true);
                        let next_is_sec = chars[i + n..].iter().find(|c| c.is_ascii_alphabetic()).is_some_and(|c| c.to_ascii_lowercase() == 's');
                        if (prev_is_hour || next_is_sec) && n <= 2 {
                            if n == 1 { DatePart::Minute } else { DatePart::Minute2 }
                        } else {
                            match n {
                                1 => DatePart::Month,
                                2 => DatePart::Month2,
                                3 => DatePart::MonthShort,
                                _ => DatePart::MonthLong,
                            }
                        }
                    }
                };
                sec.toks.push(Tok::Date(part));
                i += n;
                continue;
            }
            ' ' | '-' | '/' | '(' | ')' | ':' | '+' if in_number && !sec.is_date && placeholder_later(&chars, i) => sec.pat.push(Pat::Lit(c.to_string())),
            _ => {
                in_number = false;
                lit.push(c);
            }
        }
        i += 1;
    }
    flush(&mut lit, &mut sec.toks);
    // Only '?' placeholders (the zero section of accounting formats, like
    // `"-"??`): they stand for blank space.
    if q_marks > 0 && !digits {
        for t in &mut sec.toks {
            if *t == Tok::Number {
                *t = Tok::Lit(" ".repeat(q_marks));
            }
        }
    }
    sec
}

/// Whether a digit placeholder comes later in the code (text in quotes and
/// after `\\` doesn't count).
fn placeholder_later(chars: &[char], i: usize) -> bool {
    let (mut j, mut q) = (i + 1, chars[i] == '"');
    while j < chars.len() {
        match chars[j] {
            '"' => q = !q,
            '\\' if !q => j += 1,
            '0' | '#' | '?' if !q => return true,
            _ => {}
        }
        j += 1;
    }
    false
}

fn next_is_placeholder(chars: &[char], i: usize) -> bool {
    matches!(chars.get(i + 1), Some('0' | '#' | '?'))
}

fn group_thousands(int: &str, sep: char) -> String {
    let bytes = int.as_bytes();
    let mut out = String::with_capacity(int.len() + int.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(b as char);
    }
    out
}

fn format_fixed(n: f64, sec: &Section, loc: &Locale) -> String {
    let dec = sec.dec_zeros + sec.dec_hashes;
    let s = format!("{:.*}", dec, n);
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut int = int.trim_start_matches('0').to_string();
    while int.len() < sec.int_zeros {
        int.insert(0, '0');
    }
    if sec.thousands {
        int = group_thousands(&int, loc.thousands);
    }
    let mut frac = frac.to_string();
    let min = sec.dec_zeros;
    while frac.len() > min && frac.ends_with('0') {
        frac.pop();
    }
    if !frac.is_empty() || (dec > 0 && sec.dec_zeros > 0) {
        int.push(loc.decimal);
        int.push_str(&frac);
    }
    int
}

/// A number laid over its placeholders one by one, keeping the text
/// between them (`000"-"0000` gives 012-3456).
fn format_pattern(n: f64, sec: &Section, loc: &Locale) -> String {
    let fixed = format_fixed(n, &Section { thousands: false, ..sec.clone() }, loc);
    let (int, frac) = fixed.split_once(loc.decimal).unwrap_or((&fixed, ""));
    let int: Vec<char> = int.trim_start_matches('0').chars().collect();
    let dot = sec.pat.iter().position(|p| *p == Pat::Dot).unwrap_or(sec.pat.len());
    let first = sec.pat[..dot].iter().position(|p| matches!(p, Pat::Digit(_)));
    // Integer part, right to left; the first placeholder takes what is left.
    let mut left = int.len();
    let mut out: Vec<String> = Vec::new();
    for (k, p) in sec.pat[..dot].iter().enumerate().rev() {
        match p {
            Pat::Digit(d) if Some(k) == first => {
                let rest: String = int[..left].iter().collect();
                left = 0;
                out.push(if !rest.is_empty() { rest } else { placeholder(*d) });
            }
            Pat::Digit(_) if left > 0 => {
                left -= 1;
                out.push(int[left].to_string());
            }
            Pat::Digit(d) => out.push(placeholder(*d)),
            Pat::Lit(l) => out.push(l.clone()),
            Pat::Dot => {}
        }
    }
    out.reverse();
    let mut s = out.concat();
    if dot < sec.pat.len() {
        let mut digits = frac.chars();
        let mut f = String::new();
        for p in &sec.pat[dot + 1..] {
            match p {
                Pat::Digit(d) => match digits.next() {
                    Some(c) => f.push(c),
                    None => f.push_str(&placeholder(*d)),
                },
                Pat::Lit(l) => f.push_str(l),
                Pat::Dot => {}
            }
        }
        if !f.is_empty() || sec.dec_zeros > 0 {
            s.push(loc.decimal);
            s.push_str(&f);
        }
    }
    s
}

/// What a placeholder without a digit shows.
fn placeholder(d: char) -> String {
    match d {
        '0' => "0".into(),
        '?' => " ".into(),
        _ => String::new(),
    }
}

fn format_scientific(n: f64, sec: &Section, loc: &Locale) -> String {
    let dec = sec.dec_zeros + sec.dec_hashes;
    let s = format!("{:.*E}", dec, n);
    let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
    let e: i32 = e.parse().unwrap_or(0);
    let m = if loc.decimal != '.' { m.replace('.', &loc.decimal.to_string()) } else { m.to_string() };
    format!("{}E{}{:02}", m, if e < 0 { '-' } else { '+' }, e.abs())
}

fn format_date(sec: &Section, serial: f64, loc: &Locale, out: &mut String) {
    // Time rounded to the shown fraction of a second; it may carry into
    // the next day.
    let digits = sec.toks.iter().find_map(|t| match t {
        Tok::Date(DatePart::Frac(n)) => Some(*n as u32),
        _ => None,
    });
    let unit = 10i64.pow(digits.unwrap_or(0));
    let ticks = (serial * 86_400.0 * unit as f64).round() as i64;
    let whole = ticks / unit;
    let (y, m, d) = excel_date((whole / 86_400) as f64);
    let secs_total = whole % 86_400;
    let (hh, mm, ss) = (secs_total / 3600 % 24, secs_total / 60 % 60, secs_total % 60);
    let has_ampm = sec.toks.contains(&Tok::Date(DatePart::AmPm));
    let es = loc.decimal == ',';
    let weekday = ((serial.floor() as i64 - 2).rem_euclid(7)) as usize; // 0 = Monday
    for t in &sec.toks {
        match t {
            Tok::Lit(l) => out.push_str(l),
            Tok::Date(p) => {
                let h12 = if has_ampm { (hh + 11) % 12 + 1 } else { hh };
                let piece = match p {
                    DatePart::Year2 => format!("{:02}", y.rem_euclid(100)),
                    DatePart::Year4 => format!("{}", y),
                    DatePart::Month => m.to_string(),
                    DatePart::Month2 => format!("{:02}", m),
                    DatePart::MonthShort => {
                        let name = if es { MONTHS_ES } else { MONTHS_EN }[m as usize - 1];
                        name.chars().take(3).collect()
                    }
                    DatePart::MonthLong => (if es { MONTHS_ES } else { MONTHS_EN })[m as usize - 1].to_string(),
                    DatePart::Day => d.to_string(),
                    DatePart::Day2 => format!("{:02}", d),
                    DatePart::DayShort => (if es { DAYS_ES } else { DAYS_EN })[weekday].chars().take(3).collect(),
                    DatePart::DayLong => (if es { DAYS_ES } else { DAYS_EN })[weekday].to_string(),
                    DatePart::Hour => h12.to_string(),
                    DatePart::Hour2 => format!("{:02}", h12),
                    DatePart::Minute => mm.to_string(),
                    DatePart::Minute2 => format!("{:02}", mm),
                    DatePart::Second => ss.to_string(),
                    DatePart::Second2 => format!("{:02}", ss),
                    DatePart::AmPm => (if hh < 12 { "AM" } else { "PM" }).to_string(),
                    DatePart::Elapsed(u, w) => {
                        let n = match u {
                            'h' => whole / 3600,
                            'm' => whole / 60,
                            _ => whole,
                        };
                        format!("{:0w$}", n, w = *w)
                    }
                    DatePart::Frac(n) => {
                        let f = format!("{:0w$}", ticks % unit, w = *n);
                        format!("{}{f}", loc.decimal)
                    }
                };
                out.push_str(&piece);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// User input parsing.

/// Result of interpreting text typed in a cell (non-formula).
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedInput {
    pub value: Value,
    /// Suggested number format when the input implies one (e.g. `10%`).
    pub format: Option<String>,
}

/// Parses a locale-aware number like `1.234,5` (es) or `1,234.5` (en).
pub fn parse_number(s: &str, loc: &Locale) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let mut clean = String::with_capacity(t.len());
    let mut seen_dec = false;
    let mut digits = 0;
    let mut chars = t.chars().peekable();
    if let Some(&c) = chars.peek() {
        if c == '+' || c == '-' {
            clean.push(c);
            chars.next();
        }
    }
    let rest: String = chars.collect();
    // Accept an exponent in canonical form.
    if let Some((m, e)) = rest.split_once(['e', 'E']) {
        if !m.is_empty() && e.trim_start_matches(['+', '-']).chars().all(|c| c.is_ascii_digit()) && !e.is_empty() {
            let m = m.replace(loc.decimal, ".");
            return format!("{}{}e{}", clean, m, e).parse().ok();
        }
    }
    let groups_ok = |int: &str| -> bool {
        // Thousands separators must split the integer part in groups of 3.
        let parts: Vec<&str> = int.split(loc.thousands).collect();
        parts.len() == 1 || (!parts[0].is_empty() && parts[0].len() <= 3 && parts[1..].iter().all(|p| p.len() == 3))
    };
    let (int_part, frac_part) = match rest.split_once(loc.decimal) {
        Some((a, b)) => (a, Some(b)),
        None => (rest.as_str(), None),
    };
    if !groups_ok(int_part) {
        // Also accept the canonical '.' decimal when locale uses ','.
        if loc.decimal != '.' && !rest.contains(loc.decimal) {
            if let Ok(n) = rest.parse::<f64>() {
                if rest.matches('.').count() == 1 {
                    return Some(if clean == "-" { -n } else { n });
                }
            }
        }
        return None;
    }
    for c in int_part.chars() {
        if c.is_ascii_digit() {
            clean.push(c);
            digits += 1;
        } else if c != loc.thousands {
            return None;
        }
    }
    if let Some(f) = frac_part {
        seen_dec = true;
        clean.push('.');
        for c in f.chars() {
            if c.is_ascii_digit() {
                clean.push(c);
                digits += 1;
            } else {
                return None;
            }
        }
    }
    let _ = seen_dec;
    if digits == 0 {
        return None;
    }
    clean.parse().ok()
}

fn parse_date(s: &str, loc: &Locale) -> Option<(f64, String)> {
    let t = s.trim();
    let sep = if t.contains('/') {
        '/'
    } else if t.contains('-') {
        '-'
    } else if t.contains('.') && loc.decimal == ',' {
        '.'
    } else {
        return None;
    };
    let (date_part, time_part) = match t.split_once(' ') {
        Some((d, tm)) => (d, Some(tm)),
        None => (t, None),
    };
    let p: Vec<&str> = date_part.split(sep).collect();
    if p.len() != 3 || p.iter().any(|x| x.is_empty() || !x.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let n: Vec<i64> = p.iter().map(|x| x.parse().unwrap_or(0)).collect();
    let (y, m, d) = if p[0].len() == 4 {
        (n[0], n[1], n[2])
    } else {
        match loc.date_order {
            DateOrder::Dmy => (n[2], n[1], n[0]),
            DateOrder::Mdy => (n[2], n[0], n[1]),
            DateOrder::Ymd => (n[0], n[1], n[2]),
        }
    };
    let y = if p[2].len() <= 2 && p[0].len() != 4 { if y < 30 { 2000 + y } else { 1900 + y } } else { y };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let (_, _, dd) = serial_to_date(date_to_serial(y, m, d));
    if dd as i64 != d {
        return None; // e.g. 31/02
    }
    let mut serial = excel_serial(y, m, d);
    let mut code = if p[0].len() == 4 { "yyyy-mm-dd".to_string() } else { loc.short_date_code().to_string() };
    if let Some(tm) = time_part {
        let (f, c) = parse_time(tm)?;
        serial += f;
        code.push(' ');
        code.push_str(&c);
    }
    Some((serial, code))
}

fn parse_time(s: &str) -> Option<(f64, String)> {
    let p: Vec<&str> = s.trim().split(':').collect();
    if !(2..=3).contains(&p.len()) || p.iter().any(|x| x.is_empty() || x.len() > 2 || !x.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let h: f64 = p[0].parse().ok()?;
    let m: f64 = p[1].parse().ok()?;
    let sec: f64 = p.get(2).map(|x| x.parse().unwrap_or(0.0)).unwrap_or(0.0);
    if h >= 24.0 || m >= 60.0 || sec >= 60.0 {
        return None;
    }
    let code = if p.len() == 3 { "hh:mm:ss" } else { "hh:mm" };
    Some(((h * 3600.0 + m * 60.0 + sec) / 86_400.0, code.to_string()))
}

/// Interprets typed text: numbers, percentages, currency, dates, booleans, text.
/// A leading apostrophe forces text.
pub fn parse_input(s: &str, loc: &Locale) -> ParsedInput {
    let plain = |v: Value| ParsedInput { value: v, format: None };
    if s.is_empty() {
        return plain(Value::Empty);
    }
    if let Some(rest) = s.strip_prefix('\'') {
        return plain(Value::text(rest));
    }
    let t = s.trim();
    if let Some(n) = parse_number(t, loc) {
        return plain(Value::Number(n));
    }
    match t.to_ascii_uppercase().as_str() {
        "TRUE" | "VERDADERO" => return plain(Value::Bool(true)),
        "FALSE" | "FALSO" => return plain(Value::Bool(false)),
        _ => {}
    }
    if let Some(p) = t.strip_suffix('%') {
        if let Some(n) = parse_number(p, loc) {
            let decimals = p.split_once(loc.decimal).map(|(_, f)| f.trim().len()).unwrap_or(0);
            let code = if decimals == 0 { "0%".to_string() } else { format!("0.{}%", "0".repeat(decimals)) };
            return ParsedInput { value: Value::Number(n / 100.0), format: Some(code) };
        }
    }
    for sym in ["€", "$", "£"] {
        let stripped = t.strip_suffix(sym).map(|x| (x, true)).or_else(|| t.strip_prefix(sym).map(|x| (x, false)));
        if let Some((num, after)) = stripped {
            if let Some(n) = parse_number(num.trim(), loc) {
                let code = if after { format!("#,##0.00 \"{}\"", sym) } else { format!("\"{}\"#,##0.00", sym) };
                return ParsedInput { value: Value::Number(n), format: Some(code) };
            }
        }
    }
    if let Some((serial, code)) = parse_date(t, loc) {
        return ParsedInput { value: Value::Number(serial), format: Some(code) };
    }
    if let Some((f, code)) = parse_time(t) {
        return ParsedInput { value: Value::Number(f), format: Some(code) };
    }
    plain(Value::text(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(code: &str, n: f64, loc: &Locale) -> String {
        NumFormat::parse(code).format_number(n, loc).0
    }

    #[test]
    fn general() {
        assert_eq!(general_number(1.0 / 3.0, '.'), "0.3333333333");
        assert_eq!(general_number(1234.5, ','), "1234,5");
        assert_eq!(general_number(1e20, '.'), "1E+20");
        assert_eq!(general_number(1.5e20, '.'), "1.5E+20");
        assert_eq!(general_number(0.1 + 0.2, '.'), "0.3");
        assert_eq!(general_number(123456789012.5, '.'), "1.23457E+11");
    }

    #[test]
    fn accounting() {
        let (es, en) = (Locale::ES, Locale::EN);
        let marked = |code: &str, n: f64, loc: &Locale| NumFormat::parse(code).format_number_marked(n, loc).0.replace(FILL, "|");
        let eur = accounting_code("€", true, 2);
        assert_eq!(marked(&eur, 1234.5, &es), " |1.234,50 € ");
        assert_eq!(marked(&eur, -3.0, &es), "-|3,00 € ");
        assert_eq!(marked(&eur, 0.0, &es), " |-   € ");
        assert_eq!(fmt(&eur, 1234.5, &es), " 1.234,50 € ");
        let usd = accounting_code("$", false, 2);
        assert_eq!(marked(&usd, 1234.5, &en), " $|1,234.50 ");
        assert_eq!(marked(&usd, -3.0, &en), " $|(3.00)");
        assert!(NumFormat::parse(&usd).has_fill());
        assert_eq!(NumFormat::parse(&usd).format(&Value::Text("x".into()), &en).0, " x ");
    }

    #[test]
    fn codes() {
        let es = Locale::ES;
        let en = Locale::EN;
        assert_eq!(fmt("#,##0.00", 1234567.891, &en), "1,234,567.89");
        assert_eq!(fmt("#,##0.00", 1234567.891, &es), "1.234.567,89");
        assert_eq!(fmt("0%", 0.256, &en), "26%");
        assert_eq!(fmt("0.0%", 0.256, &en), "25.6%");
        assert_eq!(fmt("#,##0.00 \"€\"", -5.5, &es), "-5,50 €");
        assert_eq!(fmt("0.00;[Red]-0.00", -2.0, &en), "-2.00");
        assert_eq!(fmt("dd/mm/yyyy", 45000.0, &en), "15/03/2023");
        assert_eq!(fmt("yyyy-mm-dd hh:mm", 45000.5, &en), "2023-03-15 12:00");
        assert_eq!(fmt("d mmmm yyyy", 45000.0, &es), "15 marzo 2023");
        assert_eq!(fmt("0.00E+00", 12345.0, &en), "1.23E+04");
        assert_eq!(fmt("0", 2.5, &en), "3");
        assert_eq!(fmt("#,##0", 999.0, &en), "999");
        assert_eq!(fmt("0.##", 1.5, &en), "1.5");
        // Text between the digits.
        assert_eq!(fmt("000\"-\"0000\"-\"000", 1234567890.0, &en), "123-4567-890");
        assert_eq!(fmt("000\"-\"0000\"-\"000", 4567.0, &en), "000-0004-567");
        assert_eq!(fmt("(###) ###-####", 5551234567.0, &en), "(555) 123-4567");
        assert_eq!(fmt("00\\-00", 12345.0, &en), "123-45");
        assert_eq!(fmt("#,##0,", 1234567.0, &en), "1,235");
        assert_eq!(fmt("0.0,,\"M\"", 1234567.0, &en), "1.2M");
        // Elapsed time and fractions of a second.
        assert_eq!(fmt("[h]:mm:ss", 1.5, &en), "36:00:00");
        assert_eq!(fmt("[mm]:ss", 0.125, &en), "180:00");
        assert_eq!(fmt("mm:ss.00", 1.5 / 86_400.0, &en), "00:01.50");
        assert_eq!(fmt("hh:mm:ss", 0.999_999_9, &en), "00:00:00");
    }

    #[test]
    fn dates() {
        assert_eq!(date_to_serial(1970, 1, 1), 25569.0);
        assert_eq!(date_to_serial(2023, 3, 15), 45000.0);
        assert_eq!(serial_to_date(45000.0), (2023, 3, 15));
        assert_eq!(date_to_serial(2023, 13, 1), date_to_serial(2024, 1, 1));
    }

    #[test]
    fn regional_english_input() {
        for tag in ["en_NZ", "en-NZ", "en_NZ.UTF-8", "EN-gb", "en_GB.UTF-8", "en-GB@euro", "en_AU", "en_IN", "en-IE", "en-Latn-AU", "en-NZ-u-ca-gregory", "en-US-u-rg-gbzzzz"] {
            let locale = Locale::from_tag(tag);
            assert_eq!(locale, Locale { date_order: DateOrder::Dmy, ..Locale::EN }, "{tag}");
            assert_eq!(parse_input("1,234.5", &locale).value, Value::Number(1234.5));
            assert_eq!(parse_input("31/12/2020", &locale).value, Value::Number(date_to_serial(2020, 12, 31)));
            assert_eq!(parse_input("1/4/2021", &locale).value, Value::Number(date_to_serial(2021, 4, 1)));
        }
        for tag in ["en", "en_US.UTF-8", "en-US", "C", "", "en_NZZ", "en_GBextra", "en-GB-u-rg-uszzzz"] {
            let locale = Locale::from_tag(tag);
            assert_eq!(locale, Locale::EN, "{tag}");
            assert_eq!(parse_input("1/4/2021", &locale).value, Value::Number(date_to_serial(2021, 1, 4)));
        }
        assert_eq!(Locale::from_tag("es_ES.UTF-8"), Locale::ES);
    }

    #[test]
    fn cldr_date_orders() {
        for (tag, order, text, date) in [
            ("en_US", DateOrder::Mdy, "4/1/2021", (2021, 4, 1)),
            ("en_CA", DateOrder::Ymd, "21/4/1", (2021, 4, 1)),
            ("en_ZA", DateOrder::Ymd, "21/4/1", (2021, 4, 1)),
            ("fr_CA", DateOrder::Ymd, "21/4/1", (2021, 4, 1)),
            ("ja_JP", DateOrder::Ymd, "21/4/1", (2021, 4, 1)),
            ("zh_HK", DateOrder::Dmy, "1/4/2021", (2021, 4, 1)),
            ("zh-Hant-HK", DateOrder::Dmy, "1/4/2021", (2021, 4, 1)),
            ("iw_IL", DateOrder::Dmy, "1/4/2021", (2021, 4, 1)),
            ("de_DE", DateOrder::Dmy, "1/4/2021", (2021, 4, 1)),
        ] {
            let locale = Locale::from_tag(tag);
            assert_eq!(locale.date_order, order, "{tag}");
            assert_eq!(parse_input(text, &locale).value, Value::Number(date_to_serial(date.0, date.1, date.2)), "{tag}");
        }
        for tag in ["", "xx_YY", "en_NZZ", "en-123456789"] {
            assert_eq!(DateOrder::from_tag(tag), None, "{tag}");
        }
    }

    #[test]
    fn input() {
        let es = Locale::ES;
        assert_eq!(parse_input("1.234,5", &es).value, Value::Number(1234.5));
        assert_eq!(parse_input("3,5", &es).value, Value::Number(3.5));
        assert_eq!(parse_input("3.5", &es).value, Value::Number(3.5));
        assert_eq!(parse_input("10%", &es).value, Value::Number(0.1));
        assert_eq!(parse_input("12,50 €", &es).value, Value::Number(12.5));
        assert_eq!(parse_input("15/03/2023", &es).value, Value::Number(45000.0));
        assert_eq!(parse_input("31/02/2023", &es).value, Value::text("31/02/2023"));
        assert_eq!(parse_input("hola", &es).value, Value::text("hola"));
        assert_eq!(parse_input("'123", &es).value, Value::text("123"));
        assert_eq!(parse_input("1,234.5", &Locale::EN).value, Value::Number(1234.5));
        assert_eq!(parse_input("1,23", &Locale::EN).value, Value::text("1,23"));
    }
}
