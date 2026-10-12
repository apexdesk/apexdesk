//! Text copied in one document and pasted in another. Paragraphs point at
//! their document's styles, lists and sources by number or id, and take
//! its font and size where they set none: pasted as they are in another
//! document they would change look, or point at the wrong style. `Origin`
//! is what the copy keeps of its document; `Document::adopt` turns the
//! paragraphs into ones of the document they go to, looking as they did.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Attrs, CharFormat, Document, FieldKind, InCell, LineRule, ParaStyle, Paragraph, Source, Styles, builtin_id, table_style};

/// What copied paragraphs need from the document they come from.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Origin {
    styles: Styles,
    default_font: Option<Arc<str>>,
    /// The sources their citations name.
    sources: Vec<Source>,
}

fn cited(paras: &[Paragraph]) -> impl Iterator<Item = &str> {
    paras.iter().flat_map(|p| &p.runs).filter_map(|r| match &r.attrs.field.as_deref()?.kind {
        FieldKind::Citation(tag) => Some(tag.as_str()),
        _ => None,
    })
}

/// Gives text that looked like `s` (in font `font`) and now looks like `t`
/// (in font `now_font`) the difference as its own formatting. What it
/// can't say (not bold under a bold style, no color) stays as `t` has it.
fn keep_look(a: &mut Attrs, s: CharFormat, font: Option<Arc<str>>, t: &CharFormat, now_font: Option<Arc<str>>) {
    if s.size != t.size {
        a.size = Some(s.size);
    }
    if font.is_some() && font != now_font {
        a.font = font;
    }
    if s.color.is_some() && s.color != t.color {
        a.color = s.color;
    }
    a.bold |= s.bold && !t.bold;
    a.italic |= s.italic && !t.italic;
    a.underline |= s.underline && !t.underline;
    if s.fx != t.fx {
        a.fx = Some(Arc::new(s.fx));
    }
}

impl Origin {
    /// `paras` as plain paragraphs whose text says all of its look itself
    /// (font, size, color, bold...), for where the document's styles
    /// can't go: the text of a slide. Pictures, notes and other objects
    /// in the text stay behind, and so do tables: their text comes alone.
    pub fn resolved(&self, paras: &[Paragraph]) -> Vec<Paragraph> {
        let mut src = Document::new();
        src.styles = self.styles.clone();
        src.default_font = self.default_font.clone();
        let mut out = Vec::with_capacity(paras.len());
        for p in paras {
            let mut q = Paragraph::new("");
            q.style.level = p.style.level;
            for (a, b, attrs) in p.spans() {
                let objects = attrs.image.is_some() || attrs.note.is_some() || attrs.field.is_some() || attrs.math.is_some() || attrs.shape.is_some() || attrs.control.is_some();
                let Some(text) = p.text.get(a..b).filter(|_| !objects) else { continue };
                let f = src.char_format(&p.style, attrs);
                let font = f.font.clone().or_else(|| src.default_font_name());
                let font = font.map(|n| src.styles.theme.token_font(&n).cloned().unwrap_or(n));
                let look = Attrs { size: Some(f.size), font, color: f.color, bold: f.bold, italic: f.italic, underline: f.underline, char_style: None, rev: None, ..attrs.clone() };
                q.push(text, look);
            }
            out.push(q);
        }
        out
    }
}

impl Document {
    /// The text of `p` is about to join a paragraph of style `now`: what
    /// it took from its own paragraph's style (a heading's size and bold)
    /// becomes its own formatting, so it looks as it did. Text whose look
    /// can't be kept whole (plain text going into a heading can't be said
    /// not to be bold) takes the look of where it goes.
    pub(crate) fn keep_look_in(&self, p: &mut Paragraph, now: &ParaStyle) {
        if (p.style.kind, p.style.custom) == (now.kind, now.custom) {
            return;
        }
        let was = p.style;
        for r in &mut p.runs {
            let (s, t) = (self.char_format(&was, &r.attrs), self.char_format(now, &r.attrs));
            if (t.bold && !s.bold) || (t.italic && !s.italic) || (t.underline && !s.underline) || (t.color.is_some() && s.color.is_none()) {
                continue;
            }
            let (font, now_font) = (s.font.clone(), t.font.clone());
            keep_look(&mut r.attrs, s, font, &t, now_font);
        }
        p.normalize();
    }

    /// What `paras`, copied from this document, need from it.
    pub fn origin(&self, paras: &[Paragraph]) -> Origin {
        let mut sources: Vec<Source> = Vec::new();
        if !self.biblio.sources.is_empty() {
            for tag in cited(paras) {
                if let Some(s) = self.biblio.source(tag).filter(|s| !sources.iter().any(|x| x.tag == s.tag)) {
                    sources.push(s.clone());
                }
            }
        }
        Origin { styles: self.styles.clone(), default_font: self.default_font.clone(), sources }
    }

    /// `paras` copied from a document (`from`) as paragraphs of this one,
    /// looking as they did there: styles, lists, table styles and sources
    /// this document lacks are added to it; where a style of the same
    /// name differs, or the documents' font and size do, the text takes
    /// the difference as its own formatting.
    pub fn adopt(&mut self, paras: &[Paragraph], from: &Origin) -> Vec<Paragraph> {
        let mut out = paras.to_vec();
        for s in &from.sources {
            if self.biblio.source(&s.tag).is_none() && cited(paras).any(|t| t.eq_ignore_ascii_case(&s.tag)) {
                self.biblio.sources.push(s.clone());
            }
        }
        if from.styles == self.styles && from.default_font == self.default_font {
            return out;
        }
        let mut src = Document::new();
        src.styles = from.styles.clone();
        src.default_font = from.default_font.clone();
        let (mut customs, mut lists): (HashMap<u16, u16>, HashMap<u16, u16>) = Default::default();
        for p in &mut out {
            let was = p.style;
            if was.custom != 0 {
                p.style.custom = *customs.entry(was.custom).or_insert_with(|| self.adopt_para_style(&src.styles, was.custom));
            }
            if was.list != 0 {
                p.style.list = *lists.entry(was.list).or_insert_with(|| self.adopt_list(&src.styles, was.list));
            }
            if was.line_spacing == 0.0 {
                let line = src.styles.default_line(&was);
                if (line - self.styles.default_line(&p.style)).abs() > 0.005 {
                    (p.style.line_spacing, p.style.line_rule) = (line, LineRule::Multiple);
                }
            }
            let mut cell = p.cell.as_ref();
            while let Some(c) = cell {
                self.adopt_table_style(&src.styles, c);
                cell = c.nested.as_deref();
            }
            let style = p.style;
            for r in &mut p.runs {
                if let Some(id) = r.attrs.char_style.clone() {
                    self.adopt_char_style(&src.styles, &id);
                }
                let (s, t) = (src.char_format(&was, &r.attrs), self.char_format(&style, &r.attrs));
                let font = s.font.clone().or_else(|| src.default_font_name());
                let now_font = t.font.clone().or_else(|| self.default_font_name());
                keep_look(&mut r.attrs, s, font, &t, now_font);
            }
            p.normalize();
        }
        out
    }

    /// The number here of the custom paragraph style `n` of `src`: the
    /// style of that id, else a copy of it.
    fn adopt_para_style(&mut self, src: &Styles, n: u16) -> u16 {
        let Some(def) = src.custom(n) else { return 0 };
        if let Some(m) = self.styles.custom_number(&def.id) {
            return m;
        }
        let mut def = def.clone();
        // The id may be taken by a style of another type.
        if self.styles.by_id(&def.id).is_some() || builtin_id(&def.id).is_some() {
            def.id = self.styles.new_id(&def.name);
        }
        self.styles.defs.push(def);
        self.styles.defs.len() as u16
    }

    /// The number here of the list `n` of `src`.
    fn adopt_list(&mut self, src: &Styles, n: u16) -> u16 {
        let Some(def) = src.lists.get((n as usize).wrapping_sub(1)) else { return 0 };
        let at = self.styles.lists.iter().position(|l| l == def).unwrap_or_else(|| {
            self.styles.lists.push(def.clone());
            self.styles.lists.len() - 1
        });
        at as u16 + 1
    }

    /// Copies the character style `id` of `src` (or the paragraph style
    /// text is linked to) when this document has no style of that id.
    fn adopt_char_style(&mut self, src: &Styles, id: &str) {
        if self.styles.by_id(id).is_some() || builtin_id(id).is_some() {
            return;
        }
        if let Some(def) = src.by_id(id).filter(|d| !d.builtin) {
            self.styles.defs.push(def.clone());
        }
    }

    /// Copies the table style of a table of `src` this document lacks.
    fn adopt_table_style(&mut self, src: &Styles, cell: &InCell) {
        let Some(id) = cell.table_props.as_ref().and_then(|t| t.style.as_deref()) else { return };
        if table_style(id).is_some() || self.styles.tables.iter().any(|t| t.id == id) {
            return;
        }
        if let Some(t) = src.tables.iter().find(|t| t.id == id) {
            self.styles.tables.push(t.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attrs, ParaKind, ParaStyle, Pos, Selection, StyleDef};

    fn sel0() -> Selection {
        Selection::caret(Pos::default())
    }

    /// What the text at `at` looks like: size, bold, font, color.
    fn look(d: &Document, para: usize, at: usize) -> (u16, bool, Option<Arc<str>>, Option<u32>) {
        let p = &d.paragraphs[para];
        let f = d.char_format(&p.style, &p.attrs_at(at));
        (f.size, f.bold, f.font.or_else(|| d.default_font_name()), f.color)
    }

    #[test]
    fn same_styles_pass_as_they_are() {
        let a = Document::from_paragraphs(vec![Paragraph::new("one"), Paragraph::new("two")]);
        let mut b = Document::new();
        let paras = a.slice(Pos::new(0, 0), Pos::new(1, 3));
        assert_eq!(b.adopt(&paras, &a.origin(&paras)), paras);
        assert!(b.styles.is_empty());
    }

    #[test]
    fn text_keeps_its_look_in_another_document() {
        let mut a = Document::from_paragraphs(vec![Paragraph::new("Heading").with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() }), Paragraph::new("body text")]);
        a.default_font = Some("Georgia".into());
        a.set_style(StyleDef { id: "Normal".into(), name: "Normal".into(), builtin: true, size: Some(21), ..Default::default() }, sel0());
        a.set_style(StyleDef { id: "Heading1".into(), name: "Heading 1".into(), kind: ParaKind::Heading(1), builtin: true, size: Some(30), color: Some(0x2F5496), ..Default::default() }, sel0());
        a.format_range(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 4) }, &|at: &mut Attrs| at.bold = true);
        let paras = a.slice(Pos::new(0, 0), Pos::new(1, 9));
        let origin = a.origin(&paras);

        let mut b = Document::from_paragraphs(vec![Paragraph::new("")]);
        b.default_font = Some("Arial".into());
        let adopted = b.adopt(&paras, &origin);
        b.insert_paragraphs(sel0(), &adopted);
        for (para, at) in [(0, 1), (1, 1), (1, 6)] {
            assert_eq!(look(&b, para, at), look(&a, para, at), "paragraph {para} at {at}");
        }
        assert_eq!(look(&b, 1, 1), (21, true, Some("Georgia".into()), None));
        // The built-in styles of the document it went to are its own.
        assert!(b.styles.is_empty());
    }

    #[test]
    fn styles_and_lists_come_along() {
        let mut a = Document::from_paragraphs(vec![Paragraph::new("note"), Paragraph::new("item"), Paragraph::new("word")]);
        a.set_style(StyleDef { id: "Other".into(), name: "Other".into(), ..Default::default() }, sel0());
        a.set_style(StyleDef { id: "MyNote".into(), name: "My Note".into(), italic: Some(true), size: Some(30), space_after: Some(20.0), ..Default::default() }, sel0());
        a.set_style(StyleDef { id: "Strong".into(), name: "Strong".into(), char_style: true, bold: Some(true), color: Some(0xC00000), ..Default::default() }, sel0());
        let n = a.styles.custom_number("MyNote").unwrap();
        a.apply_para_style(0, 0, sel0(), ParaKind::Normal, n);
        a.styles.lists.push(crate::ListDef::default());
        a.styles.lists.push(crate::ListDef::presets()[0].clone());
        a.paragraphs[1].style.kind = ParaKind::Numbered;
        a.paragraphs[1].style.list = 2;
        a.apply_char_style(Selection { anchor: Pos::new(2, 0), cursor: Pos::new(2, 4) }, Some("Strong".into()));
        let paras = a.slice(Pos::new(0, 0), Pos::new(2, 4));
        let origin = a.origin(&paras);

        let mut b = Document::from_paragraphs(vec![Paragraph::new("")]);
        // A style of the same id that looks different: the text keeps its look.
        b.set_style(StyleDef { id: "Strong".into(), name: "Strong".into(), char_style: true, bold: Some(true), ..Default::default() }, sel0());
        b.set_style(StyleDef { id: "Em".into(), name: "Em".into(), char_style: true, italic: Some(true), ..Default::default() }, sel0());
        let adopted = b.adopt(&paras, &origin);
        b.insert_paragraphs(sel0(), &adopted);
        let m = b.styles.custom_number("MyNote").unwrap();
        assert_ne!(m, n);
        assert_eq!(b.paragraphs[0].style.custom, m);
        assert_eq!(b.paragraphs[0].style.space_after, Some(20.0));
        assert_eq!(look(&b, 0, 1), look(&a, 0, 1));
        assert_eq!(b.paragraphs[1].style.list, 1);
        assert_eq!(b.styles.lists, vec![crate::ListDef::presets()[0].clone()]);
        assert_eq!(look(&b, 2, 1), look(&a, 2, 1));
        assert_eq!(b.styles.char_styles().count(), 2);
        // Pasted again, nothing more is added.
        let (defs, again) = (b.styles.defs.len(), b.adopt(&paras, &origin));
        assert_eq!((b.styles.defs.len(), b.styles.lists.len()), (defs, 1));
        assert_eq!(again, adopted);
    }

    #[test]
    fn a_heading_pasted_into_a_paragraph_keeps_its_look() {
        let h = ParaStyle { kind: ParaKind::Heading(1), ..Default::default() };
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Heading").with_style(h), Paragraph::new("body")]);
        let paras = d.slice(Pos::new(0, 0), Pos::new(0, 7));
        let end = d.insert_paragraphs(Selection::caret(Pos::new(1, 4)), &paras);
        assert_eq!((d.paragraphs[1].text.as_str(), d.paragraphs[1].style.kind), ("bodyHeading", ParaKind::Normal));
        assert_eq!(look(&d, 1, 6), look(&d, 0, 1));
        assert_eq!(look(&d, 1, 1).0, 24);
        // Plain text into a heading can't be told not to be bold: it
        // takes the heading's look, whole.
        let paras = d.slice(Pos::new(1, 0), Pos::new(1, 4));
        d.insert_paragraphs(Selection::caret(Pos::new(0, 7)), &paras);
        assert_eq!(look(&d, 0, 9), look(&d, 0, 1));
        // Into an empty paragraph it goes with its style, as it is.
        d.undo();
        d.undo();
        let at = d.new_paragraph(Selection::caret(end.min(Pos::new(1, 4))));
        let paras = d.slice(Pos::new(0, 0), Pos::new(0, 7));
        d.insert_paragraphs(Selection::caret(at), &paras);
        assert_eq!(d.paragraphs[at.para].style.kind, ParaKind::Heading(1));
        assert_eq!(d.paragraphs[at.para].attrs_at(1), Attrs::default());
    }

    #[test]
    fn text_says_its_whole_look_for_a_slide() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Title").with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() }), Paragraph::new("plain")]);
        d.default_font = Some("Georgia".into());
        d.format_range(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 2) }, &|a: &mut Attrs| a.italic = true);
        let want = d.char_format(&d.paragraphs[0].style, &Attrs::default());
        let out = d.origin(&d.paragraphs).resolved(&d.paragraphs);
        let h = out[0].attrs_at(1);
        assert_eq!((out[0].style, h.size, h.bold, h.font.as_deref()), (ParaStyle::default(), Some(want.size), want.bold, Some("Georgia")));
        assert!(out[1].attrs_at(1).italic && !out[1].attrs_at(4).italic);
        assert_eq!(out[1].text, "plain");
    }

    #[test]
    fn cited_sources_come_along() {
        let mut a = Document::from_paragraphs(vec![Paragraph::new("see")]);
        a.biblio.sources.push(Source { tag: "Doe20".into(), title: "A book".into(), ..Default::default() });
        a.biblio.sources.push(Source { tag: "Roe21".into(), ..Default::default() });
        let at = a.insert_field(Selection::caret(Pos::new(0, 3)), crate::Field::new(FieldKind::Citation("Doe20".into())));
        let paras = a.slice(Pos::new(0, 0), at.end());
        let origin = a.origin(&paras);
        let mut b = Document::new();
        b.adopt(&paras, &origin);
        assert_eq!(b.biblio.sources.iter().map(|s| s.tag.as_str()).collect::<Vec<_>>(), ["Doe20"]);
    }
}
