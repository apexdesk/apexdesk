//! Get Data: runs the queries of a workbook ([`apex_sheet::query`]). A
//! query reads a table from its source (a file, a folder, a web page or
//! API, a database, another query) and applies its steps to it in memory.
//!
//! Running may block on the network or a database: callers run it on a
//! thread of their own with a [`Context`] they own.

pub mod expr;
pub mod m;
pub mod sources;
pub mod steps;
pub mod table;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use apex_sheet::Workbook;
use apex_sheet::query::{Query, Source};

pub use table::{DataTable, Val};

/// Why a query could not run.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// A file or folder could not be read.
    Io(String),
    /// A web request failed.
    Network(String),
    /// The data is not in the expected format.
    Parse(String),
    /// The database refused the connection or the query.
    Database(String),
    /// The database needs a password (the key to remember it by).
    NeedsPassword(String),
    MissingColumn(String),
    DuplicateColumn(String),
    /// A calculated column's formula is wrong.
    Formula(String),
    /// No query with this name.
    NoQuery(String),
    /// No table (or sheet) with this name.
    NoTable(String),
    /// A query that ends up using itself.
    Cycle(String),
    Invalid(String),
    /// The source's client was left out of this build (the feature name).
    NotBuilt(&'static str),
    /// Outside data is turned off (by the organization's policy).
    Disabled,
}

static DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turns reading outside data (files, the web, databases) off or on; the
/// workbook's own tables and queries still work.
pub fn set_disabled(off: bool) {
    DISABLED.store(off, std::sync::atomic::Ordering::Relaxed);
}

pub fn disabled() -> bool {
    DISABLED.load(std::sync::atomic::Ordering::Relaxed)
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(s) | Error::Network(s) | Error::Parse(s) | Error::Database(s) | Error::Invalid(s) | Error::Formula(s) => f.write_str(s),
            Error::NeedsPassword(k) => write!(f, "a password is needed for {k}"),
            Error::MissingColumn(c) => write!(f, "no column \"{c}\""),
            Error::DuplicateColumn(c) => write!(f, "the column name \"{c}\" is empty or taken"),
            Error::NoQuery(q) => write!(f, "no query \"{q}\""),
            Error::NoTable(t) => write!(f, "no table or sheet \"{t}\""),
            Error::Cycle(q) => write!(f, "the query \"{q}\" uses itself"),
            Error::NotBuilt(x) => write!(f, "this build has no {x} support"),
            Error::Disabled => f.write_str("getting outside data is turned off"),
        }
    }
}

impl Error {
    /// The i18n key describing the error and the argument it takes (a
    /// name, or the technical detail).
    pub fn key(&self) -> (&'static str, String) {
        match self {
            Error::Io(s) => ("getdata.err.io", s.clone()),
            Error::Network(s) => ("getdata.err.network", s.clone()),
            Error::Parse(s) => ("getdata.err.parse", s.clone()),
            Error::Database(s) => ("getdata.err.database", s.clone()),
            Error::NeedsPassword(s) => ("getdata.err.password", s.clone()),
            Error::MissingColumn(s) => ("getdata.err.column", s.clone()),
            Error::DuplicateColumn(s) => ("getdata.err.column_taken", s.clone()),
            Error::Formula(s) => ("getdata.err.formula", s.clone()),
            Error::NoQuery(s) => ("getdata.err.query", s.clone()),
            Error::NoTable(s) => ("getdata.err.table", s.clone()),
            Error::Cycle(s) => ("getdata.err.cycle", s.clone()),
            Error::Invalid(s) => ("getdata.err.invalid", s.clone()),
            Error::NotBuilt(s) => ("getdata.err.not_built", s.to_string()),
            Error::Disabled => ("getdata.err.disabled", String::new()),

        }
    }
}

/// The key a database password is remembered by (None: no password).
/// Whether running `src` signs in with the Windows session (SQL Server,
/// no user given): the server, the file's to choose, gets to see it.
pub fn windows_login(src: &Source) -> bool {
    matches!(src, Source::Database { kind: apex_sheet::query::DbKind::SqlServer, user, .. } if sources::sqlserver::INTEGRATED && user.trim().is_empty())
}

pub fn password_key(src: &Source) -> Option<String> {
    match src {
        // A password typed for an encrypted connection isn't sent over a
        // plain one without asking again.
        // Fields escaped: a user or database holding `@`, `:` or `/` can't
        // make another server's key.
        Source::Database { kind, host, port, database, user, tls, .. } => {
            let e = |s: &str| s.replace('%', "%25").replace('@', "%40").replace(':', "%3A").replace('/', "%2F");
            Some(format!("{}{}://{}@{}:{port}/{}", kind.key(), if *tls { "+tls" } else { "" }, e(user), e(host), e(database)))
        }
        _ => None,
    }
}

/// Everything a run needs, owned so it can go to another thread.
#[derive(Default)]
pub struct Context {
    /// The workbook's queries (for appends, merges and query sources).
    pub queries: Vec<Query>,
    /// The folder of the workbook: relative paths start there.
    pub base_dir: Option<PathBuf>,
    /// Tables of the workbook, by lowercase name.
    pub tables: HashMap<String, DataTable>,
    /// Database passwords by [`password_key`].
    pub passwords: HashMap<String, String>,
    /// Results of other queries in this run, and the ones being run.
    done: RefCell<HashMap<String, DataTable>>,
    running: RefCell<Vec<String>>,
}

impl Context {
    pub fn new(queries: Vec<Query>, base_dir: Option<PathBuf>) -> Context {
        Context { queries, base_dir, ..Default::default() }
    }

    /// A context with the tables of `wb` that `queries` read.
    pub fn for_workbook(wb: &Workbook, queries: Vec<Query>, base_dir: Option<PathBuf>) -> Context {
        let mut ctx = Context::new(queries, base_dir);
        for q in &ctx.queries {
            if let Source::Table { name } = &q.source
                && let Ok(t) = sources::book::workbook_table(wb, name, true)
            {
                ctx.tables.insert(name.to_lowercase(), t);
            }
        }
        ctx
    }

    /// A path as written in a query: relative ones from the workbook's
    /// folder.
    pub fn path(&self, p: &str) -> PathBuf {
        let p = Path::new(p.trim());
        match &self.base_dir {
            Some(d) if p.is_relative() => d.join(p),
            _ => p.to_path_buf(),
        }
    }

    fn query(&self, name: &str) -> Result<&Query, Error> {
        self.queries.iter().find(|q| q.name.eq_ignore_ascii_case(name)).ok_or_else(|| Error::NoQuery(name.to_string()))
    }

    /// The result of the query named `name` (run once per context).
    pub fn result_of(&self, name: &str) -> Result<DataTable, Error> {
        let key = name.to_lowercase();
        if let Some(t) = self.done.borrow().get(&key) {
            return Ok(t.clone());
        }
        if self.running.borrow().contains(&key) {
            return Err(Error::Cycle(name.to_string()));
        }
        // Each query read by the one before: a long chain would run out of stack.
        if self.running.borrow().len() >= 64 {
            return Err(Error::Invalid("the queries read each other too deeply".into()));
        }
        let q = self.query(name)?.clone();
        self.running.borrow_mut().push(key.clone());
        let r = run(&q, self);
        self.running.borrow_mut().retain(|k| *k != key);
        let t = r?;
        self.done.borrow_mut().insert(key, t.clone());
        Ok(t)
    }
}

/// Reads the table a source gives.
pub fn load_source(src: &Source, ctx: &Context) -> Result<DataTable, Error> {
    sources::load(src, ctx)
}

/// Applies the first `upto` steps of `q` to its source's table `t`.
pub fn apply_steps(q: &Query, mut t: DataTable, upto: usize, ctx: &Context) -> Result<DataTable, Error> {
    let key = q.name.to_lowercase();
    let fresh = !ctx.running.borrow().contains(&key);
    if fresh {
        ctx.running.borrow_mut().push(key.clone());
    }
    let mut other = |name: &str| ctx.result_of(name);
    let mut out = Ok(());
    for s in q.steps.iter().take(upto) {
        match steps::apply(s, t, &mut other) {
            Ok(x) => t = x,
            Err(e) => {
                out = Err(e);
                t = DataTable::default();
                break;
            }
        }
    }
    if fresh {
        ctx.running.borrow_mut().retain(|k| *k != key);
    }
    out.map(|_| t)
}

/// Runs a query: its source, then all its steps.
pub fn run(q: &Query, ctx: &Context) -> Result<DataTable, Error> {
    let key = q.name.to_lowercase();
    let fresh = !ctx.running.borrow().contains(&key);
    if fresh {
        ctx.running.borrow_mut().push(key.clone());
    }
    let r = load_source(&q.source, ctx).and_then(|t| apply_steps(q, t, q.steps.len(), ctx));
    if fresh {
        ctx.running.borrow_mut().retain(|k| *k != key);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::query::Step;

    #[test]
    fn cycles_are_caught() {
        let mut a = Query::new("A", Source::Query { name: "B".into() });
        a.steps.push(Step::PromoteHeaders);
        let b = Query::new("B", Source::Query { name: "A".into() });
        let ctx = Context::new(vec![a.clone(), b], None);
        assert!(matches!(run(&a, &ctx), Err(Error::Cycle(_))));
        let c = Query::new("C", Source::Query { name: "Nope".into() });
        assert!(matches!(run(&c, &Context::new(vec![c.clone()], None)), Err(Error::NoQuery(_))));
    }
}
