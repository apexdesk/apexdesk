//! Reading a formula as it is typed: the word being typed (for
//! autocomplete), the function call and argument at the caret (for the
//! argument hint) and the references it names (to color them).

use crate::functions::Func;
use crate::{CellRef, Range};

/// The identifier being typed at the end of `text` (for autocomplete).
pub fn word_at_end(text: &str) -> Option<&str> {
    if !text.starts_with('=') || in_string(text) {
        return None;
    }
    let start = text.rfind(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_')).map_or(0, |i| i + 1);
    let w = &text[start..];
    (!w.is_empty() && w.chars().next().is_some_and(|c| c.is_alphabetic()) && start > 0).then_some(w)
}

fn in_string(text: &str) -> bool {
    text.chars().filter(|&c| c == '"').count() % 2 == 1
}

/// The innermost open function call at the end of `text` and the index of
/// the argument being typed.
pub fn call_at_end(text: &str, sep: char) -> Option<(Func, usize)> {
    if !text.starts_with('=') {
        return None;
    }
    let mut stack: Vec<(Option<Func>, usize)> = Vec::new();
    let mut quoted = false;
    let mut ident_start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if quoted {
            quoted = c != '"';
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => {
                let f = ident_start.and_then(|s| Func::lookup(&text[s..i]));
                stack.push((f, 0));
            }
            ')' => {
                stack.pop();
            }
            c if c == sep || c == ';' => {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
            }
            _ => {}
        }
        if c.is_alphanumeric() || c == '.' || c == '_' {
            ident_start.get_or_insert(i);
        } else {
            ident_start = None;
        }
    }
    stack.into_iter().rev().find_map(|(f, n)| f.map(|f| (f, n)))
}

/// A cell or range reference found in a formula: its byte span, the range
/// and the sheet name if it is qualified.
#[derive(Clone, Debug, PartialEq)]
pub struct FormulaRef {
    pub span: std::ops::Range<usize>,
    pub range: Range,
    pub sheet: Option<String>,
}

/// Finds the references in a formula (not inside strings or function names).
pub fn references(text: &str) -> Vec<FormulaRef> {
    let mut out = Vec::new();
    if !text.starts_with('=') {
        return out;
    }
    let b = text.as_bytes();
    let mut i = 1;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            i += 1;
            continue;
        }
        // Quoted sheet name: 'My sheet'!A1
        let mut sheet = None;
        let start = i;
        if c == b'\'' {
            if let Some(end) = text[i + 1..].find('\'') {
                let name_end = i + 1 + end;
                if b.get(name_end + 1) == Some(&b'!') {
                    sheet = Some(text[i + 1..name_end].to_string());
                    i = name_end + 2;
                }
            }
            if sheet.is_none() {
                i += 1;
                continue;
            }
        }
        if !(b[i].is_ascii_alphanumeric() || b[i] == b'$' || b[i] == b'_') {
            i += 1;
            continue;
        }
        let tok_start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'$' || b[i] == b'_' || b[i] == b'.' || b[i] >= 0x80) {
            i += 1;
        }
        let tok = &text[tok_start..i];
        if sheet.is_none() && b.get(i) == Some(&b'!') {
            sheet = Some(tok.to_string());
            i += 1;
            let s2 = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'$') {
                i += 1;
            }
            let Some(r) = parse_ref(text, s2, &mut i) else { continue };
            out.push(FormulaRef { span: start..i, range: r, sheet });
            continue;
        }
        if b.get(i) == Some(&b'(') {
            continue;
        }
        let mut j = tok_start;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'$') {
            j += 1;
        }
        if j != i {
            continue;
        }
        if let Some(r) = parse_ref(text, tok_start, &mut i) {
            out.push(FormulaRef { span: start..i, range: r, sheet });
        }
    }
    out
}

/// Parses `A1` or `A1:B2` starting at `from` (the first cell already
/// scanned up to `*end`); extends `*end` over a `:B2` part.
fn parse_ref(text: &str, from: usize, end: &mut usize) -> Option<Range> {
    let a = CellRef::parse(&text[from..*end])?;
    let b = text.as_bytes();
    if b.get(*end) == Some(&b':') {
        let mut k = *end + 1;
        while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'$') {
            k += 1;
        }
        if let Some(c) = CellRef::parse(&text[*end + 1..k]) {
            *end = k;
            return Some(Range::new(a, c));
        }
    }
    Some(Range::single(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_references() {
        let r = references("=SUM(A1:B2)+$C$3*'My sheet'!D4-\"E5\"+Hoja2!F6");
        let spans: Vec<String> = r.iter().map(|x| x.range.to_a1()).collect();
        assert_eq!(spans, vec!["A1:B2", "C3", "D4", "F6"]);
        assert_eq!(r[2].sheet.as_deref(), Some("My sheet"));
        assert_eq!(r[3].sheet.as_deref(), Some("Hoja2"));
        assert!(references("=LOG10(2)").is_empty());
    }

    #[test]
    fn call_context() {
        assert_eq!(call_at_end("=SUM(A1,", ','), Some((Func::Sum, 1)));
        assert_eq!(call_at_end("=IF(A1>0;MAX(B1;", ';'), Some((Func::Max, 1)));
        assert_eq!(call_at_end("=IF(A1>0;MAX(B1;2);", ';'), Some((Func::If, 2)));
        assert_eq!(call_at_end("=A1+", ','), None);
        assert_eq!(word_at_end("=SU"), Some("SU"));
        assert_eq!(word_at_end("=A1+bus"), Some("bus"));
        assert_eq!(word_at_end("=\"SU"), None);
    }
}
