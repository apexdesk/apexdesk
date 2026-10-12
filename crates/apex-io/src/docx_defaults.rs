//! The document defaults in docx (`w:docDefaults` in styles.xml): their
//! size, language and spacing are read into `DocDefaults` (the fonts go to
//! `Document::default_font` and `Styles::asian`), and they are written back
//! as the file had them (see `apex_doc::FileDefaults`). Documents made in
//! ApexDesk get ours.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{DocDefaults, Document, FileDefaults, LineRule};

use crate::xml::{Ev, Reader, esc, local};

const TWIP: f32 = 20.0;

/// Children of `w:rPr` and `w:pPr` in schema order, which Word wants.
const RPR_ORDER: &[&str] = &[
    "rStyle", "rFonts", "b", "bCs", "i", "iCs", "caps", "smallCaps", "strike", "dstrike", "outline", "shadow", "emboss", "imprint", "noProof", "snapToGrid", "vanish", "webHidden", "color", "spacing", "w", "kern", "position", "sz", "szCs", "highlight", "u", "effect", "bdr", "shd", "fitText", "vertAlign", "rtl", "cs", "em", "lang", "eastAsianLayout", "specVanish", "oMath",
];
const PPR_ORDER: &[&str] = &[
    "pStyle", "keepNext", "keepLines", "pageBreakBefore", "framePr", "widowControl", "numPr", "suppressLineNumbers", "pBdr", "shd", "tabs", "suppressAutoHyphens", "kinsoku", "wordWrap", "overflowPunct", "topLinePunct", "autoSpaceDE", "autoSpaceDN", "bidi", "adjustRightInd", "snapToGrid", "spacing", "ind", "contextualSpacing", "mirrorIndents", "suppressOverlap", "jc", "textDirection", "textAlignment", "textboxTightWrap", "outlineLvl", "divId", "cnfStyle", "rPr", "sectPr", "pPrChange",
];

/// `xml` declaring the namespaces it uses besides `w` (from `ns`, the
/// declarations of styles.xml), to stand on its own in the styles.xml we
/// write; `None` if one isn't declared anywhere.
fn own_ns(xml: &str, ns: &HashMap<String, String>) -> Option<String> {
    let mut used: Vec<&str> = Vec::new();
    let mut declared: Vec<&str> = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => {
                for name in std::iter::once(t.name).chain(t.attrs().map(|(k, _)| k)) {
                    match name.split_once(':') {
                        Some(("xmlns", p)) => declared.push(p),
                        Some((p, _)) if !matches!(p, "w" | "xml") && !used.contains(&p) => used.push(p),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let mut decl = String::new();
    for p in used.into_iter().filter(|p| !declared.contains(p)) {
        let _ = write!(decl, r#" xmlns:{p}="{}""#, esc(ns.get(p)?));
    }
    let at = xml.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(xml.len());
    Some(format!("{}{decl}{}", &xml[..at], &xml[at..]))
}

/// Reads `w:docDefaults` of styles.xml; `None` if there are none.
pub(crate) fn read(styles: &str) -> Option<DocDefaults> {
    let mut r = Reader::new(styles);
    let mut ns = HashMap::new();
    let xml = loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) if t.local() == "styles" => ns = t.attrs().filter_map(|(k, v)| Some((k.strip_prefix("xmlns:")?.to_string(), v.into_owned()))).collect(),
            Ev::Start(t) if t.local() == "docDefaults" => break format!("<{}{}>{}", t.name, t.raw_attrs(), r.raw_element()),
            Ev::Empty(t) if t.local() == "docDefaults" => break format!("<{}{}/>", t.name, t.raw_attrs()),
            Ev::Start(t) if t.local() == "style" => return None,
            _ => {}
        }
    };
    let mut d = DocDefaults::default();
    let mut other = Vec::new();
    let mut r = Reader::new(&xml);
    // The properties element we are in: "rPr" or "pPr".
    let mut part: Option<&'static str> = None;
    loop {
        let ev = r.next();
        let is_start = matches!(ev, Ev::Start(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) if part.is_none() && matches!(t.local(), "rPr" | "pPr") => part = Some(if t.local() == "rPr" { "rPr" } else { "pPr" }),
            Ev::End(n) if matches!(local(n), "rPr" | "pPr") => part = None,
            Ev::Start(t) | Ev::Empty(t) if part.is_some() => {
                let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok());
                let known = match (part, t.local()) {
                    (Some("rPr"), "rFonts") => true,
                    (Some("rPr"), "sz") => {
                        d.size = t.attr("w:val").and_then(|v| v.parse().ok()).filter(|&s: &u16| s > 0);
                        true
                    }
                    (Some("rPr"), "szCs") => {
                        d.cs_size = t.attr("w:val").and_then(|v| v.parse().ok()).filter(|&s: &u16| s > 0);
                        true
                    }
                    (Some("rPr"), "lang") => {
                        d.lang = t.attr("w:val").filter(|v| !v.is_empty()).map(|v| Arc::from(v.as_ref()));
                        true
                    }
                    (Some("pPr"), "spacing") => {
                        d.space_before = tw("w:before").map(|v| v / TWIP);
                        d.space_after = tw("w:after").map(|v| v / TWIP);
                        d.line = tw("w:line").map(|l| match t.attr("w:lineRule").as_deref() {
                            Some("exact") => (LineRule::Exact, l / TWIP),
                            Some("atLeast") => (LineRule::AtLeast, l / TWIP),
                            _ => (LineRule::Multiple, l / 240.0),
                        });
                        true
                    }
                    (Some("pPr"), _) => crate::docx_asian::read_ppr(&t, &mut apex_doc::AsianPara::default()),
                    _ => false,
                };
                let raw = if is_start { format!("<{}{}>{}", t.name, t.raw_attrs(), r.raw_element()) } else { format!("<{}{}/>", t.name, t.raw_attrs()) };
                if let Some(raw) = own_ns(&raw, &ns).filter(|_| !known) {
                    other.push((Arc::from(part.unwrap_or_default()), Arc::from(raw.as_str())));
                }
            }
            _ => {}
        }
    }
    let xml = own_ns(&xml, &ns).unwrap_or_default();
    d.file = Some(Arc::new(FileDefaults { format: "docx".into(), xml: xml.as_str().into(), written: "".into(), other }));
    Some(d)
}

/// Notes how the defaults read are written, once the document has its
/// fonts (see `FileDefaults::written`). The ones we write for our
/// documents are ours again.
pub(crate) fn finish(doc: &mut Document) {
    if doc.styles.defaults.as_ref().and_then(|d| d.file.as_ref()).is_some_and(|f| *f.xml == *ours(doc)) {
        doc.styles.defaults = None;
        return;
    }
    let written = defaults_xml(doc);
    if let Some(f) = doc.styles.defaults.as_mut().and_then(|d| d.file.as_mut()) {
        Arc::make_mut(f).written = written.as_str().into();
    }
}

/// Elements of a string of empty elements of our own.
fn elements(s: &str) -> impl Iterator<Item = (String, String)> + '_ {
    s.split("<w:").filter(|e| !e.is_empty()).map(|e| {
        let name = e.split([' ', '/', '>']).next().unwrap_or_default().to_string();
        (name, format!("<w:{e}"))
    })
}

/// Joins elements in schema order.
fn in_order(mut items: Vec<(String, String)>, order: &[&str]) -> String {
    items.sort_by_key(|(n, _)| order.iter().position(|o| o == n).unwrap_or(order.len()));
    items.into_iter().map(|(_, x)| x).collect()
}

/// `w:docDefaults` from the document's defaults.
fn defaults_xml(doc: &Document) -> String {
    let Some(d) = &doc.styles.defaults else { return ours(doc) };
    let asian = &doc.styles.asian;
    let other = |part: &str| -> Vec<(String, String)> {
        d.file.iter().filter(|f| &*f.format == "docx").flat_map(|f| &f.other).filter(|(p, _)| &**p == part).map(|(_, x)| (local(x[1..].split([' ', '/', '>']).next().unwrap_or_default()).to_string(), x.to_string())).collect()
    };
    let mut rpr = other("rPr");
    let mut fonts = String::new();
    crate::docx_asian::write_rfonts(&mut fonts, doc.default_font_token().as_deref(), asian.text.as_deref());
    rpr.extend(elements(&fonts));
    if let Some(s) = d.size {
        rpr.push(("sz".into(), format!(r#"<w:sz w:val="{s}"/>"#)));
    }
    if let Some(s) = d.cs_size.or_else(|| asian.text.as_ref().and_then(|a| a.cs_size)) {
        rpr.push(("szCs".into(), format!(r#"<w:szCs w:val="{s}"/>"#)));
    }
    let mut lang = d.lang.as_deref().map(|l| format!(r#" w:val="{}""#, esc(l))).unwrap_or_default();
    lang.push_str(&asian.text.as_deref().map(crate::docx_asian::lang_attrs).unwrap_or_default());
    if !lang.is_empty() {
        rpr.push(("lang".into(), format!("<w:lang{lang}/>")));
    }
    let mut ppr = other("pPr");
    let mut flags = String::new();
    crate::docx_asian::write_ppr(&mut flags, &asian.para);
    crate::docx_asian::write_font_align(&mut flags, &asian.para);
    ppr.extend(elements(&flags));
    if d.space_before.is_some() || d.space_after.is_some() || d.line.is_some() {
        let mut sp = String::from("<w:spacing");
        if let Some(b) = d.space_before {
            let _ = write!(sp, r#" w:before="{}""#, ((b * TWIP).round() as i32).max(0));
        }
        if let Some(a) = d.space_after {
            let _ = write!(sp, r#" w:after="{}""#, ((a * TWIP).round() as i32).max(0));
        }
        match d.line {
            Some((LineRule::Multiple, l)) => {
                let _ = write!(sp, r#" w:line="{}" w:lineRule="auto""#, (l * 240.0).round() as i32);
            }
            Some((rule, l)) => {
                let _ = write!(sp, r#" w:line="{}" w:lineRule="{}""#, (l * TWIP).round() as i32, if rule == LineRule::Exact { "exact" } else { "atLeast" });
            }
            None => {}
        }
        sp.push_str("/>");
        ppr.push(("spacing".into(), sp));
    }
    let (rpr, ppr) = (in_order(rpr, RPR_ORDER), in_order(ppr, PPR_ORDER));
    let mut s = String::from("<w:docDefaults>");
    if !rpr.is_empty() {
        let _ = write!(s, "<w:rPrDefault><w:rPr>{rpr}</w:rPr></w:rPrDefault>");
    }
    if !ppr.is_empty() {
        let _ = write!(s, "<w:pPrDefault><w:pPr>{ppr}</w:pPr></w:pPrDefault>");
    }
    s.push_str("</w:docDefaults>");
    s
}

/// Our own defaults, for documents made in ApexDesk.
fn ours(doc: &Document) -> String {
    let mut font = String::new();
    let asian = &doc.styles.asian;
    crate::docx_asian::write_rfonts(&mut font, Some(doc.default_font_token().as_deref().unwrap_or("Calibri")), asian.text.as_deref());
    let lang = asian.text.as_deref().map(crate::docx_asian::lang_attrs).unwrap_or_default();
    let mut ppr = String::new();
    crate::docx_asian::write_ppr(&mut ppr, &asian.para);
    format!(r#"<w:docDefaults><w:rPrDefault><w:rPr>{font}<w:sz w:val="24"/><w:szCs w:val="24"/><w:lang w:val="es-ES"{lang}/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:widowControl/>{ppr}<w:spacing w:after="160" w:line="240" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>"#)
}

/// `w:docDefaults` for styles.xml: the file's as they were while nothing
/// in them changed.
pub(crate) fn write(doc: &Document) -> String {
    let xml = defaults_xml(doc);
    match doc.styles.defaults.as_ref().and_then(|d| d.file.as_deref()) {
        // Not with negative spacing, which isn't valid (some files have -1).
        Some(f) if &*f.format == "docx" && *f.written == *xml && !f.xml.is_empty() && !doc.styles.defaults.as_ref().is_some_and(|d| [d.space_before, d.space_after].into_iter().flatten().any(|v| v < 0.0)) => f.xml.to_string(),
        _ => xml,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_of_the_file() {
        let styles = r#"<w:styles xmlns:w="w" xmlns:w14="http://w14"><w:docDefaults><w:rPrDefault><w:rPr><w:kern w:val="2"/><w14:ligatures w14:val="standardContextual"/><x:y xmlns:x="x"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#;
        let f = read(styles).unwrap().file.unwrap();
        assert_eq!(&*f.xml, r#"<w:docDefaults xmlns:w14="http://w14"><w:rPrDefault><w:rPr><w:kern w:val="2"/><w14:ligatures w14:val="standardContextual"/><x:y xmlns:x="x"/></w:rPr></w:rPrDefault></w:docDefaults>"#);
        let other: Vec<&str> = f.other.iter().map(|(_, x)| &**x).collect();
        assert_eq!(other, [r#"<w:kern w:val="2"/>"#, r#"<w14:ligatures xmlns:w14="http://w14" w14:val="standardContextual"/>"#, r#"<x:y xmlns:x="x"/>"#]);
        assert!(own_ns("<v:x/>", &HashMap::new()).is_none());
    }
}
