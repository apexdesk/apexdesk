//! Document properties (title, subject, keywords, company, custom ones…):
//! `docProps/core.xml`, `app.xml` and `custom.xml` in xlsx and docx, and
//! `meta.xml` in OpenDocument files.

use std::fmt::Write as _;

use apex_doc::{CustomProp, Meta, PropValue};

use crate::xml::{Ev, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
/// Format id of custom properties.
const FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";

/// Properties OpenDocument has no element for, kept as custom ones.
const ODF_EXTRA: [&str; 3] = ["Company", "Manager", "Category"];

/// The time now, as ISO 8601 in UTC ("2026-09-25T14:05:09Z").
fn now_iso() -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Reads the properties of an xlsx or docx package.
pub(crate) fn read_ooxml(z: &ZipReader) -> Meta {
    let mut m = Meta::default();
    if let Ok(xml) = z.read_string("docProps/core.xml") {
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) => {
                    let field = match t.local() {
                        "title" => &mut m.title,
                        "creator" => &mut m.author,
                        "subject" => &mut m.subject,
                        "keywords" => &mut m.keywords,
                        "description" => &mut m.description,
                        "category" => &mut m.category,
                        "created" => &mut m.created,
                        _ => continue,
                    };
                    *field = r.text().trim().to_string();
                }
                _ => {}
            }
        }
    }
    if let Ok(xml) = z.read_string("docProps/app.xml") {
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) if t.local() == "Company" => m.company = r.text().trim().to_string(),
                Ev::Start(t) if t.local() == "Manager" => m.manager = r.text().trim().to_string(),
                _ => {}
            }
        }
    }
    if let Ok(xml) = z.read_string("docProps/custom.xml") {
        let mut r = Reader::new(&xml);
        let mut name: Option<String> = None;
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) if t.local() == "property" => name = t.attr("name").map(|v| v.into_owned()),
                Ev::Start(t) if name.is_some() => {
                    let kind = t.local().to_string();
                    let text = r.text();
                    let value = match kind.as_str() {
                        "bool" => PropValue::Bool(matches!(text.trim(), "true" | "1")),
                        "i1" | "i2" | "i4" | "i8" | "int" | "ui1" | "ui2" | "ui4" | "ui8" | "uint" | "r4" | "r8" | "decimal" => match text.trim().parse() {
                            Ok(n) => PropValue::Number(n),
                            Err(_) => PropValue::Text(text),
                        },
                        "filetime" | "date" => PropValue::Date(text.trim().to_string()),
                        _ => PropValue::Text(text),
                    };
                    if let Some(n) = name.take() {
                        m.custom.push(CustomProp { name: n, value });
                    }
                }
                _ => {}
            }
        }
    }
    m
}

/// The content type override of the custom properties part, if any.
pub(crate) fn content_types(m: &Meta) -> &'static str {
    if m.custom.is_empty() { "" } else { r#"<Override PartName="/docProps/custom.xml" ContentType="application/vnd.openxmlformats-officedocument.custom-properties+xml"/>"# }
}

/// The package relationship of the custom properties part, if any.
pub(crate) fn rels(m: &Meta) -> &'static str {
    if m.custom.is_empty() { "" } else { r#"<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties" Target="docProps/custom.xml"/>"# }
}

/// Adds `docProps/core.xml`, `app.xml` and (with custom properties)
/// `custom.xml`.
pub(crate) fn write_ooxml(z: &mut ZipWriter, m: &Meta) {
    let mut core = String::from(HEADER);
    core.push_str(r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">"#);
    for (tag, v) in [("dc:title", &m.title), ("dc:subject", &m.subject), ("dc:creator", &m.author), ("cp:keywords", &m.keywords), ("dc:description", &m.description)] {
        if !v.is_empty() {
            let _ = write!(core, "<{tag}>{}</{tag}>", esc(v));
        }
    }
    if !m.category.is_empty() {
        let _ = write!(core, "<cp:category>{}</cp:category>", esc(&m.category));
    }
    let now = now_iso();
    let created = if m.created.is_empty() { &now } else { &m.created };
    let _ = write!(core, r#"<dcterms:created xsi:type="dcterms:W3CDTF">{}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{now}</dcterms:modified></cp:coreProperties>"#, esc(created));
    z.add("docProps/core.xml", core.as_bytes(), true);
    let mut app = format!(r#"{HEADER}<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Application>ApexDesk</Application>"#);
    if !m.manager.is_empty() {
        let _ = write!(app, "<Manager>{}</Manager>", esc(&m.manager));
    }
    if !m.company.is_empty() {
        let _ = write!(app, "<Company>{}</Company>", esc(&m.company));
    }
    app.push_str("</Properties>");
    z.add("docProps/app.xml", app.as_bytes(), true);
    if m.custom.is_empty() {
        return;
    }
    let mut c = format!(r#"{HEADER}<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">"#);
    for (i, p) in m.custom.iter().enumerate() {
        let v = match &p.value {
            PropValue::Text(s) => format!("<vt:lpwstr>{}</vt:lpwstr>", esc(s)),
            PropValue::Number(n) if n.fract() == 0.0 && n.abs() < 2e9 => format!("<vt:i4>{n:.0}</vt:i4>"),
            PropValue::Number(n) => format!("<vt:r8>{n}</vt:r8>"),
            PropValue::Bool(b) => format!("<vt:bool>{b}</vt:bool>"),
            PropValue::Date(d) => format!("<vt:filetime>{}</vt:filetime>", esc(&iso_full(d))),
        };
        let _ = write!(c, r#"<property fmtid="{FMTID}" pid="{}" name="{}">{v}</property>"#, i + 2, esc(&p.name));
    }
    c.push_str("</Properties>");
    z.add("docProps/custom.xml", c.as_bytes(), true);
}

/// A date with a time ("2026-09-25" → "2026-09-25T00:00:00Z").
fn iso_full(d: &str) -> String {
    if d.len() == 10 { format!("{d}T00:00:00Z") } else { d.to_string() }
}

/// Reads OpenDocument `meta.xml`.
pub(crate) fn read_odf(xml: &str) -> Meta {
    let mut m = Meta::default();
    let mut keywords = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => match t.name {
                "dc:title" => m.title = r.text().trim().to_string(),
                "dc:subject" => m.subject = r.text().trim().to_string(),
                "dc:description" => m.description = r.text().trim().to_string(),
                "meta:keyword" => keywords.push(r.text().trim().to_string()),
                "meta:creation-date" => m.created = r.text().trim().to_string(),
                "meta:initial-creator" => m.author = r.text().trim().to_string(),
                "dc:creator" if m.author.is_empty() => m.author = r.text().trim().to_string(),
                "meta:user-defined" => {
                    let name = t.attr("meta:name").map(|v| v.into_owned()).unwrap_or_default();
                    let kind = t.attr("meta:value-type").map(|v| v.into_owned()).unwrap_or_default();
                    let text = r.text();
                    match name.as_str() {
                        "Company" => m.company = text,
                        "Manager" => m.manager = text,
                        "Category" => m.category = text,
                        _ => {
                            let value = match kind.as_str() {
                                "float" | "percentage" | "currency" => text.trim().parse().map_or(PropValue::Text(text.clone()), PropValue::Number),
                                "boolean" => PropValue::Bool(text.trim() == "true"),
                                "date" => PropValue::Date(text.trim().to_string()),
                                _ => PropValue::Text(text),
                            };
                            m.custom.push(CustomProp { name, value });
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    m.keywords = keywords.join(", ");
    m
}

/// OpenDocument `meta.xml`.
pub(crate) fn odf_xml(m: &Meta) -> String {
    let mut s = format!(r#"{HEADER}<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" office:version="1.3"><office:meta><meta:generator>ApexDesk</meta:generator>"#);
    for (tag, v) in [("dc:title", &m.title), ("dc:subject", &m.subject), ("dc:description", &m.description), ("meta:initial-creator", &m.author)] {
        if !v.is_empty() {
            let _ = write!(s, "<{tag}>{}</{tag}>", esc(v));
        }
    }
    for k in m.keywords.split([',', ';']).map(str::trim).filter(|k| !k.is_empty()) {
        let _ = write!(s, "<meta:keyword>{}</meta:keyword>", esc(k));
    }
    let now = now_iso();
    let created = if m.created.is_empty() { &now } else { &m.created };
    let _ = write!(s, "<meta:creation-date>{}</meta:creation-date><dc:date>{now}</dc:date>", esc(created.trim_end_matches('Z')));
    for (name, v) in ODF_EXTRA.iter().zip([&m.company, &m.manager, &m.category]) {
        if !v.is_empty() {
            let _ = write!(s, r#"<meta:user-defined meta:name="{name}">{}</meta:user-defined>"#, esc(v));
        }
    }
    for p in &m.custom {
        let (kind, v) = match &p.value {
            PropValue::Text(t) => ("string", t.clone()),
            PropValue::Number(n) => ("float", n.to_string()),
            PropValue::Bool(b) => ("boolean", b.to_string()),
            PropValue::Date(d) => ("date", d.clone()),
        };
        let _ = write!(s, r#"<meta:user-defined meta:name="{}" meta:value-type="{kind}">{}</meta:user-defined>"#, esc(&p.name), esc(&v));
    }
    s.push_str("</office:meta></office:document-meta>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Meta {
        Meta {
            title: "Informe".into(),
            author: "Ana".into(),
            subject: "Ventas".into(),
            keywords: "ventas, 2026".into(),
            description: "Resumen <anual>".into(),
            category: "Finanzas".into(),
            company: "ACME".into(),
            manager: "Luis".into(),
            created: "2026-01-02T03:04:05Z".into(),
            custom: vec![
                CustomProp { name: "Proyecto".into(), value: PropValue::Text("Apex".into()) },
                CustomProp { name: "Versión".into(), value: PropValue::Number(3.0) },
                CustomProp { name: "Precio".into(), value: PropValue::Number(2.5) },
                CustomProp { name: "Final".into(), value: PropValue::Bool(true) },
                CustomProp { name: "Entrega".into(), value: PropValue::Date("2026-10-01T00:00:00Z".into()) },
            ],
            read_only_recommended: false,
            vars: Vec::new(),
        }
    }

    #[test]
    fn ooxml_properties() {
        let mut z = ZipWriter::new();
        write_ooxml(&mut z, &sample());
        let data = z.finish();
        let back = read_ooxml(&ZipReader::new(&data).unwrap());
        assert_eq!(back, sample());
        assert!(now_iso().len() == 20 && now_iso().ends_with('Z'));
    }

    #[test]
    fn odf_properties() {
        let mut back = read_odf(&odf_xml(&sample()));
        back.created.push('Z');
        assert_eq!(back, sample());
    }

    #[test]
    fn files_keep_properties() {
        let mut doc = apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("x")]);
        doc.meta = sample();
        assert_eq!(crate::docx::read(&crate::docx::write(&doc)).unwrap().meta, sample());
        let mut back = crate::odt::read(&crate::odt::write(&doc)).unwrap().meta;
        back.created.push('Z');
        assert_eq!(back, sample());
        let mut wb = apex_sheet::Workbook::new();
        wb.meta = sample();
        assert_eq!(crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap().meta, sample());
        let mut back = crate::ods::read(&crate::ods::write(&wb)).unwrap().meta;
        back.created.push('Z');
        assert_eq!(back, sample());
    }
}
