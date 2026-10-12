//! Accessibility checker for documents (Review > Check Accessibility):
//! pictures and shapes without alternative text, tables without a header
//! row, headings that skip levels, hard-to-read text, links whose text
//! doesn't say where they go, and a document without a title or language.

use std::sync::Arc;

use crate::{Document, OBJECT, ParaKind, Pos, Selection};

/// What kind of problem an issue is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// A picture or shape without alternative text.
    MissingAlt,
    /// A table without a header row.
    NoHeader,
    /// Text whose color hardly differs from its background (WCAG).
    Contrast,
    /// A heading more than one level below the heading before it.
    HeadingOrder,
    /// A link whose text is "click here" or the address itself.
    LinkText,
    NoTitle,
    NoLanguage,
}

impl Category {
    pub const ALL: [Category; 7] = [Category::MissingAlt, Category::NoHeader, Category::Contrast, Category::HeadingOrder, Category::LinkText, Category::NoTitle, Category::NoLanguage];

    /// Errors must be fixed; warnings may be fine.
    pub fn is_error(self) -> bool {
        matches!(self, Category::MissingAlt | Category::NoHeader | Category::Contrast)
    }
}

/// Where an issue is.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Image(Pos),
    Shape(Pos),
    /// A table: its first paragraph.
    Table(usize),
    /// A heading paragraph, and the level it should have.
    Heading { para: usize, level: u8 },
    /// Some text (contrast or a link).
    Text(Selection),
    /// The document's properties (title, language).
    Document,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub category: Category,
    pub target: Target,
    /// What to show: the object's name or the text.
    pub label: String,
    /// Contrast issues: the ratio found.
    pub ratio: Option<f64>,
}

/// WCAG's minimum contrast for normal text, and for large text (18 pt,
/// or 14 pt bold).
pub const MIN_CONTRAST: f64 = 4.5;
pub const MIN_CONTRAST_LARGE: f64 = 3.0;
/// At most this many issues of one category are listed.
pub const MAX_PER_CATEGORY: usize = 500;

/// Relative luminance of an sRGB color (0xRRGGBB), WCAG 2.
pub fn luminance(rgb: u32) -> f64 {
    let lin = |c: u32| {
        let s = c as f64 / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin((rgb >> 16) & 0xFF) + 0.7152 * lin((rgb >> 8) & 0xFF) + 0.0722 * lin(rgb & 0xFF)
}

/// WCAG contrast ratio of two colors (1 to 21).
pub fn contrast(a: u32, b: u32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Black or white, whichever reads better on `bg`.
pub fn readable_on(bg: u32) -> u32 {
    if contrast(0, bg) >= contrast(0xFFFFFF, bg) { 0 } else { 0xFFFFFF }
}

/// Link texts that don't say where the link goes, in the program's
/// languages.
const VAGUE_LINKS: &[&str] = &[
    "click here", "here", "link", "this link", "more", "read more", "click", "this", "aquí", "haga clic aquí", "haz clic aquí", "pulse aquí", "pincha aquí", "enlace", "más", "leer más", "hier", "hier klicken", "klicken sie hier", "mehr", "weiterlesen", "ici", "cliquez ici", "lien", "plus", "en savoir plus", "qui", "clicca qui", "fai clic qui", "altro", "aqui", "clique aqui", "saiba mais", "ここ", "こちら", "ここをクリック", "詳細", "여기", "여기를 클릭", "자세히", "这里", "点击这里", "更多", "這裡", "點擊這裡",
];

/// Whether a link's text says nothing about where it goes.
pub fn vague_link_text(text: &str) -> bool {
    let t = text.trim().trim_end_matches(['.', '!', ':', '»', '>']).trim().to_lowercase();
    VAGUE_LINKS.contains(&t.as_str()) || t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.")
}

/// A short label for some text.
fn snippet(text: &str) -> String {
    let t: String = text.chars().filter(|&c| c != OBJECT).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 40 { format!("{}…", t.chars().take(40).collect::<String>()) } else { t }
}

/// The locale tag of a program language ("es" → "es-ES").
pub fn lang_tag(ui: &str) -> &'static str {
    match ui {
        "es" => "es-ES",
        "de" => "de-DE",
        "fr" => "fr-FR",
        "it" => "it-IT",
        "pt" => "pt-PT",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        "zh" => "zh-CN",
        "zh-Hant" => "zh-TW",
        _ => "en-US",
    }
}

impl Document {
    /// The document's language, if it says one (documents made here are
    /// saved with one).
    pub fn language(&self) -> Option<Arc<str>> {
        match &self.styles.defaults {
            Some(d) => d.lang.clone(),
            None => Some(Arc::from("es-ES")),
        }
    }

    /// Sets the language of the document's text (one undo step).
    pub fn set_language(&mut self, lang: &str, sel: Selection) {
        let before = self.styles.clone();
        self.styles.defaults.get_or_insert_with(Default::default).lang = Some(Arc::from(lang));
        self.record_styles(Vec::new(), 0, sel, before);
    }

    /// Everything that makes the document hard to read for people with
    /// disabilities.
    pub fn check_accessibility(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        let push = |out: &mut Vec<Issue>, i: Issue| {
            if out.iter().filter(|x: &&Issue| x.category == i.category).count() < MAX_PER_CATEGORY {
                out.push(i);
            }
        };
        for (p, img) in self.images() {
            if img.alt.trim().is_empty() {
                let label = if img.name.is_empty() { format!("#{}", p.para + 1) } else { img.name.clone() };
                push(&mut out, Issue { category: Category::MissingAlt, target: Target::Image(p), label, ratio: None });
            }
        }
        for (p, sh) in self.shapes() {
            // A text box says what it has; lines and decorations aren't told apart.
            let text: String = sh.shape.text.iter().map(|t| t.text()).collect();
            if sh.shape.alt.trim().is_empty() && text.trim().is_empty() {
                let label = if sh.shape.name.is_empty() { format!("#{}", p.para + 1) } else { sh.shape.name.clone() };
                push(&mut out, Issue { category: Category::MissingAlt, target: Target::Shape(p), label, ratio: None });
            }
        }
        for m in self.all_tables() {
            let mut tag = self.paragraphs[m.start].cell.as_ref();
            for _ in 0..m.depth {
                tag = tag.and_then(|c| c.nested.as_deref());
            }
            let header = tag.and_then(|c| c.table_props.as_ref()).is_some_and(|t| t.header_rows > 0);
            if !header && m.rows > 1 {
                let label = snippet(&self.paragraphs[m.start].text);
                push(&mut out, Issue { category: Category::NoHeader, target: Target::Table(m.start), label, ratio: None });
            }
        }
        let mut last = None;
        for (i, p) in self.paragraphs.iter().enumerate() {
            let ParaKind::Heading(level) = p.style.kind else { continue };
            if let Some(prev) = last.filter(|&prev: &u8| level > prev + 1) {
                push(&mut out, Issue { category: Category::HeadingOrder, target: Target::Heading { para: i, level: prev + 1 }, label: format!("{} ({level})", snippet(&p.text)), ratio: None });
            }
            last = Some(level);
        }
        let page = self.deco.color.unwrap_or(0xFFFFFF);
        for (i, p) in self.paragraphs.iter().enumerate() {
            let cell = p.cell.as_ref().and_then(|c| c.cell.as_ref()).and_then(|c| c.shading);
            let mut bad: Option<(usize, usize, f64)> = None;
            let mut link: Option<(usize, usize, Arc<str>)> = None;
            let flush_link = |out: &mut Vec<Issue>, link: &mut Option<(usize, usize, Arc<str>)>| {
                if let Some((s, e, _)) = link.take() {
                    if vague_link_text(&p.text[s..e]) {
                        let sel = Selection { anchor: Pos::new(i, s), cursor: Pos::new(i, e) };
                        push(out, Issue { category: Category::LinkText, target: Target::Text(sel), label: snippet(&p.text[s..e]), ratio: None });
                    }
                }
            };
            for (s, e, a) in p.spans() {
                match (&mut link, &a.link) {
                    (Some((_, end, l)), Some(k)) if *end == s && l == k => *end = e,
                    (_, Some(k)) => {
                        flush_link(&mut out, &mut link);
                        link = Some((s, e, k.clone()));
                    }
                    (_, None) => flush_link(&mut out, &mut link),
                }
                let f = self.char_format(&p.style, a);
                // Automatic color reads on any background.
                let ratio = f.color.filter(|_| p.text[s..e].chars().any(|c| !c.is_whitespace() && c != OBJECT)).map(|fg| contrast(fg, a.highlight.or(p.style.shading).or(cell).unwrap_or(page)));
                let large = f.size >= 36 || (f.bold && f.size >= 28);
                let min = if large { MIN_CONTRAST_LARGE } else { MIN_CONTRAST };
                match (ratio.filter(|r| *r < min), &mut bad) {
                    (Some(r), Some((_, end, worst))) if *end == s => {
                        *end = e;
                        *worst = worst.min(r);
                    }
                    (Some(r), _) => {
                        if let Some((bs, be, w)) = bad.take() {
                            let sel = Selection { anchor: Pos::new(i, bs), cursor: Pos::new(i, be) };
                            push(&mut out, Issue { category: Category::Contrast, target: Target::Text(sel), label: snippet(&p.text[bs..be]), ratio: Some(w) });
                        }
                        bad = Some((s, e, r));
                    }
                    (None, _) => {
                        if let Some((bs, be, w)) = bad.take() {
                            let sel = Selection { anchor: Pos::new(i, bs), cursor: Pos::new(i, be) };
                            push(&mut out, Issue { category: Category::Contrast, target: Target::Text(sel), label: snippet(&p.text[bs..be]), ratio: Some(w) });
                        }
                    }
                }
            }
            flush_link(&mut out, &mut link);
            if let Some((bs, be, w)) = bad {
                let sel = Selection { anchor: Pos::new(i, bs), cursor: Pos::new(i, be) };
                push(&mut out, Issue { category: Category::Contrast, target: Target::Text(sel), label: snippet(&p.text[bs..be]), ratio: Some(w) });
            }
        }
        if self.meta.title.trim().is_empty() {
            push(&mut out, Issue { category: Category::NoTitle, target: Target::Document, label: String::new(), ratio: None });
        }
        if self.language().is_none_or(|l| l.trim().is_empty()) {
            push(&mut out, Issue { category: Category::NoLanguage, target: Target::Document, label: String::new(), ratio: None });
        }
        out.sort_by_key(|i| i.category);
        out
    }

    /// Colors the text of `sel` (one paragraph) black or white, whichever
    /// reads better on its background (one undo step).
    pub fn fix_contrast(&mut self, sel: Selection) {
        let Some(p) = self.paragraphs.get(sel.start().para) else { return };
        let cell = p.cell.as_ref().and_then(|c| c.cell.as_ref()).and_then(|c| c.shading);
        let base = p.style.shading.or(cell).or(self.deco.color).unwrap_or(0xFFFFFF);
        self.format_range(sel, &|a| a.color = Some(readable_on(a.highlight.unwrap_or(base))));
    }

    /// A title for the document:its Title paragraph or first heading.
    pub fn suggested_title(&self) -> String {
        let first = |k: fn(ParaKind) -> bool| self.paragraphs.iter().find(|p| k(p.style.kind) && !p.text.trim().is_empty()).map(|p| snippet(&p.text));
        first(|k| k == ParaKind::Title).or_else(|| first(|k| matches!(k, ParaKind::Heading(_)))).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attrs, Image, InCell, ParaStyle, Paragraph, TableProps};

    fn heading(t: &str, l: u8) -> Paragraph {
        Paragraph::new(t).with_style(ParaStyle { kind: ParaKind::Heading(l), ..Default::default() })
    }

    #[test]
    fn checks() {
        let mut link = Paragraph::new("See ");
        link.push("click here", Attrs { link: Some("https://x.org".into()), ..Default::default() });
        link.push(" or ", Attrs::default());
        link.push("the report", Attrs { link: Some("https://x.org/r".into()), ..Default::default() });
        let mut pale = Paragraph::new("");
        pale.push("pale text", Attrs { color: Some(0xDDDDDD), ..Default::default() });
        pale.push(" dark", Attrs { color: Some(0x202020), ..Default::default() });
        let mut cells = Vec::new();
        for r in 0..2 {
            for c in 0..2 {
                let mut p = Paragraph::new(format!("{r}{c}"));
                let mut tag = InCell::new(1, r, c);
                if r == 0 && c == 0 {
                    tag.table_props = Some(Box::new(TableProps::default()));
                }
                p.cell = Some(tag);
                cells.push(p);
            }
        }
        let mut paras = vec![heading("Intro", 1), heading("Deep", 3), link, pale];
        paras.extend(cells);
        paras.push(Paragraph::new("end"));
        let mut d = Document::from_paragraphs(paras);
        let sel = Selection::caret(Pos::new(0, 0));
        d.insert_image(sel, Image::new(Arc::from(&b"x"[..]), 10.0, 10.0));
        let issues = d.check_accessibility();
        let cats: Vec<Category> = issues.iter().map(|i| i.category).collect();
        assert_eq!(cats, [Category::MissingAlt, Category::NoHeader, Category::Contrast, Category::HeadingOrder, Category::LinkText, Category::NoTitle], "{issues:?}");
        assert_eq!(issues[2].label, "pale text");
        assert!(issues[2].ratio.unwrap() < 1.5);
        assert_eq!(issues[3].target, Target::Heading { para: 1, level: 2 });
        assert_eq!(issues[4].label, "click here");
        assert_eq!(d.suggested_title(), "Intro");
        // A document read from a file may have no language.
        d.styles.defaults = Some(Default::default());
        assert_eq!(d.check_accessibility().last().map(|i| i.category), Some(Category::NoLanguage));
        d.set_language("en-US", sel);
        assert_eq!(d.language().as_deref(), Some("en-US"));
        d.undo();
        assert_eq!(d.language(), None);
        let bad = match &issues[2].target { Target::Text(s) => *s, _ => unreachable!() };
        d.fix_contrast(bad);
        assert!(!d.check_accessibility().iter().any(|i| i.category == Category::Contrast));
        assert_eq!(readable_on(0x101010), 0xFFFFFF);
        assert!(vague_link_text(" Aquí.") && vague_link_text("https://a.b") && !vague_link_text("Annual report"));
    }
}
