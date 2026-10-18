//! Content controls in .docx: run-level `w:sdt` elements.

use std::fmt::Write as _;

use apex_doc::{ContentControl, ControlKind, DataBinding, ListItem};

use crate::xml::{Ev, Reader, esc, local};

/// Reads a `w:sdtPr`. `None` for the kinds kept as plain content
/// (citations, building blocks, equations, pictures, groups…).
pub(crate) fn read_pr(r: &mut Reader, id: u32) -> Option<ContentControl> {
    let mut c = ContentControl::new(id, ControlKind::RichText);
    let mut plain = false;
    let mut special = false;
    let mut kind: Option<ControlKind> = None;
    let mut items: Vec<ListItem> = Vec::new();
    let mut depth = 0usize;
    loop {
        let (t, empty) = match r.next() {
            Ev::Eof => break,
            Ev::End(n) => {
                if depth == 0 && local(n) == "sdtPr" {
                    break;
                }
                depth = depth.saturating_sub(1);
                continue;
            }
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            _ => continue,
        };
        if !empty {
            depth += 1;
        }
        let val = t.attr("val").map(|v| v.into_owned()).unwrap_or_default();
        match t.local() {
            "alias" => c.title = val,
            "tag" => c.tag = val,
            "lock" => {
                c.lock_delete = matches!(val.as_str(), "sdtLocked" | "sdtContentLocked");
                c.lock_content = matches!(val.as_str(), "contentLocked" | "sdtContentLocked");
            }
            "showingPlcHdr" => c.placeholder = true,
            "dataBinding" => {
                let a = |k: &str| t.attr(k).map(|v| v.into_owned()).unwrap_or_default();
                c.binding = Some(DataBinding { xpath: a("xpath"), store_item: a("storeItemID"), prefixes: a("prefixMappings") }).filter(|b| !b.xpath.is_empty());
            }
            "text" => plain = true,
            "checkbox" => kind = Some(ControlKind::CheckBox { checked: false }),
            "checked" => {
                if let Some(ControlKind::CheckBox { checked }) = &mut kind {
                    *checked = matches!(val.as_str(), "1" | "true" | "on");
                }
            }
            "comboBox" => kind = Some(ControlKind::List { items: Vec::new(), editable: true }),
            "dropDownList" => kind = Some(ControlKind::List { items: Vec::new(), editable: false }),
            "listItem" => {
                let text = t.attr("displayText").map(|v| v.into_owned()).unwrap_or_default();
                let value = t.attr("value").map(|v| v.into_owned()).unwrap_or_default();
                items.push(ListItem { text: if text.is_empty() { value.clone() } else { text }, value });
            }
            "date" => {
                let date = t.attr("fullDate").map(|v| v.chars().take(10).collect());
                kind = Some(ControlKind::Date { date, format: String::new() });
            }
            "dateFormat" => {
                if let Some(ControlKind::Date { format, .. }) = &mut kind {
                    *format = val;
                }
            }
            "citation" | "bibliography" | "docPartObj" | "docPartList" | "equation" | "picture" | "group" | "repeatingSection" | "repeatingSectionItem" => special = true,
            _ => {}
        }
    }
    if special {
        return None;
    }
    c.kind = match kind {
        Some(ControlKind::List { editable, .. }) => ControlKind::List { items, editable },
        Some(k) => k,
        None if plain => ControlKind::PlainText,
        None => ControlKind::RichText,
    };
    Some(c)
}

/// Opens a control (its properties and the start of its content).
pub(crate) fn open(body: &mut String, c: &ContentControl) {
    body.push_str("<w:sdt><w:sdtPr>");
    if !c.title.is_empty() {
        let _ = write!(body, r#"<w:alias w:val="{}"/>"#, esc(&c.title));
    }
    if !c.tag.is_empty() {
        let _ = write!(body, r#"<w:tag w:val="{}"/>"#, esc(&c.tag));
    }
    let _ = write!(body, r#"<w:id w:val="{}"/>"#, c.id);
    let lock = match (c.lock_delete, c.lock_content) {
        (true, true) => Some("sdtContentLocked"),
        (true, false) => Some("sdtLocked"),
        (false, true) => Some("contentLocked"),
        _ => None,
    };
    if let Some(l) = lock {
        let _ = write!(body, r#"<w:lock w:val="{l}"/>"#);
    }
    if c.placeholder {
        body.push_str("<w:showingPlcHdr/>");
    }
    if let Some(b) = &c.binding {
        let _ = write!(body, r#"<w:dataBinding w:prefixMappings="{}" w:xpath="{}" w:storeItemID="{}"/>"#, esc(&b.prefixes), esc(&b.xpath), esc(&b.store_item));
    }
    match &c.kind {
        ControlKind::RichText => {}
        ControlKind::PlainText => body.push_str("<w:text/>"),
        ControlKind::CheckBox { checked } => {
            let _ = write!(
                body,
                r#"<w14:checkbox><w14:checked w14:val="{}"/><w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox>"#,
                u8::from(*checked)
            );
        }
        ControlKind::List { items, editable } => {
            let tag = if *editable { "comboBox" } else { "dropDownList" };
            let _ = write!(body, "<w:{tag}>");
            for it in items {
                let _ = write!(body, r#"<w:listItem w:displayText="{}" w:value="{}"/>"#, esc(&it.text), esc(&it.value));
            }
            let _ = write!(body, "</w:{tag}>");
        }
        ControlKind::Date { date, format } => {
            match date {
                Some(d) => {
                    let _ = write!(body, r#"<w:date w:fullDate="{}T00:00:00Z">"#, esc(d));
                }
                None => body.push_str("<w:date>"),
            }
            let f = if format.is_empty() { "dd/MM/yyyy" } else { format };
            let _ = write!(body, r#"<w:dateFormat w:val="{}"/><w:lid w:val="es-ES"/><w:storeMappedDataAs w:val="dateTime"/><w:calendar w:val="gregorian"/></w:date>"#, esc(f));
        }
    }
    body.push_str("</w:sdtPr><w:sdtContent>");
}

pub(crate) const CLOSE: &str = "</w:sdtContent></w:sdt>";
