//! Get Data queries in a part of their own (`urn:apexdesk:queries`): a
//! custom XML part of xlsx (Excel keeps it untouched) and
//! `ApexDesk/queries.xml` in ods (listed in the manifest; LibreOffice
//! ignores it). Passwords are never written.

use std::fmt::Write as _;

use apex_sheet::query::{AggFn, Aggregate, ColType, DbKind, FilterOp, JoinKind, Query, Source, Step};

use crate::xml::{Ev, HEADER, Reader, Tag, esc};

pub const NS: &str = "urn:apexdesk:queries";

/// Path of the part in an ods package.
pub(crate) const ODS_PATH: &str = "ApexDesk/queries.xml";

fn b(v: bool) -> &'static str {
    if v { "1" } else { "0" }
}

/// ` k="v"` for each pair.
fn attrs(out: &mut String, list: &[(&str, &str)]) {
    for (k, v) in list {
        let _ = write!(out, r#" {k}="{}""#, esc(v));
    }
}

fn cols(out: &mut String, list: &[String]) {
    for c in list {
        let _ = write!(out, r#"<col name="{}"/>"#, esc(c));
    }
}

fn write_source(out: &mut String, s: &Source) {
    out.push_str("<source");
    match s {
        Source::Text { location, delimiter, encoding, header, skip } => {
            let d = delimiter.map(String::from).unwrap_or_default();
            attrs(out, &[("kind", "text"), ("location", location), ("delimiter", &d), ("encoding", encoding), ("header", b(*header)), ("skip", &skip.to_string())]);
        }
        Source::Workbook { path, item, header } => attrs(out, &[("kind", "workbook"), ("path", path), ("item", item), ("header", b(*header))]),
        Source::Folder { path, extension, header, file_column } => attrs(out, &[("kind", "folder"), ("path", path), ("extension", extension), ("header", b(*header)), ("fileColumn", b(*file_column))]),
        Source::Json { location, path } => attrs(out, &[("kind", "json"), ("location", location), ("path", path)]),
        Source::Xml { location, element } => attrs(out, &[("kind", "xml"), ("location", location), ("element", element)]),
        Source::Html { location, index } => attrs(out, &[("kind", "html"), ("location", location), ("index", &index.to_string())]),
        Source::Pdf { location, index, header } => attrs(out, &[("kind", "pdf"), ("location", location), ("index", &index.to_string()), ("header", b(*header))]),
        Source::Sqlite { path, sql } => attrs(out, &[("kind", "sqlite"), ("path", path), ("sql", sql)]),
        Source::DbFile { path, table } => attrs(out, &[("kind", "dbfile"), ("path", path), ("table", table)]),
        Source::Database { kind, host, port, database, user, sql, tls } => {
            attrs(out, &[("kind", kind.key()), ("host", host), ("port", &port.to_string()), ("database", database), ("user", user), ("sql", sql), ("tls", b(*tls))]);
        }
        Source::Table { name } => attrs(out, &[("kind", "table"), ("name", name)]),
        Source::Query { name } => attrs(out, &[("kind", "query"), ("name", name)]),
    }
    out.push_str("/>");
}

fn write_step(out: &mut String, s: &Step) {
    out.push_str("<step");
    let kind = s.kind_key().trim_start_matches("getdata.step.");
    attrs(out, &[("kind", kind)]);
    let mut body = String::new();
    match s {
        Step::PromoteHeaders => {}
        Step::KeepRows { skip, take } => attrs(out, &[("skip", &skip.to_string()), ("take", &take.to_string())]),
        Step::Filter { column, op, value, case } => attrs(out, &[("column", column), ("op", op.key()), ("value", value), ("case", b(*case))]),
        Step::RemoveColumns(c) | Step::ChooseColumns(c) | Step::RemoveDuplicates(c) => cols(&mut body, c),
        Step::Rename(pairs) => {
            for (a, z) in pairs {
                let _ = write!(body, r#"<col name="{}" to="{}"/>"#, esc(a), esc(z));
            }
        }
        Step::Move { column, to } => attrs(out, &[("column", column), ("to", &to.to_string())]),
        Step::ChangeType { column, ty } => attrs(out, &[("column", column), ("type", ty.key())]),
        Step::Split { column, delimiter } => attrs(out, &[("column", column), ("delimiter", delimiter)]),
        Step::AddColumn { name, formula } => attrs(out, &[("name", name), ("formula", formula)]),
        Step::Replace { column, find, replace, whole } => attrs(out, &[("column", column), ("find", find), ("replace", replace), ("whole", b(*whole))]),
        Step::Sort(keys) => {
            for (c, asc) in keys {
                let _ = write!(body, r#"<col name="{}" asc="{}"/>"#, esc(c), b(*asc));
            }
        }
        Step::Group { by, aggs } => {
            cols(&mut body, by);
            for a in aggs {
                let _ = write!(body, r#"<agg column="{}" func="{}" name="{}"/>"#, esc(&a.column), a.func.key(), esc(&a.name));
            }
        }
        Step::Pivot { column, values, func } => attrs(out, &[("column", column), ("values", values), ("func", func.key())]),
        Step::Unpivot { columns, attribute, value } => {
            attrs(out, &[("attribute", attribute), ("value", value)]);
            cols(&mut body, columns);
        }
        Step::Append { query } => attrs(out, &[("query", query)]),
        Step::Merge { query, keys, join, columns, names } => {
            attrs(out, &[("query", query), ("join", join.key())]);
            for (l, r) in keys {
                let _ = write!(body, r#"<key left="{}" right="{}"/>"#, esc(l), esc(r));
            }
            cols(&mut body, columns);
            for (o, n) in names {
                let _ = write!(body, r#"<as col="{}" name="{}"/>"#, esc(o), esc(n));
            }
        }
    }
    if body.is_empty() {
        out.push_str("/>");
    } else {
        let _ = write!(out, ">{body}</step>");
    }
}

/// The part for `queries`.
pub fn write(queries: &[Query]) -> String {
    let mut out = format!(r#"{HEADER}<queries xmlns="{NS}" version="1">"#);
    for q in queries {
        out.push_str("<query");
        attrs(&mut out, &[("name", &q.name), ("refreshOnOpen", b(q.refresh_on_open))]);
        if let Some(t) = &q.table {
            attrs(&mut out, &[("table", t)]);
        }
        out.push('>');
        write_source(&mut out, &q.source);
        for s in &q.steps {
            write_step(&mut out, s);
        }
        out.push_str("</query>");
    }
    out.push_str("</queries>");
    out
}

/// Whether an XML part is ours.
pub(crate) fn is_ours(xml: &str) -> bool {
    xml.contains(NS) && xml.contains("<queries")
}

fn s(t: &Tag, k: &str) -> String {
    t.attr(k).map(|v| v.into_owned()).unwrap_or_default()
}

fn flag(t: &Tag, k: &str, d: bool) -> bool {
    t.attr(k).map_or(d, |v| v == "1" || v == "true")
}

fn num<T: std::str::FromStr>(t: &Tag, k: &str, d: T) -> T {
    t.attr(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}

fn read_source(t: &Tag) -> Option<Source> {
    Some(match s(t, "kind").as_str() {
        "text" => Source::Text { location: s(t, "location"), delimiter: s(t, "delimiter").chars().next(), encoding: s(t, "encoding"), header: flag(t, "header", true), skip: num(t, "skip", 0) },
        "workbook" => Source::Workbook { path: s(t, "path"), item: s(t, "item"), header: flag(t, "header", true) },
        "folder" => Source::Folder { path: s(t, "path"), extension: s(t, "extension"), header: flag(t, "header", true), file_column: flag(t, "fileColumn", false) },
        "json" => Source::Json { location: s(t, "location"), path: s(t, "path") },
        "xml" => Source::Xml { location: s(t, "location"), element: s(t, "element") },
        "html" => Source::Html { location: s(t, "location"), index: num(t, "index", 0) },
        "pdf" => Source::Pdf { location: s(t, "location"), index: num(t, "index", 0), header: flag(t, "header", true) },
        "sqlite" => Source::Sqlite { path: s(t, "path"), sql: s(t, "sql") },
        "dbfile" => Source::DbFile { path: s(t, "path"), table: s(t, "table") },
        "table" => Source::Table { name: s(t, "name") },
        "query" => Source::Query { name: s(t, "name") },
        k => {
            let kind = DbKind::from_key(k)?;
            Source::Database { kind, host: s(t, "host"), port: num(t, "port", kind.default_port()), database: s(t, "database"), user: s(t, "user"), sql: s(t, "sql"), tls: flag(t, "tls", false) }
        }
    })
}

/// A step from its element and its children (`col`, `agg`, `key`).
fn read_step(t: &Tag, kids: &[(String, Tag)]) -> Option<Step> {
    let names = |w: &str| kids.iter().filter(|(n, _)| n == w).map(|(_, k)| k).collect::<Vec<_>>();
    let col_names = || names("col").iter().map(|k| s(k, "name")).collect::<Vec<_>>();
    Some(match s(t, "kind").as_str() {
        "promote" => Step::PromoteHeaders,
        "keep_rows" => Step::KeepRows { skip: num(t, "skip", 0), take: num(t, "take", 0) },
        "filter" => Step::Filter { column: s(t, "column"), op: FilterOp::from_key(&s(t, "op"))?, value: s(t, "value"), case: flag(t, "case", false) },
        "remove_cols" => Step::RemoveColumns(col_names()),
        "choose_cols" => Step::ChooseColumns(col_names()),
        "dedupe" => Step::RemoveDuplicates(col_names()),
        "rename" => Step::Rename(names("col").iter().map(|k| (s(k, "name"), s(k, "to"))).collect()),
        "move" => Step::Move { column: s(t, "column"), to: num(t, "to", 0) },
        "type" => Step::ChangeType { column: s(t, "column"), ty: ColType::from_key(&s(t, "type"))? },
        "split" => Step::Split { column: s(t, "column"), delimiter: s(t, "delimiter") },
        "add_col" => Step::AddColumn { name: s(t, "name"), formula: s(t, "formula") },
        "replace" => Step::Replace { column: s(t, "column"), find: s(t, "find"), replace: s(t, "replace"), whole: flag(t, "whole", false) },
        "sort" => Step::Sort(names("col").iter().map(|k| (s(k, "name"), flag(k, "asc", true))).collect()),
        "group" => Step::Group { by: col_names(), aggs: names("agg").iter().filter_map(|k| Some(Aggregate { column: s(k, "column"), func: AggFn::from_key(&s(k, "func"))?, name: s(k, "name") })).collect() },
        "pivot" => Step::Pivot { column: s(t, "column"), values: s(t, "values"), func: AggFn::from_key(&s(t, "func")).unwrap_or(AggFn::Sum) },
        "unpivot" => Step::Unpivot { columns: col_names(), attribute: s(t, "attribute"), value: s(t, "value") },
        "append" => Step::Append { query: s(t, "query") },
        "merge" => Step::Merge { query: s(t, "query"), keys: names("key").iter().map(|k| (s(k, "left"), s(k, "right"))).collect(), join: JoinKind::from_key(&s(t, "join")).unwrap_or(JoinKind::Left), columns: col_names(), names: names("as").iter().map(|k| (s(k, "col"), s(k, "name"))).collect() },
        _ => return None,
    })
}

/// The queries of a part (those it can't read are left out).
pub fn read(xml: &str) -> Vec<Query> {
    let mut out = Vec::new();
    let mut cur: Option<Query> = None;
    let mut step: Option<(Tag, Vec<(String, Tag)>)> = None;
    let mut r = Reader::new(xml);
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "query" => {
                let mut q = Query::new(s(&t, "name"), Source::Table { name: String::new() });
                q.refresh_on_open = flag(&t, "refreshOnOpen", false);
                q.table = t.attr("table").map(|v| v.into_owned());
                cur = Some(q);
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "source" => {
                if let (Some(q), Some(src)) = (cur.as_mut(), read_source(&t)) {
                    q.source = src;
                }
            }
            Ev::Empty(t) if t.local() == "step" => {
                if let Some(st) = cur.as_mut().and(read_step(&t, &[])) {
                    cur.as_mut().unwrap().steps.push(st);
                }
            }
            Ev::Start(t) if t.local() == "step" => step = Some((t, Vec::new())),
            Ev::Start(t) | Ev::Empty(t) if step.is_some() => {
                if let Some((_, kids)) = step.as_mut() {
                    kids.push((t.local().to_string(), t));
                }
            }
            Ev::End(n) if crate::xml::local(n) == "step" => {
                if let Some((t, kids)) = step.take()
                    && let (Some(q), Some(st)) = (cur.as_mut(), read_step(&t, &kids))
                {
                    q.steps.push(st);
                }
            }
            Ev::End(n) if crate::xml::local(n) == "query" => {
                if let Some(q) = cur.take().filter(|q| !q.name.is_empty()) {
                    out.push(q);
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut a = Query::new("Sales <2024>", Source::Text { location: "C:\\data\\sales.csv".into(), delimiter: Some(';'), encoding: "windows-1252".into(), header: true, skip: 2 });
        a.table = Some("Sales".into());
        a.refresh_on_open = true;
        a.steps = vec![
            Step::PromoteHeaders,
            Step::KeepRows { skip: 1, take: 10 },
            Step::Filter { column: "Region".into(), op: FilterOp::Contains, value: "North & \"East\"".into(), case: false },
            Step::RemoveColumns(vec!["A".into(), "B".into()]),
            Step::ChooseColumns(vec!["C".into()]),
            Step::Rename(vec![("C".into(), "Cost".into())]),
            Step::Move { column: "Cost".into(), to: 2 },
            Step::ChangeType { column: "Cost".into(), ty: ColType::Number },
            Step::Split { column: "Name".into(), delimiter: ", ".into() },
            Step::AddColumn { name: "Total".into(), formula: "[Price] * [Qty]\n+ 1".into() },
            Step::Replace { column: "Name".into(), find: "a".into(), replace: "b".into(), whole: true },
            Step::RemoveDuplicates(vec![]),
            Step::Sort(vec![("Cost".into(), false), ("Name".into(), true)]),
            Step::Group { by: vec!["Region".into()], aggs: vec![Aggregate { column: "Cost".into(), func: AggFn::Sum, name: "Total".into() }] },
            Step::Pivot { column: "Year".into(), values: "Cost".into(), func: AggFn::Average },
            Step::Unpivot { columns: vec!["2023".into(), "2024".into()], attribute: "Year".into(), value: "Value".into() },
            Step::Append { query: "Other".into() },
            Step::Merge { query: "Regions".into(), keys: vec![("Region".into(), "Code".into())], join: JoinKind::Anti, columns: vec!["Label".into()], names: vec![("Label".into(), "Region name".into())] },
        ];
        let b = Query::new("Db", Source::Database { kind: DbKind::MySql, host: "db.local".into(), port: 3307, database: "shop".into(), user: "ana".into(), sql: "SELECT * FROM t WHERE x < 3".into(), tls: true });
        let c = Query::new("J", Source::Json { location: "https://api.example.com/v1?a=1&b=2".into(), path: "data.items".into() });
        let d = Query::new("P", Source::Pdf { location: "statement.pdf".into(), index: 2, header: false });
        let list = vec![a, b, c, d];
        let xml = write(&list);
        assert!(is_ours(&xml));
        assert_eq!(read(&xml), list);
    }
}
