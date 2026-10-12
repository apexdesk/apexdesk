//! Content controls in .odt: LibreOffice's `loext:content-control`.

use std::fmt::Write as _;

use apex_doc::{ContentControl, ControlKind, ListItem};

use crate::xml::{Tag, esc};

fn on(t: &Tag, k: &str) -> bool {
    t.attr(k).as_deref() == Some("true")
}

/// A `loext:content-control` start tag (its list items come after).
pub(crate) fn read(t: &Tag, id: u32) -> ContentControl {
    let kind = if on(t, "loext:checkbox") {
        ControlKind::CheckBox { checked: on(t, "loext:checked") }
    } else if on(t, "loext:dropdown") || on(t, "loext:combobox") {
        ControlKind::List { items: Vec::new(), editable: on(t, "loext:combobox") }
    } else if on(t, "loext:date") {
        let date = t.attr("loext:current-date").map(|v| v.chars().take(10).collect());
        ControlKind::Date { date, format: t.attr("loext:date-format").map(|v| v.into_owned()).unwrap_or_default() }
    } else if on(t, "loext:plain-text") {
        ControlKind::PlainText
    } else {
        ControlKind::RichText
    };
    let lock = t.attr("loext:lock").unwrap_or_default();
    ContentControl {
        id,
        kind,
        title: t.attr("loext:alias").map(|v| v.into_owned()).unwrap_or_default(),
        tag: t.attr("loext:tag").map(|v| v.into_owned()).unwrap_or_default(),
        placeholder: on(t, "loext:showing-place-holder"),
        lock_delete: matches!(lock.as_ref(), "sdtLocked" | "sdtContentLocked"),
        lock_content: matches!(lock.as_ref(), "contentLocked" | "sdtContentLocked"),
        binding: None,
        legacy: None,
    }
}

/// A `loext:list-item` of the control.
pub(crate) fn list_item(t: &Tag, c: &mut ContentControl) {
    if let ControlKind::List { items, .. } = &mut c.kind {
        let text = t.attr("loext:display-text").map(|v| v.into_owned()).unwrap_or_default();
        let value = t.attr("loext:value").map(|v| v.into_owned()).unwrap_or_default();
        items.push(ListItem { text: if text.is_empty() { value.clone() } else { text }, value });
    }
}

pub(crate) fn open(body: &mut String, c: &ContentControl) {
    body.push_str("<loext:content-control");
    if c.placeholder {
        body.push_str(r#" loext:showing-place-holder="true""#);
    }
    match &c.kind {
        ControlKind::RichText => {}
        ControlKind::PlainText => body.push_str(r#" loext:plain-text="true""#),
        ControlKind::CheckBox { checked } => {
            let _ = write!(body, r#" loext:checkbox="true" loext:checked="{checked}" loext:checked-state="☒" loext:unchecked-state="☐""#);
        }
        ControlKind::List { editable, .. } => body.push_str(if *editable { r#" loext:combobox="true""# } else { r#" loext:dropdown="true""# }),
        ControlKind::Date { date, format } => {
            let f = if format.is_empty() { "dd/MM/yyyy" } else { format };
            let _ = write!(body, r#" loext:date="true" loext:date-format="{}" loext:date-rfc-language-tag="es-ES""#, esc(f));
            if let Some(d) = date {
                let _ = write!(body, r#" loext:current-date="{}T00:00:00Z""#, esc(d));
            }
        }
    }
    if !c.title.is_empty() {
        let _ = write!(body, r#" loext:alias="{}""#, esc(&c.title));
    }
    if !c.tag.is_empty() {
        let _ = write!(body, r#" loext:tag="{}""#, esc(&c.tag));
    }
    let lock = match (c.lock_delete, c.lock_content) {
        (true, true) => Some("sdtContentLocked"),
        (true, false) => Some("sdtLocked"),
        (false, true) => Some("contentLocked"),
        _ => None,
    };
    if let Some(l) = lock {
        let _ = write!(body, r#" loext:lock="{l}""#);
    }
    body.push('>');
    if let ControlKind::List { items, .. } = &c.kind {
        for it in items {
            let _ = write!(body, r#"<loext:list-item loext:display-text="{}" loext:value="{}"/>"#, esc(&it.text), esc(&it.value));
        }
    }
}

pub(crate) const CLOSE: &str = "</loext:content-control>";
