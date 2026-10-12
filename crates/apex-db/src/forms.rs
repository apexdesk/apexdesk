//! Forms: a record at a time, a list of records, or a record with a
//! subform of related rows; made of labels, text boxes, dropdowns (values
//! from another table), checkboxes and buttons with fixed actions. The
//! form wizard's layouts are made here too ([`FormDef::wizard`]).

use crate::objects::Record;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FormKind {
    /// One record at a time.
    #[default]
    Single,
    /// Records in rows.
    List,
    /// One record and its related rows of another table below.
    WithSubform,
}

impl FormKind {
    pub const ALL: [FormKind; 3] = [FormKind::Single, FormKind::List, FormKind::WithSubform];

    pub fn key(self) -> &'static str {
        match self {
            FormKind::Single => "single",
            FormKind::List => "list",
            FormKind::WithSubform => "subform",
        }
    }

    pub fn from_key(k: &str) -> FormKind {
        FormKind::ALL.into_iter().find(|x| x.key() == k).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CtlKind {
    #[default]
    Label,
    TextBox,
    /// A dropdown of values from another table.
    ComboBox,
    CheckBox,
    Button,
}

impl CtlKind {
    pub const ALL: [CtlKind; 5] = [CtlKind::Label, CtlKind::TextBox, CtlKind::ComboBox, CtlKind::CheckBox, CtlKind::Button];

    pub fn key(self) -> &'static str {
        match self {
            CtlKind::Label => "label",
            CtlKind::TextBox => "textbox",
            CtlKind::ComboBox => "combobox",
            CtlKind::CheckBox => "checkbox",
            CtlKind::Button => "button",
        }
    }

    pub fn from_key(k: &str) -> CtlKind {
        CtlKind::ALL.into_iter().find(|x| x.key() == k).unwrap_or_default()
    }
}

/// What a button does (no code: a fixed list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    First,
    Previous,
    Next,
    Last,
    New,
    Delete,
    Save,
    Find,
    /// Opens the report named in the button's `target`.
    OpenReport,
    /// Opens the form named in the button's `target`.
    OpenForm,
    Close,
}

impl Action {
    pub const ALL: [Action; 11] = [Action::First, Action::Previous, Action::Next, Action::Last, Action::New, Action::Delete, Action::Save, Action::Find, Action::OpenReport, Action::OpenForm, Action::Close];

    pub fn key(self) -> &'static str {
        match self {
            Action::First => "first",
            Action::Previous => "previous",
            Action::Next => "next",
            Action::Last => "last",
            Action::New => "new",
            Action::Delete => "delete",
            Action::Save => "save",
            Action::Find => "find",
            Action::OpenReport => "open_report",
            Action::OpenForm => "open_form",
            Action::Close => "close",
        }
    }

    pub fn from_key(k: &str) -> Action {
        Action::ALL.into_iter().find(|x| x.key() == k).unwrap_or_default()
    }
}

/// A control of a form (places in points from the form's top left).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Control {
    pub kind: CtlKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// A label's text or a button's caption.
    pub text: String,
    /// The field a text box, dropdown or checkbox shows and edits.
    pub field: String,
    /// Dropdowns: the table the values come from, the column stored and
    /// the one shown.
    pub list_table: String,
    pub list_value: String,
    pub list_show: String,
    pub action: Action,
    /// The report or form a button opens.
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct FormDef {
    pub title: String,
    pub kind: FormKind,
    /// A table or a saved query.
    pub source: String,
    pub controls: Vec<Control>,
    /// The subform's table and the fields that link it: the subform shows
    /// rows whose `child_field` equals the record's `master_field`.
    pub sub_source: String,
    pub master_field: String,
    pub child_field: String,
    pub width: f32,
    pub height: f32,
}

impl FormDef {
    pub fn to_records(&self) -> Vec<Record> {
        let mut head = Record::default();
        head.set("title", &self.title).set("kind", self.kind.key()).set("source", &self.source).set("sub_source", &self.sub_source).set("master_field", &self.master_field).set("child_field", &self.child_field).set("width", self.width).set("height", self.height);
        let mut out = vec![head];
        for c in &self.controls {
            let mut r = Record::new("control");
            r.set("kind", c.kind.key()).set("x", c.x).set("y", c.y).set("w", c.w).set("h", c.h).set("text", &c.text).set("field", &c.field);
            if c.kind == CtlKind::ComboBox {
                r.set("list_table", &c.list_table).set("list_value", &c.list_value).set("list_show", &c.list_show);
            }
            if c.kind == CtlKind::Button {
                r.set("action", c.action.key()).set("target", &c.target);
            }
            out.push(r);
        }
        out
    }

    pub fn from_records(recs: &[Record]) -> FormDef {
        let head = recs.first().cloned().unwrap_or_default();
        let mut d = FormDef {
            title: head.get("title").to_string(),
            kind: FormKind::from_key(head.get("kind")),
            source: head.get("source").to_string(),
            sub_source: head.get("sub_source").to_string(),
            master_field: head.get("master_field").to_string(),
            child_field: head.get("child_field").to_string(),
            width: head.num("width", 480.0),
            height: head.num("height", 360.0),
            ..Default::default()
        };
        // A form's worth of controls (each is drawn every frame).
        for r in recs.iter().filter(|r| r.section == "control").take(1000) {
            d.controls.push(Control {
                kind: CtlKind::from_key(r.get("kind")),
                x: r.num("x", 0.0),
                y: r.num("y", 0.0),
                w: r.num("w", 100.0),
                h: r.num("h", 22.0),
                text: r.get("text").to_string(),
                field: r.get("field").to_string(),
                list_table: r.get("list_table").to_string(),
                list_value: r.get("list_value").to_string(),
                list_show: r.get("list_show").to_string(),
                action: Action::from_key(r.get("action")),
                target: r.get("target").to_string(),
            });
        }
        d
    }

    /// The form wizard's layout: a label and a box per field (checkboxes
    /// for yes/no fields, dropdowns for fields related to another table),
    /// then the navigation buttons. `fields`: name, is yes/no, and the
    /// related table and column when there is one.
    pub fn wizard(title: &str, kind: FormKind, source: &str, fields: &[(String, bool, Option<(String, String)>)]) -> FormDef {
        let mut controls = Vec::new();
        let label_w = 130.0;
        let box_w = 220.0;
        let row_h = 28.0;
        let mut y = 12.0;
        for (name, yes_no, rel) in fields {
            controls.push(Control { kind: CtlKind::Label, x: 12.0, y, w: label_w, h: 22.0, text: name.clone(), ..Default::default() });
            let mut c = Control { kind: CtlKind::TextBox, x: 12.0 + label_w + 8.0, y, w: box_w, h: 22.0, field: name.clone(), ..Default::default() };
            if *yes_no {
                c.kind = CtlKind::CheckBox;
                c.w = 22.0;
            } else if let Some((t, col)) = rel {
                c.kind = CtlKind::ComboBox;
                c.list_table = t.clone();
                c.list_value = col.clone();
                c.list_show = col.clone();
            }
            controls.push(c);
            y += row_h;
        }
        y += 8.0;
        let buttons = [(Action::First, "|<"), (Action::Previous, "<"), (Action::Next, ">"), (Action::Last, ">|"), (Action::New, "+"), (Action::Delete, "x"), (Action::Save, "OK")];
        for (i, (a, t)) in buttons.iter().enumerate() {
            controls.push(Control { kind: CtlKind::Button, x: 12.0 + i as f32 * 46.0, y, w: 40.0, h: 24.0, text: t.to_string(), action: *a, ..Default::default() });
        }
        FormDef { title: title.to_string(), kind, source: source.to_string(), controls, width: 12.0 + label_w + 8.0 + box_w + 20.0, height: y + 40.0, ..Default::default() }
    }

    /// The fields its controls show.
    pub fn bound_fields(&self) -> Vec<&str> {
        self.controls.iter().filter(|c| !c.field.is_empty() && c.kind != CtlKind::Label && c.kind != CtlKind::Button).map(|c| c.field.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_round_trip() {
        let f = FormDef::wizard("Orders", FormKind::Single, "Orders", &[("Id".into(), false, None), ("Paid".into(), true, None), ("Customer".into(), false, Some(("Customers".into(), "Id".into())))]);
        assert_eq!(f.controls[3].kind, CtlKind::CheckBox);
        assert_eq!(f.controls[5].kind, CtlKind::ComboBox);
        assert_eq!(f.bound_fields(), ["Id", "Paid", "Customer"]);
        let back = FormDef::from_records(&crate::objects::read(&crate::objects::write(&f.to_records())));
        assert_eq!(back, f);
    }
}
