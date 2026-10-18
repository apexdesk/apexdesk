//! PDF files read into editable documents (as Word does: the result is
//! close to the original, not the same), shown page by page as they are
//! drawn, and their tables read for Get Data.

mod build;
mod content;
pub mod edit;
mod file;
mod font;
pub(crate) mod image;
mod layout;
mod obj;
pub mod render;
mod tables;

pub use content::{Char, Page, PageImage, Shape};
pub use font::Style;
/// The file structure, for readers of other parts of a PDF (signatures'
/// validation data).
pub use file::PdfFile;
pub use obj::{Dict as PdfDict, Obj as PdfObj};

use crate::Error;
use apex_doc::Document;

/// Message of the error for PDFs without text (scanned pages).
pub(crate) const SCANNED: &str = "the PDF has no text: its pages are pictures and need text recognition (OCR)";

/// Pixels of pictures decoded per file at most.
const PIXEL_BUDGET: usize = 200_000_000;
/// Characters of text per file at most (pages repeating one long text
/// would otherwise each keep it).
const CHAR_BUDGET: usize = 20_000_000;

/// Bytes of page content decoded per file at most (pages naming one
/// inflating stream over and over would take hours).
const CONTENT_BUDGET: usize = 2 << 30;

fn budget() -> content::Budget {
    content::Budget { pixels: PIXEL_BUDGET, chars: CHAR_BUDGET, content: CONTENT_BUDGET }
}

/// Whether a PDF is read here by scanning it, its cross-references broken
/// or losing its pages (what is shown may then not be what was signed).
pub fn read_by_repair(data: &[u8], password: Option<&str>) -> bool {
    file::PdfFile::open(data, password).is_ok_and(|f| f.repaired())
}

/// The pages of a PDF as they are drawn.
pub fn pages(data: &[u8], password: Option<&str>) -> Result<Vec<Page>, Error> {
    let f = file::PdfFile::open(data, password)?;
    let refs = f.pages();
    if refs.is_empty() {
        return Err(Error::new("the PDF file has no pages"));
    }
    let mut budget = budget();
    Ok(refs.iter().map(|p| content::page(&f, p, &mut budget)).collect())
}

/// The pages of a PDF without their pictures (text, lines and links), for
/// selecting and searching text in pages drawn by something else.
pub fn text_pages(data: &[u8], password: Option<&str>) -> Result<Vec<Page>, Error> {
    let f = file::PdfFile::open(data, password)?;
    let mut budget = content::Budget { pixels: 0, chars: CHAR_BUDGET, content: CONTENT_BUDGET };
    Ok(f.pages().iter().map(|p| content::page(&f, p, &mut budget)).collect())
}

fn has_text(pages: &[Page]) -> bool {
    pages.iter().any(|p| p.chars.iter().any(|c| !c.ch.is_whitespace()))
}

/// A PDF as an editable document (a damaged file is an error, also when
/// read from the window rather than through `open`).
pub fn read(data: &[u8], password: Option<&str>) -> Result<Document, Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_unguarded(data, password))).unwrap_or_else(|_| Err(Error::new("the file is damaged and could not be read")))
}

fn read_unguarded(data: &[u8], password: Option<&str>) -> Result<Document, Error> {
    let f = file::PdfFile::open(data, password)?;
    let refs = f.pages();
    if refs.is_empty() {
        return Err(Error::new("the PDF file has no pages"));
    }
    let mut budget = budget();
    let pages: Vec<Page> = refs.iter().map(|p| content::page(&f, p, &mut budget)).collect();
    if !has_text(&pages) {
        return Err(Error::new(SCANNED));
    }
    let mut doc = build::document(&pages);
    if let Some(info) = f.info() {
        let text = |k: &str| f.key(&info, k).bytes().map(text_string).unwrap_or_default();
        doc.meta.title = text("Title");
        doc.meta.author = text("Author");
        doc.meta.subject = text("Subject");
        doc.meta.keywords = text("Keywords");
    }
    Ok(doc)
}

/// A table found in a PDF: its page (from 0) and its cells' text.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfTable {
    pub page: usize,
    pub rows: Vec<Vec<String>>,
}

/// The tables of a PDF (drawn as grids or as aligned columns of text).
pub fn tables(data: &[u8], password: Option<&str>) -> Result<Vec<PdfTable>, Error> {
    let pages = pages(data, password)?;
    if !has_text(&pages) {
        return Err(Error::new(SCANNED));
    }
    Ok(build::tables(&pages).into_iter().map(|(page, rows)| PdfTable { page, rows }).collect())
}

/// A PDF text string: UTF-16 or UTF-8 with a byte order mark, otherwise
/// PDFDocEncoding (Latin-1 for the characters that matter).
pub(crate) fn text_string(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units = rest.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]));
        return char::decode_utf16(units).filter_map(Result::ok).collect();
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    b.iter()
        .map(|&c| match c {
            0x18..=0x1F | 0x80..=0x9F => pdfdoc(c),
            c => c as char,
        })
        .collect()
}

fn pdfdoc(c: u8) -> char {
    const HIGH: [char; 32] = ['•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚', '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{FFFD}'];
    const LOW: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    match c {
        0x18..=0x1F => LOW[(c - 0x18) as usize],
        0x80..=0x9F => HIGH[(c - 0x80) as usize],
        c => c as char,
    }
}

#[cfg(test)]
mod tests;
