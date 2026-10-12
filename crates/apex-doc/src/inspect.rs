//! Document Inspector: what a document carries besides its visible text,
//! and removing it before sharing the file.

use std::sync::Arc;

use crate::comments::is_comment_mark;
use crate::{Document, EditKind, Image, KeptParts, Macros, Meta, Pos, Selection};

/// What the inspector looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Inspect {
    /// Comments and tracked changes.
    Comments,
    /// Document properties and personal information.
    Properties,
    /// Headers, footers and watermarks.
    Headers,
    /// Text formatted as hidden.
    HiddenText,
}

impl Inspect {
    pub const ALL: [Inspect; 4] = [Inspect::Comments, Inspect::Properties, Inspect::Headers, Inspect::HiddenText];
}

/// What was found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inspection {
    pub comments: usize,
    pub revisions: usize,
    /// Names of the properties that are set.
    pub properties: Vec<&'static str>,
    pub headers: usize,
    pub watermark: bool,
    /// Characters of hidden text.
    pub hidden: usize,
}

impl Inspection {
    pub fn found(&self, what: Inspect) -> bool {
        match what {
            Inspect::Comments => self.comments + self.revisions > 0,
            Inspect::Properties => !self.properties.is_empty(),
            Inspect::Headers => self.headers > 0 || self.watermark,
            Inspect::HiddenText => self.hidden > 0,
        }
    }
}

fn set_properties(m: &Meta) -> Vec<&'static str> {
    let fields = [
        ("title", &m.title),
        ("author", &m.author),
        ("subject", &m.subject),
        ("keywords", &m.keywords),
        ("comments", &m.description),
        ("category", &m.category),
        ("company", &m.company),
        ("manager", &m.manager),
    ];
    let mut out: Vec<&'static str> = fields.iter().filter(|(_, v)| !v.trim().is_empty()).map(|(n, _)| *n).collect();
    // Sensitivity labels are the organization's: they stay.
    if m.custom.iter().any(|p| !crate::kept::is_label_prop(&p.name)) {
        out.push("custom");
    }
    out
}

/// A field showing a property that identifies someone (not a
/// sensitivity label), with what it showed last.
fn shows_property(f: &crate::Field) -> bool {
    use crate::FieldKind as K;
    !f.cached.is_empty() && match &f.kind {
        K::Author | K::Title => true,
        K::DocProperty(n) => !crate::kept::is_label_prop(n),
        _ => false,
    }
}

impl Document {
    /// Fields that show a property (author, title, company...) with what
    /// they showed last: "AUTHOR: Ana".
    pub fn property_fields(&self) -> Vec<String> {
        self.paragraphs
            .iter()
            .chain(self.hf_stories().flat_map(|s| s.iter()))
            .chain(self.note_paras())
            .flat_map(|p| p.runs.iter().filter_map(|r| r.attrs.field.as_deref()))
            .filter(|f| shows_property(f))
            .map(|f| format!("{}: {}", f.instr().trim(), f.cached))
            .collect()
    }

    fn hidden_spans(&self, i: usize) -> Vec<(usize, usize)> {
        hidden_in(&self.paragraphs[i])
    }

    pub fn inspect(&self) -> Inspection {
        let parts = |h: &crate::Headers| h.parts.iter().filter(|p| p.as_ref().is_some_and(|s| s.iter().any(|p| !p.text.trim().is_empty() || !p.runs.iter().all(|r| r.attrs.image.is_none() && r.attrs.shape.is_none())))).count();
        let headers = parts(&self.headers) + self.paragraphs.iter().filter_map(|p| p.section.as_deref()).map(|s| parts(&s.headers)).sum::<usize>();
        let hidden = (0..self.paragraphs.len()).flat_map(|i| self.hidden_spans(i).into_iter().map(move |(s, e)| (i, s, e))).map(|(i, s, e)| self.paragraphs[i].text[s..e].chars().count()).sum::<usize>()
            + self.hf_stories().flat_map(|s| s.iter()).chain(self.note_paras()).flat_map(|p| hidden_in(p).into_iter().map(move |(s, e)| p.text[s..e].chars().count())).sum::<usize>();
        Inspection {
            comments: self.comments.len(),
            revisions: self.changes().len() + self.hf_changes(),
            properties: { let mut p = set_properties(&self.meta); if !self.property_fields().is_empty() { p.push("fields"); } p },
            headers,
            watermark: self.deco.watermark.as_ref().is_some_and(|w| !w.is_empty()),
            hidden,
        }
    }

    /// Removes what the inspector found of `what`, as one undo step
    /// (properties aren't part of the undo history).
    pub fn remove_inspected(&mut self, what: Inspect, sel: Selection) {
        if what == Inspect::Properties {
            self.edit_parts(|p| p.meta = cleared_meta(&p.meta));
            // Fields showing them keep what they showed last: that goes too.
            let clear = |story: &mut Vec<crate::Paragraph>| {
                let mut n = 0;
                for p in story.iter_mut() {
                    let found: Vec<(usize, usize, Arc<crate::Field>)> = p.spans().filter_map(|(s, e, a)| a.field.clone().filter(|f| shows_property(f)).map(|f| (s, e, f))).collect();
                    for (s, e, f) in found {
                        let f = Arc::new(crate::Field { cached: String::new(), ..(*f).clone() });
                        p.map_attrs(s, e, &|a| a.field = Some(f.clone()));
                        n += 1;
                    }
                }
                n
            };
            let mut body = std::mem::take(&mut self.paragraphs);
            let mut n = clear(&mut body);
            self.paragraphs = body;
            for story in self.hf_stories_mut() {
                n += clear(story);
            }
            n += self.edit_notes(&clear);
            if n > 0 {
                self.revision += 1;
            }
            return;
        }
        let state = self.page_state();
        let before = self.paragraphs.clone();
        let comments = self.comments.clone();
        match what {
            Inspect::Comments => {
                if self.has_revisions() {
                    let (track, steps) = (std::mem::replace(&mut self.track_changes, false), self.undo.len());
                    let end = self.end();
                    self.resolve_changes(Pos::new(0, 0), end, true, sel);
                    self.resolve_hf_changes(true);
                    self.track_changes = track;
                    // The steps it took are replaced by this one.
                    self.undo.truncate(steps);
                }
                self.comments.clear();
                for p in &mut self.paragraphs {
                    p.marks.retain(|m| !is_comment_mark(&m.name));
                }
            }
            Inspect::Headers => {
                self.headers.parts = Default::default();
                for s in self.paragraphs.iter_mut().filter_map(|p| p.section.as_deref_mut()) {
                    s.headers.parts = Default::default();
                }
                self.deco.watermark = None;
            }
            Inspect::HiddenText => {
                let strip = |story: &mut Vec<crate::Paragraph>| {
                    let mut n = 0;
                    for p in story.iter_mut() {
                        for (s, e) in hidden_in(p).into_iter().rev() {
                            p.remove(s, e);
                            n += 1;
                        }
                    }
                    n
                };
                for story in self.hf_stories_mut() {
                    strip(story);
                }
                self.edit_notes(&strip);
                for i in (0..self.paragraphs.len()).rev() {
                    for (s, e) in self.hidden_spans(i).into_iter().rev() {
                        self.paragraphs[i].remove(s, e);
                    }
                    // A paragraph that was all hidden goes too.
                    let p = &self.paragraphs[i];
                    let was_text = !before.get(i).is_some_and(|b| b.text.is_empty());
                    if p.text.is_empty() && was_text && self.paragraphs.len() > 1 && i + 1 < self.paragraphs.len() && p.cell.is_none() && p.section.is_none() && p.marks.is_empty() && self.paragraphs[i + 1].cell.is_none() {
                        self.paragraphs.remove(i);
                    }
                }
            }
            Inspect::Properties => unreachable!(),
        }
        let sel = Selection::caret(Pos::new(0, 0));
        let n = self.paragraphs.len();
        self.record(0, before, n, sel, sel, EditKind::Format);
        if let Some(e) = self.undo.last_mut() {
            e.page = Some(Box::new(state));
            e.comments = Some(comments);
        }
    }
}

impl Document {
    /// What a reader of a copy that can't keep things aside (a web page,
    /// an e-book, Markdown, plain text) should get: the changes accepted,
    /// and no hidden text, nor pictures or shapes hidden from the
    /// Selection Pane.
    pub fn shared(&self) -> Document {
        let mut d = self.resolved(true);
        let track = std::mem::replace(&mut d.track_changes, false);
        d.remove_inspected(Inspect::HiddenText, Selection::default());
        let unseen = |story: &mut Vec<crate::Paragraph>| {
            for p in story.iter_mut() {
                let gone: Vec<(usize, usize)> = p.spans().into_iter().filter(|(_, _, a)| a.image.as_ref().is_some_and(|i| i.hidden) || a.shape.as_ref().is_some_and(|s| s.shape.hidden)).map(|(s, e, _)| (s, e)).collect();
                for (s, e) in gone.into_iter().rev() {
                    p.remove(s, e);
                }
            }
            0
        };
        for story in d.hf_stories_mut() {
            unseen(story);
        }
        d.edit_notes(&unseen);
        unseen(&mut d.paragraphs);
        d.track_changes = track;
        d.undo.clear();
        d
    }
}

/// The hidden text of a paragraph (byte ranges, joined).
fn hidden_in(p: &crate::Paragraph) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (s, e, a) in p.spans() {
        if e > s && a.fx().hidden {
            match out.last_mut() {
                Some(last) if last.1 == s => last.1 = e,
                _ => out.push((s, e)),
            }
        }
    }
    out
}

/// The properties with what identifies people or the organization
/// cleared (the creation date and the sensitivity labels stay).
pub fn cleared_meta(m: &Meta) -> Meta {
    let mut custom = m.custom.clone();
    // The sensitivity label stays, but not who set it.
    custom.retain(|p| crate::kept::is_label_prop(&p.name) && !p.name.ends_with("_Owner"));
    Meta { created: m.created.clone(), custom, read_only_recommended: m.read_only_recommended, ..Meta::default() }
}

/// Properties, macros and kept parts: what [`Document::edit_parts`]
/// changes as one undo step.
#[derive(Clone, Debug, Default)]
pub struct FileParts {
    pub meta: Meta,
    pub macros: Macros,
    pub kept: KeptParts,
    /// Book data (book mode).
    pub book: Option<crate::book::Book>,
}

/// Whether a link goes to another file (not a web page, a mail address
/// or a place in the document).
pub fn is_file_link(t: &str) -> bool {
    let l = t.trim().to_ascii_lowercase();
    !(l.is_empty() || l.starts_with('#') || l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:") || l.starts_with("www.") || l.starts_with("ftp://") || l.starts_with("tel:"))
}

impl Document {
    pub(crate) fn swap_parts(&mut self, p: FileParts) -> FileParts {
        FileParts { meta: std::mem::replace(&mut self.meta, p.meta), macros: std::mem::replace(&mut self.macros, p.macros), kept: std::mem::replace(&mut self.kept, p.kept), book: std::mem::replace(&mut self.book, p.book) }
    }

    /// Changes the properties, macros, kept parts or book data as one undo
    /// step.
    pub fn edit_parts(&mut self, f: impl FnOnce(&mut FileParts)) {
        let mut new = FileParts { meta: self.meta.clone(), macros: self.macros.clone(), kept: self.kept.clone(), book: self.book.clone() };
        f(&mut new);
        let old = self.swap_parts(new);
        let sel = Selection::caret(Pos::new(0, 0));
        self.record(0, Vec::new(), 0, sel, sel, EditKind::Format);
        if let Some(e) = self.undo.last_mut() {
            e.parts = Some(Box::new(old));
        }
    }

    /// The targets of the links to other files, each once.
    pub fn file_links(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in self.paragraphs.iter().flat_map(|p| &p.runs) {
            if let Some(l) = r.attrs.link.as_deref().filter(|l| is_file_link(l)) {
                if !out.iter().any(|x| x == l) {
                    out.push(l.to_string());
                }
            }
        }
        out
    }

    /// The program of each object embedded by another program (OLE).
    pub fn ole_objects(&self) -> Vec<String> {
        self.objects().iter().filter_map(|(_, i)| i.object.as_ref()?.ole().map(|o| o.prog_id.clone())).collect()
    }

    /// Changes every run's attributes with `f` as one undo step; false
    /// (and no step) when nothing changed.
    fn map_runs(&mut self, f: impl Fn(&mut crate::Attrs) -> bool) -> bool {
        let before = self.paragraphs.clone();
        let mut changed = false;
        for p in &mut self.paragraphs {
            for r in &mut p.runs {
                changed |= f(&mut r.attrs);
            }
        }
        if changed {
            let sel = Selection::caret(Pos::new(0, 0));
            let n = self.paragraphs.len();
            self.record(0, before, n, sel, sel, EditKind::Link);
        }
        changed
    }

    /// Takes the links to other files off their text, as one undo step.
    pub fn remove_file_links(&mut self) -> bool {
        self.map_runs(|a| {
            let file = a.link.as_deref().is_some_and(is_file_link);
            if file {
                a.link = None;
                a.link_tip = None;
            }
            file
        })
    }

    /// Turns the objects embedded by other programs into plain pictures
    /// (their preview), as one undo step.
    pub fn remove_ole(&mut self) -> bool {
        let plain = |i: &Arc<Image>| i.object.as_ref().is_some_and(|o| o.ole().is_some()).then(|| Arc::new(Image { object: None, ..(**i).clone() }));
        self.map_runs(|a| {
            let mut hit = false;
            if let Some(n) = a.image.as_ref().and_then(plain) {
                a.image = Some(n);
                hit = true;
            }
            if let Some(ds) = &a.shape
                && let Some(n) = ds.shape.picture.as_ref().and_then(plain)
            {
                let mut ds = (**ds).clone();
                ds.shape.picture = Some(n);
                a.shape = Some(Arc::new(ds));
                hit = true;
            }
            hit
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Attrs, HfSlot, Paragraph};

    #[test]
    fn finds_and_removes() {
        let hidden = {
            let mut a = Attrs::default();
            a.set_fx(|f| f.hidden = true);
            a
        };
        let mut p = Paragraph::new("");
        p.push("visible ", Attrs::default());
        p.push("secret", hidden.clone());
        let mut all = Paragraph::new("");
        all.push("all hidden", hidden);
        let mut d = Document::from_paragraphs(vec![p, all, Paragraph::new("end")]);
        d.user = "Ana".into();
        d.meta.author = "Ana".into();
        d.headers.set(HfSlot::Header, Some(Arc::new(vec![Paragraph::new("Head")])));
        d.add_comment(Selection { anchor: Pos::new(2, 0), cursor: Pos::new(2, 3) }, "note");
        let found = d.inspect();
        assert_eq!((found.comments, found.headers, found.hidden, found.properties.clone()), (1, 1, 16, vec!["author"]));
        for w in Inspect::ALL {
            assert!(found.found(w));
            d.remove_inspected(w, Selection::default());
        }
        let after = d.inspect();
        assert!(Inspect::ALL.iter().all(|w| !after.found(*w)));
        assert_eq!(d.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["visible ", "end"]);
        // Undo brings the hidden text back.
        d.undo();
        assert_eq!(d.inspect().hidden, 16);
    }

    #[test]
    fn fields_showing_properties_are_cleared() {
        let field = |kind, cached: &str| {
            let mut a = Attrs::default();
            a.field = Some(Arc::new(crate::Field { cached: cached.into(), ..crate::Field::new(kind) }));
            a
        };
        let mut head = Paragraph::new("");
        head.push("Ana", field(crate::FieldKind::Author, "Ana"));
        let mut body = Paragraph::new("");
        body.push("ACME", field(crate::FieldKind::DocProperty("Company".into()), "ACME"));
        body.push(" 1", field(crate::FieldKind::Page, "1"));
        let mut d = Document::from_paragraphs(vec![body]);
        d.headers.set(HfSlot::Header, Some(Arc::new(vec![head])));
        // No property set, but fields still show who wrote it.
        assert!(d.inspect().found(Inspect::Properties));
        assert_eq!(d.property_fields(), ["DOCPROPERTY Company: ACME", "AUTHOR: Ana"]);
        d.remove_inspected(Inspect::Properties, Selection::default());
        assert!(!d.inspect().found(Inspect::Properties));
        assert!(d.property_fields().is_empty());
        let page = d.paragraphs[0].runs.iter().find_map(|r| r.attrs.field.as_ref().filter(|f| f.kind == crate::FieldKind::Page));
        assert_eq!(page.map(|f| f.cached.as_str()), Some("1"));
    }

    #[test]
    fn parts_links_and_objects() {
        let mut p = Paragraph::new("");
        let link = |t: &str| {
            let mut a = Attrs::default();
            a.link = Some(t.into());
            a
        };
        p.push("web ", link("https://example.com"));
        p.push("file", link("C:\\data\\book.xlsx"));
        let mut d = Document::from_paragraphs(vec![p]);
        let obj = crate::Embedded::new(crate::EmbedKind::Ole(crate::OleObject { prog_id: "Excel.Sheet.12".into(), data: Arc::from(&b"x"[..]), ..Default::default() }));
        d.insert_image(Selection::caret(Pos::new(0, 0)), Image { object: Some(Arc::new(obj)), ..Image::new(Arc::from(&b"png"[..]), 10.0, 10.0) });
        d.meta.author = "Ana".into();
        d.macros.project = Some(Arc::new(vec![1, 2]));
        assert_eq!(d.file_links(), ["C:\\data\\book.xlsx"]);
        assert_eq!(d.ole_objects(), ["Excel.Sheet.12"]);
        assert!(d.remove_file_links() && d.file_links().is_empty());
        assert!(d.remove_ole() && d.ole_objects().is_empty() && d.images().len() == 1);
        d.edit_parts(|p| p.macros = Macros::default());
        d.remove_inspected(Inspect::Properties, Selection::default());
        assert!(d.macros.project.is_none() && d.meta.author.is_empty());
        d.undo();
        assert_eq!(d.meta.author, "Ana");
        d.undo();
        assert!(d.macros.project.is_some());
        d.undo();
        assert_eq!(d.ole_objects().len(), 1);
        d.undo();
        assert_eq!(d.file_links().len(), 1);
        d.redo();
        assert!(d.file_links().is_empty());
    }
}
