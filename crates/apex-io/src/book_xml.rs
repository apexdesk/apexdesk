//! A book's data (book mode) in its own part: `customXml/apexBook.xml` in
//! .docx (a custom XML part, which Word keeps) and `ApexDesk/book.xml` in
//! .odt. Other programs ignore it; the text stays a normal document.

use std::fmt::Write as _;

use apex_doc::book::{Book, DayWords, SceneInfo, SceneStatus, Snapshot};

use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const NS: &str = "urn:apexdesk:book:1";
pub(crate) const DOCX_PART: &str = "customXml/apexBook.xml";
const DOCX_PROPS: &str = "customXml/apexBookProps.xml";
pub(crate) const DOCX_REL: &str = r#"<Relationship Id="rIdBook" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml" Target="../customXml/apexBook.xml"/>"#;
pub(crate) const DOCX_CONTENT_TYPE: &str = r#"<Override PartName="/customXml/apexBookProps.xml" ContentType="application/vnd.openxmlformats-officedocument.customXmlProperties+xml"/>"#;
pub(crate) const ODT_PART: &str = "ApexDesk/book.xml";
pub(crate) const ODT_MANIFEST: &str = r#"<manifest:file-entry manifest:full-path="ApexDesk/book.xml" manifest:media-type="text/xml"/>"#;

pub(crate) fn to_xml(b: &Book) -> String {
    let mut x = format!(r#"{HEADER}<book xmlns="{NS}">"#);
    let g = &b.goal;
    let _ = write!(x, r#"<goal words="{}" daily="{}" deadline="{}"/>"#, g.words, g.daily, esc(&g.deadline));
    for s in &b.scenes {
        let _ = write!(x, r#"<scene anchor="{}" status="{}" pov="{}">"#, esc(&s.anchor), s.status.key(), esc(&s.pov));
        if !s.synopsis.is_empty() {
            let _ = write!(x, "<synopsis>{}</synopsis>", esc(&s.synopsis));
        }
        for t in &s.tags {
            let _ = write!(x, "<tag>{}</tag>", esc(t));
        }
        x.push_str("</scene>");
    }
    for d in &b.progress {
        let _ = write!(x, r#"<day date="{}" words="{}"/>"#, esc(&d.date), d.words);
    }
    for s in &b.snapshots {
        let _ = write!(x, r#"<snapshot date="{}" label="{}" anchor="{}" title="{}">"#, esc(&s.date), esc(&s.label), esc(&s.anchor), esc(&s.title));
        for p in &s.paras {
            let _ = write!(x, "<p>{}</p>", esc(p));
        }
        x.push_str("</snapshot>");
    }
    x.push_str("</book>");
    x
}

/// The book in `xml`, if it is one.
pub(crate) fn from_xml(xml: &str) -> Option<Book> {
    if !xml.contains(NS) {
        return None;
    }
    let mut b = Book::default();
    let mut r = Reader::new(xml);
    let mut scene: Option<SceneInfo> = None;
    let mut snap: Option<Snapshot> = None;
    let num = |v: Option<std::borrow::Cow<str>>| v.and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    loop {
        let (t, empty) = match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            Ev::End(n) => {
                if crate::xml::local(n) == "scene"
                    && let Some(s) = scene.take()
                {
                    b.scenes.push(s);
                }
                if crate::xml::local(n) == "snapshot"
                    && let Some(s) = snap.take()
                {
                    b.snapshots.push(s);
                }
                continue;
            }
            _ => continue,
        };
        match t.local() {
            "goal" => {
                b.goal.words = num(t.attr("words"));
                b.goal.daily = num(t.attr("daily"));
                b.goal.deadline = t.attr("deadline").unwrap_or_default().into_owned();
            }
            "scene" => {
                let s = SceneInfo {
                    anchor: t.attr("anchor").unwrap_or_default().into_owned(),
                    status: SceneStatus::from_key(&t.attr("status").unwrap_or_default()),
                    pov: t.attr("pov").unwrap_or_default().into_owned(),
                    ..Default::default()
                };
                if empty {
                    b.scenes.push(s);
                } else {
                    scene = Some(s);
                }
            }
            "synopsis" => {
                let v = r.text();
                if let Some(s) = &mut scene {
                    s.synopsis = v;
                }
            }
            "tag" => {
                let v = r.text();
                if let Some(s) = &mut scene {
                    s.tags.push(v);
                }
            }
            "snapshot" => {
                let s = Snapshot {
                    date: t.attr("date").unwrap_or_default().into_owned(),
                    label: t.attr("label").unwrap_or_default().into_owned(),
                    anchor: t.attr("anchor").unwrap_or_default().into_owned(),
                    title: t.attr("title").unwrap_or_default().into_owned(),
                    paras: Vec::new(),
                };
                if empty {
                    b.snapshots.push(s);
                } else {
                    snap = Some(s);
                }
            }
            "p" => {
                let v = if empty { String::new() } else { r.text() };
                if let Some(s) = &mut snap {
                    s.paras.push(v);
                }
            }
            "day" => b.progress.push(DayWords { date: t.attr("date").unwrap_or_default().into_owned(), words: num(t.attr("words")) }),
            _ => {}
        }
    }
    Some(b)
}

/// The book of a .docx: the custom XML part in the book's namespace,
/// wherever Word put it.
pub(crate) fn read_docx(z: &ZipReader) -> Option<Book> {
    let names: Vec<String> = z.names().filter(|n| n.starts_with("customXml/") && !n.contains("Props") && !n.contains("_rels") && n.ends_with(".xml")).map(String::from).collect();
    names.iter().find_map(|n| z.read_string(n).ok().and_then(|x| from_xml(&x)))
}

pub(crate) fn write_docx(z: &mut ZipWriter, b: &Book) {
    z.add(DOCX_PART, to_xml(b).as_bytes(), true);
    z.add(DOCX_PROPS, format!(r#"{HEADER}<ds:datastoreItem ds:itemID="{{7C3D1A9E-2B4F-4D8A-A61C-5E9F0B2D4C7A}}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs><ds:schemaRef ds:uri="{NS}"/></ds:schemaRefs></ds:datastoreItem>"#).as_bytes(), true);
    z.add("customXml/_rels/apexBook.xml.rels", format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps" Target="apexBookProps.xml"/></Relationships>"#).as_bytes(), true);
}

pub(crate) fn read_odt(z: &ZipReader) -> Option<Book> {
    z.read_string(ODT_PART).ok().and_then(|x| from_xml(&x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_book_goes_through_its_xml() {
        let mut b = Book::default();
        b.goal.words = 80000;
        b.goal.daily = 1000;
        b.goal.deadline = "2027-03-01".into();
        let s = b.scene_mut("_Scene1");
        s.status = SceneStatus::Revision;
        s.synopsis = "Ana llega a <Madrid> & duda".into();
        s.tags = vec!["viaje".into(), "lugar/ciudad".into()];
        s.pov = "Ana".into();
        b.scene_mut("_Scene2");
        b.progress.push(DayWords { date: "2026-10-03".into(), words: 1200 });
        b.snapshots.push(Snapshot { date: "2026-10-03 10:00".into(), label: "Antes de \"cortar\"".into(), anchor: "_Scene1".into(), title: "La <llegada>".into(), paras: vec!["La <llegada>".into(), String::new(), "  Ana & Luis  ".into()] });
        b.snapshots.push(Snapshot { date: "2026-10-03 11:00".into(), ..Default::default() });
        assert_eq!(from_xml(&to_xml(&b)), Some(b));
        assert_eq!(from_xml("<x/>"), None);
    }

    #[test]
    fn a_book_stays_a_book_in_docx_and_odt() {
        let mut doc = apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("Capítulo 1")]);
        let plain = crate::docx::read(&crate::docx::write(&doc)).unwrap();
        assert_eq!(plain.book, None);
        let mut b = Book::default();
        b.scene_mut("_Scene1").synopsis = "Empieza".into();
        doc.book = Some(b);
        let back = crate::docx::read(&crate::docx::write(&doc)).unwrap();
        assert_eq!(back.book, doc.book);
        assert!(back.kept.parts.is_empty());
        // Saved again, it is written once.
        let again = crate::docx::write(&back);
        let z = ZipReader::new(&again).unwrap();
        assert_eq!(z.names().filter(|n| n.starts_with("customXml/") && !n.contains("Props") && !n.contains("_rels")).count(), 1);
        assert_eq!(crate::odt::read(&crate::odt::write(&doc)).unwrap().book, doc.book);
    }
}
