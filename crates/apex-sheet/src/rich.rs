//! Mixed formatting inside one text cell: runs of text, each with the font
//! changes it makes to the cell's own format.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Script {
    #[default]
    Normal,
    Super,
    Sub,
}

/// What a run changes; `None` keeps the cell's format.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RunFont {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// 0xRRGGBB.
    pub color: Option<u32>,
    /// Points.
    pub size: Option<f32>,
    pub font: Option<Arc<str>>,
    pub script: Script,
}

impl RunFont {
    pub fn is_plain(&self) -> bool {
        *self == RunFont::default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RichRun {
    /// Length in bytes of the cell text.
    pub len: usize,
    pub font: RunFont,
}

/// The runs of a cell's text, covering it exactly.
pub type RichText = Vec<RichRun>;

/// Whether `runs` fit `text` (they cover it and split it at characters).
pub fn fits(runs: &[RichRun], text: &str) -> bool {
    let mut at = 0;
    for r in runs {
        at += r.len;
        if !text.is_char_boundary(at.min(text.len())) || at > text.len() {
            return false;
        }
    }
    at == text.len()
}

/// Joins neighbours with the same font and drops empty runs; `None` when
/// nothing is formatted.
pub fn normalize(runs: RichText) -> Option<RichText> {
    let mut out: RichText = Vec::with_capacity(runs.len());
    for r in runs {
        if r.len == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.font == r.font => last.len += r.len,
            _ => out.push(r),
        }
    }
    (!out.iter().all(|r| r.font.is_plain())).then_some(out)
}

/// The runs split so that `from..to` (bytes) is covered by whole runs.
fn split(runs: &[RichRun], total: usize, from: usize, to: usize) -> RichText {
    let mut runs: RichText = if runs.is_empty() { vec![RichRun { len: total, font: RunFont::default() }] } else { runs.to_vec() };
    for cut in [from, to] {
        let mut at = 0;
        for i in 0..runs.len() {
            let end = at + runs[i].len;
            if at < cut && cut < end {
                let tail = RichRun { len: end - cut, font: runs[i].font.clone() };
                runs[i].len = cut - at;
                runs.insert(i + 1, tail);
                break;
            }
            at = end;
        }
    }
    runs
}

/// Changes the font of `from..to` (bytes) of a text `total` bytes long.
pub fn apply(runs: &[RichRun], total: usize, from: usize, to: usize, f: impl Fn(&mut RunFont)) -> Option<RichText> {
    let mut runs = split(runs, total, from, to);
    let mut at = 0;
    for r in &mut runs {
        if at >= from && at + r.len <= to && r.len > 0 {
            f(&mut r.font);
        }
        at += r.len;
    }
    normalize(runs)
}

/// The font at byte `at` (the run it falls in, or the one before for the
/// end of the text).
pub fn font_at(runs: &[RichRun], at: usize) -> RunFont {
    let mut pos = 0;
    for r in runs {
        if at < pos + r.len {
            return r.font.clone();
        }
        pos += r.len;
    }
    runs.last().map(|r| r.font.clone()).unwrap_or_default()
}

/// The runs of `new` after an edit of `old`: unchanged ends keep their
/// format, typed text takes the format where it was typed.
pub fn follow_edit(runs: &[RichRun], old: &str, new: &str) -> Option<RichText> {
    if runs.is_empty() || old == new {
        return (!runs.is_empty()).then(|| runs.to_vec());
    }
    let prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    let prefix = (0..=prefix).rev().find(|&i| old.is_char_boundary(i) && new.is_char_boundary(i)).unwrap_or(0);
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let suffix = old.bytes().rev().zip(new.bytes().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    let suffix = (0..=suffix).rev().find(|&i| old.is_char_boundary(old.len() - i) && new.is_char_boundary(new.len() - i)).unwrap_or(0);
    let (del_from, del_to) = (prefix, old.len() - suffix);
    let inserted = new.len() - prefix - suffix;
    let typed_font = font_at(runs, if prefix > 0 { prefix - 1 } else { 0 });
    let mut out = RichText::new();
    let mut at = 0;
    let mut placed = inserted == 0;
    for r in runs {
        let (s, e) = (at, at + r.len);
        at = e;
        // The part before the change, the part after it.
        let keep_before = e.min(del_from).saturating_sub(s);
        let keep_after = e.saturating_sub(s.max(del_to));
        if keep_before > 0 {
            out.push(RichRun { len: keep_before, font: r.font.clone() });
        }
        if !placed && e >= del_from {
            out.push(RichRun { len: inserted, font: typed_font.clone() });
            placed = true;
        }
        if keep_after > 0 {
            out.push(RichRun { len: keep_after, font: r.font.clone() });
        }
    }
    if !placed {
        out.push(RichRun { len: inserted, font: typed_font });
    }
    normalize(out).filter(|r| fits(r, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> RunFont {
        RunFont { bold: Some(true), ..Default::default() }
    }

    #[test]
    fn apply_and_edit() {
        // "Hello world" with "world" bold.
        let r = apply(&[], 11, 6, 11, |f| f.bold = Some(true)).unwrap();
        assert_eq!(r, vec![RichRun { len: 6, font: RunFont::default() }, RichRun { len: 5, font: bold() }]);
        assert!(fits(&r, "Hello world"));
        // Unbolding it all leaves nothing.
        assert_eq!(apply(&r, 11, 0, 11, |f| f.bold = None), None);
        // Typing inside the bold part stays bold; before it, plain.
        let e = follow_edit(&r, "Hello world", "Hello wide world").unwrap();
        assert_eq!(e, vec![RichRun { len: 6, font: RunFont::default() }, RichRun { len: 10, font: bold() }]);
        let e = follow_edit(&r, "Hello world", "Hi world").unwrap();
        assert_eq!(e, vec![RichRun { len: 3, font: RunFont::default() }, RichRun { len: 5, font: bold() }]);
        // Deleting the bold part drops the formatting.
        assert_eq!(follow_edit(&r, "Hello world", "Hello "), None);
        assert_eq!(font_at(&r, 11), bold());
        assert!(!fits(&r, "Hello"));
    }
}
