//! File > Properties: title, subject, keywords… and custom properties of
//! the document or workbook.

use apex_doc::kept::is_label_prop;
use apex_doc::{CustomProp, Meta, PropValue};
use eframe::egui::{self, RichText};

use super::{App, View};
use crate::i18n::tr;
use crate::widgets::{self, DialogResult};

/// Kinds of custom properties, as offered.
const KINDS: [&str; 4] = ["props.k_text", "props.k_number", "props.k_date", "props.k_bool"];

/// A custom property being edited: kind (index in [`KINDS`]) and value
/// as typed.
struct Row {
    name: String,
    kind: usize,
    text: String,
    flag: bool,
}

pub(super) struct PropsDlg {
    tab: u64,
    meta: Meta,
    rows: Vec<Row>,
    error: bool,
    /// The sensitivity labels, shown but not editable.
    label: Option<String>,
}

fn row(p: &CustomProp) -> Row {
    let (kind, flag) = match p.value {
        PropValue::Text(_) => (0, false),
        PropValue::Number(_) => (1, false),
        PropValue::Date(_) => (2, false),
        PropValue::Bool(b) => (3, b),
    };
    let text = match &p.value {
        PropValue::Date(d) => d.get(..10).unwrap_or(d).to_string(),
        PropValue::Number(_) => p.value.text().replace('.', &crate::common::decimal().to_string()),
        v => v.text(),
    };
    Row { name: p.name.clone(), kind, text, flag }
}

/// The property a row stands for (`None`: its value doesn't fit its kind).
fn prop(r: &Row) -> Option<CustomProp> {
    let t = r.text.trim();
    let value = match r.kind {
        0 => PropValue::Text(r.text.clone()),
        1 => PropValue::Number(t.replace(crate::common::decimal(), ".").parse().ok()?),
        2 => {
            let ok = t.len() == 10 && t.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
            if !ok {
                return None;
            }
            PropValue::Date(format!("{t}T00:00:00Z"))
        }
        _ => PropValue::Bool(r.flag),
    };
    Some(CustomProp { name: r.name.trim().to_string(), value })
}

impl App {
    pub(super) fn open_properties(&mut self) {
        if let Some(View::Pdf(v)) = self.tabs.get_mut(self.active).map(|t| &mut t.view) {
            v.open_props();
            return;
        }
        let Some(t) = self.tabs.get(self.active) else { return };
        let (meta, kept) = match &t.view {
            View::Sheet(v) => (v.wb.meta.clone(), &v.wb.kept),
            View::Doc(v) => (v.doc.meta.clone(), &v.doc.kept),
            View::Slides(v) => (v.pres.meta.clone(), crate::slides_view::no_kept()),
            View::Pdf(_) | View::Db(_) | View::Note(_) => return,
        };
        let label = crate::labels_ui::label_names(&meta, kept);
        // Label properties belong to the organization: not listed.
        let rows = meta.custom.iter().filter(|p| !is_label_prop(&p.name)).map(row).collect();
        self.props_dlg = Some(PropsDlg { tab: t.id, meta, rows, error: false, label });
    }

    pub(super) fn properties_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.props_dlg.take() else { return };
        let res = widgets::dialog(ctx, "doc_properties", tr("props.title"), 520.0, |ui| {
            let m = &mut d.meta;
            widgets::form(ui, "props_general", |ui| {
                for (key, v) in [("props.f_title", &mut m.title), ("props.f_subject", &mut m.subject), ("props.f_author", &mut m.author), ("props.f_manager", &mut m.manager), ("props.f_company", &mut m.company), ("props.f_category", &mut m.category), ("props.f_keywords", &mut m.keywords)] {
                    widgets::form_row(ui, tr(key), |ui| ui.add(egui::TextEdit::singleline(v).desired_width(320.0)));
                }
                widgets::form_row(ui, tr("props.f_comments"), |ui| {
                    let id = egui::Id::new("props_comments");
                    ui.data_mut(|x| x.insert_temp(id.with("multiline"), true));
                    ui.add(egui::TextEdit::multiline(&mut m.description).id(id).desired_width(320.0).desired_rows(3))
                });
                if !m.created.is_empty() {
                    let when = m.created.replacen('T', " ", 1);
                    widgets::form_row(ui, tr("props.f_created"), |ui| ui.label(when.trim_end_matches('Z')));
                }
                if let Some(l) = &d.label {
                    widgets::form_row(ui, tr("props.label"), |ui| ui.label(l).on_hover_text(tr("label.tip")));
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new(tr("props.custom")).strong());
            let mut remove = None;
            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                egui::Grid::new("props_custom").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                    if !d.rows.is_empty() {
                        ui.label(RichText::new(tr("props.name")).weak());
                        ui.label(RichText::new(tr("props.kind")).weak());
                        ui.label(RichText::new(tr("props.value")).weak());
                        ui.end_row();
                    }
                    for (i, r) in d.rows.iter_mut().enumerate() {
                        ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut r.name));
                        egui::ComboBox::from_id_salt(("props_kind", i)).width(90.0).selected_text(tr(KINDS[r.kind])).show_ui(ui, |ui| {
                            for (k, key) in KINDS.iter().enumerate() {
                                ui.selectable_value(&mut r.kind, k, tr(key));
                            }
                        });
                        if r.kind == 3 {
                            ui.checkbox(&mut r.flag, tr("props.yes"));
                        } else {
                            let bad = !r.text.trim().is_empty() && prop(r).is_none();
                            let mut e = egui::TextEdit::singleline(&mut r.text);
                            if r.kind == 2 {
                                e = e.hint_text("AAAA-MM-DD");
                            }
                            if bad {
                                e = e.text_color(ui.visuals().error_fg_color);
                            }
                            ui.add_sized([170.0, 20.0], e);
                        }
                        if ui.small_button("×").on_hover_text(tr("props.remove")).clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            });
            if let Some(i) = remove {
                d.rows.remove(i);
            }
            if ui.button(format!("+ {}", tr("props.add"))).clicked() {
                d.rows.push(Row { name: String::new(), kind: 0, text: String::new(), flag: false });
            }
            if d.error {
                ui.label(RichText::new(tr("props.invalid")).color(ui.visuals().error_fg_color));
            }
        });
        match res {
            DialogResult::None => self.props_dlg = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let rows: Vec<_> = d.rows.iter().filter(|r| !r.name.trim().is_empty()).map(prop).collect();
                let Some(custom) = rows.into_iter().collect::<Option<Vec<_>>>() else {
                    d.error = true;
                    self.props_dlg = Some(d);
                    return;
                };
                let labels: Vec<CustomProp> = d.meta.custom.iter().filter(|p| is_label_prop(&p.name)).cloned().collect();
                d.meta.custom = custom.into_iter().filter(|p| !is_label_prop(&p.name)).chain(labels).collect();
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == d.tab) {
                    let (meta, modified) = match &mut t.view {
                        View::Sheet(v) => (&mut v.wb.meta, &mut v.wb.modified),
                        View::Doc(v) => (&mut v.doc.meta, &mut v.doc.modified),
                        View::Slides(v) => (&mut v.pres.deck.meta, &mut v.pres.modified),
                        View::Pdf(_) | View::Db(_) | View::Note(_) => return,
                    };
                    if *meta != d.meta {
                        *meta = d.meta;
                        *modified = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_rows() {
        for v in [PropValue::Text("a".into()), PropValue::Number(2.5), PropValue::Bool(true), PropValue::Date("2026-10-01T00:00:00Z".into())] {
            let p = CustomProp { name: "x".into(), value: v };
            assert_eq!(prop(&row(&p)), Some(p));
        }
        let bad = Row { name: "x".into(), kind: 2, text: "1 oct".into(), flag: false };
        assert_eq!(prop(&bad), None);
    }
}
