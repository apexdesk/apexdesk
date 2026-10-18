//! Saved queries: Access keeps them in pieces in `MSysQueries`, one row per
//! part (the type, parameters, tables, columns, joins, the WHERE, GROUP BY,
//! HAVING and ORDER BY clauses), all with the query's catalog id. Their
//! SQL is put back together here as Access shows it in its SQL view.

use crate::model::{QueryKind, SavedQuery, Value};

use super::{AccessFile, TYPE_QUERY};

/// Joins of a query read, nested one in the next.
const MAX_JOINS: usize = 256;

#[derive(Clone, Debug, Default)]
struct Row {
    attr: u8,
    expr: Option<String>,
    flag: i32,
    extra: i64,
    name1: Option<String>,
    name2: Option<String>,
}

const A_TYPE: u8 = 1;
const A_PARAM: u8 = 2;
const A_FLAG: u8 = 3;
const A_REMOTE: u8 = 4;
const A_TABLE: u8 = 5;
const A_COLUMN: u8 = 6;
const A_JOIN: u8 = 7;
const A_WHERE: u8 = 8;
const A_GROUP: u8 = 9;
const A_HAVING: u8 = 10;
const A_ORDER: u8 = 11;

pub fn read(f: &AccessFile) -> Vec<SavedQuery> {
    let Some(o) = f.object("MSysQueries") else { return Vec::new() };
    let Ok(def) = f.tdef((o.id & 0x00ff_ffff) as u32, "MSysQueries") else { return Vec::new() };
    let rows = f.rows(&def);
    let ci = |n: &str| def.columns.iter().position(|c| c.col.name.eq_ignore_ascii_case(n));
    let (attr, expr, flag, extra, n1, n2, obj) = (ci("Attribute"), ci("Expression"), ci("Flag"), ci("LvExtra"), ci("Name1"), ci("Name2"), ci("ObjectId"));
    let text = |r: &[Value], c: Option<usize>| c.and_then(|c| r.get(c)).filter(|v| !v.is_null()).map(Value::to_text);
    let int = |r: &[Value], c: Option<usize>| c.and_then(|c| r.get(c)).and_then(Value::as_f64).unwrap_or(0.0) as i64;
    // The rows of each query, found in one pass.
    let mut by_query: std::collections::HashMap<i64, Vec<&Vec<Value>>> = std::collections::HashMap::new();
    for r in &rows {
        by_query.entry(int(r, obj)).or_default().push(r);
    }
    let mut out = Vec::new();
    for q in f.catalog.iter().filter(|o| o.ty == TYPE_QUERY) {
        // Access's own hidden queries (combo box sources) start with "~".
        if q.name.starts_with('~') {
            continue;
        }
        let parts: Vec<Row> = by_query
            .get(&(q.id as i64))
            .into_iter()
            .flatten()
            .map(|r| Row { attr: int(r, attr) as u8, expr: text(r, expr), flag: int(r, flag) as i32, extra: int(r, extra), name1: text(r, n1), name2: text(r, n2) })
            .collect();
        let (kind, sql) = to_sql(q.flags, &parts);
        out.push(SavedQuery { name: q.name.clone(), kind, sql });
    }
    out
}

fn kind_of(obj_flags: i32, rows: &[Row]) -> QueryKind {
    let by_flag = match obj_flags & 0xf0 {
        0 => None,
        80 => Some(QueryKind::MakeTable),
        64 => Some(QueryKind::Append),
        48 => Some(QueryKind::Update),
        32 => Some(QueryKind::Delete),
        16 => Some(QueryKind::Crosstab),
        96 => Some(QueryKind::DataDefinition),
        112 => Some(QueryKind::PassThrough),
        128 => Some(QueryKind::Union),
        _ => Some(QueryKind::Unknown),
    };
    let by_row = rows.iter().find(|r| r.attr == A_TYPE).map(|r| match r.flag {
        1 => QueryKind::Select,
        2 => QueryKind::MakeTable,
        3 => QueryKind::Append,
        4 => QueryKind::Update,
        5 => QueryKind::Delete,
        6 => QueryKind::Crosstab,
        7 => QueryKind::DataDefinition,
        8 => QueryKind::PassThrough,
        9 => QueryKind::Union,
        _ => QueryKind::Unknown,
    });
    by_flag.or(by_row).unwrap_or(QueryKind::Select)
}

/// A query's kind and its SQL.
fn to_sql(obj_flags: i32, rows: &[Row]) -> (QueryKind, String) {
    let kind = kind_of(obj_flags, rows);
    let q = Q { rows };
    let mut s = String::new();
    let standard = !matches!(kind, QueryKind::PassThrough | QueryKind::DataDefinition);
    if standard {
        let params = q.params();
        if !params.is_empty() {
            s.push_str(&format!("PARAMETERS {};\n", params.join(", ")));
        }
    }
    let type_row = q.first(A_TYPE);
    match kind {
        QueryKind::Select | QueryKind::Unknown => q.select(&mut s, true, None, false),
        QueryKind::MakeTable => {
            let mut into = String::from(" INTO ");
            into.push_str(&ident(type_row.name1.as_deref().unwrap_or("")));
            into.push_str(&remote(type_row.name2.as_deref(), type_row.expr.as_deref()));
            q.select(&mut s, true, Some(&into), false);
        }
        QueryKind::Append => {
            s.push_str("INSERT INTO ");
            s.push_str(&ident(type_row.name1.as_deref().unwrap_or("")));
            let targets: Vec<String> = q.all(A_COLUMN).filter_map(|r| r.name2.as_deref().map(ident)).collect();
            if !targets.is_empty() {
                s.push_str(&format!(" ({})", targets.join(", ")));
            }
            s.push_str(&remote(type_row.name2.as_deref(), type_row.expr.as_deref()));
            s.push('\n');
            let values: Vec<String> = q.all(A_COLUMN).filter(|r| r.flag & 0x8000 != 0).filter_map(|r| r.expr.clone()).collect();
            if !values.is_empty() {
                s.push_str(&format!("VALUES ({})", values.join(", ")));
            } else {
                q.select(&mut s, true, None, false);
            }
        }
        QueryKind::Update => {
            s.push_str(&format!("UPDATE {}", q.from().join(", ")));
            let r = q.first(A_REMOTE);
            s.push_str(&remote(r.name1.as_deref(), r.expr.as_deref()));
            let set: Vec<String> = q.all(A_COLUMN).map(|r| format!("{} = {}", ident(r.name2.as_deref().unwrap_or("")), r.expr.clone().unwrap_or_default())).collect();
            s.push_str(&format!("\nSET {}", set.join(", ")));
            if let Some(w) = &q.first(A_WHERE).expr {
                s.push_str(&format!("\nWHERE {w}"));
            }
        }
        QueryKind::Delete => {
            s.push_str("DELETE ");
            q.select(&mut s, false, None, false);
        }
        QueryKind::Crosstab => {
            let transform = q.all(A_COLUMN).find(|r| r.flag & 3 == 0);
            if let Some(t) = transform.and_then(|t| t.expr.as_ref().map(|e| (e, t))) {
                s.push_str(&format!("TRANSFORM {}{}\n", t.0, alias(t.1.name1.as_deref())));
            }
            q.select(&mut s, true, None, true);
            if let Some(p) = q.all(A_COLUMN).find(|r| r.flag & 1 != 0).and_then(|r| r.expr.clone()) {
                s.push_str(&format!("\nPIVOT {p}"));
            }
        }
        QueryKind::Union => {
            let part = |id: &str| q.all(A_TABLE).find(|r| r.name2.as_deref() == Some(id)).and_then(|r| r.expr.clone()).unwrap_or_default();
            s.push_str(part("X7YZ_____1").trim());
            s.push_str("\nUNION ");
            if q.flags() & 0x02 == 0 {
                s.push_str("ALL ");
            }
            s.push_str(part("X7YZ_____2").trim());
            let order = q.order();
            if !order.is_empty() {
                s.push_str(&format!("\nORDER BY {}", order.join(", ")));
            }
        }
        QueryKind::PassThrough | QueryKind::DataDefinition => s.push_str(type_row.expr.as_deref().unwrap_or("")),
    }
    if standard {
        if q.flags() & 0x04 != 0 {
            s.push_str("\nWITH OWNERACCESS OPTION");
        }
        s.push(';');
    }
    (kind, s.replace("\r\n", "\n"))
}

struct Q<'a> {
    rows: &'a [Row],
}

static EMPTY: Row = Row { attr: 0, expr: None, flag: 0, extra: 0, name1: None, name2: None };

impl Q<'_> {
    fn all(&self, a: u8) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(move |r| r.attr == a)
    }

    fn first(&self, a: u8) -> &Row {
        self.all(a).next().unwrap_or(&EMPTY)
    }

    fn flags(&self) -> i32 {
        self.first(A_FLAG).flag
    }

    fn params(&self) -> Vec<String> {
        self.all(A_PARAM)
            .map(|r| {
                let ty = match r.flag {
                    1 => "Bit",
                    2 => "Byte",
                    3 => "Short",
                    4 => "Long",
                    5 => "Currency",
                    6 => "IEEESingle",
                    7 => "IEEEDouble",
                    8 => "DateTime",
                    9 => "Binary",
                    10 => "Text",
                    11 => "LongBinary",
                    15 => "Guid",
                    _ => "Value",
                };
                let size = if r.flag == 10 && r.extra > 0 { format!("({})", r.extra) } else { String::new() };
                format!("{} {ty}{size}", r.name1.clone().unwrap_or_default())
            })
            .collect()
    }

    fn order(&self) -> Vec<String> {
        self.all(A_ORDER)
            .map(|r| {
                let mut s = r.expr.clone().unwrap_or_default();
                if r.name1.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("D")) {
                    s.push_str(" DESC");
                }
                s
            })
            .collect()
    }

    /// The FROM list, joins nested as Access writes them.
    fn from(&self) -> Vec<String> {
        // Each source: the table names (or aliases) it covers, its text,
        // and for joins the ON expressions so far.
        struct Src {
            names: Vec<String>,
            text: String,
            join: Option<(i32, Vec<String>, Box<Src>, Box<Src>)>,
        }
        fn render(s: &Src, top: bool) -> String {
            match &s.join {
                None => s.text.clone(),
                Some((ty, on, a, b)) => {
                    let jt = match ty {
                        2 => " LEFT JOIN ",
                        3 => " RIGHT JOIN ",
                        _ => " INNER JOIN ",
                    };
                    let on = if on.len() > 1 { format!("({})", on.join(") AND (")) } else { on.join("") };
                    let inner = format!("{}{jt}{} ON {on}", render(a, false), render(b, false));
                    if top { inner } else { format!("({inner})") }
                }
            }
        }
        let mut srcs: Vec<Src> = Vec::new();
        for t in self.all(A_TABLE) {
            let mut text = String::new();
            if let Some(e) = &t.expr {
                text.push_str(&quoted(e));
                text.push('.');
            }
            if let Some(n) = &t.name1 {
                text.push_str(&ident(n));
            }
            text.push_str(&alias(t.name2.as_deref()));
            let key = t.name2.clone().or_else(|| t.name1.clone()).unwrap_or_default();
            srcs.push(Src { names: vec![key], text, join: None });
        }
        // Each join nests the ones before it: a query of Access has a few.
        for j in self.all(A_JOIN).take(MAX_JOINS) {
            let (from, to) = (j.name1.clone().unwrap_or_default(), j.name2.clone().unwrap_or_default());
            let has = |s: &Src, n: &str| s.names.iter().any(|x| x.eq_ignore_ascii_case(n));
            let fi = srcs.iter().position(|s| has(s, &from));
            let ti = srcs.iter().position(|s| has(s, &to));
            let on = j.expr.clone().unwrap_or_default();
            match (fi, ti) {
                (Some(a), Some(b)) if a == b => {
                    if let Some((_, ons, _, _)) = &mut srcs[a].join {
                        ons.insert(0, on);
                    }
                }
                _ => {
                    let take = |srcs: &mut Vec<Src>, i: Option<usize>, n: &str| match i {
                        Some(i) => srcs.remove(i),
                        None => Src { names: vec![n.to_string()], text: ident(n), join: None },
                    };
                    // Remove the later one first so the other's place holds.
                    let (a, b) = match (fi, ti) {
                        (Some(a), Some(b)) if a > b => {
                            let a = take(&mut srcs, Some(a), &from);
                            (a, take(&mut srcs, Some(b), &to))
                        }
                        (Some(a), Some(b)) => {
                            let b = take(&mut srcs, Some(b), &to);
                            (take(&mut srcs, Some(a), &from), b)
                        }
                        (a, b) => {
                            let x = take(&mut srcs, a, &from);
                            let b = b.map(|b| if a.is_some_and(|a| a < b) { b - 1 } else { b });
                            (x, take(&mut srcs, b, &to))
                        }
                    };
                    let names = a.names.iter().chain(b.names.iter()).cloned().collect();
                    srcs.push(Src { names, text: String::new(), join: Some((j.flag, vec![on], Box::new(a), Box::new(b))) });
                }
            }
        }
        srcs.iter().map(|s| render(s, true)).collect()
    }

    /// SELECT ... FROM ... WHERE ... GROUP BY ... HAVING ... ORDER BY.
    fn select(&self, s: &mut String, prefix: bool, into: Option<&str>, cross: bool) {
        let flags = self.flags();
        if prefix {
            s.push_str("SELECT ");
            if flags & 0x02 != 0 {
                s.push_str("DISTINCT ");
            } else if flags & 0x08 != 0 {
                s.push_str("DISTINCTROW ");
            } else if flags & 0x10 != 0 {
                s.push_str(&format!("TOP {}", self.first(A_FLAG).name1.clone().unwrap_or_default()));
                if flags & 0x20 != 0 {
                    s.push_str(" PERCENT");
                }
                s.push(' ');
            }
        }
        let mut cols: Vec<String> = self
            .all(A_COLUMN)
            .filter(|r| r.flag & 0x8000 == 0)
            .filter(|r| !cross || r.flag & 2 != 0)
            .filter_map(|r| r.expr.as_ref().map(|e| format!("{e}{}", alias(r.name1.as_deref()))))
            .collect();
        if flags & 0x01 != 0 {
            cols.push("*".into());
        }
        s.push_str(&cols.join(", "));
        if let Some(i) = into {
            s.push_str(i);
        }
        let from = self.from();
        if !from.is_empty() {
            s.push_str(&format!("\nFROM {}", from.join(", ")));
            let r = self.first(A_REMOTE);
            s.push_str(&remote(r.name1.as_deref(), r.expr.as_deref()));
        }
        if let Some(w) = &self.first(A_WHERE).expr {
            s.push_str(&format!("\nWHERE {w}"));
        }
        let groups: Vec<String> = self.all(A_GROUP).filter(|r| !cross || r.flag & 2 != 0).filter_map(|r| r.expr.clone()).collect();
        if !groups.is_empty() {
            s.push_str(&format!("\nGROUP BY {}", groups.join(", ")));
        }
        if let Some(h) = &self.first(A_HAVING).expr {
            s.push_str(&format!("\nHAVING {h}"));
        }
        let order = self.order();
        if !order.is_empty() {
            s.push_str(&format!("\nORDER BY {}", order.join(", ")));
        }
    }
}

/// An identifier in brackets when it has other than letters, digits and
/// "_" ("Order Details" -> "[Order Details]"), part by part.
fn ident(s: &str) -> String {
    s.split('.').map(|p| if p.chars().any(|c| !(c.is_alphanumeric() || c == '_')) { quoted(p) } else { p.to_string() }).collect::<Vec<_>>().join(".")
}

/// `s` in brackets. Access names can't hold brackets, so any inside (a
/// file's trick to end the name early) become "_".
fn quoted(s: &str) -> String {
    let inner = if s.len() >= 2 && s.starts_with('[') && s.ends_with(']') { &s[1..s.len() - 1] } else { s };
    format!("[{}]", inner.replace(['[', ']'], "_"))
}

fn alias(a: Option<&str>) -> String {
    match a {
        Some(a) => {
            let q = if a.chars().any(|c| !(c.is_alphanumeric() || c == '_')) { quoted(a) } else { a.to_string() };
            format!(" AS {q}")
        }
        None => String::new(),
    }
}

fn remote(path: Option<&str>, ty: Option<&str>) -> String {
    if path.is_none() && ty.is_none() {
        return String::new();
    }
    let mut s = format!(" IN '{}'", path.unwrap_or(""));
    if let Some(t) = ty {
        s.push_str(&format!(" [{t}]"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_cant_end_a_name_early() {
        assert_eq!(ident("Order Details"), "[Order Details]");
        assert_eq!(quoted("[Order Details]"), "[Order Details]");
        assert_eq!(quoted("[a]; DROP TABLE t; SELECT [b]"), "[a_; DROP TABLE t; SELECT _b]");
    }
}
