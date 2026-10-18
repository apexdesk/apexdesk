//! Legacy form fields in .odt: LibreOffice's fieldmarks. A text field is a
//! `field:fieldmark-start` / `-separator` / `-end` around its text; check
//! boxes and drop-downs are a `field:fieldmark` holding their state. The
//! field's settings are `field:param`s: LibreOffice's names where it has
//! them ("Content", "Help", "Checkbox_Checked"...), ours for the rest.

use std::fmt::Write as _;

use apex_doc::{ContentControl, ControlKind, FormField, FormTextType};

use crate::xml::{Ev, Reader, Tag, esc};

const TEXT: &str = "vnd.oasis.opendocument.field.FORMTEXT";
const CHECKBOX: &str = "vnd.oasis.opendocument.field.FORMCHECKBOX";
const DROPDOWN: &str = "vnd.oasis.opendocument.field.FORMDROPDOWN";
/// The names LibreOffice gives fieldmarks without one.
const AUTO_NAME: &str = "__Fieldmark__";

/// The `field:param`s inside a fieldmark start tag, up to its end.
pub(crate) fn read_params(r: &mut Reader) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut depth = 1usize;
    loop {
        let ev = r.next();
        match &ev {
            Ev::Eof => break,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                if t.name == "field:param" {
                    let get = |k: &str| t.attr(k).map(|v| v.into_owned()).unwrap_or_default();
                    out.push((get("field:name"), get("field:value")));
                }
                if matches!(ev, Ev::Start(_)) {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
    out
}

/// The control for a fieldmark (`field:fieldmark-start` or
/// `field:fieldmark`) with its parameters; the text it shows when it isn't
/// a text field. `None` for other fieldmarks.
pub(crate) fn control(t: &Tag, params: &[(String, String)], id: u32) -> Option<(ContentControl, Option<String>)> {
    let ty = t.attr("field:type")?;
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let flag = |k: &str, d: bool| get(k).map_or(d, |v| v == "true" || v == "1");
    let name = t.attr("text:name").map(|v| v.into_owned()).unwrap_or_default();
    let mut f = FormField {
        name: if name.starts_with(AUTO_NAME) { String::new() } else { name },
        enabled: flag("Enabled", true),
        calc_on_exit: flag("CalcOnExit", false),
        entry_macro: get("EntryMacro").unwrap_or("").to_string(),
        exit_macro: get("ExitMacro").unwrap_or("").to_string(),
        help: get("Help").unwrap_or("").to_string(),
        help_auto: flag("HelpAutoText", false),
        status: get("Hint").unwrap_or("").to_string(),
        status_auto: flag("HintAutoText", false),
        label: get("Label").and_then(|v| v.parse().ok()),
        tab_index: get("TabIndex").and_then(|v| v.parse().ok()),
        ..Default::default()
    };
    Some(match ty.as_ref() {
        TEXT => {
            f.text_type = FormTextType::from_docx(get("Type").unwrap_or(""));
            f.default_text = get("Content").unwrap_or("").to_string();
            f.max_len = get("MaxLength").and_then(|v| v.parse().ok()).unwrap_or(0);
            f.format = get("Format").unwrap_or("").to_string();
            (ContentControl::form(id, ControlKind::PlainText, f), None)
        }
        CHECKBOX => {
            f.default_checked = flag("Checkbox_Default", false);
            f.check_size = get("Checkbox_Size").and_then(|v| v.parse().ok());
            let c = ContentControl::form(id, ControlKind::CheckBox { checked: flag("Checkbox_Checked", false) }, f);
            let text = c.form_text(0);
            (c, Some(text))
        }
        DROPDOWN => {
            f.default_item = get("Dropdown_Default").and_then(|v| v.parse().ok()).unwrap_or(0);
            let entries: Vec<&str> = params.iter().filter(|(n, _)| n == "Dropdown_ListEntry").map(|(_, v)| v.as_str()).collect();
            let pick = get("Dropdown_Selected").and_then(|v| v.parse().ok()).unwrap_or(0);
            let c = ContentControl::form(id, ControlKind::List { items: apex_doc::form_items(&entries), editable: false }, f);
            let text = c.form_text(pick);
            (c, Some(text))
        }
        _ => return None,
    })
}

fn param(body: &mut String, name: &str, value: &str) {
    let _ = write!(body, r#"<field:param field:name="{name}" field:value="{}"/>"#, esc(value));
}

/// Writes a form field's fieldmark: a check box or drop-down whole, the
/// start of a text field (its text and [`CLOSE`] follow).
pub(crate) fn open(body: &mut String, c: &ContentControl, f: &FormField, text: &str) {
    let ty = match c.kind {
        ControlKind::CheckBox { .. } => CHECKBOX,
        ControlKind::List { .. } => DROPDOWN,
        _ => TEXT,
    };
    let tag = if ty == TEXT { "field:fieldmark-start" } else { "field:fieldmark" };
    let name = if f.name.is_empty() { format!("{AUTO_NAME}{}", c.id) } else { f.name.clone() };
    let _ = write!(body, r#"<{tag} text:name="{}" field:type="{ty}">"#, esc(&name));
    match &c.kind {
        ControlKind::CheckBox { checked } => {
            param(body, "Checkbox_Checked", if *checked { "true" } else { "false" });
            if f.default_checked {
                param(body, "Checkbox_Default", "true");
            }
            if let Some(s) = f.check_size {
                param(body, "Checkbox_Size", &s.to_string());
            }
        }
        ControlKind::List { items, .. } => {
            for it in items {
                param(body, "Dropdown_ListEntry", &it.text);
            }
            param(body, "Dropdown_Selected", &apex_doc::form_item_index(items, text).to_string());
            if f.default_item > 0 {
                param(body, "Dropdown_Default", &f.default_item.to_string());
            }
        }
        _ => {
            if f.text_type != FormTextType::Regular {
                param(body, "Type", f.text_type.docx());
            }
            if !f.default_text.is_empty() {
                param(body, "Content", &f.default_text);
            }
            if !f.format.is_empty() {
                param(body, "Format", &f.format);
            }
            if f.max_len > 0 {
                param(body, "MaxLength", &f.max_len.to_string());
            }
        }
    }
    if !f.help.is_empty() {
        param(body, "Help", &f.help);
        if f.help_auto {
            param(body, "HelpAutoText", "true");
        }
    }
    if !f.status.is_empty() {
        param(body, "Hint", &f.status);
        if f.status_auto {
            param(body, "HintAutoText", "true");
        }
    }
    if !f.enabled {
        param(body, "Enabled", "false");
    }
    if f.calc_on_exit {
        param(body, "CalcOnExit", "true");
    }
    if !f.entry_macro.is_empty() {
        param(body, "EntryMacro", &f.entry_macro);
    }
    if !f.exit_macro.is_empty() {
        param(body, "ExitMacro", &f.exit_macro);
    }
    if let Some(l) = f.label {
        param(body, "Label", &l.to_string());
    }
    if let Some(i) = f.tab_index {
        param(body, "TabIndex", &i.to_string());
    }
    let _ = write!(body, "</{tag}>");
    if ty == TEXT {
        body.push_str("<field:fieldmark-separator/>");
    }
}

pub(crate) const CLOSE: &str = "<field:fieldmark-end/>";

/// The `ProtectForm` setting of settings.xml: only forms may be filled in.
pub(crate) fn protect_form(settings: &str) -> bool {
    let key = r#"config:name="ProtectForm""#;
    settings.find(key).is_some_and(|i| settings[i + key.len()..].split("</config:config-item>").next().is_some_and(|v| v.trim_end().ends_with(">true")))
}
