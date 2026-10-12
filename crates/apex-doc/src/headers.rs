//! Headers and footers of documents: formatted paragraphs (with pictures,
//! tables and fields) per section, with their own first page and even
//! pages. A section without a header or footer of some type uses the
//! previous section's ("linked to previous"), as in Word.

use std::sync::Arc;

use crate::{Document, EditKind, NoteFormat, Paragraph, Selection};

/// The paragraphs of one header or footer.
pub type Story = Arc<Vec<Paragraph>>;

/// Which header or footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HfSlot {
    Header,
    Footer,
    FirstHeader,
    FirstFooter,
    EvenHeader,
    EvenFooter,
}

impl HfSlot {
    pub const ALL: [HfSlot; 6] = [HfSlot::Header, HfSlot::Footer, HfSlot::FirstHeader, HfSlot::FirstFooter, HfSlot::EvenHeader, HfSlot::EvenFooter];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn is_header(self) -> bool {
        matches!(self, HfSlot::Header | HfSlot::FirstHeader | HfSlot::EvenHeader)
    }

    /// 0 every page (odd pages), 1 first page, 2 even pages.
    pub fn kind(self) -> u8 {
        self as u8 / 2
    }

    pub fn of(header: bool, kind: u8) -> HfSlot {
        HfSlot::ALL[kind.min(2) as usize * 2 + usize::from(!header)]
    }

    /// The same kind of page, header or footer.
    pub fn with_header(self, header: bool) -> HfSlot {
        HfSlot::of(header, self.kind())
    }
}

/// The headers and footers of one section. In sections after the first,
/// `None` takes the previous section's; in the first one it's empty.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Headers {
    pub parts: [Option<Story>; 6],
    /// The section's first page has its own header and footer.
    pub different_first: bool,
    /// How the section numbers its pages.
    pub numbering: PageNumbering,
}

/// How a section numbers its pages: the format and, when it restarts, the
/// number of its first page (otherwise it goes on from the previous one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PageNumbering {
    pub format: NoteFormat,
    pub start: Option<u32>,
}

impl Headers {
    pub fn get(&self, slot: HfSlot) -> Option<&Story> {
        self.parts[slot.index()].as_ref()
    }

    pub fn set(&mut self, slot: HfSlot, story: Option<Story>) {
        self.parts[slot.index()] = story;
    }
}

/// Whether a header or footer shows nothing (a table, even empty, shows).
pub fn story_is_empty(s: &[Paragraph]) -> bool {
    s.iter().all(|p| p.text.is_empty() && p.cell.is_none() && p.style.shading.is_none() && p.style.borders.is_empty())
}

impl Document {
    /// The headers and footers stored in section `s` (not resolved).
    pub fn section_headers(&self, s: usize) -> &Headers {
        if s == 0 {
            return &self.headers;
        }
        let first = self.sections().get(s).map(|r| r.first);
        match first.and_then(|i| self.paragraphs[i].section_start()) {
            Some(sec) => &sec.headers,
            None => &self.headers,
        }
    }

    fn section_headers_mut(&mut self, s: usize) -> &mut Headers {
        let first = if s == 0 { None } else { self.sections().get(s).map(|r| r.first) };
        match first {
            Some(i) if self.paragraphs[i].section_start().is_some() => &mut self.paragraphs[i].section.as_deref_mut().expect("section").headers,
            _ => &mut self.headers,
        }
    }

    /// The header or footer section `s` shows for `slot`, following links
    /// to previous sections.
    pub fn hf_story(&self, s: usize, slot: HfSlot) -> Option<&Story> {
        (0..=s).rev().find_map(|i| self.section_headers(i).get(slot))
    }

    /// Whether section `s` takes `slot` from the previous section.
    pub fn hf_linked(&self, s: usize, slot: HfSlot) -> bool {
        s > 0 && self.section_headers(s).get(slot).is_none()
    }

    /// The header and footer slots of a page: `first` when it's the first
    /// page of its section, `number` its page number (from 1).
    pub fn page_slots(&self, s: usize, first: bool, number: usize) -> (HfSlot, HfSlot) {
        let kind = if first && self.section_headers(s).different_first {
            1
        } else if self.odd_even && number.is_multiple_of(2) {
            2
        } else {
            0
        };
        (HfSlot::of(true, kind), HfSlot::of(false, kind))
    }

    /// The number each page shows, for pages given by their section and
    /// whether they're its first page.
    pub fn page_labels(&self, pages: impl IntoIterator<Item = (usize, bool)>) -> Vec<String> {
        let mut n = 0usize;
        pages
            .into_iter()
            .map(|(s, first)| {
                let num = self.section_headers(s).numbering;
                n = match num.start {
                    Some(v) if first => v as usize,
                    _ => n + 1,
                };
                // Roman numerals and letters start at 1.
                if n == 0 && num.format != NoteFormat::Arabic { String::new() } else if num.format == NoteFormat::Arabic { n.to_string() } else { num.format.label(n) }
            })
            .collect()
    }

    /// Sets how section `s` numbers its pages as one undo step.
    pub fn set_page_numbering(&mut self, s: usize, numbering: PageNumbering, sel: Selection) {
        if self.section_headers(s).numbering == numbering {
            return;
        }
        let range = self.section_para(s).map(|i| (i, i));
        self.change_layout(range, sel, EditKind::Format, &mut |d| d.section_headers_mut(s).numbering = numbering);
    }

    /// Whether any section has a header or footer with something in it.
    pub fn has_headers(&self) -> bool {
        (0..self.sections().len()).any(|s| HfSlot::ALL.iter().any(|&slot| self.section_headers(s).get(slot).is_some_and(|st| !story_is_empty(st))))
    }

    /// Sets (or links to the previous section, with `None`) a header or
    /// footer of section `s` as one undo step.
    pub fn set_hf_story(&mut self, s: usize, slot: HfSlot, story: Option<Vec<Paragraph>>, sel: Selection) {
        let story = story.map(Arc::new);
        if self.section_headers(s).get(slot) == story.as_ref() {
            return;
        }
        let range = self.section_para(s).map(|i| (i, i));
        self.change_layout(range, sel, EditKind::Format, &mut |d| d.section_headers_mut(s).set(slot, story.clone()));
    }

    /// Turns on or off a different first page (for section `s`) and
    /// different even pages (for the document) as one undo step.
    pub fn set_hf_options(&mut self, s: usize, different_first: bool, odd_even: bool, sel: Selection) {
        if self.section_headers(s).different_first == different_first && self.odd_even == odd_even {
            return;
        }
        let range = self.section_para(s).map(|i| (i, i));
        self.change_layout(range, sel, EditKind::Format, &mut |d| {
            d.section_headers_mut(s).different_first = different_first;
            d.odd_even = odd_even;
        });
    }

    /// The paragraph holding section `s`'s setup (none for the first).
    fn section_para(&self, s: usize) -> Option<usize> {
        if s == 0 {
            return None;
        }
        self.sections().get(s).map(|r| r.first).filter(|&i| self.paragraphs[i].section_start().is_some())
    }

    /// Replaces every header and footer with plain paragraphs (for the
    /// first section; the others link to it).
    pub fn set_simple_headers(&mut self, header: Option<Vec<Paragraph>>, footer: Option<Vec<Paragraph>>) {
        self.headers.set(HfSlot::Header, header.map(Arc::new));
        self.headers.set(HfSlot::Footer, footer.map(Arc::new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BreakKind, Pos, SectionStart};

    #[test]
    fn empty_tables_show() {
        let mut cell = Paragraph::new("");
        assert!(story_is_empty(std::slice::from_ref(&cell)));
        cell.cell = Some(crate::InCell { table: 1, row: 0, col: 0, cell: None, table_props: None, nested: None });
        assert!(!story_is_empty(&[cell, Paragraph::new("")]));
    }

    #[test]
    fn linking_and_slots() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b")]);
        d.set_simple_headers(Some(vec![Paragraph::new("H")]), None);
        let sel = Selection::caret(Pos { para: 1, offset: 0 });
        d.insert_break(sel, BreakKind::Section(SectionStart::NextPage));
        assert_eq!(d.sections().len(), 2);
        // Linked: the second section shows the first one's header.
        assert!(d.hf_linked(1, HfSlot::Header));
        assert_eq!(d.hf_story(1, HfSlot::Header).unwrap()[0].text, "H");
        d.set_hf_story(1, HfSlot::Header, Some(vec![Paragraph::new("H2")]), sel);
        assert_eq!(d.hf_story(1, HfSlot::Header).unwrap()[0].text, "H2");
        assert_eq!(d.hf_story(0, HfSlot::Header).unwrap()[0].text, "H");
        assert!(d.hf_story(1, HfSlot::Footer).is_none());
        d.set_hf_options(1, true, true, sel);
        assert_eq!(d.page_slots(1, true, 3), (HfSlot::FirstHeader, HfSlot::FirstFooter));
        assert_eq!(d.page_slots(1, false, 4), (HfSlot::EvenHeader, HfSlot::EvenFooter));
        assert_eq!(d.page_slots(0, true, 1), (HfSlot::Header, HfSlot::Footer));
        d.undo();
        d.undo();
        assert_eq!(d.hf_story(1, HfSlot::Header).unwrap()[0].text, "H");
        assert!(!d.odd_even);
        assert!(d.has_headers());
    }

    #[test]
    fn page_numbering() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b")]);
        let sel = Selection::caret(Pos { para: 1, offset: 0 });
        d.insert_break(sel, BreakKind::Section(SectionStart::NextPage));
        let pages = [(0, true), (0, false), (0, false), (1, true), (1, false)];
        assert_eq!(d.page_labels(pages), vec!["1", "2", "3", "4", "5"]);
        d.set_page_numbering(0, PageNumbering { format: NoteFormat::LowerRoman, start: None }, sel);
        d.set_page_numbering(1, PageNumbering { format: NoteFormat::Arabic, start: Some(1) }, sel);
        assert_eq!(d.page_labels(pages), vec!["i", "ii", "iii", "1", "2"]);
        d.set_page_numbering(1, PageNumbering { format: NoteFormat::UpperLetter, start: None }, sel);
        assert_eq!(d.page_labels(pages), vec!["i", "ii", "iii", "D", "E"]);
        d.undo();
        assert_eq!(d.section_headers(1).numbering.start, Some(1));
    }
}
