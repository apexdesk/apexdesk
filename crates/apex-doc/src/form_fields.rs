//! Legacy form fields: Word's older text boxes, check boxes and drop-down
//! lists (the `FORMTEXT`, `FORMCHECKBOX` and `FORMDROPDOWN` fields of .doc
//! and .docx, LibreOffice's fieldmarks in .odt).
//!
//! In the text they are content controls ([`ContentControl`]) with
//! `legacy` set: a text field is plain text, a check box a box character
//! and a drop-down a list that isn't typed in. `legacy` keeps what else the
//! field has (name, default, limits, help texts and macros) for the files.

use std::hash::{Hash, Hasher};

use crate::{ContentControl, ControlKind, Document, ListItem};

/// What an empty text field shows (and files keep): five en spaces.
pub const FORM_BLANK: &str = "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}";

/// The kind of text a text field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FormTextType {
    #[default]
    Regular,
    Number,
    Date,
    CurrentDate,
    CurrentTime,
    Calculated,
}

impl FormTextType {
    pub const ALL: [FormTextType; 6] = [FormTextType::Regular, FormTextType::Number, FormTextType::Date, FormTextType::CurrentDate, FormTextType::CurrentTime, FormTextType::Calculated];

    /// The `w:type` value (and LibreOffice's "Type" parameter).
    pub fn docx(self) -> &'static str {
        match self {
            FormTextType::Regular => "regular",
            FormTextType::Number => "number",
            FormTextType::Date => "date",
            FormTextType::CurrentDate => "currentDate",
            FormTextType::CurrentTime => "currentTime",
            FormTextType::Calculated => "calculated",
        }
    }

    pub fn from_docx(v: &str) -> FormTextType {
        FormTextType::ALL.into_iter().find(|t| t.docx().eq_ignore_ascii_case(v)).unwrap_or_default()
    }

    /// The `iTypeTxt` of a .doc form field.
    pub fn from_doc(v: u16) -> FormTextType {
        FormTextType::ALL.get(v as usize).copied().unwrap_or_default()
    }
}

/// A legacy form field beyond its kind and value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FormField {
    /// Its bookmark name.
    pub name: String,
    /// Filled in by the user (a disabled field is fixed).
    pub enabled: bool,
    /// Fields that depend on it are updated when leaving it.
    pub calc_on_exit: bool,
    pub entry_macro: String,
    pub exit_macro: String,
    /// F1 help and status bar texts; `*_auto`: the text names an AutoText
    /// entry.
    pub help: String,
    pub help_auto: bool,
    pub status: String,
    pub status_auto: bool,
    /// Text fields: kind, default text, maximum length (0: any) and format
    /// ("UPPERCASE", "0.00", "dd/MM/yyyy"...).
    pub text_type: FormTextType,
    pub default_text: String,
    pub max_len: u32,
    pub format: String,
    /// Check boxes: size in half points (`None`: as the text) and whether
    /// they start checked.
    pub check_size: Option<u32>,
    pub default_checked: bool,
    /// Drop-downs: the item chosen at first.
    pub default_item: u32,
    /// Word's `w:label` and `w:tabIndex`, kept for the file.
    pub label: Option<i32>,
    pub tab_index: Option<u32>,
}

impl Default for FormField {
    fn default() -> Self {
        FormField {
            name: String::new(),
            enabled: true,
            calc_on_exit: false,
            entry_macro: String::new(),
            exit_macro: String::new(),
            help: String::new(),
            help_auto: false,
            status: String::new(),
            status_auto: false,
            text_type: FormTextType::Regular,
            default_text: String::new(),
            max_len: 0,
            format: String::new(),
            check_size: None,
            default_checked: false,
            default_item: 0,
            label: None,
            tab_index: None,
        }
    }
}

impl FormField {
    /// Text typed into the field as it goes in: letters cased as the
    /// format says, only number characters in a number field, and no more
    /// than the maximum length (`len`: the characters already there).
    pub fn fit_typed(&self, typed: &str, len: usize) -> String {
        let mut s: String = match self.text_type {
            FormTextType::Number => typed.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | '%' | '$' | '€' | ' ')).collect(),
            _ => typed.to_string(),
        };
        match self.format.to_ascii_lowercase().as_str() {
            "uppercase" => s = s.to_uppercase(),
            "lowercase" => s = s.to_lowercase(),
            _ => {}
        }
        if self.max_len > 0 {
            let room = (self.max_len as usize).saturating_sub(len);
            s = s.chars().take(room).collect();
        }
        s
    }

    /// Whether the user may change the field's value.
    pub fn fillable(&self) -> bool {
        self.enabled && !matches!(self.text_type, FormTextType::CurrentDate | FormTextType::CurrentTime | FormTextType::Calculated)
    }
}

impl ContentControl {
    /// A legacy form field of `kind` (plain text, check box or list).
    pub fn form(id: u32, kind: ControlKind, field: FormField) -> ContentControl {
        let mut c = ContentControl::new(id, kind);
        c.legacy = Some(Box::new(field));
        c
    }

    /// Whether its value can't be changed (locked, or a disabled form
    /// field).
    pub fn fixed(&self) -> bool {
        self.lock_content || self.legacy.as_ref().is_some_and(|f| !f.enabled)
    }

    /// Whether this is a legacy form field.
    pub fn is_form(&self) -> bool {
        self.legacy.is_some()
    }

    /// The text a check box or drop-down shows: the box, or item `pick`
    /// (en spaces for a drop-down without items).
    pub fn form_text(&self, pick: usize) -> String {
        match &self.kind {
            ControlKind::CheckBox { checked } => (if *checked { crate::CHECKED } else { crate::UNCHECKED }).to_string(),
            ControlKind::List { items, .. } => items.get(pick).or(items.first()).map_or(FORM_BLANK.to_string(), |i| i.text.clone()),
            _ => FORM_BLANK.to_string(),
        }
    }
}

/// The index of the item `text` shows in a drop-down (0 if none).
pub fn form_item_index(items: &[ListItem], text: &str) -> usize {
    items.iter().position(|i| i.text == text).unwrap_or(0)
}

/// The items of a drop-down from their names.
pub fn form_items<S: AsRef<str>>(names: &[S]) -> Vec<ListItem> {
    names.iter().map(|n| ListItem { text: n.as_ref().to_string(), value: n.as_ref().to_string() }).collect()
}

impl Document {
    /// A fingerprint of everything but what form fields and content
    /// controls hold: it stays the same while only forms are filled in
    /// (for the "filling in forms" protection).
    pub fn form_skeleton(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.paragraphs.len().hash(&mut h);
        for p in &self.paragraphs {
            format!("{:?}{:?}{:?}", p.style, p.cell, p.section).hash(&mut h);
            for (s, e, a) in p.spans() {
                match &a.control {
                    Some(c) => c.id.hash(&mut h),
                    None => {
                        p.text[s..e].hash(&mut h);
                        a.hash(&mut h);
                    }
                }
            }
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pos, Selection};

    #[test]
    fn typed_text_fits() {
        let f = FormField { max_len: 5, format: "UPPERCASE".into(), ..Default::default() };
        assert_eq!(f.fit_typed("abcdefg", 2), "ABC");
        let n = FormField { text_type: FormTextType::Number, ..Default::default() };
        assert_eq!(n.fit_typed("1a2,5", 0), "12,5");
        assert_eq!(FormTextType::from_docx("currentDate"), FormTextType::CurrentDate);
    }

    #[test]
    fn skeleton_ignores_field_values() {
        let mut d = Document::from_plain_text("Name: ");
        let c = ContentControl { placeholder: true, ..ContentControl::form(1, ControlKind::PlainText, FormField::default()) };
        d.insert_control(Selection::caret(Pos::new(0, 6)), c, FORM_BLANK);
        let before = d.form_skeleton();
        let at = d.control_at(Pos::new(0, 7)).unwrap();
        d.set_control(&at, ContentControl { placeholder: false, ..(*at.control).clone() }, "Ana", at.sel());
        assert_eq!(d.form_skeleton(), before);
        d.insert_text(Selection::caret(Pos::new(0, 0)), "x", None);
        assert_ne!(d.form_skeleton(), before);
    }
}
