//! Linked tables: tables of a PostgreSQL, MySQL/MariaDB or SQL Server
//! database shown in this one (read only), a page at a time. The link
//! (server, database, user, table) is kept in the file; the password only
//! for the session.

use apex_db::objects::{Kind, Record};
use apex_db::sqlite::{FieldType, Page};
use apex_db::Value;
use apex_query::Val;
use apex_sheet::query::{DbKind, Source};

use super::DbView;

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub table: String,
    pub tls: bool,
}

impl Default for Link {
    fn default() -> Link {
        Link { kind: DbKind::Postgres, host: "localhost".into(), port: DbKind::Postgres.default_port(), database: String::new(), user: String::new(), table: String::new(), tls: false }
    }
}

impl Link {
    pub fn to_records(&self) -> Vec<Record> {
        let mut r = Record::default();
        r.set("kind", self.kind.key()).set("host", &self.host).set("port", self.port).set("database", &self.database).set("user", &self.user).set("table", &self.table).set_flag("tls", self.tls);
        vec![r]
    }

    pub fn from_records(recs: &[Record]) -> Option<Link> {
        let r = recs.first()?;
        let kind = DbKind::from_key(r.get("kind"))?;
        Some(Link { kind, host: r.get("host").to_string(), port: r.int("port", kind.default_port() as i64) as u16, database: r.get("database").to_string(), user: r.get("user").to_string(), table: r.get("table").to_string(), tls: r.flag("tls") })
    }

    /// Where the link connects, as `kind://user@host:port/database`.
    pub fn endpoint(&self) -> String {
        apex_query::password_key(&self.source("")).unwrap_or_default()
    }

    /// What a password typed for this link is kept under: the server and
    /// account it went to, so a link the file changes afterwards (a trigger
    /// can) asks again instead of sending it elsewhere.
    pub fn password_key(&self) -> String {
        format!("{}{}", self.endpoint(), if self.tls { " tls" } else { "" })
    }

    /// A Get Data source running `sql` on the link's server.
    pub fn source(&self, sql: &str) -> Source {
        Source::Database { kind: self.kind, host: self.host.clone(), port: self.port, database: self.database.clone(), user: self.user.clone(), sql: sql.to_string(), tls: self.tls }
    }

    fn quote(&self) -> char {
        if self.kind == DbKind::MySql { '`' } else { '"' }
    }

    /// The table's name quoted for its server.
    pub fn table_sql(&self) -> String {
        self.table.split('.').map(|p| self.ident(p)).collect::<Vec<_>>().join(".")
    }

    /// A name (a column's comes from the server) quoted for the server.
    fn ident(&self, name: &str) -> String {
        let q = self.quote();
        format!("{q}{}{q}", name.replace(q, &format!("{q}{q}")))
    }

    /// The SELECT of `limit` rows from `offset` (0: all of them).
    pub fn page_sql(&self, sort: &[(String, bool)], offset: usize, limit: usize) -> String {
        let order: Vec<String> = sort.iter().map(|(c, a)| format!("{} {}", self.ident(c), if *a { "ASC" } else { "DESC" })).collect();
        let mut sql = format!("SELECT * FROM {}", self.table_sql());
        if self.kind == DbKind::SqlServer {
            if limit > 0 {
                sql.push_str(&if order.is_empty() { " ORDER BY (SELECT NULL)".to_string() } else { format!(" ORDER BY {}", order.join(", ")) });
                sql.push_str(&format!(" OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY"));
            } else if !order.is_empty() {
                sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
            }
        } else {
            if !order.is_empty() {
                sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
            }
            if limit > 0 {
                sql.push_str(&format!(" LIMIT {limit} OFFSET {offset}"));
            }
        }
        sql
    }
}

pub fn load(v: &DbView, name: &str) -> Option<Link> {
    Link::from_records(&apex_db::objects::read(&v.db.load_object(Kind::Link, name)?))
}

pub fn value(v: &Val) -> Value {
    match v {
        Val::Null => Value::Null,
        Val::Num(n) => Value::Num(*n),
        Val::Text(s) => Value::Text(s.clone()),
        Val::Bool(b) => Value::Bool(*b),
        Val::Date(d) => Value::Date(*d),
    }
}

/// Runs `sql` on the link's server.
pub fn run(link: &Link, password: &str, sql: &str) -> Result<Page, String> {
    let t = apex_query::sources::db::server(&link.source(sql), password).map_err(|e| e.to_string())?;
    let types = t.rows.first().map(|r| r.iter().map(|x| match x {
        Val::Num(_) => FieldType::Number,
        Val::Bool(_) => FieldType::Bool,
        Val::Date(_) => FieldType::DateTime,
        _ => FieldType::Text,
    }).collect()).unwrap_or_else(|| vec![FieldType::Text; t.columns.len()]);
    Ok(Page { columns: t.columns.clone(), types, rowids: Vec::new(), rows: t.rows.iter().map(|r| r.iter().map(value).collect()).collect() })
}

/// A page of linked table `name`.
pub fn page(v: &DbView, name: &str, sort: &[(String, bool)], offset: usize, limit: usize) -> Result<Page, String> {
    let link = load(v, name).ok_or_else(|| format!("{name}?"))?;
    let pw = v.passwords.get(&link.password_key()).cloned().unwrap_or_default();
    run(&link, &pw, &link.page_sql(sort, offset, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_names_the_server() {
        let l = Link { kind: DbKind::Postgres, host: "evil.example".into(), port: 5433, database: "erp".into(), user: "ana".into(), table: "t".into(), tls: false };
        let e = l.endpoint();
        assert!(e.contains("ana@evil.example:5433/erp"), "{e}");
    }

    #[test]
    fn page_sql() {
        let mut l = Link { table: "sales.orders".into(), ..Default::default() };
        assert_eq!(l.page_sql(&[("id".into(), false)], 100, 50), "SELECT * FROM \"sales\".\"orders\" ORDER BY \"id\" DESC LIMIT 50 OFFSET 100");
        l.kind = DbKind::SqlServer;
        assert_eq!(l.page_sql(&[], 0, 10), "SELECT * FROM \"sales\".\"orders\" ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 10 ROWS ONLY");
        l.kind = DbKind::MySql;
        l.table = "t".into();
        assert_eq!(l.page_sql(&[], 0, 0), "SELECT * FROM `t`");
        // A column name from the server can't end its quotes.
        assert_eq!(l.page_sql(&[("a`; DROP TABLE t; --".into(), true)], 0, 0), "SELECT * FROM `t` ORDER BY `a``; DROP TABLE t; --` ASC");
        assert_eq!(Link::from_records(&l.to_records()), Some(l));
    }
}
