//! Headers and footers of docx documents: their references in the
//! sections and the parts written (read and written like the body in
//! `docx.rs`).

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::xml::{Ev, Reader, local};

/// Header and footer references of a section: (header?, type) -> rel id,
/// with type 0 default, 1 first and 2 even.
#[derive(Default)]
pub(crate) struct Refs {
    pub parts: HashMap<(bool, u8), String>,
    pub title_pg: bool,
}

impl Refs {
    /// The references of the last section of a document: sections without
    /// a header or footer of some type take the previous section's.
    pub fn scan(document: &str) -> Refs {
        let mut r = Reader::new(document);
        let mut last = Refs::default();
        let mut cur: Option<Refs> = None;
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) if t.local() == "sectPrChange" => r.skip(),
                Ev::Start(t) if t.local() == "sectPr" => cur = Some(Refs { parts: last.parts.clone(), title_pg: false }),
                Ev::Start(t) | Ev::Empty(t) => {
                    if let Some(c) = &mut cur {
                        match t.local() {
                            "headerReference" => c.add(&t, true),
                            "footerReference" => c.add(&t, false),
                            "titlePg" => c.title_pg = t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
                            _ => {}
                        }
                    }
                }
                Ev::End(n) if local(n) == "sectPr" => {
                    if let Some(c) = cur.take() {
                        last = c;
                    }
                }
                _ => {}
            }
        }
        last
    }

    /// Reads a `headerReference` or `footerReference`.
    fn add(&mut self, t: &crate::xml::Tag, header: bool) {
        let ty = match t.attr("w:type").as_deref() {
            Some("first") => 1,
            Some("even") => 2,
            _ => 0,
        };
        if let Some(id) = t.attr("r:id") {
            self.parts.insert((header, ty), id.into_owned());
        }
    }
}

/// Whether settings.xml turns on different even pages.
pub(crate) fn even_and_odd(settings: &str) -> bool {
    let mut r = Reader::new(settings);
    loop {
        match r.next() {
            Ev::Eof => return false,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "evenAndOddHeaders" => return t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Writing

/// A header or footer part to write: (header?, type, file name).
pub(crate) struct Part {
    pub header: bool,
    pub file: String,
    pub xml: String,
    /// The part's relationships (a picture watermark's picture).
    pub rels: Option<String>,
}

pub(crate) fn rels(parts: &[Part]) -> String {
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        let _ = write!(s, r#"<Relationship Id="rIdH{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{}" Target="{}"/>"#, i + 1, if p.header { "header" } else { "footer" }, p.file);
    }
    s
}

pub(crate) fn content_types(parts: &[Part]) -> String {
    let mut s = String::new();
    for p in parts {
        let _ = write!(s, r#"<Override PartName="/word/{}" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{}+xml"/>"#, p.file, if p.header { "header" } else { "footer" });
    }
    s
}
