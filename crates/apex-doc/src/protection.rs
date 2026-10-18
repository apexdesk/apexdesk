//! Restrict Editing (Word's document protection): what may still be
//! changed, and the password hash that stops the protection. Like sheet
//! protection it isn't encryption: it guards changes made in the editor.

use crate::{EditKind, Mark, Paragraph, Pos, Selection};

/// What a protected document still allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditLimit {
    /// Nothing can change.
    #[default]
    ReadOnly,
    /// Only comments can be added.
    Comments,
    /// Changes are allowed but always tracked.
    TrackedChanges,
    /// Only form fields can be filled in.
    Forms,
}

impl EditLimit {
    /// The `w:edit` value.
    pub fn docx(self) -> &'static str {
        match self {
            EditLimit::ReadOnly => "readOnly",
            EditLimit::Comments => "comments",
            EditLimit::TrackedChanges => "trackedChanges",
            EditLimit::Forms => "forms",
        }
    }

    pub fn from_docx(v: &str) -> Option<EditLimit> {
        Some(match v {
            "readOnly" => EditLimit::ReadOnly,
            "comments" => EditLimit::Comments,
            "trackedChanges" => EditLimit::TrackedChanges,
            "forms" => EditLimit::Forms,
            _ => return None,
        })
    }
}

/// A stored protection password: a salted, iterated hash (base64), as
/// Word writes with `w:algorithmName`. `algorithm` is empty for Word's
/// older hashes, which are kept for the file but can't be checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectionHash {
    pub algorithm: String,
    pub hash: String,
    pub salt: String,
    pub spin: u32,
    /// The attributes of an older hash, written back unchanged.
    pub legacy: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Protection {
    pub limit: EditLimit,
    /// Whether the limits apply (Word keeps unenforced settings too).
    pub enforced: bool,
    pub password: Option<ProtectionHash>,
}

impl crate::Document {
    /// The limit that applies now, if the document is protected.
    pub fn edit_limit(&self) -> Option<EditLimit> {
        self.protection.as_ref().filter(|p| p.enforced).map(|p| p.limit)
    }
}

impl crate::Document {
    /// Number of undo steps (edits made since a moment show as more).
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Kind of undo step `i` (0 = oldest).
    pub fn undo_kind(&self, i: usize) -> Option<crate::EditKind> {
        self.undo.get(i).map(|e| e.kind)
    }

    /// Kind of the step Redo would apply.
    pub fn redo_kind(&self) -> Option<crate::EditKind> {
        self.redo.last().map(|e| e.kind)
    }

    /// Keeps only the last `keep` undo steps (a macro's copy, editing
    /// many times, doesn't hold a copy of the text for each edit).
    pub fn trim_undo(&mut self, keep: usize) {
        if self.undo.len() > keep {
            let n = self.undo.len() - keep;
            self.undo.drain(..n);
        }
    }

    /// Keeps the next edit from merging into the last undo step.
    pub fn seal_undo(&mut self) {
        if let Some(e) = self.undo.last_mut() {
            e.merge_closed = true;
        }
    }
}

/// Mark names of editable ranges (Word's "exceptions", `w:permStart` and
/// `w:permEnd`) start with this: a character no bookmark name can have.
const PERM: char = '\u{2}';

/// Who may edit a range the user makes: everyone.
pub const EVERYONE: &str = "edGrp=everyone";

/// The mark name of editable range `id`; `who` is the file's editor
/// (`edGrp=everyone`, `ed=name`), kept for saving.
pub fn perm_mark(id: u32, who: &str) -> std::sync::Arc<str> {
    std::sync::Arc::from(format!("{PERM}{id}{PERM}{who}").as_str())
}

pub fn is_perm_mark(name: &str) -> bool {
    name.starts_with(PERM)
}

/// The id and editor of an editable range's mark.
pub fn perm_parts(name: &str) -> Option<(u32, &str)> {
    let (id, who) = name.strip_prefix(PERM)?.split_once(PERM)?;
    Some((id.parse().ok()?, who))
}

impl crate::Document {
    /// The ranges a read-only protection still lets anyone edit, in
    /// document order. Anyone: this editor has no users or groups.
    pub fn editable_ranges(&self) -> Vec<(Pos, Pos)> {
        let mut out: Vec<(&str, Option<Pos>, Option<Pos>)> = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            for m in p.marks.iter().filter(|m| is_perm_mark(&m.name)) {
                let k = match out.iter().position(|r| r.0 == &*m.name) {
                    Some(k) => k,
                    None => {
                        out.push((&m.name, None, None));
                        out.len() - 1
                    }
                };
                let slot = if m.end { &mut out[k].2 } else { &mut out[k].1 };
                slot.get_or_insert(Pos::new(i, m.offset));
            }
        }
        let mut v: Vec<(Pos, Pos)> = out.into_iter().filter_map(|(_, s, e)| Some((s.or(e)?, e.or(s)?))).map(|(s, e)| (s.min(e), s.max(e))).collect();
        v.sort();
        v
    }

    /// Makes the selection an editable range. Undoable. False if it's empty.
    pub fn add_editable_range(&mut self, sel: Selection) -> bool {
        let (a, b) = (self.clamp(sel.start()), self.clamp(sel.end()));
        if a == b {
            return false;
        }
        let id = self.paragraphs.iter().flat_map(|p| &p.marks).filter_map(|m| perm_parts(&m.name)).map(|(id, _)| id + 1).max().unwrap_or(1);
        let before = self.paragraphs[a.para..=b.para].to_vec();
        let name = perm_mark(id, EVERYONE);
        self.paragraphs[a.para].marks.push(Mark { name: name.clone(), offset: a.offset, end: false });
        self.paragraphs[b.para].marks.push(Mark { name, offset: b.offset, end: true });
        self.record(a.para, before, b.para - a.para + 1, sel, sel, EditKind::Bookmark);
        true
    }

    /// Removes every editable range (not its text). Undoable.
    pub fn remove_editable_ranges(&mut self, sel: Selection) -> bool {
        let has = |p: &Paragraph| p.marks.iter().any(|m| is_perm_mark(&m.name));
        let (Some(lo), Some(hi)) = (self.paragraphs.iter().position(has), self.paragraphs.iter().rposition(has)) else { return false };
        let before = self.paragraphs[lo..=hi].to_vec();
        for p in &mut self.paragraphs[lo..=hi] {
            p.marks.retain(|m| !is_perm_mark(&m.name));
        }
        self.record(lo, before, hi - lo + 1, sel, sel, EditKind::Bookmark);
        true
    }

    /// A fingerprint of everything outside the editable ranges: text,
    /// formatting, paragraph breaks and settings. Edits inside leave it be.
    pub fn locked_skeleton(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let ranges = self.editable_ranges();
        let inside = |a: Pos, b: Pos| ranges.iter().any(|&(s, e)| s <= a && b <= e);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            let len = p.text.len();
            // The break before the paragraph, then its settings.
            if i == 0 || !inside(Pos::new(i - 1, self.paragraphs[i - 1].text.len()), Pos::new(i, 0)) {
                0xB7u8.hash(&mut h);
            }
            if !inside(Pos::new(i, 0), Pos::new(i, len)) {
                format!("{:?}{:?}{:?}", p.style, p.cell, p.section).hash(&mut h);
            }
            // The ranges' parts in this paragraph are holes in its text.
            let holes: Vec<(usize, usize)> = ranges
                .iter()
                .filter(|(s, e)| s.para <= i && i <= e.para)
                .map(|(s, e)| (if s.para < i { 0 } else { s.offset }, if e.para > i { len } else { e.offset }))
                .collect();
            for (s, e, a) in p.spans() {
                let mut at = s;
                for &(hs, he) in &holes {
                    if he < at || hs > e {
                        continue;
                    }
                    if hs > at {
                        p.text[at..hs].hash(&mut h);
                        a.hash(&mut h);
                    }
                    at = at.max(he);
                }
                if at < e {
                    p.text[at..e].hash(&mut h);
                    a.hash(&mut h);
                }
            }
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn sel(a: (usize, usize), b: (usize, usize)) -> Selection {
        Selection { anchor: Pos::new(a.0, a.1), cursor: Pos::new(b.0, b.1) }
    }

    #[test]
    fn editable_ranges_follow_edits() {
        let mut d = Document::from_plain_text("Nombre: Ana\nFirma");
        assert!(d.add_editable_range(sel((0, 8), (0, 11))));
        assert!(d.bookmarks().is_empty());
        d.insert_text(Selection::caret(Pos::new(0, 0)), "El ", None);
        assert_eq!(d.editable_ranges(), vec![(Pos::new(0, 11), Pos::new(0, 14))]);
        // Typing at either end goes inside; so does a new paragraph.
        d.insert_text(Selection::caret(Pos::new(0, 14)), " Pérez", None);
        d.insert_text(Selection::caret(Pos::new(0, 11)), "Sra. ", None);
        assert_eq!(d.editable_ranges(), vec![(Pos::new(0, 11), Pos::new(0, 26))]);
        d.insert_text(Selection::caret(Pos::new(0, 26)), "\nLuis", None);
        assert_eq!(d.editable_ranges(), vec![(Pos::new(0, 11), Pos::new(1, 4))]);
        d.undo();
        assert!(d.remove_editable_ranges(Selection::caret(Pos::new(0, 0))));
        assert!(d.editable_ranges().is_empty());
        d.undo();
        assert_eq!(d.editable_ranges().len(), 1);
    }

    #[test]
    fn skeleton_ignores_edits_inside() {
        let mut d = Document::from_plain_text("Nombre: Ana\nFirma");
        d.add_editable_range(sel((0, 8), (0, 11)));
        let before = d.locked_skeleton();
        d.insert_text(sel((0, 8), (0, 11)), "Luisa\nMaría", None);
        d.toggle(sel((0, 8), (1, 2)), |a| a.bold, |a, v| a.bold = v);
        d.insert_text(Selection::caret(Pos::new(1, 5)), "!", None);
        assert_eq!(d.locked_skeleton(), before);
        d.delete(sel((1, 0), (2, 1)));
        assert_ne!(d.locked_skeleton(), before);
        d.undo();
        assert_eq!(d.locked_skeleton(), before);
        d.insert_text(Selection::caret(Pos::new(2, 0)), "x", None);
        assert_ne!(d.locked_skeleton(), before);
    }
}
