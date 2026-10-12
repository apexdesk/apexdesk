//! VBA syntax colors for the macro editor: keywords, strings, comments
//! (with `Rem`), numbers and the `Attribute` lines Office hides.

use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tok {
    Plain,
    Keyword,
    Str,
    Comment,
    Number,
    /// `Attribute …` lines.
    Hidden,
}

const KEYWORDS: &[&str] = &[
    "alias", "and", "any", "append", "as", "attribute", "base", "binary", "boolean", "byref", "byte", "byval", "call", "case", "close", "compare", "const", "currency", "date", "debug", "declare", "defbool", "defint", "deflng", "defstr", "defvar", "dim", "do", "double", "each", "else", "elseif", "empty", "end", "enum", "eqv", "erase", "error", "event", "exit", "explicit", "false", "for", "friend", "function", "get", "global", "gosub", "goto", "if", "imp", "implements", "in", "input", "integer", "is", "let", "lib", "like", "line", "local", "lock", "long", "longlong", "longptr", "loop", "lset", "me", "mod", "new", "next", "not", "nothing", "null", "object", "on", "open", "option", "optional", "or", "output", "paramarray", "preserve", "print", "private", "property", "ptrsafe", "public", "put", "raiseevent", "random", "read", "redim", "resume", "return", "rset", "seek", "select", "set", "single", "static", "step", "stop", "string", "sub", "then", "to", "true", "type", "typeof", "until", "variant", "wend", "while", "with", "withevents", "write", "xor",
];

/// The tokens of `text` as (byte range end, kind), in order.
pub(super) fn tokens(text: &str) -> Vec<(usize, Tok)> {
    let mut out: Vec<(usize, Tok)> = Vec::new();
    let mut push = |end: usize, t: Tok| match out.last_mut() {
        Some(last) if last.1 == t => last.0 = end,
        _ => out.push((end, t)),
    };
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let b = line.as_bytes();
        let body = line.trim_end_matches(['\n', '\r']).len();
        if line.trim_start().starts_with("Attribute ") {
            push(start + line.len(), Tok::Hidden);
            start += line.len();
            continue;
        }
        // Whether only blanks (or a `:`) came before, for `Rem`.
        let mut statement_start = true;
        let mut i = 0;
        while i < body {
            let c = b[i];
            if c == b'"' {
                let mut j = i + 1;
                while j < body {
                    if b[j] == b'"' {
                        if j + 1 < body && b[j + 1] == b'"' {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                push(start + j.min(body), Tok::Str);
                i = j.min(body);
                statement_start = false;
            } else if c == b'\'' {
                push(start + body, Tok::Comment);
                i = body;
            } else if c.is_ascii_alphabetic() || c == b'_' {
                let mut j = i + 1;
                while j < body && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let word = line[i..j].to_ascii_lowercase();
                // A `.` before means a member (`.Name`), not a keyword.
                let member = i > 0 && b[i - 1] == b'.';
                if word == "rem" && statement_start {
                    push(start + body, Tok::Comment);
                    i = body;
                    continue;
                }
                let t = if !member && KEYWORDS.binary_search(&word.as_str()).is_ok() { Tok::Keyword } else { Tok::Plain };
                push(start + j, t);
                i = j;
                statement_start = false;
            } else if c.is_ascii_digit() {
                let mut j = i + 1;
                while j < body && (b[j].is_ascii_alphanumeric() || b[j] == b'.') {
                    j += 1;
                }
                push(start + j, Tok::Number);
                i = j;
                statement_start = false;
            } else {
                // Other characters (UTF-8 continuation bytes included).
                let len = line[i..].chars().next().map_or(1, char::len_utf8);
                if c == b':' {
                    statement_start = true;
                } else if !c.is_ascii_whitespace() {
                    statement_start = false;
                }
                push(start + i + len, Tok::Plain);
                i += len;
            }
        }
        push(start + line.len(), Tok::Plain);
        start += line.len();
    }
    out
}

/// Colors of the tokens, for a light or dark background.
fn color(t: Tok, dark: bool, text: Color32) -> Color32 {
    let rgb = |l: (u8, u8, u8), d: (u8, u8, u8)| {
        let (r, g, b) = if dark { d } else { l };
        Color32::from_rgb(r, g, b)
    };
    match t {
        Tok::Plain => text,
        Tok::Keyword => rgb((0x00, 0x00, 0xB0), (0x56, 0x9C, 0xD6)),
        Tok::Str => rgb((0xA3, 0x15, 0x15), (0xCE, 0x91, 0x78)),
        Tok::Comment => rgb((0x00, 0x80, 0x00), (0x6A, 0x99, 0x55)),
        Tok::Number => rgb((0x09, 0x86, 0x58), (0xB5, 0xCE, 0xA8)),
        Tok::Hidden => rgb((0x90, 0x90, 0x90), (0x80, 0x80, 0x80)),
    }
}

/// The text laid out with its colors (no wrapping: code keeps its lines).
pub(super) fn job(text: &str, font: FontId, dark: bool, fg: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut at = 0;
    for (end, t) in tokens(text) {
        if end > at {
            job.append(&text[at..end], 0.0, TextFormat { font_id: font.clone(), color: color(t, dark, fg), ..Default::default() });
            at = end;
        }
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, TextFormat { font_id: font, color: fg, ..Default::default() });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<(&str, Tok)> {
        let mut at = 0;
        tokens(text)
            .into_iter()
            .map(|(end, t)| {
                let s = &text[at..end];
                at = end;
                (s, t)
            })
            .collect()
    }

    #[test]
    fn keywords_are_sorted() {
        assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn colors() {
        let text = "Sub A() ' hi\n  x = \"it's \"\"ok\"\"\" & 12: Rem note\nAttribute A.VB_X = 1\nMsgBox Me.Name, año\n";
        let s = spans(text);
        assert_eq!(s[0], ("Sub", Tok::Keyword));
        assert!(s.contains(&("' hi", Tok::Comment)));
        assert!(s.contains(&("\"it's \"\"ok\"\"\"", Tok::Str)));
        assert!(s.contains(&("12", Tok::Number)));
        assert!(s.contains(&("Rem note", Tok::Comment)));
        assert!(s.contains(&("Attribute A.VB_X = 1\n", Tok::Hidden)));
        assert!(s.contains(&("Me", Tok::Keyword)));
        // `.Name` is a member, not the Name statement; non-ASCII stays plain.
        assert!(!s.iter().any(|(t, k)| *t == "Name" && *k == Tok::Keyword));
        assert_eq!(s.iter().map(|x| x.0).collect::<String>(), text);
    }
}
