//! The database tab's files: SQLite databases (`.sqlite`, `.db`, and
//! `.apexdb`, which is also SQLite).
//!
//! Tables are designed with [`TableDesign`] (fields with their type,
//! default, required, unique, validation rule and primary key; indexes;
//! foreign keys, which are the enforced relations). SQLite can't alter most
//! of that, so a changed design rebuilds the table: a new one is created,
//! the rows copied, the old one dropped and the new one renamed.
//!
//! Our own tables keep what SQLite has no place for: `apex_objects` the
//! saved queries, forms, reports, the relations diagram and linked server
//! tables (see [`crate::objects`]), `apex_fields` the validation messages
//! and descriptions of fields.

use std::path::{Path, PathBuf};

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;

/// The longest a statement from a file may write for (an action query
/// or an export): one that never ends would fill the disk.
const WRITE_SECONDS: u64 = 600;

/// What a read's rows hold in all, so many bytes at most.
const MAX_PAGE_BYTES: usize = 512 << 20;
use rusqlite::{params_from_iter, Connection};

/// A copy of a whole database in memory (see [`Db::snapshot`]).
pub type Snapshot = Connection;

use crate::model::{fmt_date, parse_date, Relation, Value};
use crate::objects::Kind;
use crate::Error;

pub const OBJECTS: &str = "apex_objects";
pub const FIELDS: &str = "apex_fields";

fn err(e: rusqlite::Error) -> Error {
    Error::Db(e.to_string())
}

/// An identifier in double quotes.
pub fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A text literal in single quotes.
pub fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The kinds of fields the designer offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FieldType {
    /// A whole number given by the database to each new row.
    AutoNumber,
    #[default]
    Text,
    LongText,
    Integer,
    Number,
    Currency,
    DateTime,
    Bool,
    /// Pictures, files and other bytes.
    Blob,
}

impl FieldType {
    pub const ALL: [FieldType; 9] =
        [FieldType::AutoNumber, FieldType::Text, FieldType::LongText, FieldType::Integer, FieldType::Number, FieldType::Currency, FieldType::DateTime, FieldType::Bool, FieldType::Blob];

    /// Its name for i18n keys (`db.type.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            FieldType::AutoNumber => "autonumber",
            FieldType::Text => "text",
            FieldType::LongText => "longtext",
            FieldType::Integer => "integer",
            FieldType::Number => "number",
            FieldType::Currency => "currency",
            FieldType::DateTime => "datetime",
            FieldType::Bool => "bool",
            FieldType::Blob => "blob",
        }
    }

    /// The type in SQL (`size`: characters of short text).
    pub fn decl(self, size: u32) -> String {
        match self {
            FieldType::AutoNumber | FieldType::Integer => "INTEGER".into(),
            FieldType::Text => format!("VARCHAR({})", if size == 0 { 255 } else { size }),
            FieldType::LongText => "TEXT".into(),
            FieldType::Number => "REAL".into(),
            FieldType::Currency => "DECIMAL(19,4)".into(),
            FieldType::DateTime => "DATETIME".into(),
            FieldType::Bool => "BOOLEAN".into(),
            FieldType::Blob => "BLOB".into(),
        }
    }

    /// The kind of a declared SQL type (and its size), following SQLite's
    /// affinity rules for types we didn't write.
    pub fn from_decl(decl: &str) -> (FieldType, u32) {
        let d = decl.to_ascii_uppercase();
        let size = d.split_once('(').and_then(|(_, r)| r.trim_end_matches(')').split(',').next()?.trim().parse().ok()).unwrap_or(0);
        let ty = if d.contains("BOOL") || d == "BIT" {
            FieldType::Bool
        } else if d.contains("DATE") || d.contains("TIME") {
            FieldType::DateTime
        } else if d.contains("INT") {
            FieldType::Integer
        } else if d.contains("CHAR") || d.contains("CLOB") || d.contains("TEXT") {
            if d.starts_with("VARCHAR") || d.starts_with("CHAR") || d.starts_with("NVARCHAR") || d.starts_with("NCHAR") { FieldType::Text } else { FieldType::LongText }
        } else if d.contains("BLOB") || d.contains("BINARY") || d.is_empty() {
            FieldType::Blob
        } else if d.contains("DEC") || d.contains("MONEY") || d.contains("CURRENCY") || d.contains("NUMERIC") {
            FieldType::Currency
        } else {
            FieldType::Number
        };
        (ty, size)
    }
}

/// A field of a table's design.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FieldDef {
    pub name: String,
    pub ty: FieldType,
    /// Characters of short text.
    pub size: u32,
    /// An SQL expression ("0", "'none'", "CURRENT_TIMESTAMP").
    pub default: String,
    pub required: bool,
    pub unique: bool,
    /// The validation rule: an SQL condition on the field, or its end
    /// when it starts with a comparison ("> 0", "BETWEEN 1 AND 5").
    pub check: String,
    /// What to say when the rule fails.
    pub validation_text: String,
    pub description: String,
    pub pk: bool,
    /// The field it was before a rename (the rows' values come from it);
    /// None for a new field.
    pub from: Option<String>,
}

impl FieldDef {
    pub fn new(name: &str, ty: FieldType) -> FieldDef {
        FieldDef { name: name.to_string(), ty, from: None, ..Default::default() }
    }

    /// The validation rule as a full SQL condition.
    pub fn check_sql(&self) -> String {
        rule_sql(&self.name, &self.check)
    }
}

/// A validation rule as an SQL condition: rules starting with a comparison
/// are about the field.
pub fn rule_sql(field: &str, rule: &str) -> String {
    let r = rule.trim();
    let upper = r.to_ascii_uppercase();
    let starts = ["<", ">", "=", "!=", "BETWEEN ", "LIKE ", "IN ", "IN(", "IS ", "NOT ", "GLOB "];
    if starts.iter().any(|s| upper.starts_with(s)) {
        format!("{} {r}", q(field))
    } else {
        r.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

/// What a relation does to the rows pointing to a changed or deleted row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RefAction {
    /// Refuse the change while rows point to it.
    #[default]
    Restrict,
    Cascade,
    SetNull,
}

impl RefAction {
    fn sql(self) -> &'static str {
        match self {
            RefAction::Restrict => "NO ACTION",
            RefAction::Cascade => "CASCADE",
            RefAction::SetNull => "SET NULL",
        }
    }

    fn parse(s: &str) -> RefAction {
        match s.to_ascii_uppercase().as_str() {
            "CASCADE" => RefAction::Cascade,
            "SET NULL" => RefAction::SetNull,
            _ => RefAction::Restrict,
        }
    }
}

/// An enforced relation, stored in the table whose rows point.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ForeignKey {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    pub on_update: RefAction,
    pub on_delete: RefAction,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TableDesign {
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub indexes: Vec<IndexDef>,
    pub foreign_keys: Vec<ForeignKey>,
}

impl TableDesign {
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// Its CREATE TABLE statement (under name `name`).
    pub fn create_sql(&self, name: &str) -> String {
        let pk: Vec<&FieldDef> = self.fields.iter().filter(|f| f.pk).collect();
        let auto_pk = pk.len() == 1 && pk[0].ty == FieldType::AutoNumber;
        let mut parts = Vec::new();
        for f in &self.fields {
            let mut s = format!("{} {}", q(&f.name), f.ty.decl(f.size));
            if auto_pk && f.pk {
                s.push_str(" PRIMARY KEY AUTOINCREMENT");
            }
            if f.required && !(auto_pk && f.pk) {
                s.push_str(" NOT NULL");
            }
            if !f.default.trim().is_empty() {
                s.push_str(&format!(" DEFAULT ({})", f.default.trim()));
            }
            if f.unique && !f.pk {
                s.push_str(" UNIQUE");
            }
            parts.push(s);
        }
        if !pk.is_empty() && !auto_pk {
            parts.push(format!("PRIMARY KEY ({})", pk.iter().map(|f| q(&f.name)).collect::<Vec<_>>().join(", ")));
        }
        for f in &self.fields {
            if !f.check.trim().is_empty() {
                parts.push(format!("CONSTRAINT {} CHECK ({})", q(&format!("ck_{}", f.name)), f.check_sql()));
            }
        }
        for fk in &self.foreign_keys {
            parts.push(format!(
                "FOREIGN KEY ({}) REFERENCES {} ({}) ON UPDATE {} ON DELETE {}",
                fk.columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", "),
                q(&fk.ref_table),
                fk.ref_columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", "),
                fk.on_update.sql(),
                fk.on_delete.sql()
            ));
        }
        format!("CREATE TABLE {} (\n  {}\n)", q(name), parts.join(",\n  "))
    }
}

/// Where the rows of a datasheet come from.
#[derive(Clone, Debug, PartialEq)]
pub enum RowSource {
    Table(String),
    /// A SELECT (a query's SQL).
    Sql(String),
}

impl RowSource {
    fn from_sql(&self, with_rowid: bool) -> String {
        match self {
            RowSource::Table(t) if with_rowid => format!("SELECT rowid AS \"__rowid\", * FROM {}", q(t)),
            RowSource::Table(t) => format!("SELECT * FROM {}", q(t)),
            RowSource::Sql(s) => format!("SELECT * FROM ({})", s.trim().trim_end_matches(';')),
        }
    }
}

/// How the rows of a datasheet are narrowed: text searched in every column
/// and conditions on columns (all must hold).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Filter {
    pub search: String,
    /// Column, operator ("=", "<>", "<", ">", "<=", ">=", "LIKE", "IS NULL",
    /// "IS NOT NULL") and value.
    pub conditions: Vec<(String, String, Value)>,
    /// A condition in SQL (a macro's filter), may be empty.
    pub sql: String,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.search.is_empty() && self.conditions.is_empty() && self.sql.trim().is_empty()
    }

    fn where_sql(&self, columns: &[String], params: &mut Vec<SqlValue>) -> String {
        let mut parts = Vec::new();
        if !self.search.is_empty() && !columns.is_empty() {
            params.push(SqlValue::Text(format!("%{}%", self.search)));
            let n = params.len();
            parts.push(format!("({})", columns.iter().map(|c| format!("CAST({} AS TEXT) LIKE ?{n}", q(c))).collect::<Vec<_>>().join(" OR ")));
        }
        for (c, op, v) in &self.conditions {
            let op = op.to_ascii_uppercase();
            if op == "IS NULL" || op == "IS NOT NULL" {
                parts.push(format!("{} {op}", q(c)));
                continue;
            }
            let op = if ["=", "<>", "<", ">", "<=", ">=", "LIKE"].contains(&op.as_str()) { op } else { "=".into() };
            params.push(to_sql(v));
            parts.push(format!("{} {op} ?{}", q(c), params.len()));
        }
        if !self.sql.trim().is_empty() {
            parts.push(format!("({})", self.sql));
        }
        if parts.is_empty() { String::new() } else { format!(" WHERE {}", parts.join(" AND ")) }
    }
}

/// Rows read from the database.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Page {
    pub columns: Vec<String>,
    /// The field kinds, from the declared types (Text when unknown).
    pub types: Vec<FieldType>,
    /// Each row's rowid (tables only).
    pub rowids: Vec<i64>,
    pub rows: Vec<Vec<Value>>,
}

/// A value for SQLite: dates as ISO text, booleans as 0 and 1.
pub fn to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        Value::Int(i) => SqlValue::Integer(*i),
        Value::Num(n) => SqlValue::Real(*n),
        Value::Money(m) => SqlValue::Real(*m as f64 / 10_000.0),
        Value::Decimal(s) => s.parse().map(SqlValue::Real).unwrap_or_else(|_| SqlValue::Text(s.clone())),
        Value::Date(d) => {
            let s = fmt_date(*d);
            SqlValue::Text(if s.len() == 16 { format!("{s}:00") } else { s })
        }
        Value::Blob(b) => SqlValue::Blob(b.clone()),
        Value::Attachments(a) => SqlValue::Text(a.iter().map(|x| x.name.clone()).collect::<Vec<_>>().join("; ")),
        v => SqlValue::Text(v.to_text()),
    }
}

/// A value from SQLite, typed by its column's declared kind.
pub fn from_sql(v: ValueRef, ty: FieldType) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => match ty {
            FieldType::Bool => Value::Bool(i != 0),
            FieldType::DateTime => Value::Date(i as f64),
            _ => Value::Int(i),
        },
        ValueRef::Real(r) => match ty {
            FieldType::DateTime => Value::Date(r),
            _ => Value::Num(r),
        },
        ValueRef::Text(b) => {
            let s = String::from_utf8_lossy(b).into_owned();
            match ty {
                FieldType::DateTime => parse_date(&s).map(Value::Date).unwrap_or(Value::Text(s)),
                FieldType::Bool if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("false") => Value::Bool(s.eq_ignore_ascii_case("true")),
                _ => Value::Text(s),
            }
        }
        ValueRef::Blob(b) => Value::Blob(b.to_vec()),
    }
}

/// Text typed in a cell as a value of a field of kind `ty` (None when it
/// isn't one).
pub fn parse_input(s: &str, ty: FieldType) -> Option<Value> {
    let t = s.trim();
    if t.is_empty() {
        return Some(Value::Null);
    }
    Some(match ty {
        FieldType::AutoNumber | FieldType::Integer => Value::Int(t.parse().ok()?),
        FieldType::Number | FieldType::Currency => Value::Num(t.replace(',', ".").parse().ok()?),
        FieldType::DateTime => Value::Date(parse_date(t).or_else(|| parse_local_date(t))?),
        FieldType::Bool => Value::Bool(match t.to_lowercase().as_str() {
            "1" | "true" | "yes" | "sí" | "si" | "x" | "-1" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => return None,
        }),
        FieldType::Blob => return None,
        FieldType::Text | FieldType::LongText => Value::Text(s.to_string()),
    })
}

/// "31/12/2024" (day first) and "12/31/2024" when the first can't be a day.
fn parse_local_date(s: &str) -> Option<f64> {
    let (date, time) = s.split_once(' ').unwrap_or((s, ""));
    let p: Vec<&str> = date.split(['/', '.', '-']).collect();
    if p.len() != 3 {
        return None;
    }
    let (a, b, y): (u32, u32, i64) = (p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?);
    let y = if y < 100 { 2000 + y } else { y };
    let (d, m) = if a > 12 || b <= 12 { (a, b) } else { (b, a) };
    parse_date(&format!("{y:04}-{m:02}-{d:02} {time}"))
}

/// Settings every connection to a database file gets: see [`Db::setup`].
pub fn harden(conn: &Connection) -> Result<(), Error> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true).map_err(err)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false).map_err(err)?;
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0).map_err(err)?;
    // A view may make a huge value (zeroblob, printf): 64 MB at most.
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, 64 << 20).map_err(err)?;
    Ok(())
}

/// The authorizer for SQL a file brings: plain reads and writes of its
/// tables, never ATTACH, PRAGMA, transactions or virtual tables.
fn document_sql(c: AuthContext<'_>) -> Authorization {
    match c.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Pragma { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::CreateVtable { .. }
        | AuthAction::DropVtable { .. } => Authorization::Deny,
        AuthAction::Function { function_name } if function_name.eq_ignore_ascii_case("load_extension") => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

/// An open database file.
pub struct Db {
    conn: Connection,
    pub path: PathBuf,
}

impl Db {
    /// Opens (or creates) the database at `path`.
    pub fn open(path: &Path) -> Result<Db, Error> {
        check_journal(path)?;
        let conn = Connection::open(path).map_err(err)?;
        let db = Db { conn, path: path.to_path_buf() };
        db.setup()?;
        Ok(db)
    }

    /// A new database at `path`, replacing the file there. A link there
    /// is removed, not written through (one someone else left in a shared
    /// folder, or brought in an archive, may point at any file of ours).
    pub fn create(path: &Path) -> Result<Db, Error> {
        clear(path).map_err(|e| crate::io_err(path, e))?;
        Self::open_no_follow(path)
    }

    /// Opens (or creates) the database at `path`, but not through a link.
    pub fn open_no_follow(path: &Path) -> Result<Db, Error> {
        use rusqlite::OpenFlags as F;
        let flags = F::SQLITE_OPEN_READ_WRITE | F::SQLITE_OPEN_CREATE | F::SQLITE_OPEN_NOFOLLOW | F::SQLITE_OPEN_URI | F::SQLITE_OPEN_NO_MUTEX;
        check_journal(path)?;
        let conn = Connection::open_with_flags(path, flags).map_err(err)?;
        let db = Db { conn, path: path.to_path_buf() };
        db.setup()?;
        Ok(db)
    }

    /// A database in memory (tests, previews).
    pub fn in_memory() -> Result<Db, Error> {
        let db = Db { conn: Connection::open_in_memory().map_err(err)?, path: PathBuf::new() };
        db.setup()?;
        Ok(db)
    }

    fn setup(&self) -> Result<(), Error> {
        // A database file and the SQL saved in it are the file's author's:
        // no SQL of theirs may reach other files (ATTACH, VACUUM INTO),
        // change SQLite's own records or run functions from the schema.
        harden(&self.conn)?;
        // A file that isn't SQLite fails here, before we write anything.
        self.conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).map_err(err)?;
        self.conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(err)?;
        // Our own tables are written by us only: a trigger on them would
        // run the file's SQL whenever a query or form is saved, a computed
        // column make values of any size from a small file.
        let ours: i64 = self
            .conn
            .query_row(
                &format!(
                    "SELECT (SELECT count(*) FROM sqlite_master WHERE lower(tbl_name) IN ('{OBJECTS}', '{FIELDS}') AND type IN ('trigger', 'view'))
                          + (SELECT count(*) FROM sqlite_master m, pragma_table_xinfo(m.name) x WHERE m.type = 'table' AND lower(m.name) IN ('{OBJECTS}', '{FIELDS}') AND x.hidden IN (2, 3))"
                ),
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if ours > 0 {
            return Err(Error::Db("the database has triggers on ApexDesk's own tables".into()));
        }
        Ok(())
    }

    /// Runs `f` with ATTACH allowed: only our own statements (VACUUM,
    /// copying tables between files) need it.
    fn with_attach<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 2).map_err(err)?;
        let r = f();
        let _ = self.conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0);
        r
    }

    /// Runs `f` with the checks for SQL that comes from a file (a saved
    /// query, an imported design): a single statement each, and nothing
    /// that attaches files, changes settings or ends the transaction.
    fn guarded<T>(&self, f: impl FnOnce(&Connection) -> Result<T, Error>) -> Result<T, Error> {
        self.conn.authorizer(Some(document_sql)).map_err(err)?;
        let r = f(&self.conn);
        let _ = self.conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        r
    }

    /// Our tables, made when something is first stored in them (a file only
    /// read stays unchanged).
    fn own_tables(&self) -> Result<(), Error> {
        self.conn
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {OBJECTS} (kind TEXT NOT NULL, name TEXT NOT NULL, data TEXT NOT NULL DEFAULT '', PRIMARY KEY (kind, name));
                 CREATE TABLE IF NOT EXISTS {FIELDS} (tbl TEXT NOT NULL, field TEXT NOT NULL, validation_text TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '', PRIMARY KEY (tbl, field));"
            ))
            .map_err(err)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    fn has_table(&self, name: &str) -> bool {
        self.conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 COLLATE NOCASE", [name], |_| Ok(())).is_ok()
    }

    /// The user's tables (not SQLite's or ours), by name.
    pub fn table_names(&self) -> Vec<String> {
        self.names("table")
    }

    /// The views of the file (queries saved by other programs).
    pub fn view_names(&self) -> Vec<String> {
        self.names("view")
    }

    fn names(&self, kind: &str) -> Vec<String> {
        let Ok(mut st) = self.conn.prepare("SELECT name FROM sqlite_master WHERE type = ?1 AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'apex\\_%' ESCAPE '\\' ORDER BY name COLLATE NOCASE") else { return Vec::new() };
        st.query_map([kind], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// The design of table `table`.
    pub fn design(&self, table: &str) -> Result<TableDesign, Error> {
        if !self.has_table(table) {
            return Err(Error::NoTable(table.to_string()));
        }
        let sql: String = self.conn.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1 COLLATE NOCASE", [table], |r| r.get(0)).map_err(err)?;
        let auto = sql.to_ascii_uppercase().contains("AUTOINCREMENT");
        let mut fields = Vec::new();
        {
            let mut st = self.conn.prepare(&format!("PRAGMA table_info({})", q(table))).map_err(err)?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, i64>(5)?)))
                .map_err(err)?;
            for row in rows {
                let (name, decl, notnull, dflt, pk) = row.map_err(err)?;
                let (mut ty, size) = FieldType::from_decl(&decl);
                if pk > 0 && ty == FieldType::Integer && (auto || decl.eq_ignore_ascii_case("INTEGER")) {
                    ty = FieldType::AutoNumber;
                }
                let mut f = FieldDef::new(&name, ty);
                f.size = size;
                f.required = notnull != 0;
                f.default = dflt.map(|d| d.trim_start_matches('(').trim_end_matches(')').to_string()).unwrap_or_default();
                f.pk = pk > 0;
                f.from = Some(name);
                fields.push(f);
            }
        }
        // AUTOINCREMENT only goes with a single INTEGER key.
        if fields.iter().filter(|f| f.pk).count() > 1 {
            for f in fields.iter_mut().filter(|f| f.ty == FieldType::AutoNumber) {
                f.ty = FieldType::Integer;
            }
        }
        for (name, check) in checks(&sql) {
            if let Some(f) = fields.iter_mut().find(|f| f.name == name) {
                // Rules about the field are shown the short way.
                let prefix = format!("{} ", q(&f.name));
                f.check = check.strip_prefix(&prefix).map(str::to_string).unwrap_or(check);
            }
        }
        if self.has_table(FIELDS) {
            let mut st = self.conn.prepare(&format!("SELECT field, validation_text, description FROM {FIELDS} WHERE tbl = ?1")).map_err(err)?;
            let rows = st.query_map([table], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).map_err(err)?;
            for (field, vt, desc) in rows.flatten() {
                if let Some(f) = fields.iter_mut().find(|f| f.name == field) {
                    f.validation_text = vt;
                    f.description = desc;
                }
            }
        }
        let mut indexes = Vec::new();
        {
            let mut st = self.conn.prepare(&format!("PRAGMA index_list({})", q(table))).map_err(err)?;
            let list: Vec<(String, bool, String)> =
                st.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0, r.get::<_, String>(3)?))).map_err(err)?.flatten().collect();
            for (name, unique, origin) in list {
                let mut st = self.conn.prepare(&format!("PRAGMA index_info({})", q(&name))).map_err(err)?;
                let cols: Vec<String> = st.query_map([], |r| r.get::<_, Option<String>>(2)).map_err(err)?.flatten().flatten().collect();
                match origin.as_str() {
                    // A field's UNIQUE.
                    "u" if cols.len() == 1 => {
                        if let Some(f) = fields.iter_mut().find(|f| f.name == cols[0]) {
                            f.unique = true;
                        }
                    }
                    "c" => indexes.push(IndexDef { name, columns: cols, unique }),
                    _ => {}
                }
            }
        }
        let mut foreign_keys: Vec<ForeignKey> = Vec::new();
        {
            let mut st = self.conn.prepare(&format!("PRAGMA foreign_key_list({})", q(table))).map_err(err)?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?)))
                .map_err(err)?;
            let mut last = -1;
            for row in rows {
                let (id, rt, from, to, upd, del) = row.map_err(err)?;
                if id != last {
                    foreign_keys.push(ForeignKey { ref_table: rt, on_update: RefAction::parse(&upd), on_delete: RefAction::parse(&del), ..Default::default() });
                    last = id;
                }
                let fk = foreign_keys.last_mut().unwrap();
                fk.columns.push(from);
                fk.ref_columns.push(to.unwrap_or_default());
            }
        }
        // SQLite lists them from the last one.
        foreign_keys.reverse();
        // A key given without its columns means the other table's key.
        for fk in &mut foreign_keys {
            if fk.ref_columns.iter().any(String::is_empty) {
                // Its key read directly: a table's design naming itself (or
                // two naming each other) must not be read for ever.
                if let Ok(mut st) = self.conn.prepare("SELECT name FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk") {
                    if let Ok(rows) = st.query_map([&fk.ref_table], |r| r.get::<_, String>(0)) {
                        fk.ref_columns = rows.flatten().collect();
                    }
                }
            }
        }
        Ok(TableDesign { name: table.to_string(), fields, indexes, foreign_keys })
    }

    /// Creates table `d` (when `old` is None) or changes table `old` to be
    /// `d`: rows keep the values of the fields that stay (by
    /// [`FieldDef::from`]). Everything happens or nothing does.
    pub fn apply_design(&mut self, old: Option<&str>, d: &TableDesign) -> Result<(), Error> {
        if d.name.trim().is_empty() || d.fields.is_empty() {
            return Err(Error::Db("a table needs a name and a field".into()));
        }
        self.own_tables()?;
        // A new name first, so other tables' relations follow it.
        let old = match old {
            Some(o) if o != d.name => {
                self.rename_table(o, &d.name)?;
                Some(d.name.as_str())
            }
            o => o,
        };
        // The table is renamed back without SQLite checking the other
        // tables' references (they may name tables still to be made).
        let fk: i64 = self.conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).map_err(err)?;
        self.conn.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON").map_err(err)?;
        let r = self.rebuild(old, d, fk != 0);
        self.conn.execute_batch(&format!("PRAGMA foreign_keys = {fk}; PRAGMA legacy_alter_table = OFF")).map_err(err)?;
        r
    }

    fn rebuild(&mut self, old: Option<&str>, d: &TableDesign, check_keys: bool) -> Result<(), Error> {
        let tx = self.conn.transaction().map_err(err)?;
        match old {
            None => {
                tx.execute(&d.create_sql(&d.name), []).map_err(err)?;
            }
            Some(old) => {
                let tmp = format!("apex_new_{}", d.name);
                tx.execute(&d.create_sql(&tmp), []).map_err(err)?;
                let kept: Vec<&FieldDef> = d.fields.iter().filter(|f| f.from.is_some()).collect();
                if !kept.is_empty() {
                    let to = kept.iter().map(|f| q(&f.name)).collect::<Vec<_>>().join(", ");
                    let from = kept.iter().map(|f| q(f.from.as_deref().unwrap())).collect::<Vec<_>>().join(", ");
                    tx.execute_batch(&format!("INSERT INTO {} ({to}) SELECT {from} FROM {}", q(&tmp), q(old))).map_err(err)?;
                }
                // Other tables' relations name this table, which keeps its
                // name (a new name was given before, see apply_design).
                tx.execute_batch(&format!("DROP TABLE {}; ALTER TABLE {} RENAME TO {};", q(old), q(&tmp), q(&d.name))).map_err(err)?;
                if !old.eq_ignore_ascii_case(&d.name) {
                    tx.execute(&format!("UPDATE {FIELDS} SET tbl = ?1 WHERE tbl = ?2"), [&d.name, old]).map_err(err)?;
                }
            }
        }
        for i in &d.indexes {
            tx.execute(&index_sql(&d.name, i), []).map_err(err)?;
        }
        tx.execute(&format!("DELETE FROM {FIELDS} WHERE tbl = ?1"), [&d.name]).map_err(err)?;
        for f in &d.fields {
            if !f.validation_text.is_empty() || !f.description.is_empty() {
                tx.execute(&format!("INSERT INTO {FIELDS} (tbl, field, validation_text, description) VALUES (?1, ?2, ?3, ?4)"), [&d.name, &f.name, &f.validation_text, &f.description])
                    .map_err(err)?;
            }
        }
        // Rows that break a new relation stop the change (unless relations
        // aren't enforced now, as while importing).
        let bad: i64 = if check_keys { tx.query_row(&format!("SELECT count(*) FROM pragma_foreign_key_check({})", lit(&d.name)), [], |r| r.get(0)).map_err(err)? } else { 0 };
        if bad > 0 {
            return Err(Error::Db(format!("{bad} rows of {} point to rows that don't exist", d.name)));
        }
        tx.commit().map_err(err)
    }

    pub fn drop_table(&mut self, table: &str) -> Result<(), Error> {
        // Cascading deletes run the file's triggers: under the time limit.
        self.bounded(|c| c.execute_batch(&format!("DROP TABLE {}", q(table))).map_err(err))?;
        if self.has_table(FIELDS) {
            self.bounded(|c| c.execute(&format!("DELETE FROM {FIELDS} WHERE tbl = ?1"), [table]).map_err(err))?;
        }
        Ok(())
    }

    pub fn rename_table(&mut self, from: &str, to: &str) -> Result<(), Error> {
        self.bounded(|c| c.execute_batch(&format!("ALTER TABLE {} RENAME TO {}", q(from), q(to))).map_err(err))?;
        if self.has_table(FIELDS) {
            self.bounded(|c| c.execute(&format!("UPDATE {FIELDS} SET tbl = ?1 WHERE tbl = ?2"), [to, from]).map_err(err))?;
        }
        Ok(())
    }

    /// The columns (and their kinds) of a source.
    pub fn columns(&self, src: &RowSource) -> Result<(Vec<String>, Vec<FieldType>), Error> {
        let st = self.conn.prepare(&format!("{} LIMIT 0", src.from_sql(false))).map_err(err)?;
        let cols = st.columns();
        Ok((cols.iter().map(|c| c.name().to_string()).collect(), cols.iter().map(|c| c.decl_type().map(|d| FieldType::from_decl(d).0).unwrap_or(FieldType::Text)).collect()))
    }

    /// How many rows the filter leaves.
    pub fn count(&self, src: &RowSource, filter: &Filter) -> Result<usize, Error> {
        let (cols, _) = self.columns(src)?;
        let mut params = Vec::new();
        let w = filter.where_sql(&cols, &mut params);
        let n: i64 = self.bounded(|c| c.query_row(&format!("SELECT count(*) FROM ({}){w}", src.from_sql(false)), params_from_iter(params), |r| r.get(0)).map_err(err))?;
        Ok(n as usize)
    }

    /// Runs `f` stopping any statement after `READ_SECONDS`: a file's view
    /// may never end (a recursive query without a limit).
    pub(crate) fn bounded<T>(&self, f: impl FnOnce(&Connection) -> Result<T, Error>) -> Result<T, Error> {
        const READ_SECONDS: u64 = 20;
        self.bounded_for(READ_SECONDS, f)
    }

    /// [`Self::bounded`] with a limit of `secs`.
    pub(crate) fn bounded_for<T>(&self, secs: u64, f: impl FnOnce(&Connection) -> Result<T, Error>) -> Result<T, Error> {
        let end = web_time::Instant::now() + web_time::Duration::from_secs(secs);
        let _ = self.conn.progress_handler(100_000, Some(move || web_time::Instant::now() > end));
        let r = f(&self.conn);
        let _ = self.conn.progress_handler(0, None::<fn() -> bool>);
        r
    }

    /// `limit` rows from `offset`, filtered and sorted (column, ascending).
    pub fn page(&self, src: &RowSource, filter: &Filter, sort: &[(String, bool)], offset: usize, limit: usize) -> Result<Page, Error> {
        let rowid = matches!(src, RowSource::Table(t) if self.has_table(t) && !self.without_rowid(t));
        let (cols, types) = self.columns(src)?;
        let mut params = Vec::new();
        let w = filter.where_sql(&cols, &mut params);
        let order = if sort.is_empty() {
            if rowid { " ORDER BY \"__rowid\"".to_string() } else { String::new() }
        } else {
            format!(" ORDER BY {}", sort.iter().map(|(c, asc)| format!("{} {}", q(c), if *asc { "ASC" } else { "DESC" })).collect::<Vec<_>>().join(", "))
        };
        let sql = format!("SELECT * FROM ({}){w}{order} LIMIT {limit} OFFSET {offset}", src.from_sql(rowid));
        let skip = rowid as usize;
        let page = Page { columns: cols, types, ..Default::default() };
        self.bounded(|c| Self::read_page(c, &sql, params, rowid, skip, page))
    }

    fn read_page(c: &Connection, sql: &str, params: Vec<SqlValue>, rowid: bool, skip: usize, mut page: Page) -> Result<Page, Error> {
        let mut st = c.prepare(sql).map_err(err)?;
        let mut bytes = 0usize;
        let mut rows = st.query(params_from_iter(params)).map_err(err)?;
        while let Some(r) = rows.next().map_err(err)? {
            if rowid {
                page.rowids.push(r.get(0).map_err(err)?);
            }
            let mut row = Vec::with_capacity(page.columns.len());
            for (i, ty) in page.types.iter().enumerate() {
                let v = r.get_ref(i + skip).map_err(err)?;
                bytes = bytes.saturating_add(16 + match v {
                    ValueRef::Text(b) | ValueRef::Blob(b) => b.len(),
                    _ => 0,
                });
                if bytes > MAX_PAGE_BYTES {
                    return Err(Error::Db("the rows are too big to read".into()));
                }
                row.push(from_sql(v, *ty));
            }
            page.rows.push(row);
        }
        Ok(page)
    }

    fn without_rowid(&self, table: &str) -> bool {
        self.conn
            .query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1 COLLATE NOCASE", [table], |r| r.get::<_, String>(0))
            .map(|s| s.to_ascii_uppercase().replace(char::is_whitespace, "").ends_with("WITHOUTROWID"))
            .unwrap_or(false)
    }

    /// Sets a cell; gives its value before.
    pub fn update_cell(&mut self, table: &str, rowid: i64, col: &str, v: &Value) -> Result<Value, Error> {
        let ty = self.design(table)?.field(col).map(|f| f.ty).unwrap_or_default();
        let old = self
            .conn
            .query_row(&format!("SELECT {} FROM {} WHERE rowid = ?1", q(col), q(table)), [rowid], |r| Ok(from_sql(r.get_ref(0)?, ty)))
            .map_err(err)?;
        // Under the time limit: the file's triggers run too.
        self.bounded(|c| c.execute(&format!("UPDATE {} SET {} = ?1 WHERE rowid = ?2", q(table), q(col)), rusqlite::params![to_sql(v), rowid]).map_err(err))?;
        Ok(old)
    }

    /// Adds a row (with the given columns; the others take their default)
    /// and gives its rowid. With `rowid`, the row gets that one (undoing a
    /// delete).
    pub fn insert_row(&mut self, table: &str, cols: &[String], values: &[Value], rowid: Option<i64>) -> Result<i64, Error> {
        let mut names: Vec<String> = cols.iter().map(|c| q(c)).collect();
        let mut params: Vec<SqlValue> = values.iter().map(to_sql).collect();
        if let Some(id) = rowid {
            names.push("rowid".into());
            params.push(SqlValue::Integer(id));
        }
        let sql = if names.is_empty() {
            format!("INSERT INTO {} DEFAULT VALUES", q(table))
        } else {
            format!("INSERT INTO {} ({}) VALUES ({})", q(table), names.join(", "), (1..=names.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", "))
        };
        self.bounded(|c| c.execute(&sql, params_from_iter(params)).map_err(err))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Deletes a row; gives its columns and values (to put it back).
    pub fn delete_row(&mut self, table: &str, rowid: i64) -> Result<(Vec<String>, Vec<Value>), Error> {
        let old = self.row(table, rowid)?;
        self.bounded(|c| c.execute(&format!("DELETE FROM {} WHERE rowid = ?1", q(table)), [rowid]).map_err(err))?;
        Ok(old)
    }

    /// A row's columns and values.
    pub fn row(&self, table: &str, rowid: i64) -> Result<(Vec<String>, Vec<Value>), Error> {
        let (cols, types) = self.columns(&RowSource::Table(table.to_string()))?;
        let vals = self
            .conn
            .query_row(&format!("SELECT * FROM {} WHERE rowid = ?1", q(table)), [rowid], |r| (0..cols.len()).map(|i| Ok(from_sql(r.get_ref(i)?, types[i]))).collect::<Result<Vec<_>, _>>())
            .map_err(err)?;
        Ok((cols, vals))
    }

    /// Every relation: the enforced ones (foreign keys) and those only
    /// drawn (kept in the layout object).
    pub fn relations(&self) -> Vec<Relation> {
        let mut out = Vec::new();
        for t in self.table_names() {
            let Ok(d) = self.design(&t) else { continue };
            for fk in d.foreign_keys {
                out.push(Relation {
                    name: format!("{}_{}", fk.ref_table, t),
                    table: t.clone(),
                    columns: fk.columns,
                    ref_table: fk.ref_table,
                    ref_columns: fk.ref_columns,
                    enforce: true,
                    cascade_update: fk.on_update == RefAction::Cascade,
                    cascade_delete: fk.on_delete == RefAction::Cascade,
                    one_to_one: false,
                });
            }
        }
        if let Some(text) = self.load_object(Kind::Layout, "relations") {
            for r in crate::objects::read(&text).into_iter().filter(|r| r.section == "relation") {
                out.push(Relation {
                    name: r.get("name").to_string(),
                    table: r.get("table").to_string(),
                    columns: r.get("columns").split('\t').map(str::to_string).collect(),
                    ref_table: r.get("ref_table").to_string(),
                    ref_columns: r.get("ref_columns").split('\t').map(str::to_string).collect(),
                    enforce: false,
                    ..Default::default()
                });
            }
        }
        out
    }

    /// Adds a relation: enforced ones become a foreign key of `r.table`
    /// (rebuilding it), others are only drawn.
    pub fn add_relation(&mut self, r: &Relation) -> Result<(), Error> {
        if !r.enforce {
            return self.edit_layout(|recs| {
                let mut rec = crate::objects::Record::new("relation");
                rec.set("name", &r.name).set("table", &r.table).set("columns", r.columns.join("\t")).set("ref_table", &r.ref_table).set("ref_columns", r.ref_columns.join("\t"));
                recs.push(rec);
            });
        }
        let mut d = self.design(&r.table)?;
        d.foreign_keys.push(ForeignKey {
            columns: r.columns.clone(),
            ref_table: r.ref_table.clone(),
            ref_columns: r.ref_columns.clone(),
            on_update: if r.cascade_update { RefAction::Cascade } else { RefAction::Restrict },
            on_delete: if r.cascade_delete { RefAction::Cascade } else { RefAction::Restrict },
        });
        let name = d.name.clone();
        self.apply_design(Some(&name), &d)
    }

    /// Removes the relation from `table` to `ref_table` on `columns`.
    pub fn remove_relation(&mut self, r: &Relation) -> Result<(), Error> {
        if !r.enforce {
            return self.edit_layout(|recs| {
                recs.retain(|x| !(x.section == "relation" && x.get("table") == r.table && x.get("ref_table") == r.ref_table && x.get("columns") == r.columns.join("\t")));
            });
        }
        let mut d = self.design(&r.table)?;
        d.foreign_keys.retain(|fk| !(fk.ref_table.eq_ignore_ascii_case(&r.ref_table) && fk.columns == r.columns));
        let name = d.name.clone();
        self.apply_design(Some(&name), &d)
    }

    fn edit_layout(&mut self, f: impl FnOnce(&mut Vec<crate::objects::Record>)) -> Result<(), Error> {
        let mut recs = self.load_object(Kind::Layout, "relations").map(|t| crate::objects::read(&t)).unwrap_or_else(|| vec![Default::default()]);
        f(&mut recs);
        self.save_object(Kind::Layout, "relations", &crate::objects::write(&recs))
    }

    /// The names of the saved objects of a kind.
    pub fn objects(&self, kind: Kind) -> Vec<String> {
        if !self.has_table(OBJECTS) {
            return Vec::new();
        }
        self.bounded(|c| {
            let mut st = c.prepare(&format!("SELECT name FROM {OBJECTS} WHERE kind = ?1 ORDER BY name COLLATE NOCASE")).map_err(err)?;
            let mut left = MAX_PAGE_BYTES;
            let names = st.query_map([kind.key()], |r| r.get::<_, String>(0)).map_err(err)?;
            Ok(names.flatten().take_while(|n| {
                left = left.saturating_sub(n.len());
                left > 0
            }).collect())
        })
        .unwrap_or_default()
    }

    pub fn load_object(&self, kind: Kind, name: &str) -> Option<String> {
        if !self.has_table(OBJECTS) {
            return None;
        }
        self.bounded(|c| c.query_row(&format!("SELECT data FROM {OBJECTS} WHERE kind = ?1 AND name = ?2"), [kind.key(), name], |r| r.get(0)).map_err(err)).ok()
    }

    pub fn save_object(&mut self, kind: Kind, name: &str, data: &str) -> Result<(), Error> {
        self.own_tables()?;
        self.bounded(|c| c.execute(&format!("INSERT OR REPLACE INTO {OBJECTS} (kind, name, data) VALUES (?1, ?2, ?3)"), [kind.key(), name, data]).map_err(err))?;
        Ok(())
    }

    pub fn delete_object(&mut self, kind: Kind, name: &str) -> Result<(), Error> {
        if self.has_table(OBJECTS) {
            self.bounded(|c| c.execute(&format!("DELETE FROM {OBJECTS} WHERE kind = ?1 AND name = ?2"), [kind.key(), name]).map_err(err))?;
        }
        Ok(())
    }

    /// The VBA project of the database's macros (`vbaProject.bin`).
    pub fn vba_project(&self) -> Option<Vec<u8>> {
        let hex = self.load_object(Kind::Vba, "project")?;
        let hex = hex.trim().as_bytes();
        if hex.len() % 2 != 0 {
            return None;
        }
        hex.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect()
    }

    /// Keeps (or with None removes) the VBA project of the macros.
    pub fn set_vba_project(&mut self, bin: Option<&[u8]>) -> Result<(), Error> {
        use std::fmt::Write as _;
        match bin {
            Some(b) => {
                let mut hex = String::with_capacity(b.len() * 2);
                for x in b {
                    let _ = write!(hex, "{x:02x}");
                }
                self.save_object(Kind::Vba, "project", &hex)
            }
            None => self.delete_object(Kind::Vba, "project"),
        }
    }

    pub fn rename_object(&mut self, kind: Kind, from: &str, to: &str) -> Result<(), Error> {
        self.bounded(|c| c.execute(&format!("UPDATE {OBJECTS} SET name = ?1 WHERE kind = ?2 AND name = ?3"), [to, kind.key(), from]).map_err(err))?;
        Ok(())
    }

    /// Saves a query (the records of [`crate::query::QueryDesign::to_records`]
    /// or just `sql`). A select query is also kept as a view of its name, so
    /// other queries, forms and reports take rows from it; gives false when
    /// its SQL doesn't run here (it is saved anyway).
    pub fn save_query(&mut self, name: &str, recs: &[crate::objects::Record]) -> Result<bool, Error> {
        self.save_object(Kind::Query, name, &crate::objects::write(recs))?;
        let head = recs.first().cloned().unwrap_or_default();
        let sql = head.get("sql").trim().trim_end_matches(';').to_string();
        let up = sql.to_ascii_uppercase();
        let select = matches!(head.get("type"), "" | "select") && (up.starts_with("SELECT") || up.starts_with("WITH"));
        if self.names("view").iter().any(|v| v == name) {
            self.conn.execute_batch(&format!("DROP VIEW {}", q(name))).map_err(err)?;
        }
        if !select {
            return Ok(self.conn.prepare(&sql).is_ok());
        }
        // The SQL is the file's (an imported Access query, a saved
        // query): one statement, checked.
        Ok(self.guarded(|c| c.execute(&format!("CREATE VIEW {} AS {sql}", q(name)), []).map_err(err)).is_ok())
    }

    /// Deletes a saved query and its view.
    pub fn delete_query(&mut self, name: &str) -> Result<(), Error> {
        if self.names("view").iter().any(|v| v == name) {
            self.conn.execute_batch(&format!("DROP VIEW {}", q(name))).map_err(err)?;
        }
        self.delete_object(Kind::Query, name)
    }

    /// Runs a SELECT and gives all its rows.
    pub fn select(&self, sql: &str, params: &[Value]) -> Result<Page, Error> {
        self.bounded(|conn| {
            let mut st = conn.prepare(sql).map_err(err)?;
            let cols = st.columns();
            let mut page = Page {
                columns: cols.iter().map(|c| c.name().to_string()).collect(),
                types: cols.iter().map(|c| c.decl_type().map(|d| FieldType::from_decl(d).0).unwrap_or(FieldType::Text)).collect(),
                ..Default::default()
            };
            let mut rows = st.query(params_from_iter(params.iter().map(to_sql))).map_err(err)?;
            let mut bytes = 0usize;
            while let Some(r) = rows.next().map_err(err)? {
                let mut row = Vec::with_capacity(page.columns.len());
                for (i, ty) in page.types.iter().enumerate() {
                    let v = r.get_ref(i).map_err(err)?;
                    bytes = bytes.saturating_add(16 + match v {
                        ValueRef::Text(b) | ValueRef::Blob(b) => b.len(),
                        _ => 0,
                    });
                    if bytes > MAX_PAGE_BYTES {
                        return Err(Error::Db("the rows are too big to read".into()));
                    }
                    row.push(from_sql(v, *ty));
                }
                page.rows.push(row);
            }
            Ok(page)
        })
    }

    /// How many rows an action query (INSERT, UPDATE, DELETE, CREATE TABLE
    /// AS...) would change, without changing them.
    /// The query runs on a copy of the database in memory, never on the
    /// file, and is held to [`Db::run_action`]'s checks.
    pub fn preview_action(&mut self, sql: &str) -> Result<usize, Error> {
        let copy = Db { conn: self.snapshot()?, path: PathBuf::new() };
        harden(&copy.conn)?;
        copy.conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(err)?;
        copy.action(sql, false)
    }

    /// Runs an action query; gives the rows changed. It is a single
    /// statement that can't attach files, change settings or end the
    /// transaction it runs in.
    pub fn run_action(&mut self, sql: &str) -> Result<usize, Error> {
        self.action(sql, true)
    }

    fn action(&self, sql: &str, keep: bool) -> Result<usize, Error> {
        let sql = sql.trim().trim_end_matches(';');
        let tx = self.conn.unchecked_transaction().map_err(err)?;
        let before = tx.total_changes();
        // A preview (of a query the file brings) under the time limit too.
        if keep {
            self.bounded_for(WRITE_SECONDS, |_| self.guarded(|c| c.execute(sql, []).map_err(err)))?;
        } else {
            self.bounded(|_| self.guarded(|c| c.execute(sql, []).map_err(err)))?;
        }
        let n = (tx.total_changes() - before) as usize;
        if keep { tx.commit() } else { tx.rollback() }.map_err(err)?;
        Ok(n)
    }

    /// Compacts the file (gives back the space of deleted rows).
    pub fn compact(&mut self) -> Result<(), Error> {
        self.with_attach(|| self.conn.execute_batch("VACUUM").map_err(err))
    }

    /// Checks the file and rebuilds its indexes; gives the problems found
    /// (empty when it is sound).
    pub fn repair(&mut self) -> Result<Vec<String>, Error> {
        let mut st = self.conn.prepare("PRAGMA integrity_check").map_err(err)?;
        let found: Vec<String> = st.query_map([], |r| r.get::<_, String>(0)).map_err(err)?.flatten().filter(|s| s != "ok").collect();
        drop(st);
        self.with_attach(|| self.conn.execute_batch("REINDEX; VACUUM;").map_err(err))?;
        Ok(found)
    }

    /// Writes a copy of the database to `path`.
    pub fn backup_to(&self, path: &Path) -> Result<(), Error> {
        let mut failed = None;
        write_new(path, |tmp| {
            self.with_attach(|| self.conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy()]).map_err(err)).map(|_| ()).map_err(|e| {
                failed = Some(e);
                std::io::Error::other("copy")
            })
        })
        .map_err(|e| failed.take().unwrap_or_else(|| crate::io_err(path, e)))
    }

    /// A copy of the whole database in memory (for undo).
    pub fn snapshot(&self) -> Result<Connection, Error> {
        let mut dst = Connection::open_in_memory().map_err(err)?;
        rusqlite::backup::Backup::new(&self.conn, &mut dst).map_err(err)?.run_to_completion(256, web_time::Duration::ZERO, None).map_err(err)?;
        Ok(dst)
    }

    /// Puts back a copy made by [`Db::snapshot`].
    pub fn restore(&mut self, snap: &Connection) -> Result<(), Error> {
        rusqlite::backup::Backup::new(snap, &mut self.conn).map_err(err)?.run_to_completion(256, web_time::Duration::ZERO, None).map_err(err)?;
        Ok(())
    }

    /// The size of the file in bytes (0 in memory).
    pub fn size(&self) -> u64 {
        apex_fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    /// Copies the tables of another SQLite database into this one, with
    /// their design (a name already used gets a number). Gives the new
    /// tables' names.
    pub fn copy_tables_from(&mut self, path: &Path) -> Result<Vec<String>, Error> {
        let src = Db::open(path)?;
        let mut made = Vec::new();
        self.with_attach(|| self.conn.execute("ATTACH DATABASE ?1 AS apex_src", [path.to_string_lossy()]).map_err(err))?;
        let r = (|| {
            for t in src.table_names() {
                let mut d = src.design(&t)?;
                d.foreign_keys.clear();
                let names = self.table_names();
                let mut name = t.clone();
                let mut k = 2;
                while names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                    name = format!("{t}{k}");
                    k += 1;
                }
                d.name = name.clone();
                self.conn.execute(&d.create_sql(&name), []).map_err(err)?;
                let cols = d.fields.iter().map(|f| q(&f.name)).collect::<Vec<_>>().join(", ");
                self.conn.execute_batch(&format!("INSERT INTO main.{} ({cols}) SELECT {cols} FROM apex_src.{}", q(&name), q(&t))).map_err(err)?;
                for i in &d.indexes {
                    let i = IndexDef { name: format!("{name}_{}", i.columns.join("_")), ..i.clone() };
                    let _ = self.conn.execute(&index_sql(&name, &i), []);
                }
                made.push(name);
            }
            Ok(())
        })();
        let _ = self.conn.execute_batch("DETACH DATABASE apex_src");
        r.map(|_| made)
    }

    /// Writes the rows of `source` (a table or a SELECT) as table `name`
    /// of the SQLite database at `path` (made when missing; a table there
    /// with that name is replaced).
    pub fn export_rows(&self, source: &RowSource, path: &Path, name: &str) -> Result<(), Error> {
        let design = match source {
            RowSource::Table(t) if self.table_names().iter().any(|n| n == t) => self.design(t).ok(),
            _ => None,
        };
        {
            let dst = Db::open_no_follow(path)?;
            dst.conn.execute_batch(&format!("DROP TABLE IF EXISTS {}", q(name))).map_err(err)?;
            if let Some(mut d) = design.clone() {
                d.foreign_keys.clear();
                dst.conn.execute(&d.create_sql(name), []).map_err(err)?;
            }
        }
        self.with_attach(|| self.conn.execute("ATTACH DATABASE ?1 AS apex_dst", [path.to_string_lossy()]).map_err(err))?;
        let select = source.from_sql(false);
        // A query's SQL comes from the file: one statement, under the
        // document's rules.
        let sql = if design.is_some() { format!("INSERT INTO apex_dst.{} {select}", q(name)) } else { format!("CREATE TABLE apex_dst.{} AS {select}", q(name)) };
        // Writing, a query that never ends would fill the disk.
        let r = self.bounded_for(WRITE_SECONDS, |_| self.guarded(|c| c.execute(&sql, []).map_err(err)));
        let _ = self.conn.execute_batch("DETACH DATABASE apex_dst");
        r.map(|_| ())
    }
}

/// The CREATE INDEX statement of an index of `table`.
pub fn index_sql(table: &str, i: &IndexDef) -> String {
    let name = if i.name.is_empty() { format!("{}_{}", table, i.columns.join("_")) } else { i.name.clone() };
    format!("CREATE {}INDEX {} ON {} ({})", if i.unique { "UNIQUE " } else { "" }, q(&name), q(table), i.columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", "))
}

/// The named CHECK constraints ("ck_<field>") of a CREATE TABLE statement.
fn checks(sql: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = sql;
    while let Some(i) = rest.find("CONSTRAINT \"ck_") {
        let after = &rest[i + 15..];
        let Some(end) = after.find("\" CHECK (") else { break };
        let name = after[..end].replace("\"\"", "\"");
        let body = &after[end + 9..];
        let mut depth = 1;
        let mut close = body.len();
        let mut in_str = None;
        for (k, c) in body.char_indices() {
            match (c, in_str) {
                ('\'' | '"', None) => in_str = Some(c),
                (c, Some(s)) if c == s => in_str = None,
                ('(', None) => depth += 1,
                (')', None) => {
                    depth -= 1;
                    if depth == 0 {
                        close = k;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push((name, body[..close].to_string()));
        rest = &body[close.min(body.len())..];
    }
    out
}

/// Refuses a database whose rollback journal (`<path>-journal`, rolled
/// back on the first read) names a super-journal: SQLite deletes that
/// file afterwards, and one that came with the database may name any file
/// of the user's. Only transactions over several attached databases write
/// one, which nothing of ours leaves behind.
pub fn check_journal(path: &Path) -> Result<(), Error> {
    use std::io::{Read, Seek, SeekFrom};
    const MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];
    let mut j = path.as_os_str().to_owned();
    j.push("-journal");
    let Ok(mut f) = apex_fs::File::open(&j) else { return Ok(()) };
    let mut tail = [0u8; 8];
    let len = f.metadata().map_or(0, |m| m.len());
    if len >= 16 && f.seek(SeekFrom::Start(len - 8)).is_ok() && f.read_exact(&mut tail).is_ok() && tail == MAGIC {
        return Err(Error::Db(format!("{}: its journal names another file to delete; move the database without it", path.display())));
    }
    Ok(())
}

/// Removes the file or link at `path`, if any, for a new file there.
pub fn clear(path: &Path) -> std::io::Result<()> {
    match apex_fs::symlink_metadata(path) {
        Ok(_) => apex_fs::remove_file(path),
        Err(_) => Ok(()),
    }
}

/// Makes the file at `path` with `make`, which writes a new file at the
/// path it is given: in a folder of its own beside `path`, then moved in
/// place. SQLite follows a link where it writes, and one someone else put
/// in `path`'s place (in a shared folder) is replaced instead.
pub fn write_new(path: &Path, make: impl FnOnce(&Path) -> std::io::Result<()>) -> std::io::Result<()> {
    use std::hash::{BuildHasher, Hasher};
    let parent = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let mut made = None;
    for _ in 0..8 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        let dir = parent.join(format!(".{}.{:016x}.tmp~", name.to_string_lossy(), h.finish()));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut b = apex_fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        match b.create(&dir) {
            Ok(()) => {
                made = Some(dir);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let dir = made.ok_or_else(|| std::io::Error::other("no temporary folder"))?;
    let tmp = dir.join(name);
    let r = make(&tmp).and_then(|()| apex_fs::rename(&tmp, path));
    let _ = apex_fs::remove_file(&tmp);
    let _ = apex_fs::remove_dir(&dir);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_journal_naming_another_file_is_refused() {
        let dir = apex_fs::temp_dir().join(format!("apex-db-journal-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let db = dir.join("a.apexdb");
        drop(Db::create(&db).unwrap());
        let mut j = vec![0u8; 512];
        j.extend([0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]);
        apex_fs::write(dir.join("a.apexdb-journal"), &j).unwrap();
        assert!(Db::open(&db).is_err());
        apex_fs::remove_file(dir.join("a.apexdb-journal")).unwrap();
        assert!(Db::open(&db).is_ok());
        apex_fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keeps_the_vba_project() {
        let mut db = Db::in_memory().unwrap();
        assert_eq!(db.vba_project(), None);
        db.set_vba_project(Some(&[0, 1, 0xab, 0xff])).unwrap();
        assert_eq!(db.vba_project(), Some(vec![0, 1, 0xab, 0xff]));
        db.set_vba_project(None).unwrap();
        assert_eq!(db.vba_project(), None);
    }

    fn customers() -> TableDesign {
        let mut id = FieldDef::new("Id", FieldType::AutoNumber);
        id.pk = true;
        let mut name = FieldDef::new("Name", FieldType::Text);
        name.size = 40;
        name.required = true;
        let mut age = FieldDef::new("Age", FieldType::Integer);
        age.check = "> 0".into();
        age.validation_text = "Positive".into();
        age.default = "18".into();
        let mut mail = FieldDef::new("Mail", FieldType::Text);
        mail.unique = true;
        TableDesign { name: "Customers".into(), fields: vec![id, name, age, mail], indexes: vec![IndexDef { name: "by_name".into(), columns: vec!["Name".into()], unique: false }], foreign_keys: vec![] }
    }

    #[test]
    fn copy_between_files() {
        let dir = apex_fs::temp_dir().join(format!("apex_db_copy_{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.apexdb"), dir.join("b.sqlite"));
        let mut db = Db::open(&a).unwrap();
        db.apply_design(None, &customers()).unwrap();
        db.insert_row("Customers", &["Name".into(), "Age".into()], &[Value::Text("Ann".into()), Value::Num(30.0)], None).unwrap();
        db.export_rows(&RowSource::Table("Customers".into()), &b, "People").unwrap();
        db.export_rows(&RowSource::Sql("SELECT Name FROM Customers".into()), &b, "Names").unwrap();
        // A query's SQL stays one SELECT: more statements are refused.
        assert!(db.export_rows(&RowSource::Sql("SELECT 1); DROP TABLE Customers; SELECT (1".into()), &b, "Evil").is_err());
        assert!(db.table_names().iter().any(|n| n == "Customers"));
        let mut other = Db::open(&b).unwrap();
        assert_eq!(other.table_names(), ["Names", "People"]);
        // The design went along: the rule still refuses bad ages.
        assert!(other.insert_row("People", &["Name".into(), "Age".into()], &[Value::Text("Bob".into()), Value::Num(-1.0)], None).is_err());
        let made = other.copy_tables_from(&a).unwrap();
        assert_eq!(made, ["Customers"]);
        assert_eq!(other.count(&RowSource::Table("Customers".into()), &Filter::default()).unwrap(), 1);
        // Again: the name is taken.
        assert_eq!(other.copy_tables_from(&a).unwrap(), ["Customers2"]);
        drop((db, other));
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_table_naming_itself_is_read_once() {
        let db = Db::in_memory().unwrap();
        db.conn.execute_batch("CREATE TABLE t(a INTEGER PRIMARY KEY, b REFERENCES t); CREATE TABLE x(p REFERENCES y); CREATE TABLE y(q PRIMARY KEY, r REFERENCES x);").unwrap();
        assert_eq!(db.design("t").unwrap().foreign_keys[0].ref_columns, vec!["a".to_string()]);
        assert_eq!(db.design("x").unwrap().foreign_keys[0].ref_columns, vec!["q".to_string()]);
        assert!(db.design("y").unwrap().foreign_keys[0].ref_columns.is_empty());
    }

    #[test]
    fn design_round_trip() {
        let mut db = Db::in_memory().unwrap();
        db.apply_design(None, &customers()).unwrap();
        let d = db.design("Customers").unwrap();
        let f = |n: &str| d.field(n).unwrap().clone();
        assert_eq!(f("Id").ty, FieldType::AutoNumber);
        assert!(f("Id").pk);
        assert_eq!((f("Name").ty, f("Name").size, f("Name").required), (FieldType::Text, 40, true));
        assert_eq!(f("Age").check, "> 0");
        assert_eq!(f("Age").validation_text, "Positive");
        assert_eq!(f("Age").default, "18");
        assert!(f("Mail").unique);
        assert_eq!(d.indexes.len(), 1);
        // The rule holds.
        assert!(db.insert_row("Customers", &["Name".into(), "Age".into()], &[Value::Text("A".into()), Value::Int(-1)], None).is_err());
        let id = db.insert_row("Customers", &["Name".into()], &[Value::Text("Ana".into())], None).unwrap();
        let p = db.page(&RowSource::Table("Customers".into()), &Filter::default(), &[], 0, 10).unwrap();
        assert_eq!(p.rowids, vec![id]);
        assert_eq!(p.rows[0][2], Value::Int(18));
    }

    #[test]
    fn rebuild_keeps_rows() {
        let mut db = Db::in_memory().unwrap();
        db.apply_design(None, &customers()).unwrap();
        db.insert_row("Customers", &["Name".into(), "Age".into()], &[Value::Text("Ana".into()), Value::Int(30)], None).unwrap();
        let mut d = db.design("Customers").unwrap();
        // Rename a field, drop one, add one.
        d.fields[1].name = "FullName".into();
        d.fields.retain(|f| f.name != "Mail");
        d.fields.push(FieldDef::new("Born", FieldType::DateTime));
        d.indexes[0].columns = vec!["FullName".into()];
        db.apply_design(Some("Customers"), &d).unwrap();
        let p = db.page(&RowSource::Table("Customers".into()), &Filter::default(), &[], 0, 10).unwrap();
        assert_eq!(p.columns, ["Id", "FullName", "Age", "Born"]);
        assert_eq!(p.rows[0][1], Value::Text("Ana".into()));
        db.update_cell("Customers", p.rowids[0], "Born", &Value::Date(45352.5)).unwrap();
        let p = db.page(&RowSource::Table("Customers".into()), &Filter::default(), &[], 0, 10).unwrap();
        assert_eq!(p.rows[0][3], Value::Date(45352.5));
    }

    #[test]
    fn relations_and_cascades() {
        let mut db = Db::in_memory().unwrap();
        db.apply_design(None, &customers()).unwrap();
        let mut oid = FieldDef::new("Id", FieldType::AutoNumber);
        oid.pk = true;
        let orders = TableDesign { name: "Orders".into(), fields: vec![oid, FieldDef::new("Customer", FieldType::Integer)], ..Default::default() };
        db.apply_design(None, &orders).unwrap();
        let c = db.insert_row("Customers", &["Name".into()], &[Value::Text("Ana".into())], None).unwrap();
        db.insert_row("Orders", &["Customer".into()], &[Value::Int(c)], None).unwrap();
        let rel = Relation { table: "Orders".into(), columns: vec!["Customer".into()], ref_table: "Customers".into(), ref_columns: vec!["Id".into()], enforce: true, cascade_delete: true, ..Default::default() };
        db.add_relation(&rel).unwrap();
        let rels = db.relations();
        assert_eq!(rels.len(), 1);
        assert!(rels[0].cascade_delete);
        // Integrity: an order of a customer that doesn't exist is refused.
        assert!(db.insert_row("Orders", &["Customer".into()], &[Value::Int(99)], None).is_err());
        // Cascade: deleting the customer deletes the order.
        db.delete_row("Customers", c).unwrap();
        assert_eq!(db.count(&RowSource::Table("Orders".into()), &Filter::default()).unwrap(), 0);
        db.remove_relation(&rels[0]).unwrap();
        assert!(db.relations().is_empty());
    }

    #[test]
    fn filters_objects_and_actions() {
        let mut db = Db::in_memory().unwrap();
        db.apply_design(None, &customers()).unwrap();
        for (n, a) in [("Ana", 30), ("Bea", 40), ("Carlos", 50)] {
            db.insert_row("Customers", &["Name".into(), "Age".into()], &[Value::Text(n.into()), Value::Int(a)], None).unwrap();
        }
        let src = RowSource::Table("Customers".into());
        let f = Filter { search: "ar".into(), ..Default::default() };
        assert_eq!(db.count(&src, &f).unwrap(), 1);
        let f = Filter { conditions: vec![("Age".into(), ">".into(), Value::Int(35))], ..Default::default() };
        let p = db.page(&src, &f, &[("Age".into(), false)], 0, 10).unwrap();
        assert_eq!(p.rows.iter().map(|r| r[1].to_text()).collect::<Vec<_>>(), ["Carlos", "Bea"]);
        assert_eq!(db.preview_action("DELETE FROM Customers WHERE Age > 35").unwrap(), 2);
        assert_eq!(db.count(&src, &Filter::default()).unwrap(), 3);
        let snap = db.snapshot().unwrap();
        assert_eq!(db.run_action("DELETE FROM Customers WHERE Age > 35").unwrap(), 2);
        db.restore(&snap).unwrap();
        assert_eq!(db.count(&src, &Filter::default()).unwrap(), 3);
        db.save_object(Kind::Query, "Old", "sql=SELECT 1").unwrap();
        assert_eq!(db.objects(Kind::Query), ["Old"]);
        assert_eq!(db.table_names(), ["Customers"]);
        let (cols, old) = db.delete_row("Customers", 1).unwrap();
        db.insert_row("Customers", &cols, &old, Some(1)).unwrap();
        assert_eq!(db.count(&src, &Filter::default()).unwrap(), 3);
    }

    #[test]
    fn typed_input() {
        assert_eq!(parse_input("12", FieldType::Integer), Some(Value::Int(12)));
        assert_eq!(parse_input("x", FieldType::Integer), None);
        assert_eq!(parse_input("", FieldType::Integer), Some(Value::Null));
        assert_eq!(parse_input("31/12/2024", FieldType::DateTime), parse_date("2024-12-31").map(Value::Date));
        assert_eq!(parse_input("sí", FieldType::Bool), Some(Value::Bool(true)));
        assert_eq!(rule_sql("Age", "BETWEEN 1 AND 5"), "\"Age\" BETWEEN 1 AND 5");
        assert_eq!(rule_sql("Age", "Age > 1"), "Age > 1");
    }

    /// SQL a file brings (an imported query, a default value, an action
    /// query) can't reach other files or run more than one statement.
    #[test]
    fn document_sql_stays_in_the_file() {
        let dir = apex_fs::temp_dir().join(format!("apex-db-harden-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let file = dir.join("db.sqlite");
        let out = dir.join("out.txt");
        let lit = out.to_string_lossy().replace('\'', "''");
        let mut db = Db::open(&file).unwrap();
        db.apply_design(None, &customers()).unwrap();
        db.insert_row("Customers", &["Name".into(), "Age".into()], &[Value::Text("Ana".into()), Value::Int(30)], None).unwrap();
        let mut rec = crate::objects::Record::default();
        rec.set("sql", &format!("SELECT 1; ATTACH DATABASE '{lit}' AS z; CREATE TABLE z.t(a)"));
        assert!(!db.save_query("Evil", &[rec]).unwrap());
        let mut rec = crate::objects::Record::default();
        rec.set("sql", "SELECT Name FROM Customers");
        assert!(db.save_query("Names", &[rec]).unwrap());
        assert!(db.preview_action(&format!("DELETE FROM Customers; ATTACH DATABASE '{lit}' AS z")).is_err());
        assert!(db.preview_action("DELETE FROM Customers; COMMIT").is_err());
        assert!(db.run_action(&format!("VACUUM INTO '{lit}'")).is_err());
        assert!(db.run_action(&format!("ATTACH DATABASE '{lit}' AS z")).is_err());
        assert!(db.run_action("PRAGMA writable_schema = ON").is_err());
        assert!(db.conn().execute(&format!("VACUUM INTO '{lit}'"), []).is_err());
        // A default value can't end its statement either.
        let mut d = customers();
        d.name = "Evil".into();
        d.fields[2].default = format!("0); ATTACH DATABASE '{lit}' AS z; CREATE TABLE x(a DEFAULT (0");
        assert!(db.apply_design(None, &d).is_err());
        assert!(!out.exists());
        // The preview leaves the file as it was; our own VACUUM still works.
        assert_eq!(db.preview_action("DELETE FROM Customers").unwrap(), 1);
        assert_eq!(db.count(&RowSource::Table("Customers".into()), &Filter::default()).unwrap(), 1);
        db.compact().unwrap();
        db.backup_to(&dir.join("copy.sqlite")).unwrap();
        assert!(db.conn().execute(&format!("ATTACH DATABASE '{lit}' AS z"), []).is_err());
        drop(db);
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
