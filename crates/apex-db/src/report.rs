//! Reports: rows of a table or query grouped, sorted and totalled into
//! lines (title, column headings, group headers and footers with counts,
//! sums and averages, details, grand totals and page breaks) that the tab
//! lays out on pages for the preview, the printer and PDF.

use crate::model::{fmt_num, Value};
use crate::objects::Record;

/// What a column adds up in group footers and at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Agg {
    #[default]
    None,
    Sum,
    Avg,
    Count,
    Min,
    Max,
}

impl Agg {
    pub const ALL: [Agg; 6] = [Agg::None, Agg::Sum, Agg::Avg, Agg::Count, Agg::Min, Agg::Max];

    pub fn key(self) -> &'static str {
        match self {
            Agg::None => "none",
            Agg::Sum => "sum",
            Agg::Avg => "avg",
            Agg::Count => "count",
            Agg::Min => "min",
            Agg::Max => "max",
        }
    }

    pub fn from_key(k: &str) -> Agg {
        Agg::ALL.into_iter().find(|a| a.key() == k).unwrap_or_default()
    }
}

/// A column of the report.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RField {
    pub name: String,
    /// The heading (the name when empty).
    pub label: String,
    /// Width in millimetres (0: shared out).
    pub width: f32,
    pub total: Agg,
}

impl RField {
    pub fn heading(&self) -> &str {
        if self.label.is_empty() { &self.name } else { &self.label }
    }
}

/// A grouping level.
#[derive(Clone, Debug, PartialEq)]
pub struct RGroup {
    pub field: String,
    pub asc: bool,
    pub header: bool,
    /// Its footer, with the count and the columns' totals.
    pub footer: bool,
    /// Each group starts on a new page.
    pub page_break: bool,
}

impl Default for RGroup {
    fn default() -> RGroup {
        RGroup { field: String::new(), asc: true, header: true, footer: true, page_break: false }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ReportDef {
    pub title: String,
    /// A table or a saved query.
    pub source: String,
    pub fields: Vec<RField>,
    pub groups: Vec<RGroup>,
    /// Sorting inside the groups (field, ascending).
    pub sort: Vec<(String, bool)>,
    /// Page header and footer; `&P` is the page number, `&N` the number of
    /// pages, `&D` the date.
    pub page_header: String,
    pub page_footer: String,
    /// Totals of the whole report at its end.
    pub grand_total: bool,
    pub landscape: bool,
}

impl ReportDef {
    pub fn to_records(&self) -> Vec<Record> {
        let mut head = Record::default();
        head.set("title", &self.title).set("source", &self.source).set("page_header", &self.page_header).set("page_footer", &self.page_footer).set_flag("grand_total", self.grand_total).set_flag("landscape", self.landscape);
        head.set("sort", self.sort.iter().map(|(f, a)| format!("{}{f}", if *a { "+" } else { "-" })).collect::<Vec<_>>().join("\t"));
        let mut out = vec![head];
        for g in &self.groups {
            let mut r = Record::new("group");
            r.set("field", &g.field).set_flag("asc", g.asc).set_flag("header", g.header).set_flag("footer", g.footer).set_flag("page_break", g.page_break);
            out.push(r);
        }
        for f in &self.fields {
            let mut r = Record::new("field");
            r.set("name", &f.name).set("label", &f.label).set("width", f.width).set("total", f.total.key());
            out.push(r);
        }
        out
    }

    pub fn from_records(recs: &[Record]) -> ReportDef {
        let head = recs.first().cloned().unwrap_or_default();
        let mut d = ReportDef {
            title: head.get("title").to_string(),
            source: head.get("source").to_string(),
            page_header: head.get("page_header").to_string(),
            page_footer: head.get("page_footer").to_string(),
            grand_total: head.flag("grand_total"),
            landscape: head.flag("landscape"),
            sort: head.get("sort").split('\t').filter_map(|s| Some((s.strip_prefix(['+', '-'])?.to_string(), s.starts_with('+')))).filter(|s| !s.0.is_empty()).collect(),
            ..Default::default()
        };
        for r in recs {
            match r.section.as_str() {
                // A few levels of groups and a page's worth of fields:
                // the layout keeps totals for each of them by each.
                "group" if d.groups.len() >= 16 => {}
                "field" if d.fields.len() >= 256 => {}
                "group" => d.groups.push(RGroup { field: r.get("field").to_string(), asc: r.flag("asc"), header: r.flag("header"), footer: r.flag("footer"), page_break: r.flag("page_break") }),
                "field" => d.fields.push(RField { name: r.get("name").to_string(), label: r.get("label").to_string(), width: r.num("width", 0.0), total: Agg::from_key(r.get("total")) }),
                _ => {}
            }
        }
        d
    }

    /// A report of every column of a source (the wizard's start).
    pub fn plain(title: &str, source: &str, columns: &[String]) -> ReportDef {
        ReportDef {
            title: title.to_string(),
            source: source.to_string(),
            fields: columns.iter().map(|c| RField { name: c.clone(), ..Default::default() }).collect(),
            page_footer: "&P / &N".into(),
            grand_total: true,
            ..Default::default()
        }
    }
}

/// One line of a laid out report.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Title(String),
    /// The columns' headings.
    Headings(Vec<String>),
    /// A group starts: its level, field and value.
    GroupHeader { level: usize, field: String, value: String },
    Detail(Vec<String>),
    /// A group ends: its level, value, rows and the columns' totals (empty
    /// where there is none).
    GroupFooter { level: usize, value: String, count: usize, totals: Vec<String> },
    GrandTotal { count: usize, totals: Vec<String> },
    PageBreak,
}

/// Running totals of one level.
#[derive(Clone, Default)]
struct Acc {
    count: usize,
    sum: Vec<f64>,
    n: Vec<usize>,
    min: Vec<Option<Value>>,
    max: Vec<Option<Value>>,
}

impl Acc {
    fn new(cols: usize) -> Acc {
        Acc { count: 0, sum: vec![0.0; cols], n: vec![0; cols], min: vec![None; cols], max: vec![None; cols] }
    }

    fn add(&mut self, vals: &[&Value]) {
        self.count += 1;
        for (i, v) in vals.iter().enumerate() {
            if v.is_null() {
                continue;
            }
            self.n[i] += 1;
            if let Some(x) = v.as_f64() {
                self.sum[i] += x;
            }
            if self.min[i].as_ref().is_none_or(|m| cmp(v, m).is_lt()) {
                self.min[i] = Some((*v).clone());
            }
            if self.max[i].as_ref().is_none_or(|m| cmp(v, m).is_gt()) {
                self.max[i] = Some((*v).clone());
            }
        }
    }

    fn totals(&self, fields: &[RField]) -> Vec<String> {
        fields
            .iter()
            .enumerate()
            .map(|(i, f)| match f.total {
                Agg::None => String::new(),
                Agg::Sum => fmt_num(round(self.sum[i])),
                Agg::Avg if self.n[i] > 0 => fmt_num(round(self.sum[i] / self.n[i] as f64)),
                Agg::Avg => String::new(),
                Agg::Count => self.n[i].to_string(),
                Agg::Min => self.min[i].as_ref().map(Value::to_text).unwrap_or_default(),
                Agg::Max => self.max[i].as_ref().map(Value::to_text).unwrap_or_default(),
            })
            .collect()
    }
}

fn round(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Values in order: numbers by value, the rest by text (empty first).
pub fn cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => return std::cmp::Ordering::Equal,
        (true, false) => return std::cmp::Ordering::Less,
        (false, true) => return std::cmp::Ordering::Greater,
        _ => {}
    }
    match (a, b) {
        (Value::Text(_), _) | (_, Value::Text(_)) => a.to_text().to_lowercase().cmp(&b.to_text().to_lowercase()),
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            _ => a.to_text().cmp(&b.to_text()),
        },
    }
}

/// Lays out `rows` (with `columns`) as the lines of report `def`.
pub fn lay_out(def: &ReportDef, columns: &[String], rows: &[Vec<Value>]) -> Vec<Line> {
    let col = |name: &str| columns.iter().position(|c| c.eq_ignore_ascii_case(name));
    let fields: Vec<RField> = def.fields.iter().filter(|f| col(&f.name).is_some()).cloned().collect();
    let fcols: Vec<usize> = fields.iter().filter_map(|f| col(&f.name)).collect();
    let groups: Vec<(&RGroup, usize)> = def.groups.iter().filter_map(|g| col(&g.field).map(|c| (g, c))).collect();
    let sorts: Vec<(usize, bool)> = def.sort.iter().filter_map(|(f, a)| col(f).map(|c| (c, *a))).collect();
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        for &(g, c) in &groups {
            let o = cmp(&rows[a][c], &rows[b][c]);
            let o = if g.asc { o } else { o.reverse() };
            if o.is_ne() {
                return o;
            }
        }
        for &(c, asc) in &sorts {
            let o = cmp(&rows[a][c], &rows[b][c]);
            let o = if asc { o } else { o.reverse() };
            if o.is_ne() {
                return o;
            }
        }
        a.cmp(&b)
    });
    let mut out = Vec::new();
    if !def.title.is_empty() {
        out.push(Line::Title(def.title.clone()));
    }
    out.push(Line::Headings(fields.iter().map(|f| f.heading().to_string()).collect()));
    let n = fields.len();
    let mut accs: Vec<Acc> = vec![Acc::new(n); groups.len()];
    let mut grand = Acc::new(n);
    let mut current: Vec<Option<String>> = vec![None; groups.len()];
    let close = |out: &mut Vec<Line>, accs: &mut Vec<Acc>, current: &mut Vec<Option<String>>, from: usize| {
        for l in (from..groups.len()).rev() {
            if let Some(v) = current[l].take() {
                if groups[l].0.footer {
                    out.push(Line::GroupFooter { level: l, value: v, count: accs[l].count, totals: accs[l].totals(&fields) });
                }
                accs[l] = Acc::new(n);
            }
        }
    };
    let mut first = true;
    for &r in &order {
        let row = &rows[r];
        // The first level whose value changes closes it and the ones below.
        let changed = (0..groups.len()).find(|&l| current[l].as_deref() != Some(row[groups[l].1].to_text().as_str()));
        if let Some(from) = changed {
            close(&mut out, &mut accs, &mut current, from);
            for l in from..groups.len() {
                let v = row[groups[l].1].to_text();
                if groups[l].0.page_break && !first && l == from {
                    out.push(Line::PageBreak);
                }
                if groups[l].0.header {
                    out.push(Line::GroupHeader { level: l, field: groups[l].0.field.clone(), value: v.clone() });
                }
                current[l] = Some(v);
            }
        }
        first = false;
        let vals: Vec<&Value> = fcols.iter().map(|&c| &row[c]).collect();
        for a in accs.iter_mut() {
            a.add(&vals);
        }
        grand.add(&vals);
        out.push(Line::Detail(vals.iter().map(|v| v.to_text()).collect()));
    }
    close(&mut out, &mut accs, &mut current, 0);
    if def.grand_total {
        out.push(Line::GrandTotal { count: grand.count, totals: grand.totals(&fields) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_totals() {
        let cols = vec!["Country".to_string(), "Name".to_string(), "Amount".to_string()];
        let rows = vec![
            vec![Value::Text("Spain".into()), Value::Text("Bea".into()), Value::Int(20)],
            vec![Value::Text("France".into()), Value::Text("Luc".into()), Value::Int(5)],
            vec![Value::Text("Spain".into()), Value::Text("Ana".into()), Value::Int(10)],
        ];
        let def = ReportDef {
            title: "Sales".into(),
            fields: vec![RField { name: "Name".into(), ..Default::default() }, RField { name: "Amount".into(), total: Agg::Sum, ..Default::default() }],
            groups: vec![RGroup { field: "Country".into(), page_break: true, ..Default::default() }],
            sort: vec![("Name".into(), true)],
            grand_total: true,
            ..Default::default()
        };
        let lines = lay_out(&def, &cols, &rows);
        assert_eq!(
            lines,
            vec![
                Line::Title("Sales".into()),
                Line::Headings(vec!["Name".into(), "Amount".into()]),
                Line::GroupHeader { level: 0, field: "Country".into(), value: "France".into() },
                Line::Detail(vec!["Luc".into(), "5".into()]),
                Line::GroupFooter { level: 0, value: "France".into(), count: 1, totals: vec!["".into(), "5".into()] },
                Line::PageBreak,
                Line::GroupHeader { level: 0, field: "Country".into(), value: "Spain".into() },
                Line::Detail(vec!["Ana".into(), "10".into()]),
                Line::Detail(vec!["Bea".into(), "20".into()]),
                Line::GroupFooter { level: 0, value: "Spain".into(), count: 2, totals: vec!["".into(), "30".into()] },
                Line::GrandTotal { count: 3, totals: vec!["".into(), "35".into()] },
            ]
        );
        let back = ReportDef::from_records(&crate::objects::read(&crate::objects::write(&def.to_records())));
        assert_eq!(back, def);
    }
}
