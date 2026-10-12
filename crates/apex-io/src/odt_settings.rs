//! LibreOffice's document settings in odt (`ooo:configuration-settings` of
//! settings.xml): compatibility options that change the layout, like
//! Word's grid metrics in documents from Word, kept for the next save; and
//! the form protection, written from the document.

use std::fmt::Write as _;

use apex_doc::Document;

use crate::xml::{Ev, Reader};

/// Slot of the kept settings in `KeptParts::xml`.
pub(crate) const SLOT: &str = "odt:settings";

/// Word's grid metrics, which LibreOffice turns on in documents from Word
/// (the document grid of our layout is Word's).
pub(crate) const WORD_GRID: &str = r#"<config:config-item config:name="MsWordCompGridMetrics" config:type="boolean">true</config:config-item>"#;

/// Settings that name this computer's printers and databases: they don't
/// travel with the document.
const LOCAL: [&str; 7] = ["PrinterName", "PrinterSetup", "PrintFaxName", "CurrentDatabaseDataSource", "CurrentDatabaseCommand", "CurrentDatabaseCommandType", "EmbeddedDatabaseName"];

/// The configuration settings of settings.xml, but the form protection
/// and the local ones.
pub(crate) fn read(xml: &str) -> Option<String> {
    let mut r = Reader::new(xml);
    let mut out = String::new();
    let mut inside = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "config:config-item-set" && !inside => {
                if t.attr("config:name").as_deref() == Some("ooo:configuration-settings") {
                    inside = true;
                } else {
                    r.skip();
                }
            }
            Ev::End("config:config-item-set") if inside => break,
            Ev::Start(t) if inside => {
                let raw = format!("<{}{}>{}", t.name, t.raw_attrs(), r.raw_element());
                if !matches!(t.attr("config:name").as_deref(), Some("ProtectForm" | crate::odt_fonts::SETTING)) && !t.attr("config:name").is_some_and(|n| LOCAL.contains(&&*n)) {
                    out.push_str(&raw);
                }
            }
            Ev::Empty(t) if inside && !matches!(t.attr("config:name").as_deref(), Some("ProtectForm" | crate::odt_fonts::SETTING)) && !t.attr("config:name").is_some_and(|n| LOCAL.contains(&&*n)) => {
                let _ = write!(out, "<{}{}/>", t.name, t.raw_attrs());
            }
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

/// settings.xml for the document; `None` if it has nothing to set.
pub(crate) fn write(doc: &Document) -> Option<String> {
    let mut items = doc.kept.xml.iter().find(|(slot, _)| slot == SLOT).map(|(_, x)| x.clone()).unwrap_or_default();
    // Filling in forms only: LibreOffice's "protect form" setting.
    if doc.edit_limit() == Some(apex_doc::EditLimit::Forms) {
        items.push_str(r#"<config:config-item config:name="ProtectForm" config:type="boolean">true</config:config-item>"#);
    }
    if doc.fonts.embed {
        let _ = write!(items, r#"<config:config-item config:name="{}" config:type="boolean">true</config:config-item>"#, crate::odt_fonts::SETTING);
    }
    if items.is_empty() {
        return None;
    }
    // LibreOffice finds the set by its qualified name: `ooo` must be declared.
    Some(format!(
        r#"{}<office:document-settings xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0" xmlns:ooo="http://openoffice.org/2004/office" office:version="1.3"><office:settings><config:config-item-set config:name="ooo:configuration-settings">{items}</config:config-item-set></office:settings></office:document-settings>"#,
        crate::xml::HEADER
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_but_form_protection() {
        let xml = r#"<office:document-settings xmlns:office="o" xmlns:config="c" xmlns:ooo="x"><office:settings><config:config-item-set config:name="ooo:view-settings"><config:config-item config:name="ViewAreaTop" config:type="long">0</config:config-item></config:config-item-set><config:config-item-set config:name="ooo:configuration-settings"><config:config-item config:name="ProtectForm" config:type="boolean">false</config:config-item><config:config-item config:name="MsWordCompGridMetrics" config:type="boolean">true</config:config-item><config:config-item-map-indexed config:name="ForbiddenCharacters"><config:config-item-map-entry/></config:config-item-map-indexed></config:config-item-set></office:settings></office:document-settings>"#;
        let items = read(xml).unwrap();
        assert!(items.starts_with(WORD_GRID) && items.contains("ForbiddenCharacters") && !items.contains("ProtectForm") && !items.contains("ViewAreaTop"), "{items}");
        let mut doc = Document::default();
        assert!(write(&doc).is_none());
        doc.kept.xml.push((SLOT.into(), items));
        let s = write(&doc).unwrap();
        assert!(s.contains(r#"xmlns:ooo="http://openoffice.org/2004/office""#) && s.contains(WORD_GRID));
    }
}
