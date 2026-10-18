//! OpenDocument spreadsheets (.ods).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::format::{date_to_serial, serial_to_date};
use apex_sheet::formula::parse_canonical;
use apex_sheet::protect::allow;
use apex_sheet::sheet::{ROTATION_STACKED, border};
use apex_sheet::{Borders, CellRef, CellStyle, Content, ErrorKind, HAlign, Line, LineStyle, Link, MAX_COLS, MAX_ROWS, Note, Password, Protection, Range, Sheet, VAlign, Value, Workbook};

use crate::xml::{Ev, HEADER, Reader, Tag, esc, local};
use crate::zip::{ZipReader, ZipWriter};
use crate::{Error, parse_hex_color, parse_length_pt};

pub(crate) mod chart;
mod data;
mod tables;
mod images;
mod named;
mod scenarios;
mod shapes;
mod controls;
mod pivot;
pub(crate) mod page;

const MIME: &str = "application/vnd.oasis.opendocument.spreadsheet";
// Logical pixels per point for column widths and row heights.
const COL_PX_PER_PT: f32 = 88.0 / 64.0;
const ROW_PX_PER_PT: f32 = 22.0 / 15.0;
/// Empty styled repeats above this are treated as "rest of sheet" filler.
const MAX_STYLED_REPEAT: u32 = 1024;
/// Blank formatted cells a sheet keeps before dropping invisible ones.
const BLANK_BUDGET: u64 = 1_000_000;
/// Sheets a book keeps.
const MAX_SHEETS: usize = 1024;
/// Cells repeated or formatted blank a book keeps, all its sheets together.
const BOOK_COPY_BUDGET: u64 = 60_000_000;
/// Cells with values a workbook may get by repetition.
const COPY_BUDGET: u64 = 20_000_000;

// ---------------------------------------------------------------------------
// Formula conversion

/// Converts an ODF formula (`of:=SUM([.A1:.B2];[$Hoja2.C3])`) to canonical
/// Excel-style syntax (`SUM(A1:B2,Hoja2!C3)`).
pub fn odf_to_canonical(src: &str) -> String {
    let s = src.trim();
    let s = s.split_once(':').filter(|(ns, _)| matches!(*ns, "of" | "oooc" | "msoxl")).map_or(s, |(_, r)| r);
    let s = s.strip_prefix('=').unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    // Inside an inline array `;` separates columns and `|` rows.
    let mut braces = 0;
    // Whether the last thing written was a reference.
    let mut after_ref = false;
    while let Some((i, c)) = chars.next() {
        let prev_ref = std::mem::replace(&mut after_ref, false);
        match c {
            '{' => {
                braces += 1;
                out.push(c);
            }
            '}' => {
                braces -= 1;
                out.push(c);
            }
            '|' if braces > 0 => out.push(';'),
            '"' => {
                out.push('"');
                while let Some((_, c2)) = chars.next() {
                    out.push(c2);
                    if c2 == '"' {
                        if chars.peek().map(|x| x.1) == Some('"') {
                            out.push('"');
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            // A structured reference to a table: `Table1[[#Data];[Qty]]`.
            '[' if out.ends_with(|p: char| p.is_alphanumeric() || p == '_' || p == '.') => {
                out.push('[');
                let mut depth = 1;
                while let Some((_, c2)) = chars.next() {
                    match c2 {
                        // `'` escapes the next character.
                        '\'' => {
                            out.push(c2);
                            if let Some((_, c3)) = chars.next() {
                                out.push(c3);
                            }
                            continue;
                        }
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {}
                    }
                    out.push(if c2 == ';' { ',' } else { c2 });
                    if depth == 0 {
                        break;
                    }
                }
                after_ref = true;
            }
            '[' => {
                // Find the closing bracket, skipping quoted sheet names.
                let mut end = None;
                let mut in_q = false;
                for (j, c2) in s[i + 1..].char_indices() {
                    match c2 {
                        '\'' => in_q = !in_q,
                        ']' if !in_q => {
                            end = Some(i + 1 + j);
                            break;
                        }
                        _ => {}
                    }
                }
                let Some(end) = end else {
                    out.push(c);
                    continue;
                };
                out.push_str(&convert_ref(&s[i + 1..end]));
                after_ref = true;
                while chars.peek().is_some_and(|&(j, _)| j <= end) {
                    chars.next();
                }
            }
            ';' => out.push(','),
            // Reference operators: `!` intersection, `~` union.
            '!' if prev_ref || out.ends_with(')') => out.push(' '),
            '~' => out.push(','),
            c if (c.is_alphabetic() || c == '_') && !out.ends_with(|p: char| p.is_alphanumeric() || p == '_' || p == '.') => {
                let mut end = i + c.len_utf8();
                while let Some(&(j, c2)) = chars.peek() {
                    if c2.is_alphanumeric() || c2 == '_' || c2 == '.' {
                        end = j + c2.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                let word = &s[i..end];
                // LibreOffice writes the logical constants as functions.
                if (word.eq_ignore_ascii_case("TRUE") || word.eq_ignore_ascii_case("FALSE")) && s[end..].starts_with("()") {
                    out.push_str(&word.to_ascii_uppercase());
                    chars.next();
                    chars.next();
                } else if chars.peek().is_some_and(|x| x.1 == '(') {
                    out.push_str(&odf_function(word));
                } else {
                    out.push_str(word);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Maps a function name as LibreOffice writes it to the Excel one.
fn odf_function(name: &str) -> String {
    let up = name.to_ascii_uppercase();
    for p in ["COM.MICROSOFT.", "LEGACY."] {
        if let Some(rest) = up.strip_prefix(p) {
            return rest.to_string();
        }
    }
    match up.as_str() {
        "FDIST" => "F.DIST".into(),
        "FINV" => "F.INV".into(),
        "SKEWP" => "SKEW.P".into(),
        "FORMULA" => "FORMULATEXT".into(),
        _ => name.to_string(),
    }
}

fn convert_ref(r: &str) -> String {
    // Split on ':' outside quotes.
    let mut parts = Vec::new();
    let mut in_q = false;
    let mut start = 0;
    for (i, c) in r.char_indices() {
        match c {
            '\'' => in_q = !in_q,
            ':' if !in_q => {
                parts.push(&r[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&r[start..]);
    let mut sheet: Option<String> = None;
    let mut addrs = Vec::new();
    for (k, p) in parts.iter().enumerate() {
        let p = p.trim().trim_start_matches('$');
        let (sh, addr) = if let Some(rest) = p.strip_prefix('.') {
            (None, rest)
        } else if let Some(q) = p.strip_prefix('\'') {
            // Quoted sheet name with '' escapes.
            let mut name = String::new();
            let mut it = q.char_indices().peekable();
            let mut after = q.len();
            while let Some((j, c)) = it.next() {
                if c == '\'' {
                    if it.peek().map(|x| x.1) == Some('\'') {
                        name.push('\'');
                        it.next();
                    } else {
                        after = j + 1;
                        break;
                    }
                } else {
                    name.push(c);
                }
            }
            let rest = &q[after..];
            // Another document: `'file:///home/u/Budget.ods'#$Sheet1.A1`.
            if let Some(inner) = rest.strip_prefix('#') {
                let inner = inner.trim_start_matches('$');
                let (sheet, addr) = match inner.strip_prefix('\'') {
                    Some(qs) => match qs.find("'.") {
                        Some(e) => (qs[..e].replace("''", "'"), &qs[e + 2..]),
                        None => (qs.to_string(), ""),
                    },
                    None => inner.rsplit_once('.').map_or((inner.to_string(), ""), |(s, a)| (s.to_string(), a)),
                };
                (Some(format!("[{name}]{sheet}")), addr)
            } else {
                (Some(name), rest.trim_start_matches('.'))
            }
        } else if let Some((a, b)) = p.rsplit_once('.') {
            (Some(a.to_string()), b)
        } else {
            (None, p)
        };
        if k == 0 {
            sheet = sh;
        } else if let (Some(first), Some(last)) = (&sheet, sh) {
            // A 3D reference: `[$Jan.A1:$Mar.A1]`.
            if !first.eq_ignore_ascii_case(&last) {
                sheet = Some(format!("{first}:{last}"));
            }
        }
        if addr.contains("#REF!") {
            return "#REF!".into();
        }
        addrs.push(addr.to_string());
    }
    // `[$Jan.A1:$Mar.A1]` is one cell through the sheets: `Jan:Mar!A1`.
    if addrs.len() == 2 && addrs[0] == addrs[1] && sheet.as_deref().is_some_and(|s| !s.starts_with('[') && s.contains(':')) {
        addrs.pop();
    }
    let mut out = String::new();
    if let Some(sh) = sheet {
        let plain = |x: &str| x.chars().all(|c| c.is_alphanumeric() || c == '_') && !x.starts_with(|c: char| c.is_ascii_digit());
        if !sh.starts_with('[') && sh.split(':').all(plain) {
            out.push_str(&sh);
        } else {
            out.push('\'');
            out.push_str(&sh.replace('\'', "''"));
            out.push('\'');
        }
        out.push('!');
    }
    out.push_str(&addrs.join(":"));
    out
}

// ---------------------------------------------------------------------------
// Number styles

/// Reads the children of a `number:*-style` element into a format code.
fn read_number_style(r: &mut Reader, kind: &str, style_tag: &Tag) -> String {
    let mut code = String::new();
    let truncate = style_tag.attr("number:truncate-on-overflow").is_none_or(|v| v != "false");
    let mut depth = 1;
    let mut first_hours = true;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ev @ (Ev::Start(_) | Ev::Empty(_)) => {
                let (t, is_start) = match ev {
                    Ev::Start(t) => (t, true),
                    Ev::Empty(t) => (t, false),
                    _ => unreachable!(),
                };
                let long = t.attr("number:style").as_deref() == Some("long");
                let dp: usize = t.attr("number:decimal-places").and_then(|v| v.parse().ok()).unwrap_or(0usize).min(30);
                match t.local() {
                    "number" => {
                        let min_int: usize = t.attr("number:min-integer-digits").and_then(|v| v.parse().ok()).unwrap_or(1usize).min(40);
                        let grouping = t.attr("number:grouping").as_deref() == Some("true");
                        let embedded = if is_start { embedded_texts(r) } else { Vec::new() };
                        if !embedded.is_empty() {
                            push_embedded(&mut code, min_int, &embedded);
                        } else if grouping {
                            code.push_str("#,##");
                            code.push_str(&"0".repeat(min_int.max(1)));
                        } else if min_int == 0 {
                            code.push('#');
                        } else {
                            code.push_str(&"0".repeat(min_int));
                        }
                        if dp > 0 {
                            code.push('.');
                            code.push_str(&"0".repeat(dp));
                        }
                        // Its children were read through its end.
                        if is_start {
                            continue;
                        }
                    }
                    "scientific-number" => {
                        let exp: usize = t.attr("number:min-exponent-digits").and_then(|v| v.parse().ok()).unwrap_or(2usize).min(10);
                        code.push('0');
                        if dp > 0 {
                            code.push('.');
                            code.push_str(&"0".repeat(dp));
                        }
                        code.push_str("E+");
                        code.push_str(&"0".repeat(exp.max(1)));
                    }
                    "fraction" => {
                        let min_int: usize = t.attr("number:min-integer-digits").and_then(|v| v.parse().ok()).unwrap_or(0usize).min(40);
                        code.push_str(&if min_int == 0 { "#".to_string() } else { "0".repeat(min_int) });
                        code.push_str(" ?/?");
                    }
                    "day" => code.push_str(if long { "dd" } else { "d" }),
                    "day-of-week" => code.push_str(if long { "dddd" } else { "ddd" }),
                    "month" => {
                        let textual = t.attr("number:textual").as_deref() == Some("true");
                        code.push_str(match (textual, long) {
                            (true, true) => "mmmm",
                            (true, false) => "mmm",
                            (false, true) => "mm",
                            (false, false) => "m",
                        });
                    }
                    "year" => code.push_str(if long { "yyyy" } else { "yy" }),
                    "hours" => {
                        let h = if long { "hh" } else { "h" };
                        if !truncate && first_hours {
                            code.push('[');
                            code.push_str(h);
                            code.push(']');
                        } else {
                            code.push_str(h);
                        }
                        first_hours = false;
                    }
                    "minutes" => code.push_str(if long { "mm" } else { "m" }),
                    "seconds" => {
                        code.push_str(if long { "ss" } else { "s" });
                        if dp > 0 {
                            code.push('.');
                            code.push_str(&"0".repeat(dp));
                        }
                    }
                    "am-pm" => code.push_str("AM/PM"),
                    "text-content" => code.push('@'),
                    "fill-character" if is_start => {
                        let txt = r.text();
                        code.push('*');
                        code.push(txt.chars().next().unwrap_or(' '));
                        continue;
                    }
                    "text" | "currency-symbol" if is_start => {
                        let txt = r.text();
                        push_literal(&mut code, &txt);
                        continue;
                    }
                    _ => {}
                }
                if is_start {
                    // Unknown containers (style:map, text-properties...): skip.
                    r.skip();
                }
            }
            _ => {}
        }
    }
    if code.is_empty() {
        return match kind {
            "boolean-style" => "General".into(),
            "text-style" => "@".into(),
            _ => "General".into(),
        };
    }
    code
}

/// The number:embedded-text children of a number:number (read through
/// its end), as (position, text).
fn embedded_texts(r: &mut Reader) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    loop {
        match r.next() {
            Ev::Start(t) if t.local() == "embedded-text" => {
                let position = t.attr("number:position").and_then(|v| v.parse().ok()).unwrap_or(0usize).min(40);
                out.push((position, r.text()));
            }
            Ev::Start(_) => r.skip(),
            Ev::End(_) | Ev::Eof => break,
            _ => {}
        }
    }
    out
}

/// Integer digits with texts among them: `position` digits to the right
/// of each text, and a digit at least before each.
fn push_embedded(code: &mut String, min_int: usize, texts: &[(usize, String)]) {
    let total = texts.iter().map(|(p, _)| *p + 1).max().unwrap_or(0).max(min_int);
    for i in (0..total).rev() {
        code.push(if i < min_int { '0' } else { '#' });
        for (_, text) in texts.iter().filter(|(p, _)| *p == i) {
            push_literal(code, text);
        }
    }
}

fn push_literal(code: &mut String, txt: &str) {
    let plain = |c: char| matches!(c, ' ' | '-' | '/' | ':' | ',' | '.' | '(' | ')');
    if txt == "%" || txt.chars().all(plain) {
        code.push_str(txt);
        return;
    }
    // Keep surrounding punctuation unquoted: " €" -> ` "€"`.
    let core = txt.trim_matches(plain);
    let lead = &txt[..txt.len() - txt.trim_start_matches(plain).len()];
    let trail = &txt[lead.len() + core.len()..];
    code.push_str(lead);
    code.push('"');
    code.push_str(&core.replace('"', ""));
    code.push('"');
    code.push_str(trail);
}

#[derive(Debug)]
enum Tok {
    Lit(String),
    /// `digits`: the integer placeholders (0, # and ?).
    Num { int_zeros: usize, digits: usize, dp: usize, grouping: bool },
    Sci { dp: usize, exp: usize },
    Frac,
    Percent,
    Currency(String),
    Date(&'static str, bool),
    Text,
    /// `*x`: repeat x to fill the cell.
    Fill(char),
}

/// Splits the first section of a format code into tokens for ODF output.
fn tokenize(code: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut lit = String::new();
    let flush = |lit: &mut String, toks: &mut Vec<Tok>| {
        if !lit.is_empty() {
            toks.push(Tok::Lit(std::mem::take(lit)));
        }
    };
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    let mut last_was_hours = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ';' => break,
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    lit.push(chars[i]);
                    i += 1;
                }
                i += 1;
                continue;
            }
            '\\' => {
                if let Some(&n) = chars.get(i + 1) {
                    lit.push(n);
                }
                i += 2;
                continue;
            }
            '*' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Fill(chars.get(i + 1).copied().unwrap_or(' ')));
                i += 2;
                continue;
            }
            '_' => {
                i += 2;
                continue;
            }
            '[' => {
                let end = chars[i..].iter().position(|&x| x == ']').map_or(chars.len(), |p| i + p);
                let inner: String = chars[i + 1..end.min(chars.len())].iter().collect();
                if let Some(cur) = inner.strip_prefix('$') {
                    let sym = cur.split('-').next().unwrap_or("");
                    if !sym.is_empty() {
                        flush(&mut lit, &mut toks);
                        toks.push(Tok::Currency(sym.to_string()));
                    }
                } else if inner.eq_ignore_ascii_case("h") || inner.eq_ignore_ascii_case("hh") {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::Date("hours-elapsed", inner.len() == 2));
                    last_was_hours = true;
                }
                i = end + 1;
                continue;
            }
            '0' | '#' | '?' | ',' | '.' if !matches!(c, ',' | '.') || i + 1 < chars.len() && matches!(chars[i + 1], '0' | '#' | '?') => {
                // Numeric block.
                let start = i;
                while i < chars.len() && matches!(chars[i], '0' | '#' | '?' | ',' | '.' | '/') {
                    i += 1;
                }
                let block: String = chars[start..i].iter().collect();
                flush(&mut lit, &mut toks);
                if block.contains('/') {
                    toks.push(Tok::Frac);
                    continue;
                }
                let (int, frac) = block.split_once('.').unwrap_or((&block, ""));
                let dp = frac.chars().filter(|c| matches!(c, '0' | '#' | '?')).count();
                if i < chars.len() && matches!(chars[i], 'E' | 'e') && matches!(chars.get(i + 1), Some('+') | Some('-')) {
                    i += 2;
                    let mut exp = 0;
                    while i < chars.len() && chars[i] == '0' {
                        exp += 1;
                        i += 1;
                    }
                    toks.push(Tok::Sci { dp, exp: exp.max(1) });
                    continue;
                }
                let digits = int.chars().filter(|c| matches!(c, '0' | '#' | '?')).count();
                toks.push(Tok::Num { int_zeros: int.chars().filter(|&c| c == '0').count(), digits, dp, grouping: int.contains(',') });
                continue;
            }
            '%' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Percent);
            }
            '@' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Text);
            }
            '€' | '$' | '£' | '¥' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Currency(c.to_string()));
            }
            'd' | 'D' | 'y' | 'Y' | 'h' | 'H' | 's' | 'S' | 'm' | 'M' => {
                let lc = c.to_ascii_lowercase();
                let mut n = 0;
                while i < chars.len() && chars[i].to_ascii_lowercase() == lc {
                    n += 1;
                    i += 1;
                }
                flush(&mut lit, &mut toks);
                let tok = match lc {
                    'd' => match n {
                        1 => Tok::Date("day", false),
                        2 => Tok::Date("day", true),
                        3 => Tok::Date("day-of-week", false),
                        _ => Tok::Date("day-of-week", true),
                    },
                    'y' => Tok::Date("year", n > 2),
                    'h' => Tok::Date("hours", n > 1),
                    's' => Tok::Date("seconds", n > 1),
                    _ => {
                        // Minutes when after hours or before seconds.
                        let rest: String = chars[i..].iter().collect::<String>().to_ascii_lowercase();
                        let before_secs = rest.trim_start_matches(|c: char| !c.is_ascii_alphabetic()).starts_with('s');
                        if (last_was_hours || before_secs) && n <= 2 {
                            Tok::Date("minutes", n > 1)
                        } else {
                            match n {
                                1 => Tok::Date("month", false),
                                2 => Tok::Date("month", true),
                                3 => Tok::Date("month-text", false),
                                _ => Tok::Date("month-text", true),
                            }
                        }
                    }
                };
                last_was_hours = matches!(tok, Tok::Date("hours" | "hours-elapsed", _));
                toks.push(tok);
                continue;
            }
            'A' | 'a' if chars[i..].iter().take(5).collect::<String>().eq_ignore_ascii_case("AM/PM") => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Date("am-pm", false));
                i += 5;
                continue;
            }
            _ => lit.push(c),
        }
        i += 1;
    }
    flush(&mut lit, &mut toks);
    toks
}

/// The texts between blocks of integer digits at the start of `toks`
/// (`0000"-"000`), with the digits to the right of each; empty when the
/// number isn't split so.
fn split_number(toks: &[Tok]) -> Vec<(&str, usize)> {
    let mut texts = Vec::new();
    let mut j = 0;
    while let (Some(Tok::Num { dp: 0, grouping: false, .. }), Some(Tok::Lit(text)), Some(Tok::Num { grouping: false, .. })) = (toks.get(j), toks.get(j + 1), toks.get(j + 2)) {
        texts.push(text.as_str());
        j += 2;
    }
    let digits = |t: &Tok| if let Tok::Num { digits, .. } = t { *digits } else { 0 };
    texts
        .into_iter()
        .enumerate()
        .map(|(k, text)| (text, toks[2 * k + 2..=j].iter().map(digits).sum()))
        .collect()
}

/// Writes an ODF number style for a format code, or None for General.
fn write_number_style(name: &str, code: &str) -> Option<String> {
    if code.eq_ignore_ascii_case("general") || code.is_empty() {
        return None;
    }
    let toks = tokenize(code);
    let has_date = toks.iter().any(|t| matches!(t, Tok::Date(k, _) if matches!(*k, "day" | "day-of-week" | "month" | "month-text" | "year")));
    let has_time = toks.iter().any(|t| matches!(t, Tok::Date(k, _) if matches!(*k, "hours" | "hours-elapsed" | "minutes" | "seconds" | "am-pm")));
    let elem = if has_date {
        "date-style"
    } else if has_time {
        "time-style"
    } else if toks.iter().any(|t| matches!(t, Tok::Percent)) {
        "percentage-style"
    } else if toks.iter().any(|t| matches!(t, Tok::Currency(_))) {
        "currency-style"
    } else if toks.iter().any(|t| matches!(t, Tok::Text)) {
        "text-style"
    } else {
        "number-style"
    };
    let mut s = String::new();
    let _ = write!(s, r#"<number:{} style:name="{}""#, elem, name);
    if toks.iter().any(|t| matches!(t, Tok::Date("hours-elapsed", _))) {
        s.push_str(r#" number:truncate-on-overflow="false""#);
    }
    s.push('>');
    let mut pending_text = String::new();
    let flush = |s: &mut String, p: &mut String| {
        if !p.is_empty() {
            let _ = write!(s, "<number:text>{}</number:text>", esc(p));
            p.clear();
        }
    };
    let mut skip = 0;
    for (i, t) in toks.iter().enumerate() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        match t {
            Tok::Lit(l) => pending_text.push_str(l),
            Tok::Percent => pending_text.push('%'),
            Tok::Currency(c) => {
                flush(&mut s, &mut pending_text);
                if elem == "currency-style" {
                    let _ = write!(s, "<number:currency-symbol>{}</number:currency-symbol>", esc(c));
                } else {
                    pending_text.push_str(c);
                }
            }
            Tok::Num { int_zeros, dp, grouping, .. } => {
                flush(&mut s, &mut pending_text);
                let grouping = if *grouping { r#" number:grouping="true""# } else { "" };
                // `# ?/?`: the whole part belongs to the fraction element.
                if let (Some(Tok::Lit(l)), Some(Tok::Frac)) = (toks.get(i + 1), toks.get(i + 2))
                    && l.trim().is_empty()
                {
                    let _ = write!(s, r#"<number:fraction number:min-integer-digits="{int_zeros}" number:min-numerator-digits="1" number:min-denominator-digits="1"/>"#);
                    skip = 2;
                    continue;
                }
                let split = split_number(&toks[i..]);
                if split.is_empty() {
                    let _ = write!(s, r#"<number:number number:decimal-places="{dp}" number:min-decimal-places="{dp}" number:min-integer-digits="{int_zeros}"{grouping}/>"#);
                    continue;
                }
                // `0000"-"000`: one number with the texts embedded (ODF
                // allows one number element per style).
                let last = split.len() * 2;
                let (zeros, dp) = toks[i..=i + last].iter().fold((0, 0), |(z, d), t| match t {
                    Tok::Num { int_zeros, dp, .. } => (z + int_zeros, *dp),
                    _ => (z, d),
                });
                let _ = write!(s, r#"<number:number number:decimal-places="{dp}" number:min-decimal-places="{dp}" number:min-integer-digits="{zeros}">"#);
                for (text, position) in split {
                    let _ = write!(s, r#"<number:embedded-text number:position="{position}">{}</number:embedded-text>"#, esc(text));
                }
                s.push_str("</number:number>");
                skip = last;
            }
            Tok::Sci { dp, exp } => {
                flush(&mut s, &mut pending_text);
                let _ = write!(s, r#"<number:scientific-number number:decimal-places="{}" number:min-integer-digits="1" number:min-exponent-digits="{}"/>"#, dp, exp);
            }
            Tok::Frac => {
                flush(&mut s, &mut pending_text);
                s.push_str(r#"<number:fraction number:min-integer-digits="0" number:min-numerator-digits="1" number:min-denominator-digits="1"/>"#);
            }
            Tok::Text => {
                flush(&mut s, &mut pending_text);
                s.push_str("<number:text-content/>");
            }
            Tok::Fill(c) => {
                flush(&mut s, &mut pending_text);
                let _ = write!(s, "<number:fill-character>{}</number:fill-character>", esc(&c.to_string()));
            }
            Tok::Date(kind, long) => {
                flush(&mut s, &mut pending_text);
                let style = if *long { r#" number:style="long""# } else { "" };
                match *kind {
                    "month-text" => {
                        let _ = write!(s, r#"<number:month number:textual="true"{}/>"#, style);
                    }
                    "hours-elapsed" => {
                        let _ = write!(s, "<number:hours{}/>", style);
                    }
                    "am-pm" => s.push_str("<number:am-pm/>"),
                    // `ss.00`: fractions of a second.
                    "seconds" if matches!(toks.get(i + 1), Some(Tok::Num { digits: 0, dp: 1.., .. })) => {
                        let Some(Tok::Num { dp, .. }) = toks.get(i + 1) else { continue };
                        let _ = write!(s, r#"<number:seconds{style} number:decimal-places="{dp}"/>"#);
                        skip = 1;
                    }
                    k => {
                        let _ = write!(s, "<number:{}{}/>", k, style);
                    }
                }
            }
        }
    }
    flush(&mut s, &mut pending_text);
    let _ = write!(s, "</number:{}>", elem);
    Some(s)
}

// ---------------------------------------------------------------------------
// Reading

#[derive(Default, Clone)]
struct OdfStyle {
    parent: Option<String>,
    cell: CellStyle,
    /// What a text style (`style:family="text"`) changes in a span.
    run: apex_sheet::rich::RunFont,
    data_style: Option<String>,
    col_width: Option<f32>,
    row_height: Option<f32>,
    /// Table (sheet) properties.
    hidden: bool,
    tab_color: Option<u32>,
    master_page: Option<String>,
    /// Row or column starting a new page.
    page_break: bool,
    /// Which cell fields this style sets, so parents only fill the rest.
    set_borders: bool,
    set_font: bool,
    set_indent: bool,
    set_rotation: bool,
    set_protect: bool,
    /// A common style of styles.xml (not an automatic one).
    common: bool,
    table_cell: bool,
    display: Option<String>,
    /// The parts of a cell format it sets (for named styles).
    parts: u8,
}

#[derive(Default)]
struct Styles {
    styles: HashMap<String, OdfStyle>,
    numbers: HashMap<String, String>,
    /// Font face declaration name -> family.
    fonts: HashMap<String, String>,
    /// The cells' default size (tenths of a point): shown at the
    /// program's default.
    base_size: Option<u16>,
}

/// The size of cells without one (points): the program's default.
const BASE_PT: u16 = 11;

/// Points per indentation level of a cell.
const INDENT_PT: f32 = 10.0;

/// Parses an ODF border like "0.74pt solid #000000".
fn parse_border(v: &str) -> Line {
    let mut width = None;
    let mut style = None;
    let mut color = None;
    for part in v.split_whitespace() {
        if let Some(c) = parse_hex_color(part).filter(|_| part.starts_with('#')) {
            color = Some(c);
        } else if let Some(w) = parse_length_pt(part) {
            width = Some(w);
        } else {
            style = Some(part);
        }
    }
    let w = width.unwrap_or(0.75);
    let style = match style.unwrap_or("solid") {
        "none" | "hidden" => return Line::default(),
        "double" | "double-thin" => LineStyle::Double,
        "dashed" | "dash-dot" | "dash-dot-dot" | "long-dash" => LineStyle::Dashed,
        "dotted" | "fine-dashed" => LineStyle::Dotted,
        _ if w <= 0.0 => return Line::default(),
        _ if w < 0.5 => LineStyle::Hair,
        _ if w < 1.3 => LineStyle::Thin,
        _ if w < 2.2 => LineStyle::Medium,
        _ => LineStyle::Thick,
    };
    Line::new(style, color.filter(|&c| c != 0))
}

fn write_border(l: Line) -> String {
    let (w, st) = match l.style {
        LineStyle::None => return "none".into(),
        LineStyle::Hair => ("0.05pt", "solid"),
        LineStyle::Thin => ("0.74pt", "solid"),
        LineStyle::Medium => ("1.76pt", "solid"),
        LineStyle::Thick => ("2.49pt", "solid"),
        LineStyle::Dashed => ("0.74pt", "dashed"),
        LineStyle::Dotted => ("0.74pt", "dotted"),
        LineStyle::Double => ("2.01pt", "double"),
    };
    format!("{w} {st} {}", color(l.color.unwrap_or(0)))
}

/// The font changes of a text style's `style:text-properties`.
fn read_run_font(t: &Tag, fonts: &HashMap<String, String>) -> apex_sheet::rich::RunFont {
    use apex_sheet::rich::{RunFont, Script};
    RunFont {
        bold: t.attr("fo:font-weight").map(|v| v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600)),
        italic: t.attr("fo:font-style").map(|v| v == "italic" || v == "oblique"),
        underline: t.attr("style:text-underline-style").map(|v| v != "none"),
        strike: t.attr("style:text-line-through-style").map(|v| v != "none"),
        color: t.attr("fo:color").and_then(|v| parse_hex_color(&v)),
        size: t.attr("fo:font-size").and_then(|v| parse_length_pt(&v)),
        font: t.attr("style:font-name").map(|n| fonts.get(n.as_ref()).cloned().unwrap_or_else(|| n.into_owned())).or_else(|| t.attr("fo:font-family").map(|f| f.trim_matches(['\'', '"']).to_string())).map(|f| Arc::from(f.as_str())),
        script: match t.attr("style:text-position").as_deref().and_then(|v| v.split_whitespace().next()) {
            Some("super") => Script::Super,
            Some("sub") => Script::Sub,
            Some(p) if p.trim_end_matches('%').parse::<f32>().is_ok_and(|x| x > 0.0) => Script::Super,
            Some(p) if p.trim_end_matches('%').parse::<f32>().is_ok_and(|x| x < 0.0) => Script::Sub,
            _ => Script::Normal,
        },
    }
}

fn read_styles(xml: &str, st: &mut Styles) {
    use apex_sheet::cellstyles::part;
    let mut r = Reader::new(xml);
    let mut cur: Option<(String, OdfStyle)> = None;
    let mut common = false;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "office:styles" => common = true,
            Ev::End("office:styles") => common = false,
            Ev::Start(t) | Ev::Empty(t) => {
                let is_start = matches!(ev, Ev::Start(_));
                match t.name {
                    "style:font-face" => {
                        if let (Some(n), Some(f)) = (t.attr("style:name"), t.attr("svg:font-family")) {
                            st.fonts.insert(n.into_owned(), f.trim_matches(['\'', '"']).to_string());
                        }
                    }
                    "style:table-properties" => {
                        if let Some((_, s)) = &mut cur {
                            s.hidden = t.attr("table:display").is_some_and(|v| v == "false");
                            s.tab_color = t.attr("tableooo:tab-color").and_then(|v| parse_hex_color(&v));
                        }
                    }
                    "style:style" | "style:default-style" => {
                        let name = if t.name == "style:default-style" { "__default".to_string() } else { t.attr("style:name").unwrap_or_default().into_owned() };
                        let data_style = t.attr("style:data-style-name").map(|v| v.into_owned());
                        let s = OdfStyle {
                            parent: t.attr("style:parent-style-name").map(|v| v.into_owned()),
                            parts: if data_style.is_some() { part::NUMBER } else { 0 },
                            data_style,
                            master_page: t.attr("style:master-page-name").map(|v| v.into_owned()),
                            common,
                            table_cell: t.attr("style:family").is_some_and(|f| f == "table-cell"),
                            display: t.attr("style:display-name").map(|v| v.into_owned()),
                            ..Default::default()
                        };
                        if is_start {
                            cur = Some((name, s));
                        } else {
                            st.styles.insert(name, s);
                        }
                    }
                    "style:text-properties" => {
                        if let Some((_, s)) = &mut cur {
                            s.run = read_run_font(&t, &st.fonts);
                            s.parts |= part::FONT;
                            let c = &mut s.cell;
                            if let Some(v) = t.attr("fo:font-weight") {
                                c.bold = v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600);
                            }
                            if let Some(v) = t.attr("fo:font-style") {
                                c.italic = v == "italic" || v == "oblique";
                            }
                            if let Some(v) = t.attr("style:text-underline-style") {
                                c.underline = v != "none";
                            }
                            if let Some(v) = t.attr("style:text-line-through-style") {
                                c.strike = v != "none";
                            }
                            if let Some(pt) = t.attr("fo:font-size").and_then(|v| parse_length_pt(&v)) {
                                c.font_size = (pt * 10.0).round() as u16;
                            }
                            if let Some(col) = t.attr("fo:color").and_then(|v| parse_hex_color(&v)) {
                                c.color = Some(col);
                            }
                            if let Some(f) = t.attr("style:font-name").or_else(|| t.attr("fo:font-family")) {
                                c.font = Some(Arc::from(f.trim_matches(['\'', '"'])));
                                s.set_font = true;
                            }
                        }
                    }
                    "style:table-cell-properties" => {
                        if let Some((_, s)) = &mut cur {
                            let c = &mut s.cell;
                            if let Some(v) = t.attr("fo:background-color") {
                                c.fill = parse_hex_color(&v);
                                s.parts |= part::FILL;
                            }
                            if let Some(v) = t.attr("fo:border") {
                                c.borders = Borders::sides(border::ALL, parse_border(&v));
                                s.set_borders = true;
                            }
                            for (k, f) in [("fo:border-top", border::TOP), ("fo:border-bottom", border::BOTTOM), ("fo:border-left", border::LEFT), ("fo:border-right", border::RIGHT)] {
                                if let Some(v) = t.attr(k) {
                                    c.borders.set(f, parse_border(&v));
                                    s.set_borders = true;
                                }
                            }
                            if let Some(a) = t.attr("style:rotation-angle").and_then(|v| v.trim_end_matches("deg").parse::<f32>().ok()) {
                                let a = a.rem_euclid(360.0).round() as i16;
                                c.rotation = if a <= 90 { a } else if a >= 270 { a - 360 } else { 0 };
                                s.set_rotation = true;
                            }
                            if t.attr("style:direction").as_deref() == Some("ttb") {
                                c.rotation = ROTATION_STACKED;
                                s.set_rotation = true;
                            }
                            if let Some(v) = t.attr("style:vertical-align") {
                                c.valign = match v.as_ref() {
                                    "top" => VAlign::Top,
                                    "middle" => VAlign::Center,
                                    _ => VAlign::Bottom,
                                };
                            }
                            if t.attr("fo:wrap-option").as_deref() == Some("wrap") {
                                c.wrap = true;
                            }
                            if s.set_borders {
                                s.parts |= part::BORDER;
                            }
                            if ["style:rotation-angle", "style:direction", "style:vertical-align", "fo:wrap-option"].iter().any(|k| t.attr(k).is_some()) {
                                s.parts |= part::ALIGN;
                            }
                            if let Some(v) = t.attr("style:cell-protect") {
                                s.parts |= part::PROTECT;
                                c.unlocked = !v.split_whitespace().any(|w| w == "protected" || w == "hidden-and-protected");
                                c.formula_hidden = v.split_whitespace().any(|w| w == "formula-hidden" || w == "hidden-and-protected");
                                s.set_protect = true;
                            }
                            if t.attr("apex:checkbox").is_some_and(|v| v == "true") {
                                c.checkbox = true;
                            }
                        }
                    }
                    "style:paragraph-properties" => {
                        if let Some((_, s)) = &mut cur {
                            if t.attr("fo:text-align").is_some() || t.attr("fo:margin-left").is_some() {
                                s.parts |= apex_sheet::cellstyles::part::ALIGN;
                            }
                            if let Some(v) = t.attr("fo:text-align") {
                                s.cell.halign = match v.as_ref() {
                                    "center" => HAlign::Center,
                                    "end" | "right" => HAlign::Right,
                                    "start" | "left" => HAlign::Left,
                                    "justify" => HAlign::Justify,
                                    _ => HAlign::General,
                                };
                            }
                            if let Some(m) = t.attr("fo:margin-left").and_then(|v| parse_length_pt(&v)) {
                                s.cell.indent = (m / INDENT_PT).round().clamp(0.0, 15.0) as u8;
                                s.set_indent = true;
                            }
                        }
                    }
                    "style:table-column-properties" => {
                        if let Some((_, s)) = &mut cur {
                            s.col_width = t.attr("style:column-width").and_then(|v| parse_length_pt(&v));
                            s.page_break = t.attr("fo:break-before").is_some_and(|v| v == "page");
                        }
                    }
                    "style:table-row-properties" => {
                        if let Some((_, s)) = &mut cur {
                            s.row_height = t.attr("style:row-height").and_then(|v| parse_length_pt(&v));
                            s.page_break = t.attr("fo:break-before").is_some_and(|v| v == "page");
                        }
                    }
                    n if n.starts_with("number:") && n.ends_with("-style") && is_start => {
                        let name = t.attr("style:name").unwrap_or_default().into_owned();
                        let code = read_number_style(&mut r, local(n), &t);
                        // A format code's worth (each style copies it).
                        let code = if code.len() <= 1024 { code } else { code.chars().take(1024).collect() };
                        st.numbers.insert(name, code);
                    }
                    _ => {}
                }
            }
            Ev::End(n) if n == "style:style" || n == "style:default-style" => {
                if let Some((name, s)) = cur.take() {
                    if name == "__default" && s.table_cell && s.cell.font_size > 0 {
                        st.base_size = Some(s.cell.font_size);
                    }
                    st.styles.insert(name, s);
                }
            }
            _ => {}
        }
    }
}

impl Styles {
    /// The user-visible name of a common style.
    fn display_name(&self, key: &str) -> String {
        self.styles.get(key).and_then(|s| s.display.clone()).unwrap_or_else(|| named::decode_name(key))
    }

    /// Whether a style is a named cell style of the workbook.
    fn is_named(&self, key: &str) -> bool {
        self.styles.get(key).is_some_and(|s| s.common && s.table_cell) && key != "Default" && !key.starts_with("ApexCf")
    }

    /// The named style a cell style descends from.
    /// Found once per named style (`found`): a file may have many.
    fn named_of(&self, key: &str, styles: &apex_sheet::CellStyles, found: &mut HashMap<String, Option<Arc<str>>>) -> Option<Arc<str>> {
        let mut cur = Some(key);
        for _ in 0..16 {
            let k = cur?;
            if self.is_named(k) {
                return found
                    .entry(k.to_string())
                    .or_insert_with(|| {
                        let name = apex_sheet::cellstyles::file_style_name(&self.display_name(k), None).0;
                        styles.get(&name).filter(|n| !n.is_normal()).map(|n| n.name.clone())
                    })
                    .clone();
            }
            cur = self.styles.get(k)?.parent.as_deref();
        }
        None
    }

    /// The workbook's named styles: ours with the file's definitions
    /// (LibreOffice's own styles only when some cell uses them).
    fn named_styles(&self, used: &std::collections::HashSet<String>, wb: &Workbook) -> apex_sheet::CellStyles {
        let mut out = apex_sheet::CellStyles::new(apex_sheet::Theme::default(), &wb.locale);
        let mut keys: Vec<&String> = self.styles.keys().filter(|k| self.is_named(k) || k.as_str() == "Default").collect();
        keys.sort();
        let mut at: HashMap<String, usize> = out.list.iter().enumerate().map(|(i, s)| (s.name.to_ascii_lowercase(), i)).collect();
        for k in keys {
            let name = if k == "Default" { "Normal".to_string() } else { self.display_name(k) };
            if k != "Default" && named::is_lo_builtin(&name) && !used.contains(k.as_str()) {
                continue;
            }
            let Some(s) = self.styles.get(k.as_str()) else { continue };
            let (name, builtin) = apex_sheet::cellstyles::file_style_name(&name, None);
            let mut style = self.resolve(k);
            style.named = None;
            let parts = if builtin == Some(0) { apex_sheet::cellstyles::part::ALL } else { s.parts };
            let def = apex_sheet::NamedStyle { name: Arc::from(name), builtin, parts, style };
            let low = def.name.to_ascii_lowercase();
            match at.get(&low) {
                Some(&i) => out.list[i] = def,
                None if out.list.len() < MAX_NAMED_STYLES => {
                    at.insert(low, out.list.len());
                    out.list.push(def);
                }
                None => {}
            }
        }
        out
    }

    /// The named styles automatic cell styles descend from (keys).
    fn used_named(&self) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        for k in self.styles.iter().filter(|(_, s)| !s.common && s.table_cell).map(|(k, _)| k) {
            let mut cur = Some(k.as_str());
            for _ in 0..16 {
                let Some(c) = cur else { break };
                if self.is_named(c) {
                    out.insert(c.to_string());
                    break;
                }
                cur = self.styles.get(c).and_then(|s| s.parent.as_deref());
            }
        }
        out
    }

    /// Resolves a style with its parents into a CellStyle.
    fn resolve(&self, name: &str) -> CellStyle {
        let mut chain = Vec::new();
        let mut cur = Some(name);
        while let Some(n) = cur {
            if chain.len() > 16 {
                break;
            }
            match self.styles.get(n) {
                Some(s) => {
                    chain.push(s);
                    cur = s.parent.as_deref();
                }
                None => break,
            }
        }
        let default = CellStyle::default();
        let mut out = CellStyle::default();
        // Apply from the root ancestor down; later (child) fields override.
        for s in chain.iter().rev() {
            let c = &s.cell;
            macro_rules! take {
                ($f:ident) => {
                    if c.$f != default.$f {
                        out.$f = c.$f.clone();
                    }
                };
            }
            take!(bold);
            take!(italic);
            take!(underline);
            take!(strike);
            take!(font_size);
            take!(color);
            take!(fill);
            take!(halign);
            take!(valign);
            take!(wrap);
            take!(checkbox);
            if s.set_borders {
                out.borders = c.borders;
            }
            if s.set_font {
                out.font = c.font.as_deref().map(|f| Arc::from(self.fonts.get(f).map_or(f, String::as_str)));
            }
            if s.set_indent {
                out.indent = c.indent;
            }
            if s.set_rotation {
                out.rotation = c.rotation;
            }
            if s.set_protect {
                out.unlocked = c.unlocked;
                out.formula_hidden = c.formula_hidden;
            }
            if let Some(code) = s.data_style.as_ref().and_then(|d| self.numbers.get(d)) {
                out.num_fmt = Arc::from(code.as_str());
            }
        }
        // Base font size of the document is treated as default.
        if out.font_size == self.base_size.unwrap_or(100) {
            out.font_size = 0;
        }
        out
    }
}

struct SheetReader<'a> {
    st: &'a Styles,
    ids: HashMap<String, u32>,
    /// Cells carrying a content validation: (name, sheet, area).
    val_cells: Vec<(String, usize, Range)>,
    /// Array formula areas: (sheet, area).
    matrices: Vec<(usize, Range)>,
    /// Notes and links, set once cells are loaded.
    extras: Vec<(usize, CellRef, Option<Note>, Option<Link>)>,
    /// Mixed formatting of text cells.
    rich: Vec<(usize, CellRef, Arc<apex_sheet::rich::RichText>)>,
    /// Picture frames, placed once the sheets are read.
    frames: Vec<images::RawFrame>,
    /// Shapes, placed likewise.
    shapes: Vec<shapes::RawShape>,
    /// Form controls by sheet and id, for the `draw:control` shapes.
    forms: HashMap<(usize, String), apex_sheet::FormControl>,
    /// Solver names of the sheets (they become Solver models).
    solver: Vec<crate::solver_names::RawName>,
    /// The named style each named style key stands for.
    named: HashMap<String, Option<Arc<str>>>,
    /// Cells the book's sheets may still copy (see `BOOK_COPY_BUDGET`).
    copies: u64,
}

impl SheetReader<'_> {
    fn style_id(&mut self, wb: &mut Workbook, name: &str) -> u32 {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let mut style = self.st.resolve(name);
        style.named = self.st.named_of(name, &wb.cell_styles, &mut self.named);
        let id = wb.styles.intern(style);
        self.ids.insert(name.to_string(), id);
        id
    }
}

/// Named cell styles kept from a file (the list is searched by name).
const MAX_NAMED_STYLES: usize = 4096;

std::thread_local! {
    static SPACES_LEFT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The spaces a `<text:s text:c="n"/>` adds: at most 1000, and 64 MB for
/// the whole file (20 bytes make 1000).
pub(crate) fn spaces(n: usize) -> usize {
    SPACES_LEFT.with(|left| {
        let k = n.min(1000).min(left.get());
        left.set(left.get() - k);
        k
    })
}

/// A new file (or part) read: its own budget of spaces.
pub(crate) fn reset_spaces() {
    SPACES_LEFT.with(|left| left.set(64 << 20));
}

pub fn read(data: &[u8]) -> Result<Workbook, Error> {
    reset_spaces();
    let z = ZipReader::new(data)?;
    let mut st = Styles::default();
    let mut pages = page::Pages::default();
    if let Ok(s) = z.read_string("styles.xml") {
        read_styles(&s, &mut st);
        pages = page::read_pages(&s, &|_| None, 1);
    }
    let content = z.read_string("content.xml")?;
    read_styles(&content, &mut st);
    let mut wb = Workbook::empty();
    if let Ok(x) = z.read_string(crate::queries_xml::ODS_PATH) {
        wb.queries = crate::queries_xml::read(&x);
    }
    if let Ok(x) = z.read_string(crate::checks_xml::ODS_PATH) {
        wb.checks = crate::checks_xml::read(&x);
    }
    wb.cell_styles = st.named_styles(&st.used_named(), &wb);
    wb.refresh_cell_styles();
    let new_reader = |st| SheetReader { st, ids: HashMap::new(), named: HashMap::new(), val_cells: Vec::new(), matrices: Vec::new(), extras: Vec::new(), rich: Vec::new(), frames: Vec::new(), shapes: Vec::new(), forms: HashMap::new(), solver: Vec::new(), copies: BOOK_COPY_BUDGET };
    let mut sr = new_reader(&st);
    let mut tables_read = 0usize;
    // Scenarios: (sheet, scenario, active).
    let mut scens = Vec::new();
    let mut validations: HashMap<String, apex_sheet::data::Validation> = HashMap::new();
    // Data pilot tables, read once the sheets are.
    let mut pilots: Vec<String> = Vec::new();
    let mut r = Reader::new(&content);
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:calculation-settings" => {
                wb.calc.as_shown = t.attr("table:precision-as-shown").is_some_and(|v| v == "true");
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:null-date" => {
                wb.calc.date1904 = t.attr("table:date-value").is_some_and(|v| v.starts_with("1904-01-01"));
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:iteration" => {
                wb.calc.iterate = t.attr("table:status").is_some_and(|v| v == "enable");
                if let Some(n) = t.attr("table:steps").and_then(|v| v.parse().ok()) {
                    wb.calc.max_iter = n;
                }
                if let Some(d) = t.attr("table:maximum-difference").or_else(|| t.attr("table:minimum-difference")).and_then(|v| v.parse().ok()) {
                    wb.calc.max_change = d;
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:content-validation" => {
                let (name, v) = data::read_validation(&mut r, &t, empty);
                validations.insert(name, v);
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:database-range" => data::read_database_range(&mut r, &t, empty, &mut wb),
            // A book's worth of pivot tables.
            Ev::Start(t) if t.name == "table:data-pilot-table" && pilots.len() >= 256 => r.skip(),
            Ev::Start(t) if t.name == "table:data-pilot-table" => pilots.push(format!("<{}{}>{}", t.name, t.raw_attrs(), r.raw_element())),
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:named-range" || t.name == "table:named-expression" => {
                wb.names.extend(data::read_named(&t, None));
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "office:spreadsheet" => {
                if t.attr("table:structure-protected").is_some_and(|v| v == "true") {
                    wb.protection = Some(Protection { passwords: odf_passwords(&t), allow: 0 });
                }
            }
            // A book's worth of sheets (scenario tables included).
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:table" && tables_read >= MAX_SHEETS => {
                if !empty {
                    r.skip();
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:table" => {
                tables_read += 1;
                let name = t.attr("table:name").map(|v| v.chars().take(255).collect()).unwrap_or_else(|| wb.unique_sheet_name("Sheet"));
                // A scenario's table: its values are read apart and the
                // table left out.
                if let Some(attrs) = scenarios::peek(&r).filter(|_| !wb.sheets.is_empty() && !empty) {
                    let mut tmp = Workbook::empty();
                    tmp.sheets.push(Sheet::new(name.clone()));
                    let mut tr = new_reader(&st);
                    tr.copies = sr.copies;
                    read_table(&mut r, &mut tmp, 0, &mut tr);
                    sr.copies = tr.copies;
                    scens.extend(scenarios::read_table(&tmp, &name, &attrs).map(|(sc, active)| (wb.sheets.len() - 1, sc, active)));
                    continue;
                }
                let mut sheet = Sheet::new(name);
                if t.attr("table:protected").is_some_and(|v| v == "true") {
                    sheet.protection = Some(Protection { passwords: odf_passwords(&t), ..Default::default() });
                }
                let ts = t.attr("table:style-name").and_then(|n| st.styles.get(n.as_ref()));
                if let Some(ts) = ts {
                    sheet.hidden = ts.hidden;
                    sheet.tab_color = ts.tab_color;
                }
                // Tables without a page style print with the default one.
                let master = ts.and_then(|ts| ts.master_page.as_deref()).unwrap_or("Default");
                page::apply_to_sheet(&pages, master, &mut sheet.page);
                sheet.page.background = page::background_of(&pages, master).and_then(|href| {
                    let data = z.read(href.trim_start_matches("./")).ok()?;
                    let format = apex_sheet::ImageFormat::detect(&data, href.rsplit_once('.').map_or("", |x| x.1))?;
                    Some(apex_sheet::SheetBackground { data: data.into(), format })
                });
                if let Some(pr) = t.attr("table:print-ranges") {
                    sheet.page.print_areas = page::parse_print_ranges(&pr);
                }
                wb.sheets.push(sheet);
                let si = wb.sheets.len() - 1;
                if !empty {
                    read_table(&mut r, &mut wb, si, &mut sr);
                }
            }
            _ => {}
        }
    }
    // A spreadsheet without tables opens with an empty sheet, as in
    // LibreOffice.
    if wb.sheets.is_empty() {
        wb.sheets.push(Sheet::new("Sheet1"));
    }
    scenarios::store(&mut wb, scens);
    let names = crate::solver_names::read(&mut wb, std::mem::take(&mut sr.solver));
    crate::xlsx::read_user_names(&mut wb, names);
    // Validations apply to the cells that name them.
    let mut areas: HashMap<(String, usize), Vec<Range>> = HashMap::new();
    for (name, si, r) in std::mem::take(&mut sr.val_cells) {
        areas.entry((name, si)).or_default().push(r);
    }
    let mut areas: Vec<((String, usize), Vec<Range>)> = areas.into_iter().collect();
    areas.sort_by(|a, b| (a.0.1, &a.0.0).cmp(&(b.0.1, &b.0.0)));
    for ((name, si), rects) in areas {
        if let Some(v) = validations.get(&name) {
            let mut v = v.clone();
            v.ranges = data::merge_rects(rects);
            wb.sheets[si].validations.push(v);
        }
    }
    if wb.sheets.iter().all(|s| s.hidden) {
        wb.sheets[0].hidden = false;
    }
    if let Ok(s) = z.read_string("settings.xml") {
        read_settings(&s, &mut wb);
    }
    let mut work = 0;
    for (si, area) in &sr.matrices {
        if work > crate::xlsx::MAX_ARRAY_WORK {
            break;
        }
        work += wb.sheets[*si].mark_fixed_array(area);
    }
    if !sr.frames.is_empty() {
        let mut clips = HashMap::new();
        if let Ok(s) = z.read_string("styles.xml") {
            images::graphic_clips(&s, &mut clips);
        }
        images::graphic_clips(&content, &mut clips);
        images::resolve(&z, std::mem::take(&mut sr.frames), &clips, &mut wb.sheets);
    }
    if !sr.shapes.is_empty() {
        let mut st = crate::odf_shape::Styles::default();
        if let Ok(s) = z.read_string("styles.xml") {
            st.read(&s);
        }
        st.read(&content);
        shapes::resolve(std::mem::take(&mut sr.shapes), &st, &mut wb.sheets, &mut sr.forms);
    }
    for (si, c, runs) in std::mem::take(&mut sr.rich) {
        wb.sheets[si].edit_extra(c, |x| x.rich = Some(runs));
    }
    for (si, c, note, link) in std::mem::take(&mut sr.extras) {
        wb.sheets[si].edit_extra(c, |x| {
            if note.is_some() {
                x.note = note;
            }
            if link.is_some() {
                x.link = link;
            }
        });
    }
    if let Ok(s) = z.read_string("meta.xml") {
        wb.meta = crate::props::read_odf(&s);
    }
    wb.macros.odf = crate::macro_parts::read_odf(&z);
    let part = z.read_string(MODEL_PART).ok();
    if let Some(m) = part.as_deref().and_then(crate::xlsx_model::read_model_xml) {
        wb.analysis.model = m;
    }
    if let Some(x) = &part {
        crate::plan_xml::read_plans(x, &mut wb);
    }
    let ours = part.is_some_and(|x| crate::pivot_xml::read_pivots_xml(&x, &mut wb));
    pivot::read_data_pilots(pilots, ours, &mut wb);
    wb.normalize_externals();
    wb.finish_load();
    Ok(wb)
}

fn read_table(r: &mut Reader, wb: &mut Workbook, si: usize, sr: &mut SheetReader) {
    let mut row: u32 = 0;
    let mut col: u32 = 0;
    let mut row_repeat: u32 = 1;
    let mut next_col_idx: u32 = 0;
    let mut col_default_styles: Vec<(u32, u32, String)> = Vec::new();
    let mut depth = 1;
    // Nesting of row and column groups (outline levels).
    let (mut row_group, mut col_group) = (0u8, 0u8);
    // Where the print title rows and columns start.
    let (mut title_row, mut title_col) = (None, None);
    // Row buffer: cells of the current row, replicated on row repeats.
    let mut row_cells: Vec<(u32, Content, u32)> = Vec::new();
    // Blank formatted cells kept for the sheet: past this, runs whose
    // format shows nothing (the font of the rest of a row, in old files
    // with 1024 columns in every row) are left out, and blank formatted
    // rows are repeated no more than a thousand times.
    let mut blank_left: u64 = BLANK_BUDGET;
    // Copies of cells with values, made by repeating them: a small file
    // could otherwise ask for billions.
    let mut copies_left: u64 = COPY_BUDGET.min(sr.copies);
    let mut cf_left = crate::MAX_CF_RANGES;
    let start = copies_left;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => break,
            Ev::End(n) => {
                depth -= 1;
                match n {
                    "table:table-row-group" => row_group = row_group.saturating_sub(1),
                    "table:table-column-group" => col_group = col_group.saturating_sub(1),
                    "table:table-header-rows" => {
                        if let Some(a) = title_row.take().filter(|&a| row > a && a < MAX_ROWS) {
                            wb.sheets[si].page.title_rows = Some((a, (row - 1).min(MAX_ROWS - 1)));
                        }
                    }
                    "table:table-header-columns" => {
                        if let Some(a) = title_col.take().filter(|&a| next_col_idx > a && a < MAX_COLS) {
                            wb.sheets[si].page.title_cols = Some((a, (next_col_idx - 1).min(MAX_COLS - 1)));
                        }
                    }
                    _ => {}
                }
                if n == "table:table-row" {
                    // Repeated rows with values (LibreOffice writes equal rows
                    // so) are all kept up to a few million cells; blank
                    // formatted ones to a thousand rows.
                    let values = row_cells.iter().any(|x| !matches!(x.1, Content::Empty));
                    let cells = row_cells.len().max(1) as u64;
                    let cap = row_repeat_cap(values, row_cells.len());
                    if !values {
                        blank_left = blank_left.saturating_sub(cells * u64::from(row_repeat.min(cap).saturating_sub(1)));
                    }
                    let mut reps = if row_cells.is_empty() { row_repeat } else { row_repeat.min(cap) };
                    // Formatted blank rows count as copies too.
                    if !row_cells.is_empty() && reps > 1 {
                        let extra = u64::from(reps - 1).min(copies_left / cells);
                        copies_left -= extra * cells;
                        reps = extra as u32 + 1;
                    }
                    for k in 0..reps {
                        let rr = row.saturating_add(k);
                        if rr >= MAX_ROWS {
                            break;
                        }
                        for (c, content, style) in &row_cells {
                            wb.load_cell(si, CellRef::new(rr, *c), content.clone(), *style);
                        }
                    }
                    row = row.saturating_add(row_repeat);
                    row_cells.clear();
                }
                if depth == 0 {
                    break;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let is_start = matches!(ev, Ev::Start(_));
                if is_start {
                    depth += 1;
                }
                match t.name {
                    "table:named-range" | "table:named-expression" if t.attr("table:name").is_some_and(|n| crate::solver_names::is_solver_name(&n)) => {
                        sr.solver.extend(scenarios::solver_name(&t, si));
                    }
                    "table:named-range" | "table:named-expression" => {
                        let scope = Some(wb.sheets[si].name.clone());
                        wb.names.extend(data::read_named(&t, scope));
                    }
                    "table:table-row-group" if is_start => row_group += 1,
                    "table:table-column-group" if is_start => col_group += 1,
                    "table:table-header-rows" if is_start => title_row = Some(row),
                    "table:table-header-columns" if is_start => title_col = Some(next_col_idx),
                    "table:table-column" => {
                        let n: u32 = t.attr("table:number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1);
                        if col_group > 0 && n <= MAX_STYLED_REPEAT {
                            for c in next_col_idx..(next_col_idx.saturating_add(n)).min(MAX_COLS) {
                                wb.sheets[si].col_levels.insert(c, col_group.min(apex_sheet::MAX_LEVEL));
                            }
                        }
                        if let Some(sname) = t.attr("table:style-name") {
                            if sr.st.styles.get(sname.as_ref()).is_some_and(|s| s.page_break) && n <= MAX_STYLED_REPEAT {
                                wb.sheets[si].page.col_breaks.extend((next_col_idx..(next_col_idx.saturating_add(n)).min(MAX_COLS)).filter(|&c| c > 0));
                            }
                            if let Some(w) = sr.st.styles.get(sname.as_ref()).and_then(|s| s.col_width) {
                                let px = (w * COL_PX_PER_PT).round();
                                if n <= MAX_STYLED_REPEAT && (px - wb.sheets[si].default_col_width).abs() > 1.0 {
                                    for c in next_col_idx..(next_col_idx.saturating_add(n)).min(MAX_COLS) {
                                        wb.sheets[si].col_widths.insert(c, px);
                                    }
                                }
                            }
                        }
                        if t.attr("table:visibility").is_some_and(|v| v == "collapse" || v == "hidden") && n <= MAX_STYLED_REPEAT {
                            for c in next_col_idx..(next_col_idx.saturating_add(n)).min(MAX_COLS) {
                                wb.sheets[si].col_widths.insert(c, 0.0);
                            }
                        }
                        if let Some(ds) = t.attr("table:default-cell-style-name") {
                            if ds != "Default" && n > 0 {
                                col_default_styles.push((next_col_idx, next_col_idx.saturating_add(n), ds.into_owned()));
                            }
                        }
                        next_col_idx = next_col_idx.saturating_add(n);
                        if is_start {
                            depth -= 1;
                            r.skip();
                        }
                    }
                    "loext:table-protection" | "table:table-protection" => {
                        if let Some(p) = &mut wb.sheets[si].protection {
                            let set = |p: &mut Protection, k: &str, flag: u32| {
                                if let Some(v) = t.attr(k) {
                                    if v == "true" {
                                        p.allow |= flag;
                                    } else {
                                        p.allow &= !flag;
                                    }
                                }
                            };
                            set(p, "select-protected-cells", allow::SELECT_LOCKED);
                            set(p, "select-unprotected-cells", allow::SELECT_UNLOCKED);
                            set(p, "insert-columns", allow::INSERT_COLUMNS);
                            set(p, "insert-rows", allow::INSERT_ROWS);
                            set(p, "delete-columns", allow::DELETE_COLUMNS);
                            set(p, "delete-rows", allow::DELETE_ROWS);
                        }
                        if is_start {
                            depth -= 1;
                            r.skip();
                        }
                    }
                    "office:forms" if is_start => {
                        depth -= 1;
                        controls::read_forms(r, si, &mut sr.forms);
                    }
                    "table:shapes" if is_start => {
                        depth -= 1;
                        let mut d = 1;
                        loop {
                            match r.next() {
                                Ev::Start(f) if shapes::is_object(f.name) => shapes::read(r, &f, false, si, None, &mut sr.frames, &mut sr.shapes),
                                Ev::Empty(f) if shapes::is_object(f.name) => shapes::read(r, &f, true, si, None, &mut sr.frames, &mut sr.shapes),
                                Ev::Start(_) => d += 1,
                                Ev::End(_) => {
                                    d -= 1;
                                    if d == 0 {
                                        break;
                                    }
                                }
                                Ev::Eof => break,
                                _ => {}
                            }
                        }
                    }
                    "calcext:sparkline-groups" if is_start => {
                        depth -= 1;
                        chart::read_sparklines(r, &mut wb.sheets[si]);
                    }
                    "calcext:conditional-formats" if is_start => {
                        depth -= 1;
                        data::read_cond_formats(r, sr.st, &mut wb.sheets[si], &mut cf_left);
                    }
                    "table:table-row" => {
                        row_repeat = t.attr("table:number-rows-repeated").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
                        col = 0;
                        if row_group > 0 && row_repeat <= MAX_STYLED_REPEAT {
                            for rr in row..(row.saturating_add(row_repeat)).min(MAX_ROWS) {
                                wb.sheets[si].row_levels.insert(rr, row_group.min(apex_sheet::MAX_LEVEL));
                            }
                        }
                        if t.attr("table:style-name").and_then(|s| sr.st.styles.get(s.as_ref())).is_some_and(|s| s.page_break) && row_repeat <= MAX_STYLED_REPEAT {
                            wb.sheets[si].page.row_breaks.extend((row..(row.saturating_add(row_repeat)).min(MAX_ROWS)).filter(|&r| r > 0));
                        }
                        if let Some(h) = t.attr("table:style-name").and_then(|s| sr.st.styles.get(s.as_ref())).and_then(|s| s.row_height) {
                            let px = (h * ROW_PX_PER_PT).round();
                            if row_repeat <= MAX_STYLED_REPEAT && (px - wb.sheets[si].default_row_height).abs() > 1.5 {
                                for rr in row..(row.saturating_add(row_repeat)).min(MAX_ROWS) {
                                    wb.sheets[si].row_heights.insert(rr, px);
                                }
                            }
                        }
                        if t.attr("table:visibility").is_some_and(|v| v == "collapse" || v == "hidden" || v == "filter") && row_repeat <= MAX_STYLED_REPEAT {
                            for rr in row..(row.saturating_add(row_repeat)).min(MAX_ROWS) {
                                wb.sheets[si].row_heights.insert(rr, 0.0);
                            }
                        }
                        if !is_start {
                            row = row.saturating_add(row_repeat);
                        }
                    }
                    "table:table-cell" | "table:covered-table-cell" => {
                        let reps: u32 = t.attr("table:number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
                        let cs: u32 = t.attr("table:number-columns-spanned").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
                        let rs: u32 = t.attr("table:number-rows-spanned").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
                        // (Past the most a sheet keeps, merges aren't gathered.)
                        if (cs > 1 || rs > 1) && t.name == "table:table-cell" && row < MAX_ROWS && col < MAX_COLS && wb.sheets[si].merges.len() < apex_sheet::workbook::MAX_MERGES {
                            let end = CellRef::new(row.saturating_add(rs - 1).min(MAX_ROWS - 1), col.saturating_add(cs - 1).min(MAX_COLS - 1));
                            wb.sheets[si].merges.push(Range::new(CellRef::new(row, col), end));
                        }
                        let span = |k: &str| t.attr(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).max(1);
                        let (mc, mr) = (span("table:number-matrix-columns-spanned"), span("table:number-matrix-rows-spanned"));
                        if (mc > 1 || mr > 1) && row.saturating_add(mr) <= MAX_ROWS && col.saturating_add(mc) <= MAX_COLS {
                            sr.matrices.push((si, Range::new(CellRef::new(row, col), CellRef::new(row + mr - 1, col + mc - 1))));
                        }
                        if let Some(vn) = t.attr("table:content-validation-name") {
                            if row < MAX_ROWS && col < MAX_COLS {
                                let area = Range::new(CellRef::new(row, col), CellRef::new(row.saturating_add(row_repeat - 1).min(MAX_ROWS - 1), col.saturating_add(reps - 1).min(MAX_COLS - 1)));
                                sr.val_cells.push((vn.into_owned(), si, area));
                            }
                        }
                        let style_name = t.attr("table:style-name").map(|v| v.into_owned()).or_else(|| {
                            // Sorted, disjoint ranges: the columns only move right.
                            let i = col_default_styles.partition_point(|(a, _, _)| *a <= col);
                            i.checked_sub(1).map(|i| &col_default_styles[i]).filter(|(_, b, _)| col < *b).map(|x| x.2.clone())
                        });
                        let style = style_name.as_deref().map_or(0, |n| sr.style_id(wb, n));
                        let (text, note, link, runs) = if is_start {
                            depth -= 1;
                            let at = CellRef::new(row.min(MAX_ROWS - 1), col.min(MAX_COLS - 1));
                            read_cell_text(r, &mut |r, t, empty| shapes::read(r, t, empty, si, Some(at), &mut sr.frames, &mut sr.shapes))
                        } else {
                            (None, None, None, Vec::new())
                        };
                        if runs.iter().any(|x| x.1.is_some()) && row < MAX_ROWS {
                            let rich: apex_sheet::rich::RichText =
                                runs.iter().map(|(len, st)| apex_sheet::rich::RichRun { len: *len, font: st.as_deref().and_then(|n| sr.st.styles.get(n)).map(|s| s.run.clone()).unwrap_or_default() }).collect();
                            if let Some(rich) = apex_sheet::rich::normalize(rich) {
                                let rich = Arc::new(rich);
                                for k in 0..reps.min(MAX_STYLED_REPEAT) {
                                    // Each becomes a cell of its own: as
                                    // many as styled cells may be.
                                    if col + k < MAX_COLS && sr.rich.len() < 1 << 20 {
                                        sr.rich.push((si, CellRef::new(row, col + k), rich.clone()));
                                    }
                                }
                            }
                        }
                        if (note.is_some() || link.is_some()) && row < MAX_ROWS && col < MAX_COLS {
                            sr.extras.push((si, CellRef::new(row, col), note, link));
                        }
                        let content = cell_content(&t, text, wb, style);
                        let has = !matches!(content, Content::Empty);
                        // Blank cells whose format shows nothing are left out
                        // when they fill the rest of a row of an old file (to
                        // column 1024) or once the budget is spent: they
                        // would be millions of cells for nothing.
                        let invisible_ok = reps < 1000 && blank_left >= u64::from(reps);
                        let styled = style != 0 && reps <= MAX_STYLED_REPEAT && (invisible_ok || shows_when_blank(wb.styles.get(style)));
                        if styled && !has {
                            blank_left = blank_left.saturating_sub(u64::from(reps));
                        }
                        let mut n = reps.min(MAX_COLS.saturating_sub(col));
                        if has && n > 1 {
                            let extra = u64::from(n - 1).min(copies_left);
                            copies_left -= extra;
                            n = extra as u32 + 1;
                        } else if styled && !has {
                            // Formatted blank cells are copies too.
                            n = u64::from(n).min(copies_left) as u32;
                            copies_left -= u64::from(n);
                        }
                        if has || styled {
                            for k in 0..n {
                                row_cells.push((col + k, content.clone(), style));
                            }
                        }
                        col = col.saturating_add(reps);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    sr.copies -= start - copies_left;
}

/// How many times the reader repeats a row: rows with values up to a few
/// million cells, blank formatted ones a thousand times.
fn row_repeat_cap(values: bool, cells: usize) -> u32 {
    if values { MAX_STYLED_REPEAT.max(2_000_000 / cells.max(1) as u32) } else { MAX_STYLED_REPEAT }
}

/// The repeat cap of a written row (longer runs are split so that the
/// reader keeps them whole).
fn repeat_cap(cells: &[(u32, &apex_sheet::Cell)]) -> u32 {
    row_repeat_cap(cells.iter().any(|c| !matches!(c.1.content, Content::Empty)), cells.len())
}

/// Whether a format changes how an empty cell looks or behaves.
fn shows_when_blank(s: &apex_sheet::CellStyle) -> bool {
    s.fill.is_some() || s.borders != apex_sheet::Borders::NONE || s.unlocked
}

/// Reads `text:p` children of a cell, returning its text (None if no
/// text), its note and its first hyperlink. Drawing objects go to `frame`.
fn read_cell_text(r: &mut Reader, frame: &mut dyn FnMut(&mut Reader, &Tag, bool)) -> (Option<String>, Option<Note>, Option<Link>, Vec<(usize, Option<std::rc::Rc<str>>)>) {
    let mut out: Option<String> = None;
    let mut note = None;
    let mut link = None;
    let mut depth = 1;
    let mut in_p = false;
    // Text spans: the style in effect in each open element (its own or its
    // parent's), and the text written under each style (bytes); shared,
    // as many elements may have one long name.
    let mut spans: Vec<Option<std::rc::Rc<str>>> = Vec::new();
    let mut runs: Vec<(usize, Option<std::rc::Rc<str>>)> = Vec::new();
    let mut written = 0;
    loop {
        let now = out.as_ref().map_or(0, String::len);
        if now > written {
            let style = spans.last().cloned().flatten();
            match runs.last_mut() {
                Some((n, s)) if *s == style => *n += now - written,
                _ => runs.push((now - written, style)),
            }
            written = now;
        }
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => {
                depth += 1;
                let own = if t.name == "text:span" { t.attr("text:style-name").map(|v| std::rc::Rc::from(v.as_ref())) } else { None };
                spans.push(own.or_else(|| spans.last().cloned().flatten()));
                match t.name {
                    "office:annotation" => {
                        depth -= 1;
                        spans.pop();
                        note = Some(read_annotation(r, &t));
                    }
                    name if shapes::is_object(name) => {
                        depth -= 1;
                        spans.pop();
                        frame(r, &t, false);
                    }
                    "text:a" => {
                        if link.is_none() {
                            link = t.attr("xlink:href").map(|h| Link { target: odf_href_to_target(&h), tip: t.attr("office:title").unwrap_or_default().into_owned() });
                        }
                    }
                    "text:p" | "text:h" => {
                        let s = out.get_or_insert_with(String::new);
                        if in_p || !s.is_empty() {
                            s.push('\n');
                        }
                        in_p = true;
                    }
                    _ => {}
                }
            }
            Ev::Empty(t) if shapes::is_object(t.name) => frame(r, &t, true),
            Ev::Empty(t) => {
                let s = out.get_or_insert_with(String::new);
                match t.name {
                    "text:s" => {
                        let n: usize = t.attr("text:c").and_then(|v| v.parse().ok()).unwrap_or(1);
                        // Not past what a cell holds: 20 bytes make 1000.
                        if s.len() < 1 << 17 {
                            s.extend(std::iter::repeat_n(' ', spaces(n)));
                        }
                    }
                    "text:tab" => s.push('\t'),
                    "text:line-break" => s.push('\n'),
                    "text:p" => {
                        if !s.is_empty() || in_p {
                            s.push('\n');
                        }
                        in_p = true;
                    }
                    _ => {}
                }
            }
            Ev::Text(x) => {
                if in_p {
                    out.get_or_insert_with(String::new).push_str(&x);
                }
            }
            Ev::End(_) => {
                depth -= 1;
                spans.pop();
                if depth == 0 {
                    break;
                }
            }
        }
    }
    (out, note, link, runs)
}

fn read_annotation(r: &mut Reader, t: &Tag) -> Note {
    let mut note = Note { visible: t.attr("office:display").is_some_and(|v| v == "true"), ..Default::default() };
    let mut depth = 1;
    let mut paras: Vec<String> = Vec::new();
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(x) => match x.name {
                "dc:creator" => note.author = r.text(),
                "dc:date" => note.date = r.text().trim().to_string(),
                "meta:date-string" => r.skip(),
                "text:p" | "text:h" => {
                    // Text with spaces, tabs and line breaks.
                    let mut p = String::new();
                    let mut d = 1;
                    while d > 0 {
                        match r.next() {
                            Ev::Start(_) => d += 1,
                            Ev::End(_) => d -= 1,
                            Ev::Text(tx) => p.push_str(&tx),
                            Ev::Empty(e) => match e.name {
                                "text:s" => p.extend(std::iter::repeat_n(' ', spaces(e.attr("text:c").and_then(|v| v.parse().ok()).unwrap_or(1)))),
                                "text:tab" => p.push('\t'),
                                "text:line-break" => p.push('\n'),
                                _ => {}
                            },
                            Ev::Eof => break,
                        }
                    }
                    paras.push(p);
                }
                _ => depth += 1,
            },
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    note.text = paras.join("\n");
    note
}

/// `#Sheet2.A1` (ODF) to `#Sheet2!A1`; URLs stay as they are.
fn odf_href_to_target(h: &str) -> String {
    let Some(place) = h.strip_prefix('#') else { return h.to_string() };
    match place.rsplit_once('.') {
        Some((sheet, cell)) if CellRef::parse(&cell.replace('$', "")).is_some() => format!("#{}!{}", sheet.trim_start_matches('$'), cell.replace('$', "")),
        _ => h.to_string(),
    }
}

fn target_to_odf_href(t: &str) -> String {
    let Some(place) = t.strip_prefix('#') else { return t.to_string() };
    match place.rsplit_once('!') {
        Some((sheet, cell)) => format!("#{sheet}.{cell}"),
        None => t.to_string(),
    }
}

fn odf_passwords(t: &Tag) -> Vec<Password> {
    let Some(key) = t.attr("table:protection-key").filter(|k| !k.is_empty()) else { return Vec::new() };
    // ODF 1.2 defaults to SHA-1.
    let algorithm = t.attr("table:protection-key-digest-algorithm").map_or_else(|| "http://www.w3.org/2000/09/xmldsig#sha1".to_string(), |a| a.into_owned());
    vec![Password::Digest { algorithm, key: key.into_owned() }]
}

fn odf_key_attrs(out: &mut String, p: &Protection) {
    let digest = p.passwords.iter().find_map(|pw| match pw {
        Password::Digest { algorithm, key } => Some((algorithm, key)),
        _ => None,
    });
    if let Some((algorithm, key)) = digest {
        let _ = write!(out, r#" table:protection-key="{}" table:protection-key-digest-algorithm="{}""#, esc(key), esc(algorithm));
    }
}

fn parse_duration(s: &str) -> Option<f64> {
    // PT12H30M15.5S (optionally with days: P1DT2H)
    let s = s.trim();
    let neg = s.starts_with('-');
    let s = s.trim_start_matches('-').strip_prefix('P')?;
    let (days, time) = s.split_once('T').unwrap_or((s, ""));
    let mut total = 0.0;
    if let Some(d) = days.strip_suffix('D') {
        total += d.parse::<f64>().ok()? * 86400.0;
    }
    let mut num = String::new();
    for c in time.chars() {
        match c {
            'H' => {
                total += num.parse::<f64>().ok()? * 3600.0;
                num.clear();
            }
            'M' => {
                total += num.parse::<f64>().ok()? * 60.0;
                num.clear();
            }
            'S' => {
                total += num.parse::<f64>().ok()?;
                num.clear();
            }
            c => num.push(c),
        }
    }
    let v = total / 86400.0;
    Some(if neg { -v } else { v })
}

fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date, time) = s.split_once('T').unwrap_or((s, ""));
    let mut it = date.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    let mut v = date_to_serial(y, m, d);
    if !time.is_empty() {
        let mut it = time.split(':');
        let h: f64 = it.next()?.parse().ok()?;
        let mi: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let se: f64 = it.next().and_then(|x| x.trim_end_matches('Z').parse().ok()).unwrap_or(0.0);
        v += (h * 3600.0 + mi * 60.0 + se) / 86400.0;
    }
    Some(v)
}

fn cell_content(t: &Tag, text: Option<String>, wb: &mut Workbook, style: u32) -> Content {
    let vt = t.attr("office:value-type").or_else(|| t.attr("calcext:value-type"));
    // An error cached by LibreOffice: value-type "string" for older
    // readers, calcext:value-type "error" and the error as the text.
    let error = t.attr("calcext:value-type").as_deref() == Some("error");
    let value = match vt.as_deref() {
        _ if error => Some(Value::Error(text.as_deref().map(str::trim).and_then(ErrorKind::parse).unwrap_or(ErrorKind::Value))),
        Some("float") | Some("percentage") | Some("currency") => t.attr("office:value").and_then(|v| v.parse::<f64>().ok()).map(Value::Number),
        Some("date") => t.attr("office:date-value").and_then(|v| parse_date(&v)).map(Value::Number),
        Some("time") => t.attr("office:time-value").and_then(|v| parse_duration(&v)).map(Value::Number),
        Some("boolean") => t.attr("office:boolean-value").map(|v| Value::Bool(v == "true" || v == "1")),
        Some("string") => Some(Value::text(t.attr("office:string-value").map(|v| v.into_owned()).or(text.clone()).unwrap_or_default())),
        _ => None,
    };
    let _ = (wb, style);
    if let Some(f) = t.attr("table:formula") {
        if let Ok(expr) = parse_canonical(&odf_to_canonical(&f)) {
            // Only matrix formulas return arrays in OpenDocument.
            let matrix = t.attr("table:number-matrix-columns-spanned").is_some() || t.attr("table:number-matrix-rows-spanned").is_some();
            let expr = if matrix { expr } else { expr.legacy() };
            return Content::Formula { expr: expr.into(), value: value.unwrap_or(Value::Empty) };
        }
    }
    match value {
        // An empty text (`=""` pasted as a value) says so explicitly.
        Some(Value::Text(s)) if s.is_empty() && t.attr("office:string-value").is_none() => Content::Empty,
        Some(v) => Content::Value(v),
        None => match text {
            Some(s) if !s.is_empty() => Content::Value(Value::text(s)),
            _ => Content::Empty,
        },
    }
}

fn read_settings(xml: &str, wb: &mut Workbook) {
    let mut r = Reader::new(xml);
    let mut in_tables = 0;
    let mut cur_sheet: Option<usize> = None;
    let mut vals: HashMap<String, u32> = HashMap::new();
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "config:config-item-map-named" && t.attr("config:name").as_deref() == Some("Tables") => in_tables = 1,
            Ev::Start(t) if in_tables > 0 && t.name == "config:config-item-map-entry" => {
                in_tables += 1;
                if in_tables == 2 {
                    cur_sheet = t.attr("config:name").and_then(|n| wb.sheet_index(&n));
                    vals.clear();
                }
            }
            Ev::Start(t) if in_tables == 0 && t.name == "config:config-item" && t.attr("config:name").as_deref() == Some("AutoCalculate") => {
                wb.calc.manual = r.text().trim() == "false";
            }
            // LibreOffice's page break view is one setting for the whole
            // window.
            Ev::Start(t) if in_tables == 0 && t.name == "config:config-item" && t.attr("config:name").as_deref() == Some("ShowPageBreakPreview") => {
                if r.text().trim() == "true" {
                    for sh in &mut wb.sheets {
                        sh.view = apex_sheet::ViewMode::PageBreakPreview;
                    }
                }
            }
            Ev::Start(t) if in_tables == 2 && t.name == "config:config-item" => {
                let name = t.attr("config:name").unwrap_or_default().into_owned();
                let v = r.text();
                if let Ok(n) = v.trim().parse::<u32>() {
                    vals.insert(name, n);
                }
            }
            Ev::Start(t) if in_tables > 0 => {
                if t.name.starts_with("config:config-item-map") {
                    in_tables += 1;
                }
            }
            Ev::End(n) if in_tables > 0 && n.starts_with("config:config-item-map") => {
                if in_tables == 2 {
                    if let Some(si) = cur_sheet.take() {
                        // The first column and row shown left of and above the split.
                        let sh = &mut wb.sheets[si];
                        if vals.get("HorizontalSplitMode") == Some(&2) {
                            sh.frozen_cols = vals.get("HorizontalSplitPosition").copied().unwrap_or(0);
                            sh.frozen_top.col = vals.get("PositionLeft").copied().unwrap_or(0).min(sh.frozen_cols.saturating_sub(1));
                        }
                        if vals.get("VerticalSplitMode") == Some(&2) {
                            sh.frozen_rows = vals.get("VerticalSplitPosition").copied().unwrap_or(0);
                            sh.frozen_top.row = vals.get("PositionTop").copied().unwrap_or(0).min(sh.frozen_rows.saturating_sub(1));
                        }
                    }
                }
                in_tables -= 1;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Writing

fn fmt_len(pt: f32) -> String {
    format!("{:.4}in", pt / 72.0)
}

fn color(c: u32) -> String {
    format!("#{:06x}", c & 0xFF_FFFF)
}

fn iso_date(serial: f64) -> String {
    // Nanoseconds of the day, so that fractions of a second survive.
    let mut day = serial.floor();
    let mut nanos = ((serial - day) * NANOS_PER_DAY).round() as u64;
    if nanos >= NANOS_PER_DAY as u64 {
        day += 1.0;
        nanos = 0;
    }
    let (y, m, d) = serial_to_date(day);
    if nanos == 0 {
        format!("{:04}-{:02}-{:02}", y, m, d)
    } else {
        format!("{:04}-{:02}-{:02}T{}", y, m, d, clock(nanos, true))
    }
}

const NANOS_PER_DAY: f64 = 86_400e9;

/// `hh:mm:ss[.fff]` (or `hhHmmMss[.fff]S` for a duration) of a count of
/// nanoseconds, with the fraction of a second only when there is one.
fn clock(nanos: u64, colons: bool) -> String {
    let secs = nanos / 1_000_000_000;
    let frac = nanos % 1_000_000_000;
    let s = if frac == 0 { format!("{:02}", secs % 60) } else { format!("{:02}.{}", secs % 60, format!("{frac:09}").trim_end_matches('0')) };
    if colons {
        format!("{:02}:{:02}:{s}", secs / 3600, secs / 60 % 60)
    } else {
        format!("{}H{:02}M{s}S", secs / 3600, secs / 60 % 60)
    }
}

fn duration(serial: f64) -> String {
    let neg = serial < 0.0;
    let nanos = (serial.abs() * NANOS_PER_DAY).round() as u64;
    format!("{}PT{}", if neg { "-" } else { "" }, clock(nanos, false))
}

/// A text with mixed formatting: one paragraph per line, a span per run.
fn write_rich_p(out: &mut String, text: &str, runs: &[apex_sheet::rich::RichRun], names: &mut Vec<apex_sheet::rich::RunFont>) {
    out.push_str("<text:p>");
    let mut at = 0;
    for run in runs {
        let part = &text[at..at + run.len];
        at += run.len;
        let style = (!run.font.is_plain()).then(|| match names.iter().position(|f| *f == run.font) {
            Some(i) => i + 1,
            None => {
                names.push(run.font.clone());
                names.len()
            }
        });
        for (k, line) in part.split('\n').enumerate() {
            if k > 0 {
                out.push_str("</text:p><text:p>");
            }
            if line.is_empty() {
                continue;
            }
            if let Some(n) = style {
                let _ = write!(out, r#"<text:span text:style-name="T{n}">"#);
            }
            let mut spaces = 0;
            for ch in line.chars() {
                if ch == ' ' {
                    spaces += 1;
                    continue;
                }
                if spaces > 0 {
                    out.push(' ');
                    if spaces > 1 {
                        let _ = write!(out, r#"<text:s text:c="{}"/>"#, spaces - 1);
                    }
                    spaces = 0;
                }
                match ch {
                    '\t' => out.push_str("<text:tab/>"),
                    c => {
                        let mut b = [0u8; 4];
                        out.push_str(&esc(c.encode_utf8(&mut b)));
                    }
                }
            }
            if spaces > 0 {
                let _ = write!(out, r#"<text:s text:c="{spaces}"/>"#);
            }
            if style.is_some() {
                out.push_str("</text:span>");
            }
        }
    }
    out.push_str("</text:p>");
}

/// `style:text-properties` attributes of a run's font changes.
fn run_font_attrs(f: &apex_sheet::rich::RunFont) -> String {
    use apex_sheet::rich::Script;
    let mut a = String::new();
    if let Some(b) = f.bold {
        let _ = write!(a, r#" fo:font-weight="{}""#, if b { "bold" } else { "normal" });
    }
    if let Some(i) = f.italic {
        let _ = write!(a, r#" fo:font-style="{}""#, if i { "italic" } else { "normal" });
    }
    if let Some(u) = f.underline {
        let _ = write!(a, r#" style:text-underline-style="{}""#, if u { "solid" } else { "none" });
        if u {
            a.push_str(r#" style:text-underline-width="auto" style:text-underline-color="font-color""#);
        }
    }
    if let Some(s) = f.strike {
        let _ = write!(a, r#" style:text-line-through-style="{}""#, if s { "solid" } else { "none" });
    }
    if let Some(c) = f.color {
        let _ = write!(a, r##" fo:color="#{c:06x}""##);
    }
    if let Some(sz) = f.size {
        let _ = write!(a, r#" fo:font-size="{sz}pt""#);
    }
    if let Some(n) = &f.font {
        let _ = write!(a, r#" fo:font-family="{}""#, esc(n));
    }
    match f.script {
        Script::Super => a.push_str(r#" style:text-position="super 58%""#),
        Script::Sub => a.push_str(r#" style:text-position="sub 58%""#),
        Script::Normal => {}
    }
    a
}

fn write_text_p(out: &mut String, text: &str) {
    for line in text.split('\n') {
        out.push_str("<text:p>");
        let mut spaces = 0usize;
        let flush_spaces = |out: &mut String, n: &mut usize, at_start: bool| {
            if *n == 0 {
                return;
            }
            if *n == 1 && !at_start {
                out.push(' ');
            } else if at_start {
                let _ = write!(out, r#"<text:s text:c="{}"/>"#, n);
            } else {
                out.push(' ');
                if *n > 1 {
                    let _ = write!(out, r#"<text:s text:c="{}"/>"#, *n - 1);
                }
            }
            *n = 0;
        };
        let mut started = false;
        for ch in line.chars() {
            match ch {
                ' ' => spaces += 1,
                '\t' => {
                    flush_spaces(out, &mut spaces, !started);
                    out.push_str("<text:tab/>");
                    started = true;
                }
                c => {
                    flush_spaces(out, &mut spaces, !started);
                    started = true;
                    let mut b = [0u8; 4];
                    out.push_str(&esc(c.encode_utf8(&mut b)));
                }
            }
        }
        if spaces > 0 {
            let _ = write!(out, r#"<text:s text:c="{}"/>"#, spaces);
        }
        out.push_str("</text:p>");
    }
}

fn write_annotation(out: &mut String, n: &Note) {
    out.push_str("<office:annotation");
    if n.visible {
        out.push_str(r#" office:display="true""#);
    }
    out.push('>');
    if !n.author.is_empty() {
        let _ = write!(out, "<dc:creator>{}</dc:creator>", esc(&n.author));
    }
    if !n.date.is_empty() {
        let _ = write!(out, "<dc:date>{}</dc:date>", esc(&n.date));
    }
    // ODF has no threads in cells: the replies follow the text.
    write_text_p(out, &n.flat_text());
    out.push_str("</office:annotation>");
}

fn value_attrs(out: &mut String, v: &Value, wb: &Workbook, style: u32) {
    match v {
        Value::Number(n) => {
            let f = wb.styles.format(style);
            let code = &wb.styles.get(style).num_fmt;
            if f.is_date() && f.has_date_part() {
                let _ = write!(out, r#" office:value-type="date" office:date-value="{}""#, iso_date(*n));
            } else if f.is_date() {
                let _ = write!(out, r#" office:value-type="time" office:time-value="{}""#, duration(*n));
            } else if f.is_percent() {
                let _ = write!(out, r#" office:value-type="percentage" office:value="{}""#, n);
            } else if code.contains(['€', '$', '£']) {
                let _ = write!(out, r#" office:value-type="currency" office:value="{}""#, n);
            } else {
                let _ = write!(out, r#" office:value-type="float" office:value="{}""#, n);
            }
        }
        Value::Bool(b) => {
            let _ = write!(out, r#" office:value-type="boolean" office:boolean-value="{}""#, b);
        }
        Value::Text(t) if t.is_empty() => out.push_str(r#" office:value-type="string" office:string-value="""#),
        Value::Text(_) => out.push_str(r#" office:value-type="string""#),
        Value::Error(_) => out.push_str(r#" office:value-type="string" calcext:value-type="error""#),
        Value::Empty | Value::Array(_) | Value::Lambda(_) => {}
    }
}

/// ApexDesk's data model in the package.
const MODEL_PART: &str = "apexdesk/model.xml";

pub fn write(wb: &Workbook) -> Vec<u8> {
    // ODF has no table styles: tables keep their look as cell formats.
    let baked;
    let wb = if wb.sheets.iter().any(|s| !s.tables.is_empty()) {
        baked = wb.with_baked_tables();
        &baked
    } else {
        wb
    };
    // Nor pictures as cell values: they float over their cells.
    let floating;
    let wb = if wb.has_cell_pictures() {
        floating = wb.with_floating_cell_pictures();
        &floating
    } else {
        wb
    };
    let mut z = ZipWriter::new();
    z.add("mimetype", MIME.as_bytes(), false);
    let mut pics = images::Pictures::default();
    let content = write_content(wb, &mut pics);
    let backgrounds: Vec<Option<String>> = wb.sheets.iter().map(|sh| sh.page.background.as_ref().map(|b| pics.add_data(&b.data, b.format))).collect();
    let mut entries = String::new();
    for (path, _, mime) in &pics.files {
        let _ = write!(entries, r#"<manifest:file-entry manifest:full-path="{path}" manifest:media-type="{mime}"/>"#);
    }
    let basic = crate::macro_parts::odf_files(&wb.macros);
    for (path, mime, data) in &basic {
        let _ = write!(entries, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{mime}"/>"#, crate::xml::esc(path));
        z.add(path, data, true);
    }
    entries.push_str(&crate::odf_ole::manifest_entries(&pics.embedded));
    // The data model (relationships, measures): a part of our own.
    let model = crate::xlsx_model::ods_part(wb);
    if !model.is_empty() {
        let _ = write!(entries, r#"<manifest:file-entry manifest:full-path="{MODEL_PART}" manifest:media-type="text/xml"/>"#);
        z.add(MODEL_PART, format!("{HEADER}{model}").as_bytes(), true);
    }
    // Get Data queries: a part of our own that LibreOffice ignores.
    if !wb.queries.is_empty() {
        let _ = write!(entries, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="text/xml"/>"#, crate::queries_xml::ODS_PATH);
        z.add(crate::queries_xml::ODS_PATH, crate::queries_xml::write(&wb.queries).as_bytes(), true);
    }
    // The tests (Formulas > Tests): the same.
    if !wb.checks.is_empty() {
        let _ = write!(entries, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="text/xml"/>"#, crate::checks_xml::ODS_PATH);
        z.add(crate::checks_xml::ODS_PATH, crate::checks_xml::write(&wb.checks).as_bytes(), true);
    }
    z.add("META-INF/manifest.xml", format!(
        r#"{}<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="{}"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="settings.xml" manifest:media-type="text/xml"/>{entries}</manifest:manifest>"#,
        HEADER, MIME
    ).as_bytes(), true);
    z.add("meta.xml", crate::props::odf_xml(&wb.meta).as_bytes(), true);
    let (mut layouts, mut masters) = (String::new(), String::new());
    for (i, sh) in wb.sheets.iter().enumerate() {
        page::write_sheet_page(&mut layouts, &mut masters, i + 1, sh, backgrounds[i].as_deref());
    }
    let (mut named_styles, mut numbers) = (String::new(), String::new());
    named::write_named(wb, &mut named_styles, &mut numbers, &|st| cell_props(st, None), &write_number_style);
    z.add("styles.xml", format!(r#"{}<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" office:version="1.3"><office:styles><style:default-style style:family="table-cell"><style:text-properties fo:font-size="{BASE_PT}pt"/></style:default-style>{numbers}{named_styles}{}{}{}</office:styles><office:automatic-styles>{layouts}</office:automatic-styles><office:master-styles>{masters}</office:master-styles></office:document-styles>"#, HEADER, data::write_cf_styles(&data::cf_styles(wb)), if wb.sheets.iter().any(|s| !s.shapes.is_empty()) { crate::odf_shape::common_styles() } else { String::new() }, pics.shapes.named.xml).as_bytes(), true);
    z.add("settings.xml", write_settings(wb).as_bytes(), true);
    z.add("content.xml", content.as_bytes(), true);
    for (path, data, _) in &pics.files {
        // Folders ("Object 1/") are only named in the manifest.
        if path.ends_with('/') {
            continue;
        }
        let packed = matches!(apex_sheet::ImageFormat::sniff(data), Some(apex_sheet::ImageFormat::Png | apex_sheet::ImageFormat::Jpeg | apex_sheet::ImageFormat::Gif | apex_sheet::ImageFormat::Webp));
        z.add(path, data, !packed);
    }
    for (path, data, _) in &pics.embedded {
        if let Some(data) = data {
            z.add(path, data, true);
        }
    }
    z.finish()
}

fn write_settings(wb: &Workbook) -> String {
    let mut s = String::from(HEADER);
    s.push_str(r#"<office:document-settings xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0" office:version="1.3"><office:settings><config:config-item-set config:name="ooo:view-settings"><config:config-item-map-indexed config:name="Views"><config:config-item-map-entry><config:config-item config:name="ViewId" config:type="string">view1</config:config-item>"#);
    if wb.sheets.iter().any(|s| s.view == apex_sheet::ViewMode::PageBreakPreview) {
        s.push_str(r#"<config:config-item config:name="ShowPageBreakPreview" config:type="boolean">true</config:config-item>"#);
    }
    s.push_str(r#"<config:config-item-map-named config:name="Tables">"#);
    for sh in &wb.sheets {
        let _ = write!(s, r#"<config:config-item-map-entry config:name="{}">"#, esc(&sh.name));
        let item = |s: &mut String, n: &str, ty: &str, v: u32| {
            let _ = write!(s, r#"<config:config-item config:name="{}" config:type="{}">{}</config:config-item>"#, n, ty, v);
        };
        item(&mut s, "HorizontalSplitMode", "short", if sh.frozen_cols > 0 { 2 } else { 0 });
        item(&mut s, "VerticalSplitMode", "short", if sh.frozen_rows > 0 { 2 } else { 0 });
        item(&mut s, "HorizontalSplitPosition", "int", sh.frozen_cols);
        item(&mut s, "VerticalSplitPosition", "int", sh.frozen_rows);
        item(&mut s, "PositionLeft", "int", if sh.frozen_cols > 0 { sh.frozen_top.col } else { 0 });
        item(&mut s, "PositionRight", "int", sh.frozen_cols);
        item(&mut s, "PositionTop", "int", if sh.frozen_rows > 0 { sh.frozen_top.row } else { 0 });
        item(&mut s, "PositionBottom", "int", sh.frozen_rows);
        s.push_str("</config:config-item-map-entry>");
    }
    s.push_str("</config:config-item-map-named></config:config-item-map-entry></config:config-item-map-indexed></config:config-item-set>");
    if wb.calc.manual {
        s.push_str(r#"<config:config-item-set config:name="ooo:configuration-settings"><config:config-item config:name="AutoCalculate" config:type="boolean">false</config:config-item></config:config-item-set>"#);
    }
    s.push_str("</office:settings></office:document-settings>");
    s
}

/// The properties of a cell format (inside its `style:style`). Fonts are
/// declared in `fonts` when given, else named by family.
fn cell_props(st: &CellStyle, mut fonts: Option<&mut std::collections::BTreeSet<Arc<str>>>) -> String {
    let mut out = String::new();
    let mut cell_props = String::new();
    if let Some(f) = st.fill {
        let _ = write!(cell_props, r#" fo:background-color="{}""#, color(f));
    }
    let b = &st.borders;
    if !b.is_empty() {
        if b.top == b.bottom && b.top == b.left && b.top == b.right {
            let _ = write!(cell_props, r#" fo:border="{}""#, write_border(b.top));
        } else {
            for (l, k) in [(b.top, "top"), (b.bottom, "bottom"), (b.left, "left"), (b.right, "right")] {
                let _ = write!(cell_props, r#" fo:border-{}="{}""#, k, write_border(l));
            }
        }
    }
    match st.rotation {
        0 => {}
        ROTATION_STACKED => cell_props.push_str(r#" style:direction="ttb""#),
        r => {
            let _ = write!(cell_props, r#" style:rotation-angle="{}""#, (r as i32).rem_euclid(360));
        }
    }
    match st.valign {
        VAlign::Top => cell_props.push_str(r#" style:vertical-align="top""#),
        VAlign::Center => cell_props.push_str(r#" style:vertical-align="middle""#),
        VAlign::Bottom => {}
    }
    if st.wrap {
        cell_props.push_str(r#" fo:wrap-option="wrap""#);
    }
    if st.halign != HAlign::General {
        cell_props.push_str(r#" style:text-align-source="fix""#);
    }
    match (st.unlocked, st.formula_hidden) {
        (false, false) => {}
        (true, false) => cell_props.push_str(r#" style:cell-protect="none""#),
        (true, true) => cell_props.push_str(r#" style:cell-protect="formula-hidden""#),
        (false, true) => cell_props.push_str(r#" style:cell-protect="protected formula-hidden""#),
    }
    if st.checkbox {
        // Ours (Insert > Checkbox): other programs show TRUE and FALSE.
        let _ = write!(cell_props, r#" xmlns:apex="{}" apex:checkbox="true""#, crate::xlsx_model::NS);
    }
    if !cell_props.is_empty() {
        let _ = write!(out, "<style:table-cell-properties{}/>", cell_props);
    }
    let align = match st.halign {
        HAlign::Left => Some("start"),
        HAlign::Center => Some("center"),
        HAlign::Right => Some("end"),
        HAlign::Justify => Some("justify"),
        HAlign::General => None,
    };
    if align.is_some() || st.indent > 0 {
        out.push_str("<style:paragraph-properties");
        if let Some(a) = align {
            let _ = write!(out, r#" fo:text-align="{}""#, a);
        }
        if st.indent > 0 {
            let _ = write!(out, r#" fo:margin-left="{}pt""#, st.indent as f32 * INDENT_PT);
        }
        out.push_str("/>");
    }
    let mut text = String::new();
    if st.bold {
        text.push_str(r#" fo:font-weight="bold""#);
    }
    if st.italic {
        text.push_str(r#" fo:font-style="italic""#);
    }
    if st.underline {
        text.push_str(r#" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color""#);
    }
    if st.strike {
        text.push_str(r#" style:text-line-through-style="solid""#);
    }
    if st.font_size > 0 {
        let _ = write!(text, r#" fo:font-size="{}pt""#, st.font_size as f32 / 10.0);
    }
    if let Some(c) = st.color {
        let _ = write!(text, r#" fo:color="{}""#, color(c));
    }
    if let Some(f) = &st.font {
        match fonts.as_deref_mut() {
            Some(fonts) => {
                let _ = write!(text, r#" style:font-name="{}""#, esc(f));
                fonts.insert(f.clone());
            }
            None => {
                let _ = write!(text, r#" fo:font-family="{}""#, esc(f));
            }
        }
    }
    if !text.is_empty() {
        let _ = write!(out, "<style:text-properties{}/>", text);
    }
    out
}

fn write_content(wb: &Workbook, pics: &mut images::Pictures) -> String {
    let cf_styles = data::cf_styles(wb);
    let mut styles = String::new();
    // Fonts of the runs of mixed formatting, written as T1, T2...
    let mut text_styles: Vec<apex_sheet::rich::RunFont> = Vec::new();
    let mut fonts: std::collections::BTreeSet<Arc<str>> = Default::default();
    // Number styles, one per distinct code.
    let mut num_names: HashMap<Arc<str>, Option<String>> = HashMap::new();
    for (_, st) in wb.styles.iter() {
        if num_names.contains_key(&st.num_fmt) {
            continue;
        }
        let name = format!("N{}", num_names.len() + 1);
        let xml = write_number_style(&name, &st.num_fmt);
        let entry = xml.map(|x| {
            styles.push_str(&x);
            name
        });
        num_names.insert(st.num_fmt.clone(), entry);
    }
    // Cell styles.
    for (id, st) in wb.styles.iter() {
        if id == 0 {
            continue;
        }
        let _ = write!(styles, r#"<style:style style:name="ce{}" style:family="table-cell" style:parent-style-name="{}""#, id, esc(&named::parent_of(st, wb)));
        if let Some(Some(n)) = num_names.get(&st.num_fmt) {
            let _ = write!(styles, r#" style:data-style-name="{}""#, n);
        }
        styles.push('>');
        // The default size over a parent style of another size is written.
        let inherited = |ns: &apex_sheet::NamedStyle| named::masked(ns).font_size;
        let parent = st.named.as_deref().and_then(|n| wb.cell_styles.get(n)).map_or(0, inherited);
        let parent = if parent > 0 { parent } else { wb.cell_styles.normal().map_or(0, inherited) };
        if st.font_size == 0 && parent > 0 && parent != BASE_PT * 10 {
            styles.push_str(&cell_props(&CellStyle { font_size: BASE_PT * 10, ..st.clone() }, Some(&mut fonts)));
        } else {
            styles.push_str(&cell_props(st, Some(&mut fonts)));
        }
        styles.push_str("</style:style>");
    }
    // Column and row size styles.
    let mut col_styles: HashMap<u32, String> = HashMap::new();
    let mut row_styles: HashMap<u32, String> = HashMap::new();
    let size_style = |map: &mut HashMap<u32, String>, px: f32, brk: bool, rows: bool, styles: &mut String| -> String {
        let key = ((px * 10.0).round() as u32).saturating_mul(2) + brk as u32;
        let len = map.len();
        map.entry(key)
            .or_insert_with(|| {
                let name = format!("{}{}", if rows { "ro" } else { "co" }, len + 1);
                let brk = if brk { "page" } else { "auto" };
                if rows {
                    let _ = write!(styles, r#"<style:style style:name="{}" style:family="table-row"><style:table-row-properties style:row-height="{}" style:use-optimal-row-height="false" fo:break-before="{brk}"/></style:style>"#, name, fmt_len(px / ROW_PX_PER_PT));
                } else {
                    let _ = write!(styles, r#"<style:style style:name="{}" style:family="table-column"><style:table-column-properties style:column-width="{}" fo:break-before="{brk}"/></style:style>"#, name, fmt_len(px / COL_PX_PER_PT));
                }
                name
            })
            .clone()
    };
    let mut body = String::new();
    let mut taken: std::collections::HashSet<String> = wb.sheets.iter().map(|s| s.name.to_lowercase()).collect();
    if wb.sheets.iter().any(|s| wb.analysis.scenarios.contains_key(&s.name)) {
        styles.push_str(scenarios::STYLE);
    }
    for (si, sh) in wb.sheets.iter().enumerate() {
        let _ = write!(body, r#"<table:table table:name="{}""#, esc(&sh.name));
        let _ = write!(styles, r#"<style:style style:name="ta{si}" style:family="table" style:master-page-name="PageStyle_{}"><style:table-properties table:display="{}""#, si + 1, !sh.hidden);
        if let Some(c) = sh.tab_color {
            let _ = write!(styles, r#" tableooo:tab-color="{}""#, color(c));
        }
        styles.push_str("/></style:style>");
        let _ = write!(body, r#" table:style-name="ta{si}""#);
        let pg = &sh.page;
        if !pg.print_areas.is_empty() {
            let r: Vec<String> = pg.print_areas.iter().map(|a| page::print_range(&sh.name, a)).collect();
            let _ = write!(body, r#" table:print-ranges="{}""#, esc(&r.join(" ")));
        }
        if let Some(p) = &sh.protection {
            body.push_str(r#" table:protected="true""#);
            odf_key_attrs(&mut body, p);
        }
        body.push('>');
        if let Some(p) = &sh.protection {
            let _ = write!(
                body,
                r#"<loext:table-protection loext:select-protected-cells="{}" loext:select-unprotected-cells="{}" loext:insert-columns="{}" loext:insert-rows="{}" loext:delete-columns="{}" loext:delete-rows="{}"/>"#,
                p.allows(allow::SELECT_LOCKED),
                p.allows(allow::SELECT_UNLOCKED),
                p.allows(allow::INSERT_COLUMNS),
                p.allows(allow::INSERT_ROWS),
                p.allows(allow::DELETE_COLUMNS),
                p.allows(allow::DELETE_ROWS)
            );
        }
        let (shapes, frames) = images::frames(wb, si, pics);
        body.push_str(&controls::write_forms(sh, si));
        body.push_str(&shapes);
        let frame_end = |f: fn(&(u32, u32)) -> u32| frames.keys().map(f).max().unwrap_or(0);
        let ext = sh.extent();
        let val_end = |f: fn(&Range) -> u32| sh.validations.iter().flat_map(|v| v.ranges.iter()).map(f).max().unwrap_or(0);
        let last_key = |m: &std::collections::BTreeMap<u32, u8>| m.keys().next_back().copied().unwrap_or(0);
        let last_set = |s: &std::collections::BTreeSet<u32>| s.iter().next_back().copied().unwrap_or(0);
        let last_col = ext.map_or(0, |e| e.col).max(sh.col_widths.keys().next_back().copied().unwrap_or(0)).max(val_end(|r| r.end.col)).max(last_key(&sh.col_levels));
        let last_col = last_col.max(last_set(&pg.col_breaks)).max(pg.title_cols.map_or(0, |t| t.1)).max(frame_end(|k| k.1)).min(MAX_COLS - 1);
        let last_row = ext.map_or(0, |e| e.row).max(sh.row_heights.keys().next_back().copied().unwrap_or(0)).max(val_end(|r| r.end.row)).max(last_key(&sh.row_levels));
        let last_row = last_row.max(last_set(&pg.row_breaks)).max(pg.title_rows.map_or(0, |t| t.1)).max(frame_end(|k| k.0)).max(images::bottom_row(sh)).min(MAX_ROWS - 1);
        let frame_rows: std::collections::BTreeSet<u32> = frames.keys().map(|k| k.0).collect();
        let row_lv = |r: u32| sh.row_levels.get(&r).copied().unwrap_or(0);
        let col_lv = |c: u32| sh.col_levels.get(&c).copied().unwrap_or(0);
        // Print titles, when their rows (columns) share one outline level.
        let title_rows = pg.title_rows.filter(|&(a, b)| a <= b && sh.row_levels.range(a..=b).all(|(_, &l)| l == row_lv(a)) && (sh.row_levels.range(a..=b).count() as u32 == b - a + 1 || row_lv(a) == 0));
        let title_cols = pg.title_cols.filter(|&(a, b)| a <= b && sh.col_levels.range(a..=b).all(|(_, &l)| l == col_lv(a)) && (sh.col_levels.range(a..=b).count() as u32 == b - a + 1 || col_lv(a) == 0));
        // Whether a run of rows (columns) ending at `prev` may take in
        // `next`: runs stop at print titles and page breaks.
        let joins = |prev: u32, next: u32, titles: Option<(u32, u32)>, breaks: &std::collections::BTreeSet<u32>| !breaks.contains(&next) && titles.is_none_or(|(a, b)| next != a && prev != b);
        // Opens or closes group elements to reach outline level `to`.
        let regroup = |body: &mut String, cur: &mut u8, to: u8, tag: &str| {
            while *cur > to {
                let _ = write!(body, "</table:table-{tag}-group>");
                *cur -= 1;
            }
            while *cur < to {
                let _ = write!(body, "<table:table-{tag}-group>");
                *cur += 1;
            }
        };
        let vattr = |i: Option<usize>| i.map_or(String::new(), |i| format!(r#" table:content-validation-name="val{si}_{i}""#));
        // Content validation along a row, as (first column, past the last,
        // index in the sheet) from column 0 to the last: the same for each
        // band of rows between where validation ranges start and end (a
        // validation over whole rows and columns mustn't be looked up per
        // cell).
        let vbreaks: Vec<u32> = {
            let mut b: Vec<u32> = sh.validations.iter().flat_map(|v| v.ranges.iter().flat_map(|x| [x.start.row, x.end.row.saturating_add(1)])).collect();
            b.sort_unstable();
            b.dedup();
            b
        };
        let band = |r: u32| vbreaks.partition_point(|&b| b <= r);
        type Segs = std::rc::Rc<Vec<(u32, u32, Option<usize>)>>;
        // The band last looked up (rows are written in order), and the work
        // bands may still take: staggered validations make many bands of
        // many ranges each, and past it rows are written without them.
        let seg_cache: std::cell::RefCell<Option<(usize, Segs)>> = Default::default();
        let seg_work = std::cell::Cell::new(1usize << 26);
        let segs = |r: u32| -> Segs {
            if let Some((b, s)) = &*seg_cache.borrow()
                && *b == band(r)
            {
                return s.clone();
            }
            let mut cov: Vec<(usize, u32, u32)> = Vec::new();
            let all = sh.validations.iter().map(|v| v.ranges.len()).sum::<usize>();
            if let Some(left) = seg_work.get().checked_sub(all) {
                seg_work.set(left);
                for (i, v) in sh.validations.iter().enumerate() {
                    for x in v.ranges.iter().filter(|x| x.start.row <= r && x.end.row >= r && x.start.col <= last_col) {
                        cov.push((i, x.start.col, x.end.col.min(last_col)));
                    }
                }
            }
            let mut cuts: Vec<u32> = cov.iter().flat_map(|x| [x.1, x.2 + 1]).chain([0, last_col + 1]).collect();
            cuts.sort_unstable();
            cuts.dedup();
            // A sweep along the row: the ranges open at each cut, the first
            // validation of them in effect.
            let mut starts: Vec<(u32, usize)> = cov.iter().map(|x| (x.1, x.0)).collect();
            let mut ends: Vec<(u32, usize)> = cov.iter().map(|x| (x.2 + 1, x.0)).collect();
            starts.sort_unstable();
            ends.sort_unstable();
            let (mut ns, mut ne) = (0, 0);
            let mut open: std::collections::BTreeMap<usize, usize> = Default::default();
            let mut out: Vec<(u32, u32, Option<usize>)> = Vec::new();
            for w in cuts.windows(2) {
                while ns < starts.len() && starts[ns].0 <= w[0] {
                    *open.entry(starts[ns].1).or_default() += 1;
                    ns += 1;
                }
                while ne < ends.len() && ends[ne].0 <= w[0] {
                    if let Some(n) = open.get_mut(&ends[ne].1) {
                        *n -= 1;
                        if *n == 0 {
                            open.remove(&ends[ne].1);
                        }
                    }
                    ne += 1;
                }
                let v = open.keys().next().copied();
                match out.last_mut() {
                    Some(last) if last.2 == v => last.1 = w[1],
                    _ => out.push((w[0], w[1], v)),
                }
            }
            let out = std::rc::Rc::new(out);
            *seg_cache.borrow_mut() = Some((band(r), out.clone()));
            out
        };
        // Runs of equal validation along a row without cells.
        let row_runs = |r: u32| -> Vec<(u32, Option<usize>)> { segs(r).iter().map(|s| (s.1 - s.0, s.2)).collect() };
        let hidden_attr = |r: u32| if sh.filter.as_ref().and_then(|f| f.data_rows()).is_some_and(|d| d.start.row <= r && d.end.row >= r) { r#" table:visibility="filter""# } else { r#" table:visibility="collapse""# };
        // Columns, grouping runs of equal style.
        let mut c = 0;
        let mut cur = 0;
        while c <= last_col {
            let w = sh.col_width(c);
            regroup(&mut body, &mut cur, col_lv(c), "column");
            if title_cols.is_some_and(|t| t.0 == c) {
                body.push_str("<table:table-header-columns>");
            }
            let mut n = 1;
            while c + n <= last_col && sh.col_width(c + n) == w && col_lv(c + n) == col_lv(c) && joins(c + n - 1, c + n, title_cols, &pg.col_breaks) {
                n += 1;
            }
            let brk = c > 0 && pg.col_breaks.contains(&c);
            let name = size_style(&mut col_styles, if w <= 0.0 { sh.default_col_width } else { w }, brk, false, &mut styles);
            let _ = write!(body, r#"<table:table-column table:style-name="{}""#, name);
            if n > 1 {
                let _ = write!(body, r#" table:number-columns-repeated="{}""#, n);
            }
            if w <= 0.0 {
                body.push_str(r#" table:visibility="collapse""#);
            }
            body.push_str(r#" table:default-cell-style-name="Default"/>"#);
            c += n;
            if title_cols.is_some_and(|t| t.1 == c - 1) {
                body.push_str("</table:table-header-columns>");
            }
        }
        regroup(&mut body, &mut cur, 0, "column");
        // Gather cells by row.
        let mut rows: std::collections::BTreeMap<u32, Vec<(u32, &apex_sheet::Cell)>> = Default::default();
        for (cr, cell) in sh.iter() {
            rows.entry(cr.row).or_default().push((cr.col, cell));
        }
        // Merges by the rows they cover, within the table: covered cells
        // are written one by one, so what merges cover (and how many a row
        // has) is bounded; overlapping ones past that are left out.
        let mut merges_at: std::collections::HashMap<u32, Vec<&Range>> = Default::default();
        let mut area_left = 1usize << 21;
        for m in &sh.merges {
            let (er, ec) = (m.end.row.min(last_row), m.end.col.min(last_col));
            if m.start.row > er || m.start.col > ec {
                continue;
            }
            let area = (er - m.start.row + 1) as usize * (ec - m.start.col + 1) as usize;
            let Some(left) = area_left.checked_sub(area) else { continue };
            area_left = left;
            for r in m.start.row..=er {
                let v = merges_at.entry(r).or_default();
                if v.len() < 512 {
                    v.push(m);
                }
            }
        }
        let row_merges = |r: u32| merges_at.get(&r).map_or(&[][..], |v| &v[..]);
        let covered = |r: u32, c: u32| row_merges(r).iter().any(|m| m.contains(CellRef::new(r, c)) && !(m.start.row == r && m.start.col == c));
        let in_merge = |r: u32| merges_at.contains_key(&r);
        // Whether row `b` writes just like row `a` (the same plain values
        // and styles), so that it can join its run of repeated rows.
        let plain = |x: &apex_sheet::Cell| x.extra.is_none() && matches!(x.content, Content::Empty | Content::Value(_));
        let same_value = |x: &Content, y: &Content| match (x, y) {
            (Content::Value(Value::Text(p)), Content::Value(Value::Text(q))) => Arc::ptr_eq(p, q) || p == q,
            _ => x == y,
        };
        let same_row = |rows: &std::collections::BTreeMap<u32, Vec<(u32, &apex_sheet::Cell)>>, a: u32, b: u32| {
            let (Some(x), Some(y)) = (rows.get(&a), rows.get(&b)) else { return false };
            x.len() == y.len()
                && x.iter().zip(y).all(|(p, q)| p.0 == q.0 && p.1.style == q.1.style && plain(p.1) && plain(q.1) && same_value(&p.1.content, &q.1.content))
                && sh.row_height(a) == sh.row_height(b)
                && row_lv(a) == row_lv(b)
                && hidden_attr(a) == hidden_attr(b)
                && joins(a, b, title_rows, &pg.row_breaks)
                && !frame_rows.contains(&a)
                && !frame_rows.contains(&b)
                && !in_merge(a)
                && !in_merge(b)
                && band(a) == band(b)
        };
        // The last row written with cells: its number, how many times it
        // repeats and where its repeat attribute goes.
        let mut run: Option<(u32, u32, usize)> = None;
        let end_run = |body: &mut String, run: &mut Option<(u32, u32, usize)>| {
            if let Some((_, n, at)) = run.take().filter(|x| x.1 > 1) {
                body.insert_str(at, &format!(r#" table:number-rows-repeated="{n}""#));
            }
        };
        let mut r = 0;
        while r <= last_row {
            // Rows equal to the one before are written once, repeated (as
            // LibreOffice does): a long text shared down a column would
            // otherwise be written again for every cell.
            if let Some((a, n, _)) = run.as_mut()
                && *a + *n == r
                && *n < repeat_cap(rows.get(a).map_or(&[][..], |v| &v[..]))
                && same_row(&rows, *a, r)
            {
                *n += 1;
                r += 1;
                continue;
            }
            end_run(&mut body, &mut run);
            regroup(&mut body, &mut cur, row_lv(r), "row");
            if title_rows.is_some_and(|t| t.0 == r) {
                body.push_str("<table:table-header-rows>");
            }
            let cells = rows.get(&r);
            let empty_row = cells.is_none() && !frame_rows.contains(&r) && !in_merge(r);
            let h = sh.row_height(r);
            let brk = r > 0 && pg.row_breaks.contains(&r);
            let rname = size_style(&mut row_styles, if h <= 0.0 { sh.default_row_height } else { h }, brk, true, &mut styles);
            if empty_row {
                let runs = row_runs(r);
                let mut n = 1;
                while r + n <= last_row
                    && !rows.contains_key(&(r + n))
                    && !frame_rows.contains(&(r + n))
                    && sh.row_height(r + n) == h
                    && row_lv(r + n) == row_lv(r)
                    && hidden_attr(r + n) == hidden_attr(r)
                    && !in_merge(r + n)
                    && band(r + n) == band(r)
                    && joins(r + n - 1, r + n, title_rows, &pg.row_breaks)
                {
                    n += 1;
                }
                let _ = write!(body, r#"<table:table-row table:style-name="{}""#, rname);
                if n > 1 {
                    let _ = write!(body, r#" table:number-rows-repeated="{}""#, n);
                }
                if h <= 0.0 {
                    body.push_str(hidden_attr(r));
                }
                body.push('>');
                for (len, v) in runs {
                    let reps = if len > 1 { format!(r#" table:number-columns-repeated="{len}""#) } else { String::new() };
                    let _ = write!(body, "<table:table-cell{reps}{}/>", vattr(v));
                }
                body.push_str("</table:table-row>");
                r += n;
                if title_rows.is_some_and(|t| t.1 == r - 1) {
                    body.push_str("</table:table-header-rows>");
                }
                continue;
            }
            let _ = write!(body, r#"<table:table-row table:style-name="{}""#, rname);
            run = Some((r, 1, body.len()));
            if h <= 0.0 {
                body.push_str(hidden_attr(r));
            }
            body.push('>');
            let vsegs = segs(r);
            let vidx = |_: u32, c: u32| vsegs.get(vsegs.partition_point(|s| s.1 <= c)).and_then(|s| s.2);
            let mut cells: Vec<(u32, &apex_sheet::Cell)> = cells.cloned().unwrap_or_default();
            cells.sort_by_key(|x| x.0);
            let mut it = cells.into_iter().peekable();
            let mut c = 0;
            while c <= last_col {
                let is_covered = covered(r, c);
                let cell = if it.peek().is_some_and(|x| x.0 == c) { it.next().map(|x| x.1) } else { None };
                // Covered cells keep their own style (borders and fills of
                // merged areas live on them) and any hidden content.
                let frame = frames.get(&(r, c));
                if is_covered && cell.is_none_or(|x| matches!(x.content, Content::Empty | Content::Value(Value::Empty))) {
                    body.push_str("<table:covered-table-cell");
                    if let Some(st) = cell.map(|x| x.style).filter(|&s| s != 0) {
                        let _ = write!(body, r#" table:style-name="ce{st}""#);
                    }
                    match frame {
                        Some(f) => {
                            let _ = write!(body, ">{f}</table:covered-table-cell>");
                        }
                        None => body.push_str("/>"),
                    }
                    c += 1;
                    continue;
                }
                let merge = row_merges(r).iter().find(|m| m.start.row == r && m.start.col == c).filter(|_| !is_covered);
                let Some(cell) = cell else {
                    if let Some(m) = merge {
                        let _ = write!(body, r#"<table:table-cell table:number-columns-spanned="{}" table:number-rows-spanned="{}""#, m.cols(), m.rows());
                        match frame {
                            Some(f) => {
                                let _ = write!(body, ">{f}</table:table-cell>");
                            }
                            None => body.push_str("/>"),
                        }
                        c += 1;
                        continue;
                    }
                    if let Some(f) = frame {
                        let _ = write!(body, "<table:table-cell{}>{f}</table:table-cell>", vattr(vidx(r, c)));
                        c += 1;
                        continue;
                    }
                    // Run of blank cells up to the next cell or merge.
                    let v = vidx(r, c);
                    let mut n = 1;
                    // The next merge along the row (this cell is in none).
                    let stop = row_merges(r).iter().map(|m| m.start.col).filter(|&s| s > c).min().unwrap_or(u32::MAX);
                    while c + n <= last_col && c + n < stop && !it.peek().is_some_and(|x| x.0 == c + n) && !frames.contains_key(&(r, c + n)) && vidx(r, c + n) == v {
                        n += 1;
                    }
                    if n > 1 {
                        let _ = write!(body, r#"<table:table-cell table:number-columns-repeated="{}"{}/>"#, n, vattr(v));
                    } else {
                        let _ = write!(body, "<table:table-cell{}/>", vattr(v));
                    }
                    c += n;
                    continue;
                };
                let tag = if is_covered { "table:covered-table-cell" } else { "table:table-cell" };
                body.push('<');
                body.push_str(tag);
                if cell.style != 0 {
                    let _ = write!(body, r#" table:style-name="ce{}""#, cell.style);
                }
                if let Some(m) = merge {
                    let _ = write!(body, r#" table:number-columns-spanned="{}" table:number-rows-spanned="{}""#, m.cols(), m.rows());
                }
                body.push_str(&vattr(vidx(r, c)));
                if let Content::Formula { expr, .. } = &cell.content {
                    let expr = &*expr.expr();
                    let here = CellRef::new(r, c);
                    let _ = write!(body, r#" table:formula="{}""#, esc(&wb.formula_for_odf(si, here, expr)));
                    // Formulas taking ranges whole where one value is due
                    // are one-cell matrix formulas (plain ones get `@`).
                    if wb.is_array_formula(si, here) || expr.needs_array() {
                        let area = wb.spill_range(si, here).unwrap_or(Range::single(here));
                        let _ = write!(body, r#" table:number-matrix-columns-spanned="{}" table:number-matrix-rows-spanned="{}""#, area.cols(), area.rows());
                    }
                }
                let v = cell.value();
                value_attrs(&mut body, v, wb, cell.style);
                if matches!(v, Value::Empty) && cell.extra.is_none() && frame.is_none() {
                    body.push_str("/>");
                } else {
                    body.push('>');
                    if let Some(n) = cell.note() {
                        write_annotation(&mut body, n);
                    }
                    if let Some(f) = frame {
                        body.push_str(f);
                    }
                    if !matches!(v, Value::Empty) {
                        let shown = match v {
                            Value::Text(t) => t.to_string(),
                            Value::Error(e) => e.as_str().to_string(),
                            _ => wb.display(si, CellRef::new(r, c)).0,
                        };
                        match cell.link() {
                            None if let (Value::Text(t), Some(runs)) = (v, cell.rich()) => write_rich_p(&mut body, t, runs, &mut text_styles),
                            Some(l) => {
                                let _ = write!(body, r#"<text:p><text:a xlink:href="{}" xlink:type="simple""#, esc(&target_to_odf_href(&l.target)));
                                if !l.tip.is_empty() {
                                    let _ = write!(body, r#" office:title="{}""#, esc(&l.tip));
                                }
                                let _ = write!(body, ">{}</text:a></text:p>", esc(&shown).replace('\n', "<text:line-break/>"));
                            }
                            None => write_text_p(&mut body, &shown),
                        }
                    }
                    let _ = write!(body, "</{tag}>");
                }
                c += 1;
            }
            body.push_str("</table:table-row>");
            if title_rows.is_some_and(|t| t.1 == r) {
                body.push_str("</table:table-header-rows>");
            }
            r += 1;
        }
        end_run(&mut body, &mut run);
        regroup(&mut body, &mut cur, 0, "row");
        body.push_str(&data::write_named(wb, Some(&sh.name), &scenarios::solver_named(wb, si)));
        body.push_str(&data::write_cond_formats(sh, &cf_styles));
        body.push_str(&chart::write_sparklines(sh));
        body.push_str("</table:table>");
        scenarios::write_tables(wb, &sh.name, &mut taken, &mut body);
    }
    for (i, f) in text_styles.iter().enumerate() {
        let _ = write!(styles, r#"<style:style style:name="T{}" style:family="text"><style:text-properties{}/></style:style>"#, i + 1, run_font_attrs(f));
    }
    let mut s = String::with_capacity(body.len() + styles.len() + 2048);
    s.push_str(HEADER);
    s.push_str(r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:tableooo="http://openoffice.org/2009/table" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" xmlns:form="urn:oasis:names:tc:opendocument:xmlns:form:1.0" xmlns:script="urn:oasis:names:tc:opendocument:xmlns:script:1.0" xmlns:ooo="http://openoffice.org/2004/office" xmlns:xforms="http://www.w3.org/2002/xforms" office:version="1.3">"#);
    if !fonts.is_empty() {
        s.push_str("<office:font-face-decls>");
        for f in &fonts {
            let _ = write!(s, r#"<style:font-face style:name="{0}" svg:font-family="&apos;{0}&apos;"/>"#, esc(f));
        }
        s.push_str("</office:font-face-decls>");
    }
    s.push_str("<office:automatic-styles>");
    s.push_str(&styles);
    s.push_str(&pics.styles);
    s.push_str(&pics.shapes.styles);
    s.push_str("</office:automatic-styles><office:body><office:spreadsheet");
    if let Some(p) = &wb.protection {
        s.push_str(r#" table:structure-protected="true""#);
        odf_key_attrs(&mut s, p);
    }
    s.push('>');
    if wb.calc.iterate || wb.calc.date1904 || wb.calc.as_shown {
        s.push_str(if wb.calc.as_shown { r#"<table:calculation-settings table:precision-as-shown="true">"# } else { "<table:calculation-settings>" });
        // Dates are written as dates, so the null date only matters to
        // programs that keep the setting.
        if wb.calc.date1904 {
            s.push_str(r#"<table:null-date table:date-value="1904-01-01"/>"#);
        }
        if wb.calc.iterate {
            let _ = write!(s, r#"<table:iteration table:status="enable" table:steps="{}" table:maximum-difference="{}"/>"#, wb.calc.max_iter, wb.calc.max_change);
        }
        s.push_str("</table:calculation-settings>");
    }
    s.push_str(&data::write_validations(wb));
    s.push_str(&body);
    s.push_str(&data::write_named(wb, None, ""));
    s.push_str(&data::write_database_ranges(wb));
    s.push_str(&pivot::write_data_pilots(wb));
    s.push_str("</office:spreadsheet></office:body></office:document-content>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas() {
        assert_eq!(odf_to_canonical("of:=SUM([.A1:.B2];[$Hoja2.C3])"), "SUM(A1:B2,Hoja2!C3)");
        assert_eq!(odf_to_canonical("of:=['My sheet'.A1]&\"a;b\""), "'My sheet'!A1&\"a;b\"");
        assert_eq!(odf_to_canonical("of:=[.$A$1]*2"), "$A$1*2");
        assert_eq!(odf_to_canonical("of:=SUM([$Jan.A1:$'Mar Q1'.B2])"), "SUM('Jan:Mar Q1'!A1:B2)");
        assert_eq!(odf_to_canonical("of:=['file:///home/u/B.ods'#$Sheet1.A1]+['file:///x.ods'#$'My s'.B2:.C3]"), "'[file:///home/u/B.ods]Sheet1'!A1+'[file:///x.ods]My s'!B2:C3");
        assert_eq!(odf_to_canonical("of:=SUM([.A1:.C3]![.B2:.B9];([.A1]~[.C3]))+#DIV/0!"), "SUM(A1:C3 B2:B9,(A1,C3))+#DIV/0!");
        assert_eq!(odf_to_canonical("of:=TRANSPOSE({1;2|3;4})"), "TRANSPOSE({1,2;3,4})");
        // LibreOffice names for Excel functions, both ways.
        let src = "of:=COM.MICROSOFT.SORT([.B2:.B6];;-1)+FDIST(1;2;3;TRUE())+LEGACY.TDIST(1;2;1)+COM.MICROSOFT.LET(_xlpm.x;5;_xlpm.x*2)";
        let e = parse_canonical(&odf_to_canonical(src)).unwrap();
        assert_eq!(e.to_canonical(), "SORT(B2:B6,,-1)+F.DIST(1,2,3,TRUE)+TDIST(1,2,1)+LET(x,5,x*2)");
        assert_eq!(e.to_odf(), src);
        let e = parse_canonical("SKEW.P(A1:A3)&FORMULATEXT(A1)&SUM(A1#)").unwrap();
        assert_eq!(e.to_odf(), "of:=SKEWP([.A1:.A3])&FORMULA([.A1])&SUM(_xlfn.ANCHORARRAY([.A1]))");
        assert_eq!(parse_canonical(&odf_to_canonical(&e.to_odf())).unwrap().to_canonical(), "SKEW.P(A1:A3)&FORMULATEXT(A1)&SUM(A1#)");
        // Functions LibreOffice doesn't know, as it writes them, and a
        // function passed as a value.
        let e = parse_canonical(&odf_to_canonical("of:=_xlfn.groupby([.A6:.A11];[.E6:.E11];_xleta.sum)+_xlfn.trimrange([.A1:.F20])+DSUM([.A1:.C9];3;[.E1:.E2])")).unwrap();
        assert_eq!(e.to_canonical(), "GROUPBY(A6:A11,E6:E11,SUM)+TRIMRANGE(A1:F20)+DSUM(A1:C9,3,E1:E2)");
        assert_eq!(e.to_odf(), "of:=_xlfn.GROUPBY([.A6:.A11];[.E6:.E11];_xleta.SUM)+_xlfn.TRIMRANGE([.A1:.F20])+DSUM([.A1:.C9];3;[.E1:.E2])");
        assert_eq!(e.to_xlsx(), "_xlfn.GROUPBY(A6:A11,E6:E11,_xleta.SUM)+_xlfn.TRIMRANGE(A1:F20)+DSUM(A1:C9,3,E1:E2)");
        assert_eq!(parse_canonical(&e.to_xlsx()).unwrap(), e);
    }

    #[test]
    fn double_byte_and_web_formulas() {
        let src = "of:=LENB(JIS([.A1]))&ASC(\"x\")&COM.MICROSOFT.BAHTTEXT(1)&COM.MICROSOFT.ENCODEURL(\"a\")&COM.MICROSOFT.FILTERXML([.A1];\"//b\")&INFO(\"system\")";
        let e = parse_canonical(&odf_to_canonical(src)).unwrap();
        assert_eq!(e.to_canonical(), "LENB(DBCS(A1))&ASC(\"x\")&BAHTTEXT(1)&ENCODEURL(\"a\")&FILTERXML(A1,\"//b\")&INFO(\"system\")");
        assert_eq!(e.to_odf(), src);
        assert_eq!(e.to_xlsx(), "LENB(DBCS(A1))&ASC(\"x\")&_xlfn.BAHTTEXT(1)&_xlfn.ENCODEURL(\"a\")&_xlfn.FILTERXML(A1,\"//b\")&INFO(\"system\")");
    }

    #[test]
    fn complex_and_legacy_names() {
        // As LibreOffice writes them.
        let src = "of:=IMSUM(IMCOSH([.A1]);COMPLEX(1;2))&BESSELK(1.5;1)&_xlfn.ECMA.CEILING(2.5;1)&BETADIST(0.5;2;3)&HYPGEOMDIST(1;4;8;20)&LOGNORMDIST(4;3.5;1.2)&LEGACY.NORMSDIST(1)";
        let e = parse_canonical(&odf_to_canonical(src)).unwrap();
        assert_eq!(e.to_canonical(), "IMSUM(IMCOSH(A1),COMPLEX(1,2))&BESSELK(1.5,1)&ECMA.CEILING(2.5,1)&BETADIST(0.5,2,3)&HYPGEOMDIST(1,4,8,20)&LOGNORMDIST(4,3.5,1.2)&NORMSDIST(1)");
        assert_eq!(e.to_odf(), src);
    }

    #[test]
    fn fractional_seconds() {
        assert_eq!(iso_date(45042.5), "2023-04-26T12:00:00");
        assert_eq!(duration(0.5 + 0.25 / 86400.0), "PT12H00M00.25S");
        assert_eq!(iso_date(45042.0 + 0.999_999_999_999_9), "2023-04-27");
        for (n, time) in [(45042.6051406, false), (0.233, true), (1.5 + 1.0 / 3.0 / 86400.0, true)] {
            let back = if time { parse_duration(&duration(n)) } else { parse_date(&iso_date(n)) }.unwrap();
            assert!((back - n).abs() < 1e-11, "{n} {back}");
        }
    }

    #[test]
    fn number_styles() {
        for code in ["0.00", "#,##0.00", "0%", "0.00E+00", "dd/mm/yyyy", "h:mm:ss", "hh:mm:ss.0000", "[h]:mm:ss.00", "#,##0.00 \"€\"", "[h]:mm:ss", "d mmm yyyy", "* #,##0.00 \"€\""] {
            let xml = write_number_style("N1", code).unwrap();
            let doc = format!("<r xmlns:number='x' xmlns:style='y'>{}</r>", xml);
            let mut st = Styles::default();
            read_styles(&doc, &mut st);
            assert_eq!(st.numbers.get("N1").map(String::as_str), Some(code), "{}", xml);
        }
    }

    #[test]
    fn views_and_background() {
        use apex_sheet::{SheetBackground, ViewMode};
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "x");
        let png = crate::xlsx_drawing::tests::png();
        wb.set_background(0, Some(SheetBackground { data: png, format: apex_sheet::ImageFormat::Png }));
        wb.sheets[0].view = ViewMode::PageBreakPreview;
        wb.sheets.push(Sheet::new("Two"));
        let bytes = write(&wb);
        if let Some(d) = std::env::var_os("APEX_DUMP") {
            apex_fs::write(std::path::Path::new(&d).join("views.ods"), &bytes).unwrap();
        }
        let z = ZipReader::new(&bytes).unwrap();
        assert!(z.read_string("META-INF/manifest.xml").unwrap().contains("Pictures/image1.png"));
        assert!(z.read_string("styles.xml").unwrap().contains(r#"<style:background-image xlink:href="Pictures/image1.png""#));
        let back = read(&bytes).unwrap();
        assert_eq!(back.sheets[0].page.background, wb.sheets[0].page.background);
        assert!(back.sheets[1].page.background.is_none());
        // LibreOffice has one page break setting for the window.
        assert_eq!(back.sheets[0].view, ViewMode::PageBreakPreview);
        wb.sheets[0].view = ViewMode::Normal;
        assert_eq!(read(&write(&wb)).unwrap().sheets[0].view, ViewMode::Normal);
    }

    #[test]
    fn page_setup() {
        let mut wb = Workbook::new();
        wb.sheets[0].name = "It's 1".into();
        for r in 0..30 {
            wb.set_input(0, CellRef::new(r, r % 4), "x");
        }
        wb.sheets[0].row_levels.insert(1, 1);
        wb.sheets[0].row_levels.insert(25, 1);
        wb.sheets[0].col_widths.insert(0, 150.0);
        wb.sheets[0].page = crate::xlsx_page::tests::sample_page();
        wb.sheets.push(Sheet::new("Plain"));
        let mut p = apex_sheet::SheetPage { scale: 75, ..Default::default() };
        p.hf.footer = apex_sheet::HfText::center("&P");
        p.title_rows = Some((3, 4));
        p.row_breaks.insert(4);
        wb.sheets[1].page = p;
        wb.sheets.push(Sheet::new("Default"));
        let bytes = write(&wb);
        if let Some(d) = std::env::var_os("APEX_DUMP") {
            apex_fs::write(std::path::Path::new(&d).join("page_setup.ods"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        // Title rows inside an outline group of another level are dropped.
        let mut orig = wb.sheets[0].page.clone();
        orig.title_rows = None;
        crate::xlsx_page::tests::check_page(&orig, &back.sheets[0].page, 0.1);
        for i in 1..3 {
            crate::xlsx_page::tests::check_page(&wb.sheets[i].page, &back.sheets[i].page, 0.1);
        }
        assert_eq!(back.sheets[0].row_levels, wb.sheets[0].row_levels);
        assert_eq!(back.display(0, CellRef::new(29, 1)).0, "x");
        wb.sheets[0].row_levels.clear();
        let back = read(&write(&wb)).unwrap();
        crate::xlsx_page::tests::check_page(&wb.sheets[0].page, &back.sheets[0].page, 0.1);
    }

    #[test]
    fn data_tools() {
        let wb = crate::xlsx_data::sample::workbook();
        let bytes = write(&wb);
        if let Some(d) = std::env::var_os("APEX_DUMP") {
            apex_fs::write(std::path::Path::new(&d).join("data_tools.ods"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        crate::xlsx_data::sample::check(&wb, &back, false);
        // ODS has no Stop If True nor reversed icon sets.
        let mut rules = wb.sheets[0].cond_formats.clone();
        for r in &mut rules {
            r.stop = false;
            if let apex_sheet::data::CfKind::IconSet { reverse, .. } = &mut r.kind {
                *reverse = false;
            }
        }
        assert_eq!(back.sheets[0].cond_formats, rules);
    }

    #[test]
    fn roundtrip() {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::EN;
        wb.set_input(0, CellRef::new(0, 0), "Nombre");
        wb.set_input(0, CellRef::new(0, 1), "  dos  espacios");
        wb.set_input(0, CellRef::new(1, 0), "10");
        wb.set_input(0, CellRef::new(2, 0), "=A2*3");
        wb.set_input(0, CellRef::new(3, 0), "2024-03-15");
        wb.set_input(0, CellRef::new(4, 0), "TRUE");
        wb.set_input(0, CellRef::new(500, 3), "lejos");
        wb.modify_style(0, &Range::single(CellRef::new(0, 0)), |s| {
            s.bold = true;
            s.fill = Some(0x336699);
            s.halign = HAlign::Center;
            s.font = Some(Arc::from("DejaVu Serif"));
            s.indent = 3;
            s.rotation = 30;
            s.borders = Borders::sides(border::BOTTOM | border::LEFT, Line::new(LineStyle::Medium, Some(0x00FF00)));
        });
        wb.modify_style(0, &Range::single(CellRef::new(1, 0)), |s| {
            s.halign = HAlign::Justify;
            s.rotation = -90;
            s.borders = Borders::sides(border::ALL, Line::new(LineStyle::Double, None));
        });
        wb.sheets[0].tab_color = Some(0xFF0000);
        wb.sheets[0].col_widths.insert(1, 200.0);
        wb.sheets[0].row_heights.insert(0, 40.0);
        wb.sheets[0].merges.push(Range::parse("C1:D2").unwrap());
        wb.sheets[0].frozen_rows = 1;
        wb.execute(apex_sheet::Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("Datos 2")) });
        wb.set_input(1, CellRef::new(0, 0), "='Sheet1'!A3+1");
        let mut hidden = Sheet::new("Oculta");
        hidden.hidden = true;
        wb.sheets.push(hidden);
        let bytes = write(&wb);
        let mut back = read(&bytes).unwrap();
        back.locale = apex_sheet::Locale::EN;
        let sh = &back.sheets[0];
        assert_eq!(sh.value(CellRef::new(0, 1)), &Value::text("  dos  espacios"));
        assert_eq!(sh.value(CellRef::new(2, 0)), &Value::Number(30.0));
        assert_eq!(back.edit_text(0, CellRef::new(2, 0)), "=A2*3");
        assert_eq!(sh.value(CellRef::new(4, 0)), &Value::Bool(true));
        assert_eq!(sh.value(CellRef::new(500, 3)), &Value::text("lejos"));
        assert_eq!(back.display(0, CellRef::new(3, 0)).0, wb.display(0, CellRef::new(3, 0)).0);
        let st = back.styles.get(sh.get(CellRef::new(0, 0)).unwrap().style);
        assert!(st.bold);
        assert_eq!(st.fill, Some(0x336699));
        assert_eq!(st.halign, HAlign::Center);
        assert_eq!(st.font.as_deref(), Some("DejaVu Serif"));
        assert_eq!((st.indent, st.rotation), (3, 30));
        assert_eq!(st.borders, Borders::sides(border::BOTTOM | border::LEFT, Line::new(LineStyle::Medium, Some(0x00FF00))));
        let st2 = back.styles.get(sh.get(CellRef::new(1, 0)).unwrap().style);
        assert_eq!((st2.halign, st2.rotation), (HAlign::Justify, -90));
        assert_eq!(st2.borders, Borders::sides(border::ALL, Line::new(LineStyle::Double, None)));
        assert_eq!(sh.tab_color, Some(0xFF0000));
        assert!((sh.col_width(1) - 200.0).abs() < 2.0);
        assert!((sh.row_height(0) - 40.0).abs() < 2.0);
        assert_eq!(sh.merges, vec![Range::parse("C1:D2").unwrap()]);
        assert_eq!(sh.frozen_rows, 1);
        assert_eq!(back.sheets[1].name, "Datos 2");
        assert_eq!(back.sheets[1].value(CellRef::new(0, 0)), &Value::Number(31.0));
        assert!(back.sheets[2].hidden && !back.sheets[1].hidden);
    }
}
