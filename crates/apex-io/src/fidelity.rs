//! Whether a document file holds what other programs show otherwise than
//! this one reads it. A signature over it would vouch for what its signer
//! never saw: Word inserts embedded chunks and picks the alternative
//! content it understands, which is read here only for shapes; it leaves
//! out elements of namespaces it doesn't know, which are read here by
//! their names; LibreOffice works out fields and hides sections and
//! paragraphs whose text is shown here as kept. Saving the document here
//! writes what is shown.

use crate::ooxml_keep::decode_xml;
use crate::xml::{Ev, Reader};
use crate::zip::ZipReader;

/// WordprocessingML (transitional and strict).
const W: [&str; 2] = ["http://schemas.openxmlformats.org/wordprocessingml/2006/main", "http://purl.oclc.org/ooxml/wordprocessingml/main"];

/// Elements that hold or make up text, as the reader takes them (by their
/// local names).
const TEXT: [&str; 18] = ["p", "r", "t", "tbl", "tr", "tc", "instrText", "delText", "fldSimple", "fldChar", "sym", "hyperlink", "sdt", "sdtContent", "ins", "moveTo", "smartTag", "customXml"];

/// LibreOffice's fields worked out from elsewhere in the file, and text
/// it can hide.
const ODF_COMPUTED: [&str; 9] = ["user-field-get", "user-field-input", "variable-get", "variable-input", "conditional-text", "hidden-text", "hidden-paragraph", "database-display", "expression"];

/// Whether the document `data` (a ZIP package) holds content other
/// programs show otherwise.
pub fn shown_otherwise(data: &[u8]) -> bool {
    let Ok(z) = ZipReader::new(data) else { return false };
    let names: Vec<String> = z.names().map(str::to_string).collect();
    names.iter().any(|n| {
        let l = n.to_ascii_lowercase();
        if l.ends_with(".rels") || l.starts_with("docprops/") || l.starts_with("customxml/") || l.starts_with("_xmlsignatures/") || l.starts_with("meta-inf/") || l == "[content_types].xml" {
            return false;
        }
        let Ok(b) = z.read(n) else { return false };
        let (text, _) = decode_xml(&b);
        if !text.trim_start().starts_with('<') {
            return false;
        }
        if l == "content.xml" || l == "styles.xml" { odf_otherwise(&text) } else { word_otherwise(&text) }
    })
}

/// A WordprocessingML part with text inside elements of other namespaces,
/// text elements of other namespaces, or embedded chunks.
fn word_otherwise(xml: &str) -> bool {
    let mut r = Reader::new(xml);
    // The open elements: whether WordprocessingML, and the namespaces
    // each declares.
    let mut open: Vec<(bool, Vec<(String, String)>)> = Vec::new();
    loop {
        let (t, empty) = match r.next() {
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            Ev::End(_) => {
                open.pop();
                continue;
            }
            Ev::Eof => return false,
            Ev::Text(_) => continue,
        };
        let decls: Vec<(String, String)> = t
            .attrs()
            .filter_map(|(k, v)| if k == "xmlns" { Some((String::new(), v.into_owned())) } else { k.strip_prefix("xmlns:").map(|p| (p.to_string(), v.into_owned())) })
            .collect();
        let (prefix, local) = t.name.split_once(':').unwrap_or(("", t.name));
        let uri = decls.iter().rev().chain(open.iter().rev().flat_map(|(_, d)| d.iter().rev())).find(|(p, _)| p == prefix).map_or("", |(_, u)| u.as_str());
        let word = W.contains(&uri);
        // Not a Word part.
        if open.is_empty() && !word {
            return false;
        }
        if local == "altChunk" {
            return true;
        }
        // Shapes, pictures, objects and equations are read on their own.
        if (word && matches!(local, "drawing" | "pict" | "object" | "txbxContent")) || matches!(local, "oMath" | "oMathPara") {
            if !empty {
                r.skip();
            }
            continue;
        }
        if TEXT.contains(&local) && (!word || open.iter().any(|(w, _)| !w)) {
            return true;
        }
        if !empty {
            open.push((word, decls));
        }
    }
}

/// An OpenDocument part with fields LibreOffice works out from elsewhere,
/// or text it may hide.
fn odf_otherwise(xml: &str) -> bool {
    let mut r = Reader::new(xml);
    loop {
        let t = match r.next() {
            Ev::Start(t) | Ev::Empty(t) => t,
            Ev::Eof => return false,
            _ => continue,
        };
        let local = t.local();
        if ODF_COMPUTED.contains(&local) {
            return true;
        }
        if local == "section" && (t.attr("text:condition").is_some() || t.attr("text:display").is_some_and(|d| d != "true")) {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docx(body: &str) -> Vec<u8> {
        let mut w = crate::zip::ZipWriter::new();
        let doc = format!(r#"<w:document xmlns:w="{}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:z="urn:z" xmlns:wps="urn:wps" mc:Ignorable="z"><w:body>{body}</w:body></w:document>"#, W[0]);
        w.add("word/document.xml", doc.as_bytes(), true);
        w.finish()
    }

    #[test]
    fn content_other_programs_show_otherwise_is_found() {
        // Plain text, shapes with alternatives, equations.
        assert!(!shown_otherwise(&docx(r#"<w:p><w:r><w:t>a</w:t></w:r></w:p>"#)));
        assert!(!shown_otherwise(&docx(
            r#"<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>box</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp></w:drawing></mc:Choice><mc:Fallback><w:pict/></mc:Fallback></mc:AlternateContent></w:r></w:p>"#
        )));
        assert!(!shown_otherwise(&docx(r#"<w:p><m:oMath xmlns:m="urn:m"><m:r><m:t>x</m:t></m:r></m:oMath></w:p>"#)));
        // Text in alternative content, or in elements of another namespace.
        assert!(shown_otherwise(&docx(r#"<w:p><mc:AlternateContent><mc:Choice Requires="w14"><w:r><w:t>10.000</w:t></w:r></mc:Choice><mc:Fallback/></mc:AlternateContent></w:p>"#)));
        assert!(shown_otherwise(&docx(r#"<z:x><w:p><w:r><w:t>100</w:t></w:r></w:p></z:x>"#)));
        assert!(shown_otherwise(&docx(r#"<z:p><w:r><w:t>100</w:t></w:r></z:p>"#)));
        // The main prefix bound elsewhere.
        assert!(shown_otherwise(&docx(r#"<w:p xmlns:w="urn:other"><w:r><w:t>100</w:t></w:r></w:p>"#)));
        assert!(shown_otherwise(&docx(r#"<w:altChunk r:id="rId9"/>"#)));
        // LibreOffice's worked-out fields and hidden sections.
        let odt = |body: &str| {
            let mut w = crate::zip::ZipWriter::new();
            w.add("content.xml", format!("<office:document-content><office:body><office:text>{body}</office:text></office:body></office:document-content>").as_bytes(), true);
            w.finish()
        };
        assert!(!shown_otherwise(&odt("<text:p>a</text:p><text:section text:name=\"s\"><text:p>b</text:p></text:section>")));
        assert!(shown_otherwise(&odt("<text:p><text:user-field-get text:name=\"price\">100</text:user-field-get></text:p>")));
        assert!(shown_otherwise(&odt("<text:section text:name=\"s\" text:display=\"none\"><text:p>b</text:p></text:section>")));
    }
}
