//! The database tab: a SQLite file (`.apexdb`, `.sqlite`, `.db`) with its
//! tables, saved queries, forms and reports, like Access. The navigation
//! pane at the left lists them; each one opens in an inner tab: the
//! datasheet ([`sheet`]), the table designer ([`design`]), the relations
//! diagram ([`relations`]), the query designer ([`query_ui`]), forms
//! ([`form_run`], [`form_design`]) and reports ([`report_ui`]).
//!
//! Changes go to the file at once, as in Access; undo takes back cell and
//! row edits one by one and other changes (designs, relations, action
//! queries, imports) from a copy of the database taken before them.

use std::path::{Path, PathBuf};

use apex_db::objects::Kind;
use apex_db::sqlite::Db;
use eframe::egui::{self, RichText};

use crate::app::Notice;
use crate::common::Shell;
use crate::i18n::{tr, trf};

pub mod design;
pub mod form_design;
pub mod form_run;
pub mod links;
mod nav;
pub mod query_ui;
pub mod relations;
pub mod report_ui;
mod toolstrip;
pub mod sheet;
pub mod transfer;
pub mod wizards;
#[cfg(test)]
mod tests;

/// Extensions of the files the database tab opens.
pub const EXTS: &[&str] = &["apexdb", "sqlite", "sqlite3", "db"];

/// Whether the database tab opens this file.
pub fn is_db_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The text of key `prefix.name` (names of kinds, actions, totals...).
pub fn trk(prefix: &str, name: &str) -> &'static str {
    crate::i18n::lookup(&format!("{prefix}.{name}")).unwrap_or("?")
}

/// Keys built by [`trk`], checked by the tests.
#[cfg(test)]
pub fn built_keys() -> Vec<String> {
    use apex_db::{forms, query, report, sqlite};
    let mut keys: Vec<String> = Vec::new();
    keys.extend(sqlite::FieldType::ALL.iter().map(|t| format!("db.type.{}", t.key())));
    keys.extend(report::Agg::ALL.iter().map(|a| format!("db.agg.{}", if *a == report::Agg::None { "none" } else { a.key() })));
    keys.extend(query::QueryType::ALL.iter().map(|t| format!("db.qtype.{}", t.key())));
    keys.extend(query::Total::ALL.iter().map(|t| format!("db.total.{}", t.key())));
    keys.extend(forms::FormKind::ALL.iter().map(|k| format!("db.fkind.{}", k.key())));
    keys.extend(forms::CtlKind::ALL.iter().map(|k| format!("db.ctl.{}", k.key())));
    keys.extend(forms::Action::ALL.iter().map(|a| format!("db.action.{}", a.key())));
    keys
}

/// Bigger databases aren't copied for undo (the change asks first).
const SNAPSHOT_LIMIT: u64 = 64 << 20;

/// A step of the undo history.
pub enum Step {
    Cell { table: String, rowid: i64, col: String, old: apex_db::Value, new: apex_db::Value },
    Insert { table: String, rowid: i64, cols: Vec<String>, vals: Vec<apex_db::Value> },
    Delete { table: String, rowid: i64, cols: Vec<String>, vals: Vec<apex_db::Value> },
    /// The whole database before (and, once undone, after) a change.
    Snapshot { name: &'static str, before: apex_db::sqlite::Snapshot, after: Option<apex_db::sqlite::Snapshot> },
}

impl Step {
    fn name(&self) -> &'static str {
        match self {
            Step::Cell { .. } => tr("db.undo.cell"),
            Step::Insert { .. } => tr("db.undo.insert"),
            Step::Delete { .. } => tr("db.undo.delete"),
            Step::Snapshot { name, .. } => name,
        }
    }
}

/// What the tab asks the app for.
pub enum Request {
    /// Shows a document in a tab of its own (a report or labels to print).
    OpenDoc(Box<apex_doc::Document>),
    /// Shows a workbook in a tab of its own (an export).
    OpenWorkbook(Box<apex_sheet::Workbook>),
    /// Opens another database file.
    OpenDb(PathBuf),
}

/// An inner tab.
pub enum Pane {
    Sheet(sheet::Datasheet),
    Design(design::Designer),
    Relations(relations::Diagram),
    Query(query_ui::QueryEditor),
    Form(form_run::FormRun),
    FormDesign(form_design::FormDesigner),
    Report(report_ui::ReportPane),
}

impl Pane {
    fn title(&self) -> String {
        match self {
            Pane::Sheet(s) => s.title(),
            Pane::Design(d) => format!("{} ({})", d.title(), tr("db.design")),
            Pane::Relations(_) => tr("db.relations").to_string(),
            Pane::Query(q) => q.title(),
            Pane::Form(f) => f.def.title.clone(),
            Pane::FormDesign(f) => format!("{} ({})", f.def.title, tr("db.design")),
            Pane::Report(r) => r.def.title.clone(),
        }
    }
}

/// A question before something that can't be taken back easily.
pub struct Confirm {
    pub text: String,
    pub action: ConfirmAction,
}

pub enum ConfirmAction {
    /// Runs an action query's SQL.
    RunAction(String),
    DeleteObject(Kind, String),
    DeleteTable(String),
    Relation(apex_db::Relation, bool),
}

pub struct DbView {
    pub db: Db,
    pub path: PathBuf,
    pub panes: Vec<Pane>,
    pub cur: usize,
    pub nav_filter: String,
    pub nav_open: bool,
    /// The object chosen in the navigation pane (`None` kind: a table).
    pub nav_sel: Option<(Option<Kind>, String)>,
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
    pub notices: Vec<Notice>,
    pub requests: Vec<Request>,
    pub zoom: f32,
    pub read_only: bool,
    pub confirm: Option<Confirm>,
    pub dialog: Option<transfer::Dialog>,
    pub wizard: Option<wizards::Wizard>,
    /// Passwords of linked servers for this session, by link name.
    pub passwords: std::collections::HashMap<String, String>,
    /// Asking a name for a new object.
    pub naming: Option<(Kind, String)>,
    /// Bumped on every change, so panes read their rows again.
    pub revision: u64,
    pub macros: apex_doc::Macros,
}

impl DbView {
    pub fn open(path: &Path) -> Result<DbView, apex_db::Error> {
        let db = Db::open(path)?;
        Ok(DbView::new(db, path))
    }

    pub fn new(db: Db, path: &Path) -> DbView {
        DbView {
            db,
            path: path.to_path_buf(),
            panes: Vec::new(),
            cur: 0,
            nav_filter: String::new(),
            nav_open: true,
            nav_sel: None,
            undo: Vec::new(),
            redo: Vec::new(),
            notices: Vec::new(),
            requests: Vec::new(),
            zoom: 1.0,
            read_only: false,
            confirm: None,
            dialog: None,
            wizard: None,
            passwords: Default::default(),
            naming: None,
            revision: 0,
            macros: Default::default(),
        }
    }

    /// Changes go to the file at once: nothing is ever left to save.
    pub fn is_modified(&self) -> bool {
        false
    }

    pub fn error(&mut self, e: impl std::fmt::Display) {
        self.notices.push(Notice::error(trf("db.error", &[("error", &e.to_string())])));
    }

    /// Notes a change: panes read again.
    pub fn changed(&mut self) {
        self.revision += 1;
    }

    // --- undo ----------------------------------------------------------------------------

    pub fn push_step(&mut self, s: Step) {
        self.undo.push(s);
        self.redo.clear();
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.changed();
    }

    /// Runs a change that can't be undone step by step: the database is
    /// copied first (when it isn't too big). Gives whether it went well.
    pub fn with_snapshot(&mut self, name: &'static str, f: impl FnOnce(&mut Db) -> Result<(), apex_db::Error>) -> bool {
        let before = if self.db.size() <= SNAPSHOT_LIMIT { self.db.snapshot().ok() } else { None };
        match f(&mut self.db) {
            Ok(()) => {
                match before {
                    Some(before) => self.push_step(Step::Snapshot { name, before, after: None }),
                    None => {
                        // Too big to copy: this one can't be undone.
                        self.undo.clear();
                        self.redo.clear();
                        self.changed();
                    }
                }
                true
            }
            Err(e) => {
                // What it did before failing (a table made for its notes,
                // a new name) is taken back.
                if let Some(before) = before {
                    let _ = self.db.restore(&before);
                }
                self.error(e);
                self.changed();
                false
            }
        }
    }

    fn apply(&mut self, s: Step, undo: bool) -> Result<Step, apex_db::Error> {
        Ok(match s {
            Step::Cell { table, rowid, col, old, new } => {
                self.db.update_cell(&table, rowid, &col, if undo { &old } else { &new })?;
                Step::Cell { table, rowid, col, old, new }
            }
            Step::Insert { table, rowid, cols, vals } => {
                if undo {
                    self.db.delete_row(&table, rowid)?;
                } else {
                    self.db.insert_row(&table, &cols, &vals, Some(rowid))?;
                }
                Step::Insert { table, rowid, cols, vals }
            }
            Step::Delete { table, rowid, cols, vals } => {
                if undo {
                    self.db.insert_row(&table, &cols, &vals, Some(rowid))?;
                } else {
                    self.db.delete_row(&table, rowid)?;
                }
                Step::Delete { table, rowid, cols, vals }
            }
            Step::Snapshot { name, before, after } => {
                if undo {
                    let now = self.db.snapshot()?;
                    self.db.restore(&before)?;
                    Step::Snapshot { name, before, after: Some(now) }
                } else {
                    if let Some(a) = &after {
                        self.db.restore(a)?;
                    }
                    Step::Snapshot { name, before, after: None }
                }
            }
        })
    }

    pub fn undo(&mut self) {
        let Some(s) = self.undo.pop() else { return };
        match self.apply(s, true) {
            Ok(s) => self.redo.push(s),
            Err(e) => self.error(e),
        }
        self.after_history();
    }

    pub fn redo(&mut self) {
        let Some(s) = self.redo.pop() else { return };
        match self.apply(s, false) {
            Ok(s) => self.undo.push(s),
            Err(e) => self.error(e),
        }
        self.after_history();
    }

    fn after_history(&mut self) {
        self.changed();
        // Designs may have changed under open designers.
        for p in &mut self.panes {
            if let Pane::Design(d) = p {
                d.reload(&self.db);
            }
        }
    }

    pub fn undo_names(&self) -> Vec<&'static str> {
        self.undo.iter().rev().map(Step::name).collect()
    }

    pub fn redo_names(&self) -> Vec<&'static str> {
        self.redo.iter().rev().map(Step::name).collect()
    }

    // --- panes ---------------------------------------------------------------------------

    /// Shows a pane (the one already open for the same object when there is one).
    pub fn show(&mut self, p: Pane) {
        let title = p.title();
        if let Some(i) = self.panes.iter().position(|x| x.title() == title && std::mem::discriminant(x) == std::mem::discriminant(&p)) {
            self.cur = i;
            return;
        }
        self.panes.push(p);
        self.cur = self.panes.len() - 1;
    }

    pub fn open_table(&mut self, name: &str) {
        self.show(Pane::Sheet(sheet::Datasheet::new(sheet::Src::Table(name.to_string()))));
    }

    pub fn open_query(&mut self, name: &str) {
        let recs = self.db.load_object(Kind::Query, name).map(|t| apex_db::objects::read(&t)).unwrap_or_default();
        let sql = recs.first().map(|r| r.get("sql").to_string()).unwrap_or_default();
        let ty = apex_db::query::QueryType::from_key(recs.first().map(|r| r.get("type")).unwrap_or(""));
        if ty.is_action() || sql.is_empty() || recs.first().is_some_and(|r| r.flag("untranslated")) {
            self.design_query(name);
            return;
        }
        self.show(Pane::Sheet(sheet::Datasheet::new(sheet::Src::Query(name.to_string(), sql))));
    }

    pub fn design_query(&mut self, name: &str) {
        let q = query_ui::QueryEditor::load(&self.db, name);
        self.show(Pane::Query(q));
    }

    pub fn design_table(&mut self, name: Option<&str>) {
        match designer_for(&self.db, name) {
            Ok(d) => self.show(Pane::Design(d)),
            Err(e) => self.error(e),
        }
    }

    pub fn open_form(&mut self, name: &str) {
        let def = self.load_form(name);
        self.show(Pane::Form(form_run::FormRun::new(name, def)));
    }

    pub fn load_form(&self, name: &str) -> apex_db::forms::FormDef {
        let recs = self.db.load_object(Kind::Form, name).map(|t| apex_db::objects::read(&t)).unwrap_or_default();
        let mut def = apex_db::forms::FormDef::from_records(&recs);
        if def.title.is_empty() {
            def.title = name.to_string();
        }
        def
    }

    pub fn design_form(&mut self, name: &str) {
        let def = self.load_form(name);
        self.show(Pane::FormDesign(form_design::FormDesigner::new(name, def)));
    }

    pub fn open_report(&mut self, name: &str) {
        let recs = self.db.load_object(Kind::Report, name).map(|t| apex_db::objects::read(&t)).unwrap_or_default();
        let mut def = apex_db::report::ReportDef::from_records(&recs);
        if def.title.is_empty() {
            def.title = name.to_string();
        }
        self.show(Pane::Report(report_ui::ReportPane::new(name, def)));
    }

    pub fn open_relations(&mut self) {
        self.show(Pane::Relations(relations::Diagram::load(&self.db)));
    }

    pub fn open_link(&mut self, name: &str) {
        self.show(Pane::Sheet(sheet::Datasheet::new(sheet::Src::Link(name.to_string()))));
    }

    /// Saves an object's records (a snapshot for undo).
    pub fn save_object(&mut self, kind: Kind, name: &str, recs: &[apex_db::objects::Record]) -> bool {
        let name = name.to_string();
        let recs = recs.to_vec();
        self.with_snapshot(tr("db.undo.save_object"), move |db| {
            if kind == Kind::Query {
                db.save_query(&name, &recs).map(|_| ())
            } else {
                db.save_object(kind, &name, &apex_db::objects::write(&recs))
            }
        })
    }

    pub fn delete_object(&mut self, kind: Kind, name: &str) {
        let n = name.to_string();
        self.with_snapshot(tr("db.undo.delete_object"), move |db| if kind == Kind::Query { db.delete_query(&n) } else { db.delete_object(kind, &n) });
        self.panes.retain(|p| p.title() != name);
        self.cur = self.cur.min(self.panes.len().saturating_sub(1));
    }

    // --- the view ------------------------------------------------------------------------

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        self.keys(ui);
        self.dialogs(ui.ctx());
        if self.nav_open {
            egui::Panel::left(egui::Id::new(("db_nav", self.path.to_string_lossy().to_string()))).resizable(true).default_size(210.0).min_size(140.0).show(ui, |ui| self.nav_ui(ui));
        }
        if self.panes.is_empty() {
            ui.centered_and_justified(|ui| ui.label(RichText::new(tr("db.empty_hint")).weak()));
            return;
        }
        self.cur = self.cur.min(self.panes.len() - 1);
        let mut close = None;
        ui.horizontal_wrapped(|ui| {
            for (i, p) in self.panes.iter().enumerate() {
                let r = ui.selectable_label(i == self.cur, p.title());
                if r.clicked() {
                    self.cur = i;
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                if i == self.cur && ui.small_button("×").on_hover_text(tr("db.close_pane")).clicked() {
                    close = Some(i);
                }
            }
        });
        ui.separator();
        if let Some(i) = close {
            self.panes.remove(i);
            self.cur = self.cur.min(self.panes.len().saturating_sub(1));
            return;
        }
        let cur = self.cur;
        // The pane is taken out while it draws, so it can use the view.
        let mut pane = std::mem::replace(&mut self.panes[cur], Pane::Relations(relations::Diagram::default()));
        match &mut pane {
            Pane::Sheet(s) => s.ui(ui, self),
            Pane::Design(d) => d.ui(ui, self),
            Pane::Relations(r) => r.ui(ui, self),
            Pane::Query(q) => q.ui(ui, self),
            Pane::Form(f) => f.ui(ui, self),
            Pane::FormDesign(f) => f.ui(ui, self),
            Pane::Report(r) => r.ui(ui, self, shell),
        }
        // A form's Close button removes its pane.
        let closed = matches!(&pane, Pane::Form(f) if f.close);
        if cur < self.panes.len() {
            if closed {
                self.panes.remove(cur);
                self.cur = self.cur.min(self.panes.len().saturating_sub(1));
            } else {
                self.panes[cur] = pane;
            }
        }
    }

    fn keys(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (z, y) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z), i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)));
        if z {
            self.undo();
        }
        if y {
            self.redo();
        }
    }

    pub fn status(&mut self, ui: &mut egui::Ui) {
        let tables = self.db.table_names().len();
        ui.label(trf("db.status", &[("tables", &tables), ("size", &format_size(self.db.size()))]));
        if let Some(Pane::Sheet(s)) = self.panes.get(self.cur) {
            ui.separator();
            ui.label(s.status());
        }
    }

    pub fn zoom_status(&mut self, ui: &mut egui::Ui) {
        crate::common::zoom_control(ui, &mut self.zoom, false);
    }

    /// The document printed for Print and Export as PDF: the open report,
    /// or the rows of the open datasheet.
    pub fn print_doc(&mut self) -> apex_doc::Document {
        let cur = self.cur;
        match self.panes.get(cur) {
            Some(Pane::Report(r)) => r.document(&self.db),
            Some(Pane::Sheet(s)) => s.document(self),
            Some(Pane::Form(f)) => f.document(&self.db),
            _ => {
                let mut d = apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new(self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())]);
                d.modified = false;
                d
            }
        }
    }

    /// Asks for a name for a new object of `kind` (tables are designed first).
    pub fn ask_name(&mut self, kind: Kind) {
        self.naming = Some((kind, String::new()));
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        self.confirm_ui(ctx);
        transfer::dialog_ui(self, ctx);
        wizards::wizard_ui(self, ctx);
        self.naming_ui(ctx);
    }

    fn naming_ui(&mut self, ctx: &egui::Context) {
        let Some((kind, name)) = &mut self.naming else { return };
        let kind = *kind;
        let mut done = None;
        let mut open = true;
        let win_id = egui::Id::new("db.new_name");
        crate::widgets::esc_closes(ctx, win_id, &mut open);
        egui::Window::new(tr("db.new_name")).id(win_id).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            let r = ui.text_edit_singleline(name);
            r.request_focus();
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                let named = !name.trim().is_empty();
                if (ui.add_enabled(named, egui::Button::new(tr("dialog.ok"))).clicked() || enter) && named {
                    done = Some(name.trim().to_string());
                }
            });
        });
        if !open {
            self.naming = None;
        }
        if let Some(n) = done {
            self.naming = None;
            self.create_named(kind, &n);
        }
    }

    /// Makes a new empty object named `n`.
    pub fn create_named(&mut self, kind: Kind, n: &str) {
        match kind {
            Kind::Query => {
                let q = query_ui::QueryEditor::new(n);
                self.show(Pane::Query(q));
            }
            Kind::Form => {
                let def = apex_db::forms::FormDef { title: n.to_string(), width: 480.0, height: 360.0, ..Default::default() };
                self.show(Pane::FormDesign(form_design::FormDesigner::new(n, def)));
            }
            Kind::Report => {
                let def = apex_db::report::ReportDef { title: n.to_string(), page_footer: "&P / &N".into(), grand_total: true, ..Default::default() };
                self.show(Pane::Report(report_ui::ReportPane::new(n, def)));
            }
            _ => {}
        }
    }

    fn confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.confirm else { return };
        let mut answer = None;
        egui::Modal::new(egui::Id::new("db_confirm")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(&c.text);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr("dialog.ok")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr("dialog.cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
            });
        });
        let Some(yes) = answer else { return };
        let c = self.confirm.take().unwrap();
        if yes {
            self.confirmed(c.action);
        }
    }

    pub fn confirmed(&mut self, a: ConfirmAction) {
        match a {
            ConfirmAction::RunAction(sql) => {
                let mut n = 0;
                if self.with_snapshot(tr("db.undo.action_query"), |db| db.run_action(&sql).map(|x| n = x)) {
                    self.notices.push(Notice::info(trf("db.rows_changed", &[("n", &n)])));
                }
            }
            ConfirmAction::DeleteObject(kind, name) => self.delete_object(kind, &name),
            ConfirmAction::DeleteTable(name) => {
                let n = name.clone();
                self.with_snapshot(tr("db.undo.delete_table"), move |db| db.drop_table(&n));
                self.panes.retain(|p| !matches!(p, Pane::Sheet(s) if s.src == sheet::Src::Table(name.clone())));
                self.cur = self.cur.min(self.panes.len().saturating_sub(1));
            }
            ConfirmAction::Relation(r, add) => {
                self.with_snapshot(tr("db.undo.relation"), move |db| if add { db.add_relation(&r) } else { db.remove_relation(&r) });
                for p in &mut self.panes {
                    if let Pane::Relations(d) = p {
                        d.reload(&self.db);
                    }
                }
            }
        }
    }

    /// Asks before running an action query, saying how many rows it changes.
    pub fn ask_run_action(&mut self, sql: &str) {
        match self.db.preview_action(sql) {
            Ok(n) => {
                let mut text = trf("db.confirm_action", &[("n", &n)]);
                if self.db.size() > SNAPSHOT_LIMIT {
                    text = format!("{text}\n{}", tr("db.no_undo"));
                }
                self.confirm = Some(Confirm { text, action: ConfirmAction::RunAction(sql.to_string()) });
            }
            Err(e) => self.error(e),
        }
    }

    /// Compacts the file.
    pub fn compact(&mut self) {
        let before = self.db.size();
        match self.db.compact() {
            Ok(()) => self.notices.push(Notice::info(trf("db.compacted", &[("before", &format_size(before)), ("after", &format_size(self.db.size()))]))),
            Err(e) => self.error(e),
        }
    }

    pub fn repair(&mut self) {
        match self.db.repair() {
            Ok(p) if p.is_empty() => self.notices.push(Notice::info(tr("db.repaired_ok"))),
            Ok(p) => self.notices.push(Notice::error(trf("db.repaired_problems", &[("problems", &p.join("; "))]))),
            Err(e) => self.error(e),
        }
        self.changed();
    }

    /// Writes a copy of the database (asking where).
    pub fn backup(&mut self) {
        let stem = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = self.path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "apexdb".into());
        let date = chrono_date();
        crate::file_dialog::FileDialog::new().set_title(tr("db.backup")).set_file_name(format!("{stem}_{date}.{ext}")).save_file(crate::file_dialog::in_view(|v: &mut DbView, p: std::path::PathBuf| match v.db.backup_to(&p) {
            Ok(()) => v.notices.push(Notice::info(trf("db.backup_done", &[("file", &p.display().to_string())]))),
            Err(e) => v.error(e),
        }));
    }
}

/// Today as YYYY-MM-DD (for backup names).
fn chrono_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as f64 + 25_569.0;
    apex_db::model::fmt_date(days).chars().take(10).collect()
}

pub fn format_size(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

/// A designer for table `name` (a new one when None).
fn designer_for(db: &Db, name: Option<&str>) -> Result<design::Designer, apex_db::Error> {
    Ok(match name {
        Some(n) => design::Designer::new(Some(n), db.design(n)?),
        None => design::Designer::new(None, design::Designer::blank(db)),
    })
}

/// Makes a new database file at `path` (replacing one there).
#[cfg_attr(target_os = "android", allow(dead_code))]
pub fn create(path: &Path) -> Result<DbView, apex_db::Error> {
    // Made new here (not through a link), then opened as any other.
    drop(apex_db::sqlite::Db::create(path)?);
    let mut v = DbView::open(path)?;
    // The file gets our tables, so other programs see a database.
    v.db.save_object(Kind::Layout, "relations", "")?;
    Ok(v)
}
