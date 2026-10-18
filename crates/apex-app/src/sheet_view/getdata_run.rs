//! Get Data work off the UI thread: previews for the query editor and full
//! runs for loading and refreshing. Files, the web and databases may take
//! their time; the UI polls for the results. Database passwords are kept
//! by the workbook's view while it is open, never in the file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use apex_query::sources::{book, db, html, pdf};
use apex_query::{Context, DataTable, Error};
use apex_sheet::Workbook;
use apex_sheet::query::{Query, Source};
use eframe::egui;

use crate::i18n::trf;

/// Rows a preview keeps.
pub(super) const PREVIEW_ROWS: usize = 200;

/// A context for running `queries` over `wb`, with the passwords typed
/// for this workbook (by kind, server, database and user: a password
/// typed for one workbook is never handed to another).
pub(super) fn context(wb: &Workbook, queries: Vec<Query>, dir: Option<PathBuf>, passwords: &HashMap<String, String>) -> Context {
    let mut ctx = Context::for_workbook(wb, queries, dir);
    ctx.passwords = passwords.clone();
    ctx
}

/// What the editor shows for a step.
pub(super) struct PreviewOk {
    /// Columns going into the selected step (for its settings).
    pub input: Vec<String>,
    /// The table after it (its first rows) and how many rows it has.
    pub table: DataTable,
    pub rows: usize,
    /// Columns of the query a merge step joins.
    pub other: Vec<String>,
}

pub(super) struct Preview {
    pub generation: u64,
    /// The source's table, when it was read again, and what the source
    /// offers to pick from (sheets, tables of a page or a database).
    pub source: Option<(Arc<DataTable>, Vec<String>)>,
    pub result: Result<PreviewOk, Error>,
}

/// The table of a source and its choices.
fn read_source(src: &Source, ctx: &Context) -> Result<(DataTable, Vec<String>), Error> {
    match src {
        Source::Workbook { path, item, header } => {
            let wb = book::open(&ctx.path(path))?;
            let items = book::items(&wb).into_iter().map(|(n, _)| n).collect();
            Ok((book::workbook_table(&wb, item, *header)?, items))
        }
        Source::Html { location, index } => {
            let data = apex_query::sources::read_bytes(location, ctx)?;
            let tables = html::tables(&apex_query::sources::decode(&data, "")?);
            let names = tables.iter().enumerate().map(|(i, t)| format!("{} · {}×{} · {}", i + 1, t.rows.len(), t.width(), t.columns.iter().take(4).cloned().collect::<Vec<_>>().join(", "))).collect();
            let n = tables.len();
            let t = tables.into_iter().nth(*index).ok_or_else(|| Error::Parse(format!("the page has {n} tables")))?;
            Ok((t, names))
        }
        Source::Pdf { location, index, header } => {
            let data = apex_query::sources::read_bytes(location, ctx)?;
            let tables = pdf::tables_with_pages(&data, *header)?;
            let names = tables
                .iter()
                .enumerate()
                .map(|(i, (page, t))| format!("{} · {} · {}×{} · {}", i + 1, trf("getdata.pdf_page", &[("n", &(page + 1))]), t.rows.len(), t.width(), t.columns.iter().take(4).cloned().collect::<Vec<_>>().join(", ")))
                .collect();
            let n = tables.len();
            let t = tables.into_iter().nth(*index).map(|(_, t)| t).ok_or_else(|| Error::Parse(format!("the PDF has {n} tables")))?;
            Ok((t, names))
        }
        Source::Sqlite { path, sql } => {
            let list = db::list_tables(&Source::Sqlite { path: ctx.path(path).to_string_lossy().into_owned(), sql: String::new() }, "")?;
            let t = if sql.trim().is_empty() { DataTable::default() } else { apex_query::load_source(src, ctx)? };
            Ok((t, list))
        }
        Source::DbFile { path, .. } => {
            let list = db::list_tables(&Source::DbFile { path: ctx.path(path).to_string_lossy().into_owned(), table: String::new() }, "")?;
            Ok((apex_query::load_source(src, ctx)?, list))
        }
        Source::Database { sql, .. } => {
            let key = apex_query::password_key(src).unwrap_or_default();
            let pw = ctx.passwords.get(&key).cloned().ok_or(Error::NeedsPassword(key))?;
            let list = db::list_tables(src, &pw)?;
            let t = if sql.trim().is_empty() { DataTable::default() } else { db::server(src, &pw)? };
            Ok((t, list))
        }
        _ => Ok((apex_query::load_source(src, ctx)?, Vec::new())),
    }
}

/// Previews step `upto` of `q` (0: the source) on a thread; `cached` is
/// the source's table from before, when the source didn't change.
pub(super) fn preview(egui_ctx: &egui::Context, generation: u64, q: Query, upto: usize, cached: Option<Arc<DataTable>>, ctx: Context) -> Receiver<Preview> {
    let (tx, rx) = mpsc::channel();
    let egui_ctx = egui_ctx.clone();
    let _ = apex_fs::thread::Builder::new().name("apex-getdata".into()).spawn(move || {
        let mut fresh = None;
        let src = match cached {
            Some(t) => Ok(t),
            None => read_source(&q.source, &ctx).map(|(t, choices)| {
                let t = Arc::new(t);
                fresh = Some((t.clone(), choices));
                t
            }),
        };
        let result = src.and_then(|src| {
            let input = if upto == 0 { (*src).clone() } else { apex_query::apply_steps(&q, (*src).clone(), upto - 1, &ctx)? };
            let cols = input.columns.clone();
            let out = if upto == 0 { input } else { apex_query::apply_steps(&q, (*src).clone(), upto, &ctx)? };
            let other = match upto.checked_sub(1).and_then(|i| q.steps.get(i)) {
                Some(apex_sheet::query::Step::Merge { query, .. }) => ctx.result_of(query).map(|t| t.columns).unwrap_or_default(),
                _ => Vec::new(),
            };
            Ok(PreviewOk { input: cols, rows: out.rows.len(), table: out.head(PREVIEW_ROWS), other })
        });
        let _ = tx.send(Preview { generation, source: fresh, result });
        egui_ctx.request_repaint();
    });
    rx
}

/// The result of a query run in full.
pub(super) struct Done {
    pub name: String,
    pub result: Result<DataTable, Error>,
}

/// Runs the queries named `names` (in that order) on a thread.
pub(super) fn run(egui_ctx: &egui::Context, names: Vec<String>, ctx: Context) -> Receiver<Vec<Done>> {
    let (tx, rx) = mpsc::channel();
    let egui_ctx = egui_ctx.clone();
    let _ = apex_fs::thread::Builder::new().name("apex-getdata".into()).spawn(move || {
        let out = names.into_iter().map(|name| Done { result: ctx.result_of(&name), name }).collect();
        let _ = tx.send(out);
        egui_ctx.request_repaint();
    });
    rx
}
