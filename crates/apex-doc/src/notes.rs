//! Footnotes and endnotes. A note lives on its reference mark, an `OBJECT`
//! character with `Attrs::note` set, so it follows edits, copy and paste and
//! undo together with the text, and deleting the mark deletes the note.

use std::sync::Arc;

use crate::{Attrs, Document, EditKind, OBJECT, Paragraph, Pos, Selection};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum NoteKind {
    #[default]
    Foot,
    End,
}

/// The text of a footnote or endnote.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Note {
    pub kind: NoteKind,
    /// Its paragraphs (never empty once in a document).
    pub paras: Vec<Paragraph>,
}

impl Eq for Note {}

impl std::hash::Hash for Note {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.kind.hash(h);
        for p in &self.paras {
            p.text.hash(h);
            let st = &p.style;
            for v in [st.space_before.unwrap_or(-1.0), st.space_after.unwrap_or(-1.0), st.line_spacing] {
                v.to_bits().hash(h);
            }
            st.line_rule.hash(h);
            for r in &p.runs {
                r.len.hash(h);
                r.attrs.hash(h);
            }
        }
    }
}

/// Font size of note text in half points (Word's "Footnote Text" is 10 pt).
pub const NOTE_SIZE: u16 = 20;

impl Note {
    /// An empty note, ready to type in.
    pub fn new(kind: NoteKind) -> Note {
        let mut p = Paragraph::new("");
        p.runs[0].attrs.size = Some(NOTE_SIZE);
        Note { kind, paras: vec![p] }
    }

    /// A note holding one field (a citation in a footnote).
    pub fn with_field(kind: NoteKind, field: crate::Field) -> Note {
        let mut n = Note::new(kind);
        let p = &mut n.paras[0];
        let mut attrs = p.runs[0].attrs.clone();
        attrs.field = Some(Arc::new(field));
        p.text = crate::OBJECT.to_string();
        p.runs = vec![crate::Run { len: crate::OBJECT.len_utf8(), attrs }];
        n
    }

    /// A note with plain text (paragraphs split at '\n').
    pub fn from_text(kind: NoteKind, text: &str) -> Note {
        let mut n = Note::new(kind);
        n.set_text(text);
        n
    }

    /// Its text, paragraphs joined with '\n'.
    pub fn text(&self) -> String {
        self.paras.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Replaces the text with `text`, keeping the formatting of what didn't
    /// change: only the differing middle is replaced, so an edit made in a
    /// plain text box doesn't lose the note's italics or links elsewhere.
    pub fn set_text(&mut self, text: &str) {
        let old = self.text();
        if old == text {
            return;
        }
        let a = old.bytes().zip(text.bytes()).take_while(|(x, y)| x == y).count();
        let a = floor(&old, floor(text, a));
        let max_b = (old.len() - a).min(text.len() - a);
        let b = old.bytes().rev().zip(text.bytes().rev()).take(max_b).take_while(|(x, y)| x == y).count();
        let (mut ob, mut nb) = (old.len() - b, text.len() - b);
        while !old.is_char_boundary(ob) || !text.is_char_boundary(nb) {
            ob += 1;
            nb += 1;
        }
        let mut d = Document::from_paragraphs(std::mem::take(&mut self.paras));
        let (pa, pb) = (text_pos(&d, a), text_pos(&d, ob));
        let attrs = if a > 0 || pb != pa { None } else { Some(d.paragraphs[pa.para].attrs_at(pa.offset)) };
        d.insert_text(Selection { anchor: pa, cursor: pb }, &text[a..nb], attrs);
        self.paras = d.paragraphs;
        for p in &mut self.paras {
            p.cell = None;
        }
    }
}

fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Position of byte `i` of the '\n'-joined text of `d`.
fn text_pos(d: &Document, mut i: usize) -> Pos {
    for (k, p) in d.paragraphs.iter().enumerate() {
        if i <= p.text.len() {
            return Pos::new(k, i);
        }
        i -= p.text.len() + 1;
    }
    d.end()
}

/// How notes are numbered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum NoteFormat {
    #[default]
    Arabic,
    LowerRoman,
    UpperRoman,
    LowerLetter,
    UpperLetter,
    /// *, †, ‡, §, then doubled.
    Symbols,
}

impl NoteFormat {
    pub const ALL: [NoteFormat; 6] = [NoteFormat::Arabic, NoteFormat::LowerRoman, NoteFormat::UpperRoman, NoteFormat::LowerLetter, NoteFormat::UpperLetter, NoteFormat::Symbols];

    /// The mark of note number `n` (1-based).
    pub fn label(self, n: usize) -> String {
        let n = n.max(1);
        match self {
            NoteFormat::Arabic => n.to_string(),
            NoteFormat::LowerRoman => crate::roman(n),
            NoteFormat::UpperRoman => crate::roman(n).to_uppercase(),
            NoteFormat::LowerLetter => crate::letters(n),
            NoteFormat::UpperLetter => crate::letters(n).to_uppercase(),
            NoteFormat::Symbols => {
                const S: [char; 4] = ['*', '†', '‡', '§'];
                // Past ten of a mark, numbers: the marks would grow
                // with the square of the notes.
                if n > 40 { n.to_string() } else { S[(n - 1) % 4].to_string().repeat((n - 1) / 4 + 1) }
            }
        }
    }
}

/// Where numbering starts again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum NoteRestart {
    #[default]
    Continuous,
    EachPage,
    EachSection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NoteSettings {
    pub format: NoteFormat,
    /// Number of the first note.
    pub start: u32,
    pub restart: NoteRestart,
}

/// Footnote and endnote numbering of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NotesConfig {
    pub foot: NoteSettings,
    pub end: NoteSettings,
    /// The paragraph that holds the line above the footnotes, as the file
    /// sets it (`None`: 12 pt).
    pub separator: Option<NoteSeparator>,
}

/// Word's footnote separator is a paragraph with the line in it: the room
/// it takes above the notes is its line and the space around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NoteSeparator {
    /// Font size in half points.
    pub size: u16,
    /// Line spacing: hundredths of a line with the multiple rule, twips
    /// with the others.
    pub line: u16,
    pub rule: crate::LineRule,
    /// Space before and after in twips.
    pub before: u16,
    pub after: u16,
}

impl NoteSeparator {
    /// Its height in points for a font whose single line is `factor` times
    /// its size.
    pub fn height(&self, factor: f32) -> f32 {
        let single = self.size as f32 / 2.0 * factor;
        let line = match self.rule {
            crate::LineRule::Multiple => single * self.line as f32 / 100.0,
            crate::LineRule::Exact => self.line as f32 / 20.0,
            crate::LineRule::AtLeast => single.max(self.line as f32 / 20.0),
        };
        line + (self.before + self.after) as f32 / 20.0
    }
}

impl Default for NotesConfig {
    fn default() -> Self {
        // Word's defaults: 1, 2, 3 for footnotes and i, ii, iii for endnotes.
        NotesConfig {
            foot: NoteSettings { format: NoteFormat::Arabic, start: 1, restart: NoteRestart::Continuous },
            end: NoteSettings { format: NoteFormat::LowerRoman, start: 1, restart: NoteRestart::Continuous },
            separator: None,
        }
    }
}

impl NotesConfig {
    pub fn get(&self, kind: NoteKind) -> &NoteSettings {
        match kind {
            NoteKind::Foot => &self.foot,
            NoteKind::End => &self.end,
        }
    }
}

impl Document {
    /// Inserts a new empty footnote or endnote at the end of the selection
    /// (the selected text stays). Returns the position after its mark.
    pub fn insert_note(&mut self, sel: Selection, kind: NoteKind) -> Pos {
        self.insert_note_with(sel, Note::new(kind))
    }

    /// Inserts a note with its text (a citation in a footnote, say).
    pub fn insert_note_with(&mut self, sel: Selection, note: Note) -> Pos {
        let at = self.clamp(sel.end());
        let mut attrs = self.typing_attrs(at);
        (attrs.link, attrs.link_tip, attrs.field) = (None, None, None);
        attrs.note = Some(Arc::new(note));
        let end = self.insert_text(Selection::caret(at), &OBJECT.to_string(), Some(attrs));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Note;
        }
        end
    }

    /// The note whose mark is right after `p`, and the mark's extent.
    pub fn note_at(&self, p: Pos) -> Option<(Selection, Arc<Note>)> {
        let para = self.paragraphs.get(p.para)?;
        if !para.text.get(p.offset..)?.starts_with(OBJECT) {
            return None;
        }
        let n = para.attrs_at(p.offset + OBJECT.len_utf8()).note?;
        Some((Selection { anchor: p, cursor: Pos::new(p.para, p.offset + OBJECT.len_utf8()) }, n))
    }

    /// Changes the note at `p`. Consecutive changes to the same note (typing
    /// in the notes pane) make one undo step.
    pub fn set_note(&mut self, p: Pos, note: Note) {
        let Some((sel, old)) = self.note_at(p) else { return };
        if *old == note {
            return;
        }
        let note = Arc::new(note);
        let before = vec![self.paragraphs[p.para].clone()];
        self.paragraphs[p.para].map_attrs(sel.start().offset, sel.end().offset, &|a| a.note = Some(note.clone()));
        let caret = Selection::caret(sel.end());
        self.record(p.para, before, 1, caret, caret, EditKind::Note);
    }

    /// Every note mark in document order: position and note.
    pub fn notes(&self) -> Vec<(Pos, Arc<Note>)> {
        let mut out = Vec::new();
        for (i, para) in self.paragraphs.iter().enumerate() {
            for (s, e, a) in para.spans() {
                if let Some(n) = &a.note {
                    for (o, _) in para.text[s..e].match_indices(OBJECT) {
                        out.push((Pos::new(i, s + o), n.clone()));
                    }
                }
            }
        }
        out
    }

    /// True if any paragraph has a note or field (quick check for callers
    /// that can skip their work otherwise).
    pub fn has_objects(&self) -> bool {
        self.paragraphs.iter().any(para_has_objects)
    }
}

/// True if the paragraph holds notes or fields.
pub fn para_has_objects(p: &Paragraph) -> bool {
    p.runs.iter().any(|r| r.attrs.note.is_some() || r.attrs.field.is_some())
}

/// Attributes of the `OBJECT` characters of a paragraph that are notes or
/// fields: byte offset and attributes.
pub fn objects_of(p: &Paragraph) -> impl Iterator<Item = (usize, &Attrs)> {
    p.spans().filter(|(_, _, a)| a.note.is_some() || a.field.is_some()).flat_map(move |(s, e, a)| p.text[s..e].match_indices(OBJECT).map(move |(o, _)| (s + o, a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(NoteFormat::Arabic.label(12), "12");
        assert_eq!(NoteFormat::LowerRoman.label(4), "iv");
        assert_eq!(NoteFormat::UpperLetter.label(27), "AA");
        assert_eq!(NoteFormat::Symbols.label(1), "*");
        assert_eq!(NoteFormat::Symbols.label(6), "††");
    }

    #[test]
    fn notes_follow_text() {
        let mut d = Document::from_plain_text("hello world");
        let end = d.insert_note(Selection::caret(Pos::new(0, 5)), NoteKind::Foot);
        assert_eq!(end, Pos::new(0, 5 + OBJECT.len_utf8()));
        assert_eq!(d.notes().len(), 1);
        // Typing after the mark doesn't make more notes.
        d.insert_text(Selection::caret(end), "!", None);
        assert_eq!(d.notes().len(), 1);
        assert!(d.paragraphs[0].attrs_at(end.offset + 1).note.is_none());
        let at = Pos::new(0, 5);
        let mut n = (*d.note_at(at).unwrap().1).clone();
        n.set_text("A note");
        d.set_note(at, n.clone());
        n.set_text("A note.");
        d.set_note(at, n);
        assert_eq!(d.note_at(at).unwrap().1.text(), "A note.");
        // Two changes to the note are one undo step.
        d.undo();
        assert_eq!(d.note_at(at).unwrap().1.text(), "");
        // Deleting the mark deletes the note; undo brings it back.
        d.redo();
        d.delete(Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 9) });
        assert!(d.notes().is_empty());
        d.undo();
        assert_eq!(d.notes()[0].1.text(), "A note.");
        // Copy and paste copies the note.
        let clip = d.slice(Pos::new(0, 0), d.end());
        d.insert_paragraphs(Selection::caret(d.end()), &clip);
        assert_eq!(d.notes().len(), 2);
    }

    #[test]
    fn set_text_keeps_formatting() {
        let mut n = Note::new(NoteKind::Foot);
        n.paras[0].push("Read ", Attrs::default());
        n.paras[0].push("Book", Attrs { italic: true, ..Default::default() });
        n.paras[0].push(" now", Attrs::default());
        n.set_text("Read Book today");
        assert_eq!(n.text(), "Read Book today");
        assert!(n.paras[0].attrs_at(7).italic);
        n.set_text("Read Book today\nSecond");
        assert_eq!(n.paras.len(), 2);
        n.set_text("Año");
        assert_eq!(n.text(), "Año");
        n.set_text("Añadió");
        assert_eq!(n.text(), "Añadió");
    }
}
