//! A form in use: one record at a time with its controls where the
//! designer put them (single record), the records as a datasheet (list),
//! or a record with the related rows of another table under it (subform).
//! Navigation, search, new, delete and save; the database's rules
//! (required, validation rules, relations) refuse bad records, with the
//! field's validation text.

use apex_db::forms::{Action, CtlKind, FormDef, FormKind};
use apex_db::sqlite::{parse_input, q, Db, FieldType, Filter, RowSource};
use apex_db::Value;
use eframe::egui::{self, RichText, Rect, vec2};

use super::sheet::{cell_text, refusal, Datasheet, Src};
use super::{DbView, Step};
use crate::i18n::{tr, trf};

pub struct FormRun {
    pub name: String,
    pub def: FormDef,
    /// The record's place among the (found) records.
    pub pos: usize,
    pub total: usize,
    pub filter: Filter,
    pub rowid: Option<i64>,
    pub cols: Vec<String>,
    types: Vec<FieldType>,
    /// The record as read, and the texts being edited.
    pub orig: Vec<Value>,
    pub values: Vec<String>,
    /// A new record being filled.
    pub new: bool,
    pub error: Option<String>,
    loaded: Option<u64>,
    list: Option<Datasheet>,
    sub: Option<Datasheet>,
    /// Dropdowns' choices (value, shown), by control.
    choices: std::collections::HashMap<usize, Vec<(String, String)>>,
    /// A button asked to close the form.
    pub close: bool,
}

impl FormRun {
    pub fn new(name: &str, def: FormDef) -> FormRun {
        FormRun {
            name: name.to_string(),
            def,
            pos: 0,
            total: 0,
            filter: Filter::default(),
            rowid: None,
            cols: Vec::new(),
            types: Vec::new(),
            orig: Vec::new(),
            values: Vec::new(),
            new: false,
            error: None,
            loaded: None,
            list: None,
            sub: None,
            choices: Default::default(),
            close: false,
        }
    }

    fn src(&self) -> RowSource {
        RowSource::Table(self.def.source.clone())
    }

    /// The source is a table (a saved query's rows can't be changed here).
    fn table(&self, db: &Db) -> Option<String> {
        db.table_names().into_iter().find(|t| t.eq_ignore_ascii_case(&self.def.source))
    }

    /// Reads the current record when something changed.
    pub fn load(&mut self, v: &DbView) {
        if self.loaded == Some(v.revision) {
            return;
        }
        self.loaded = Some(v.revision);
        let src = self.src();
        let res = v.db.count(&src, &self.filter).and_then(|n| {
            self.total = n;
            self.pos = self.pos.min(n.saturating_sub(1));
            v.db.page(&src, &self.filter, &[], self.pos, 1)
        });
        match res {
            Ok(p) => {
                self.types = p.types.clone();
                if let Some(t) = self.table(&v.db) {
                    if let Ok(d) = v.db.design(&t) {
                        self.types = p.columns.iter().map(|c| d.field(c).map(|f| f.ty).unwrap_or_default()).collect();
                    }
                }
                self.cols = p.columns;
                self.rowid = p.rowids.first().copied();
                self.orig = p.rows.into_iter().next().unwrap_or_else(|| vec![Value::Null; self.cols.len()]);
                if self.total == 0 {
                    self.new = true;
                }
                if !self.new {
                    self.values = self.orig.iter().map(cell_text).collect();
                } else {
                    self.values.resize(self.cols.len(), String::new());
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.load_sub();
    }

    fn load_sub(&mut self) {
        if self.def.kind != FormKind::WithSubform || self.def.sub_source.is_empty() {
            return;
        }
        let master = self.col(&self.def.master_field).map(|i| self.orig[i].clone()).unwrap_or(Value::Null);
        let sub = self.sub.get_or_insert_with(|| Datasheet::new(Src::Table(self.def.sub_source.clone())));
        sub.filter.conditions = vec![(self.def.child_field.clone(), "=".into(), if self.new { Value::Null } else { master.clone() })];
        sub.offset = 0;
        sub.reload();
        // A new related row points to this record.
        if let Some(i) = sub.page.columns.iter().position(|c| c.eq_ignore_ascii_case(&self.def.child_field)) {
            if let Some(t) = sub.new_row.get_mut(i) {
                *t = cell_text(&master);
            }
        }
    }

    fn col(&self, field: &str) -> Option<usize> {
        self.cols.iter().position(|c| c.eq_ignore_ascii_case(field))
    }

    /// Whether the record's texts differ from what was read.
    pub fn dirty(&self) -> bool {
        if self.new {
            return self.values.iter().any(|t| !t.trim().is_empty());
        }
        self.values.iter().zip(&self.orig).any(|(t, o)| *t != cell_text(o))
    }

    /// Writes the record; gives whether it went well.
    pub fn save(&mut self, v: &mut DbView) -> bool {
        if !self.dirty() {
            return true;
        }
        let Some(table) = self.table(&v.db) else {
            self.error = Some(tr("db.read_only_source").to_string());
            return false;
        };
        let mut vals = Vec::new();
        for (i, t) in self.values.iter().enumerate() {
            let ty = self.types.get(i).copied().unwrap_or_default();
            match parse_input(t, ty) {
                Some(x) => vals.push(x),
                None => {
                    self.error = Some(trf("db.bad_value", &[("field", &self.cols[i])]));
                    return false;
                }
            }
        }
        if self.new {
            let (cols, vals): (Vec<String>, Vec<Value>) = self.cols.iter().cloned().zip(vals).filter(|(_, x)| *x != Value::Null).unzip();
            match v.db.insert_row(&table, &cols, &vals, None) {
                Ok(rowid) => {
                    let (all, row) = v.db.row(&table, rowid).unwrap_or((cols, vals));
                    v.push_step(Step::Insert { table, rowid, cols: all, vals: row });
                    self.new = false;
                    // The new record is the last.
                    self.pos = usize::MAX;
                }
                Err(e) => {
                    self.error = Some(refusal(v, Some(&table), &e));
                    return false;
                }
            }
        } else {
            let Some(rowid) = self.rowid else { return false };
            for (i, x) in vals.into_iter().enumerate() {
                if cell_text(&self.orig[i]) == self.values[i] {
                    continue;
                }
                match v.db.update_cell(&table, rowid, &self.cols[i], &x) {
                    Ok(old) => v.push_step(Step::Cell { table: table.clone(), rowid, col: self.cols[i].clone(), old, new: x }),
                    Err(e) => {
                        self.error = Some(refusal(v, Some(&table), &e));
                        return false;
                    }
                }
            }
        }
        self.error = None;
        self.loaded = None;
        true
    }

    /// Goes to record `pos` (saving the one shown first).
    pub fn go(&mut self, v: &mut DbView, pos: usize) {
        if !self.save(v) {
            return;
        }
        self.new = false;
        self.pos = pos.min(self.total.saturating_sub(1));
        self.loaded = None;
    }

    pub fn act(&mut self, v: &mut DbView, a: Action, target: &str) {
        match a {
            Action::First => self.go(v, 0),
            Action::Previous => self.go(v, self.pos.saturating_sub(1)),
            Action::Next => self.go(v, self.pos + 1),
            Action::Last => self.go(v, usize::MAX),
            Action::New => {
                if self.save(v) {
                    self.new = true;
                    self.values = vec![String::new(); self.cols.len()];
                    self.load_sub();
                }
            }
            Action::Delete => {
                if self.new {
                    self.new = false;
                    self.loaded = None;
                    return;
                }
                let (Some(table), Some(rowid)) = (self.table(&v.db), self.rowid) else { return };
                match v.db.delete_row(&table, rowid) {
                    Ok((cols, vals)) => {
                        v.push_step(Step::Delete { table, rowid, cols, vals });
                        self.error = None;
                    }
                    Err(e) => self.error = Some(refusal(v, Some(&table), &e)),
                }
                self.loaded = None;
            }
            Action::Save => {
                self.save(v);
            }
            Action::Find => {
                self.pos = 0;
                self.loaded = None;
            }
            Action::OpenReport => {
                if self.save(v) {
                    v.open_report(target);
                }
            }
            Action::OpenForm => {
                if self.save(v) {
                    v.open_form(target);
                }
            }
            Action::Close => {
                if self.save(v) {
                    self.close = true;
                }
            }
            // The macro sees the record as it is on screen (saved or not).
            Action::RunMacro => {
                if !target.trim().is_empty() {
                    v.run_macro = Some((target.trim().to_string(), self.name.clone()));
                }
            }
        }
    }

    /// The form as a macro sees it (Me).
    pub fn macro_state(&self) -> MacroForm {
        let fields = self
            .cols
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let text = self.values.get(i).cloned().unwrap_or_default();
                let orig = self.orig.get(i).filter(|o| !self.new && cell_text(o) == text);
                let v = match orig {
                    Some(o) => o.clone(),
                    None if text.is_empty() => Value::Null,
                    None => Value::Text(text),
                };
                (c.clone(), v)
            })
            .collect();
        let controls = self.def.controls.iter().filter(|c| !c.name.is_empty()).map(|c| (c.name.clone(), c.field.clone())).collect();
        MacroForm { name: self.name.clone(), caption: self.def.title.clone(), source: self.def.source.clone(), fields, controls, new_record: self.new, dirty: self.dirty(), filter: self.filter.sql.clone() }
    }

    /// Narrows the records to an Access condition (None: all of them).
    pub fn set_filter(&mut self, filter: Option<&str>) {
        self.filter.sql = filter.map(apex_db::query::from_access).unwrap_or_default();
        self.pos = 0;
        self.loaded = None;
    }

    /// A macro put `value` in `field` (`Me!Field = ...`).
    pub fn set_field(&mut self, field: &str, value: String) {
        if let Some(k) = self.col(field) {
            self.values[k] = value;
        }
    }

    pub fn go_to(&mut self, v: &mut DbView, to: apex_vba::access::GoTo) {
        use apex_vba::access::GoTo;
        match to {
            GoTo::First => self.act(v, Action::First, ""),
            GoTo::Previous => self.act(v, Action::Previous, ""),
            GoTo::Next => self.act(v, Action::Next, ""),
            GoTo::Last => self.act(v, Action::Last, ""),
            GoTo::New => self.act(v, Action::New, ""),
            GoTo::At(n) => self.go(v, n.saturating_sub(1)),
        }
    }

    pub fn requery(&mut self) {
        self.loaded = None;
        self.choices.clear();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        if self.def.kind == FormKind::List {
            ui.label(RichText::new(&self.def.title).heading());
            let list = self.list.get_or_insert_with(|| Datasheet::new(Src::Table(self.def.source.clone())));
            list.ui(ui, v);
            return;
        }
        self.load(v);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.def.title).heading());
            ui.separator();
            for (a, t, tip) in [(Action::First, "⏮", "db.first_record"), (Action::Previous, "◀", "db.prev_record")] {
                if ui.button(t).on_hover_text(tr(tip)).clicked() {
                    self.act(v, a, "");
                }
            }
            let n = if self.new { tr("db.new_record").to_string() } else { trf("db.record_of", &[("n", &(self.pos + 1)), ("total", &self.total)]) };
            ui.label(n);
            for (a, t, tip) in [(Action::Next, "▶", "db.next_record"), (Action::Last, "⏭", "db.last_record"), (Action::New, "✚", "db.new_record"), (Action::Delete, "🗑", "db.delete_record"), (Action::Save, "💾", "db.save_record")] {
                if ui.button(t).on_hover_text(tr(tip)).clicked() {
                    self.act(v, a, "");
                }
            }
            ui.separator();
            if ui.add(egui::TextEdit::singleline(&mut self.filter.search).hint_text(tr("db.search")).desired_width(150.0)).changed() {
                self.act(v, Action::Find, "");
            }
            if self.dirty() {
                ui.label(RichText::new("✎").strong()).on_hover_text(tr("db.record_changed"));
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.separator();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(self.def.width.max(100.0), self.def.height.max(60.0)), egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
            let mut action = None;
            for (i, c) in self.def.controls.clone().iter().enumerate() {
                let r = Rect::from_min_size(rect.min + vec2(c.x, c.y), vec2(c.w.max(10.0), c.h.max(10.0)));
                let col = self.col(&c.field);
                match c.kind {
                    CtlKind::Label => {
                        ui.put(r, egui::Label::new(&c.text).truncate());
                    }
                    CtlKind::TextBox => match col {
                        Some(k) => {
                            let multi = c.h > 30.0;
                            let te = if multi { egui::TextEdit::multiline(&mut self.values[k]) } else { egui::TextEdit::singleline(&mut self.values[k]) };
                            ui.put(r, te);
                        }
                        None => {
                            ui.put(r, egui::Label::new(RichText::new(format!("#{}", c.field)).weak()));
                        }
                    },
                    CtlKind::CheckBox => {
                        if let Some(k) = col {
                            let mut on = matches!(self.values[k].trim().to_ascii_lowercase().as_str(), "1" | "true" | "-1" | "yes" | "sí" | "si");
                            if ui.put(r, egui::Checkbox::without_text(&mut on)).changed() {
                                self.values[k] = if on { "1".into() } else { "0".into() };
                            }
                        }
                    }
                    CtlKind::ComboBox => {
                        if let Some(k) = col {
                            let choices = self.choices.entry(i).or_insert_with(|| choices(&v.db, &c.list_table, &c.list_value, &c.list_show)).clone();
                            let shown = choices.iter().find(|(val, _)| *val == self.values[k]).map(|x| x.1.clone()).unwrap_or_else(|| self.values[k].clone());
                            ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
                                egui::ComboBox::from_id_salt(("db_form_combo", i)).width(r.width()).selected_text(shown).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.values[k], String::new(), "—");
                                    for (val, show) in &choices {
                                        ui.selectable_value(&mut self.values[k], val.clone(), show);
                                    }
                                });
                            });
                        }
                    }
                    CtlKind::Button => {
                        if ui.put(r, egui::Button::new(&c.text)).clicked() {
                            action = Some((c.action, c.target.clone()));
                        }
                    }
                }
            }
            if let Some((a, t)) = action {
                self.act(v, a, &t);
            }
            if let Some(sub) = &mut self.sub {
                ui.add_space(8.0);
                ui.label(RichText::new(&self.def.sub_source).strong());
                ui.push_id("db_subform", |ui| {
                    ui.set_max_height(260.0);
                    sub.ui(ui, v);
                });
            }
        });
        self.keys(ui, v);
    }

    fn keys(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        let (up, down) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::PageUp), i.consume_key(egui::Modifiers::NONE, egui::Key::PageDown)));
        if up {
            self.act(v, Action::Previous, "");
        }
        if down {
            self.act(v, Action::Next, "");
        }
    }

    /// Printed: the source's rows.
    pub fn document(&self, db: &Db) -> apex_doc::Document {
        let page = db.page(&self.src(), &self.filter, &[], 0, 20_000).unwrap_or_default();
        let bound = self.def.bound_fields();
        let idx: Vec<usize> = if bound.is_empty() { (0..page.columns.len()).collect() } else { page.columns.iter().enumerate().filter(|(_, c)| bound.iter().any(|b| b.eq_ignore_ascii_case(c))).map(|(i, _)| i).collect() };
        let cols: Vec<String> = idx.iter().map(|&i| page.columns[i].clone()).collect();
        let rows: Vec<Vec<Value>> = page.rows.iter().map(|r| idx.iter().map(|&i| r[i].clone()).collect()).collect();
        super::report_ui::table_doc(&self.def.title, &cols, &rows)
    }
}

/// A value of the database as a macro's.
/// An open form as a macro will see it, sent to the macro's thread.
pub struct MacroForm {
    name: String,
    caption: String,
    source: String,
    fields: Vec<(String, Value)>,
    controls: Vec<(String, String)>,
    new_record: bool,
    dirty: bool,
    filter: String,
}

impl MacroForm {
    pub fn state(self) -> apex_vba::access::FormState {
        let fields = self.fields.iter().map(|(c, v)| (c.clone(), macro_value(v))).collect();
        apex_vba::access::FormState { name: self.name, caption: self.caption, source: self.source, fields, controls: self.controls, new_record: self.new_record, dirty: self.dirty, filter: self.filter }
    }
}

pub fn macro_value(v: &Value) -> apex_vba::value::Value {
    use apex_vba::value::Value as V;
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::Bool(*b),
        Value::Int(i) => i32::try_from(*i).map_or(V::Double(*i as f64), V::Long),
        Value::Num(n) => V::Double(*n),
        Value::Money(m) => V::Double(*m as f64 / 10000.0),
        Value::Decimal(d) => d.parse().map_or_else(|_| V::str(d.as_str()), V::Double),
        Value::Date(d) => V::Date(*d),
        Value::Text(s) | Value::Guid(s) => V::str(s.as_str()),
        other => V::str(cell_text(other)),
    }
}

/// A dropdown's choices: the stored value and the one shown.
pub fn choices(db: &Db, table: &str, value: &str, show: &str) -> Vec<(String, String)> {
    if table.is_empty() || value.is_empty() {
        return Vec::new();
    }
    let show = if show.is_empty() { value } else { show };
    let sql = format!("SELECT {}, {} FROM {} ORDER BY 2 LIMIT 5000", q(value), q(show), q(table));
    db.select(&sql, &[]).map(|p| p.rows.iter().map(|r| (cell_text(&r[0]), cell_text(&r[1]))).collect()).unwrap_or_default()
}
