//! Microsoft SQL Server (tiberius, a TDS client in Rust, on a small tokio
//! runtime). The login is a SQL Server user or, on Windows with no user
//! given, the Windows session (integrated authentication, NTLM or
//! Kerberos through SSPI), or another domain account (`DOMAIN\user`)
//! with its password. The whole session is always encrypted: with TLS
//! the server's certificate must be valid for the host (system roots);
//! without it the certificate isn't checked (servers on an office
//! network often have one of their own), which keeps the password and
//! the data from whoever only listens on the network.

use crate::Error;
use crate::table::DataTable;

/// A connection's settings.
pub struct Login<'a> {
    pub host: &'a str,
    pub port: u16,
    pub database: &'a str,
    /// Empty: the Windows session (Windows only).
    pub user: &'a str,
    pub password: &'a str,
    /// Verify the server's certificate (the session is encrypted either
    /// way).
    pub tls: bool,
}

/// Whether an empty user logs in as the Windows session.
pub const INTEGRATED: bool = cfg!(all(windows, feature = "sqlserver"));

/// Whether `user` names a domain account (`DOMAIN\user`), which signs in
/// with Windows authentication rather than as a SQL Server user.
pub fn domain_user(user: &str) -> bool {
    user.trim().split_once('\\').is_some_and(|(d, n)| !d.is_empty() && !n.is_empty())
}

#[cfg(feature = "sqlserver")]
pub fn query(l: &Login, sql: &str) -> Result<DataTable, Error> {
    use tiberius::{AuthMethod, Client, Config, EncryptionLevel};
    use tokio_util::compat::TokioAsyncWriteCompatExt;
    let err = |e: tiberius::error::Error| Error::Database(e.to_string());
    let mut cfg = Config::new();
    cfg.host(l.host);
    cfg.port(l.port);
    if !l.database.is_empty() {
        cfg.database(l.database);
    }
    cfg.application_name("ApexDesk");
    // ApplicationIntent=ReadOnly: read-only replicas take the session.
    cfg.readonly(true);
    cfg.encryption(EncryptionLevel::Required);
    if !l.tls {
        cfg.trust_cert();
    }
    if l.user.is_empty() {
        #[cfg(windows)]
        cfg.authentication(AuthMethod::Integrated);
        #[cfg(not(windows))]
        return Err(Error::Database("a user name is needed (signing in with the Windows session works on Windows only)".into()));
    } else if domain_user(l.user) {
        // DOMAIN\user: that account through SSPI (Kerberos or NTLM).
        #[cfg(windows)]
        cfg.authentication(AuthMethod::windows(l.user, l.password));
        // tiberius does it elsewhere only with sspi-rs, a large stack.
        #[cfg(not(windows))]
        return Err(Error::Database("signing in with a domain account works on Windows only".into()));
    } else {
        cfg.authentication(AuthMethod::sql_server(l.user, l.password));
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| Error::Io(e.to_string()))?;
    rt.block_on(async {
        let addr = cfg.get_addr();
        let tcp = tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect(&addr))
            .await
            .map_err(|_| Error::Database(format!("{addr}: no answer")))?
            .map_err(|e| Error::Database(format!("{addr}: {e}")))?;
        tcp.set_nodelay(true).map_err(|e| Error::Io(e.to_string()))?;
        let mut client = Client::connect(cfg, tcp.compat_write()).await.map_err(err)?;
        // The query has passed the read-only check; it also runs in a
        // transaction that is always rolled back, so nothing it might
        // change stays changed.
        client.simple_query("BEGIN TRANSACTION").await.map_err(err)?.into_results().await.map_err(err)?;
        let read = async {
            use futures_util::TryStreamExt;
            let mut stream = client.simple_query(sql).await.map_err(err)?;
            let names: Vec<String> = stream.columns().await.map_err(err)?.map(|cols| cols.iter().map(|c| c.name().to_string()).collect()).unwrap_or_default();
            // The first result's rows, while they fit.
            let (mut rows, mut budget, mut sets) = (Vec::new(), super::Budget::new(), 0);
            while let Some(item) = tokio::time::timeout(TIMEOUT * 6, stream.try_next()).await.map_err(|_| Error::Database("no answer".into()))?.map_err(err)? {
                match item {
                    tiberius::QueryItem::Metadata(_) => {
                        sets += 1;
                        if sets > 1 {
                            break;
                        }
                    }
                    tiberius::QueryItem::Row(r) => {
                        let row: Vec<_> = r.cells().map(|(_, v)| value(v)).collect();
                        budget.take(&row)?;
                        rows.push(row);
                    }
                }
            }
            Ok::<_, Error>((names, rows))
        }
        .await;
        // Closing the connection would roll back too; this is explicit.
        if let Ok(s) = client.simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await {
            let _ = s.into_results().await;
        }
        let (names, rows) = read?;
        let mut t = DataTable::new(crate::table::unique_names(names));
        t.rows = rows;
        t.square();
        Ok(t)
    })
}

#[cfg(feature = "sqlserver")]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg(not(feature = "sqlserver"))]
pub fn query(_l: &Login, _sql: &str) -> Result<DataTable, Error> {
    Err(Error::NotBuilt("SQL Server"))
}

/// A cell's value.
#[cfg(feature = "sqlserver")]
fn value(v: &tiberius::ColumnData<'static>) -> crate::table::Val {
    use crate::table::Val;
    use tiberius::ColumnData as C;
    let num = |n: Option<f64>| n.map_or(Val::Null, Val::Num);
    match v {
        C::U8(n) => num(n.map(f64::from)),
        C::I16(n) => num(n.map(f64::from)),
        C::I32(n) => num(n.map(f64::from)),
        C::I64(n) => num(n.map(|n| n as f64)),
        C::F32(n) => num(n.map(f64::from)),
        C::F64(n) => num(*n),
        C::Bit(b) => b.map_or(Val::Null, Val::Bool),
        C::String(s) => s.as_ref().map_or(Val::Null, |s| Val::Text(s.to_string())),
        C::Guid(g) => g.map_or(Val::Null, |g| Val::Text(g.to_string())),
        C::Binary(b) => b.as_ref().map_or(Val::Null, |b| Val::Text(format!("({} bytes)", b.len()))),
        C::Numeric(n) => num(n.map(|n| n.value() as f64 / 10f64.powi(n.scale() as i32))),
        C::Xml(x) => x.as_ref().map_or(Val::Null, |x| Val::Text(x.to_string())),
        C::DateTime(d) => d.map_or(Val::Null, |d| Val::Date(serial_1900(d.days() as i64) + d.seconds_fragments() as f64 / 300.0 / 86_400.0)),
        C::SmallDateTime(d) => d.map_or(Val::Null, |d| Val::Date(serial_1900(d.days() as i64) + d.seconds_fragments() as f64 / 1440.0)),
        C::Time(t) => t.map_or(Val::Null, |t| Val::Num(day_part(t))),
        C::Date(d) => d.map_or(Val::Null, |d| Val::Date(serial_0001(d.days() as i64))),
        C::DateTime2(d) => d.map_or(Val::Null, |d| Val::Date(serial_0001(d.date().days() as i64) + day_part(d.time()))),
        C::DateTimeOffset(d) => d.map_or(Val::Null, |d| {
            let d2 = d.datetime2();
            // Stored in UTC: shown at the offset it was written with.
            Val::Date(serial_0001(d2.date().days() as i64) + day_part(d2.time()) + d.offset() as f64 / 1440.0)
        }),
    }
}

/// The spreadsheet serial of `days` after 1900-01-01 (serials count from
/// 1899-12-30, as Excel's do from March 1900 on).
#[cfg(feature = "sqlserver")]
fn serial_1900(days: i64) -> f64 {
    days as f64 + 2.0
}

/// The spreadsheet serial of `days` after 0001-01-01.
#[cfg(feature = "sqlserver")]
fn serial_0001(days: i64) -> f64 {
    // 0001-01-01 is 693,595 days before 1900-01-01 (proleptic Gregorian).
    serial_1900(days - 693_595)
}

/// A time of day as a fraction of the day.
#[cfg(feature = "sqlserver")]
fn day_part(t: tiberius::time::Time) -> f64 {
    t.increments() as f64 / 10f64.powi(t.scale() as i32) / 86_400.0
}

#[cfg(all(test, feature = "sqlserver"))]
mod tests {
    use super::*;
    use crate::table::Val;

    #[test]
    fn values() {
        use tiberius::ColumnData as C;
        use tiberius::time::{Date, DateTime, DateTime2, Time};
        let jan1 = apex_sheet::format::date_to_serial(2024, 1, 1);
        // 2024-01-01 is 45,290 days after 1900-01-01 and 738,885 after 0001-01-01.
        assert!(matches!(value(&C::DateTime(Some(DateTime::new(45_290, 300 * 3600)))), Val::Date(d) if (d - jan1 - 1.0 / 24.0).abs() < 1e-9));
        assert!(matches!(value(&C::Date(Some(Date::new(738_885)))), Val::Date(d) if d == jan1));
        assert!(matches!(value(&C::DateTime2(Some(DateTime2::new(Date::new(738_885), Time::new(432_000_000, 4))))), Val::Date(d) if (d - jan1 - 0.5).abs() < 1e-9));
        assert!(matches!(value(&C::I32(Some(7))), Val::Num(n) if n == 7.0));
        assert!(matches!(value(&C::String(None)), Val::Null));
        assert!(matches!(value(&C::Numeric(Some(tiberius::numeric::Numeric::new_with_scale(12345, 2)))), Val::Num(n) if (n - 123.45).abs() < 1e-9));
    }

    #[test]
    #[cfg(not(windows))]
    fn needs_a_user() {
        let l = Login { host: "127.0.0.1", port: 1, database: "", user: "", password: "", tls: false };
        assert!(matches!(query(&l, "SELECT 1"), Err(Error::Database(m)) if m.contains("user")));
        let l = Login { user: "CORP\\ana", password: "x", ..l };
        assert!(matches!(query(&l, "SELECT 1"), Err(Error::Database(m)) if m.contains("domain")));
    }

    #[test]
    fn domain_accounts() {
        assert!(domain_user("CORP\\ana"));
        assert!(!domain_user("sa"));
        assert!(!domain_user("\\ana"));
        assert!(!domain_user("ana@corp.example"));
    }
}
