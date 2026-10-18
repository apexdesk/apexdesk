//! Tokens of VBA source: words (keywords are words too, the parser tells
//! them apart), numbers, strings, `#date#` literals and symbols, with the
//! line each one is on. Comments (`'`, `Rem`) and line continuations
//! (` _`) go away here.

use crate::error::{Error, R};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// A name or keyword, without its type suffix (`Left$` is `Left`).
    Word(String),
    Int(i64),
    Num(f64),
    Str(String),
    Date(f64),
    /// `( ) , . = <> < > <= >= + - * / \ ^ & ; := !`
    Sym(&'static str),
    /// A new line.
    Eol,
    /// `:` between statements.
    Colon,
    Eof,
    /// `Name:` or a line number at the start of a statement.
    Label(String),
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
}

const SYMBOLS: [&str; 21] = ["#", ":=", "<>", "<=", ">=", "(", ")", ",", ".", "=", "<", ">", "+", "-", "*", "/", "\\", "^", "&", ";", "!"];

/// Month/day/year and time of a `#…#` literal.
fn date_literal(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date, time) = match s.find(|c: char| c == ' ') {
        Some(i) if s[..i].contains(['/', '-']) => (&s[..i], s[i..].trim()),
        _ if s.contains(':') => ("", s),
        _ => (s, ""),
    };
    let mut serial = 0.0;
    if !date.is_empty() {
        let parts: Vec<i64> = date.split(['/', '-']).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        let (y, m, d) = match parts[..] {
            [y, m, d] if y > 31 => (y, m, d),
            [m, d, y] => (if y < 30 { y + 2000 } else if y < 100 { y + 1900 } else { y }, m, d),
            _ => return None,
        };
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        serial = apex_sheet::format::date_to_serial(y, m, d);
    }
    if !time.is_empty() {
        let lower = time.to_ascii_lowercase();
        let (t, pm, am) = match lower.strip_suffix("pm") {
            Some(t) => (t.trim().to_string(), true, false),
            None => match lower.strip_suffix("am") {
                Some(t) => (t.trim().to_string(), false, true),
                None => (lower.clone(), false, false),
            },
        };
        let p: Vec<f64> = t.split(':').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
        let mut h = *p.first()?;
        if pm && h < 12.0 {
            h += 12.0;
        }
        if am && h == 12.0 {
            h = 0.0;
        }
        let m = p.get(1).copied().unwrap_or(0.0);
        let sec = p.get(2).copied().unwrap_or(0.0);
        serial += (h * 3600.0 + m * 60.0 + sec) / 86_400.0;
    }
    Some(serial)
}

pub fn tokenize(src: &str, module: &str) -> R<Vec<Token>> {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    // Whether the next token starts a statement (for `Rem`).
    let mut start = true;
    let mut space = false;
    let n = chars.len();
    let push = |out: &mut Vec<Token>, tok: Tok, line: u32, _space: bool| out.push(Token { tok, line });
    while i < n {
        // Lines starting `Attribute` are left out of the project's signed
        // hash: whatever such a line holds (`Attribute: Shell ...`, a label
        // and a statement) would run unsigned, so none of it runs.
        if (i == 0 || matches!(chars[i - 1], '\n' | '\r')) && chars.get(i..i + 9).is_some_and(|w| w.iter().collect::<String>().eq_ignore_ascii_case("attribute")) {
            while i < n && !matches!(chars[i], '\n' | '\r') {
                i += 1;
            }
            continue;
        }
        let c = chars[i];
        if c == '\n' {
            if matches!(out.last().map(|t| &t.tok), Some(Tok::Colon)) {
                out.pop();
            }
            if !matches!(out.last().map(|t| &t.tok), Some(Tok::Eol) | None) {
                push(&mut out, Tok::Eol, line, space);
            }
            line += 1;
            i += 1;
            start = true;
            space = false;
            continue;
        }
        if c == ' ' || c == '\t' || c == '\r' || c == '\u{a0}' {
            i += 1;
            space = true;
            continue;
        }
        // Line continuation: ` _` at the end of the line.
        if c == '_' && space {
            let mut j = i + 1;
            while j < n && matches!(chars[j], ' ' | '\t' | '\r') {
                j += 1;
            }
            if j >= n || chars[j] == '\n' {
                i = j + 1;
                line += 1;
                continue;
            }
        }
        if c == '\'' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let tok_line = line;
        let sp = std::mem::take(&mut space);
        if c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                if i >= n || chars[i] == '\n' {
                    return Err(Error::syntax(module, line, "Unterminated string"));
                }
                if chars[i] == '"' {
                    if i + 1 < n && chars[i + 1] == '"' {
                        s.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            push(&mut out, Tok::Str(s), tok_line, sp);
            start = false;
            continue;
        }
        if c == ':' && chars.get(i + 1) != Some(&'=') {
            if !matches!(out.last().map(|t| &t.tok), Some(Tok::Eol | Tok::Colon) | None) {
                push(&mut out, Tok::Colon, tok_line, sp);
            }
            i += 1;
            start = true;
            continue;
        }
        if c == '#' {
            if let Some(len) = chars[i + 1..].iter().take_while(|&&x| x != '\n').position(|&x| x == '#') {
                let body: String = chars[i + 1..i + 1 + len].iter().collect();
                if body.chars().any(|x| x.is_ascii_digit()) && body.chars().all(|x| x.is_ascii_alphanumeric() || " /:-.".contains(x)) {
                    // Else `#1: Close #2`: file numbers.
                    if let Some(d) = date_literal(&body) {
                        push(&mut out, Tok::Date(d), tok_line, sp);
                        i += len + 2;
                        start = false;
                        continue;
                    }
                }
            }
        }
        // &H1F, &O17
        if c == '&' && i + 1 < n && matches!(chars[i + 1], 'h' | 'H' | 'o' | 'O') {
            let radix = if matches!(chars[i + 1], 'h' | 'H') { 16 } else { 8 };
            let mut j = i + 2;
            while j < n && chars[j].is_digit(radix) {
                j += 1;
            }
            if j > i + 2 {
                let digits: String = chars[i + 2..j].iter().collect();
                let v = u64::from_str_radix(&digits, radix).map_err(|_| Error::syntax(module, line, "Invalid number"))?;
                // &HFFFF is -1 as an Integer, &HFFFFFFFF as a Long.
                let v = if j - (i + 2) <= 4 && v > 0x7FFF && v <= 0xFFFF && chars.get(j) != Some(&'&') {
                    v as i64 - 0x10000
                } else if v > 0x7FFF_FFFF && v <= 0xFFFF_FFFF {
                    v as i64 - 0x1_0000_0000
                } else {
                    v as i64
                };
                if matches!(chars.get(j), Some('&' | '%' | '^')) {
                    j += 1;
                }
                push(&mut out, Tok::Int(v), tok_line, sp);
                i = j;
                start = false;
                continue;
            }
        }
        let prev_value = matches!(out.last().map(|t| &t.tok), Some(Tok::Word(_) | Tok::Sym(")")));
        if c.is_ascii_digit() || (c == '.' && !prev_value && chars.get(i + 1).is_some_and(|x| x.is_ascii_digit())) {
            let mut j = i;
            while j < n && chars[j].is_ascii_digit() {
                j += 1;
            }
            let mut float = false;
            if j < n && chars[j] == '.' && chars.get(j + 1).is_none_or(|x| !x.is_ascii_alphabetic()) {
                float = true;
                j += 1;
                while j < n && chars[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if j < n && matches!(chars[j], 'e' | 'E' | 'd' | 'D') {
                let mut k = j + 1;
                if k < n && matches!(chars[k], '+' | '-') {
                    k += 1;
                }
                if k < n && chars[k].is_ascii_digit() {
                    while k < n && chars[k].is_ascii_digit() {
                        k += 1;
                    }
                    float = true;
                    j = k;
                }
            }
            let text: String = chars[i..j].iter().map(|&x| if x == 'd' || x == 'D' { 'e' } else { x }).collect();
            let mut suffix_float = false;
            if j < n && matches!(chars[j], '#' | '!' | '@') {
                suffix_float = true;
                j += 1;
            } else if j < n && matches!(chars[j], '%' | '&' | '^') {
                j += 1;
            }
            // A line number at the start of a statement is a label.
            let tok = if float || suffix_float {
                Tok::Num(text.parse().map_err(|_| Error::syntax(module, line, "Invalid number"))?)
            } else {
                match text.parse::<i64>() {
                    Ok(v) => Tok::Int(v),
                    Err(_) => Tok::Num(text.parse().map_err(|_| Error::syntax(module, line, "Invalid number"))?),
                }
            };
            if start && !float {
                if let Tok::Int(v) = tok {
                    push(&mut out, Tok::Label(v.to_string()), tok_line, sp);
                    push(&mut out, Tok::Eol, tok_line, false);
                    i = j;
                    while i < n && matches!(chars[i], ' ' | '\t') {
                        i += 1;
                    }
                    if i < n && chars[i] == ':' && chars.get(i + 1) != Some(&'=') {
                        i += 1;
                    }
                    continue;
                }
            }
            push(&mut out, tok, tok_line, sp);
            i = j;
            start = false;
            continue;
        }
        if c.is_alphabetic() || c == '_' || (c == '[' && chars[i..].iter().take_while(|&&x| x != '\n').any(|&x| x == ']')) {
            let mut j = i;
            let word: String = if c == '[' {
                // [Name With Spaces] or [A1] (Evaluate): taken as a word.
                let end = i + chars[i..].iter().position(|&x| x == ']').unwrap_or(0);
                j = end + 1;
                chars[i..=end].iter().collect()
            } else {
                while j < n && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                chars[i..j].iter().collect()
            };
            // Type suffixes: Left$, n%, x# (`&` and `!` only before a separator).
            if j < n && matches!(chars[j], '$' | '%' | '#' | '@') {
                j += 1;
            } else if j < n && matches!(chars[j], '&' | '!') && chars.get(j + 1).is_none_or(|x| matches!(x, ' ' | '\t' | '\r' | '\n' | ')' | ',' | ':' | '=')) {
                j += 1;
            }
            if start && word.eq_ignore_ascii_case("rem") && chars.get(j).is_none_or(|x| !x.is_alphanumeric()) {
                while j < n && chars[j] != '\n' {
                    j += 1;
                }
                i = j;
                continue;
            }
            if start && !word.eq_ignore_ascii_case("else") {
                let mut k = j;
                while k < n && matches!(chars[k], ' ' | '\t') {
                    k += 1;
                }
                if k < n && chars[k] == ':' && chars.get(k + 1) != Some(&'=') {
                    push(&mut out, Tok::Label(word), tok_line, sp);
                    push(&mut out, Tok::Eol, tok_line, false);
                    i = k + 1;
                    continue;
                }
            }
            push(&mut out, Tok::Word(word), tok_line, sp);
            i = j;
            start = false;
            continue;
        }
        let rest: String = chars[i..(i + 2).min(n)].iter().collect();
        let Some(sym) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) else {
            return Err(Error::syntax(module, line, format!("Unexpected character: {c}")));
        };
        push(&mut out, Tok::Sym(sym), tok_line, sp);
        i += sym.chars().count();
        start = false;
    }
    if !matches!(out.last().map(|t| &t.tok), Some(Tok::Eol) | None) {
        out.push(Token { tok: Tok::Eol, line });
    }
    out.push(Token { tok: Tok::Eof, line });
    Ok(split_closers(out))
}

/// `EndIf` as `End If` and `Next i, j` as `Next i: Next j`, in one pass
/// (the parser inserting them one at a time is quadratic).
fn split_closers(toks: Vec<Token>) -> Vec<Token> {
    let stmt_start = |t: Option<&Token>| matches!(t.map(|t| &t.tok), None | Some(Tok::Eol | Tok::Colon | Tok::Label(_)));
    let word = |t: &Token, w: &str| matches!(&t.tok, Tok::Word(x) if x.eq_ignore_ascii_case(w));
    let mut out: Vec<Token> = Vec::with_capacity(toks.len());
    let mut it = toks.into_iter().peekable();
    while let Some(t) = it.next() {
        let line = t.line;
        if word(&t, "endif") && stmt_start(out.last()) && it.peek().is_some_and(|n| matches!(n.tok, Tok::Eol | Tok::Colon | Tok::Eof)) {
            out.push(Token { tok: Tok::Word("End".into()), line });
            out.push(Token { tok: Tok::Word("If".into()), line });
            continue;
        }
        if t.tok == Tok::Sym(",") {
            let n = out.len();
            if n >= 2 && word(&out[n - 2], "next") && matches!(out[n - 1].tok, Tok::Word(_)) && stmt_start(n.checked_sub(3).map(|k| &out[k])) {
                out.push(Token { tok: Tok::Colon, line });
                out.push(Token { tok: Tok::Word("Next".into()), line });
                continue;
            }
        }
        out.push(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s, "M").unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn basics() {
        let w = |s: &str| Tok::Word(s.into());
        assert_eq!(toks("x = Left$(\"a\"\"b\", 2) ' note"), vec![w("x"), Tok::Sym("="), w("Left"), Tok::Sym("("), Tok::Str("a\"b".into()), Tok::Sym(","), Tok::Int(2), Tok::Sym(")"), Tok::Eol, Tok::Eof]);
        assert_eq!(toks("a = 1: Rem hi\nb = &HFF"), vec![w("a"), Tok::Sym("="), Tok::Int(1), Tok::Eol, w("b"), Tok::Sym("="), Tok::Int(255), Tok::Eol, Tok::Eof]);
        assert_eq!(toks("Fin: x\n10 y"), vec![Tok::Label("Fin".into()), Tok::Eol, w("x"), Tok::Eol, Tok::Label("10".into()), Tok::Eol, w("y"), Tok::Eol, Tok::Eof]);
        assert_eq!(toks("x = 1 + _\n  2"), vec![w("x"), Tok::Sym("="), Tok::Int(1), Tok::Sym("+"), Tok::Int(2), Tok::Eol, Tok::Eof]);
        assert_eq!(toks("d = #1/15/2024#")[2], Tok::Date(45306.0));
        assert_eq!(toks("y = 1.5E3 + .5")[2], Tok::Num(1500.0));
        assert_eq!(toks("y = 1.5E3 + .5")[4], Tok::Num(0.5));
        assert_eq!(toks("r.Value")[1], Tok::Sym("."));
        assert_eq!(toks("f a:=1")[2], Tok::Sym(":="));
    }

    #[test]
    fn lines() {
        let t = tokenize("a = 1\n\n' c\nb = 2 _\n + 3\nc = 4", "M").unwrap();
        let line_of = |name: &str| t.iter().find(|x| x.tok == Tok::Word(name.into())).map(|x| x.line);
        assert_eq!(line_of("a"), Some(1));
        assert_eq!(line_of("b"), Some(4));
        assert_eq!(line_of("c"), Some(6));
    }
}
