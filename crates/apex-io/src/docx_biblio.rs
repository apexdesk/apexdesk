//! Bibliography sources in .docx files: a custom XML part
//! (`customXml/itemN.xml`) holding `b:Sources`, as Word keeps them.

use std::fmt::Write as _;

use apex_doc::{Bibliography, CiteStyle, Source, SourceKind};

use crate::xml::{Ev, HEADER, Reader, esc, local};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/bibliography";
pub(crate) const REL: &str = r#"<Relationship Id="rIdB" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml" Target="../customXml/item1.xml"/>"#;
pub(crate) const CONTENT_TYPE: &str = r#"<Override PartName="/customXml/itemProps1.xml" ContentType="application/vnd.openxmlformats-officedocument.customXmlProperties+xml"/>"#;

fn kind_of(v: &str) -> SourceKind {
    match v {
        "Book" | "BookSection" => SourceKind::Book,
        "JournalArticle" | "ArticleInAPeriodical" | "ConferenceProceedings" => SourceKind::Article,
        "InternetSite" | "DocumentFromInternetSite" => SourceKind::Web,
        "Report" => SourceKind::Report,
        _ => SourceKind::Other,
    }
}

fn kind_name(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Book => "Book",
        SourceKind::Article => "JournalArticle",
        SourceKind::Web => "InternetSite",
        SourceKind::Report => "Report",
        SourceKind::Other => "Misc",
    }
}

/// The sources of the file, if it has any.
pub(crate) fn read(z: &ZipReader) -> Bibliography {
    let mut b = Bibliography::default();
    let names: Vec<String> = z.names().filter(|n| n.starts_with("customXml/item") && !n.contains("Props") && n.ends_with(".xml")).map(String::from).collect();
    for n in names {
        let Ok(xml) = z.read_string(&n) else { continue };
        if !xml.contains(NS) {
            continue;
        }
        parse(&xml, &mut b);
    }
    b
}

fn parse(xml: &str, b: &mut Bibliography) {
    let mut r = Reader::new(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut cur: Option<Source> = None;
    let mut person: Option<(String, String)> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => {
                let name = t.local();
                match name {
                    "Sources" => {
                        let s = t.attr("StyleName").map(|v| v.to_uppercase()).unwrap_or_default();
                        let sel = t.attr("SelectedStyle").map(|v| v.to_uppercase()).unwrap_or_default();
                        b.style = if s.contains("IEEE") || sel.contains("IEEE") {
                            CiteStyle::Ieee
                        } else if (s.contains("CHICAGO") || sel.contains("CHICAGO")) && s.contains("NOTE") {
                            CiteStyle::ChicagoNotes
                        } else if s.contains("CHICAGO") || sel.contains("CHICAGO") {
                            CiteStyle::ChicagoAuthorDate
                        } else if s.contains("MLA") || sel.contains("MLA") {
                            CiteStyle::Mla
                        } else {
                            CiteStyle::Apa
                        };
                    }
                    "Source" => cur = Some(Source::default()),
                    "Person" => person = Some(Default::default()),
                    _ => {}
                }
                let leaf = matches!(name, "Tag" | "SourceType" | "Title" | "Year" | "City" | "Publisher" | "JournalName" | "PeriodicalTitle" | "InternetSiteTitle" | "Volume" | "Pages" | "URL" | "Last" | "First" | "Middle" | "Corporate");
                if !leaf {
                    stack.push(name.to_string());
                    continue;
                }
                // Only the authors (not editors, translators…).
                let author_role = stack.iter().rev().nth(if name == "Corporate" { 0 } else { 2 }).is_some_and(|x| x == "Author") && stack.iter().filter(|x| *x == "Author").count() >= 2;
                let v = r.text();
                let Some(s) = cur.as_mut() else { continue };
                match name {
                    "Tag" if stack.last().is_some_and(|x| x == "Source") => s.tag = v,
                    "SourceType" => s.kind = kind_of(v.trim()),
                    "Title" if stack.last().is_some_and(|x| x == "Source") => s.title = v,
                    "Year" => s.year = v,
                    "City" => s.city = v,
                    "Publisher" => s.publisher = v,
                    "JournalName" | "PeriodicalTitle" | "InternetSiteTitle" => s.journal = v,
                    "Volume" => s.volume = v,
                    "Pages" => s.pages = v,
                    "URL" => s.url = v,
                    "Last" if author_role => {
                        if let Some(p) = person.as_mut() {
                            p.0 = v;
                        }
                    }
                    "First" | "Middle" if author_role => {
                        if let Some(p) = person.as_mut() {
                            if !p.1.is_empty() {
                                p.1.push(' ');
                            }
                            p.1.push_str(&v);
                        }
                    }
                    "Corporate" if author_role => add_author(s, &v, ""),
                    _ => {}
                }
            }
            Ev::End(n) => {
                let n = local(n);
                stack.pop();
                match n {
                    "Person" => {
                        if let (Some((l, f)), Some(s)) = (person.take(), cur.as_mut())
                            && stack.iter().filter(|x| *x == "Author").count() >= 2
                        {
                            add_author(s, &l, &f);
                        }
                    }
                    "Source" => {
                        if let Some(s) = cur.take()
                            && !s.tag.is_empty()
                            && b.source(&s.tag).is_none()
                        {
                            b.sources.push(s);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn add_author(s: &mut Source, last: &str, first: &str) {
    if last.trim().is_empty() {
        return;
    }
    if !s.authors.is_empty() {
        s.authors.push_str("; ");
    }
    s.authors.push_str(last.trim());
    if !first.trim().is_empty() {
        let _ = write!(s.authors, ", {}", first.trim());
    }
}

/// Whether `write` writes a part for `b`.
pub(crate) fn written(b: &Bibliography) -> bool {
    !b.sources.is_empty() || b.style != CiteStyle::default()
}

/// Writes the sources part (with its properties) when there are sources
/// or a style other than the default: Word keeps the chosen style there
/// even with no sources.
pub(crate) fn write(z: &mut ZipWriter, b: &Bibliography) -> bool {
    if !written(b) {
        return false;
    }
    let (name, xsl) = match b.style {
        CiteStyle::Apa => ("APA", "\\APASixthEditionOfficeOnline.xsl"),
        CiteStyle::Mla => ("MLA", "\\MLASeventhEditionOfficeOnline.xsl"),
        CiteStyle::Ieee => ("IEEE", "\\IEEE2006OfficeOnline.xsl"),
        // Word's Chicago style; the variant is kept in the style name.
        CiteStyle::ChicagoAuthorDate => ("Chicago", "\\ChicagoSixteenthEditionOfficeOnline.xsl"),
        CiteStyle::ChicagoNotes => ("Chicago Notes", "\\ChicagoSixteenthEditionOfficeOnline.xsl"),
    };
    let mut x = format!(r#"{HEADER}<b:Sources SelectedStyle="{xsl}" StyleName="{name}" xmlns:b="{NS}" xmlns="{NS}">"#);
    for s in &b.sources {
        let _ = write!(x, "<b:Source><b:Tag>{}</b:Tag><b:SourceType>{}</b:SourceType>", esc(&s.tag), kind_name(s.kind));
        let people = s.people();
        if !people.is_empty() {
            x.push_str("<b:Author><b:Author>");
            if people.len() == 1 && people[0].1.is_empty() && s.authors.split_whitespace().count() > 1 {
                let _ = write!(x, "<b:Corporate>{}</b:Corporate>", esc(&people[0].0));
            } else {
                x.push_str("<b:NameList>");
                for (l, f) in &people {
                    let _ = write!(x, "<b:Person><b:Last>{}</b:Last>", esc(l));
                    if !f.is_empty() {
                        let _ = write!(x, "<b:First>{}</b:First>", esc(f));
                    }
                    x.push_str("</b:Person>");
                }
                x.push_str("</b:NameList>");
            }
            x.push_str("</b:Author></b:Author>");
        }
        let journal = match s.kind {
            SourceKind::Web => "InternetSiteTitle",
            _ => "JournalName",
        };
        for (tag, v) in [("Title", &s.title), ("Year", &s.year), ("City", &s.city), ("Publisher", &s.publisher), (journal, &s.journal), ("Volume", &s.volume), ("Pages", &s.pages), ("URL", &s.url)] {
            if !v.is_empty() {
                let _ = write!(x, "<b:{tag}>{}</b:{tag}>", esc(v));
            }
        }
        x.push_str("</b:Source>");
    }
    x.push_str("</b:Sources>");
    z.add("customXml/item1.xml", x.as_bytes(), true);
    z.add("customXml/itemProps1.xml", format!(r#"{HEADER}<ds:datastoreItem ds:itemID="{{5B1E8A2C-4D3F-4E6A-9C1B-7A2D3E4F5A6B}}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs><ds:schemaRef ds:uri="{NS}"/></ds:schemaRefs></ds:datastoreItem>"#).as_bytes(), true);
    z.add("customXml/_rels/item1.xml.rels", format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps" Target="itemProps1.xml"/></Relationships>"#).as_bytes(), true);
    true
}
