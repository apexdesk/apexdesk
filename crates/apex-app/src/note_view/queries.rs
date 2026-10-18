//! Saved queries: the table's filters (tag, dates, conditions on
//! fields) kept under a name, and opened again from the table or the
//! sections panel.

use apex_doc::notebook::FieldKind;
use apex_doc::notebook::query::{Cond, Op};

use super::table::TablePanel;
use super::types::line_edit;
use super::*;

fn op_name(o: Op) -> String {
    tr(match o {
        Op::Is => "nb.op_is",
        Op::IsNot => "nb.op_not",
        Op::Contains => "nb.op_has",
        Op::Less => "nb.op_lt",
        Op::More => "nb.op_gt",
        Op::Empty => "nb.op_empty",
        Op::NotEmpty => "nb.op_set",
    })
    .to_string()
}

impl NoteView {
    /// Shows saved query `id` in the table.
    pub fn open_query(&mut self, id: i64) {
        let Some(q) = self.nb.query(id).cloned() else { return };
        self.open_table(q.ptype);
        if let Some(t) = &mut self.table {
            t.filters = q.active_filters() > 0;
            t.query = q;
        }
    }

    /// The fields a condition can be on: the type's, or every type's.
    fn query_fields(&self, ptype: Option<i64>) -> Vec<apex_doc::notebook::Field> {
        let mut v: Vec<apex_doc::notebook::Field> = Vec::new();
        for t in self.nb.types.iter().filter(|t| ptype.is_none_or(|p| p == t.id)) {
            for f in &t.fields {
                if !v.iter().any(|g| g.name == f.name) {
                    v.push(f.clone());
                }
            }
        }
        v
    }

    /// Tag, dates and field conditions.
    pub(super) fn filters_ui(&mut self, ui: &mut egui::Ui, panel: &mut TablePanel) {
        let q = &mut panel.query;
        let uid = self.uid;
        ui.add_space(6.0);
        egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(4.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("nb.filter_tag"));
                let tags: Vec<String> = self.nb.tags().into_keys().collect();
                let shown = if q.tag.is_empty() { tr("nb.any").to_string() } else { format!("#{}", q.tag) };
                egui::ComboBox::from_id_salt(("note_query_tag", uid)).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut q.tag, String::new(), tr("nb.any"));
                    for t in tags {
                        let label = format!("#{t}");
                        ui.selectable_value(&mut q.tag, t, label);
                    }
                });
                ui.add_space(12.0);
                egui::ComboBox::from_id_salt(("note_query_dates", uid)).selected_text(tr(if q.by_created { "nb.filter_created" } else { "nb.filter_modified" })).show_ui(ui, |ui| {
                    ui.selectable_value(&mut q.by_created, false, tr("nb.filter_modified"));
                    ui.selectable_value(&mut q.by_created, true, tr("nb.filter_created"));
                });
                for (key, date, salt) in [("nb.filter_since", &mut q.since, "since"), ("nb.filter_until", &mut q.until, "until")] {
                    ui.label(tr(key));
                    if let Some(d) = line_edit(ui, Id::new(("note_query_date", uid, salt)), date, tr("nb.date_hint"), 90.0) {
                        let d = d.trim().to_string();
                        if d.is_empty() || apex_doc::notebook::tasks::is_date(&d) {
                            *date = d;
                        }
                    }
                }
            });
            let fields = self.query_fields(q.ptype);
            let mut remove = None;
            for (i, c) in q.conds.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("note_query_field", uid, i)).width(130.0).selected_text(c.field.clone()).show_ui(ui, |ui| {
                        for f in &fields {
                            ui.selectable_value(&mut c.field, f.name.clone(), &f.name);
                        }
                    });
                    egui::ComboBox::from_id_salt(("note_query_op", uid, i)).width(110.0).selected_text(op_name(c.op)).show_ui(ui, |ui| {
                        for o in Op::ALL {
                            ui.selectable_value(&mut c.op, o, op_name(o));
                        }
                    });
                    if c.op.takes_value() {
                        match fields.iter().find(|f| f.name == c.field) {
                            Some(f) if f.kind == FieldKind::Choice && matches!(c.op, Op::Is | Op::IsNot) => {
                                egui::ComboBox::from_id_salt(("note_query_choice", uid, i)).width(150.0).selected_text(c.value.clone()).show_ui(ui, |ui| {
                                    for o in &f.options {
                                        ui.selectable_value(&mut c.value, o.clone(), o);
                                    }
                                });
                            }
                            Some(f) if f.kind == FieldKind::Check => {
                                let mut on = !c.value.is_empty();
                                if ui.checkbox(&mut on, "").changed() {
                                    c.value = if on { "1".into() } else { String::new() };
                                }
                            }
                            f => {
                                let hint = if f.is_some_and(|f| f.kind == FieldKind::Date) { tr("nb.date_hint") } else { "" };
                                if let Some(v) = line_edit(ui, Id::new(("note_query_value", uid, i)), &c.value, hint, 150.0) {
                                    c.value = v.trim().to_string();
                                }
                            }
                        }
                    }
                    if ui.small_button("×").on_hover_text(tr("nb.filter_remove")).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                q.conds.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!fields.is_empty(), egui::Button::new(format!("+ {}", tr("nb.filter_add")))).on_disabled_hover_text(tr("nb.filter_no_fields")).clicked() {
                    q.conds.push(Cond { field: fields[0].name.clone(), op: Op::Is, value: String::new() });
                }
                if q.active_filters() > 0 && ui.button(tr("nb.filter_clear")).clicked() {
                    q.conds.clear();
                    q.tag.clear();
                    q.since.clear();
                    q.until.clear();
                }
            });
        });
    }

    /// Saved queries: pick one, save the one shown, delete it.
    pub(super) fn saved_queries_ui(&mut self, ui: &mut egui::Ui, panel: &mut TablePanel) {
        let uid = self.uid;
        let saved = self.nb.query(panel.query.id).cloned();
        if let Some(s) = &saved {
            if ui.button(tr("nb.query_delete")).clicked() {
                let id = s.id;
                self.change(|nb| nb.queries.retain(|q| q.id != id));
                panel.query.id = 0;
                panel.query.name.clear();
            }
            if panel.query != *s && ui.button(tr("nb.query_update")).on_hover_text(trf("nb.query_update_tip", &[("name", &s.name)])).clicked() {
                let q = panel.query.clone();
                self.change(|nb| {
                    if let Some(old) = nb.queries.iter_mut().find(|x| x.id == q.id) {
                        *old = q;
                    }
                });
            }
        }
        match &mut panel.naming {
            Some(name) => {
                let mut done = None;
                if ui.button(tr("dialog.cancel")).clicked() {
                    done = Some(false);
                }
                if ui.add_enabled(!name.trim().is_empty(), egui::Button::new(tr("dialog.ok"))).clicked() {
                    done = Some(true);
                }
                let r = ui.add(egui::TextEdit::singleline(name).id(Id::new(("note_query_name", uid))).hint_text(tr("nb.query_name")).desired_width(160.0));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !name.trim().is_empty() {
                    done = Some(true);
                }
                match done {
                    Some(true) => {
                        let (q, name) = (panel.query.clone(), name.clone());
                        let mut id = 0;
                        self.change(|nb| id = nb.save_query(&q, &name));
                        if let Some(s) = self.nb.query(id) {
                            panel.query = s.clone();
                        }
                        panel.naming = None;
                    }
                    Some(false) => panel.naming = None,
                    None => {}
                }
            }
            None => {
                if ui.button(format!("{}…", tr("nb.query_save"))).clicked() {
                    panel.naming = Some(String::new());
                    ui.memory_mut(|m| m.request_focus(Id::new(("note_query_name", uid))));
                }
            }
        }
        let shown = saved.as_ref().map_or_else(|| tr("nb.queries").to_string(), |s| s.name.clone());
        let mut pick = None;
        egui::ComboBox::from_id_salt(("note_queries", uid)).selected_text(shown).show_ui(ui, |ui| {
            if self.nb.queries.is_empty() {
                ui.label(RichText::new(tr("nb.queries_none")).weak());
            }
            for q in &self.nb.queries {
                if ui.selectable_label(q.id == panel.query.id, &q.name).clicked() {
                    pick = Some(q.clone());
                }
            }
        });
        if let Some(q) = pick {
            panel.filters = q.active_filters() > 0;
            panel.query = q;
        }
    }

    /// The saved queries in the sections panel.
    pub(super) fn queries_list(&mut self, ui: &mut egui::Ui) {
        let list: Vec<(i64, String)> = self.nb.queries.iter().map(|q| (q.id, q.name.clone())).collect();
        for (id, name) in list.into_iter().rev() {
            let on = self.table.as_ref().is_some_and(|t| t.query.id == id);
            if ui.add(egui::Button::new(RichText::new(name).small()).frame(false).selected(on)).clicked() {
                self.open_query(id);
            }
        }
    }
}
