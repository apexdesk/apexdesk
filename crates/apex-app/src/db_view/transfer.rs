//! Moving data in and out: importing sheets (xlsx, ods, csv...), Access and
//! dBase files and other SQLite databases as new tables; exporting a table
//! or a query's rows to a workbook, a CSV file or another database; and
//! linking tables of a database server.

use std::path::Path;

use apex_db::objects::Kind;
use apex_db::sqlite::{Filter, RowSource};
use apex_db::Database;
use apex_sheet::query::DbKind;
use eframe::egui;

use super::links::{self, Link};
use super::sheet::{cell_text, Src};
use super::{DbView, Pane, Request};
use crate::app::Notice;
use crate::i18n::{tr, trf};

/// Most rows exported to a workbook.
const MAX_EXPORT: usize = 1_000_000;

pub enum Dialog {
    Link(LinkDialog),
}

pub struct LinkDialog {
    pub name: String,
    pub link: Link,
    pub password: String,
    pub error: Option<String>,
    /// The server's answer to a test: its columns.
    pub tested: Option<Vec<String>>,
}

impl Dialog {
    pub fn link() -> Dialog {
        Dialog::Link(LinkDialog { name: String::new(), link: Link::default(), password: String::new(), error: None, tested: None })
    }
}

pub fn dialog_ui(v: &mut DbView, ctx: &egui::Context) {
    let Some(Dialog::Link(mut d)) = v.dialog.take() else { return };
    let mut open = true;
    let mut done = false;
    let win_id = egui::Id::new("db.link_table");
    crate::widgets::esc_closes(ctx, win_id, &mut open);
    egui::Window::new(tr("db.link_table")).id(win_id).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
        egui::Grid::new("db_link_grid").num_columns(2).show(ui, |ui| {
            ui.label(tr("db.link_kind"));
            let before = d.link.kind;
            egui::ComboBox::from_id_salt("db_link_kind").selected_text(kind_name(d.link.kind)).show_ui(ui, |ui| {
                for k in [DbKind::Postgres, DbKind::MySql, DbKind::SqlServer] {
                    ui.selectable_value(&mut d.link.kind, k, kind_name(k));
                }
            });
            if d.link.kind != before {
                d.link.port = d.link.kind.default_port();
            }
            ui.end_row();
            ui.label(tr("db.link_host"));
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut d.link.host);
                ui.add(egui::DragValue::new(&mut d.link.port));
            });
            ui.end_row();
            ui.label(tr("db.link_database"));
            ui.text_edit_singleline(&mut d.link.database);
            ui.end_row();
            ui.label(tr("db.link_user"));
            ui.text_edit_singleline(&mut d.link.user);
            ui.end_row();
            ui.label(tr("db.link_password"));
            ui.add(egui::TextEdit::singleline(&mut d.password).password(true));
            ui.end_row();
            ui.label(tr("db.link_server_table"));
            ui.text_edit_singleline(&mut d.link.table);
            ui.end_row();
            ui.label(tr("db.link_name"));
            ui.add(egui::TextEdit::singleline(&mut d.name).hint_text(d.link.table.as_str()));
            ui.end_row();
        });
        ui.checkbox(&mut d.link.tls, tr("db.link_tls"));
        if let Some(e) = &d.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if let Some(cols) = &d.tested {
            ui.label(trf("db.link_ok", &[("columns", &cols.join(", "))]));
        }
        ui.horizontal(|ui| {
            if ui.button(tr("db.link_test")).clicked() {
                match links::run(&d.link, &d.password, &d.link.page_sql(&[], 0, 1)) {
                    Ok(p) => {
                        d.tested = Some(p.columns);
                        d.error = None;
                    }
                    Err(e) => {
                        d.tested = None;
                        d.error = Some(e);
                    }
                }
            }
            if ui.add_enabled(!d.link.table.trim().is_empty(), egui::Button::new(tr("dialog.ok"))).clicked() {
                done = true;
            }
        });
    });
    if done {
        let name = if d.name.trim().is_empty() { d.link.table.trim().to_string() } else { d.name.trim().to_string() };
        if v.save_object(Kind::Link, &name, &d.link.to_records()) {
            v.passwords.insert(d.link.password_key(), d.password.clone());
            v.open_link(&name);
        }
        return;
    }
    if open {
        v.dialog = Some(Dialog::Link(d));
    }
}

fn kind_name(k: DbKind) -> &'static str {
    match k {
        DbKind::Postgres => "PostgreSQL",
        DbKind::MySql => "MySQL / MariaDB",
        DbKind::SqlServer => "SQL Server",
    }
}

// --- import --------------------------------------------------------------------------------

/// Asks for a file and imports it.
pub fn import(_: &mut DbView) {
    crate::file_dialog::FileDialog::new()
        .set_title(tr("db.import"))
        .add_filter(tr("db.importable"), &["xlsx", "xlsm", "xls", "ods", "csv", "tsv", "txt", "mdb", "accdb", "dbf", "apexdb", "sqlite", "sqlite3", "db"])
        .pick_file(crate::file_dialog::in_view(|v: &mut DbView, p: std::path::PathBuf| import_path(v, &p)));
}

/// Imports the tables of the file at `path` (one undo step).
pub fn import_path(v: &mut DbView, path: &Path) {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut made: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let ok = if super::EXTS.contains(&ext.as_str()) {
        let p = path.to_path_buf();
        v.with_snapshot(tr("db.undo.import"), |db| db.copy_tables_from(&p).map(|m| made = m))
    } else {
        let data = if apex_db::ACCESS_EXTS.contains(&ext.as_str()) || apex_db::DBASE_EXTS.contains(&ext.as_str()) {
            apex_db::open(path).map_err(|e| e.to_string())
        } else {
            match apex_io::open(path) {
                Ok((apex_io::Loaded::Sheet(wb), _)) => {
                    let mut data = apex_io::database::from_workbook(&wb);
                    // A text file's only sheet is named after the file.
                    if matches!(ext.as_str(), "csv" | "tsv" | "txt") && data.tables.len() == 1 {
                        if let Some(stem) = path.file_stem() {
                            data.tables[0].name = stem.to_string_lossy().into_owned();
                        }
                    }
                    Ok(data)
                }
                Ok(_) => Err(tr("db.not_a_table_file").to_string()),
                Err(e) => Err(e.to_string()),
            }
        };
        match data {
            Ok(mut data) => {
                rename_taken(&mut data, &v.db.table_names());
                made = data.tables.iter().map(|t| t.name.clone()).collect();
                v.with_snapshot(tr("db.undo.import"), |db| {
                    let rep = apex_db::import::import(&data, db)?;
                    warnings = rep.warnings;
                    warnings.extend(rep.untranslated.iter().map(|q| trf("db.untranslated_query", &[("name", q)])));
                    Ok(())
                })
            }
            Err(e) => {
                v.error(e);
                false
            }
        }
    };
    if ok {
        v.notices.push(Notice::info(trf("db.imported", &[("n", &made.len()), ("tables", &made.join(", "))])));
        for w in warnings.iter().take(5) {
            v.notices.push(Notice::info(w.clone()));
        }
        if let Some(t) = made.first() {
            v.open_table(t);
        }
    }
}

/// Renames the tables whose names are taken (and the relations to them).
pub fn rename_taken(data: &mut Database, taken: &[String]) {
    let mut used: Vec<String> = taken.to_vec();
    for i in 0..data.tables.len() {
        let old = data.tables[i].name.clone();
        let mut name = old.clone();
        let mut k = 2;
        while used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
            name = format!("{old}{k}");
            k += 1;
        }
        used.push(name.clone());
        if name != old {
            for r in &mut data.relations {
                if r.table == old {
                    r.table = name.clone();
                }
                if r.ref_table == old {
                    r.ref_table = name.clone();
                }
            }
            data.tables[i].name = name;
        }
    }
}

// --- export --------------------------------------------------------------------------------

/// The rows the export takes: the open datasheet's (a table or query).
pub fn current_source(v: &DbView) -> Option<(String, RowSource)> {
    match v.panes.get(v.cur)? {
        Pane::Sheet(s) => match &s.src {
            Src::Table(t) => Some((t.clone(), RowSource::Table(t.clone()))),
            Src::Query(n, sql) => Some((n.clone(), RowSource::Sql(sql.clone()))),
            Src::Link(_) => None,
        },
        Pane::Design(d) => d.original.clone().map(|t| (t.clone(), RowSource::Table(t))),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportTo {
    Workbook,
    Csv,
    Database,
}

pub fn export(v: &mut DbView, to: ExportTo) {
    let Some((name, src)) = current_source(v) else {
        v.notices.push(Notice::info(tr("db.export_nothing")));
        return;
    };
    match to {
        ExportTo::Workbook => match workbook(v, &name, &src) {
            Ok(wb) => v.requests.push(Request::OpenWorkbook(Box::new(wb))),
            Err(e) => v.error(e),
        },
        ExportTo::Csv => {
            crate::file_dialog::FileDialog::new().set_title(tr("db.export")).add_filter("CSV", &["csv"]).set_file_name(format!("{name}.csv")).save_file(crate::file_dialog::in_view(move |v: &mut DbView, p: std::path::PathBuf| match write_csv(v, &src, &p) {
                Ok(()) => v.notices.push(Notice::info(trf("db.exported", &[("file", &p.display().to_string())]))),
                Err(e) => v.error(e),
            }));
        }
        ExportTo::Database => {
            crate::file_dialog::FileDialog::new().set_title(tr("db.export")).add_filter(tr("db.filter_db"), super::EXTS).set_file_name("Export.apexdb").save_file(crate::file_dialog::in_view(move |v: &mut DbView, p: std::path::PathBuf| match v.db.export_rows(&src, &p, &name) {
                Ok(()) => {
                    v.notices.push(Notice::info(trf("db.exported", &[("file", &p.display().to_string())])));
                    v.requests.push(super::Request::OpenDb(p.canonicalize().unwrap_or(p)));
                }
                Err(e) => v.error(e),
            }));
        }
    }
}

fn col_type(t: apex_db::sqlite::FieldType) -> apex_db::ColType {
    use apex_db::sqlite::FieldType as F;
    match t {
        F::AutoNumber | F::Integer => apex_db::ColType::Int64,
        F::Number => apex_db::ColType::Double,
        F::Currency => apex_db::ColType::Money,
        F::DateTime => apex_db::ColType::DateTime,
        F::Bool => apex_db::ColType::Bool,
        F::Blob => apex_db::ColType::Binary,
        _ => apex_db::ColType::Memo,
    }
}

/// The rows as a workbook of one sheet (reusing Access files' opening).
pub fn workbook(v: &DbView, name: &str, src: &RowSource) -> Result<apex_sheet::Workbook, apex_db::Error> {
    let p = v.db.page(src, &Filter::default(), &[], 0, MAX_EXPORT)?;
    let cols = p.columns.iter().zip(&p.types).map(|(c, t)| apex_db::Column::new(c.as_str(), col_type(*t))).collect();
    let mut t = apex_db::Table::new(name, cols);
    t.rows = p.rows;
    Ok(apex_io::database::to_workbook(&Database { tables: vec![t], ..Default::default() }))
}

pub fn csv_field(s: &str) -> String {
    // Data from the database, which a spreadsheet must not run.
    let s = &apex_io::csv::guard_formula(s);
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

pub fn write_csv(v: &DbView, src: &RowSource, path: &Path) -> Result<(), apex_db::Error> {
    let p = v.db.page(src, &Filter::default(), &[], 0, usize::MAX >> 1)?;
    let mut out = p.columns.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(",");
    out.push_str("\r\n");
    for r in &p.rows {
        out.push_str(&r.iter().map(|x| csv_field(&cell_text(x))).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    crate::common::write_out(path, out).map_err(|e| apex_db::Error::Io(e.to_string()))
}
