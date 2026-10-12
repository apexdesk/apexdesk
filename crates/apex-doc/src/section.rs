//! Page structure: sections (page setup and text columns per section,
//! each started by a section break), column breaks, and the page
//! decoration (color, borders and watermark).
//!
//! The first section's setup is `Document::page` and `Document::columns`;
//! each later section starts at a paragraph carrying a `Section` (like a
//! page break before it), so sections follow edits and undo like any other
//! paragraph property. Headers and footers are document-wide.

use std::sync::Arc;

use crate::{Document, EditKind, Headers, Line, PageSetup, Paragraph, Pos, Selection};

/// How a section starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SectionStart {
    #[default]
    NextPage,
    Continuous,
    EvenPage,
    OddPage,
}

/// Most text columns a section can have.
pub const MAX_COLUMNS: usize = 12;
/// Default space between columns, in points (half an inch).
pub const COLUMN_GAP: f32 = 36.0;

/// Text columns of a section.
#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    /// Number of columns (1 = no columns).
    pub count: u8,
    /// Space between equal columns, in points.
    pub spacing: f32,
    /// A line between the columns.
    pub separator: bool,
    /// Custom widths: each column's width and the space after it, in
    /// points. Empty for equal columns.
    pub custom: Vec<(f32, f32)>,
}

impl Default for Columns {
    fn default() -> Self {
        Columns { count: 1, spacing: COLUMN_GAP, separator: false, custom: Vec::new() }
    }
}

impl Columns {
    pub fn equal(count: u8, spacing: f32) -> Columns {
        Columns { count, spacing, ..Default::default() }
    }

    /// The number of columns, within limits.
    pub fn n(&self) -> usize {
        if self.custom.len() > 1 { self.custom.len().min(MAX_COLUMNS) } else { (self.count as usize).clamp(1, MAX_COLUMNS) }
    }

    pub fn is_single(&self) -> bool {
        self.n() == 1
    }

    /// Left edge and width of each column, in points from the text area's
    /// left edge, for a text area `width` wide. Custom widths are scaled to
    /// fill the width.
    pub fn layout(&self, width: f32) -> Vec<(f32, f32)> {
        let n = self.n();
        let width = width.max(1.0);
        if n == 1 {
            return vec![(0.0, width)];
        }
        let sane = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
        let parts: Vec<(f32, f32)> = if self.custom.len() > 1 {
            self.custom.iter().take(n).enumerate().map(|(i, &(w, g))| (sane(w).max(1.0), if i + 1 < n { sane(g) } else { 0.0 })).collect()
        } else {
            let gap = sane(self.spacing).min(width / n as f32 * 0.8);
            let w = (width - gap * (n - 1) as f32) / n as f32;
            (0..n).map(|i| (w, if i + 1 < n { gap } else { 0.0 })).collect()
        };
        let total: f32 = parts.iter().map(|(w, g)| w + g).sum();
        let k = if total > 0.0 { width / total } else { 1.0 };
        let mut x = 0.0;
        parts
            .into_iter()
            .map(|(w, g)| {
                let c = (x, (w * k).max(1.0));
                x += (w + g) * k;
                c
            })
            .collect()
    }
}

/// The setup of a section after the first.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Section {
    pub start: SectionStart,
    pub page: PageSetup,
    pub columns: Columns,
    pub headers: Headers,
}

/// A section of the document: its paragraphs and setup.
#[derive(Clone, Copy, Debug)]
pub struct SectionRef<'a> {
    /// First paragraph and end (exclusive).
    pub first: usize,
    pub end: usize,
    pub start: SectionStart,
    pub page: &'a PageSetup,
    pub columns: &'a Columns,
}

/// Page borders: lines around the text (or near the page edges).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PageBorders {
    pub top: Option<Line>,
    pub bottom: Option<Line>,
    pub left: Option<Line>,
    pub right: Option<Line>,
    /// `Line::space` is measured from the page edge instead of the text.
    pub from_edge: bool,
}

impl PageBorders {
    pub fn is_empty(&self) -> bool {
        self.top.is_none() && self.bottom.is_none() && self.left.is_none() && self.right.is_none()
    }
}

/// Text or a picture behind the text of every page.
#[derive(Clone, Debug, PartialEq)]
pub struct Watermark {
    pub text: String,
    pub font: Option<Arc<str>>,
    /// Font size in points; `None` fits the page.
    pub size: Option<f32>,
    pub color: u32,
    pub semitransparent: bool,
    /// Diagonal (bottom left to top right) or horizontal.
    pub diagonal: bool,
    /// A picture instead of text.
    pub picture: Option<Arc<[u8]>>,
    /// Picture scale (1 = its natural size); 0 fits the page.
    pub scale: f32,
    /// Lightened picture.
    pub washout: bool,
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark { text: String::new(), font: None, size: None, color: 0xC0C0C0, semitransparent: true, diagonal: true, picture: None, scale: 0.0, washout: true }
    }
}

impl Watermark {
    pub fn text(s: &str) -> Watermark {
        Watermark { text: s.into(), ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.picture.is_none()
    }
}

/// Page color, borders and watermark (document-wide).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PageDeco {
    pub color: Option<u32>,
    /// A gradient over the page, drawn in place of `color` (which stands
    /// for it where gradients aren't drawn).
    pub gradient: Option<crate::Gradient>,
    pub borders: PageBorders,
    pub watermark: Option<Watermark>,
}

impl PageDeco {
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.borders.is_empty() && self.watermark.as_ref().is_none_or(Watermark::is_empty)
    }
}

/// Document-wide page settings, saved in undo steps that change them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PageState {
    page: PageSetup,
    columns: Columns,
    deco: PageDeco,
    headers: Headers,
    odd_even: bool,
    fonts: crate::EmbeddedFonts,
    auto_hyphenation: bool,
    /// The sources and the citation style.
    biblio: crate::Bibliography,
    default_font: Option<std::sync::Arc<str>>,
}

/// A break inserted at the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakKind {
    Page,
    Column,
    Section(SectionStart),
}

/// Which sections a setup change applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Whole,
    /// A section by index.
    Section(usize),
    /// From a section to the end.
    From(usize),
}

impl Paragraph {
    /// The section this paragraph starts (ignored in the first paragraph
    /// and in tables).
    pub fn section_start(&self) -> Option<&Section> {
        self.section.as_deref().filter(|_| self.cell.is_none())
    }
}

impl Document {
    pub(crate) fn page_state(&self) -> PageState {
        PageState { page: self.page, columns: self.columns.clone(), deco: self.deco.clone(), headers: self.headers.clone(), odd_even: self.odd_even, fonts: self.fonts.clone(), auto_hyphenation: self.auto_hyphenation, biblio: self.biblio.clone(), default_font: self.default_font.clone() }
    }

    /// Puts back saved page settings, returning the current ones.
    pub(crate) fn swap_page_state(&mut self, s: PageState) -> PageState {
        let cur = self.page_state();
        self.page = s.page;
        self.columns = s.columns;
        self.deco = s.deco;
        self.headers = s.headers;
        self.odd_even = s.odd_even;
        self.fonts = s.fonts;
        self.auto_hyphenation = s.auto_hyphenation;
        self.biblio = s.biblio;
        self.default_font = s.default_font;
        cur
    }

    /// Paragraphs that start a section (after the first one).
    fn section_starts(&self) -> impl Iterator<Item = (usize, &Section)> {
        self.paragraphs.iter().enumerate().skip(1).filter_map(|(i, p)| p.section_start().map(|s| (i, s)))
    }

    /// The sections, in order; there is always at least one.
    pub fn sections(&self) -> Vec<SectionRef<'_>> {
        let mut out = vec![SectionRef { first: 0, end: self.paragraphs.len(), start: SectionStart::NextPage, page: &self.page, columns: &self.columns }];
        for (i, s) in self.section_starts() {
            if let Some(last) = out.last_mut() {
                last.end = i;
            }
            out.push(SectionRef { first: i, end: self.paragraphs.len(), start: s.start, page: &s.page, columns: &s.columns });
        }
        out
    }

    /// Whether the document has more than one section.
    /// Changes the page setup of every section (not an undo step: for
    /// readers).
    pub fn set_all_pages(&mut self, f: impl Fn(&mut PageSetup)) {
        f(&mut self.page);
        for s in self.paragraphs.iter_mut().filter_map(|p| p.section.as_deref_mut()) {
            f(&mut s.page);
        }
    }

    pub fn has_sections(&self) -> bool {
        self.section_starts().next().is_some()
    }

    /// Index of the section holding paragraph `para`.
    pub fn section_index(&self, para: usize) -> usize {
        self.section_starts().take_while(|(i, _)| *i <= para).count()
    }

    /// The page setup and columns of the section holding paragraph `para`.
    pub fn section_setup(&self, para: usize) -> (&PageSetup, &Columns) {
        match self.section_starts().take_while(|(i, _)| *i <= para).last() {
            Some((_, s)) => (&s.page, &s.columns),
            None => (&self.page, &self.columns),
        }
    }

    /// Inserts a page, column or section break at the selection, as one
    /// undo step; returns the caret (at the start of the text after it).
    /// Column and section breaks aren't allowed in tables.
    pub fn insert_break(&mut self, sel: Selection, kind: BreakKind) -> Option<Pos> {
        let at = sel.start().para;
        if kind != BreakKind::Page && (self.in_table(at) || self.in_table(sel.end().para)) {
            return None;
        }
        let (page, columns) = {
            let (p, c) = self.section_setup(at);
            (*p, c.clone())
        };
        let caret = self.insert_text(sel, "\n", None);
        let p = &mut self.paragraphs[caret.para];
        match kind {
            BreakKind::Page => p.style.page_break_before = true,
            BreakKind::Column => p.style.column_break_before = true,
            BreakKind::Section(start) => p.section = Some(Box::new(Section { start, page, columns, headers: Headers::default() })),
        }
        // The new paragraph is inside the step just recorded.
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Paragraph;
        }
        Some(caret)
    }

    /// Paragraph ranges of the sections a scope covers.
    fn scope_sections(&self, scope: Scope) -> Vec<usize> {
        let n = self.sections().len();
        match scope {
            Scope::Whole => (0..n).collect(),
            Scope::Section(i) => vec![i.min(n - 1)],
            Scope::From(i) => (i.min(n - 1)..n).collect(),
        }
    }

    /// Changes the page setup and columns of some sections as one undo step.
    pub fn change_sections(&mut self, scope: Scope, sel: Selection, f: &dyn Fn(&mut PageSetup, &mut Columns)) {
        self.change_sections_each(scope, sel, &|_, p, c| f(p, c));
    }

    /// Like [`Document::change_sections`], `f` told which section (by
    /// index) it changes: one undo step for changes that differ by section.
    pub fn change_sections_each(&mut self, scope: Scope, sel: Selection, f: &dyn Fn(usize, &mut PageSetup, &mut Columns)) {
        let which = self.scope_sections(scope);
        let starts: Vec<usize> = self.sections().iter().map(|s| s.first).collect();
        let paras: Vec<usize> = which.iter().filter(|&&i| i > 0).map(|&i| starts[i]).collect();
        self.change_layout(paras.first().zip(paras.last()).map(|(a, b)| (*a, *b)), sel, EditKind::Paragraph, &mut |d| {
            if which.contains(&0) {
                f(0, &mut d.page, &mut d.columns);
            }
            for (&i, &at) in which.iter().filter(|&&i| i > 0).zip(&paras) {
                if let Some(s) = d.paragraphs[at].section.as_deref_mut() {
                    f(i, &mut s.page, &mut s.columns);
                }
            }
        });
    }

    /// Sets how section `index` (after the first) starts.
    pub fn set_section_start(&mut self, index: usize, start: SectionStart, sel: Selection) {
        let Some(first) = self.sections().get(index).filter(|_| index > 0).map(|s| s.first) else { return };
        self.change_layout(Some((first, first)), sel, EditKind::Paragraph, &mut |d| {
            if let Some(s) = d.paragraphs[first].section.as_deref_mut() {
                s.start = start;
            }
        });
    }

    /// Removes the section break before section `index` (the sections
    /// merge, keeping the first one's setup).
    pub fn remove_section_break(&mut self, index: usize, sel: Selection) {
        let Some(first) = self.sections().get(index).filter(|_| index > 0).map(|s| s.first) else { return };
        self.change_layout(Some((first, first)), sel, EditKind::Paragraph, &mut |d| d.paragraphs[first].section = None);
    }

    /// Changes the page color, borders or watermark as one undo step.
    pub fn set_page_deco(&mut self, deco: PageDeco, sel: Selection) {
        if deco == self.deco {
            return;
        }
        self.change_layout(None, sel, EditKind::Format, &mut |d| d.deco = deco.clone());
    }

    /// Runs `f`, which may change paragraphs `range` (not their number) and
    /// the document-wide page settings, and records it as one undo step.
    pub fn change_layout(&mut self, range: Option<(usize, usize)>, sel: Selection, kind: EditKind, f: &mut dyn FnMut(&mut Document)) {
        let state = self.page_state();
        let (start, before) = match range {
            Some((a, b)) if a <= b && b < self.paragraphs.len() => (a, self.paragraphs[a..=b].to_vec()),
            _ => (0, Vec::new()),
        };
        let n = before.len();
        f(self);
        self.record(start, before, n, sel, sel, kind);
        if let Some(e) = self.undo.last_mut() {
            e.page = Some(Box::new(state));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_layout() {
        let c = Columns::equal(2, 36.0);
        assert_eq!(c.layout(436.0), vec![(0.0, 200.0), (236.0, 200.0)]);
        assert_eq!(Columns::default().layout(300.0), vec![(0.0, 300.0)]);
        // Custom widths fill the text area.
        let c = Columns { custom: vec![(100.0, 20.0), (280.0, 0.0)], ..Default::default() };
        let l = c.layout(800.0);
        assert_eq!(l.len(), 2);
        assert!((l[1].0 + l[1].1 - 800.0).abs() < 0.01);
        assert!((l[0].1 - 200.0).abs() < 0.01);
        // Nonsense stays usable.
        let c = Columns { count: 200, spacing: f32::NAN, ..Default::default() };
        assert_eq!(c.layout(100.0).len(), MAX_COLUMNS);
    }

    #[test]
    fn sections_and_breaks() {
        let mut d = Document::from_plain_text("uno\ndos\ntres");
        assert_eq!(d.sections().len(), 1);
        let caret = d.insert_break(Selection::caret(Pos::new(1, 1)), BreakKind::Section(SectionStart::Continuous)).unwrap();
        assert_eq!(caret, Pos::new(2, 0));
        assert_eq!(d.paragraphs[2].text, "os");
        let s = d.sections();
        assert_eq!(s.len(), 2);
        assert_eq!((s[1].first, s[1].end, s[1].start), (2, 4, SectionStart::Continuous));
        assert_eq!(d.section_index(3), 1);
        assert_eq!(d.section_index(1), 0);
        // Landscape for the second section only.
        d.change_sections(Scope::Section(1), Selection::default(), &|p, c| {
            p.set_landscape(true);
            c.count = 2;
        });
        assert!(d.section_setup(3).0.landscape() && !d.page.landscape());
        assert_eq!(d.section_setup(3).1.n(), 2);
        d.change_sections(Scope::Whole, Selection::default(), &|p, _| p.margin_left = 10.0);
        assert!(d.sections().iter().all(|s| s.page.margin_left == 10.0));
        d.undo();
        assert!(d.sections().iter().all(|s| s.page.margin_left != 10.0));
        d.undo();
        assert!(!d.section_setup(3).0.landscape());
        d.undo();
        assert_eq!(d.sections().len(), 1);
        assert_eq!(d.to_plain_text(), "uno\ndos\ntres");
        d.redo();
        assert_eq!(d.sections().len(), 2);
        // Column break.
        let c = d.insert_break(Selection::caret(Pos::new(0, 3)), BreakKind::Column).unwrap();
        assert!(d.paragraphs[c.para].style.column_break_before);
        // Page decoration undo.
        d.set_page_deco(PageDeco { color: Some(0xFFEEDD), ..Default::default() }, Selection::default());
        assert_eq!(d.deco.color, Some(0xFFEEDD));
        d.undo();
        assert_eq!(d.deco.color, None);
        d.redo();
        assert_eq!(d.deco.color, Some(0xFFEEDD));
    }

    #[test]
    fn no_section_breaks_in_tables() {
        let mut d = Document::from_plain_text("a");
        let s = d.insert_table(Selection::caret(Pos::new(0, 1)), 2, 2, 400.0);
        let at = s.start();
        assert!(d.in_table(at.para));
        assert!(d.insert_break(Selection::caret(at), BreakKind::Section(SectionStart::NextPage)).is_none());
        assert!(d.insert_break(Selection::caret(at), BreakKind::Column).is_none());
    }
}
