//! Data > Relationships and Data > Measures: the data model dialog. The
//! relationships join the workbook's tables (a key of the "many" table to
//! the unique key of the "one" table); the measures are DAX formulas that
//! pivot tables on the model show. Every change is one undo step and
//! refreshes the pivot tables on the model. An Excel data model the file
//! brought along is listed, read only.

use apex_sheet::model::{self, DataModel, DaxError, Measure, RelError, Relationship, dax};

use super::*;

pub(super) struct ModelDlg {
    /// 0: relationships, 1: measures.
    page: usize,
    data: model::ModelData,
    excel: apex_io::ExcelModel,
    sel: Option<usize>,
    /// The relationship being edited (None in `.0`: a new one).
    edit: Option<(Option<usize>, Relationship)>,
    rel_error: Option<String>,
    /// Measure editor: the one edited (None: new), name, table, formula,
    /// format, and the last check's outcome (ok, text).
    current: Option<usize>,
    name: String,
    table: String,
    formula: String,
    format: String,
    check: Option<(bool, String)>,
}

/// Number formats offered for measures.
const FORMATS: [&str; 6] = ["General", "0", "#,##0", "#,##0.00", "0.00%", "#,##0.00 \"€\""];

/// The text of a formula error.
fn dax_error(e: &DaxError) -> String {
    let t = trf(e.kind.key(), &[("x", &e.arg)]);
    if e.pos > 0 { trf("dax.at", &[("t", &t), ("n", &e.pos)]) } else { t }
}

fn rel_error(e: RelError) -> &'static str {
    tr(match e {
        RelError::NoTable => "model.err_no_table",
        RelError::NoColumn => "model.err_no_column",
        RelError::SameTable => "model.err_same_table",
        RelError::NotUnique => "model.err_not_unique",
        RelError::Duplicate => "model.err_duplicate",
    })
}

/// A combo box of names; true when changed.
fn pick(ui: &mut egui::Ui, id: (&str, usize), value: &mut String, choices: &[String], width: f32) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).width(width).selected_text(value.as_str()).show_ui(ui, |ui| {
        for c in choices {
            if ui.selectable_label(value.eq_ignore_ascii_case(c), c).clicked() {
                *value = c.clone();
                changed = true;
            }
        }
    });
    changed
}

impl SheetView {
    /// Opens the dialog on its relationships (0) or measures (1) page.
    pub(super) fn open_data_model(&mut self, page: usize) {
        self.settle_edit();
        let data = self.wb.model_data();
        let table = data.tables.first().map(|t| t.name.clone()).unwrap_or_default();
        let mut d = ModelDlg { page, data, excel: apex_io::excel_model(&self.wb), sel: None, edit: None, rel_error: None, current: None, name: String::new(), table, formula: String::new(), format: String::new(), check: None };
        if page == 1 && !self.wb.analysis.model.measures.is_empty() {
            d.load_measure(&self.wb.analysis.model, 0);
        } else if page == 1 {
            d.new_measure(&self.wb.analysis.model);
        }
        self.model_dlg = Some(Box::new(d));
    }

    fn set_model(&mut self, m: DataModel) {
        self.wb.set_data_model(m);
        if let Some(d) = &mut self.model_dlg {
            d.data = self.wb.model_data();
        }
    }

    pub(super) fn data_model_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.model_dlg.take() else { return };
        let mut model = self.wb.analysis.model.clone();
        let before = model.clone();
        let closed = widgets::info_dialog(ctx, "data_model", tr("model.title"), 720.0, |ui| {
            ui.horizontal(|ui| {
                for (i, key) in ["model.relationships", "model.measures"].iter().enumerate() {
                    if ui.selectable_label(d.page == i, RichText::new(tr(key)).strong()).clicked() && d.page != i {
                        d.page = i;
                        if i == 1 && d.name.is_empty() {
                            match model.measures.is_empty() {
                                true => d.new_measure(&model),
                                false => d.load_measure(&model, 0),
                            }
                        }
                    }
                }
            });
            ui.separator();
            if d.page == 0 {
                d.relationships_page(ui, &mut model);
            } else {
                d.measures_page(ui, &mut model, &self.wb);
            }
        });
        if model != before {
            self.model_dlg = Some(d);
            self.set_model(model);
            return;
        }
        if closed {
            self.focus_grid = true;
        } else {
            self.model_dlg = Some(d);
        }
    }
}

impl ModelDlg {
    /// The page shown (0: relationships, 1: measures).
    pub(super) fn page(&self) -> usize {
        self.page
    }

    fn table_names(&self) -> Vec<String> {
        self.data.tables.iter().map(|t| t.name.clone()).collect()
    }

    fn column_names(&self, table: &str) -> Vec<String> {
        self.data.table(table).map(|t| self.data.tables[t].columns.clone()).unwrap_or_default()
    }

    fn relationships_page(&mut self, ui: &mut egui::Ui, model: &mut DataModel) {
        ui.label(RichText::new(tr("model.rel_note")).weak());
        ui.add_space(4.0);
        if self.data.tables.len() < 2 {
            ui.label(tr("model.need_two_tables"));
        }
        let mut toggle: Option<usize> = None;
        egui::ScrollArea::vertical().id_salt("model_rels").max_height(190.0).auto_shrink([false, true]).show(ui, |ui| {
            if model.relationships.is_empty() {
                ui.label(RichText::new(tr("model.no_relationships")).italics().weak());
            }
            egui::Grid::new("model_rel_grid").num_columns(4).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                if !model.relationships.is_empty() {
                    ui.label(RichText::new(tr("model.active")).small().strong());
                    ui.label(RichText::new(tr("model.many_side")).small().strong());
                    ui.label(RichText::new(tr("model.one_side")).small().strong());
                    ui.label("");
                    ui.end_row();
                }
                for (i, r) in model.relationships.iter().enumerate() {
                    let mut on = r.active;
                    if ui.checkbox(&mut on, "").on_hover_text(tr("model.active_tip")).changed() {
                        toggle = Some(i);
                    }
                    let from = format!("{}[{}]", r.from_table, r.from_column);
                    if ui.selectable_label(self.sel == Some(i), &from).clicked() {
                        self.sel = Some(i);
                    }
                    if ui.selectable_label(self.sel == Some(i), format!("{}[{}]", r.to_table, r.to_column)).clicked() {
                        self.sel = Some(i);
                    }
                    match model::check_relationship(&self.data, &model.relationships, r, Some(i)) {
                        Ok(()) => ui.label(""),
                        Err(e) => ui.label(RichText::new(format!("⚠ {}", rel_error(e))).color(ui.visuals().error_fg_color).small()),
                    };
                    ui.end_row();
                }
            });
        });
        if let Some(i) = toggle {
            model.relationships[i].active = !model.relationships[i].active;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let tables = self.table_names();
            if ui.add_enabled(tables.len() >= 2, egui::Button::new(format!("{}…", tr("model.new")))).clicked() {
                let t = tables.first().cloned().unwrap_or_default();
                let u = tables.get(1).cloned().unwrap_or_default();
                let (c, k) = (self.column_names(&t).first().cloned().unwrap_or_default(), self.column_names(&u).first().cloned().unwrap_or_default());
                self.edit = Some((None, Relationship::new(&t, &c, &u, &k)));
                self.rel_error = None;
            }
            let sel = self.sel.filter(|i| *i < model.relationships.len());
            if ui.add_enabled(sel.is_some(), egui::Button::new(format!("{}…", tr("model.edit")))).clicked() {
                if let Some(i) = sel {
                    self.edit = Some((Some(i), model.relationships[i].clone()));
                    self.rel_error = None;
                }
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new(tr("model.delete"))).clicked() {
                if let Some(i) = sel {
                    model.relationships.remove(i);
                    self.sel = None;
                    self.edit = None;
                }
            }
            ui.separator();
            if ui.add_enabled(tables.len() >= 2, egui::Button::new(tr("model.detect"))).on_hover_text(tr("model.detect_tip")).clicked() {
                let found = model::detect_relationships(&self.data, &model.relationships);
                self.rel_error = Some(if found.is_empty() { tr("model.detect_none").to_string() } else { trf("model.detect_found", &[("n", &found.len())]) });
                model.relationships.extend(found);
            }
        });
        if let Some((at, r)) = &mut self.edit {
            let (at, mut r) = (*at, r.clone());
            let tables = self.table_names();
            ui.add_space(6.0);
            widgets::section(ui, tr(if at.is_some() { "model.edit_rel" } else { "model.new_rel" }));
            egui::Grid::new("model_rel_edit").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(tr("model.table"));
                if pick(ui, ("rel_from", 0), &mut r.from_table, &tables, 180.0) {
                    r.from_column = self.column_names(&r.from_table).first().cloned().unwrap_or_default();
                }
                pick(ui, ("rel_from_col", 0), &mut r.from_column, &self.column_names(&r.from_table), 180.0);
                ui.end_row();
                ui.label(tr("model.related_table"));
                if pick(ui, ("rel_to", 0), &mut r.to_table, &tables, 180.0) {
                    // The same column name is the likely key.
                    let cols = self.column_names(&r.to_table);
                    r.to_column = cols.iter().find(|c| c.eq_ignore_ascii_case(&r.from_column)).or(cols.first()).cloned().unwrap_or_default();
                }
                pick(ui, ("rel_to_col", 0), &mut r.to_column, &self.column_names(&r.to_table), 180.0);
                ui.end_row();
            });
            let mut done = false;
            ui.horizontal(|ui| {
                if ui.button(RichText::new(tr("model.save")).strong()).clicked() {
                    match model::check_relationship(&self.data, &model.relationships, &r, at) {
                        Ok(()) => {
                            match at {
                                Some(i) => model.relationships[i] = Relationship { active: model.relationships[i].active, ..r.clone() },
                                None => model.relationships.push(r.clone()),
                            }
                            done = true;
                            self.rel_error = None;
                        }
                        Err(e) => self.rel_error = Some(rel_error(e).to_string()),
                    }
                }
                if ui.button(tr("unsaved.cancel")).clicked() {
                    done = true;
                    self.rel_error = None;
                }
            });
            self.edit = if done { None } else { Some((at, r)) };
        }
        if let Some(e) = &self.rel_error {
            ui.label(RichText::new(e).color(if self.edit.is_some() { ui.visuals().error_fg_color } else { ui.visuals().text_color() }));
        }
        self.excel_section(ui, model);
    }

    /// The Excel data model kept from the file, read only.
    fn excel_section(&mut self, ui: &mut egui::Ui, model: &mut DataModel) {
        let x = &self.excel;
        if !x.present && x.tables.is_empty() {
            return;
        }
        widgets::section(ui, tr("model.excel"));
        ui.label(RichText::new(tr("model.excel_note")).weak().small());
        if !x.tables.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr("model.excel_tables")).strong());
                ui.label(x.tables.join(", "));
            });
        }
        for r in &x.relationships {
            ui.label(format!("{}[{}]  >  {}[{}]", r.from_table, r.from_column, r.to_table, r.to_column));
        }
        if !x.measures.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr("model.excel_measures")).strong());
                ui.label(x.measures.join(", "));
            });
        }
        // Relationships between tables of this workbook can be taken over.
        let usable: Vec<Relationship> = x.relationships.iter().filter(|r| model::check_relationship(&self.data, &model.relationships, r, None).is_ok()).cloned().collect();
        if !usable.is_empty() && ui.button(trf("model.excel_import", &[("n", &usable.len())])).clicked() {
            model.relationships.extend(usable);
        }
    }

    fn load_measure(&mut self, model: &DataModel, i: usize) {
        let Some(m) = model.measures.get(i) else { return };
        self.current = Some(i);
        self.name = m.name.clone();
        self.table = m.table.clone();
        self.formula = m.formula.clone();
        self.format = m.format.clone();
        self.check = None;
    }

    /// The name a new measure gets: the first "Measure n" free.
    fn new_name(model: &DataModel) -> String {
        let n = (1..).find(|n| model.measure(&trf("model.measure_n", &[("n", n)])).is_none()).unwrap_or(1);
        trf("model.measure_n", &[("n", &n)])
    }

    /// A new measure, untouched, in the editor: New has nothing to do.
    fn is_fresh(&self, model: &DataModel) -> bool {
        self.current.is_none() && self.formula.trim().is_empty() && self.format.is_empty() && self.check.is_none() && self.name == Self::new_name(model)
    }

    fn new_measure(&mut self, model: &DataModel) {
        self.current = None;
        self.name = Self::new_name(model);
        if self.data.table(&self.table).is_none() {
            self.table = self.data.tables.first().map(|t| t.name.clone()).unwrap_or_default();
        }
        self.formula.clear();
        self.format.clear();
        self.check = None;
    }

    fn measures_page(&mut self, ui: &mut egui::Ui, model: &mut DataModel, wb: &apex_sheet::Workbook) {
        ui.horizontal_top(|ui| {
            // The list.
            ui.vertical(|ui| {
                ui.set_width(200.0);
                egui::ScrollArea::vertical().id_salt("model_measures").max_height(300.0).auto_shrink([false, false]).show(ui, |ui| {
                    if model.measures.is_empty() {
                        ui.label(RichText::new(tr("model.no_measures")).italics().weak());
                    }
                    let mut load = None;
                    for (i, m) in model.measures.iter().enumerate() {
                        let broken = model::eval::check_measure(&m.name, &m.formula, &self.data, &model.measures).is_err();
                        let text = if broken { RichText::new(format!("⚠ {}", m.name)).color(ui.visuals().error_fg_color) } else { RichText::new(format!("ƒx {}", m.name)) };
                        if ui.selectable_label(self.current == Some(i), text).on_hover_text(format!("{}\n= {}", m.table, m.formula)).clicked() {
                            load = Some(i);
                        }
                    }
                    if let Some(i) = load {
                        self.load_measure(model, i);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.is_fresh(model), egui::Button::new(tr("model.new"))).clicked() {
                        self.new_measure(model);
                    }
                    if ui.add_enabled(self.current.is_some(), egui::Button::new(tr("model.delete"))).clicked() {
                        if let Some(i) = self.current.filter(|i| *i < model.measures.len()) {
                            model.measures.remove(i);
                            self.new_measure(model);
                        }
                    }
                });
            });
            // (No vertical separator: it would grow the window every frame.)
            ui.add_space(12.0);
            // The editor.
            ui.vertical(|ui| {
                let tables = self.table_names();
                egui::Grid::new("model_measure_form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("model.measure_name"));
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(300.0));
                    ui.end_row();
                    ui.label(tr("model.measure_table"));
                    pick(ui, ("measure_table", 0), &mut self.table, &tables, 300.0);
                    ui.end_row();
                    ui.label(tr("model.measure_format"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.format).desired_width(200.0).font(egui::TextStyle::Monospace).hint_text("General"));
                        crate::widgets::menu_button(ui, "…", |ui| {
                            for f in FORMATS {
                                if ui.button(f).clicked() {
                                    self.format = if f == "General" { String::new() } else { f.to_string() };
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    ui.label(tr("model.formula"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.insert_menu(ui, model));
                });
                let r = ui.add(egui::TextEdit::multiline(&mut self.formula).code_editor().desired_rows(6).desired_width(f32::INFINITY).hint_text("SUM(Sales[Amount])"));
                // Enter adds a line here instead of closing the dialog.
                ui.data_mut(|dm| dm.insert_temp(r.id.with("multiline"), true));
                if r.changed() {
                    self.check = None;
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("model.check")).on_hover_text(tr("model.check_tip")).clicked() {
                        self.check = Some(match wb.measure_total(self.name.trim(), Some(&self.formula)) {
                            Ok(v) => (true, trf("model.check_ok", &[("v", &value_text(&v))])),
                            Err(e) => (false, dax_error(&e)),
                        });
                    }
                    let label = if self.current.is_some() { tr("model.save") } else { tr("model.add") };
                    if ui.button(RichText::new(label).strong()).clicked() {
                        self.save_measure(model);
                    }
                });
                if let Some((ok, t)) = &self.check {
                    ui.label(RichText::new(t).color(if *ok { theme::SHEET_ACCENT } else { ui.visuals().error_fg_color }));
                }
                ui.add_space(4.0);
                ui.label(RichText::new(tr("model.dax_help")).weak().small());
            });
        });
    }

    /// Menu to insert a column, measure or function at the end of the formula.
    fn insert_menu(&mut self, ui: &mut egui::Ui, model: &DataModel) {
        let mut add: Option<String> = None;
        crate::widgets::menu_button(ui, tr("model.insert"), |ui| {
            crate::widgets::menu_button(ui, tr("model.insert_function"), |ui| {
                for (name, ..) in dax::FUNCS {
                    if ui.button(name).clicked() {
                        add = Some(format!("{name}("));
                        ui.close();
                    }
                }
            });
            crate::widgets::menu_button(ui, tr("model.insert_column"), |ui| {
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for t in &self.data.tables {
                        ui.label(RichText::new(&t.name).strong());
                        for c in &t.columns {
                            if ui.button(format!("   {c}")).clicked() {
                                let plain = t.name.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
                                let tn = if plain { t.name.clone() } else { format!("'{}'", t.name.replace('\'', "''")) };
                                add = Some(format!("{tn}[{}]", c.replace(']', "]]")));
                                ui.close();
                            }
                        }
                    }
                });
            });
            ui.add_enabled_ui(!model.measures.is_empty(), |ui| {
                crate::widgets::menu_button(ui, tr("model.insert_measure"), |ui| {
                    for m in &model.measures {
                        if ui.button(&m.name).clicked() {
                            add = Some(format!("[{}]", m.name.replace(']', "]]")));
                            ui.close();
                        }
                    }
                });
            });
        });
        if let Some(a) = add {
            self.formula.push_str(&a);
            self.check = None;
        }
    }

    fn save_measure(&mut self, model: &mut DataModel) {
        let name = self.name.trim().to_string();
        if name.is_empty() || name.contains(['[', ']']) {
            self.check = Some((false, tr("model.err_name").to_string()));
            return;
        }
        if model.measures.iter().enumerate().any(|(i, m)| Some(i) != self.current && m.name.eq_ignore_ascii_case(&name)) {
            self.check = Some((false, tr("model.err_name_taken").to_string()));
            return;
        }
        let others: Vec<Measure> = model.measures.iter().enumerate().filter(|(i, _)| Some(*i) != self.current).map(|(_, m)| m.clone()).collect();
        if let Err(e) = model::eval::check_measure(&name, &self.formula, &self.data, &others) {
            self.check = Some((false, dax_error(&e)));
            return;
        }
        let formula = self.formula.trim().trim_start_matches('=').trim_start().to_string();
        let m = Measure { name: name.clone(), table: self.table.clone(), formula, format: self.format.trim().to_string() };
        match self.current {
            Some(i) if i < model.measures.len() => {
                let old = std::mem::replace(&mut model.measures[i], m);
                // Other measures follow a rename.
                if !old.name.eq_ignore_ascii_case(&name) {
                    let pat = format!("[{}]", old.name.replace(']', "]]"));
                    let to = format!("[{}]", name.replace(']', "]]"));
                    for x in &mut model.measures {
                        x.formula = replace_ci(&x.formula, &pat, &to);
                    }
                }
            }
            _ => {
                model.measures.push(m);
                self.current = Some(model.measures.len() - 1);
            }
        }
        self.check = Some((true, tr("model.saved").to_string()));
    }
}

/// `s` with every `pat` (ignoring case) replaced by `to`.
fn replace_ci(s: &str, pat: &str, to: &str) -> String {
    let (low, lp) = (s.to_lowercase(), pat.to_lowercase());
    if low.len() != s.len() {
        return s.replace(pat, to);
    }
    let mut out = String::new();
    let mut last = 0;
    for (i, _) in low.match_indices(&lp) {
        out.push_str(&s[last..i]);
        out.push_str(to);
        last = i + pat.len();
    }
    out.push_str(&s[last..]);
    out
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Empty => tr("model.blank").to_string(),
        Value::Number(n) => apex_sheet::format::general_number(*n, '.'),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Error(e) => e.as_str().to_string(),
        Value::Text(t) => format!("\"{t}\""),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// New starts another measure, and has nothing to do on one untouched.
    #[test]
    fn new_measure_only_when_editing() {
        let mut wb = apex_sheet::Workbook::empty();
        wb.sheets.push(apex_sheet::Sheet::new("Sheet1"));
        let mut v = SheetView::new(wb);
        v.open_data_model(1);
        let model = v.wb.analysis.model.clone();
        let d = v.model_dlg.as_mut().unwrap();
        assert!(d.is_fresh(&model));
        d.formula = "1+1".into();
        assert!(!d.is_fresh(&model));
        d.new_measure(&model);
        assert!(d.is_fresh(&model));
    }
}
