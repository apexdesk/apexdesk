//! Mail merge data from outside a spreadsheet: a database (SQLite file,
//! PostgreSQL, MySQL/MariaDB or SQL Server server) or a Get Data query of
//! a workbook, as field names and records of text.

use std::path::Path;

use apex_query::table::DataTable;
use apex_sheet::query::{DbKind, Source};

/// Field names and records.
pub type Records = (Vec<String>, Vec<Vec<String>>);

fn records_of(t: DataTable) -> Records {
    let rows = t.rows.iter().map(|r| r.iter().map(|v| v.to_text()).collect()).collect();
    (t.columns, rows)
}

fn error_text(e: apex_query::Error) -> String {
    let (key, arg) = e.key();
    crate::i18n::trf(key, &[("x", &arg)])
}

/// The records a database source gives (`password` for servers).
pub fn from_source(src: &Source, password: &str) -> Result<Records, String> {
    let mut ctx = apex_query::Context::new(Vec::new(), None);
    if let Some(key) = apex_query::password_key(src) {
        ctx.passwords.insert(key, password.to_string());
    }
    apex_query::load_source(src, &ctx).map(records_of).map_err(error_text)
}

/// The files of this computer the workbook's queries read (any of them
/// may be one the chosen query builds on): a received workbook could load
/// a private file of the user's (`../.ssh/id_rsa`) into the letters.
fn local_reads(queries: &[apex_sheet::query::Query]) -> Option<String> {
    let v: Vec<String> = queries.iter().filter(|q| !q.source.is_remote() && !matches!(q.source, Source::Table { .. } | Source::Query { .. })).map(|q| q.source.describe()).collect();
    (!v.is_empty()).then(|| v.join("; "))
}

/// What running query `q` reaches that the user says yes to first: the
/// place it connects to, else the local files the workbook's queries read.
fn reaches(q: &apex_sheet::query::Query, queries: &[apex_sheet::query::Query]) -> Option<String> {
    if q.source.is_remote() { Some(q.source.describe()) } else { local_reads(queries) }
}

/// The Get Data queries of a workbook, each with what it reaches that the
/// user says yes to first (see [`from_query`]).
pub fn query_names(path: &Path) -> Result<Vec<(String, Option<String>)>, String> {
    let (loaded, _) = apex_io::open(path).map_err(|e| e.to_string())?;
    let apex_io::Loaded::Sheet(wb) = loaded else { return Err(crate::i18n::tr("mail.not_sheet").to_string()) };
    Ok(wb.queries.iter().map(|q| (q.name.clone(), reaches(q, &wb.queries))).collect())
}

/// The records of query `name` of the workbook at `path`, run now
/// (`password` for a database it reads). One that connects outside this
/// computer runs only when `allow` says yes to where it connects (the
/// place the user saw: the file may have changed since).
pub fn from_query(path: &Path, name: &str, password: &str, allow: &dyn Fn(&str) -> bool) -> Result<Records, String> {
    let (loaded, _) = apex_io::open(path).map_err(|e| e.to_string())?;
    let apex_io::Loaded::Sheet(wb) = loaded else { return Err(crate::i18n::tr("mail.not_sheet").to_string()) };
    let q = wb.queries.iter().find(|q| q.name.eq_ignore_ascii_case(name)).cloned().ok_or_else(|| error_text(apex_query::Error::NoQuery(name.to_string())))?;
    if let Some(dest) = reaches(&q, &wb.queries)
        && !allow(&dest)
    {
        return Err(crate::i18n::trf("mail.query_target", &[("name", &q.name), ("dest", &dest)]));
    }
    // The workbook may be anyone's: the others it builds on run without
    // the bar Get Data asks with, so none may reach out, and the password
    // goes to the server of the query chosen only.
    if let Some(r) = wb.queries.iter().find(|r| !r.name.eq_ignore_ascii_case(name) && r.source.is_remote()) {
        return Err(crate::i18n::trf("mail.query_remote", &[("name", &r.name)]));
    }
    let mut ctx = apex_query::Context::for_workbook(&wb, wb.queries.clone(), path.parent().map(Path::to_path_buf));
    if let Some(key) = apex_query::password_key(&q.source) {
        ctx.passwords.insert(key, password.to_string());
    }
    apex_query::run(&q, &ctx).map(records_of).map_err(error_text)
}

/// A database named on the command line: a SQLite file, or a server as
/// `postgres://user@host:port/database` (also `mysql://`, `sqlserver://`;
/// the port may be left out).
pub fn parse_database(s: &str, sql: &str) -> Result<Source, String> {
    let Some((scheme, rest)) = s.split_once("://") else { return Ok(Source::Sqlite { path: s.to_string(), sql: sql.to_string() }) };
    let kind = match scheme.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" => DbKind::Postgres,
        "mysql" | "mariadb" => DbKind::MySql,
        "sqlserver" | "mssql" => DbKind::SqlServer,
        "sqlite" | "file" => return Ok(Source::Sqlite { path: rest.to_string(), sql: sql.to_string() }),
        _ => return Err(format!("{s}: unknown database (use postgres://, mysql://, sqlserver:// or a SQLite file)")),
    };
    let (user, rest) = match rest.split_once('@') {
        Some((u, r)) => (u.to_string(), r),
        None => (String::new(), rest),
    };
    let (addr, database) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = match addr.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("{s}: bad port"))?),
        None => (addr, kind.default_port()),
    };
    if host.is_empty() {
        return Err(format!("{s}: no server"));
    }
    Ok(Source::Database { kind, host: host.to_string(), port, database: database.to_string(), user, sql: sql.to_string(), tls: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_names() {
        assert_eq!(parse_database("clients.db", "t").unwrap(), Source::Sqlite { path: "clients.db".into(), sql: "t".into() });
        let s = parse_database("sqlserver://ana@sql.local/crm", "Clients").unwrap();
        assert_eq!(s, Source::Database { kind: DbKind::SqlServer, host: "sql.local".into(), port: 1433, database: "crm".into(), user: "ana".into(), sql: "Clients".into(), tls: false });
        let s = parse_database("postgres://h:6543/x", "").unwrap();
        assert!(matches!(s, Source::Database { kind: DbKind::Postgres, port: 6543, .. }));
        assert!(parse_database("oracle://h/x", "").is_err());
    }

    #[test]
    fn sqlite_records() {
        let dir = std::env::temp_dir().join(format!("apexdesk-merge-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("people.db");
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch("CREATE TABLE people (Name TEXT, City TEXT); INSERT INTO people VALUES ('Ana', 'Lugo'), ('Luis', 'Vigo');").unwrap();
        }
        let src = parse_database(&db.display().to_string(), "people").unwrap();
        let (fields, records) = from_source(&src, "").unwrap();
        assert_eq!(fields, ["Name", "City"]);
        assert_eq!(records, [["Ana", "Lugo"], ["Luis", "Vigo"]]);
        // A workbook whose query reads it.
        let mut wb = apex_sheet::Workbook::new();
        wb.queries.push(apex_sheet::query::Query::new("People", src));
        let book = dir.join("query.xlsx");
        apex_io::save_sheet(&wb, &book, apex_io::Format::Xlsx).unwrap();
        let reads = db.display().to_string();
        assert_eq!(query_names(&book).unwrap(), [("People".to_string(), Some(reads.clone()))]);
        assert!(from_query(&book, "people", "", &|_| false).is_err());
        assert_eq!(from_query(&book, "people", "", &|d| d == reads).unwrap().1.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
