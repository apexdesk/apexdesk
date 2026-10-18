//! The datasheet: rows of a table, a query or a linked table, a page at a
//! time, with sorting (a click on a heading), a search box, filters on
//! columns, editing in place (tables), a new row at the end, deleting rows
//! and the related rows of other tables under a row (subdatasheet).

use apex_db::sqlite::{parse_input, Filter, FieldType, Page, RowSource};
use apex_db::Value;
use eframe::egui::{self, RichText};

use super::{DbView, Step};
use crate::i18n::{tr, trf};

pub const PAGE: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum Src {
    Table(String),
    /// A saved query: its name and SQL.
    Query(String, String),
    /// A table of a database server.
    Link(String),
}

/// An expanded row's related rows.
struct Sub {
    row: usize,
    /// Table, its column and the parent's value.
    table: String,
    page: Page,
}

pub struct Datasheet {
    pub src: Src,
    pub filter: Filter,
    pub sort: Vec<(String, bool)>,
    pub offset: usize,
    pub page: Page,
    pub total: usize,
    loaded: Option<u64>,
    pub sel: Option<(usize, usize)>,
    /// The cell being edited and its text.
    pub edit: Option<(usize, usize, String)>,
    /// The new row's texts.
    pub new_row: Vec<String>,
    pub error: Option<String>,
    types: Vec<FieldType>,
    /// Filter being added: column, operator, value.
    cond: (usize, usize, String),
    sub: Option<Sub>,
    password: String,
}

const OPS: [&str; 9] = ["=", "<>", "<", ">", "<=", ">=", "LIKE", "IS NULL", "IS NOT NULL"];

impl Datasheet {
    pub fn new(src: Src) -> Datasheet {
        Datasheet {
            src,
            filter: Filter::default(),
            sort: Vec::new(),
            offset: 0,
            page: Page::default(),
            total: 0,
            loaded: None,
            sel: None,
            edit: None,
            new_row: Vec::new(),
            error: None,
            types: Vec::new(),
            cond: (0, 0, String::new()),
            sub: None,
            password: String::new(),
        }
    }

    pub fn title(&self) -> String {
        match &self.src {
            Src::Table(t) | Src::Query(t, _) | Src::Link(t) => t.clone(),
        }
    }

    pub fn editable(&self) -> bool {
        matches!(self.src, Src::Table(_))
    }

    fn row_source(&self) -> Option<RowSource> {
        match &self.src {
            Src::Table(t) => Some(RowSource::Table(t.clone())),
            Src::Query(_, sql) => Some(RowSource::Sql(sql.clone())),
            Src::Link(_) => None,
        }
    }

    pub fn status(&self) -> String {
        trf("db.rows_total", &[("n", &self.total)])
    }

    /// Reads the page again when something changed.
    pub fn load(&mut self, v: &mut DbView) {
        if self.loaded == Some(v.revision) {
            return;
        }
        self.loaded = Some(v.revision);
        self.error = None;
        let res = match self.row_source() {
            Some(src) => v.db.count(&src, &self.filter).and_then(|n| {
                self.total = n;
                if self.offset >= n && n > 0 {
                    self.offset = (n - 1) / PAGE * PAGE;
                }
                v.db.page(&src, &self.filter, &self.sort, self.offset, PAGE)
            }),
            None => {
                let name = self.title();
                match super::links::page(v, &name, &self.sort, self.offset, PAGE) {
                    Ok(p) => {
                        self.total = self.offset + p.rows.len() + if p.rows.len() == PAGE { 1 } else { 0 };
                        Ok(p)
                    }
                    Err(e) => Err(apex_db::Error::Db(e)),
                }
            }
        };
        match res {
            Ok(p) => {
                self.types = p.types.clone();
                if let Src::Table(t) = &self.src {
                    // The declared kinds of the table (AutoNumber included).
                    if let Ok(d) = v.db.design(t) {
                        self.types = p.columns.iter().map(|c| d.field(c).map(|f| f.ty).unwrap_or_default()).collect();
                    }
                }
                self.new_row.resize(p.columns.len(), String::new());
                self.page = p;
            }
            Err(e) => {
                self.page = Page::default();
                self.error = Some(e.to_string());
            }
        }
        self.sub = None;
    }

    pub fn reload(&mut self) {
        self.loaded = None;
    }

    /// Sorts by column `c`: up, then down, then not.
    pub fn toggle_sort(&mut self, c: usize) {
        let Some(name) = self.page.columns.get(c).cloned() else { return };
        self.sort = match self.sort.first() {
            Some((n, true)) if *n == name => vec![(name, false)],
            Some((n, false)) if *n == name => Vec::new(),
            _ => vec![(name, true)],
        };
        self.offset = 0;
        self.reload();
    }

    /// The message for a refused change: the field's validation text when
    /// its rule failed.
    fn refusal(&self, v: &DbView, e: &apex_db::Error) -> String {
        let table = if let Src::Table(t) = &self.src { Some(t.as_str()) } else { None };
        refusal(v, table, e)
    }

    /// Ends the edit of a cell, writing its value.
    pub fn commit(&mut self, v: &mut DbView) {
        let Some((r, c, text)) = self.edit.take() else { return };
        let Src::Table(table) = self.src.clone() else { return };
        let ty = self.types.get(c).copied().unwrap_or_default();
        let (Some(&rowid), Some(col)) = (self.page.rowids.get(r), self.page.columns.get(c).cloned()) else { return };
        let Some(val) = parse_input(&text, ty) else {
            self.error = Some(trf("db.bad_value", &[("field", &col)]));
            return;
        };
        if self.page.rows[r][c] == val {
            return;
        }
        match v.db.update_cell(&table, rowid, &col, &val) {
            Ok(old) => v.push_step(Step::Cell { table, rowid, col, old, new: val }),
            Err(e) => self.error = Some(self.refusal(v, &e)),
        }
    }

    /// Adds the new row's values as a row.
    pub fn insert_new(&mut self, v: &mut DbView) {
        let Src::Table(table) = self.src.clone() else { return };
        let mut cols = Vec::new();
        let mut vals = Vec::new();
        for (i, t) in self.new_row.iter().enumerate() {
            if t.trim().is_empty() {
                continue;
            }
            let ty = self.types.get(i).copied().unwrap_or_default();
            let Some(val) = parse_input(t, ty) else {
                self.error = Some(trf("db.bad_value", &[("field", &self.page.columns[i])]));
                return;
            };
            cols.push(self.page.columns[i].clone());
            vals.push(val);
        }
        match v.db.insert_row(&table, &cols, &vals, None) {
            Ok(rowid) => {
                // The row as stored (defaults and numbers given).
                let (all, row) = v.db.row(&table, rowid).unwrap_or((cols, vals));
                v.push_step(Step::Insert { table, rowid, cols: all, vals: row });
                self.new_row.iter_mut().for_each(String::clear);
                // The last page shows it.
                self.total += 1;
                self.offset = self.total.saturating_sub(1) / PAGE * PAGE;
                self.error = None;
            }
            Err(e) => self.error = Some(self.refusal(v, &e)),
        }
    }

    /// Deletes row `r` of the page.
    pub fn delete_row(&mut self, v: &mut DbView, r: usize) {
        let Src::Table(table) = self.src.clone() else { return };
        let Some(&rowid) = self.page.rowids.get(r) else { return };
        match v.db.delete_row(&table, rowid) {
            Ok((cols, vals)) => {
                v.push_step(Step::Delete { table, rowid, cols, vals });
                self.sel = None;
            }
            Err(e) => self.error = Some(self.refusal(v, &e)),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        if let Src::Link(name) = &self.src {
            let link = super::links::load(v, name);
            if let Some(key) = link.as_ref().map(super::links::Link::password_key)
                && !v.passwords.contains_key(&key)
            {
                let name = name.clone();
                // The server is the file's to choose: show where the
                // password goes, and when it goes unencrypted.
                let server = link.as_ref().map(super::links::Link::endpoint).unwrap_or_default();
                ui.label(trf("db.link_password_for", &[("name", &name), ("server", &server)]));
                if link.as_ref().is_some_and(|l| l.kind == apex_sheet::query::DbKind::SqlServer && l.user.trim().is_empty() && apex_query::sources::sqlserver::INTEGRATED) {
                    ui.colored_label(ui.visuals().warn_fg_color, tr("db.windows_account"));
                }
                if let Some(l) = link.filter(|l| !l.tls) {
                    let key = if l.kind == apex_sheet::query::DbKind::SqlServer { "db.link_unverified_tls" } else { "db.link_no_tls" };
                    ui.colored_label(ui.visuals().warn_fg_color, tr(key));
                }
                let r = ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                if ui.button(tr("dialog.ok")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    v.passwords.insert(key, std::mem::take(&mut self.password));
                    self.reload();
                }
                return;
            }
        }
        self.load(v);
        self.toolbar(ui, v);
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        self.keys(ui, v);
        self.grid(ui, v);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        ui.horizontal_wrapped(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.filter.search).hint_text(tr("db.search")).desired_width(160.0));
            if r.changed() {
                self.offset = 0;
                self.reload();
            }
            ui.separator();
            // A filter on a column.
            let cols = self.page.columns.clone();
            if !cols.is_empty() {
                self.cond.0 = self.cond.0.min(cols.len() - 1);
                egui::ComboBox::from_id_salt("db_filter_col").selected_text(&cols[self.cond.0]).show_ui(ui, |ui| {
                    for (i, c) in cols.iter().enumerate() {
                        ui.selectable_value(&mut self.cond.0, i, c);
                    }
                });
                egui::ComboBox::from_id_salt("db_filter_op").width(60.0).selected_text(OPS[self.cond.1]).show_ui(ui, |ui| {
                    for (i, o) in OPS.iter().enumerate() {
                        ui.selectable_value(&mut self.cond.1, i, *o);
                    }
                });
                if self.cond.1 < 7 {
                    ui.add(egui::TextEdit::singleline(&mut self.cond.2).desired_width(90.0));
                }
                if ui.button(tr("db.add_filter")).clicked() {
                    let ty = self.types.get(self.cond.0).copied().unwrap_or_default();
                    let val = if OPS[self.cond.1] == "LIKE" { Value::Text(apex_db::query::like_pattern(&self.cond.2)) } else { parse_input(&self.cond.2, ty).unwrap_or(Value::Text(self.cond.2.clone())) };
                    self.filter.conditions.push((cols[self.cond.0].clone(), OPS[self.cond.1].to_string(), val));
                    self.offset = 0;
                    self.reload();
                }
            }
            let mut remove = None;
            for (i, (c, op, val)) in self.filter.conditions.iter().enumerate() {
                if ui.small_button(format!("{c} {op} {} ×", val.to_text())).on_hover_text(tr("db.remove_filter")).clicked() {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                self.filter.conditions.remove(i);
                self.reload();
            }
            ui.separator();
            let last = self.total.saturating_sub(1) / PAGE * PAGE;
            if ui.add_enabled(self.offset > 0, egui::Button::new("⏮")).on_hover_text(tr("db.first_page")).clicked() {
                self.offset = 0;
                self.reload();
            }
            if ui.add_enabled(self.offset > 0, egui::Button::new("◀")).on_hover_text(tr("db.prev_page")).clicked() {
                self.offset = self.offset.saturating_sub(PAGE);
                self.reload();
            }
            let shown = self.page.rows.len();
            ui.label(trf("db.rows_of", &[("from", &(if shown == 0 { 0 } else { self.offset + 1 })), ("to", &(self.offset + shown)), ("n", &self.total)]));
            if ui.add_enabled(self.offset < last, egui::Button::new("▶")).on_hover_text(tr("db.next_page")).clicked() {
                self.offset += PAGE;
                self.reload();
            }
            if ui.add_enabled(self.offset < last, egui::Button::new("⏭")).on_hover_text(tr("db.last_page")).clicked() {
                self.offset = last;
                self.reload();
            }
            if self.editable() {
                ui.separator();
                if ui.add_enabled(self.sel.is_some(), egui::Button::new(tr("db.delete_row"))).clicked() {
                    if let Some((r, _)) = self.sel {
                        self.delete_row(v, r);
                    }
                }
            }
        });
    }

    fn keys(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        let editing = self.edit.is_some();
        let (enter, esc, del, f2, up, down, left, right) = ui.input(|i| {
            (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Delete), i.key_pressed(egui::Key::F2), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight))
        });
        self.key_step(v, editing, enter, esc, del && !ui.ctx().egui_wants_keyboard_input(), f2, (up, down, left, right), ui.ctx().egui_wants_keyboard_input());
    }

    /// What a key does (apart from egui, for the tests).
    #[allow(clippy::too_many_arguments)]
    pub fn key_step(&mut self, v: &mut DbView, editing: bool, enter: bool, esc: bool, del: bool, f2: bool, arrows: (bool, bool, bool, bool), typing: bool) {
        if editing {
            if esc {
                self.edit = None;
            } else if enter {
                let at = self.edit.as_ref().map(|e| (e.0, e.1));
                self.commit(v);
                // Enter goes down, like a spreadsheet.
                if let Some((r, c)) = at {
                    self.sel = Some(((r + 1).min(self.page.rows.len().saturating_sub(1)), c));
                }
            }
            return;
        }
        if typing {
            return;
        }
        let Some((r, c)) = self.sel else { return };
        let (up, down, left, right) = arrows;
        let rows = self.page.rows.len();
        let cols = self.page.columns.len();
        if up && r > 0 {
            self.sel = Some((r - 1, c));
        }
        if down && r + 1 < rows {
            self.sel = Some((r + 1, c));
        }
        if left && c > 0 {
            self.sel = Some((r, c - 1));
        }
        if right && c + 1 < cols {
            self.sel = Some((r, c + 1));
        }
        if (enter || f2) && self.editable() && r < rows {
            self.edit = Some((r, c, cell_text(&self.page.rows[r][c])));
        }
        if del && self.editable() && r < rows {
            self.delete_row(v, r);
        }
    }

    pub(super) fn grid(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        let cols = self.page.columns.len();
        if cols == 0 {
            return;
        }
        let children: Vec<apex_db::Relation> = match &self.src {
            Src::Table(t) => v.db.relations().into_iter().filter(|r| r.ref_table.eq_ignore_ascii_case(t)).collect(),
            _ => Vec::new(),
        };
        // The zoom of the status bar scales the text, the cells and the gaps.
        let z = v.zoom;
        let widths: Vec<f32> = (0..cols).map(|c| col_width(&self.page, c) * z).collect();
        let h = 20.0 * z;
        let mut sort_click = None;
        let mut start_edit = None;
        let mut expand = None;
        let editable = self.editable();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            zoom_style(ui, z);
            egui::Grid::new(("db_grid", self.title())).striped(true).min_col_width(16.0 * z).spacing([6.0 * z, 3.0 * z]).show(ui, |ui| {
                ui.label("");
                for (c, name) in self.page.columns.iter().enumerate() {
                    let arrow = match self.sort.first() {
                        Some((n, true)) if n == name => " ▲",
                        Some((n, false)) if n == name => " ▼",
                        _ => "",
                    };
                    let r = ui.add_sized([widths[c], h], egui::Button::new(RichText::new(format!("{name}{arrow}")).strong()).frame(false));
                    if r.on_hover_text(tr("db.sort_tip")).clicked() {
                        sort_click = Some(c);
                    }
                }
                ui.end_row();
                for r in 0..self.page.rows.len() {
                    let expanded = self.sub.as_ref().is_some_and(|s| s.row == r);
                    if !children.is_empty() {
                        if ui.small_button(if expanded { "−" } else { "+" }).on_hover_text(tr("db.related_rows")).clicked() {
                            expand = Some(r);
                        }
                    } else {
                        ui.label(RichText::new(format!("{}", self.offset + r + 1)).weak());
                    }
                    for c in 0..cols {
                        let editing = matches!(&self.edit, Some((er, ec, _)) if *er == r && *ec == c);
                        if editing {
                            let (_, _, text) = self.edit.as_mut().unwrap();
                            let resp = ui.add_sized([widths[c], h], egui::TextEdit::singleline(text));
                            resp.request_focus();
                            continue;
                        }
                        let val = &self.page.rows[r][c];
                        let text = match val {
                            Value::Bool(b) => if *b { "☑".to_string() } else { "☐".to_string() },
                            v => cell_text(v),
                        };
                        let selected = self.sel == Some((r, c));
                        let resp = ui.add_sized([widths[c], h], egui::Button::selectable(selected, truncate(&text, 60)).frame(false));
                        if resp.clicked() {
                            self.sel = Some((r, c));
                            // A click on a yes/no cell flips it.
                            if editable && matches!(val, Value::Bool(_)) && selected {
                                start_edit = Some((r, c, if matches!(val, Value::Bool(true)) { "0" } else { "1" }.to_string(), true));
                            }
                        }
                        if resp.double_clicked() && editable {
                            start_edit = Some((r, c, cell_text(val), false));
                        }
                    }
                    ui.end_row();
                    if expanded {
                        ui.label("");
                        if let Some(s) = &self.sub {
                            let sp = &s.page;
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&s.table).italics());
                                egui::Grid::new(("db_sub", r)).striped(true).show(ui, |ui| {
                                    for c in &sp.columns {
                                        ui.label(RichText::new(c).strong());
                                    }
                                    ui.end_row();
                                    for row in &sp.rows {
                                        for val in row {
                                            ui.label(truncate(&cell_text(val), 40));
                                        }
                                        ui.end_row();
                                    }
                                });
                            });
                        }
                        ui.end_row();
                    }
                }
                // The new row.
                if editable {
                    ui.label(RichText::new("*").strong()).on_hover_text(tr("db.new_row_tip"));
                    let mut enter = false;
                    for c in 0..cols {
                        let auto = self.types.get(c) == Some(&FieldType::AutoNumber);
                        let hint = if auto { tr("db.auto") } else { "" };
                        let r = ui.add_enabled(!auto, egui::TextEdit::singleline(&mut self.new_row[c]).hint_text(hint).desired_width(widths[c]));
                        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    }
                    ui.end_row();
                    if enter && self.new_row.iter().any(|t| !t.trim().is_empty()) {
                        self.insert_new(v);
                    }
                }
            });
        });
        if let Some(c) = sort_click {
            self.toggle_sort(c);
        }
        if let Some((r, c, t, commit)) = start_edit {
            self.edit = Some((r, c, t));
            if commit {
                self.commit(v);
            }
        }
        if let Some(r) = expand {
            if self.sub.as_ref().is_some_and(|s| s.row == r) {
                self.sub = None;
            } else {
                self.expand(v, r, &children);
            }
        }
    }

    /// Reads the rows related to row `r` (of the first relation).
    fn expand(&mut self, v: &mut DbView, r: usize, children: &[apex_db::Relation]) {
        let Some(rel) = children.first() else { return };
        let conds: Vec<(String, String, Value)> = rel
            .columns
            .iter()
            .zip(&rel.ref_columns)
            .filter_map(|(child, parent)| {
                let pc = self.page.columns.iter().position(|c| c.eq_ignore_ascii_case(parent))?;
                Some((child.clone(), "=".to_string(), self.page.rows[r][pc].clone()))
            })
            .collect();
        let f = Filter { conditions: conds, ..Default::default() };
        match v.db.page(&RowSource::Table(rel.table.clone()), &f, &[], 0, 200) {
            Ok(page) => self.sub = Some(Sub { row: r, table: rel.table.clone(), page }),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// The rows (all pages, filtered and sorted) as a document to print.
    pub fn document(&self, v: &DbView) -> apex_doc::Document {
        let page = match self.row_source() {
            Some(src) => v.db.page(&src, &self.filter, &self.sort, 0, 20_000).unwrap_or_default(),
            None => self.page.clone(),
        };
        super::report_ui::table_doc(&self.title(), &page.columns, &page.rows)
    }
}

/// The message for a refused change: the field's validation text when
/// its rule failed.
pub fn refusal(v: &DbView, table: Option<&str>, e: &apex_db::Error) -> String {
    let msg = e.to_string();
    if let (Some(t), Some(i)) = (table, msg.find("CHECK constraint failed: ck_")) {
        let field = msg[i + 28..].trim().to_string();
        if let Ok(d) = v.db.design(t) {
            if let Some(f) = d.field(&field) {
                if !f.validation_text.is_empty() {
                    return f.validation_text.clone();
                }
                return trf("db.rule_failed", &[("field", &f.name), ("rule", &f.check)]);
            }
        }
    }
    if msg.contains("NOT NULL constraint failed") {
        return trf("db.required_failed", &[("field", &msg.rsplit('.').next().unwrap_or(""))]);
    }
    if msg.contains("FOREIGN KEY constraint failed") {
        return tr("db.fk_failed").to_string();
    }
    if msg.contains("UNIQUE constraint failed") {
        return tr("db.unique_failed").to_string();
    }
    trf("db.error", &[("error", &msg)])
}

/// A value as the text shown and edited.
pub fn cell_text(v: &Value) -> String {
    match v {
        Value::Date(d) => apex_db::model::fmt_date(*d),
        Value::Blob(b) => trf("db.bytes", &[("n", &b.len())]),
        v => v.to_text(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > n || first.len() < s.len() {
        format!("{}…", first.chars().take(n).collect::<String>())
    } else {
        first.to_string()
    }
}

/// A column's width from its heading and first rows.
/// Scales the text and the sizes of controls drawn in `ui` by `z`.
fn zoom_style(ui: &mut egui::Ui, z: f32) {
    let s = ui.style_mut();
    for f in s.text_styles.values_mut() {
        f.size *= z;
    }
    s.spacing.interact_size *= z;
    s.spacing.button_padding *= z;
}

fn col_width(p: &Page, c: usize) -> f32 {
    let mut n = p.columns[c].chars().count() + 2;
    for r in p.rows.iter().take(30) {
        n = n.max(cell_text(&r[c]).chars().count().min(40));
    }
    (n as f32 * 7.5).clamp(50.0, 320.0)
}
