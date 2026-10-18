//! Plan tables in files: the table itself is a normal table with values
//! (what Excel and LibreOffice show); the plan settings (calendars,
//! project start, baseline date, custom fields) and the role of each column are an
//! `<apex:plans>` element of our namespace, which other programs skip.
//! It lives in our workbook.xml extension in xlsx and in our own part in
//! ods. Plans are keyed by table name.

use std::fmt::Write as _;

use apex_sheet::Workbook;
use apex_sheet::plan::{Calendar, CustomField, Exception, Field, Plan, PlanKind, Rollup};

use crate::xml::{Ev, Reader, esc};

/// `<apex:plans>` for the plan tables of `wb` ("" when there are none).
/// The `apex` prefix is declared by the enclosing element.
pub(crate) fn plans_xml(wb: &Workbook) -> String {
    let mut x = String::new();
    for t in wb.sheets.iter().flat_map(|s| &s.tables) {
        let has_fields = t.columns.iter().any(|c| c.field.is_some());
        if t.plan.is_none() && !has_fields {
            continue;
        }
        let fields: Vec<&str> = t.columns.iter().map(|c| c.field.map_or("-", |f| f.key())).collect();
        let _ = write!(x, r#"<apex:plan table="{}" fields="{}""#, esc(&t.name), fields.join(" "));
        let Some(p) = &t.plan else {
            x.push_str("/>");
            continue;
        };
        let kind = match p.kind {
            PlanKind::Tasks => "tasks",
            PlanKind::Resources => "resources",
        };
        let _ = write!(x, r#" kind="{kind}" start="{}""#, p.start);
        if !p.resources.is_empty() {
            let _ = write!(x, r#" resources="{}""#, esc(&p.resources));
        }
        if let Some(b) = p.baseline {
            let _ = write!(x, r#" baseline="{b}""#);
        }
        if let Some(s) = p.status {
            let _ = write!(x, r#" status="{s}""#);
        }
        x.push('>');
        for c in &p.calendars {
            let days: String = c.weekdays.iter().map(|&d| if d { '1' } else { '0' }).collect();
            let hours: Vec<String> = c.hours.iter().map(|(a, b)| format!("{a},{b}")).collect();
            let _ = write!(x, r#"<apex:calendar name="{}" days="{days}" hours="{}""#, esc(&c.name), hours.join(";"));
            if c.inherit {
                x.push_str(r#" inherit="1""#);
            }
            if c.exceptions.is_empty() {
                x.push_str("/>");
                continue;
            }
            x.push('>');
            for e in &c.exceptions {
                let _ = write!(x, r#"<apex:exception name="{}" from="{}" to="{}" working="{}"/>"#, esc(&e.name), e.from, e.to, e.working as u8);
            }
            x.push_str("</apex:calendar>");
        }
        for c in &p.custom {
            let _ = write!(x, r#"<apex:custom column="{}" formula="{}" rollup="{}"/>"#, esc(&c.column), esc(&c.formula), c.rollup.key());
        }
        x.push_str("</apex:plan>");
    }
    if x.is_empty() {
        return x;
    }
    format!("<apex:plans>{x}</apex:plans>")
}

/// Reads the first `<…:plans>` element in `xml` into the tables of `wb`
/// with the same names. Returns whether there was one.
pub(crate) fn read_plans(xml: &str, wb: &mut Workbook) -> bool {
    let Some(at) = xml.find(":plans>").and_then(|i| xml[..i].rfind('<')) else { return false };
    let mut r = Reader::new(&xml[at..]);
    let mut current: Option<Pending> = None;
    let mut found = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "plans" => found = true,
            Ev::End(n) if crate::xml::local(n) == "plans" => break,
            Ev::Start(t) if t.local() == "plan" => current = Some(plan_start(&t)),
            Ev::Empty(t) if t.local() == "plan" => apply(wb, Some(plan_start(&t))),
            Ev::End(n) if crate::xml::local(n) == "plan" => apply(wb, current.take()),
            Ev::Start(t) | Ev::Empty(t) if t.local() == "calendar" => {
                let mut c = Calendar::standard(t.attr("name").unwrap_or_default());
                let days = t.attr("days").unwrap_or_default();
                if days.len() == 7 {
                    for (i, ch) in days.chars().enumerate() {
                        c.weekdays[i] = ch == '1';
                    }
                }
                if let Some(h) = t.attr("hours") {
                    c.hours = h
                        .split(';')
                        .filter_map(|p| {
                            let (a, b) = p.split_once(',')?;
                            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                        })
                        .collect();
                }
                c.inherit = t.attr("inherit").is_some_and(|v| v == "1" || v == "true");
                c.exceptions.clear();
                if let Some((_, _, Some(p))) = &mut current {
                    p.calendars.push(c);
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "custom" => {
                let column = t.attr("column").unwrap_or_default().into_owned();
                if let Some((_, _, Some(p))) = &mut current
                    && !column.is_empty()
                {
                    let rollup = t.attr("rollup").map(|k| Rollup::from_key(&k)).unwrap_or_default();
                    p.custom.push(CustomField { column, formula: t.attr("formula").unwrap_or_default().into_owned(), rollup });
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "exception" => {
                let int = |k: &str| t.attr(k).and_then(|v| v.parse::<i64>().ok());
                let (Some(from), to) = (int("from"), int("to")) else { continue };
                let e = Exception { name: t.attr("name").unwrap_or_default().into_owned(), from, to: to.unwrap_or(from).max(from), working: t.attr("working").is_some_and(|v| v == "1" || v == "true") };
                if let Some((_, _, Some(p))) = &mut current
                    && let Some(c) = p.calendars.last_mut()
                {
                    c.exceptions.push(e);
                }
            }
            _ => {}
        }
    }
    found
}

type Pending = (String, Vec<Option<Field>>, Option<Plan>);

/// The table name, column roles and settings of a `<plan>` element.
fn plan_start(t: &crate::xml::Tag) -> Pending {
    let name = t.attr("table").unwrap_or_default().into_owned();
    let fields = t.attr("fields").unwrap_or_default().split_whitespace().map(Field::from_key).collect();
    let plan = t.attr("kind").map(|k| {
        let num = |k: &str| t.attr(k).and_then(|v| v.parse::<f64>().ok());
        let mut p = if k == "resources" { Plan::resources() } else { Plan::tasks(0.0) };
        p.start = num("start").unwrap_or(0.0);
        p.baseline = num("baseline");
        p.status = num("status");
        p.resources = t.attr("resources").unwrap_or_default().into_owned();
        p.calendars.clear();
        p
    });
    (name, fields, plan)
}

/// Gives the table named like the plan its plan and column roles.
fn apply(wb: &mut Workbook, plan: Option<Pending>) {
    let Some((name, fields, mut plan)) = plan else { return };
    if let Some(p) = &mut plan
        && p.kind == PlanKind::Tasks
        && p.calendars.is_empty()
    {
        p.calendars.push(Calendar::default());
    }
    let Some(t) = wb.sheets.iter_mut().flat_map(|s| s.tables.iter_mut()).find(|t| t.name.eq_ignore_ascii_case(&name)) else { return };
    // Column roles only when the columns still match.
    if fields.len() == t.columns.len() {
        for (c, f) in t.columns.iter_mut().zip(fields) {
            c.field = f;
        }
    }
    t.plan = plan.map(Box::new);
}

#[cfg(test)]
mod tests;
