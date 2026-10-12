//! The query designer: the tables at the top (a click on a field adds it
//! to the grid; tables related in the database are joined when added), the
//! grid below (field, alias, sort, shown, total, criteria and "or" rows,
//! the new value of update queries), or the SQL itself. Select queries
//! show their result as a datasheet; action queries ask first, saying how
//! many rows they change.

use apex_db::objects::{Kind, Record};
use apex_db::query::{Join, JoinKind, QField, QTable, QueryDesign, QueryType, Sort, Total};
use apex_db::sqlite::{Db, RowSource};
use eframe::egui::{self, RichText};

use super::{sheet, DbView, Pane};
use crate::i18n::{tr, trf};

pub struct QueryEditor {
    pub name: String,
    pub design: QueryDesign,
    /// Written in SQL (the designer can't show it).
    pub sql_mode: bool,
    pub sql: String,
    pub error: Option<String>,
    /// How many "or" rows of criteria the grid shows.
    or_rows: usize,
}

impl QueryEditor {
    pub fn new(name: &str) -> QueryEditor {
        QueryEditor { name: name.to_string(), design: QueryDesign::default(), sql_mode: false, sql: String::new(), error: None, or_rows: 2 }
    }

    pub fn load(db: &Db, name: &str) -> QueryEditor {
        let mut q = QueryEditor::new(name);
        let recs = db.load_object(Kind::Query, name).map(|t| apex_db::objects::read(&t)).unwrap_or_default();
        match QueryDesign::from_records(&recs) {
            Some(d) => {
                q.or_rows = d.fields.iter().map(|f| f.criteria.len()).max().unwrap_or(0).max(2);
                q.sql = d.to_sql();
                q.design = d;
            }
            None => {
                q.sql_mode = true;
                let head = recs.first().cloned().unwrap_or_default();
                q.sql = head.get("sql").to_string();
                if q.sql.is_empty() || head.flag("untranslated") {
                    // Access SQL that couldn't be translated: shown to be rewritten.
                    q.sql = head.get("access_sql").to_string();
                    q.error = Some(tr("db.untranslated").to_string());
                }
                q.design.kind = sql_kind(&q.sql);
            }
        }
        q
    }

    pub fn title(&self) -> String {
        self.name.clone()
    }

    pub fn current_sql(&self) -> String {
        if self.sql_mode { self.sql.clone() } else { self.design.to_sql() }
    }

    pub fn records(&self) -> Vec<Record> {
        if self.sql_mode {
            let mut r = Record::default();
            r.set("sql", &self.sql).set("type", sql_kind(&self.sql).key());
            vec![r]
        } else {
            self.design.to_records()
        }
    }

    /// Adds a table (joined to those already there by the database's relations).
    pub fn add_table(&mut self, db: &Db, name: &str) {
        let n = self.design.tables.len();
        let placed: Vec<String> = self.design.tables.iter().map(|t| t.name.clone()).collect();
        self.design.tables.push(QTable { name: name.to_string(), x: n as f32 * 180.0, y: 0.0 });
        for r in db.relations() {
            let (Some(c), Some(rc)) = (r.columns.first(), r.ref_columns.first()) else { continue };
            if r.table.eq_ignore_ascii_case(name) && placed.iter().any(|p| p.eq_ignore_ascii_case(&r.ref_table)) {
                self.design.joins.push(Join { left: r.ref_table.clone(), left_col: rc.clone(), right: name.to_string(), right_col: c.clone(), kind: JoinKind::Inner });
            } else if r.ref_table.eq_ignore_ascii_case(name) && placed.iter().any(|p| p.eq_ignore_ascii_case(&r.table)) {
                self.design.joins.push(Join { left: name.to_string(), left_col: rc.clone(), right: r.table.clone(), right_col: c.clone(), kind: JoinKind::Inner });
            }
        }
    }

    pub fn remove_table(&mut self, i: usize) {
        let t = self.design.tables.remove(i).name;
        self.design.joins.retain(|j| j.left != t && j.right != t);
        self.design.fields.retain(|f| f.table != t);
    }

    /// Runs it: a select opens its result, an action asks first.
    pub fn run(&mut self, v: &mut DbView) {
        let sql = self.current_sql();
        if sql.trim().is_empty() {
            return;
        }
        let kind = if self.sql_mode { sql_kind(&sql) } else { self.design.kind };
        if kind.is_action() {
            v.ask_run_action(&sql);
            return;
        }
        match v.db.select(&format!("SELECT * FROM ({}) LIMIT 0", sql.trim().trim_end_matches(';')), &[]) {
            Ok(_) => {
                self.error = None;
                let title = trf("db.query_result", &[("name", &self.name)]);
                // A result already open shows the new SQL.
                v.panes.retain(|p| !matches!(p, Pane::Sheet(s) if s.title() == title));
                v.show(Pane::Sheet(sheet::Datasheet::new(sheet::Src::Query(title, sql))));
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub fn save(&mut self, v: &mut DbView) {
        if v.save_object(Kind::Query, &self.name, &self.records()) {
            v.notices.push(crate::app::Notice::info(trf("db.saved", &[("name", &self.name)])));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        ui.horizontal(|ui| {
            if ui.button(tr("db.run")).clicked() {
                self.run(v);
            }
            if ui.button(tr("db.save")).clicked() {
                self.save(v);
            }
            ui.separator();
            let was = self.sql_mode;
            ui.selectable_value(&mut self.sql_mode, false, tr("db.q_design"));
            ui.selectable_value(&mut self.sql_mode, true, "SQL");
            if self.sql_mode && !was {
                self.sql = self.design.to_sql();
            }
            if !self.sql_mode && was && self.design.tables.is_empty() && !self.sql.trim().is_empty() {
                // The designer can't read SQL back.
                self.sql_mode = true;
                self.error = Some(tr("db.sql_only").to_string());
            }
            ui.separator();
            if !self.sql_mode {
                egui::ComboBox::from_id_salt("db_qtype").selected_text(super::trk("db.qtype", self.design.kind.key())).show_ui(ui, |ui| {
                    for k in QueryType::ALL {
                        ui.selectable_value(&mut self.design.kind, k, super::trk("db.qtype", k.key()));
                    }
                });
                if matches!(self.design.kind, QueryType::Append | QueryType::MakeTable) {
                    ui.label(tr("db.target"));
                    ui.add(egui::TextEdit::singleline(&mut self.design.target).desired_width(120.0));
                }
                if self.design.kind == QueryType::Select {
                    ui.checkbox(&mut self.design.totals, tr("db.totals"));
                    ui.checkbox(&mut self.design.distinct, tr("db.distinct"));
                    ui.label(tr("db.top"));
                    ui.add(egui::DragValue::new(&mut self.design.top).range(0..=1_000_000));
                }
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.separator();
        if self.sql_mode {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.sql).code_editor().desired_width(f32::INFINITY).desired_rows(16));
            });
            return;
        }
        self.tables_ui(ui, v);
        ui.separator();
        ui.label(RichText::new(tr("db.q_sql_preview")).weak().small());
        ui.label(RichText::new(self.design.to_sql()).monospace().small());
        ui.separator();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.grid_ui(ui));
    }

    fn tables_ui(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        let mut add = None;
        let mut remove = None;
        egui::ScrollArea::horizontal().id_salt("db_qtables").max_height(190.0).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for (i, t) in self.design.tables.iter().enumerate() {
                    let cols = v.db.columns(&RowSource::Table(t.name.clone())).map(|c| c.0).unwrap_or_default();
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(150.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&t.name).strong());
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                        });
                        egui::ScrollArea::vertical().id_salt(("db_qt", i)).max_height(140.0).show(ui, |ui| {
                            for c in std::iter::once("*".to_string()).chain(cols) {
                                if ui.selectable_label(false, &c).on_hover_text(tr("db.q_add_field")).clicked() {
                                    add = Some(QField::column(&t.name, &c));
                                }
                            }
                        });
                    });
                }
                crate::widgets::menu_button(ui, tr("db.add_table"), |ui| {
                    for t in v.db.table_names().into_iter().chain(v.db.view_names()) {
                        if ui.button(&t).clicked() {
                            let db = &v.db;
                            self.add_table(db, &t);
                            ui.close();
                        }
                    }
                });
            });
        });
        if let Some(i) = remove {
            self.remove_table(i);
        }
        if let Some(f) = add {
            self.design.fields.push(f);
        }
        // Joins: their kind can change, or they can be removed.
        let mut remove = None;
        for (i, j) in self.design.joins.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}.{} = {}.{}", j.left, j.left_col, j.right, j.right_col));
                for (k, key) in [(JoinKind::Inner, "db.join_inner"), (JoinKind::Left, "db.join_left"), (JoinKind::Right, "db.join_right")] {
                    ui.selectable_value(&mut j.kind, k, tr(key));
                }
                if ui.small_button("×").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.design.joins.remove(i);
        }
    }

    fn grid_ui(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.design;
        let tables: Vec<String> = d.tables.iter().map(|t| t.name.clone()).collect();
        let mut remove = None;
        let or_rows = self.or_rows;
        egui::Grid::new("db_qgrid").striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
            ui.label(RichText::new(tr("db.q_field")).strong());
            for (i, f) in d.fields.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut f.field).desired_width(110.0));
                    if ui.small_button("×").clicked() {
                        remove = Some(i);
                    }
                });
            }
            ui.end_row();
            ui.label(RichText::new(tr("db.q_table")).strong());
            for (i, f) in d.fields.iter_mut().enumerate() {
                egui::ComboBox::from_id_salt(("db_qft", i)).width(120.0).selected_text(f.table.as_str()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.table, String::new(), tr("db.q_expression"));
                    for t in &tables {
                        ui.selectable_value(&mut f.table, t.clone(), t);
                    }
                });
            }
            ui.end_row();
            ui.label(RichText::new(tr("db.q_alias")).strong());
            for f in d.fields.iter_mut() {
                ui.add(egui::TextEdit::singleline(&mut f.alias).desired_width(120.0));
            }
            ui.end_row();
            if d.kind == QueryType::Update {
                ui.label(RichText::new(tr("db.q_update_to")).strong());
                for f in d.fields.iter_mut() {
                    ui.add(egui::TextEdit::singleline(&mut f.update_to).desired_width(120.0));
                }
                ui.end_row();
            }
            if d.totals {
                ui.label(RichText::new(tr("db.q_total")).strong());
                for (i, f) in d.fields.iter_mut().enumerate() {
                    egui::ComboBox::from_id_salt(("db_qtot", i)).width(120.0).selected_text(super::trk("db.total", f.total.key())).show_ui(ui, |ui| {
                        for t in Total::ALL {
                            ui.selectable_value(&mut f.total, t, super::trk("db.total", t.key()));
                        }
                    });
                }
                ui.end_row();
            }
            ui.label(RichText::new(tr("db.q_sort")).strong());
            for (i, f) in d.fields.iter_mut().enumerate() {
                let label = |s: Sort| match s {
                    Sort::None => tr("db.sort_none"),
                    Sort::Asc => tr("db.sort_asc"),
                    Sort::Desc => tr("db.sort_desc"),
                };
                egui::ComboBox::from_id_salt(("db_qsort", i)).width(120.0).selected_text(label(f.sort)).show_ui(ui, |ui| {
                    for s in [Sort::None, Sort::Asc, Sort::Desc] {
                        ui.selectable_value(&mut f.sort, s, label(s));
                    }
                });
            }
            ui.end_row();
            ui.label(RichText::new(tr("db.q_show")).strong());
            for f in d.fields.iter_mut() {
                ui.checkbox(&mut f.show, "");
            }
            ui.end_row();
            for r in 0..or_rows {
                ui.label(RichText::new(if r == 0 { tr("db.q_criteria") } else { tr("db.q_or") }).strong());
                for f in d.fields.iter_mut() {
                    if f.criteria.len() <= r {
                        f.criteria.resize(r + 1, String::new());
                    }
                    ui.add(egui::TextEdit::singleline(&mut f.criteria[r]).desired_width(120.0));
                }
                ui.end_row();
            }
        });
        for f in d.fields.iter_mut() {
            while f.criteria.last().is_some_and(|c| c.trim().is_empty()) {
                f.criteria.pop();
            }
        }
        if let Some(i) = remove {
            d.fields.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.button(tr("db.q_add_expression")).clicked() {
                d.fields.push(QField::default());
            }
            if ui.button(tr("db.q_add_or")).clicked() {
                self.or_rows += 1;
            }
        });
    }
}

/// What a query written in SQL does.
pub fn sql_kind(sql: &str) -> QueryType {
    let s = sql.trim_start().to_ascii_uppercase();
    if s.starts_with("UPDATE") {
        QueryType::Update
    } else if s.starts_with("DELETE") {
        QueryType::Delete
    } else if s.starts_with("INSERT") || s.starts_with("REPLACE") {
        QueryType::Append
    } else if s.starts_with("CREATE TABLE") {
        QueryType::MakeTable
    } else {
        QueryType::Select
    }
}
