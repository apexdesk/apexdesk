//! Hyphenation: optional hyphens (U+00AD) placed at syllable breaks.
//!
//! The rules are Spanish syllabification, which also gives acceptable
//! breaks for most other Latin-script words: a single consonant goes with
//! the next vowel, two consonants split unless they form an inseparable
//! cluster (pr, bl, ch, ll, rr…), and three or more leave the last one or
//! the cluster for the next syllable.

use crate::{Document, EditKind, Pos, SOFT_HYPHEN, Selection};

/// Words shorter than this are never hyphenated.
pub const MIN_WORD: usize = 7;
/// Letters kept at least before and after a break.
const MIN_HEAD: usize = 3;
const MIN_TAIL: usize = 3;

fn vowel(c: char) -> bool {
    matches!(c.to_lowercase().next().unwrap_or(c), 'a' | 'e' | 'i' | 'o' | 'u' | 'y' | 'á' | 'é' | 'í' | 'ó' | 'ú' | 'ü' | 'à' | 'è' | 'ì' | 'ò' | 'ù' | 'â' | 'ê' | 'î' | 'ô' | 'û' | 'ä' | 'ë' | 'ï' | 'ö')
}

/// Two consonants that start a syllable together.
fn cluster(a: char, b: char) -> bool {
    let a = a.to_lowercase().next().unwrap_or(a);
    let b = b.to_lowercase().next().unwrap_or(b);
    matches!((a, b), ('p' | 'b' | 'f' | 't' | 'd' | 'c' | 'k' | 'g', 'r') | ('p' | 'b' | 'f' | 'c' | 'k' | 'g', 'l') | ('c' | 's' | 't' | 'p', 'h') | ('l', 'l') | ('r', 'r') | ('q', 'u'))
}

/// Where a word may break: char indices a hyphen goes before.
pub fn hyphen_points(word: &str) -> Vec<usize> {
    let cs: Vec<char> = word.chars().collect();
    let n = cs.len();
    if n < MIN_WORD || !cs.iter().all(|c| c.is_alphabetic()) || !cs.iter().any(|&c| vowel(c)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    // From one vowel group to the next.
    while i < n {
        while i < n && !vowel(cs[i]) {
            i += 1;
        }
        while i < n && vowel(cs[i]) {
            i += 1;
        }
        let start = i;
        while i < n && !vowel(cs[i]) {
            i += 1;
        }
        if i >= n {
            break;
        }
        let k = i - start;
        let at = match k {
            0 => continue,
            1 => start,
            2 if cluster(cs[start], cs[start + 1]) => start,
            2 => start + 1,
            _ if cluster(cs[i - 2], cs[i - 1]) => i - 2,
            _ => i - 1,
        };
        if at >= MIN_HEAD && n - at >= MIN_TAIL {
            out.push(at);
        }
    }
    out
}

/// A word with its optional hyphens.
pub fn hyphenate_word(word: &str) -> String {
    let pts = hyphen_points(word);
    let mut out = String::with_capacity(word.len() + pts.len() * 2);
    for (i, c) in word.chars().enumerate() {
        if pts.contains(&i) {
            out.push(SOFT_HYPHEN);
        }
        out.push(c);
    }
    out
}

/// The text without optional hyphens (for spelling, find, word counts…).
pub fn strip_soft_hyphens(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(SOFT_HYPHEN) { s.replace(SOFT_HYPHEN, "").into() } else { s.into() }
}

impl Document {
    /// Puts optional hyphens in the long words of the document that have
    /// none yet (one undo step). Returns how many were added.
    pub fn hyphenate(&mut self, sel: Selection) -> usize {
        let mut plan: Vec<(usize, Vec<usize>)> = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if p.style.toc.is_some() {
                continue;
            }
            let mut at = Vec::new();
            let mut word_start = None;
            let text = &p.text;
            for (b, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
                if c.is_alphabetic() || c == SOFT_HYPHEN {
                    word_start.get_or_insert(b);
                    continue;
                }
                if let Some(s) = word_start.take() {
                    let w = &text[s..b];
                    if !w.contains(SOFT_HYPHEN) {
                        let offs: Vec<usize> = w.char_indices().map(|(o, _)| s + o).collect();
                        at.extend(hyphen_points(w).into_iter().map(|k| offs[k]));
                    }
                }
            }
            if !at.is_empty() {
                plan.push((i, at));
            }
        }
        let (Some(first), Some(last)) = (plan.first().map(|x| x.0), plan.last().map(|x| x.0)) else { return 0 };
        let before = self.paragraphs[first..=last].to_vec();
        let shift = |p: Pos, plan: &[(usize, Vec<usize>)]| {
            let extra = plan.iter().find(|x| x.0 == p.para).map_or(0, |x| x.1.iter().filter(|&&o| o < p.offset).count());
            Pos::new(p.para, p.offset + extra * SOFT_HYPHEN.len_utf8())
        };
        let after = Selection { anchor: shift(sel.anchor, &plan), cursor: shift(sel.cursor, &plan) };
        let mut n = 0;
        for (i, at) in &plan {
            let p = &mut self.paragraphs[*i];
            for &o in at.iter().rev() {
                let attrs = p.attrs_at(o + 1);
                p.insert_str(o, &SOFT_HYPHEN.to_string(), attrs);
                n += 1;
            }
        }
        self.record(first, before, last - first + 1, sel, after, EditKind::Insert);
        n
    }

    /// Turns automatic hyphenation on or off, as one undo step with the
    /// optional hyphens it puts in (on: the long words that have none
    /// yet, also when it already was on) or takes out (`strip`).
    pub fn set_auto_hyphenation(&mut self, on: bool, strip: bool, sel: Selection) -> (usize, Selection) {
        let state = self.page_state();
        let was = self.auto_hyphenation;
        let rev = self.revision;
        let n = if on { self.hyphenate(sel) } else if strip { self.remove_soft_hyphens(sel) } else { 0 };
        let edited = self.revision != rev;
        let after = if edited { self.undo.last().map_or(sel, |e| e.sel_after) } else { sel };
        if edited {
            self.auto_hyphenation = on;
            if let Some(e) = self.undo.last_mut() {
                e.page = Some(Box::new(state));
            }
        } else if was != on {
            self.change_layout(None, sel, EditKind::Paragraph, &mut |d| d.auto_hyphenation = on);
        }
        (n, after)
    }

    /// Takes every optional hyphen out (one undo step).
    pub fn remove_soft_hyphens(&mut self, sel: Selection) -> usize {
        let paras: Vec<usize> = (0..self.paragraphs.len()).filter(|&i| self.paragraphs[i].text.contains(SOFT_HYPHEN)).collect();
        let (Some(&first), Some(&last)) = (paras.first(), paras.last()) else { return 0 };
        let before = self.paragraphs[first..=last].to_vec();
        let shift = |p: Pos, paras: &[crate::Paragraph]| {
            let Some(q) = p.para.checked_sub(first).and_then(|k| paras.get(k)) else { return p };
            let t = &q.text[..p.offset.min(q.text.len())];
            Pos::new(p.para, p.offset - t.matches(SOFT_HYPHEN).count() * SOFT_HYPHEN.len_utf8())
        };
        let after = Selection { anchor: shift(sel.anchor, &before), cursor: shift(sel.cursor, &before) };
        let mut n = 0;
        for &i in &paras {
            let p = &mut self.paragraphs[i];
            let at: Vec<usize> = p.text.match_indices(SOFT_HYPHEN).map(|(o, _)| o).collect();
            for &o in at.iter().rev() {
                p.remove(o, o + SOFT_HYPHEN.len_utf8());
                n += 1;
            }
        }
        self.record(first, before, last - first + 1, sel, after, EditKind::Delete);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(w: &str) -> String {
        hyphenate_word(w).replace(SOFT_HYPHEN, "-")
    }

    // Turning it on, with the hyphens it puts in, is one undo step; so is
    // turning it off.
    #[test]
    fn automatic_hyphenation_undoes() {
        let text = "La biblioteca municipal tiene extraordinariamente muchos libros";
        let mut d = Document::from_paragraphs(vec![crate::Paragraph::new(text)]);
        let sel = Selection::caret(Pos::new(0, 0));
        let (n, _) = d.set_auto_hyphenation(true, false, sel);
        assert!(n > 0 && d.auto_hyphenation);
        d.undo();
        assert!(!d.auto_hyphenation);
        assert_eq!(d.paragraphs[0].text, text);
        d.redo();
        assert!(d.auto_hyphenation && d.paragraphs[0].text != text);
        // Off, keeping the hyphens: a step of its own.
        d.set_auto_hyphenation(false, false, sel);
        assert!(!d.auto_hyphenation);
        d.undo();
        assert!(d.auto_hyphenation);
        // Nothing to hyphenate: still a step.
        let mut d = Document::from_paragraphs(vec![crate::Paragraph::new("a b")]);
        d.set_auto_hyphenation(true, false, sel);
        assert!(d.auto_hyphenation);
        d.undo();
        assert!(!d.auto_hyphenation);
    }

    #[test]
    fn syllables() {
        assert_eq!(show("biblioteca"), "biblio-teca");
        assert_eq!(show("construcción"), "cons-truc-ción");
        assert_eq!(show("ferrocarril"), "ferro-ca-rril");
        assert_eq!(show("hyphenation"), "hyphe-na-tion");
        assert_eq!(show("casa"), "casa");
        assert_eq!(show("abc1234"), "abc1234");
    }

    #[test]
    fn hyphenate_and_remove() {
        let mut d = Document::from_plain_text("La biblioteca municipal abre");
        let caret = Selection::caret(Pos::new(0, 28));
        assert_eq!(d.hyphenate(caret), 3);
        assert_eq!(d.paragraphs[0].text.replace(SOFT_HYPHEN, "-"), "La biblio-teca muni-ci-pal abre");
        // Again: words with hyphens are left alone.
        assert_eq!(d.hyphenate(caret), 0);
        assert_eq!(d.remove_soft_hyphens(caret), 3);
        assert_eq!(d.paragraphs[0].text, "La biblioteca municipal abre");
        d.undo();
        assert!(d.paragraphs[0].text.contains(SOFT_HYPHEN));
    }
}
