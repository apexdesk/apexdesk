//! The JSON a database query and its table travel in between ApexDesk in
//! the browser and `apexdesk serve`, which reaches the database for it
//! (`POST api/db`).

use apex_query::table::{DataTable, Val};
use apex_sheet::query::{DbKind, Source};
use serde_json::{Value, json};

/// A database source and its password, as the page sends them.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn request(src: &Source, password: &str) -> Option<String> {
    let Source::Database { kind, host, port, database, user, sql, tls } = src else { return None };
    Some(json!({ "kind": kind.key(), "host": host, "port": port, "database": database, "user": user, "password": password, "sql": sql, "tls": tls }).to_string())
}

/// The source and password of a request.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn parse_request(body: &[u8]) -> Option<(Source, String)> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let kind = DbKind::from_key(&s("kind")?)?;
    let port = v.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(kind.default_port());
    let src = Source::Database { kind, host: s("host")?, port, database: s("database").unwrap_or_default(), user: s("user").unwrap_or_default(), sql: s("sql")?, tls: v.get("tls").and_then(Value::as_bool).unwrap_or(false) };
    Some((src, s("password").unwrap_or_default()))
}

/// A table as the server answers: its column names and rows, dates as
/// `{"date": serial}`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn table(t: &DataTable) -> String {
    let val = |v: &Val| match v {
        Val::Null => Value::Null,
        Val::Num(n) => json!(n),
        Val::Text(s) => json!(s),
        Val::Bool(b) => json!(b),
        Val::Date(d) => json!({ "date": d }),
    };
    json!({ "columns": t.columns, "rows": t.rows.iter().map(|r| r.iter().map(val).collect::<Vec<_>>()).collect::<Vec<_>>() }).to_string()
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn parse_table(body: &str) -> Option<DataTable> {
    let v: Value = serde_json::from_str(body).ok()?;
    let columns = v.get("columns")?.as_array()?.iter().map(|c| c.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?;
    let mut t = DataTable::new(columns);
    for r in v.get("rows")?.as_array()? {
        let row = r
            .as_array()?
            .iter()
            .map(|c| match c {
                Value::Null => Val::Null,
                Value::Bool(b) => Val::Bool(*b),
                Value::Number(n) => Val::Num(n.as_f64().unwrap_or(0.0)),
                Value::String(s) => Val::Text(s.clone()),
                v => Val::Date(v.get("date").and_then(Value::as_f64).unwrap_or(0.0)),
            })
            .collect::<Vec<_>>();
        if row.len() != t.columns.len() {
            return None;
        }
        t.rows.push(row);
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let src = Source::Database { kind: DbKind::MySql, host: "db".into(), port: 3307, database: "shop".into(), user: "ana".into(), sql: "SELECT 1".into(), tls: true };
        let (back, pw) = parse_request(request(&src, "pw").unwrap().as_bytes()).unwrap();
        assert_eq!((back, pw.as_str()), (src, "pw"));
        let mut t = DataTable::new(vec!["a".into(), "b".into()]);
        t.rows.push(vec![Val::Num(1.5), Val::Date(45000.25)]);
        t.rows.push(vec![Val::Null, Val::Text("x".into())]);
        let back = parse_table(&table(&t)).unwrap();
        assert_eq!(back.columns, t.columns);
        assert_eq!(back.rows, t.rows);
    }
}
