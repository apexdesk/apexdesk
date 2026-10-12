//! Publishing a book: its text as a manuscript for publishers, as a book
//! to print (trim size, mirrored margins, chapters on odd pages) or as an
//! e-book. Scene headings become a separator between scenes.

use std::sync::Arc;

use crate::book::{ANCHOR, CHAPTER_ANCHOR};
use crate::{Align, Attrs, Document, Field, FieldKind, Headers, HfSlot, LineRule, OBJECT, PageSetup, ParaKind, Paragraph, Section, SectionStart};

/// What goes between scenes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneBreak {
    #[default]
    Hashes,
    Stars,
    Blank,
    /// The scenes' headings, as they are.
    Titles,
}

impl SceneBreak {
    pub const ALL: [SceneBreak; 4] = [SceneBreak::Hashes, SceneBreak::Stars, SceneBreak::Blank, SceneBreak::Titles];

    /// The separator's text (None: the headings stay).
    pub fn text(self) -> Option<&'static str> {
        match self {
            SceneBreak::Hashes => Some("###"),
            SceneBreak::Stars => Some("* * *"),
            SceneBreak::Blank => Some(""),
            SceneBreak::Titles => None,
        }
    }
}

/// Trim sizes for printing: name and size in points.
pub const TRIM_SIZES: [(&str, f32, f32); 5] = [("A5", 419.5, 595.3), ("6 × 9 in", 432.0, 648.0), ("5 × 8 in", 360.0, 576.0), ("5,5 × 8,5 in", 396.0, 612.0), ("B5", 498.9, 708.7)];

/// What publishing needs besides the text, in the user's language.
#[derive(Clone, Debug, Default)]
pub struct Publish {
    pub title: String,
    pub author: String,
    pub breaks: SceneBreak,
    /// "about 80,000 words".
    pub about_words: String,
    /// "by Ana Pérez".
    pub by: String,
}

/// The part a published paragraph plays.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Chapter,
    Break,
    /// The first paragraph after a chapter heading or a break.
    Opening,
    Body,
    Other,
}

/// `text` as a paragraph aligned `align`.
fn line(text: &str, align: Align) -> Paragraph {
    let mut p = Paragraph::new(text);
    p.style.align = align;
    p
}

/// The page number, as a paragraph aligned `align` after `before`.
fn page_number(before: &str, align: Align, size: u16) -> Paragraph {
    let mut p = line(before, align);
    p.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(Field::new(FieldKind::Page))), ..Default::default() });
    p.map_attrs(0, p.text.len(), &|a| a.size = Some(size));
    p
}

fn story(p: Paragraph) -> Option<crate::Story> {
    Some(Arc::new(vec![p]))
}

/// Nothing: an empty header or footer.
fn blank() -> Option<crate::Story> {
    story(Paragraph::new(""))
}

/// The first paragraph with text before the first chapter: what the
/// author wrote ahead of it (a title, a dedication) starts there, on a
/// page of its own after the generated title page.
fn front_start(body: &Document, roles: &[Role]) -> Option<usize> {
    body.paragraphs.iter().zip(roles).take_while(|(_, r)| **r != Role::Chapter).position(|(p, _)| !p.text.trim().is_empty())
}

impl Document {
    /// The book's text ready to publish, each paragraph with its part:
    /// changes accepted, no comments or hidden marks, and scene headings
    /// turned into `breaks` (none before a chapter's first scene).
    fn publish_text(&self, breaks: SceneBreak) -> (Document, Vec<Role>) {
        let src = self.resolved(true);
        let mut paras = Vec::with_capacity(src.paragraphs.len());
        let mut roles = Vec::with_capacity(src.paragraphs.len());
        let mut after_head = true;
        for p in &src.paragraphs {
            let mut q = p.clone();
            q.marks.retain(|m| !m.name.starts_with(ANCHOR) && !m.name.starts_with(CHAPTER_ANCHOR));
            let role = match p.style.kind {
                ParaKind::Heading(1) if p.cell.is_none() => Role::Chapter,
                ParaKind::Heading(2) if p.cell.is_none() => match breaks.text() {
                    None => Role::Other,
                    Some(_) if after_head => continue,
                    Some(t) => {
                        q = line(t, Align::Center);
                        Role::Break
                    }
                },
                ParaKind::Normal if p.cell.is_none() && !p.text.trim().is_empty() => {
                    if after_head {
                        Role::Opening
                    } else {
                        Role::Body
                    }
                }
                _ => Role::Other,
            };
            if !p.text.trim().is_empty() || role == Role::Break {
                after_head = matches!(role, Role::Chapter | Role::Break) || (role == Role::Other && matches!(p.style.kind, ParaKind::Heading(2)));
            }
            paras.push(q);
            roles.push(role);
        }
        let mut doc = src.layout_copy(paras);
        doc.comments.clear();
        (doc, roles)
    }

    /// A manuscript for publishers: a cover with the author, the words and
    /// the title, then the text in `font` at 12 points, double spaced,
    /// first lines indented, each chapter on a new page a third of the way
    /// down and "Author / TITLE / page" at the top of each page.
    pub fn manuscript(&self, p: &Publish, font: &str) -> Document {
        let (body, roles) = self.publish_text(p.breaks);
        let page = PageSetup { margin_top: 72.0, margin_bottom: 72.0, margin_left: 72.0, margin_right: 72.0, margin_header: 36.0, margin_footer: 36.0, mirror: false, gutter: 0.0, ..self.page };
        let mut paras = vec![line(&p.author, Align::Left), line(&p.about_words, Align::Right)];
        let mut title = line(&p.title.to_uppercase(), Align::Center);
        title.style.space_before = Some(((page.height - 144.0) / 3.0).max(0.0));
        paras.push(title);
        paras.push(line(&p.by, Align::Center));
        let front = front_start(&body, &roles);
        for (i, (mut q, role)) in body.paragraphs.into_iter().zip(roles).enumerate() {
            let s = &mut q.style;
            s.page_break_before |= front == Some(i);
            match role {
                Role::Chapter => {
                    s.kind = ParaKind::Normal;
                    s.align = Align::Center;
                    s.page_break_before = true;
                    s.space_before = Some(((page.height - 144.0) / 3.0).max(0.0));
                    s.space_after = Some(24.0);
                }
                Role::Break => {}
                Role::Opening | Role::Body => {
                    s.align = Align::Left;
                    s.first_line = 36.0;
                    s.indent_left = 0.0;
                }
                Role::Other => {}
            }
            if role != Role::Other || s.kind == ParaKind::Normal {
                s.line_spacing = 2.0;
                s.line_rule = LineRule::Multiple;
                s.space_before.get_or_insert(0.0);
                s.space_after.get_or_insert(0.0);
            }
            q.map_attrs(0, q.text.len(), &|a| {
                a.font = None;
                a.size = Some(24);
                a.color = None;
                a.highlight = None;
            });
            paras.push(q);
        }
        for q in &mut paras[..4] {
            q.map_attrs(0, q.text.len(), &|a| a.size = Some(24));
            q.style.line_spacing = 1.0;
        }
        let mut doc = self.layout_copy(paras);
        doc.comments.clear();
        doc.page = page;
        doc.default_font = Some(Arc::from(font));
        doc.odd_even = false;
        let surname = p.author.split_whitespace().last().unwrap_or("");
        let mut head = Headers { different_first: true, ..Default::default() };
        head.parts[HfSlot::Header.index()] = story(page_number(&format!("{surname} / {} / ", p.title.to_uppercase()), Align::Right, 24));
        head.parts[HfSlot::FirstHeader.index()] = blank();
        head.parts[HfSlot::FirstFooter.index()] = blank();
        doc.headers = head;
        doc.meta.title = p.title.clone();
        doc.meta.author = p.author.clone();
        doc
    }

    /// A book to print on `trim` (width and height in points): a title
    /// page, then each chapter on an odd page; mirrored margins, the text
    /// justified with indented first lines, the author and the title at
    /// the top of even and odd pages (not on a chapter's first page) and
    /// the page number at the foot.
    pub fn print_book(&self, p: &Publish, trim: (f32, f32)) -> Document {
        let (body, roles) = self.publish_text(p.breaks);
        let (w, h) = trim;
        let page = PageSetup { width: w, height: h, margin_top: 0.09 * h, margin_bottom: 0.09 * h, margin_left: 0.14 * w, margin_right: 0.1 * w, margin_header: 0.045 * h, margin_footer: 0.045 * h, mirror: true, gutter: 0.0, ..self.page };
        let mut title = line(&p.title, Align::Center);
        title.style.kind = ParaKind::Title;
        title.style.space_before = Some(h * 0.25);
        let mut paras = vec![title, line(&p.author, Align::Center)];
        let mut first = true;
        let front = front_start(&body, &roles);
        for (i, (mut q, role)) in body.paragraphs.into_iter().zip(roles).enumerate() {
            let s = &mut q.style;
            s.page_break_before |= front == Some(i);
            match role {
                Role::Chapter => {
                    s.align = Align::Center;
                    s.space_before = Some(h * 0.18);
                    s.space_after = Some(h * 0.06);
                    s.page_break_before = false;
                    let mut head = Headers { different_first: true, ..Default::default() };
                    if first {
                        head.parts[HfSlot::FirstHeader.index()] = blank();
                        head.parts[HfSlot::FirstFooter.index()] = story(page_number("", Align::Center, 18));
                        head.parts[HfSlot::Header.index()] = story(line(&p.title, Align::Center));
                        head.parts[HfSlot::EvenHeader.index()] = story(line(&p.author, Align::Center));
                        head.parts[HfSlot::Footer.index()] = story(page_number("", Align::Center, 18));
                        head.parts[HfSlot::EvenFooter.index()] = story(page_number("", Align::Center, 18));
                        first = false;
                    }
                    q.section = Some(Box::new(Section { start: SectionStart::OddPage, page, columns: Default::default(), headers: head }));
                }
                Role::Break => {
                    s.space_before = Some(12.0);
                    s.space_after = Some(12.0);
                }
                Role::Opening | Role::Body => {
                    s.align = Align::Justify;
                    s.first_line = if role == Role::Opening { 0.0 } else { 14.0 };
                    s.indent_left = 0.0;
                    s.space_before = Some(0.0);
                    s.space_after = Some(0.0);
                }
                Role::Other => {}
            }
            paras.push(q);
        }
        let mut doc = self.layout_copy(paras);
        doc.comments.clear();
        doc.page = page;
        doc.odd_even = true;
        doc.auto_hyphenation = true;
        // The title page: nothing at the top or foot.
        let mut head = Headers { different_first: true, ..Default::default() };
        for slot in HfSlot::ALL {
            head.parts[slot.index()] = blank();
        }
        doc.headers = head;
        doc.meta.title = p.title.clone();
        doc.meta.author = p.author.clone();
        doc
    }

    /// The book as an e-book's text: chapters and scene breaks, the
    /// paragraphs with indented first lines.
    pub fn ebook(&self, p: &Publish) -> Document {
        let (mut doc, roles) = self.publish_text(p.breaks);
        for (q, role) in doc.paragraphs.iter_mut().zip(roles) {
            if role == Role::Break {
                q.style.space_before = Some(12.0);
                q.style.space_after = Some(12.0);
            }
        }
        doc.meta.title = p.title.clone();
        doc.meta.author = p.author.clone();
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(text: &str, level: u8) -> Paragraph {
        let mut p = Paragraph::new(text);
        p.style.kind = ParaKind::Heading(level);
        p
    }

    fn novel() -> Document {
        Document::from_paragraphs(vec![heading("Uno", 1), heading("Llega", 2), Paragraph::new("Ana llega."), Paragraph::new("Luego duerme."), heading("Cena", 2), Paragraph::new("Cenan."), heading("Dos", 1), heading("Vuelve", 2), Paragraph::new("Vuelve.")])
    }

    fn publish() -> Publish {
        Publish { title: "El puerto".into(), author: "Ana Pérez".into(), breaks: SceneBreak::Hashes, about_words: "unas 100 palabras".into(), by: "de Ana Pérez".into() }
    }

    #[test]
    fn scenes_become_breaks_but_not_after_a_chapter() {
        let (d, roles) = novel().publish_text(SceneBreak::Hashes);
        let texts: Vec<&str> = d.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["Uno", "Ana llega.", "Luego duerme.", "###", "Cenan.", "Dos", "Vuelve."]);
        assert!(roles[1] == Role::Opening && roles[2] == Role::Body && roles[4] == Role::Opening);
        let (d, _) = novel().publish_text(SceneBreak::Titles);
        assert_eq!(d.paragraphs.len(), 9);
    }

    #[test]
    fn a_manuscript_has_a_cover_and_double_spacing() {
        let m = novel().manuscript(&publish(), "Times New Roman");
        assert_eq!(m.paragraphs[1].text, "unas 100 palabras");
        assert_eq!(m.paragraphs[2].text, "EL PUERTO");
        let ch = &m.paragraphs[4];
        assert!(ch.text == "Uno" && ch.style.page_break_before && ch.style.align == Align::Center);
        assert_eq!((m.paragraphs[5].style.line_spacing, m.paragraphs[5].style.first_line), (2.0, 36.0));
        assert_eq!(m.default_font.as_deref(), Some("Times New Roman"));
        let head = m.headers.get(HfSlot::Header).unwrap();
        assert!(head[0].text.starts_with("Pérez / EL PUERTO / "));
        assert!(m.book.is_none());
    }

    #[test]
    fn a_printed_book_starts_its_chapters_on_odd_pages() {
        let mut src = novel();
        src.book = Some(Default::default());
        src.heading_anchor(0);
        let b = src.print_book(&publish(), (TRIM_SIZES[1].1, TRIM_SIZES[1].2));
        assert_eq!((b.page.width, b.page.mirror, b.odd_even), (432.0, true, true));
        assert_eq!(b.paragraphs[0].text, "El puerto");
        let chapters: Vec<&Paragraph> = b.paragraphs.iter().filter(|p| p.section.is_some()).collect();
        assert_eq!(chapters.len(), 2);
        assert!(chapters.iter().all(|p| p.section.as_ref().unwrap().start == SectionStart::OddPage && p.section.as_ref().unwrap().headers.different_first));
        // No hidden anchors and no book data in the copy.
        assert!(b.paragraphs.iter().all(|p| p.marks.is_empty()) && b.book.is_none());
        let sections = b.sections();
        assert_eq!(sections.len(), 3);
        assert!(b.hf_story(1, HfSlot::FirstHeader).is_some_and(|s| s[0].text.is_empty()));
        assert!(b.hf_story(2, HfSlot::EvenHeader).is_some_and(|s| s[0].text == "Ana Pérez"));
        let e = src.ebook(&publish());
        assert_eq!(e.paragraphs.iter().filter(|p| p.text == "###").count(), 1);
    }

    #[test]
    fn text_before_the_first_chapter_stays_on_a_page_of_its_own() {
        let mut title = Paragraph::new("El puerto");
        title.style.kind = ParaKind::Title;
        let mut paras = vec![Paragraph::new("Nota previa."), Paragraph::new(""), title, Paragraph::new("A mi madre.")];
        paras.extend(novel().paragraphs);
        let src = Document::from_paragraphs(paras);
        let b = src.print_book(&publish(), (TRIM_SIZES[0].1, TRIM_SIZES[0].2));
        let texts: Vec<&str> = b.paragraphs.iter().take(6).map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["El puerto", "Ana Pérez", "Nota previa.", "", "El puerto", "A mi madre."]);
        let breaks: Vec<bool> = b.paragraphs.iter().take(6).map(|p| p.style.page_break_before).collect();
        assert_eq!(breaks, [false, false, true, false, false, false]);
        let m = src.manuscript(&publish(), "Times New Roman");
        assert!(m.paragraphs[4].text == "Nota previa." && m.paragraphs[4].style.page_break_before);
        // Nothing before the first chapter: no extra page.
        assert!(!novel().print_book(&publish(), (TRIM_SIZES[0].1, TRIM_SIZES[0].2)).paragraphs[2].style.page_break_before);
    }
}
