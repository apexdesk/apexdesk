//! The document defaults in odt (the default paragraph style,
//! `style:default-style`): their font, size, language and spacing are read
//! into `DocDefaults` (the Asian part into `Styles::asian`) and written
//! back with the settings ApexDesk doesn't read (see
//! `apex_doc::FileDefaults`). Documents made in ApexDesk get ours.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{DocDefaults, Document, FileDefaults, LineRule};

use crate::parse_length_pt;
use crate::xml::{Ev, Reader, esc};

/// Attributes written from the model (or from `Document::auto_hyphenation`
/// and `Styles::asian`), not kept as they are.
fn modeled(part: &str, key: &str) -> bool {
    match part {
        "text" => {
            ["font-name", "font-family", "font-style-name", "font-pitch", "font-charset"].iter().any(|k| key.contains(k))
                || matches!(key, "fo:font-size" | "style:font-size-asian" | "style:font-size-complex" | "fo:language" | "fo:country" | "fo:script" | "style:language-asian" | "style:country-asian" | "style:script-asian" | "style:language-complex" | "style:country-complex" | "style:script-complex" | "style:rfc-language-tag" | "style:rfc-language-tag-asian" | "style:rfc-language-tag-complex" | "fo:hyphenate" | "style:text-emphasize")
                || key.starts_with("style:text-combine")
        }
        _ => matches!(key, "fo:margin-top" | "fo:margin-bottom" | "fo:line-height" | "style:line-height-at-least" | "style:line-break" | "style:punctuation-wrap" | "style:text-autospace" | "style:snap-to-layout-grid" | "style:vertical-align"),
    }
}

/// Slot of the default page layout (`style:default-page-layout`) in
/// `KeptParts::xml`.
pub(crate) const PAGE_SLOT: &str = "odt:default-page-layout";

/// The default page layout LibreOffice gives documents from Word: the
/// document grid in "standard mode", as Word lays it out.
pub(crate) const WORD_PAGE: &str = r#"<style:default-page-layout><style:page-layout-properties style:layout-grid-standard-mode="true"/></style:default-page-layout>"#;

/// Prefixes our styles.xml declares.
const PREFIXES: [&str; 9] = ["office", "style", "text", "table", "fo", "svg", "draw", "xlink", "loext"];

/// The default page layout of styles.xml, if it only uses the namespaces
/// we declare.
pub(crate) fn read_page(styles: &str) -> Option<String> {
    let mut r = Reader::new(styles);
    let raw = loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) if t.name == "style:default-page-layout" => break format!("<{}{}>{}", t.name, t.raw_attrs(), r.raw_element()),
            Ev::Start(t) if t.name == "office:automatic-styles" => return None,
            _ => {}
        }
    };
    let known = |n: &str| n.split_once(':').is_some_and(|(p, _)| PREFIXES.contains(&p));
    let mut r = Reader::new(&raw);
    loop {
        match r.next() {
            Ev::Eof => return Some(raw),
            Ev::Start(t) | Ev::Empty(t) if !known(t.name) || t.attrs().any(|(k, _)| !known(k)) => return None,
            _ => {}
        }
    }
}

/// Font families of the font declarations (`style:font-face`) by name.
pub(crate) fn font_faces(xml: &str) -> HashMap<String, Arc<str>> {
    let mut out = HashMap::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.name == "style:font-face" => {
                if let (Some(n), Some(f)) = (t.attr("style:name"), t.attr("svg:font-family")) {
                    let f = f.trim_matches(|c| c == '\'' || c == '"');
                    if !f.is_empty() {
                        out.insert(n.into_owned(), Arc::from(f));
                    }
                }
            }
            Ev::Start(t) if matches!(t.name, "office:styles" | "office:automatic-styles" | "office:body") => break,
            _ => {}
        }
    }
    out
}

/// A font name of the file: a declaration's family, else the name.
pub(crate) fn family(faces: &HashMap<String, Arc<str>>, name: &str) -> Arc<str> {
    faces.get(name).cloned().unwrap_or_else(|| Arc::from(name.trim_matches('\'')))
}

/// Whether styles.xml has a default paragraph style (LibreOffice's
/// defaults apply without one).
pub(crate) fn has_default_style(styles: &str) -> bool {
    let mut r = Reader::new(styles);
    loop {
        match r.next() {
            Ev::Eof => return false,
            Ev::Start(t) | Ev::Empty(t) if t.name == "style:default-style" && t.attr("style:family").as_deref() == Some("paragraph") => return true,
            _ => {}
        }
    }
}

/// Reads the default paragraph style of styles.xml: the defaults and the
/// Latin font; `None` if there is none, `Err` with the font if it is the
/// one we write for our documents (with the Asian defaults `asian`).
pub(crate) fn read(styles: &str, asian: &apex_doc::AsianDefaults) -> Option<Result<(DocDefaults, Option<Arc<str>>), Option<Arc<str>>>> {
    let faces = font_faces(styles);
    let mut r = Reader::new(styles);
    let xml = loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) if t.name == "style:default-style" && t.attr("style:family").as_deref() == Some("paragraph") => break format!("<{}{}>{}", t.name, t.raw_attrs(), r.raw_element()),
            _ => {}
        }
    };
    let mut d = DocDefaults::default();
    let mut font = None;
    let mut other = Vec::new();
    let mut r = Reader::new(&xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if matches!(t.name, "style:text-properties" | "style:paragraph-properties") => {
                let part = if t.name == "style:text-properties" { "text" } else { "para" };
                for (k, v) in t.attrs() {
                    // Hyphenation counts are positive integers (some files
                    // have "0", which means no limit to LibreOffice).
                    let invalid = matches!(k, "fo:hyphenation-remain-char-count" | "fo:hyphenation-push-char-count") && v.trim().parse::<u32>().is_ok_and(|n| n == 0);
                    if !modeled(part, k) && !invalid {
                        other.push((Arc::from(part), Arc::from(format!(r#" {k}="{}""#, esc(&v)).as_str())));
                    }
                }
                if part == "text" {
                    font = t.attr("style:font-name").map(|n| family(&faces, &n)).or_else(|| t.attr("fo:font-family").map(|f| Arc::from(f.trim_matches('\'')))).filter(|f| !f.is_empty());
                    d.size = t.attr("fo:font-size").and_then(|v| parse_length_pt(&v)).map(|pt| (pt * 2.0).round() as u16);
                    d.lang = match (t.attr("fo:language"), t.attr("fo:country")) {
                        (Some(l), c) if !l.is_empty() && l != "none" && l != "zxx" => Some(Arc::from(match c.filter(|c| !c.is_empty() && c != "none") {
                            Some(c) => format!("{l}-{c}"),
                            None => l.into_owned(),
                        }.as_str())),
                        _ => None,
                    };
                } else {
                    d.space_before = t.attr("fo:margin-top").and_then(|v| parse_length_pt(&v));
                    d.space_after = t.attr("fo:margin-bottom").and_then(|v| parse_length_pt(&v));
                    d.line = match t.attr("fo:line-height") {
                        Some(v) if v.ends_with('%') => v.trim_end_matches('%').parse::<f32>().ok().map(|p| (LineRule::Multiple, p / 100.0)),
                        Some(v) => parse_length_pt(&v).map(|p| (LineRule::Exact, p)),
                        None => t.attr("style:line-height-at-least").and_then(|v| parse_length_pt(&v)).map(|p| (LineRule::AtLeast, p)),
                    };
                }
            }
            _ => {}
        }
    }
    let mut own = Document::default();
    (own.default_font, own.styles.asian, own.auto_hyphenation) = (font.clone(), asian.clone(), xml.contains(r#"fo:hyphenate="true""#));
    if write(&own) == xml {
        // The font we write for documents without one is no font.
        return Some(Err(font.filter(|f| &**f != "Liberation Serif")));
    }
    d.file = Some(Arc::new(FileDefaults { format: "odt".into(), xml: xml.as_str().into(), written: "".into(), other }));
    Some(Ok((d, font)))
}

/// `style:default-style` of the paragraph family for styles.xml.
pub(crate) fn write(doc: &Document) -> String {
    let asian_para = crate::odt_asian::para_attrs(&doc.styles.asian.para);
    let asian_text = doc.styles.asian.text.as_deref().map(|x| crate::odt_asian::text_attrs(x, None)).unwrap_or_default();
    let hyphenate = if doc.auto_hyphenation { r#" fo:hyphenate="true""# } else { "" };
    let Some(d) = &doc.styles.defaults else {
        let font = crate::theme_xml::font_name(doc.default_font_token().as_deref().unwrap_or("Liberation Serif"));
        let para = if asian_para.is_empty() { asian_para } else { format!("<style:paragraph-properties{asian_para}/>") };
        return format!(r#"<style:default-style style:family="paragraph">{para}<style:text-properties fo:font-family="'{}'" fo:font-size="12pt" fo:language="es" fo:country="ES"{hyphenate}{asian_text}/></style:default-style>"#, esc(&font));
    };
    let other = |part: &str| -> String { d.file.iter().filter(|f| &*f.format == "odt").flat_map(|f| &f.other).filter(|(p, _)| &**p == part).map(|(_, a)| &**a).collect() };
    let mut para = other("para");
    para.push_str(&asian_para);
    if let Some(b) = d.space_before {
        let _ = write!(para, r#" fo:margin-top="{}pt""#, (b.max(0.0) * 100.0).round() / 100.0);
    }
    if let Some(a) = d.space_after {
        let _ = write!(para, r#" fo:margin-bottom="{}pt""#, (a.max(0.0) * 100.0).round() / 100.0);
    }
    match d.line {
        Some((LineRule::Multiple, l)) => {
            let _ = write!(para, r#" fo:line-height="{}%""#, (l * 100.0).round() as i32);
        }
        Some((LineRule::Exact, l)) => {
            let _ = write!(para, r#" fo:line-height="{}pt""#, (l * 100.0).round() / 100.0);
        }
        Some((LineRule::AtLeast, l)) => {
            let _ = write!(para, r#" style:line-height-at-least="{}pt""#, (l * 100.0).round() / 100.0);
        }
        None => {}
    }
    let mut text = other("text");
    if let Some(f) = doc.default_font_token() {
        let _ = write!(text, r#" fo:font-family="'{}'""#, esc(&crate::theme_xml::font_name(&f)));
    }
    if let Some(s) = d.size {
        let _ = write!(text, r#" fo:font-size="{}pt""#, s as f32 / 2.0);
    }
    if let Some((lang, country)) = d.lang.as_deref().and_then(crate::odt_asian::split_lang) {
        let _ = write!(text, r#" fo:language="{}""#, esc(lang));
        if let Some(country) = country {
            let _ = write!(text, r#" fo:country="{}""#, esc(country));
        }
    }
    text.push_str(hyphenate);
    text.push_str(&asian_text);
    if let Some(s) = d.cs_size.filter(|_| doc.styles.asian.text.as_ref().is_none_or(|a| a.cs_size.is_none())) {
        let _ = write!(text, r#" style:font-size-complex="{}pt""#, s as f32 / 2.0);
    }
    let mut s = String::from(r#"<style:default-style style:family="paragraph">"#);
    if !para.is_empty() {
        let _ = write!(s, "<style:paragraph-properties{para}/>");
    }
    if !text.is_empty() {
        let _ = write!(s, "<style:text-properties{text}/>");
    }
    s.push_str("</style:default-style>");
    s
}
