//! Databases: SQLite files (rusqlite with SQLite compiled in), PostgreSQL
//! (postgres, TLS by rustls), MySQL/MariaDB (mysql, TLS by rustls) and
//! SQL Server (see `sqlserver`).

//! Connections only read: SQLite files are opened read only (and can't
//! attach or write other files), and the servers get one statement that
//! reads ([`read_only_sql`]) in a session that can't write where the
//! server allows it (PostgreSQL, MySQL). A workbook's query is the
//! workbook author's SQL, run with the user's password.

use std::path::Path;

use apex_sheet::query::{DbKind, Source};

#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mysql"))]
use crate::table::unique_names;
use crate::table::{DataTable, Val};
use crate::Error;

/// A bare table name ("sales", "public.sales") as a SELECT; anything else
/// is taken as SQL.
pub fn select_sql(sql: &str, quote: char) -> String {
    let s = sql.trim().trim_end_matches(';').trim();
    let ident = !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$'));
    if ident {
        let q: Vec<String> = s.split('.').map(|p| format!("{quote}{p}{quote}")).collect();
        format!("SELECT * FROM {}", q.join("."))
    } else {
        s.to_string()
    }
}

/// Words a reading query never needs (SQL Server runs statements one
/// after another without `;`, so they are looked for anywhere).
const WRITES: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "DROP", "ALTER", "CREATE", "EXEC", "EXECUTE", "GRANT", "REVOKE", "DENY", "TRUNCATE", "COMMIT", "BEGIN", "ROLLBACK", "SAVE", "INTO",
    "DECLARE", "SET", "USE", "BACKUP", "RESTORE", "SHUTDOWN", "KILL", "DBCC", "BULK", "OPENROWSET", "OPENDATASOURCE", "OPENQUERY", "RECONFIGURE", "WAITFOR", "CALL", "DO",
    "COPY", "LOAD", "HANDLER", "LOCK", "UNLOCK", "PREPARE", "DEALLOCATE", "VACUUM", "REINDEX", "CLUSTER", "LISTEN", "NOTIFY", "REFRESH", "IMPORT", "INSTALL", "UNINSTALL",
];

/// Server functions with effects beyond reading (writes through another
/// session, files of the server, other sessions, settings, waits): exact
/// names, then name beginnings.
const ACTING: &[&str] = &["SET_CONFIG", "LOAD_FILE", "SYS_EXEC", "SYS_EVAL", "SLEEP", "BENCHMARK", "PG_SLEEP"];
const ACTING_PREFIXES: &[&str] = &["QUERY_TO_XML", "CURSOR_TO_XML", "DBLINK", "LO_", "PG_READ", "PG_WRITE", "PG_LS_", "PG_STAT_FILE", "PG_FILE", "PG_TERMINATE", "PG_CANCEL", "PG_RELOAD", "PG_ROTATE", "PG_SLEEP", "PG_PROMOTE", "PG_ADVISORY", "XP_", "SP_", "FN_XE_", "FN_GET_AUDIT_FILE", "FN_TRACE_", "FN_DUMP_DBLOG"];

/// Whether `word` (maybe schema-qualified) names one of [`ACTING`].
fn acts(word: &str) -> bool {
    let w = word.rsplit('.').next().unwrap_or(word).trim_matches(['"', '`', '[', ']']).to_ascii_uppercase();
    ACTING.contains(&w.as_str()) || ACTING_PREFIXES.iter().any(|p| w.starts_with(p))
}

/// Checks that `sql` (as [`select_sql`] gives it) is one statement that
/// reads, for a `kind` server: it starts with SELECT, WITH, VALUES, TABLE,
/// SHOW, DESCRIBE or EXPLAIN, has no `;` but at its end, and none of
/// [`WRITES`] outside quotes and comments. Backslashes in strings escape
/// on some servers and not on others: both readings must pass.
pub fn read_only_sql(sql: &str, kind: DbKind) -> Result<(), Error> {
    let bad = |why: &str| Err(Error::Invalid(format!("only a query that reads data can run here ({why})")));
    for backslash in [false, true] {
        let d = Dialect { backslash, hash: kind == DbKind::MySql, dollar: kind == DbKind::Postgres, brackets: kind == DbKind::SqlServer, backtick: kind == DbKind::MySql };
        let Some(words) = sql_words(sql.trim().trim_end_matches(';'), d) else { return bad(";") };
        let first = words.first().map(|w| w.to_ascii_uppercase()).unwrap_or_default();
        if !matches!(first.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "(") {
            return bad(&first);
        }
        if let Some(w) = words.iter().find(|w| WRITES.iter().any(|x| w.eq_ignore_ascii_case(x))) {
            return bad(w);
        }
        // Called, not a column that happens to be named alike.
        if let Some(w) = words.windows(2).find(|p| p[1] == "(" && acts(&p[0])) {
            return bad(&w[0]);
        }
    }
    Ok(())
}

/// How a server reads quotes and comments.
#[derive(Clone, Copy)]
struct Dialect {
    /// `\'` inside a string doesn't end it.
    backslash: bool,
    /// `#` starts a comment, and `--` only before a space or control
    /// character (MySQL: `1--1` is one minus minus one).
    hash: bool,
    /// `$tag$ ... $tag$` strings (PostgreSQL).
    dollar: bool,
    /// `[name]` (SQL Server).
    brackets: bool,
    /// `` `name` `` (MySQL).
    backtick: bool,
}

/// The bare words of `sql` (and "(" when it starts with one), skipping
/// strings, quoted names and comments; `None` when a `;` separates
/// statements.
fn sql_words(sql: &str, d: Dialect) -> Option<Vec<String>> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let line_end = |i: usize| b[i..].iter().position(|&x| x == b'\n').map_or(b.len(), |p| i + p + 1);
    // To after the closing quote (doubled ones are part of the text).
    let close = |i: usize, end: u8| -> usize {
        let mut j = i + 1;
        while j < b.len() {
            if b[j] == end {
                if b.get(j + 1) == Some(&end) && end != b']' {
                    j += 2;
                    continue;
                }
                return j + 1;
            }
            if d.backslash && b[j] == b'\\' && end != b']' {
                j += 1;
            }
            j += 1;
        }
        b.len()
    };
    while i < b.len() {
        let c = b[i];
        match c {
            b'\'' => i = close(i, c),
            // Quoted names are kept with their opening quote: never a
            // keyword, but a function may be called by one.
            b'"' | b'`' | b'[' if c == b'"' || (c == b'`' && d.backtick) || (c == b'[' && d.brackets) => {
                let end = close(i, if c == b'[' { b']' } else { c });
                out.push(String::from_utf8_lossy(&b[i..end.saturating_sub(1).max(i + 1)]).into_owned());
                i = end;
            }
            b'-' if b.get(i + 1) == Some(&b'-') && (!d.hash || b.get(i + 2).is_none_or(|x| *x <= b' ')) => i = line_end(i),
            b'#' if d.hash => i = line_end(i),
            // MySQL and MariaDB run what is in /*! ... */ and /*M! ... */.
            b'/' if b.get(i + 1) == Some(&b'*') && (b.get(i + 2) == Some(&b'!') || (matches!(b.get(i + 2), Some(b'M' | b'm')) && b.get(i + 3) == Some(&b'!'))) => return None,
            // PostgreSQL's U&"..." names hide their letters in escapes.
            b'U' | b'u' if b.get(i + 1) == Some(&b'&') => return None,
            b'/' if b.get(i + 1) == Some(&b'*') => i = b[i + 2..].windows(2).position(|w| w == b"*/").map_or(b.len(), |p| i + 2 + p + 2),
            // $tag$ ... $tag$ ($1 is a parameter).
            b'$' if d.dollar && b.get(i + 1).is_some_and(|x| !x.is_ascii_digit()) && b[i + 1..].iter().find(|x| !(x.is_ascii_alphanumeric() || **x == b'_')) == Some(&b'$') => {
                let tag_len = b[i + 1..].iter().position(|&x| x == b'$').unwrap_or(0) + 2;
                let tag = &b[i..i + tag_len];
                i = b[i + tag_len..].windows(tag.len()).position(|w| w == tag).map_or(b.len(), |p| i + tag_len + p + tag.len());
            }
            b';' => return None,
            b'(' => {
                out.push("(".to_string());
                i += 1;
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 0x80) {
                    i += 1;
                }
                out.push(String::from_utf8_lossy(&b[start..i]).into_owned());
            }
            _ => i += 1,
        }
    }
    Some(out)
}

/// Text columns that are all `t`/`f` (PostgreSQL's booleans as text).
#[cfg(feature = "postgres")]
fn pg_bools(t: &mut DataTable) {
    for c in 0..t.width() {
        let all = t.rows.iter().all(|r| matches!(&r[c], Val::Null) || matches!(&r[c], Val::Text(s) if s == "t" || s == "f"));
        if all && t.rows.iter().any(|r| !r[c].is_null()) {
            for r in &mut t.rows {
                if let Val::Text(s) = &r[c] {
                    r[c] = Val::Bool(s == "t");
                }
            }
        }
    }
}

#[cfg(feature = "sqlite")]
pub fn sqlite(path: &Path, sql: &str) -> Result<DataTable, Error> {
    use rusqlite::types::ValueRef;
    let err = |e: rusqlite::Error| Error::Database(e.to_string());
    if !apex_fs::PathExt::fs_is_file(path) {
        return Err(Error::Io(format!("{}: not found", path.display())));
    }
    apex_db::sqlite::check_journal(path).map_err(|e| Error::Database(e.to_string()))?;
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(err)?;
    // Read only covers this file; VACUUM INTO and ATTACH could still make
    // others.
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0).map_err(err)?;
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true).map_err(err)?;
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false).map_err(err)?;
    // A query may compute forever before its first row, or make one huge
    // value: a minute in all and 64 MB a value.
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 64 << 20).map_err(err)?;
    let end = web_time::Instant::now() + web_time::Duration::from_secs(60);
    conn.progress_handler(100_000, Some(move || web_time::Instant::now() > end)).map_err(err)?;
    let mut st = conn.prepare(&select_sql(sql, '"')).map_err(err)?;
    let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    // Dates are text (or numbers) in SQLite: the declared type tells.
    let dates: Vec<bool> = st.columns().iter().map(|c| c.decl_type().is_some_and(|t| t.to_ascii_uppercase().contains("DATE") || t.to_ascii_uppercase().contains("TIME"))).collect();
    let n = names.len();
    let mut t = DataTable::new(unique_names(names));
    let mut rows = st.query([]).map_err(err)?;
    // A recursive query can yield rows forever: stop at a sheet's worth or
    // the read budget.
    let mut bytes = 0u64;
    while let Some(r) = rows.next().map_err(err)? {
        if t.rows.len() >= 1 << 20 || bytes > super::MAX_BYTES {
            return Err(Error::Database("the query returns too much data".into()));
        }
        bytes += 16 * n as u64;
        let mut row = Vec::with_capacity(n);
        for i in 0..n {
            row.push(match r.get_ref(i).map_err(err)? {
                ValueRef::Null => Val::Null,
                ValueRef::Integer(v) => Val::Num(v as f64),
                ValueRef::Real(v) => Val::Num(v),
                ValueRef::Text(b) => {
                    bytes += b.len() as u64;
                    let s = String::from_utf8_lossy(b).into_owned();
                    match crate::table::parse_date(&s).filter(|_| dates[i]) {
                        Some(d) => Val::Date(d),
                        None => Val::Text(s),
                    }
                }
                ValueRef::Blob(b) => Val::Text(format!("({} bytes)", b.len())),
            });
        }
        t.rows.push(row);
    }
    Ok(t)
}

#[cfg(not(feature = "sqlite"))]
pub fn sqlite(_path: &Path, _sql: &str) -> Result<DataTable, Error> {
    Err(Error::NotBuilt("SQLite"))
}

/// Where database sources go instead of the network when set: in the
/// browser, to the server the page came from (`apexdesk serve`), which
/// runs them itself.
pub static BRIDGE: std::sync::OnceLock<fn(&Source, &str) -> Result<DataTable, Error>> = std::sync::OnceLock::new();

/// Runs a PostgreSQL or MySQL source with its password.
pub fn server(src: &Source, password: &str) -> Result<DataTable, Error> {
    let Source::Database { kind, host, port, database, user, sql, tls } = src else { return Err(Error::Invalid("not a database".into())) };
    let c = Conn { host, port: *port, database, user, password, tls: *tls };
    let sql = select_sql(sql, if *kind == DbKind::MySql { '`' } else { '"' });
    read_only_sql(&sql, *kind)?;
    if let Some(bridge) = BRIDGE.get() {
        let Source::Database { kind, host, port, database, user, tls, .. } = src else { unreachable!() };
        let s = Source::Database { kind: *kind, host: host.clone(), port: *port, database: database.clone(), user: user.clone(), sql, tls: *tls };
        return bridge(&s, password);
    }
    match kind {
        DbKind::Postgres => postgres(&c, &sql),
        DbKind::MySql => mysql(&c, &sql),
        DbKind::SqlServer => {
            let l = super::sqlserver::Login { host, port: *port, database, user, password, tls: *tls };
            super::sqlserver::query(&l, &sql)
        }
    }
}

/// A table of an Access or dBase file (the first one when `table` is
/// empty).
pub fn db_file(path: &Path, table: &str) -> Result<DataTable, Error> {
    use apex_db::Value;
    let err = |e: apex_db::Error| match e {
        apex_db::Error::Io(e) => Error::Io(e),
        e => Error::Database(e.to_string()),
    };
    let name = if table.is_empty() { apex_db::table_names(path).map_err(err)?.into_iter().next().unwrap_or_default() } else { table.to_string() };
    let t = apex_db::read_table(path, &name).map_err(err)?;
    let mut out = DataTable::new(crate::table::unique_names(t.columns.iter().map(|c| c.name.clone()).collect()));
    out.rows = t
        .rows
        .into_iter()
        .map(|r| {
            r.into_iter()
                .map(|v| match v {
                    Value::Null => Val::Null,
                    Value::Bool(b) => Val::Bool(b),
                    Value::Int(i) => Val::Num(i as f64),
                    Value::Num(n) => Val::Num(n),
                    Value::Money(m) => Val::Num(m as f64 / 10_000.0),
                    Value::Decimal(s) => s.parse().map(Val::Num).unwrap_or(Val::Text(s)),
                    Value::Date(d) => Val::Date(d),
                    v => Val::Text(v.to_text()),
                })
                .collect()
        })
        .collect();
    Ok(out)
}

/// The tables a source's database has, for picking one.
pub fn list_tables(src: &Source, password: &str) -> Result<Vec<String>, Error> {
    if crate::disabled() {
        return Err(Error::Disabled);
    }
    let (t, col) = match src {
        Source::DbFile { path, .. } => return apex_db::table_names(Path::new(path)).map_err(|e| Error::Database(e.to_string())),
        Source::Sqlite { path, .. } => (sqlite(Path::new(path), "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name")?, 0),
        Source::Database { kind: DbKind::Postgres, .. } => {
            let mut s = src.clone();
            if let Source::Database { sql, .. } = &mut s {
                *sql = "SELECT CASE WHEN table_schema = 'public' THEN table_name ELSE table_schema || '.' || table_name END FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY 1".into();
            }
            (server(&s, password)?, 0)
        }
        Source::Database { kind: DbKind::MySql, .. } => {
            let mut s = src.clone();
            if let Source::Database { sql, .. } = &mut s {
                *sql = "SHOW TABLES".into();
            }
            (server(&s, password)?, 0)
        }
        Source::Database { kind: DbKind::SqlServer, .. } => {
            let mut s = src.clone();
            if let Source::Database { sql, .. } = &mut s {
                *sql = "SELECT CASE WHEN TABLE_SCHEMA = 'dbo' THEN TABLE_NAME ELSE TABLE_SCHEMA + '.' + TABLE_NAME END FROM INFORMATION_SCHEMA.TABLES ORDER BY 1".into();
            }
            (server(&s, password)?, 0)
        }
        _ => return Ok(Vec::new()),
    };
    Ok(t.rows.iter().map(|r| r.get(col).map(Val::to_text).unwrap_or_default()).collect())
}

#[cfg_attr(not(all(feature = "postgres", feature = "mysql")), allow(dead_code))]
struct Conn<'a> {
    host: &'a str,
    port: u16,
    database: &'a str,
    user: &'a str,
    password: &'a str,
    tls: bool,
}

#[cfg(any(feature = "postgres", feature = "mysql"))]
const TIMEOUT: web_time::Duration = web_time::Duration::from_secs(20);

#[cfg(feature = "postgres")]
fn postgres(c: &Conn, sql: &str) -> Result<DataTable, Error> {
    use postgres::SimpleQueryMessage;
    let err = |e: postgres::Error| Error::Database(e.to_string());
    let mut cfg = postgres::Config::new();
    cfg.host(c.host).port(c.port).dbname(c.database).user(c.user).password(c.password).connect_timeout(TIMEOUT).application_name("ApexDesk");
    // Every transaction of the session reads only.
    cfg.options("-c default_transaction_read_only=on -c statement_timeout=120000");
    let mut client = if c.tls {
        // Not "prefer": a server (or whoever answers for it) saying no
        // to TLS must not get the password in the clear.
        cfg.ssl_mode(postgres::config::SslMode::Require);
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Database(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        cfg.connect(tokio_postgres_rustls::MakeRustlsConnect::new(config)).map_err(err)?
    } else {
        cfg.connect(postgres::NoTls).map_err(err)?
    };
    // The simple protocol gives every value as text, whatever its type. A
    // query's rows come through a cursor, a batch at a time, so that too
    // many stop the reading (SHOW and EXPLAIN can't be in a cursor).
    let first = sql.trim_start().split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_ascii_uppercase();
    let cursor = sql.trim_start().starts_with('(') || matches!(first.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE");
    if cursor {
        client.simple_query("BEGIN READ ONLY").map_err(err)?;
        client.simple_query(&format!("DECLARE apex_rows NO SCROLL CURSOR FOR {}", sql.trim().trim_end_matches(';'))).map_err(err)?;
    }
    let mut t: Option<DataTable> = None;
    let mut budget = super::Budget::new();
    loop {
        let msgs = client.simple_query(if cursor { "FETCH 10000 FROM apex_rows" } else { sql }).map_err(err)?;
        let mut got = false;
        for m in msgs {
            match m {
                SimpleQueryMessage::RowDescription(cols) if t.is_none() => t = Some(DataTable::new(unique_names(cols.iter().map(|c| c.name().to_string()).collect()))),
                SimpleQueryMessage::Row(row) => {
                    got = true;
                    let tt = t.get_or_insert_with(|| DataTable::new(unique_names(row.columns().iter().map(|c| c.name().to_string()).collect())));
                    if tt.width() != row.len() {
                        continue;
                    }
                    let r: Vec<Val> = (0..row.len()).map(|i| row.get(i).map_or(Val::Null, |s| Val::Text(s.to_string()))).collect();
                    budget.take(&r)?;
                    tt.rows.push(r);
                }
                _ => {}
            }
        }
        if !cursor || !got {
            break;
        }
    }
    if cursor {
        let _ = client.simple_query("ROLLBACK");
    }
    let mut t = t.unwrap_or_default();
    t.detect_types();
    pg_bools(&mut t);
    Ok(t)
}

#[cfg(not(feature = "postgres"))]
fn postgres(_c: &Conn, _sql: &str) -> Result<DataTable, Error> {
    Err(Error::NotBuilt("PostgreSQL"))
}

#[cfg(feature = "mysql")]
fn mysql(c: &Conn, sql: &str) -> Result<DataTable, Error> {
    use mysql::prelude::Queryable;
    let err = |e: mysql::Error| Error::Database(e.to_string());
    let mut opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(c.host))
        .tcp_port(c.port)
        .user(Some(c.user))
        .pass(Some(c.password))
        .db_name(Some(c.database))
        .tcp_connect_timeout(Some(TIMEOUT))
        .read_timeout(Some(TIMEOUT * 15))
        // A server asking for a local file (LOAD DATA LOCAL) gets nothing.
        .local_infile_handler(Some(mysql::LocalInfileHandler::new(|_, _| Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "local files are not sent")))));
    if c.tls {
        opts = opts.ssl_opts(Some(mysql::SslOpts::default()));
    }
    let mut conn = mysql::Conn::new(opts).map_err(err)?;
    conn.query_drop("SET SESSION TRANSACTION READ ONLY").map_err(err)?;
    let mut result = conn.query_iter(sql).map_err(err)?;
    let mut t = DataTable::new(unique_names(result.columns().as_ref().iter().map(|c| c.name_str().into_owned()).collect()));
    let mut budget = super::Budget::new();
    if let Some(set) = result.iter() {
        for row in set {
            let row = row.map_err(err)?;
            let r: Vec<Val> = row.unwrap()
                    .into_iter()
                    .map(|v| match v {
                        mysql::Value::NULL => Val::Null,
                        mysql::Value::Bytes(b) => Val::Text(String::from_utf8_lossy(&b).into_owned()),
                        mysql::Value::Int(i) => Val::Num(i as f64),
                        mysql::Value::UInt(u) => Val::Num(u as f64),
                        mysql::Value::Float(f) => Val::Num(f as f64),
                        mysql::Value::Double(f) => Val::Num(f),
                        mysql::Value::Date(y, mo, d, h, mi, s, _) => Val::Date(apex_sheet::format::date_to_serial(y as i64, mo as i64, d as i64) + (h as f64 * 3600.0 + mi as f64 * 60.0 + s as f64) / 86400.0),
                        v @ mysql::Value::Time(..) => Val::Text(v.as_sql(true).trim_matches('\'').to_string()),
                    })
                    .collect();
            budget.take(&r)?;
            t.rows.push(r);
        }
    }
    t.detect_types();
    Ok(t)
}

#[cfg(not(feature = "mysql"))]
fn mysql(_c: &Conn, _sql: &str) -> Result<DataTable, Error> {
    Err(Error::NotBuilt("MySQL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names() {
        assert_eq!(select_sql("sales", '"'), "SELECT * FROM \"sales\"");
        assert_eq!(select_sql(" shop.sales; ", '`'), "SELECT * FROM `shop`.`sales`");
        assert_eq!(select_sql("SELECT 1", '"'), "SELECT 1");
    }

    #[test]
    fn only_reading_queries() {
        use DbKind::*;
        for (kind, ok) in [
            (Postgres, "SELECT * FROM \"sales\""),
            (Postgres, "select a, b from t where c = 'x; DROP TABLE t' -- ; insert\n order by 1;"),
            (Postgres, "WITH x AS (SELECT 1) SELECT * FROM x"),
            (Postgres, "SELECT $$ ; insert $$, $q$;$q$, $1"),
            (SqlServer, "SELECT \"update\", [delete] FROM t /* ; drop */"),
            (SqlServer, "SELECT REPLACE(name, 'a', 'b') FROM t"),
            (MySql, "SELECT `set` FROM t # ; drop"),
            (MySql, "SHOW TABLES"),
            (MySql, "(SELECT 1) UNION (SELECT 2)"),
            (Postgres, "SELECT lo_orderkey, sleep FROM lineorder"),
            (MySql, "SELECT 1 /* Mostly ! */"),
        ] {
            assert!(read_only_sql(ok, kind).is_ok(), "{ok}");
        }
        for (kind, bad) in [
            (Postgres, "SELECT 1; DROP TABLE t"),
            (SqlServer, "SELECT 1 DELETE FROM t"),
            (Postgres, "DELETE FROM t"),
            (SqlServer, "SELECT * INTO copy FROM t"),
            (Postgres, "SELECT 1; SET default_transaction_read_only = off"),
            (SqlServer, "EXEC xp_cmdshell 'dir'"),
            (MySql, "SELECT 'a'';DROP' ; UPDATE t SET a = 1"),
            (SqlServer, "SELECT * FROM OPENROWSET('x', 'y')"),
            (Postgres, "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"),
            (Postgres, "SELECT 1 /* x */; COMMIT"),
            // A backslash ends the string on SQL Server.
            (SqlServer, "SELECT 'a\\' DELETE FROM t --'"),
            (MySql, "SELECT 'a\\' ; DELETE FROM t; -- '"),
            // "#" is a temporary table there, not a comment.
            (SqlServer, "SELECT * FROM #t DELETE FROM t"),
            (Postgres, "SELECT 1 # 2; DELETE FROM t"),
            // Functions that act beyond reading.
            (MySql, "SELECT LOAD_FILE('/etc/passwd')"),
            (Postgres, "SELECT dblink_exec('dbname=prod', 'DROP TABLE x')"),
            (Postgres, "SELECT public.dblink_exec ('x', 'y')"),
            (Postgres, "SELECT pg_terminate_backend(42)"),
            (Postgres, "SELECT set_config('a', 'b', false)"),
            (Postgres, "SELECT \"dblink_exec\"('x', 'y')"),
            (MySql, "SELECT `load_file`('/etc/passwd')"),
            // Comments MySQL runs, and names spelt in escapes.
            (MySql, "SELECT 1 /*!; SET GLOBAL x = 1 */"),
            (MySql, "SELECT 1 --1; SET SESSION TRANSACTION READ WRITE; DELETE FROM t"),
            (MySql, "SELECT 1 /*M!100000 ; DELETE FROM t */"),
            (Postgres, "SELECT U&\"d\\0062link_exec\"('x', 'y')"),
            (Postgres, "SELECT query_to_xml_and_xmlschema('DELETE FROM t', true, true, '')"),
        ] {
            assert!(read_only_sql(bad, kind).is_err(), "{bad}");
        }
    }
}
