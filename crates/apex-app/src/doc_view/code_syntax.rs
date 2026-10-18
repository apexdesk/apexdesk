//! Colors for source code in the code view, the same for every language:
//! comments, strings, numbers and common keywords. Deliberately small (no
//! grammar per language): how a language writes comments comes from the
//! file's extension.

use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};

/// How a language writes comments and strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Lang {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    /// `'x'` is a string (not in Rust, where it starts lifetimes too, nor
    /// in markup, where it is an apostrophe).
    apos: bool,
}

impl Lang {
    /// The language of a file extension (lowercase); C-like by default.
    pub(super) fn of(ext: &str) -> Lang {
        const C: Option<(&str, &str)> = Some(("/*", "*/"));
        let (line, block, apos): (&'static [&'static str], _, _) = match ext {
            "rs" => (&["//"], C, false),
            "py" | "sh" | "bash" | "zsh" | "fish" | "rb" | "pl" | "r" | "yaml" | "yml" | "toml" | "conf" | "cfg" | "properties" | "env" | "cmake" | "ps1" => (&["#"], None, true),
            "ini" => (&[";", "#"], None, true),
            "sql" => (&["--"], C, true),
            "lua" => (&["--"], None, true),
            "html" | "htm" | "xml" | "vue" => (&[], Some(("<!--", "-->")), false),
            "bat" | "cmd" => (&["REM ", "rem ", "::"], None, false),
            "diff" | "patch" => (&[], None, false),
            _ => (&["//"], C, true),
        };
        Lang { line, block, apos }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Plain,
    Comment,
    Str,
    Num,
    Key,
}

/// Keywords common to many languages.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "def", "default", "do", "elif", "else", "enum", "except", "export", "extends", "false", "finally", "fn", "for",
    "from", "func", "function", "if", "impl", "import", "in", "interface", "let", "loop", "match", "mod", "mut", "new", "nil", "None", "null", "package", "private", "pub", "public", "return",
    "self", "static", "struct", "switch", "this", "throw", "trait", "True", "False", "true", "try", "type", "use", "var", "void", "where", "while", "with", "yield",
];

/// The tokens of `text` as (byte range end, kind), neighbours of one kind
/// joined.
fn tokens(text: &str, lang: Lang) -> Vec<(usize, Tok)> {
    let mut out: Vec<(usize, Tok)> = Vec::new();
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        let rest = &text[i..];
        let word = |ok: fn(char) -> bool| rest.find(|ch: char| !ok(ch)).unwrap_or(rest.len());
        let (n, t) = if lang.line.iter().any(|p| rest.starts_with(p)) {
            (rest.find('\n').unwrap_or(rest.len()), Tok::Comment)
        } else if let Some((a, b)) = lang.block.filter(|(a, _)| rest.starts_with(a)) {
            (rest[a.len()..].find(b).map_or(rest.len(), |k| a.len() + k + b.len()), Tok::Comment)
        } else if c == '"' || c == '`' || (c == '\'' && lang.apos) {
            (string(rest, c), Tok::Str)
        } else if c.is_ascii_digit() {
            (word(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_'), Tok::Num)
        } else if c.is_alphabetic() || c == '_' {
            let n = word(|ch| ch.is_alphanumeric() || ch == '_');
            (n, if KEYWORDS.contains(&&rest[..n]) { Tok::Key } else { Tok::Plain })
        } else {
            (c.len_utf8(), Tok::Plain)
        };
        i += n;
        match out.last_mut() {
            Some(last) if last.1 == t => last.0 = i,
            _ => out.push((i, t)),
        }
    }
    out
}

/// The string at the start of `s`, quoted with `q` (backslash escapes):
/// its length; an unclosed one ends with its line.
fn string(s: &str, q: char) -> usize {
    let mut esc = false;
    for (k, ch) in s.char_indices().skip(1) {
        if ch == '\n' {
            return k;
        } else if esc {
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == q {
            return k + ch.len_utf8();
        }
    }
    s.len()
}

/// The code laid out with its colors (none for plain text: `lang` None),
/// without wrapping, so line numbers line up.
pub(super) fn job(text: &str, lang: Option<Lang>, font: FontId, dark: bool, fg: Color32) -> LayoutJob {
    let rgb = |l: (u8, u8, u8), d: (u8, u8, u8)| {
        let (r, g, b) = if dark { d } else { l };
        Color32::from_rgb(r, g, b)
    };
    let format = |t: Tok| {
        let mut f = TextFormat { font_id: font.clone(), color: fg, ..Default::default() };
        match t {
            Tok::Plain => {}
            Tok::Comment => {
                f.color = rgb((0x5F, 0x7F, 0x5F), (0x7F, 0xA0, 0x7F));
                f.italics = true;
            }
            Tok::Str => f.color = rgb((0xA3, 0x40, 0x15), (0xCE, 0x91, 0x78)),
            Tok::Num => f.color = rgb((0x09, 0x86, 0x58), (0xB5, 0xCE, 0xA8)),
            Tok::Key => f.color = rgb((0x1F, 0x4E, 0xA8), (0x6C, 0xB0, 0xF0)),
        }
        f
    };
    let mut job = LayoutJob::default();
    let Some(lang) = lang else {
        job.append(text, 0.0, format(Tok::Plain));
        return job;
    };
    let mut at = 0;
    for (end, t) in tokens(text, lang) {
        job.append(&text[at..end], 0.0, format(t));
        at = end;
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans<'t>(text: &'t str, ext: &str) -> Vec<(&'t str, Tok)> {
        let mut at = 0;
        tokens(text, Lang::of(ext))
            .into_iter()
            .map(|(end, t)| {
                let s = &text[at..end];
                at = end;
                (s, t)
            })
            .collect()
    }

    #[test]
    fn colors() {
        let s = spans("fn f<'a>(x: &'a str) -> u8 { // ñ \"no\"\n    let s = \"a\\\"b\"; /* 2\n */ 0x1F }\n", "rs");
        assert!(s.contains(&("fn", Tok::Key)) && s.contains(&("let", Tok::Key)));
        assert!(s.contains(&("// ñ \"no\"", Tok::Comment)));
        assert!(s.contains(&("\"a\\\"b\"", Tok::Str)));
        assert!(s.contains(&("/* 2\n */", Tok::Comment)));
        assert!(s.contains(&("0x1F", Tok::Num)));
        assert!(!s.iter().any(|x| x.1 == Tok::Str && x.0.starts_with('\'')), "lifetimes aren't strings");
        let p = spans("# hola\nx = 'dos' if y else 3.5\n", "py");
        assert_eq!(p[0], ("# hola", Tok::Comment));
        assert!(p.contains(&("'dos'", Tok::Str)) && p.contains(&("3.5", Tok::Num)) && p.contains(&("else", Tok::Key)));
        let h = spans("<!-- c -->\n<p>it's</p>\n", "html");
        assert_eq!(h[0], ("<!-- c -->", Tok::Comment));
        assert!(h.iter().all(|x| x.1 != Tok::Str));
        // Unclosed strings stop at the line's end.
        assert!(spans("\"abc\nfor\n", "js").contains(&("for", Tok::Key)));
    }
}
