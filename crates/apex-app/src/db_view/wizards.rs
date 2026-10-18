//! The form and report wizards, in three steps: the source and its
//! fields; the layout (form kind and subform, or grouping, sorting and
//! totals); the name. The result is saved and opened, to be refined in the
//! designer.

use apex_db::forms::{FormDef, FormKind};
use apex_db::objects::Kind;
use apex_db::report::{Agg, RGroup, ReportDef};
use apex_db::sqlite::{Db, FieldType, RowSource};
use eframe::egui::{self, RichText};

use super::DbView;
use crate::i18n::{tr, trf};
use crate::widgets::FitWindow as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizKind {
    Form,
    Report,
}

pub struct Wizard {
    pub kind: WizKind,
    pub step: usize,
    pub source: String,
    /// The source's fields and whether each is chosen.
    pub fields: Vec<(String, bool)>,
    pub form_kind: FormKind,
    pub sub_source: String,
    pub master_field: String,
    pub child_field: String,
    pub group: String,
    pub group_break: bool,
    pub sort: String,
    /// Fields summed in the totals.
    pub sums: Vec<String>,
    pub landscape: bool,
    pub name: String,
}

impl Wizard {
    pub fn new(kind: WizKind, db: &Db) -> Wizard {
        let source = db.table_names().into_iter().next().unwrap_or_default();
        let mut w = Wizard {
            kind,
            step: 0,
            source: String::new(),
            fields: Vec::new(),
            form_kind: FormKind::Single,
            sub_source: String::new(),
            master_field: String::new(),
            child_field: String::new(),
            group: String::new(),
            group_break: false,
            sort: String::new(),
            sums: Vec::new(),
            landscape: false,
            name: String::new(),
        };
        w.set_source(db, &source);
        w
    }

    pub fn set_source(&mut self, db: &Db, source: &str) {
        self.source = source.to_string();
        self.fields = db.columns(&RowSource::Table(source.to_string())).map(|c| c.0).unwrap_or_default().into_iter().map(|c| (c, true)).collect();
        let n = db.objects(if self.kind == WizKind::Form { Kind::Form } else { Kind::Report }).len();
        self.name = if n == 0 { source.to_string() } else { format!("{source} {}", n + 1) };
        // The first table pointing to this one makes the subform.
        self.sub_source.clear();
        if let Some(r) = db.relations().into_iter().find(|r| r.ref_table.eq_ignore_ascii_case(source)) {
            self.sub_source = r.table.clone();
            self.child_field = r.columns.first().cloned().unwrap_or_default();
            self.master_field = r.ref_columns.first().cloned().unwrap_or_default();
        }
        self.group.clear();
        self.sort.clear();
        self.sums.clear();
    }

    pub fn chosen(&self) -> Vec<String> {
        self.fields.iter().filter(|f| f.1).map(|f| f.0.clone()).collect()
    }

    pub fn form(&self, db: &Db) -> FormDef {
        let design = db.design(&self.source).ok();
        let rels = db.relations();
        let fields: Vec<(String, bool, Option<(String, String)>)> = self
            .chosen()
            .into_iter()
            .map(|f| {
                let is_bool = design.as_ref().and_then(|d| d.field(&f)).is_some_and(|x| x.ty == FieldType::Bool);
                let rel = rels.iter().find(|r| r.table.eq_ignore_ascii_case(&self.source) && r.columns.first().is_some_and(|c| c.eq_ignore_ascii_case(&f))).map(|r| (r.ref_table.clone(), r.ref_columns.first().cloned().unwrap_or_default()));
                (f, is_bool, rel)
            })
            .collect();
        let mut d = FormDef::wizard(&self.name, self.form_kind, &self.source, &fields);
        if self.form_kind == FormKind::WithSubform {
            d.sub_source = self.sub_source.clone();
            d.master_field = self.master_field.clone();
            d.child_field = self.child_field.clone();
        }
        d
    }

    pub fn report(&self) -> ReportDef {
        let mut d = ReportDef::plain(&self.name, &self.source, &self.chosen());
        // The grouping field leads its group, not the rows.
        if !self.group.is_empty() {
            d.fields.retain(|f| f.name != self.group);
            d.groups.push(RGroup { field: self.group.clone(), page_break: self.group_break, ..Default::default() });
        }
        if !self.sort.is_empty() {
            d.sort.push((self.sort.clone(), true));
        }
        for f in &mut d.fields {
            if self.sums.contains(&f.name) {
                f.total = Agg::Sum;
            }
        }
        d.landscape = self.landscape;
        d
    }

    /// Saves the result and opens it.
    pub fn finish(&self, v: &mut DbView) {
        let name = self.name.trim().to_string();
        match self.kind {
            WizKind::Form => {
                let d = self.form(&v.db);
                if v.save_object(Kind::Form, &name, &d.to_records()) {
                    v.open_form(&name);
                }
            }
            WizKind::Report => {
                let d = self.report();
                if v.save_object(Kind::Report, &name, &d.to_records()) {
                    v.open_report(&name);
                }
            }
        }
    }
}

pub fn wizard_ui(v: &mut DbView, ctx: &egui::Context) {
    let Some(mut w) = v.wizard.take() else { return };
    let mut open = true;
    let mut done = false;
    let title = if w.kind == WizKind::Form { tr("db.form_wizard") } else { tr("db.report_wizard") };
    egui::Window::new(title).collapsible(false).resizable(false).open(&mut open).default_width(420.0).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show_fit(ctx, |ui| {
        ui.label(RichText::new(trf("db.wizard_step", &[("n", &(w.step + 1)), ("of", &3)])).weak());
        ui.separator();
        match w.step {
            0 => {
                ui.horizontal(|ui| {
                    ui.label(tr("db.source"));
                    let mut s = w.source.clone();
                    super::report_ui::source_combo(ui, "db_wiz_source", &v.db, &mut s);
                    if s != w.source {
                        w.set_source(&v.db, &s);
                    }
                });
                ui.label(tr("db.wizard_fields"));
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (f, on) in &mut w.fields {
                        ui.checkbox(on, f.as_str());
                    }
                });
                ui.horizontal(|ui| {
                    if ui.small_button(tr("db.select_all")).clicked() {
                        w.fields.iter_mut().for_each(|f| f.1 = true);
                    }
                    if ui.small_button(tr("db.select_none")).clicked() {
                        w.fields.iter_mut().for_each(|f| f.1 = false);
                    }
                });
            }
            1 if w.kind == WizKind::Form => {
                for k in FormKind::ALL {
                    ui.radio_value(&mut w.form_kind, k, super::trk("db.fkind", k.key()));
                }
                if w.form_kind == FormKind::WithSubform {
                    if w.sub_source.is_empty() {
                        ui.colored_label(ui.visuals().warn_fg_color, tr("db.no_related"));
                    } else {
                        ui.label(format!("{} ({}.{} = {}.{})", w.sub_source, w.sub_source, w.child_field, w.source, w.master_field));
                    }
                }
            }
            1 => {
                let chosen = w.chosen();
                ui.horizontal(|ui| {
                    ui.label(tr("db.group_by"));
                    egui::ComboBox::from_id_salt("db_wiz_group").selected_text(w.group.as_str()).show_ui(ui, |ui| {
                        ui.selectable_value(&mut w.group, String::new(), "—");
                        for c in &chosen {
                            ui.selectable_value(&mut w.group, c.clone(), c);
                        }
                    });
                });
                ui.add_enabled(!w.group.is_empty(), egui::Checkbox::new(&mut w.group_break, tr("db.group_page_break")));
                ui.horizontal(|ui| {
                    ui.label(tr("db.sort_by"));
                    egui::ComboBox::from_id_salt("db_wiz_sort").selected_text(w.sort.as_str()).show_ui(ui, |ui| {
                        ui.selectable_value(&mut w.sort, String::new(), "—");
                        for c in &chosen {
                            ui.selectable_value(&mut w.sort, c.clone(), c);
                        }
                    });
                });
                ui.label(tr("db.sum_fields"));
                let numeric: Vec<String> = match v.db.design(&w.source) {
                    Ok(d) => chosen.iter().filter(|c| d.field(c).is_some_and(|f| matches!(f.ty, FieldType::Integer | FieldType::Number | FieldType::Currency))).cloned().collect(),
                    Err(_) => chosen.clone(),
                };
                for c in numeric {
                    let mut on = w.sums.contains(&c);
                    if ui.checkbox(&mut on, c.as_str()).changed() {
                        if on {
                            w.sums.push(c);
                        } else {
                            w.sums.retain(|x| *x != c);
                        }
                    }
                }
                ui.checkbox(&mut w.landscape, tr("db.landscape"));
            }
            _ => {
                ui.label(tr("db.wizard_name"));
                ui.text_edit_singleline(&mut w.name);
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add_enabled(w.step > 0, egui::Button::new(tr("db.back"))).clicked() {
                w.step -= 1;
            }
            let can = !w.chosen().is_empty();
            if w.step < 2 {
                if ui.add_enabled(can, egui::Button::new(tr("db.next"))).clicked() {
                    w.step += 1;
                }
            } else if ui.add_enabled(can && !w.name.trim().is_empty(), egui::Button::new(tr("db.finish"))).clicked() {
                done = true;
            }
        });
    });
    if done {
        w.finish(v);
        return;
    }
    if open {
        v.wizard = Some(w);
    }
}
