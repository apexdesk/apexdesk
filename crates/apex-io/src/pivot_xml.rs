//! ApexDesk's pivot tables and slicers as a small XML element of our own
//! (`<apex:pivots>`), with everything their spec holds: it goes next to
//! the data model (an extension of workbook.xml in xlsx, the
//! `apexdesk/model.xml` part in ods), so tables on the data model and the
//! details the native formats can't say come back as they were.

use std::fmt::Write as _;

use apex_sheet::analysis::pivot::{
    AggFunc, BaseField, BaseItem, CalcField, DatePart, Field, FieldFilter, Grouping, LabelFilter, LabelOp, Layout, ModelSource, PivotSpec, ShowAs, SortOrder, ValueField, ValueFilter, ValueOp,
};
use apex_sheet::analysis::{Lang, Slicer, StoredPivot};
use apex_sheet::{CellRef, Range, Workbook};

use crate::xml::{Ev, Reader, Tag, esc};

/// `<apex:pivots>` with the pivot tables and slicers ("" when none).
pub(crate) fn pivots_xml(wb: &Workbook) -> String {
    let st = &wb.analysis;
    if st.pivots.is_empty() && st.slicers.is_empty() {
        return String::new();
    }
    let mut x = String::from("<apex:pivots>");
    for p in &st.pivots {
        let _ = write!(x, r#"<apex:pivot sheet="{}" at="{}""#, esc(&p.sheet), p.at.to_a1());
        if let Some(a) = &p.area {
            let _ = write!(x, r#" area="{}""#, a.to_a1());
        }
        // Charts by position on their sheet (ids are made again on load).
        let charts = wb.sheet_index(&p.sheet).map(|si| &wb.sheets[si].charts);
        let pos: Vec<String> = p.charts.iter().filter_map(|id| charts.and_then(|c| c.iter().position(|c| c.id == *id))).map(|i| i.to_string()).collect();
        if !pos.is_empty() {
            let _ = write!(x, r#" charts="{}""#, pos.join(" "));
        }
        x.push('>');
        write_spec(&mut x, &p.spec);
        x.push_str("</apex:pivot>");
    }
    for s in &st.slicers {
        let _ = write!(
            x,
            r#"<apex:slicer sheet="{}" x="{}" y="{}" width="{}" height="{}" pivot="{}" col="{}" caption="{}" columns="{}"/>"#,
            esc(&s.sheet), s.x, s.y, s.width, s.height, s.pivot, s.col, esc(&s.caption), s.columns
        );
    }
    x.push_str("</apex:pivots>");
    x
}

fn write_spec(x: &mut String, s: &PivotSpec) {
    let layout = match s.layout {
        Layout::Compact => "compact",
        Layout::Outline => "outline",
        Layout::Tabular => "tabular",
    };
    let _ = write!(
        x,
        r#"<apex:spec source-sheet="{}" source="{}" layout="{layout}" subtotals="{}" grand-rows="{}" grand-cols="{}" repeat-labels="{}" lang="{}" style="{}""#,
        esc(&s.source_sheet), s.source.to_a1(), s.subtotals as u8, s.grand_rows as u8, s.grand_cols as u8, s.repeat_labels as u8, if s.lang == Lang::Es { "es" } else { "en" }, esc(&s.style)
    );
    if !s.empty_text.is_empty() {
        let _ = write!(x, r#" empty-text="{}""#, esc(&s.empty_text));
    }
    x.push('>');
    for f in &s.rows {
        write_field(x, "row", f, &[]);
    }
    for f in &s.cols {
        write_field(x, "col", f, &[]);
    }
    for f in &s.filters {
        write_field(x, "page", &f.field, &f.items);
    }
    for f in &s.slicers {
        write_field(x, "slicer-filter", &f.field, &f.items);
    }
    for v in &s.values {
        let _ = write!(x, r#"<apex:data col="{}" func="{}""#, v.col, func_key(v.func));
        write_show_as(x, &v.show_as);
        if let Some(n) = &v.name {
            let _ = write!(x, r#" name="{}""#, esc(n));
        }
        if let Some(f) = &v.number_format {
            let _ = write!(x, r#" format="{}""#, esc(f));
        }
        x.push_str("/>");
    }
    for c in &s.calculated {
        let _ = write!(x, r#"<apex:calc name="{}" formula="{}"/>"#, esc(&c.name), esc(&c.formula));
    }
    if let Some(m) = &s.model {
        x.push_str("<apex:model-source>");
        for (t, c) in &m.cols {
            let _ = write!(x, r#"<apex:model-col table="{}" col="{}"/>"#, esc(t), esc(c));
        }
        for n in &m.measures {
            let _ = write!(x, r#"<apex:model-measure name="{}"/>"#, esc(n));
        }
        x.push_str("</apex:model-source>");
    }
    x.push_str("</apex:spec>");
}

fn write_field(x: &mut String, tag: &str, f: &Field, items: &[String]) {
    let _ = write!(x, r#"<apex:{tag} col="{}""#, f.col);
    match f.group {
        Grouping::None => {}
        Grouping::Date(p) => {
            let _ = write!(x, r#" group="{}""#, date_key(p));
        }
        Grouping::Number { start, end, by } => {
            let _ = write!(x, r#" group="number" start="{start}" end="{end}" by="{by}""#);
        }
    }
    match f.sort {
        SortOrder::LabelAsc => {}
        SortOrder::LabelDesc => x.push_str(r#" sort="desc""#),
        SortOrder::ValueAsc(i) => {
            let _ = write!(x, r#" sort="value-asc" sort-value="{i}""#);
        }
        SortOrder::ValueDesc(i) => {
            let _ = write!(x, r#" sort="value-desc" sort-value="{i}""#);
        }
    }
    if let Some(n) = &f.name {
        let _ = write!(x, r#" name="{}""#, esc(n));
    }
    x.push('>');
    for h in &f.hidden {
        let _ = write!(x, "<apex:hidden>{}</apex:hidden>", esc(h));
    }
    for i in items {
        let _ = write!(x, "<apex:item>{}</apex:item>", esc(i));
    }
    if let Some(l) = &f.label_filter {
        let _ = write!(x, r#"<apex:label-filter op="{:?}" a="{}" b="{}"/>"#, l.op, esc(&l.a), esc(&l.b));
    }
    if let Some(v) = &f.value_filter {
        let _ = write!(x, r#"<apex:value-filter value="{}" op="{:?}" a="{}" b="{}"/>"#, v.value, v.op, v.a, v.b);
    }
    let _ = write!(x, "</apex:{tag}>");
}

fn write_show_as(x: &mut String, s: &ShowAs) {
    let (key, base, item) = show_as_parts(s);
    if key == "normal" {
        return;
    }
    let _ = write!(x, r#" show-as="{key}""#);
    if let Some(b) = base {
        let _ = write!(x, r#" base="{}""#, match b {
            BaseField::Row(i) => format!("r{i}"),
            BaseField::Col(i) => format!("c{i}"),
        });
    }
    if let Some(i) = item {
        let v = match i {
            BaseItem::Previous => "(previous)".to_string(),
            BaseItem::Next => "(next)".to_string(),
            BaseItem::Named(n) => format!("={n}"),
        };
        let _ = write!(x, r#" base-item="{}""#, esc(&v));
    }
}

/// Key, base field and base item of a "show values as".
fn show_as_parts(s: &ShowAs) -> (&'static str, Option<BaseField>, Option<&BaseItem>) {
    match s {
        ShowAs::Normal => ("normal", None, None),
        ShowAs::PercentOfGrand => ("percent-of-grand", None, None),
        ShowAs::PercentOfRow => ("percent-of-row", None, None),
        ShowAs::PercentOfCol => ("percent-of-col", None, None),
        ShowAs::PercentOfParent => ("percent-of-parent", None, None),
        ShowAs::PercentOfParentCol => ("percent-of-parent-col", None, None),
        ShowAs::Index => ("index", None, None),
        ShowAs::DifferenceFrom(b, i) => ("difference", Some(*b), Some(i)),
        ShowAs::PercentOf(b, i) => ("percent-of", Some(*b), Some(i)),
        ShowAs::PercentDifferenceFrom(b, i) => ("percent-difference", Some(*b), Some(i)),
        ShowAs::RunningTotal(b) => ("running-total", Some(*b), None),
        ShowAs::PercentRunningTotal(b) => ("percent-running-total", Some(*b), None),
        ShowAs::RankAsc(b) => ("rank-asc", Some(*b), None),
        ShowAs::RankDesc(b) => ("rank-desc", Some(*b), None),
    }
}

fn read_show_as(t: &Tag) -> ShowAs {
    let base = t.attr("base").and_then(|b| {
        let n = b.get(1..)?.parse().ok()?;
        match b.chars().next()? {
            'r' => Some(BaseField::Row(n)),
            'c' => Some(BaseField::Col(n)),
            _ => None,
        }
    });
    let item = t.attr("base-item").map(|v| match v.as_ref() {
        "(previous)" => BaseItem::Previous,
        "(next)" => BaseItem::Next,
        s => BaseItem::Named(s.strip_prefix('=').unwrap_or(s).to_string()),
    });
    let b = base.unwrap_or(BaseField::Row(0));
    let i = item.unwrap_or(BaseItem::Previous);
    match t.attr("show-as").as_deref().unwrap_or("normal") {
        "percent-of-grand" => ShowAs::PercentOfGrand,
        "percent-of-row" => ShowAs::PercentOfRow,
        "percent-of-col" => ShowAs::PercentOfCol,
        "percent-of-parent" => ShowAs::PercentOfParent,
        "percent-of-parent-col" => ShowAs::PercentOfParentCol,
        "index" => ShowAs::Index,
        "difference" => ShowAs::DifferenceFrom(b, i),
        "percent-of" => ShowAs::PercentOf(b, i),
        "percent-difference" => ShowAs::PercentDifferenceFrom(b, i),
        "running-total" => ShowAs::RunningTotal(b),
        "percent-running-total" => ShowAs::PercentRunningTotal(b),
        "rank-asc" => ShowAs::RankAsc(b),
        "rank-desc" => ShowAs::RankDesc(b),
        _ => ShowAs::Normal,
    }
}

pub(crate) const FUNCS: [(AggFunc, &str); 12] = [
    (AggFunc::Sum, "sum"),
    (AggFunc::Count, "count"),
    (AggFunc::Average, "average"),
    (AggFunc::Max, "max"),
    (AggFunc::Min, "min"),
    (AggFunc::Product, "product"),
    (AggFunc::CountNums, "countNums"),
    (AggFunc::StdDev, "stdDev"),
    (AggFunc::StdDevP, "stdDevp"),
    (AggFunc::Var, "var"),
    (AggFunc::VarP, "varp"),
    (AggFunc::DistinctCount, "distinctCount"),
];

/// The name of a summary function (as OOXML's `subtotal` has it).
pub(crate) fn func_key(f: AggFunc) -> &'static str {
    FUNCS.iter().find(|(g, _)| *g == f).map_or("sum", |(_, k)| k)
}

pub(crate) fn func_of(key: &str) -> Option<AggFunc> {
    FUNCS.iter().find(|(_, k)| k.eq_ignore_ascii_case(key)).map(|(f, _)| *f)
}

fn date_key(p: DatePart) -> &'static str {
    match p {
        DatePart::Years => "years",
        DatePart::Quarters => "quarters",
        DatePart::Months => "months",
        DatePart::Days => "days",
    }
}

fn read_field(t: &Tag) -> Field {
    let num = |k: &str| t.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let group = match t.attr("group").as_deref() {
        Some("years") => Grouping::Date(DatePart::Years),
        Some("quarters") => Grouping::Date(DatePart::Quarters),
        Some("months") => Grouping::Date(DatePart::Months),
        Some("days") => Grouping::Date(DatePart::Days),
        Some("number") => Grouping::Number { start: num("start"), end: num("end"), by: num("by") },
        _ => Grouping::None,
    };
    let sv = t.attr("sort-value").and_then(|v| v.parse().ok()).unwrap_or(0);
    let sort = match t.attr("sort").as_deref() {
        Some("desc") => SortOrder::LabelDesc,
        Some("value-asc") => SortOrder::ValueAsc(sv),
        Some("value-desc") => SortOrder::ValueDesc(sv),
        _ => SortOrder::LabelAsc,
    };
    Field { col: t.attr("col").and_then(|v| v.parse().ok()).unwrap_or(0), group, sort, name: t.attr("name").map(|v| v.into_owned()), ..Field::default() }
}

/// Reads a field element (its start tag just read) with its children:
/// the field and its selected items.
fn read_field_body(r: &mut Reader, t: &Tag, empty: bool) -> (Field, Vec<String>) {
    let mut f = read_field(t);
    let mut items = Vec::new();
    if empty {
        return (f, items);
    }
    loop {
        let ev = r.next();
        let leaf = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof | Ev::End(_) => break,
            Ev::Start(c) if c.local() == "hidden" => f.hidden.push(r.text()),
            Ev::Start(c) if c.local() == "item" => items.push(r.text()),
            Ev::Start(c) | Ev::Empty(c) => {
                if c.local() == "label-filter" {
                    let op = c.attr("op").and_then(|o| LabelOp::ALL.into_iter().find(|x| format!("{x:?}") == o)).unwrap_or_default();
                    let s = |k: &str| c.attr(k).unwrap_or_default().into_owned();
                    f.label_filter = Some(LabelFilter { op, a: s("a"), b: s("b") });
                } else if c.local() == "value-filter" {
                    let op = c.attr("op").and_then(|o| ValueOp::ALL.into_iter().find(|x| format!("{x:?}") == o)).unwrap_or_default();
                    let n = |k: &str| c.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                    f.value_filter = Some(ValueFilter { value: c.attr("value").and_then(|v| v.parse().ok()).unwrap_or(0), op, a: n("a"), b: n("b") });
                }
                if !leaf {
                    r.skip();
                }
            }
            _ => {}
        }
    }
    (f, items)
}

/// Reads the first `<…:pivots>` element of our namespace in `xml` into
/// the analysis state (pivot tables and slicers); false when there is none.
pub(crate) fn read_pivots_xml(xml: &str, wb: &mut Workbook) -> bool {
    let mut r = Reader::new(xml);
    let mut found = false;
    let mut pivots: Vec<(StoredPivot, Vec<usize>)> = Vec::new();
    let mut slicers = Vec::new();
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "pivots" => found = true,
            Ev::Empty(t) if t.local() == "pivots" => return true,
            Ev::End(n) if found && crate::xml::local(n) == "pivots" => break,
            Ev::Start(t) if found && t.local() == "pivot" => {
                let at = t.attr("at").and_then(|v| CellRef::parse(&v)).unwrap_or_default();
                let area = t.attr("area").and_then(|v| Range::parse(&v));
                let charts: Vec<usize> = t.attr("charts").map(|v| v.split_whitespace().filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
                let sheet = t.attr("sheet").unwrap_or_default().into_owned();
                let mut spec = PivotSpec::default();
                loop {
                    match r.next() {
                        Ev::Eof | Ev::End(_) => break,
                        Ev::Start(s) if s.local() == "spec" => spec = read_spec(&mut r, &s),
                        Ev::Start(_) => r.skip(),
                        _ => {}
                    }
                }
                pivots.push((StoredPivot { spec, sheet, at, area, charts: Vec::new() }, charts));
            }
            Ev::Start(t) | Ev::Empty(t) if found && t.local() == "slicer" => {
                let n = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                let u = |k: &str| t.attr(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
                slicers.push(Slicer {
                    sheet: t.attr("sheet").unwrap_or_default().into_owned(),
                    x: n("x"),
                    y: n("y"),
                    width: n("width").max(20.0),
                    height: n("height").max(20.0),
                    pivot: u("pivot") as usize,
                    col: u("col"),
                    caption: t.attr("caption").unwrap_or_default().into_owned(),
                    columns: u("columns").clamp(1, 20) as u8,
                });
            }
            _ => {}
        }
    }
    if !found {
        return false;
    }
    let base = wb.analysis.pivots.len();
    for (mut p, charts) in pivots {
        if let Some(si) = wb.sheet_index(&p.sheet) {
            p.charts = charts.iter().filter_map(|&i| wb.sheets[si].charts.get(i).map(|c| c.id)).collect();
        }
        wb.analysis.pivots.push(p);
    }
    let n = wb.analysis.pivots.len();
    wb.analysis.slicers.extend(slicers.into_iter().map(|mut s| {
        s.pivot += base;
        s
    }).filter(|s| s.pivot < n));
    true
}

fn read_spec(r: &mut Reader, t: &Tag) -> PivotSpec {
    let flag = |k: &str, d: bool| t.attr(k).map_or(d, |v| v == "1" || v == "true");
    let mut s = PivotSpec {
        source_sheet: t.attr("source-sheet").unwrap_or_default().into_owned(),
        source: t.attr("source").and_then(|v| Range::parse(&v)).unwrap_or_default(),
        layout: match t.attr("layout").as_deref() {
            Some("outline") => Layout::Outline,
            Some("tabular") => Layout::Tabular,
            _ => Layout::Compact,
        },
        subtotals: flag("subtotals", true),
        grand_rows: flag("grand-rows", true),
        grand_cols: flag("grand-cols", true),
        repeat_labels: flag("repeat-labels", false),
        empty_text: t.attr("empty-text").unwrap_or_default().into_owned(),
        lang: if t.attr("lang").is_some_and(|v| v == "es") { Lang::Es } else { Lang::En },
        ..PivotSpec::default()
    };
    if let Some(st) = t.attr("style").filter(|v| !v.is_empty()) {
        s.style = st.into_owned();
    }
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof | Ev::End(_) => break,
            Ev::Start(c) | Ev::Empty(c) => match c.local() {
                "row" => s.rows.push(read_field_body(r, &c, empty).0),
                "col" => s.cols.push(read_field_body(r, &c, empty).0),
                "page" | "slicer-filter" => {
                    let (field, items) = read_field_body(r, &c, empty);
                    let list = if c.local() == "page" { &mut s.filters } else { &mut s.slicers };
                    list.push(FieldFilter { field, items });
                }
                "data" => {
                    s.values.push(ValueField {
                        col: c.attr("col").and_then(|v| v.parse().ok()).unwrap_or(0),
                        func: c.attr("func").and_then(|v| func_of(&v)).unwrap_or_default(),
                        show_as: read_show_as(&c),
                        name: c.attr("name").map(|v| v.into_owned()),
                        number_format: c.attr("format").map(|v| v.into_owned()),
                    });
                    if !empty {
                        r.skip();
                    }
                }
                "calc" => {
                    s.calculated.push(CalcField { name: c.attr("name").unwrap_or_default().into_owned(), formula: c.attr("formula").unwrap_or_default().into_owned() });
                    if !empty {
                        r.skip();
                    }
                }
                "model-source" => {
                    let mut m = ModelSource::default();
                    if !empty {
                        loop {
                            let ev = r.next();
                            let leaf = matches!(ev, Ev::Empty(_));
                            match ev {
                                Ev::Eof | Ev::End(_) => break,
                                Ev::Start(k) | Ev::Empty(k) => {
                                    let a = |n: &str| k.attr(n).unwrap_or_default().into_owned();
                                    match k.local() {
                                        "model-col" => m.cols.push((a("table"), a("col"))),
                                        "model-measure" => m.measures.push(a("name")),
                                        _ => {}
                                    }
                                    if !leaf {
                                        r.skip();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    s.model = Some(m);
                }
                _ => {
                    if !empty {
                        r.skip();
                    }
                }
            },
            _ => {}
        }
    }
    s
}
