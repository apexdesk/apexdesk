//! Markdown colors for the source view: headings, emphasis, code (inline
//! and fenced), links and pictures, and the marks of lists, quotes, rules
//! and tables.

use eframe::egui::{Color32, FontId, Stroke, TextFormat, text::LayoutJob};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tok {
    Plain,
    Heading,
    /// `**strong**`, `*emphasis*`, `~~strike~~`, marks included.
    Emph,
    Code,
    /// `[text](target)` and `![alt](source)`.
    Link,
    /// List bullets and numbers, `>`, rules, table pipes and delimiter rows.
    Mark,
}

/// The tokens of `text` as (byte range end, kind), in order; neighbours
/// of one kind are joined.
pub(super) fn tokens(text: &str) -> Vec<(usize, Tok)> {
    let mut out: Vec<(usize, Tok)> = Vec::new();
    let mut fence = false;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        let end = at + line.len();
        at = end;
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let ind = body.len() - trimmed.len();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
            push(&mut out, end, Tok::Code);
            continue;
        }
        if fence {
            push(&mut out, end, Tok::Code);
            continue;
        }
        if is_heading(trimmed) {
            push(&mut out, end, Tok::Heading);
            continue;
        }
        if is_rule(trimmed) || is_delimiter_row(trimmed) {
            push(&mut out, end, Tok::Mark);
            continue;
        }
        push(&mut out, start + ind, Tok::Plain);
        let mark = block_mark(trimmed);
        push(&mut out, start + ind + mark, Tok::Mark);
        let from = start + ind + mark;
        inline(&text[from..end], from, trimmed.starts_with('|'), &mut out);
    }
    out.retain(|t| t.0 > 0);
    out.dedup_by(|b, a| b.0 == a.0);
    out
}

fn push(out: &mut Vec<(usize, Tok)>, end: usize, t: Tok) {
    match out.last_mut() {
        Some(last) if last.0 >= end => {}
        Some(last) if last.1 == t => last.0 = end,
        _ => out.push((end, t)),
    }
}

/// `#` to `######` followed by a space (or nothing).
fn is_heading(s: &str) -> bool {
    let n = s.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&n) && s[n..].chars().next().is_none_or(char::is_whitespace)
}

/// `---`, `***` or `___` (spaces allowed between).
fn is_rule(s: &str) -> bool {
    let c: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    c.len() >= 3 && matches!(c[0], '-' | '*' | '_') && c.iter().all(|&x| x == c[0])
}

/// A table's `|---|:--:|` row.
fn is_delimiter_row(s: &str) -> bool {
    s.contains('|') && s.contains('-') && s.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// Length of the block marks at the start of a line: quotes (`> `), a
/// bullet (`- `, `* `, `+ `) or a number (`1. `, `1) `), and a task box.
fn block_mark(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] == b'>' {
        i += 1;
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
    }
    let spaced = |k: usize| k < b.len() && b[k] == b' ';
    if i < b.len() && matches!(b[i], b'-' | b'*' | b'+') && spaced(i + 1) {
        i += 2;
    } else {
        let d = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if d > 0 && d < 10 && i + d < b.len() && matches!(b[i + d], b'.' | b')') && spaced(i + d + 1) {
            i += d + 2;
        }
    }
    for task in ["[ ] ", "[x] ", "[X] "] {
        if i > 0 && s[i..].starts_with(task) {
            i += task.len();
        }
    }
    i
}

/// The inline tokens of `s` (which starts at byte `base` of the text).
fn inline(s: &str, base: usize, table: bool, out: &mut Vec<(usize, Tok)>) {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &s[i..];
        let span = match b[i] {
            b'\\' => {
                // An escaped character stays plain.
                let n = rest[1..].chars().next().map_or(0, char::len_utf8);
                Some((1 + n, Tok::Plain))
            }
            b'`' => {
                let n = rest.bytes().take_while(|&c| c == b'`').count();
                let fence = &rest[..n];
                rest[n..].find(fence).map(|k| (n + k + n, Tok::Code))
            }
            b'!' if rest.starts_with("![") => link(&rest[1..]).map(|n| (n + 1, Tok::Link)),
            b'[' => link(rest).map(|n| (n, Tok::Link)),
            b'*' | b'_' | b'~' => emphasis(rest).map(|n| (n, Tok::Emph)),
            b'|' if table => Some((1, Tok::Mark)),
            _ => None,
        };
        let (n, t) = span.unwrap_or_else(|| (rest.chars().next().map_or(1, char::len_utf8), Tok::Plain));
        i += n;
        push(out, base + i, t);
    }
}

/// `[text](target)` at the start of `s`: its length.
fn link(s: &str) -> Option<usize> {
    let line = s.split('\n').next().unwrap_or(s);
    let close = line.find("](")?;
    let end = close + line[close..].find(')')?;
    Some(end + 1)
}

/// Emphasis at the start of `s` (`**x**`, `*x*`, `__x__`, `_x_`, `~~x~~`):
/// its length, marks included.
fn emphasis(s: &str) -> Option<usize> {
    let c = s.as_bytes()[0];
    let n = s.bytes().take_while(|&b| b == c).count().min(3);
    if c == b'~' && n != 2 {
        return None;
    }
    let marks = &s[..n];
    let inner = &s[n..];
    // The text right after the marks can't be a space.
    if inner.chars().next().is_none_or(char::is_whitespace) {
        return None;
    }
    let line = inner.split('\n').next().unwrap_or(inner);
    let k = line.find(marks)?;
    (k > 0).then_some(n + k + n)
}

/// Colors of the tokens, for a light or dark background.
fn format(t: Tok, font: &FontId, dark: bool, text: Color32) -> TextFormat {
    let rgb = |l: (u8, u8, u8), d: (u8, u8, u8)| {
        let (r, g, b) = if dark { d } else { l };
        Color32::from_rgb(r, g, b)
    };
    let mut f = TextFormat { font_id: font.clone(), color: text, ..Default::default() };
    match t {
        Tok::Plain => {}
        Tok::Heading => f.color = rgb((0x1F, 0x4E, 0xA8), (0x6C, 0xB0, 0xF0)),
        Tok::Emph => {
            f.color = rgb((0x8B, 0x2C, 0x9E), (0xD0, 0x96, 0xE0));
            f.italics = true;
        }
        Tok::Code => {
            f.color = rgb((0xA3, 0x40, 0x15), (0xCE, 0x91, 0x78));
            f.background = rgb((0xF2, 0xEE, 0xE8), (0x33, 0x30, 0x2C));
        }
        Tok::Link => {
            f.color = rgb((0x00, 0x6A, 0x8E), (0x4E, 0xC9, 0xB0));
            f.underline = Stroke::new(1.0, f.color.gamma_multiply(0.5));
        }
        Tok::Mark => f.color = rgb((0xB0, 0x60, 0x00), (0xE0, 0xA0, 0x50)),
    }
    f
}

/// The text laid out with its colors (no wrapping: lines stay lines, so
/// their numbers line up).
pub(super) fn job(text: &str, font: FontId, dark: bool, fg: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut at = 0;
    for (end, t) in tokens(text) {
        if end > at {
            job.append(&text[at..end], 0.0, format(t, &font, dark, fg));
            at = end;
        }
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, format(Tok::Plain, &font, dark, fg));
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
    fn colors() {
        let text = "# Título\n\nUn **fuerte** y *leve*, `código` y [enlace](http://x.y).\n- uno\n12. dos\n> cita\n\n| a | b |\n|---|:-:|\n| ñ | 2 |\n\n```rust\nlet x = *y*;\n```\n---\n\\*no\\* _sí_ ~~tachado~~\n";
        let s = spans(text);
        assert_eq!(s.iter().map(|x| x.0).collect::<String>(), text);
        assert_eq!(s[0], ("# Título\n", Tok::Heading));
        assert!(s.contains(&("**fuerte**", Tok::Emph)));
        assert!(s.contains(&("*leve*", Tok::Emph)));
        assert!(s.contains(&("`código`", Tok::Code)));
        assert!(s.contains(&("[enlace](http://x.y)", Tok::Link)));
        assert!(s.contains(&("- ", Tok::Mark)));
        assert!(s.contains(&("12. ", Tok::Mark)));
        assert!(s.contains(&("> ", Tok::Mark)));
        assert!(s.iter().any(|(t, k)| t.starts_with("|---|:-:|\n") && *k == Tok::Mark));
        assert!(s.contains(&("```rust\nlet x = *y*;\n```\n", Tok::Code)));
        assert!(s.contains(&("_sí_", Tok::Emph)));
        assert!(s.contains(&("~~tachado~~", Tok::Emph)));
        // Escaped stars and a lone star aren't emphasis.
        assert!(!s.iter().any(|(t, k)| t.contains("no") && *k == Tok::Emph));
        assert!(spans("2 * 3 = 6\n").iter().all(|x| x.1 == Tok::Plain));
    }
}
