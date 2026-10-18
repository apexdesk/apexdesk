//! Pivot tables in ods: `<table:data-pilot-tables>`, LibreOffice's data
//! pilots (source range, fields by orientation, functions, grand totals).
//! ApexDesk writes its tables on sheet ranges there and reads the ones
//! it can recalculate; the others (database sources, grouped fields...)
//! are kept as they were. The whole spec of ours, slicers and tables on
//! the data model included, goes in `apexdesk/model.xml` (see
//! `pivot_xml`); a marker attribute tells our data pilots from others.

use std::collections::HashSet;
use std::fmt::Write as _;

use apex_sheet::analysis::pivot::{AggFunc, BaseField, BaseItem, Field, FieldFilter, Layout, PivotSpec, ShowAs, SortOrder, ValueField, ValueFilter, ValueOp, CALC_BASE};
use apex_sheet::analysis::StoredPivot;
use apex_sheet::{CellRef, Range, Workbook};

use super::data::{parse_ranges, range_addr};
use crate::xml::{Ev, Reader, Tag, esc};

/// Slot of `wb.kept.xml` holding data pilots ApexDesk doesn't recalculate.
const KEPT_SLOT: &str = "ods-data-pilot";

/// ODF functions of the summary functions (distinct count has none).
const FUNCS: [(AggFunc, &str); 11] = [
    (AggFunc::Sum, "sum"),
    (AggFunc::Count, "count"),
    (AggFunc::Average, "average"),
    (AggFunc::Max, "max"),
    (AggFunc::Min, "min"),
    (AggFunc::Product, "product"),
    (AggFunc::CountNums, "countnums"),
    (AggFunc::StdDev, "stdev"),
    (AggFunc::StdDevP, "stdevp"),
    (AggFunc::Var, "var"),
    (AggFunc::VarP, "varp"),
];

/// `<table:data-pilot-tables>` ("" when there are none).
pub(super) fn write_data_pilots(wb: &Workbook) -> String {
    let mut s = String::new();
    let mut names: HashSet<String> = HashSet::new();
    for (_, x) in wb.kept.xml.iter().filter(|(slot, _)| slot == KEPT_SLOT) {
        if kept_fits(wb, x) {
            if let Some(n) = x.split("table:name=\"").nth(1).and_then(|v| v.split('"').next()) {
                names.insert(n.to_string());
            }
            s.push_str(x);
        }
    }
    for (k, p) in wb.analysis.pivots.iter().enumerate() {
        if p.spec.model.is_none() {
            write_pilot(&mut s, wb, k, p, &mut names);
        }
    }
    if s.is_empty() { s } else { format!("<table:data-pilot-tables>{s}</table:data-pilot-tables>") }
}

/// A kept data pilot still has its sheets.
fn kept_fits(wb: &Workbook, x: &str) -> bool {
    let attr = |k: &str| x.split(&format!("{k}=\"")).nth(1).and_then(|v| v.split('"').next()).map(|v| crate::xml::unescape(v).into_owned());
    let has = |a: Option<String>| a.is_none_or(|a| parse_ranges(&a).0.is_none_or(|sh| wb.sheet_index(&sh).is_some()));
    has(attr("table:target-range-address")) && has(attr("table:cell-range-address"))
}

fn write_pilot(s: &mut String, wb: &Workbook, k: usize, p: &StoredPivot, names: &mut HashSet<String>) {
    let spec = &p.spec;
    let (Some(si), Some(_)) = (wb.sheet_index(&spec.source_sheet), wb.sheet_index(&p.sheet)) else { return };
    let Some(range) = wb.clamp_to_used(si, &spec.source) else { return };
    let probe = PivotSpec { rows: Vec::new(), cols: Vec::new(), values: Vec::new(), filters: Vec::new(), slicers: Vec::new(), calculated: Vec::new(), ..spec.clone() };
    let Ok(src) = wb.pivot_source(&probe) else { return };
    let field_name = |c: u32| src.names.get(c as usize).cloned();
    let mut n = names.len() + 1;
    while names.contains(&format!("PivotTable{n}")) {
        n += 1;
    }
    let name = format!("PivotTable{n}");
    names.insert(name.clone());
    let area = p.area.unwrap_or(Range::single(p.at));
    let grand = match (spec.grand_rows, spec.grand_cols) {
        (true, true) => "both",
        (true, false) => "row",
        (false, true) => "column",
        (false, false) => "none",
    };
    let _ = write!(
        s,
        r#"<table:data-pilot-table xmlns:apex="{}" apex:pivot="{k}" table:name="{name}" table:target-range-address="{}" table:grand-total="{grand}"><table:source-cell-range table:cell-range-address="{}"/>"#,
        crate::xlsx_model::NS,
        esc(&range_addr(&p.sheet, &area)),
        esc(&range_addr(&spec.source_sheet, &range))
    );
    let captions: Vec<String> = spec.values.iter().map(|v| v.name.clone().unwrap_or_else(|| lo_caption(v, &src.names, spec))).collect();
    let layout = match spec.layout {
        Layout::Tabular => r#" table:layout-mode="tabular-layout""#,
        Layout::Outline => r#" table:layout-mode="outline-subtotals-top""#,
        Layout::Compact => r#" table:layout-mode="outline-subtotals-top" loext:layout-mode="compact-layout""#,
    };
    let mut used: HashSet<u32> = HashSet::new();
    let axis_field = |s: &mut String, f: &Field, orient: &str, page: Option<&[String]>, used: &mut HashSet<u32>| {
        let Some(fname) = field_name(f.col) else { return };
        // A column shows once on the axes of a data pilot.
        if !used.insert(f.col) {
            return;
        }
        let _ = write!(s, r#"<table:data-pilot-field table:source-field-name="{}" table:orientation="{orient}""#, esc(&fname));
        if let Some(n) = &f.name {
            let _ = write!(s, r#" tableooo:display-name="{}""#, esc(n));
        }
        if let Some(sel) = page.filter(|p| p.len() == 1) {
            let _ = write!(s, r#" table:selected-page="{}""#, esc(&sel[0]));
        }
        s.push_str(r#"><table:data-pilot-level table:show-empty="false">"#);
        if spec.subtotals && page.is_none() {
            s.push_str(r#"<table:data-pilot-subtotals><table:data-pilot-subtotal table:function="auto"/></table:data-pilot-subtotals>"#);
        }
        let members: Vec<(String, bool)> = match page {
            Some(sel) if sel.len() > 1 => {
                let src1 = wb.pivot_source(&PivotSpec { rows: vec![Field::new(f.col)], ..probe.clone() }).unwrap_or_default();
                apex_sheet::analysis::pivot::field_labels(&src1, &Field::new(f.col), spec.lang).into_iter().map(|l| {
                    let on = sel.iter().any(|x| x.to_lowercase() == l.to_lowercase());
                    (l, on)
                }).collect()
            }
            Some(_) => Vec::new(),
            None => f.hidden.iter().map(|h| (h.clone(), false)).collect(),
        };
        if !members.is_empty() {
            s.push_str("<table:data-pilot-members>");
            for (m, on) in &members {
                let _ = write!(s, r#"<table:data-pilot-member table:name="{}" table:display="{on}" table:show-details="true"/>"#, esc(m));
            }
            s.push_str("</table:data-pilot-members>");
        }
        // Top and bottom items; other value filters have no equivalent.
        if let Some((v, cap)) = f.value_filter.as_ref().filter(|v| matches!(v.op, ValueOp::TopItems | ValueOp::BottomItems)).and_then(|v| Some((v, captions.get(v.value)?))) {
            let _ = write!(s, r#"<table:data-pilot-display-info table:enabled="true" table:data-field="{}" table:member-count="{}" table:display-member-mode="{}"/>"#, esc(cap), v.a.max(0.0) as u32, if v.op == ValueOp::TopItems { "from-top" } else { "from-bottom" });
        }
        let (mode, order, data) = match f.sort {
            SortOrder::LabelAsc => ("name", "ascending", None),
            SortOrder::LabelDesc => ("name", "descending", None),
            SortOrder::ValueAsc(i) => ("data", "ascending", captions.get(i)),
            SortOrder::ValueDesc(i) => ("data", "descending", captions.get(i)),
        };
        match data {
            Some(d) => {
                let _ = write!(s, r#"<table:data-pilot-sort-info table:sort-mode="data" table:data-field="{}" table:order="{order}"/>"#, esc(d));
            }
            None => {
                let _ = write!(s, r#"<table:data-pilot-sort-info table:sort-mode="{}" table:order="{order}"/>"#, if mode == "data" { "name" } else { mode });
            }
        }
        let _ = write!(s, r#"<table:data-pilot-layout-info{layout} table:add-empty-lines="false"/></table:data-pilot-level></table:data-pilot-field>"#);
    };
    for f in &spec.rows {
        axis_field(s, f, "row", None, &mut used);
    }
    for f in &spec.cols {
        axis_field(s, f, "column", None, &mut used);
    }
    let values: Vec<(usize, &ValueField)> = spec.values.iter().enumerate().filter(|(_, v)| v.col < CALC_BASE && field_name(v.col).is_some()).collect();
    if values.len() > 1 {
        s.push_str(r#"<table:data-pilot-field table:source-field-name="" table:is-data-layout-field="true" table:orientation="column"/>"#);
    }
    for ff in &spec.filters {
        axis_field(s, &ff.field, "page", Some(&ff.items), &mut used);
    }
    for (i, v) in values {
        let Some(fname) = field_name(v.col) else { continue };
        let func = FUNCS.iter().find(|(f, _)| *f == v.func).map_or("count", |(_, k)| k);
        let _ = write!(s, r#"<table:data-pilot-field table:source-field-name="{}" table:orientation="data" table:function="{func}""#, esc(&fname));
        if v.name.is_some() {
            let _ = write!(s, r#" tableooo:display-name="{}""#, esc(&captions[i]));
        }
        s.push_str(r#"><table:data-pilot-level table:show-empty="false"/>"#);
        if let Some(r) = field_reference(spec, &v.show_as, &field_name) {
            s.push_str(&r);
        }
        s.push_str("</table:data-pilot-field>");
    }
    s.push_str("</table:data-pilot-table>");
}

/// LibreOffice's caption of a value field ("Sum - Qty").
fn lo_caption(v: &ValueField, names: &[String], spec: &PivotSpec) -> String {
    let f = FUNCS.iter().find(|(f, _)| *f == v.func).map_or("Count", |(_, k)| k);
    let func = match f {
        "countnums" => "Count".to_string(),
        "stdev" => "StDev".to_string(),
        "stdevp" => "StDevP".to_string(),
        "varp" => "VarP".to_string(),
        k => format!("{}{}", k[..1].to_uppercase(), &k[1..]),
    };
    let field = if v.col >= CALC_BASE { spec.calculated.get((v.col - CALC_BASE) as usize).map(|c| c.name.clone()).unwrap_or_default() } else { names.get(v.col as usize).cloned().unwrap_or_default() };
    format!("{func} - {field}")
}

/// `<table:data-pilot-field-reference>` of a "show values as".
fn field_reference(spec: &PivotSpec, s: &ShowAs, name: &dyn Fn(u32) -> Option<String>) -> Option<String> {
    let (ty, base, item) = match s {
        ShowAs::DifferenceFrom(b, i) => ("member-difference", Some(*b), Some(i)),
        ShowAs::PercentOf(b, i) => ("member-percentage", Some(*b), Some(i)),
        ShowAs::PercentDifferenceFrom(b, i) => ("member-percentage-difference", Some(*b), Some(i)),
        ShowAs::RunningTotal(b) => ("running-total", Some(*b), None),
        ShowAs::PercentOfRow => ("row-percentage", None, None),
        ShowAs::PercentOfCol => ("column-percentage", None, None),
        ShowAs::PercentOfGrand => ("total-percentage", None, None),
        ShowAs::Index => ("index", None, None),
        _ => return None,
    };
    let mut x = format!(r#"<table:data-pilot-field-reference table:type="{ty}""#);
    if let Some(b) = base {
        let f = match b {
            BaseField::Row(l) => spec.rows.get(l),
            BaseField::Col(l) => spec.cols.get(l),
        }?;
        let _ = write!(x, r#" table:field-name="{}""#, esc(&name(f.col)?));
    } else {
        // LibreOffice wants a field even where it doesn't use it.
        let f = spec.rows.first().or(spec.cols.first())?;
        let _ = write!(x, r#" table:field-name="{}""#, esc(&name(f.col)?));
    }
    match item {
        Some(BaseItem::Previous) => x.push_str(r#" table:member-type="previous""#),
        Some(BaseItem::Next) => x.push_str(r#" table:member-type="next""#),
        Some(BaseItem::Named(n)) => {
            let _ = write!(x, r#" table:member-type="named" table:member-name="{}""#, esc(n));
        }
        None => {}
    }
    x.push_str("/>");
    Some(x)
}

// ---------------------------------------------------------------------------
// Reading

/// A data pilot field as read.
#[derive(Default)]
struct PField {
    name: String,
    orientation: String,
    function: String,
    layout_field: bool,
    selected: Option<String>,
    display_name: Option<String>,
    members: Vec<(String, bool)>,
    sort: Option<(String, String, String)>,
    layout: Option<(String, bool)>,
    subtotals: bool,
    reference: Option<(String, String, String, String)>,
    top: Option<(String, u32, bool)>,
}

/// Reads the data pilots (`raw`: each whole element) into pivot tables;
/// with `ours`, the ones ApexDesk wrote are already read (from our part).
/// Those ApexDesk can't recalculate are kept.
pub(super) fn read_data_pilots(raw: Vec<String>, ours: bool, wb: &mut Workbook) {
    for x in raw {
        if ours && x.contains("urn:apexdesk:model") {
            continue;
        }
        match read_pilot(&x, wb) {
            Some(p) => wb.analysis.pivots.push(p),
            None => wb.kept.xml.push((KEPT_SLOT.to_string(), x)),
        }
    }
}

fn read_pilot(x: &str, wb: &Workbook) -> Option<StoredPivot> {
    let mut r = Reader::new(x);
    let mut target = None;
    let mut grand = String::from("both");
    let mut source = None;
    let mut fields: Vec<PField> = Vec::new();
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::End(n) if n == "table:data-pilot-field" => {}
            Ev::Start(t) | Ev::Empty(t) => match t.name {
                "table:data-pilot-table" => {
                    let (sh, rs) = parse_ranges(&t.attr("table:target-range-address")?);
                    target = Some((sh?, *rs.first()?));
                    if let Some(g) = t.attr("table:grand-total") {
                        grand = g.into_owned();
                    }
                }
                "table:source-cell-range" => {
                    let (sh, rs) = parse_ranges(&t.attr("table:cell-range-address")?);
                    source = Some((sh?, *rs.first()?));
                    // A filter on the source isn't ApexDesk's.
                    if !empty {
                        return None;
                    }
                }
                "table:database-source-sql" | "table:database-source-table" | "table:database-source-query" | "table:source-service" | "table:data-pilot-groups" => return None,
                "table:data-pilot-field" => fields.push(read_field_attrs(&t)),
                "table:data-pilot-member" => {
                    let f = fields.last_mut()?;
                    f.members.push((t.attr("table:name").unwrap_or_default().into_owned(), t.attr("table:display").is_none_or(|v| v != "false")));
                }
                "table:data-pilot-subtotal" => fields.last_mut()?.subtotals = true,
                "table:data-pilot-sort-info" => {
                    let a = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                    fields.last_mut()?.sort = Some((a("table:sort-mode"), a("table:order"), a("table:data-field")));
                }
                "table:data-pilot-layout-info" => {
                    let lo = t.attr("loext:layout-mode").is_some_and(|v| v == "compact-layout");
                    fields.last_mut()?.layout = Some((t.attr("table:layout-mode").unwrap_or_default().into_owned(), lo));
                }
                "table:data-pilot-display-info" => {
                    if t.attr("table:enabled").is_some_and(|v| v == "true") {
                        let n = t.attr("table:member-count").and_then(|v| v.parse().ok()).unwrap_or(10);
                        let top = t.attr("table:display-member-mode").is_none_or(|v| v != "from-bottom");
                        fields.last_mut()?.top = Some((t.attr("table:data-field").unwrap_or_default().into_owned(), n, top));
                    }
                }
                "table:data-pilot-field-reference" => {
                    let a = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                    fields.last_mut()?.reference = Some((a("table:type"), a("table:field-name"), a("table:member-type"), a("table:member-name")));
                }
                _ => {}
            },
            _ => {}
        }
    }
    let (sheet, area) = target?;
    let (src_sheet, src_range) = source?;
    // As many fields as a table takes (see apex_sheet::pivot).
    fields.truncate(256);
    let tsi = wb.sheet_index(&sheet)?;
    let ssi = wb.sheet_index(&src_sheet)?;
    let names: Vec<String> = (0..src_range.cols()).map(|c| wb.display(ssi, CellRef::new(src_range.start.row, src_range.start.col + c)).0).collect();
    let col_of = |n: &str| names.iter().position(|x| x == n).or_else(|| names.iter().position(|x| x.eq_ignore_ascii_case(n))).map(|c| c as u32);
    let mut spec = PivotSpec { source_sheet: wb.sheets[ssi].name.clone(), source: src_range, grand_rows: grand == "both" || grand == "row", grand_cols: grand == "both" || grand == "column", ..PivotSpec::default() };
    let field = |f: &PField| -> Option<Field> {
        let mut out = Field::new(col_of(&f.name)?);
        out.name = f.display_name.clone();
        out.hidden = f.members.iter().filter(|(_, on)| !on).map(|(m, _)| m.clone()).collect();
        Some(out)
    };
    let data: Vec<&PField> = fields.iter().filter(|f| f.orientation == "data" && !f.layout_field).collect();
    let captions: Vec<String> = data.iter().map(|f| f.display_name.clone().unwrap_or_default()).collect();
    // Captions LibreOffice makes up for value fields without a name.
    let lo_names: Vec<String> = data
        .iter()
        .map(|f| {
            let func = FUNCS.iter().find(|(_, k)| *k == f.function).map_or(AggFunc::Sum, |(g, _)| *g);
            lo_caption(&ValueField { col: col_of(&f.name).unwrap_or(0), func, ..Default::default() }, &names, &spec)
        })
        .collect();
    let value_index = |d: &str| captions.iter().position(|c| !c.is_empty() && c == d).or_else(|| lo_names.iter().position(|c| c == d)).unwrap_or(0);
    let mut first_layout = None;
    let mut any_sub = false;
    for f in fields.iter().filter(|f| !f.layout_field) {
        match f.orientation.as_str() {
            "row" | "column" => {
                let mut fl = field(f)?;
                if let Some((mode, order, d)) = &f.sort {
                    let desc = order == "descending";
                    fl.sort = match mode.as_str() {
                        "data" if desc => SortOrder::ValueDesc(value_index(d)),
                        "data" => SortOrder::ValueAsc(value_index(d)),
                        _ if desc => SortOrder::LabelDesc,
                        _ => SortOrder::LabelAsc,
                    };
                }
                if let Some((d, n, top)) = &f.top {
                    fl.value_filter = Some(ValueFilter { value: value_index(d), op: if *top { ValueOp::TopItems } else { ValueOp::BottomItems }, a: *n as f64, b: 0.0 });
                }
                any_sub |= f.subtotals;
                if f.orientation == "row" {
                    first_layout = first_layout.or(f.layout.clone());
                    spec.rows.push(fl);
                } else {
                    spec.cols.push(fl);
                }
            }
            "page" => {
                let mut fl = field(f)?;
                let items = match &f.selected {
                    Some(s) => vec![s.clone()],
                    None if fl.hidden.is_empty() => Vec::new(),
                    None => f.members.iter().filter(|(_, on)| *on).map(|(m, _)| m.clone()).collect(),
                };
                fl.hidden.clear();
                spec.filters.push(FieldFilter { field: fl, items });
            }
            "data" => {
                let func = FUNCS.iter().find(|(_, k)| *k == f.function).map(|(g, _)| *g)?;
                let show_as = match &f.reference {
                    None => ShowAs::Normal,
                    Some((ty, fname, mtype, mname)) => {
                        let base = || {
                            let c = col_of(fname);
                            if let Some(l) = spec.rows.iter().position(|x| Some(x.col) == c) {
                                BaseField::Row(l)
                            } else if let Some(l) = spec.cols.iter().position(|x| Some(x.col) == c) {
                                BaseField::Col(l)
                            } else {
                                BaseField::Row(0)
                            }
                        };
                        let item = || match mtype.as_str() {
                            "next" => BaseItem::Next,
                            "named" => BaseItem::Named(mname.clone()),
                            _ => BaseItem::Previous,
                        };
                        match ty.as_str() {
                            "member-difference" => ShowAs::DifferenceFrom(base(), item()),
                            "member-percentage" => ShowAs::PercentOf(base(), item()),
                            "member-percentage-difference" => ShowAs::PercentDifferenceFrom(base(), item()),
                            "running-total" => ShowAs::RunningTotal(base()),
                            "row-percentage" => ShowAs::PercentOfRow,
                            "column-percentage" => ShowAs::PercentOfCol,
                            "total-percentage" => ShowAs::PercentOfGrand,
                            "index" => ShowAs::Index,
                            _ => ShowAs::Normal,
                        }
                    }
                };
                spec.values.push(ValueField { col: col_of(&f.name)?, func, show_as, name: f.display_name.clone(), number_format: None });
            }
            _ => {}
        }
    }
    spec.layout = match first_layout {
        Some((_, true)) => Layout::Compact,
        Some((m, _)) if m == "tabular-layout" => Layout::Tabular,
        Some(_) => Layout::Outline,
        None => Layout::Compact,
    };
    spec.subtotals = any_sub || (spec.rows.is_empty() && spec.cols.is_empty());
    Some(StoredPivot { spec, sheet: wb.sheets[tsi].name.clone(), at: area.start, area: Some(area), charts: Vec::new() })
}

fn read_field_attrs(t: &Tag) -> PField {
    let a = |k: &str| t.attr(k).map(|v| v.into_owned());
    PField {
        name: a("table:source-field-name").unwrap_or_default(),
        orientation: a("table:orientation").unwrap_or_default(),
        function: a("table:function").unwrap_or_default(),
        layout_field: t.attr("table:is-data-layout-field").is_some_and(|v| v == "true"),
        selected: a("table:selected-page").filter(|_| t.attr("loext:ignore-selected-page").is_none_or(|v| v != "true")),
        display_name: a("tableooo:display-name"),
        ..PField::default()
    }
}
