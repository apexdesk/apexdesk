//! Content controls: inline regions of a paragraph for forms and
//! templates (plain and rich text, check boxes, lists and dates), as Word's
//! `w:sdt` and LibreOffice's `loext:content-control`.
//!
//! A control is the run of characters whose attributes carry the same
//! control (same `id`). An empty control shows its placeholder text, marked
//! with `placeholder`.

use std::ops::Range;
use std::sync::Arc;

use crate::{Attrs, Document, EditKind, Paragraph, Pos, Selection};

/// An entry of a list control: shown text and stored value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct ListItem {
    pub text: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ControlKind {
    RichText,
    PlainText,
    CheckBox { checked: bool },
    /// A list: `editable` (a combo box) also takes typed text.
    List { items: Vec<ListItem>, editable: bool },
    /// A date picker: the date (yyyy-mm-dd) and how it shows ("dd/MM/yyyy").
    Date { date: Option<String>, format: String },
}

impl ControlKind {
    /// Whether the text inside is typed (not picked).
    pub fn typed(&self) -> bool {
        match self {
            ControlKind::RichText | ControlKind::PlainText | ControlKind::Date { .. } => true,
            ControlKind::List { editable, .. } => *editable,
            ControlKind::CheckBox { .. } => false,
        }
    }
}

pub const CHECKED: char = '☒';
pub const UNCHECKED: char = '☐';

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentControl {
    /// Tells neighbouring controls apart (unique in the document).
    pub id: u32,
    pub kind: ControlKind,
    /// Shown on the control's tab.
    pub title: String,
    pub tag: String,
    /// The text inside is the placeholder.
    pub placeholder: bool,
    /// The control can't be deleted.
    pub lock_delete: bool,
    /// Its contents can't be edited.
    pub lock_content: bool,
    /// The custom XML node the control shows and edits.
    pub binding: Option<DataBinding>,
    /// A legacy form field (see `form_fields.rs`).
    pub legacy: Option<Box<crate::FormField>>,
}

/// Where a control's value lives in the file's custom XML (Word's
/// `w:dataBinding`): the part with `store_item` as its id, and the node
/// `xpath` picks with the namespace `prefixes` ("xmlns:ns0='urn:x'").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct DataBinding {
    pub xpath: String,
    pub store_item: String,
    pub prefixes: String,
}

impl ContentControl {
    pub fn new(id: u32, kind: ControlKind) -> Self {
        ContentControl { id, kind, title: String::new(), tag: String::new(), placeholder: false, lock_delete: false, lock_content: false, binding: None, legacy: None }
    }

    /// Whether typing may change its text.
    pub fn editable(&self) -> bool {
        !self.lock_content && self.kind.typed() && self.legacy.as_ref().is_none_or(|f| f.fillable())
    }
}

/// A control found in the text: where and which.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlAt {
    pub para: usize,
    pub range: Range<usize>,
    pub control: Arc<ContentControl>,
}

impl ControlAt {
    pub fn sel(&self) -> Selection {
        Selection { anchor: Pos::new(self.para, self.range.start), cursor: Pos::new(self.para, self.range.end) }
    }
}

/// The controls of a paragraph, in order.
pub fn para_controls(p: &Paragraph, para: usize) -> Vec<ControlAt> {
    let mut out: Vec<ControlAt> = Vec::new();
    for (s, e, a) in p.spans().filter(|(s, e, _)| e > s) {
        let Some(c) = &a.control else { continue };
        match out.last_mut() {
            Some(last) if last.range.end == s && last.control.id == c.id => last.range.end = e,
            _ => out.push(ControlAt { para, range: s..e, control: c.clone() }),
        }
    }
    out
}

/// A date in yyyy-mm-dd shown with a Word date picture ("dd/MM/yyyy").
pub fn control_date_text(iso: &str, picture: &str, months: &[&str], days: &[&str]) -> String {
    let mut it = iso.split('-').map(|x| x.parse::<i64>().unwrap_or(0));
    let (y, m, d) = (it.next().unwrap_or(0), it.next().unwrap_or(1) as u32, it.next().unwrap_or(1) as u32);
    crate::format_date(if picture.is_empty() { "dd/MM/yyyy" } else { picture }, (y, m, d, 0, 0, 0), months, days)
}

impl Document {
    /// Every control of the document.
    pub fn controls(&self) -> Vec<ControlAt> {
        self.paragraphs.iter().enumerate().flat_map(|(i, p)| para_controls(p, i)).collect()
    }

    /// The control holding the character at `p` or, failing that, the one
    /// before it (so a caret at a control's end is in it).
    pub fn control_at(&self, p: Pos) -> Option<ControlAt> {
        let para = self.paragraphs.get(p.para)?;
        let all = para_controls(para, p.para);
        all.iter().find(|c| c.range.start <= p.offset && p.offset < c.range.end).or_else(|| all.iter().find(|c| c.range.start < p.offset && p.offset <= c.range.end)).cloned()
    }

    /// The control a selection is inside (both ends in the same one).
    pub fn control_of(&self, sel: Selection) -> Option<ControlAt> {
        let (a, b) = (sel.start(), sel.end());
        let c = self.control_at(a)?;
        (b.para == c.para && b.offset <= c.range.end && a.offset >= c.range.start).then_some(c)
    }

    pub fn next_control_id(&self) -> u32 {
        self.controls().iter().map(|c| c.control.id).max().map_or(1, |m| m + 1)
    }

    /// Inserts a control at the selection. Text controls wrap selected
    /// text; otherwise the control holds `text` (its placeholder unless
    /// `filled`). Returns the control's extent.
    pub fn insert_control(&mut self, sel: Selection, control: ContentControl, text: &str) -> Selection {
        let (a, b) = (sel.start(), sel.end());
        if a.para == b.para && a != b && matches!(control.kind, ControlKind::RichText | ControlKind::PlainText) && self.control_of(sel).is_none() {
            let before = vec![self.paragraphs[a.para].clone()];
            let c = Arc::new(ContentControl { placeholder: false, ..control });
            self.paragraphs[a.para].map_attrs(a.offset, b.offset, &|x| x.control = Some(c.clone()));
            let s = Selection { anchor: a, cursor: b };
            self.record(a.para, before, 1, sel, s, EditKind::Insert);
            return s;
        }
        let mut attrs = self.typing_attrs(a);
        attrs.control = Some(Arc::new(control));
        let end = self.insert_text(Selection { anchor: a, cursor: b }, text, Some(attrs));
        Selection { anchor: Pos::new(end.para, end.offset - text.len()), cursor: end }
    }

    /// Replaces a control's text (a new state of the control goes with it),
    /// as one undo step. Returns the caret after it.
    pub fn set_control(&mut self, at: &ControlAt, control: ContentControl, text: &str, sel: Selection) -> Pos {
        let para = &self.paragraphs[at.para];
        let mut attrs = para.attrs_at(at.range.start + para.text[at.range.start..].chars().next().map_or(0, char::len_utf8));
        attrs.control = Some(Arc::new(control));
        let before = vec![para.clone()];
        let p = &mut self.paragraphs[at.para];
        if p.text[at.range.clone()] == *text {
            p.map_attrs(at.range.start, at.range.end, &|x| x.control = attrs.control.clone());
        } else {
            p.remove(at.range.start, at.range.end);
            p.insert_str(at.range.start, text, attrs);
        }
        let caret = Pos::new(at.para, at.range.start + text.len());
        self.record(at.para, before, 1, sel, Selection::caret(caret), EditKind::Replace);
        caret
    }

    /// Gives a control a new state and text outside the undo history: a
    /// bound control following its data (undoing the edit that changed the
    /// data brings it back the same way).
    pub fn refresh_control(&mut self, at: &ControlAt, control: ContentControl, text: &str) {
        let para = &self.paragraphs[at.para];
        let mut attrs = para.attrs_at(at.range.start + para.text[at.range.start..].chars().next().map_or(0, char::len_utf8));
        attrs.control = Some(Arc::new(control));
        let p = &mut self.paragraphs[at.para];
        p.remove(at.range.start, at.range.end);
        p.insert_str(at.range.start, text, attrs);
        self.revision += 1;
        self.modified = true;
    }

    /// Takes the control away, keeping its text (a placeholder goes too).
    pub fn remove_control(&mut self, at: &ControlAt, sel: Selection) -> Pos {
        let before = vec![self.paragraphs[at.para].clone()];
        let p = &mut self.paragraphs[at.para];
        let caret = if at.control.placeholder {
            p.remove(at.range.start, at.range.end);
            Pos::new(at.para, at.range.start)
        } else {
            p.map_attrs(at.range.start, at.range.end, &|x| x.control = None);
            Pos::new(at.para, at.range.end)
        };
        self.record(at.para, before, 1, sel, Selection::caret(caret), EditKind::Format);
        caret
    }

    /// After an edit at `p`: puts the placeholder back in a control whose
    /// text was all deleted (`gone` is the control as it was).
    pub fn refill_control(&mut self, p: Pos, gone: &ContentControl, placeholder: &str) -> Selection {
        let mut attrs = self.typing_attrs(p);
        attrs.control = Some(Arc::new(ContentControl { placeholder: true, ..gone.clone() }));
        let end = self.insert_text(Selection::caret(p), placeholder, Some(attrs));
        Selection { anchor: p, cursor: end }
    }
}

impl Attrs {
    /// The control with placeholder text, for new text typed over it.
    pub fn filled_control(&mut self) {
        if let Some(c) = self.control.as_mut().filter(|c| c.placeholder) {
            Arc::make_mut(c).placeholder = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONTHS: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];

    #[test]
    fn dates() {
        let days = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
        assert_eq!(control_date_text("2026-03-05", "dd/MM/yyyy", &MONTHS, &days), "05/03/2026");
        assert_eq!(control_date_text("2026-03-05", "d 'de' MMMM 'de' yyyy", &MONTHS, &days), "5 de marzo de 2026");
    }

    #[test]
    fn insert_fill_and_remove() {
        let mut d = Document::from_plain_text("Name: ");
        let c = ContentControl { placeholder: true, title: "Name".into(), ..ContentControl::new(d.next_control_id(), ControlKind::PlainText) };
        let s = d.insert_control(Selection::caret(Pos::new(0, 6)), c, "Click here");
        assert_eq!(s, Selection { anchor: Pos::new(0, 6), cursor: Pos::new(0, 16) });
        let at = d.control_at(Pos::new(0, 16)).unwrap();
        assert!(at.control.placeholder && at.range == (6..16));
        assert_eq!(d.control_at(Pos::new(0, 6)).unwrap().range, 6..16);
        assert!(d.control_at(Pos::new(0, 5)).is_none());
        // Filled in.
        let c = ContentControl { placeholder: false, ..(*at.control).clone() };
        let p = d.set_control(&at, c, "Ana", s);
        assert_eq!((d.paragraphs[0].text.as_str(), p), ("Name: Ana", Pos::new(0, 9)));
        let at = d.control_at(p).unwrap();
        assert_eq!((at.range.clone(), at.control.placeholder), (6..9, false));
        // Typing at its end goes in it.
        let p = d.insert_text(Selection::caret(p), "s", None);
        assert_eq!(d.control_at(p).unwrap().range, 6..10);
        // Removed: the text stays.
        d.remove_control(&d.control_at(p).unwrap(), Selection::caret(p));
        assert!(d.controls().is_empty());
        assert_eq!(d.paragraphs[0].text, "Name: Anas");
        d.undo();
        assert_eq!(d.controls().len(), 1);
    }

    #[test]
    fn neighbours_stay_apart() {
        let mut d = Document::from_plain_text("");
        let id = d.next_control_id();
        d.insert_control(Selection::caret(Pos::new(0, 0)), ContentControl::new(id, ControlKind::CheckBox { checked: false }), &UNCHECKED.to_string());
        let id2 = d.next_control_id();
        assert_ne!(id, id2);
        let end = d.paragraphs[0].text.len();
        d.insert_control(Selection::caret(Pos::new(0, end)), ContentControl::new(id2, ControlKind::PlainText), "x");
        assert_eq!(d.controls().len(), 2);
    }
}
