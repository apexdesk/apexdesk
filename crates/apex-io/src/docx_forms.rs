//! Legacy form fields in .docx: `FORMTEXT`, `FORMCHECKBOX` and
//! `FORMDROPDOWN` complex fields whose `w:fldChar` holds a `w:ffData`.
//! A text field's result is its text; check boxes and drop-downs have no
//! result (Word shows them from `w:ffData`).

use std::fmt::Write as _;

use apex_doc::{ContentControl, ControlKind, FORM_BLANK, FormField, FormTextType};

use crate::xml::{Ev, Reader, Tag, esc, local};

/// A `w:ffData`: the field and what its kind needs.
#[derive(Clone, Debug, Default)]
pub(crate) struct FfData {
    pub field: FormField,
    /// A check box: whether it is checked (`None`: as its default).
    pub checkbox: Option<Option<bool>>,
    /// A drop-down: its entries and the chosen one.
    pub dropdown: Option<(Vec<String>, Option<usize>)>,
}

fn on(t: &Tag) -> bool {
    t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off"))
}

fn val(t: &Tag) -> String {
    t.attr("w:val").map(|v| v.into_owned()).unwrap_or_default()
}

/// Reads the children of a `w:fldChar` start tag up to its end: the
/// `w:ffData`, if any.
pub(crate) fn read_fld_char(r: &mut Reader) -> Option<FfData> {
    let mut out: Option<FfData> = None;
    let mut depth = 1usize;
    // The element `w:default` and `w:type` belong to.
    let mut parent = "";
    loop {
        let (t, empty) = match r.next() {
            Ev::Eof => break,
            Ev::End(n) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                if matches!(local(n), "checkBox" | "ddList" | "textInput") {
                    parent = "";
                }
                continue;
            }
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            _ => continue,
        };
        if !empty {
            depth += 1;
        }
        let ff = out.get_or_insert_with(FfData::default);
        let f = &mut ff.field;
        match t.local() {
            "ffData" => {}
            "name" => f.name = val(&t),
            "label" => f.label = val(&t).parse().ok(),
            "tabIndex" => f.tab_index = val(&t).parse().ok(),
            "enabled" => f.enabled = on(&t),
            "calcOnExit" => f.calc_on_exit = on(&t),
            "entryMacro" => f.entry_macro = val(&t),
            "exitMacro" => f.exit_macro = val(&t),
            "helpText" => {
                f.help = val(&t);
                f.help_auto = t.attr("w:type").as_deref() == Some("autoText");
            }
            "statusText" => {
                f.status = val(&t);
                f.status_auto = t.attr("w:type").as_deref() == Some("autoText");
            }
            "checkBox" => {
                parent = "checkBox";
                ff.checkbox = Some(None);
            }
            "size" if parent == "checkBox" => f.check_size = val(&t).parse().ok(),
            "checked" if parent == "checkBox" => ff.checkbox = Some(Some(on(&t))),
            "ddList" => {
                parent = "ddList";
                ff.dropdown.get_or_insert_with(Default::default);
            }
            "result" if parent == "ddList" => {
                if let Some(d) = &mut ff.dropdown {
                    d.1 = val(&t).parse().ok();
                }
            }
            "listEntry" => {
                if let Some(d) = &mut ff.dropdown {
                    d.0.push(val(&t));
                }
            }
            "textInput" => parent = "textInput",
            "type" if parent == "textInput" => f.text_type = FormTextType::from_docx(&val(&t)),
            "maxLength" => f.max_len = val(&t).parse().unwrap_or(0),
            "format" => f.format = val(&t),
            "default" => match parent {
                "checkBox" => f.default_checked = on(&t),
                "ddList" => f.default_item = val(&t).parse().unwrap_or(0),
                "textInput" => f.default_text = val(&t),
                _ => {}
            },
            _ => {}
        }
    }
    out
}

/// The field word of an instruction, if a form field's.
pub(crate) fn form_word(instr: &str) -> Option<&'static str> {
    let w = instr.split_whitespace().next()?.to_ascii_uppercase();
    ["FORMTEXT", "FORMCHECKBOX", "FORMDROPDOWN"].into_iter().find(|k| *k == w)
}

/// The control for a form field read from a file (`word` from
/// [`form_word`]); its text for check boxes and drop-downs.
pub(crate) fn control(ff: FfData, word: &str, id: u32) -> (ContentControl, Option<String>) {
    match word {
        "FORMCHECKBOX" => {
            let checked = ff.checkbox.flatten().unwrap_or(ff.field.default_checked);
            let c = ContentControl::form(id, ControlKind::CheckBox { checked }, ff.field);
            let text = c.form_text(0);
            (c, Some(text))
        }
        "FORMDROPDOWN" => {
            let (entries, result) = ff.dropdown.unwrap_or_default();
            let pick = result.unwrap_or(ff.field.default_item as usize);
            let c = ContentControl::form(id, ControlKind::List { items: apex_doc::form_items(&entries), editable: false }, ff.field);
            let text = c.form_text(pick);
            (c, Some(text))
        }
        _ => (ContentControl::form(id, ControlKind::PlainText, ff.field), None),
    }
}

/// The `w:ffData` of a form field whose text is `text`.
fn ff_data(body: &mut String, c: &ContentControl, f: &FormField, text: &str) {
    body.push_str("<w:ffData>");
    let _ = write!(body, r#"<w:name w:val="{}"/>"#, esc(&f.name));
    if let Some(l) = f.label {
        let _ = write!(body, r#"<w:label w:val="{l}"/>"#);
    }
    if let Some(i) = f.tab_index {
        let _ = write!(body, r#"<w:tabIndex w:val="{i}"/>"#);
    }
    body.push_str(if f.enabled { "<w:enabled/>" } else { r#"<w:enabled w:val="0"/>"# });
    let _ = write!(body, r#"<w:calcOnExit w:val="{}"/>"#, u8::from(f.calc_on_exit));
    if !f.entry_macro.is_empty() {
        let _ = write!(body, r#"<w:entryMacro w:val="{}"/>"#, esc(&f.entry_macro));
    }
    if !f.exit_macro.is_empty() {
        let _ = write!(body, r#"<w:exitMacro w:val="{}"/>"#, esc(&f.exit_macro));
    }
    let kind = |auto: bool| if auto { "autoText" } else { "text" };
    if !f.help.is_empty() {
        let _ = write!(body, r#"<w:helpText w:type="{}" w:val="{}"/>"#, kind(f.help_auto), esc(&f.help));
    }
    if !f.status.is_empty() {
        let _ = write!(body, r#"<w:statusText w:type="{}" w:val="{}"/>"#, kind(f.status_auto), esc(&f.status));
    }
    match &c.kind {
        ControlKind::CheckBox { checked } => {
            body.push_str("<w:checkBox>");
            match f.check_size {
                Some(s) => {
                    let _ = write!(body, r#"<w:size w:val="{s}"/>"#);
                }
                None => body.push_str("<w:sizeAuto/>"),
            }
            let _ = write!(body, r#"<w:default w:val="{}"/>"#, u8::from(f.default_checked));
            if *checked != f.default_checked {
                let _ = write!(body, r#"<w:checked w:val="{}"/>"#, u8::from(*checked));
            }
            body.push_str("</w:checkBox>");
        }
        ControlKind::List { items, .. } => {
            let pick = apex_doc::form_item_index(items, text);
            let _ = write!(body, r#"<w:ddList><w:result w:val="{pick}"/><w:default w:val="{}"/>"#, f.default_item);
            for it in items {
                let _ = write!(body, r#"<w:listEntry w:val="{}"/>"#, esc(&it.text));
            }
            body.push_str("</w:ddList>");
        }
        _ => {
            body.push_str("<w:textInput>");
            if f.text_type != FormTextType::Regular {
                let _ = write!(body, r#"<w:type w:val="{}"/>"#, f.text_type.docx());
            }
            if !f.default_text.is_empty() {
                let _ = write!(body, r#"<w:default w:val="{}"/>"#, esc(&f.default_text));
            }
            if f.max_len > 0 {
                let _ = write!(body, r#"<w:maxLength w:val="{}"/>"#, f.max_len);
            }
            if !f.format.is_empty() {
                let _ = write!(body, r#"<w:format w:val="{}"/>"#, esc(&f.format));
            }
            body.push_str("</w:textInput>");
        }
    }
    body.push_str("</w:ffData>");
}

/// Opens a form field: its start, data and instruction, in runs with
/// `rpr`. Check boxes and drop-downs are whole (their text isn't written);
/// a text field's result follows and [`close`] ends it.
pub(crate) fn open(body: &mut String, c: &ContentControl, f: &FormField, text: &str, rpr: &str) {
    let _ = write!(body, r#"<w:r>{rpr}<w:fldChar w:fldCharType="begin">"#);
    ff_data(body, c, f, text);
    let word = match c.kind {
        ControlKind::CheckBox { .. } => "FORMCHECKBOX",
        ControlKind::List { .. } => "FORMDROPDOWN",
        _ => "FORMTEXT",
    };
    let _ = write!(body, r#"</w:fldChar></w:r><w:r>{rpr}<w:instrText xml:space="preserve"> {word} </w:instrText></w:r>"#);
    if word == "FORMTEXT" {
        let _ = write!(body, r#"<w:r>{rpr}<w:fldChar w:fldCharType="separate"/></w:r>"#);
    } else {
        let _ = write!(body, r#"<w:r>{rpr}<w:fldChar w:fldCharType="end"/></w:r>"#);
    }
}

/// Ends a text field.
pub(crate) fn close(body: &mut String, c: &ContentControl, rpr: &str) {
    if matches!(c.kind, ControlKind::PlainText | ControlKind::RichText) {
        let _ = write!(body, r#"<w:r>{rpr}<w:fldChar w:fldCharType="end"/></w:r>"#);
    }
}

/// Whether a text field's result is the blank Word shows when empty.
pub(crate) fn is_blank(text: &str) -> bool {
    text.is_empty() || text == FORM_BLANK
}
