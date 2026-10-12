//! Several edits as one undo step: a command made of smaller ones (a
//! citation style that also rebuilds the bibliographies) undoes at once.

use crate::{Bibliography, Document, EditKind, Paragraph, Selection};

/// Where a group of edits starts (see [`Document::undo_mark`]).
pub struct UndoMark {
    len: usize,
    paragraphs: Vec<Paragraph>,
}

impl Document {
    /// Marks the start of a group of edits that [`Document::join_undo`]
    /// makes one undo step.
    pub fn undo_mark(&mut self) -> UndoMark {
        // The first edit of the group mustn't merge into the step before.
        self.seal_undo();
        UndoMark { len: self.undo.len(), paragraphs: self.paragraphs.clone() }
    }

    /// Joins the undo steps recorded since `mark` into one.
    pub fn join_undo(&mut self, mark: UndoMark) {
        if self.undo.len() <= mark.len + 1 {
            return;
        }
        let steps: Vec<_> = self.undo.drain(mark.len..).collect();
        let (first, last) = (&steps[0], &steps[steps.len() - 1]);
        let (sel_before, sel_after, kind) = (first.sel_before, last.sel_after, first.kind);
        // What each step saved is the state before it: the earliest one is
        // the state before the group.
        let mut e = crate::Edit { start: 0, before: mark.paragraphs, after_len: self.paragraphs.len(), sel_before, sel_after, kind, merge_closed: true, styles: None, comments: None, page: None, parts: None };
        for s in steps {
            e.styles = e.styles.or(s.styles);
            e.comments = e.comments.or(s.comments);
            e.page = e.page.or(s.page);
            e.parts = e.parts.or(s.parts);
        }
        self.undo.push(e);
    }

    /// Replaces the sources and the citation style, as an undo step.
    pub fn set_biblio(&mut self, b: Bibliography, sel: Selection) {
        self.change_layout(None, sel, EditKind::Paragraph, &mut |d| d.biblio = b.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CiteStyle, Pos};

    #[test]
    fn a_group_undoes_at_once() {
        let mut d = Document::from_plain_text("one\ntwo");
        let sel = Selection::caret(Pos::new(0, 0));
        let before = (d.to_plain_text(), d.biblio.clone());
        let mark = d.undo_mark();
        let style = CiteStyle::ALL.into_iter().find(|s| *s != d.biblio.style).unwrap();
        d.set_biblio(Bibliography { style, ..d.biblio.clone() }, sel);
        d.insert_text(Selection::caret(Pos::new(1, 3)), "\nthree", None);
        d.join_undo(mark);
        assert_eq!(d.biblio.style, style);
        d.undo();
        assert_eq!((d.to_plain_text(), d.biblio.clone()), before);
        d.redo();
        assert_eq!((d.to_plain_text(), d.biblio.style), ("one\ntwo\nthree".to_string(), style));
    }
}
