//! Running a database's macros: Access's DoCmd, CurrentDb and Recordsets
//! on a connection of the macro's own to the file, so rows change in it
//! at once; what it asks of the forms on screen is done when it ends.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use web_time::Instant;

use apex_db::objects::Kind;
use apex_db::query::{QueryType, from_access};
use apex_db::sqlite::{Db, Filter, RowSource};
use apex_vba::access::{Database, Rows};
use apex_vba::value::Value;
use apex_vba::{DbOutcome, Options};
use eframe::egui;

use super::super::{App, Notice, View};
use super::{ChannelUi, Entry, Job, Msg, debug_ui, form_ui, parse, sources};
use crate::db_view::form_run::macro_value;
use crate::i18n::tr;

/// The database as a macro reaches it.
struct DbHost {
    db: Db,
    /// Opened read-only: rows can be read, not changed.
    read_only: bool,
}

impl DbHost {
    fn writable(&self) -> Result<(), String> {
        if self.read_only { Err("the database is open read-only".to_string()) } else { Ok(()) }
    }

    /// The table named `name` as it is spelled in the file.
    fn table(&self, name: &str) -> Option<String> {
        self.db.table_names().into_iter().find(|t| t.eq_ignore_ascii_case(name))
    }

    /// A saved query's SQL and type.
    fn query(&self, name: &str) -> Option<(String, QueryType)> {
        let recs = apex_db::objects::read(&self.db.load_object(Kind::Query, name)?);
        let r = recs.first()?;
        Some((r.get("sql").to_string(), QueryType::from_key(r.get("type"))))
    }

    fn select(&self, sql: &str) -> Result<Rows, String> {
        let page = self.db.select(sql, &[]).map_err(|e| e.to_string())?;
        Ok(Rows { columns: page.columns, rows: page.rows.iter().map(|r| r.iter().map(macro_value).collect()).collect(), table: None })
    }
}

/// A macro's value as the database keeps it.
fn db_value(v: &Value) -> apex_db::Value {
    use apex_db::Value as D;
    match v {
        Value::Empty | Value::Null | Value::Nothing | Value::Missing => D::Null,
        Value::Bool(b) => D::Bool(*b),
        Value::Int(i) => D::Int(*i as i64),
        Value::Long(i) => D::Int(*i as i64),
        Value::Double(n) => D::Num(*n),
        Value::Date(d) => D::Date(*d),
        Value::Str(s) => D::Text(s.to_string()),
        other => D::Text(other.to_str().unwrap_or_default()),
    }
}

fn split(values: &[(String, Value)]) -> (Vec<String>, Vec<apex_db::Value>) {
    values.iter().map(|(c, v)| (c.clone(), db_value(v))).unzip()
}

impl Database for DbHost {
    fn rows(&mut self, source: &str) -> Result<Rows, String> {
        let source = source.trim();
        if let Some(t) = self.table(source) {
            let page = self.db.page(&RowSource::Table(t.clone()), &Filter::default(), &[], 0, i64::MAX as usize).map_err(|e| e.to_string())?;
            let rows = page.rows.iter().map(|r| r.iter().map(macro_value).collect()).collect();
            let table = (page.rowids.len() == page.rows.len()).then_some((t, page.rowids));
            return Ok(Rows { columns: page.columns, rows, table });
        }
        if let Some((sql, ty)) = self.query(source) {
            if ty.is_action() {
                return Err(format!("{source}: an action query has no rows"));
            }
            return self.select(&sql);
        }
        let sql = from_access(source);
        // Only what reads: changes go through Execute.
        let reads = self.db.conn().prepare(&sql).map(|s| s.readonly()).map_err(|e| e.to_string())?;
        if !reads {
            return Err(format!("{source}: not a SELECT"));
        }
        self.select(&sql)
    }

    fn execute(&mut self, sql: &str) -> Result<usize, String> {
        self.writable()?;
        let sql = match self.query(sql.trim()) {
            Some((sql, _)) => sql,
            None => from_access(sql),
        };
        self.db.run_action(&sql).map_err(|e| e.to_string())
    }

    fn is_action_query(&mut self, name: &str) -> Option<bool> {
        self.query(name).map(|(_, ty)| ty.is_action())
    }

    fn update(&mut self, table: &str, rowid: i64, values: &[(String, Value)]) -> Result<(), String> {
        self.writable()?;
        for (c, v) in values {
            self.db.update_cell(table, rowid, c, &db_value(v)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn insert(&mut self, table: &str, values: &[(String, Value)]) -> Result<i64, String> {
        self.writable()?;
        let (cols, vals) = split(values);
        self.db.insert_row(table, &cols, &vals, None).map_err(|e| e.to_string())
    }

    fn delete(&mut self, table: &str, rowid: i64) -> Result<(), String> {
        self.writable()?;
        self.db.delete_row(table, rowid).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Runs `entry` on the database at `path` (in the macro's thread).
fn run_db(bin: &[u8], host: &mut DbHost, forms: Vec<apex_vba::access::FormState>, me: Option<String>, ui: &mut ChannelUi, opts: &Options, entry: Entry) -> DbOutcome {
    let none = |error| DbOutcome { error, changed: false, requests: Vec::new() };
    let project = match parse(bin) {
        Ok(p) => p,
        Err(e) => return none(Some(e)),
    };
    match entry {
        Entry::Macro(name) | Entry::Step(name) => apex_vba::run_db_macro(&project, host, forms, me, ui, opts, &name),
        Entry::Call(name, args) => apex_vba::call_db_macro(&project, host, forms, me, ui, opts, &name, &args),
        Entry::Immediate(line) => apex_vba::db_immediate(&project, host, forms, ui, opts, &line),
        // A database has no Open or sheet events here.
        Entry::Open | Entry::New | Entry::Events(_) => none(None),
    }
}

impl App {
    /// [`Self::start_macro`] for a database: its own connection to the
    /// file, the forms on screen as they are now.
    pub(super) fn start_db_macro(&mut self, ctx: &egui::Context, idx: usize, entry: Entry) {
        let id = self.tabs[idx].id;
        self.flush_macros(id);
        self.hold(idx);
        let file_name = self.tabs[idx].name();
        let downloaded = self.tabs[idx].downloaded();
        // The form whose button ran it is its Me.
        let me = self.macro_run.db_form.take().filter(|(t, m, _)| *t == id && matches!(&entry, Entry::Macro(n) if n == m)).map(|(_, _, f)| f);
        let View::Db(v) = &mut self.tabs[idx].view else { return };
        let Some(bin) = v.macros.project.clone() else { return };
        let path = v.path.clone();
        let file_path = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let read_only = v.read_only;
        let forms = v.macro_forms();
        let name = match &entry {
            Entry::Macro(n) | Entry::Step(n) | Entry::Call(n, _) => n.clone(),
            Entry::Immediate(_) => tr("debug.immediate").to_string(),
            Entry::Open | Entry::New | Entry::Events(_) => return,
        };
        v.macro_started();
        let cancel = Arc::new(AtomicBool::new(false));
        let interrupt = Arc::new(AtomicBool::new(false));
        let conf = self.macro_run.debug.entry(id).or_default();
        let debug = apex_vba::debug::Debugger { breakpoints: conf.breakpoints.clone(), watches: conf.watches.clone(), step: matches!(entry, Entry::Step(_)) };
        let opts = Options {
            file_name,
            file_path,
            user_name: crate::doc_view::user_name(),
            utc_offset: crate::common::local_utc_offset(),
            // The thread has 64 MiB of stack.
            max_stack: 48 << 20,
            cancel: Some(cancel.clone()),
            interrupt: Some(interrupt.clone()),
            debug: Some(debug),
            ..Options::default()
        };
        let mods = sources(&bin).map(|s| s.into_iter().map(|s| (s.name, s.code)).collect::<Vec<_>>()).unwrap_or_default();
        let names = debug_ui::casing(&mods);
        let (tx, rx) = mpsc::channel();
        let mut ui = ChannelUi { tx: tx.clone(), ctx: ctx.clone(), password: None, downloaded, trial: None };
        let spawned = apex_fs::thread::Builder::new().name("apex-macro".into()).stack_size(64 << 20).spawn(move || {
            let out = match Db::open(&path) {
                Ok(db) => run_db(&bin, &mut DbHost { db, read_only }, forms.into_iter().map(|f| f.state()).collect(), me, &mut ui, &opts, entry),
                Err(e) => DbOutcome { error: Some(apex_vba::Error::new(3051, e.to_string())), changed: false, requests: Vec::new() },
            };
            let _ = tx.send(Msg::DbDone(Box::new(out)));
            ui.ctx.request_repaint();
        });
        if spawned.is_err() {
            if let View::Db(v) = &mut self.tabs[idx].view {
                v.macro_ended(false);
            }
            self.notify(Notice::error(tr("macros.crashed")));
            return;
        }
        self.macro_run.failure = None;
        let now = Instant::now();
        self.macro_run.job = Some(Job { tab: id, name, rx, cancel, ask: None, input: String::new(), focus_input: false, dialog_open: false, status: None, started: now, interrupt, pause: None, panel: debug_ui::Panel::default(), resumed: now, original: None, sources: mods, grant: None, names, after: None, trial: None, forms: Vec::new(), form: form_ui::State::default() });
    }

    /// The macro ended: its changes one undo step, the panes read again,
    /// then what it asked of the forms.
    pub(super) fn finish_db_macro(&mut self, id: u64, out: DbOutcome) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.hold(idx);
        if let View::Db(v) = &mut self.tabs[idx].view {
            v.macro_ended(out.changed);
            v.changed();
            v.macro_requests(out.requests);
        }
        if let Some(e) = out.error {
            self.macro_run.failure = Some((id, e));
        }
    }
}

#[cfg(test)]
mod tests {
    use web_time::Duration;

    use super::*;

    /// A macro changes rows in the file, walks a recordset and opens a
    /// table; its changes are one undo step.
    #[test]
    fn a_databases_macro_changes_rows_and_opens_a_table() {
        let dir = apex_fs::temp_dir().join(format!("apex_dbmacro_{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let path = dir.join("People.apexdb");
        let _ = apex_fs::remove_file(&path);
        let db = Db::create(&path).unwrap();
        db.conn().execute_batch("CREATE TABLE People (Name TEXT, Age INTEGER); INSERT INTO People VALUES ('Ann', 30), ('Bob', 41);").unwrap();
        drop(db);
        let mut v = crate::db_view::DbView::open(&path).unwrap();
        let mut p = apex_io::vba::read(&apex_io::vba::new_presentation_project()).unwrap();
        let mut m = apex_io::vba::VbaModule::standard("Module1");
        m.code = "Sub Main()\n    Dim rs, n\n    CurrentDb.Execute \"UPDATE [People] SET Age = Age + 1\"\n    Set rs = CurrentDb.OpenRecordset(\"SELECT * FROM People WHERE Age > 40\")\n    Do Until rs.EOF\n        n = n + 1\n        rs.MoveNext\n    Loop\n    If n <> 1 Then Err.Raise 5\n    Set rs = CurrentDb.OpenRecordset(\"People\")\n    rs.AddNew\n    rs!Name = \"Cy\"\n    rs.Update\n    DoCmd.OpenTable \"People\"\nEnd Sub\n".into();
        p.modules.push(m);
        v.macros.project = Some(Arc::new(p.write().unwrap()));
        v.store_macros();
        let ctx = egui::Context::default();
        let settings = crate::settings::Settings { macros_enabled: true, ..Default::default() };
        let mut app = App::blank(&ctx, settings, None, Instant::now());
        app.push_tab(View::Db(Box::new(v)), Some(path.clone()), None);
        let id = app.tabs[0].id;
        app.enable_macros(id);
        app.start_macro(&ctx, id, Entry::Macro("Main".into()), None);
        let t = Instant::now();
        while app.macro_run.job.is_some() {
            assert!(t.elapsed() < Duration::from_secs(20), "the macro never ended");
            apex_fs::thread::sleep(Duration::from_millis(5));
            app.poll_macro();
        }
        assert!(app.macro_run.failure.is_none(), "{:?}", app.macro_run.failure);
        let View::Db(v) = &mut app.tabs[0].view else { unreachable!() };
        let ages = |v: &crate::db_view::DbView| v.db.select("SELECT Name, Age FROM People ORDER BY rowid", &[]).unwrap().rows;
        let rows = ages(v);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1][1], apex_db::Value::Int(42));
        assert!(matches!(v.panes.last(), Some(crate::db_view::Pane::Sheet(_))));
        // The project is kept in the file.
        assert!(Db::open(&path).unwrap().vba_project().is_some());
        v.undo();
        let rows = ages(v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1], apex_db::Value::Int(41));
        drop(app);
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
