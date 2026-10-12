//! The table designer: the fields with their type, size, default value,
//! required, unique, validation rule and message, description and primary
//! key; and the table's indexes. Saving rebuilds the table keeping its
//! rows (see [`apex_db::sqlite::Db::apply_design`]).

use apex_db::sqlite::{Db, FieldDef, FieldType, IndexDef, TableDesign};
use eframe::egui::{self, RichText};

use super::DbView;
use crate::i18n::{tr, trf};

pub struct Designer {
    /// The table's name when it exists.
    pub original: Option<String>,
    pub design: TableDesign,
    pub sel: usize,
    pub error: Option<String>,
    /// Indexes' columns as typed ("a, b").
    index_text: Vec<String>,
}

impl Designer {
    pub fn new(original: Option<&str>, design: TableDesign) -> Designer {
        let index_text = design.indexes.iter().map(|i| i.columns.join(", ")).collect();
        Designer { original: original.map(str::to_string), design, sel: 0, error: None, index_text }
    }

    /// A new table: "Table1" (or the next free) with a numbered key.
    pub fn blank(db: &Db) -> TableDesign {
        let names = db.table_names();
        let name = (1..).map(|i| format!("{}{i}", tr("db.table_prefix"))).find(|n| !names.iter().any(|x| x.eq_ignore_ascii_case(n))).unwrap();
        let mut id = FieldDef::new("Id", FieldType::AutoNumber);
        id.pk = true;
        TableDesign { name, fields: vec![id], ..Default::default() }
    }

    pub fn title(&self) -> String {
        self.original.clone().unwrap_or_else(|| self.design.name.clone())
    }

    /// Reads the design again (after undo).
    pub fn reload(&mut self, db: &Db) {
        if let Some(n) = &self.original {
            if let Ok(d) = db.design(n) {
                *self = Designer::new(Some(n), d);
            }
        }
    }

    /// Checks the design before saving; gives the problem.
    pub fn problem(&self) -> Option<String> {
        if self.design.name.trim().is_empty() {
            return Some(tr("db.need_name").to_string());
        }
        if self.design.fields.is_empty() {
            return Some(tr("db.need_field").to_string());
        }
        for (i, f) in self.design.fields.iter().enumerate() {
            if f.name.trim().is_empty() {
                return Some(tr("db.need_field_name").to_string());
            }
            if self.design.fields[..i].iter().any(|g| g.name.eq_ignore_ascii_case(&f.name)) {
                return Some(trf("db.field_twice", &[("field", &f.name)]));
            }
            if f.ty == FieldType::AutoNumber && !(f.pk && self.design.fields.iter().filter(|g| g.pk).count() == 1) {
                return Some(tr("db.autonumber_key").to_string());
            }
        }
        for (i, ix) in self.design.indexes.iter().enumerate() {
            if self.design.indexes[..i].iter().any(|j| j.name.trim().eq_ignore_ascii_case(ix.name.trim())) {
                return Some(trf("db.index_twice", &[("index", &ix.name)]));
            }
        }
        None
    }

    /// Saves the design; gives whether it went well.
    pub fn save(&mut self, v: &mut DbView) -> bool {
        if let Some(p) = self.problem() {
            self.error = Some(p);
            return false;
        }
        self.design.indexes = self
            .index_text
            .iter()
            .zip(&self.design.indexes)
            .map(|(t, i)| IndexDef { columns: t.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect(), ..i.clone() })
            .filter(|i| !i.columns.is_empty())
            .collect();
        let d = self.design.clone();
        let old = self.original.clone();
        if v.with_snapshot(tr("db.undo.design"), |db| db.apply_design(old.as_deref(), &d)) {
            self.error = None;
            let name = d.name.clone();
            if let Ok(nd) = v.db.design(&name) {
                *self = Designer::new(Some(&name), nd);
            }
            v.notices.push(crate::app::Notice::info(trf("db.table_saved", &[("name", &name)])));
            true
        } else {
            false
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        ui.horizontal(|ui| {
            ui.label(tr("db.table_name"));
            ui.text_edit_singleline(&mut self.design.name);
            if ui.button(tr("db.save_design")).clicked() {
                self.save(v);
            }
            if let Some(n) = self.original.clone() {
                if ui.button(tr("db.open")).clicked() {
                    v.open_table(&n);
                }
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.separator();
        let mut action = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("db_design").striped(true).spacing([6.0, 4.0]).show(ui, |ui| {
                for k in ["db.f_key", "db.f_name", "db.f_type", "db.f_size", "db.f_required", "db.f_unique", "db.f_default", "db.f_rule", "db.f_rule_text", "db.f_description", ""] {
                    ui.label(RichText::new(if k.is_empty() { "" } else { tr(k) }).strong());
                }
                ui.end_row();
                let n = self.design.fields.len();
                for (i, f) in self.design.fields.iter_mut().enumerate() {
                    ui.checkbox(&mut f.pk, "").on_hover_text(tr("db.f_key_tip"));
                    let r = ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(120.0));
                    if r.gained_focus() {
                        self.sel = i;
                    }
                    egui::ComboBox::from_id_salt(("db_ftype", i)).selected_text(super::trk("db.type", f.ty.key())).show_ui(ui, |ui| {
                        for t in FieldType::ALL {
                            ui.selectable_value(&mut f.ty, t, super::trk("db.type", t.key()));
                        }
                    });
                    if f.ty == FieldType::Text {
                        ui.add(egui::DragValue::new(&mut f.size).range(0..=4000));
                    } else {
                        ui.label("");
                    }
                    ui.checkbox(&mut f.required, "");
                    ui.checkbox(&mut f.unique, "");
                    ui.add(egui::TextEdit::singleline(&mut f.default).desired_width(80.0).hint_text("0"));
                    ui.add(egui::TextEdit::singleline(&mut f.check).desired_width(100.0).hint_text("> 0"));
                    ui.add(egui::TextEdit::singleline(&mut f.validation_text).desired_width(120.0));
                    ui.add(egui::TextEdit::singleline(&mut f.description).desired_width(140.0));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text(tr("db.move_up")).clicked() {
                            action = Some((i, 0));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).on_hover_text(tr("db.move_down")).clicked() {
                            action = Some((i, 1));
                        }
                        if ui.small_button("×").on_hover_text(tr("db.remove_field")).clicked() {
                            action = Some((i, 2));
                        }
                    });
                    ui.end_row();
                }
            });
            if ui.button(tr("db.add_field")).clicked() {
                let n = self.design.fields.len() + 1;
                self.design.fields.push(FieldDef::new(&format!("{}{n}", tr("db.field_prefix")), FieldType::Text));
            }
            ui.add_space(10.0);
            ui.label(RichText::new(tr("db.indexes")).strong());
            let mut remove = None;
            egui::Grid::new("db_indexes").spacing([6.0, 4.0]).show(ui, |ui| {
                for (i, ix) in self.design.indexes.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut ix.name).desired_width(140.0).hint_text(tr("db.index_name")));
                    ui.add(egui::TextEdit::singleline(&mut self.index_text[i]).desired_width(200.0).hint_text(tr("db.index_columns")));
                    ui.checkbox(&mut ix.unique, tr("db.f_unique"));
                    if ui.small_button("×").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = remove {
                self.design.indexes.remove(i);
                self.index_text.remove(i);
            }
            if ui.button(tr("db.add_index")).clicked() {
                let col = self.design.fields.get(self.sel).map(|f| f.name.clone()).unwrap_or_default();
                let base = format!("{}_{col}", self.design.name);
                let taken = |n: &str| self.design.indexes.iter().any(|i| i.name.eq_ignore_ascii_case(n));
                let name = std::iter::once(base.clone()).chain((2..).map(|k| format!("{base}{k}"))).find(|n| !taken(n)).unwrap();
                self.design.indexes.push(IndexDef { name, columns: vec![col.clone()], unique: false });
                self.index_text.push(col);
            }
            if !self.design.foreign_keys.is_empty() {
                ui.add_space(10.0);
                ui.label(RichText::new(tr("db.relations")).strong());
                for fk in &self.design.foreign_keys {
                    ui.label(format!("{} → {}.{}", fk.columns.join(", "), fk.ref_table, fk.ref_columns.join(", ")));
                }
            }
        });
        match action {
            Some((i, 0)) => self.design.fields.swap(i, i - 1),
            Some((i, 1)) => self.design.fields.swap(i, i + 1),
            Some((i, _)) => {
                self.design.fields.remove(i);
            }
            None => {}
        }
    }
}
