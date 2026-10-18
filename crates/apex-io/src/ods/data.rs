//! ODS parts for sheet data tools: content validations, database ranges
//! with autofilters, and LibreOffice's `calcext:conditional-formats`.

use std::fmt::Write as _;

use apex_sheet::data::{AutoFilter, CfKind, CfRule, CfStyle, Cfvo, CmpOp, ColumnFilter, ErrorStyle, FilterKind, Period, TextOp, ValKind, Validation};
use apex_sheet::formula::parse_canonical;
use apex_sheet::functions::CritOp;
use apex_sheet::{CellRef, Range, Sheet, Workbook};

use super::{Styles, odf_to_canonical};
use crate::parse_hex_color;
use crate::xml::{Ev, Reader, Tag, esc};

// ---------------------------------------------------------------------------
// Helpers

/// Splits `s` at top-level occurrences of any of `seps` (outside quotes,
/// brackets and parentheses).
pub(super) fn split_top(s: &str, seps: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut q = false;
    // Quoted sheet names ('My sheet'.A1).
    let mut sq = false;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '"' if !sq => q = !q,
            '\'' if !q => sq = !sq,
            '(' | '[' if !q && !sq => depth += 1,
            ')' | ']' if !q && !sq => depth -= 1,
            _ if !q && !sq && depth == 0 && seps.contains(&c) => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

/// Contents of `name(...)` when `s` is exactly that call.
fn call<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    s.trim().strip_prefix(name)?.trim_start().strip_prefix('(')?.strip_suffix(')')
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    match s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        Some(x) => x.replace("\"\"", "\""),
        None => s.to_string(),
    }
}

/// Canonical formula → ODF expression without the `of:=` prefix.
fn to_odf(f: &str) -> String {
    let f = f.trim();
    let f = f.strip_prefix('=').unwrap_or(f);
    match parse_canonical(f) {
        Ok(e) => e.to_odf().trim_start_matches("of:=").to_string(),
        Err(_) => f.to_string(),
    }
}

fn strip_ns(s: &str) -> &str {
    s.split_once(':').filter(|(ns, _)| matches!(*ns, "of" | "oooc" | "msoxl")).map_or(s, |(_, r)| r)
}

/// Parses an ODF cell range address list ("Sheet1.A1:Sheet1.B4 Sheet1.D2").
/// Returns the sheet name of the first part and the ranges.
pub(super) fn parse_ranges(s: &str) -> (Option<String>, Vec<Range>) {
    let mut sheet = None;
    let mut out = Vec::new();
    for part in split_top(s, &[' ']) {
        if part.is_empty() {
            continue;
        }
        let canon = super::convert_ref(&part);
        let (sh, addr) = match canon.rsplit_once('!') {
            Some((a, b)) => (Some(a.trim_matches('\'').replace("''", "'")), b.to_string()),
            None => (None, canon.clone()),
        };
        if sheet.is_none() {
            sheet = sh;
        }
        let addr = addr.replace('$', "");
        if let Some(r) = Range::parse(&addr).or_else(|| CellRef::parse(&addr).map(Range::single)) {
            out.push(r);
        }
    }
    (sheet, out)
}

fn quote_sheet(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()) {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

pub(super) fn range_addr(sheet: &str, r: &Range) -> String {
    let s = quote_sheet(sheet);
    format!("{s}.{}:{s}.{}", r.start.to_a1(), r.end.to_a1())
}

pub(super) fn ranges_addr(sheet: &str, v: &[Range]) -> String {
    v.iter().map(|r| range_addr(sheet, r)).collect::<Vec<_>>().join(" ")
}

fn cell_addr(sheet: &str, c: CellRef) -> String {
    format!("{}.{}", quote_sheet(sheet), c.to_a1())
}

fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xFF_FFFF)
}

/// Merges cell rectangles into as few ranges as possible.
pub(super) fn merge_rects(mut v: Vec<Range>) -> Vec<Range> {
    for vertical in [true, false] {
        if vertical {
            v.sort_by_key(|r| (r.start.col, r.end.col, r.start.row));
        } else {
            v.sort_by_key(|r| (r.start.row, r.end.row, r.start.col));
        }
        let mut out: Vec<Range> = Vec::with_capacity(v.len());
        for r in v {
            if let Some(last) = out.last_mut() {
                let joins = if vertical {
                    last.start.col == r.start.col && last.end.col == r.end.col && last.end.row + 1 == r.start.row
                } else {
                    last.start.row == r.start.row && last.end.row == r.end.row && last.end.col + 1 == r.start.col
                };
                if joins {
                    last.end = r.end;
                    continue;
                }
            }
            out.push(r);
        }
        v = out;
    }
    v
}

// ---------------------------------------------------------------------------
// Validations

/// Reads a `table:content-validation` (its start tag already consumed).
pub(super) fn read_validation(r: &mut Reader, t: &Tag, empty: bool) -> (String, Validation) {
    let name = t.attr("table:name").map(|v| v.into_owned()).unwrap_or_default();
    let mut v = Validation::new(Vec::new());
    v.allow_blank = t.attr("table:allow-empty-cell").is_none_or(|x| x == "true");
    v.dropdown = t.attr("table:display-list").is_none_or(|x| x != "none");
    v.show_input = false;
    v.show_error = false;
    if let Some(cond) = t.attr("table:condition") {
        parse_condition(strip_ns(&cond), &mut v);
    }
    if empty {
        return (name, v);
    }
    let mut depth = 1;
    let mut section = 0u8;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => {
                depth += 1;
                match t.name {
                    "table:help-message" | "table:error-message" => {
                        let err = t.name == "table:error-message";
                        section = if err { 2 } else { 1 };
                        let title = t.attr("table:title").map(|v| v.chars().take(4096).collect()).unwrap_or_default();
                        let display = t.attr("table:display").is_some_and(|x| x == "true");
                        if err {
                            v.error_title = title;
                            v.show_error = display;
                            v.error_style = match t.attr("table:message-type").as_deref() {
                                Some("warning") => ErrorStyle::Warning,
                                Some("information") => ErrorStyle::Info,
                                _ => ErrorStyle::Stop,
                            };
                        } else {
                            v.input_title = title;
                            v.show_input = display;
                        }
                    }
                    "text:p" => {
                        depth -= 1;
                        let text = r.text();
                        let target = if section == 2 { &mut v.error_msg } else { &mut v.input_msg };
                        // A message's worth (each sheet using it copies it).
                        if target.len() >= 4096 {
                            continue;
                        }
                        if !target.is_empty() {
                            target.push('\n');
                        }
                        target.extend(text.chars().take(4096));
                    }
                    _ => {}
                }
            }
            Ev::Empty(t) => match t.name {
                "table:help-message" => {
                    v.input_title = t.attr("table:title").map(|v| v.chars().take(4096).collect()).unwrap_or_default();
                    v.show_input = t.attr("table:display").is_some_and(|x| x == "true");
                }
                "table:error-message" => {
                    v.error_title = t.attr("table:title").map(|v| v.chars().take(4096).collect()).unwrap_or_default();
                    v.show_error = t.attr("table:display").is_some_and(|x| x == "true");
                    v.error_style = match t.attr("table:message-type").as_deref() {
                        Some("warning") => ErrorStyle::Warning,
                        Some("information") => ErrorStyle::Info,
                        _ => ErrorStyle::Stop,
                    };
                }
                _ => {}
            },
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    (name, v)
}

fn op_prefix(s: &str) -> Option<(CmpOp, &str)> {
    for (p, op) in [("<=", CmpOp::LessEq), (">=", CmpOp::GreaterEq), ("!=", CmpOp::NotEqual), ("<>", CmpOp::NotEqual), ("<", CmpOp::Less), (">", CmpOp::Greater), ("=", CmpOp::Equal)] {
        if let Some(rest) = s.strip_prefix(p) {
            return Some((op, rest));
        }
    }
    None
}

fn two_args(inner: &str) -> (String, String) {
    let a = split_top(inner, &[',', ';']);
    (a.first().map(|x| odf_to_canonical(x)).unwrap_or_default(), a.get(1).map(|x| odf_to_canonical(x)).unwrap_or_default())
}

fn parse_condition(cond: &str, v: &mut Validation) {
    for part in split_top(cond, &[' ']).join(" ").split(" and ") {
        let p = part.trim();
        if p.starts_with("cell-content-is-whole-number") {
            v.kind = ValKind::Whole;
        } else if p.starts_with("cell-content-is-decimal-number") {
            v.kind = ValKind::Decimal;
        } else if p.starts_with("cell-content-is-date") {
            v.kind = ValKind::Date;
        } else if p.starts_with("cell-content-is-time") {
            v.kind = ValKind::Time;
        } else if let Some(inner) = call(p, "cell-content-is-in-list") {
            v.kind = ValKind::List;
            if inner.contains('[') {
                v.f1 = odf_to_canonical(inner);
            } else {
                let items: Vec<String> = split_top(inner, &[';']).iter().map(|x| unquote(x).replace('"', "\"\"")).collect();
                v.f1 = format!("\"{}\"", items.join(","));
            }
        } else if let Some(inner) = call(p, "cell-content-text-length-is-between") {
            v.kind = ValKind::TextLength;
            v.op = CmpOp::Between;
            (v.f1, v.f2) = two_args(inner);
        } else if let Some(inner) = call(p, "cell-content-text-length-is-not-between") {
            v.kind = ValKind::TextLength;
            v.op = CmpOp::NotBetween;
            (v.f1, v.f2) = two_args(inner);
        } else if let Some(rest) = p.strip_prefix("cell-content-text-length()") {
            v.kind = ValKind::TextLength;
            if let Some((op, x)) = op_prefix(rest.trim()) {
                v.op = op;
                v.f1 = odf_to_canonical(x);
            }
        } else if let Some(inner) = call(p, "cell-content-is-between") {
            v.op = CmpOp::Between;
            (v.f1, v.f2) = two_args(inner);
        } else if let Some(inner) = call(p, "cell-content-is-not-between") {
            v.op = CmpOp::NotBetween;
            (v.f1, v.f2) = two_args(inner);
        } else if let Some(rest) = p.strip_prefix("cell-content()") {
            if let Some((op, x)) = op_prefix(rest.trim()) {
                v.op = op;
                v.f1 = odf_to_canonical(x);
            }
        } else if let Some(inner) = call(p, "is-true-formula") {
            v.kind = ValKind::Custom;
            v.f1 = odf_to_canonical(inner);
        }
    }
    if v.kind == ValKind::Any && !v.f1.is_empty() {
        v.kind = ValKind::Decimal;
    }
}

fn odf_op(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Equal => "=",
        CmpOp::NotEqual => "!=",
        CmpOp::Greater => ">",
        CmpOp::Less => "<",
        CmpOp::GreaterEq => ">=",
        CmpOp::LessEq => "<=",
        CmpOp::Between | CmpOp::NotBetween => "",
    }
}

fn condition(v: &Validation) -> Option<String> {
    let bounds = |len: bool| -> String {
        let (a, b) = (to_odf(&v.f1), to_odf(&v.f2));
        match (v.op, len) {
            (CmpOp::Between, false) => format!("cell-content-is-between({a},{b})"),
            (CmpOp::NotBetween, false) => format!("cell-content-is-not-between({a},{b})"),
            (CmpOp::Between, true) => format!("cell-content-text-length-is-between({a},{b})"),
            (CmpOp::NotBetween, true) => format!("cell-content-text-length-is-not-between({a},{b})"),
            (op, false) => format!("cell-content(){}{a}", odf_op(op)),
            (op, true) => format!("cell-content-text-length(){}{a}", odf_op(op)),
        }
    };
    let c = match v.kind {
        ValKind::Any => return None,
        ValKind::Whole => format!("cell-content-is-whole-number() and {}", bounds(false)),
        ValKind::Decimal => format!("cell-content-is-decimal-number() and {}", bounds(false)),
        ValKind::Date => format!("cell-content-is-date() and {}", bounds(false)),
        ValKind::Time => format!("cell-content-is-time() and {}", bounds(false)),
        ValKind::TextLength => bounds(true),
        ValKind::List => match v.literal_items() {
            Some(items) => format!("cell-content-is-in-list({})", items.iter().map(|i| format!("\"{}\"", i.replace('"', "\"\""))).collect::<Vec<_>>().join(";")),
            None => format!("cell-content-is-in-list({})", to_odf(&v.f1)),
        },
        ValKind::Custom => format!("is-true-formula({})", to_odf(&v.f1)),
    };
    Some(format!("of:{c}"))
}

fn write_p(s: &mut String, text: &str) {
    for line in text.split('\n') {
        let _ = write!(s, "<text:p>{}</text:p>", esc(line));
    }
}

/// `<table:content-validations>` for every sheet; names are `val{sheet}_{i}`.
pub(super) fn write_validations(wb: &Workbook) -> String {
    let mut s = String::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        for (i, v) in sh.validations.iter().enumerate() {
            let base = v.ranges.iter().map(|r| r.start).min_by_key(|c| (c.row, c.col)).unwrap_or_default();
            let _ = write!(s, r#"<table:content-validation table:name="val{si}_{i}""#);
            if let Some(c) = condition(v) {
                let _ = write!(s, r#" table:condition="{}""#, esc(&c));
            }
            let _ = write!(s, r#" table:allow-empty-cell="{}""#, v.allow_blank);
            if v.kind == ValKind::List {
                s.push_str(if v.dropdown { r#" table:display-list="unsorted""# } else { r#" table:display-list="none""# });
            }
            let _ = write!(s, r#" table:base-cell-address="{}">"#, esc(&cell_addr(&sh.name, base)));
            let _ = write!(s, r#"<table:help-message table:title="{}" table:display="{}">"#, esc(&v.input_title), v.show_input);
            write_p(&mut s, &v.input_msg);
            let kind = match v.error_style {
                ErrorStyle::Stop => "stop",
                ErrorStyle::Warning => "warning",
                ErrorStyle::Info => "information",
            };
            let _ = write!(s, r#"</table:help-message><table:error-message table:message-type="{kind}" table:title="{}" table:display="{}">"#, esc(&v.error_title), v.show_error);
            write_p(&mut s, &v.error_msg);
            s.push_str("</table:error-message></table:content-validation>");
        }
    }
    if s.is_empty() { s } else { format!("<table:content-validations>{s}</table:content-validations>") }
}

// ---------------------------------------------------------------------------
// Autofilter (database ranges)

struct Cond {
    field: u32,
    op: String,
    value: String,
    number: bool,
    items: Vec<String>,
    /// The enclosing group is `filter-and`.
    and: bool,
}

/// Reads a `table:database-range` into its sheet's filter.
pub(super) fn read_database_range(r: &mut Reader, t: &Tag, empty: bool, wb: &mut Workbook) {
    let buttons = t.attr("table:display-filter-buttons").is_some_and(|v| v == "true");
    let name = t.attr("table:name").map(|v| v.into_owned()).unwrap_or_default();
    let header = t.attr("table:contains-header").is_none_or(|v| v != "false");
    let (sheet, ranges) = parse_ranges(&t.attr("table:target-range-address").unwrap_or_default());
    let mut conds: Vec<Cond> = Vec::new();
    if !empty {
        let mut groups: Vec<bool> = vec![true];
        let mut depth = 1;
        loop {
            let ev = r.next();
            let is_empty = matches!(ev, Ev::Empty(_));
            match ev {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) => {
                    if !is_empty {
                        depth += 1;
                    }
                    match t.name {
                        "table:filter-and" => groups.push(true),
                        "table:filter-or" => groups.push(false),
                        "table:filter-condition" => conds.push(Cond {
                            field: t.attr("table:field-number").and_then(|v| v.parse().ok()).unwrap_or(0),
                            op: t.attr("table:operator").map(|v| v.into_owned()).unwrap_or_else(|| "=".into()),
                            value: t.attr("table:value").map(|v| v.into_owned()).unwrap_or_default(),
                            number: t.attr("table:data-type").is_some_and(|v| v == "number"),
                            items: Vec::new(),
                            and: *groups.last().unwrap_or(&true),
                        }),
                        "table:filter-set-item" => {
                            if let (Some(c), Some(v)) = (conds.last_mut(), t.attr("table:value")) {
                                c.items.push(v.into_owned());
                            }
                        }
                        _ => {}
                    }
                    if is_empty && matches!(t.name, "table:filter-and" | "table:filter-or") {
                        groups.pop();
                    }
                }
                Ev::End(n) => {
                    if matches!(n, "table:filter-and" | "table:filter-or") {
                        groups.pop();
                    }
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let Some(si) = sheet.and_then(|n| wb.sheet_index(&n)) else { return };
    let Some(range) = ranges.first().copied() else { return };
    // Named ranges are tables.
    if !name.starts_with("__Anonymous_Sheet_DB__") {
        super::tables::add_table(wb, si, &name, range, header, buttons);
    }
    if !buttons {
        return;
    }
    let mut f = AutoFilter::new(range);
    // The conditions by field, in the range's columns.
    let mut by_field: std::collections::BTreeMap<u32, Vec<&Cond>> = std::collections::BTreeMap::new();
    for c in conds.iter().filter(|c| c.field < range.cols()) {
        by_field.entry(c.field).or_default().push(c);
    }
    for (field, cs) in by_field {
        let col = range.start.col + field;
        let mut items = Vec::new();
        let mut blanks = false;
        let mut customs: Vec<(CritOp, String)> = Vec::new();
        let mut top = None;
        for c in &cs {
            match c.op.as_str() {
                "=" if !c.items.is_empty() => {
                    for i in &c.items {
                        if i.is_empty() {
                            blanks = true;
                        } else {
                            items.push(i.clone());
                        }
                    }
                }
                "=" if !c.number => items.push(c.value.clone()),
                "empty" => blanks = true,
                "!empty" => customs.push((CritOp::Ne, String::new())),
                "begins-with" => customs.push((CritOp::Eq, format!("{}*", c.value))),
                "ends-with" => customs.push((CritOp::Eq, format!("*{}", c.value))),
                "contains" => customs.push((CritOp::Eq, format!("*{}*", c.value))),
                "does-not-begin-with" => customs.push((CritOp::Ne, format!("{}*", c.value))),
                "does-not-end-with" => customs.push((CritOp::Ne, format!("*{}", c.value))),
                "does-not-contain" => customs.push((CritOp::Ne, format!("*{}*", c.value))),
                "top values" | "bottom values" | "top percent" | "bottom percent" => {
                    top = Some(FilterKind::Top { top: c.op.starts_with("top"), percent: c.op.ends_with("percent"), n: c.value.parse().unwrap_or(10.0) });
                }
                op => {
                    let o = match op {
                        "!=" => CritOp::Ne,
                        "<" => CritOp::Lt,
                        ">" => CritOp::Gt,
                        "<=" => CritOp::Le,
                        ">=" => CritOp::Ge,
                        _ => CritOp::Eq,
                    };
                    customs.push((o, c.value.clone()));
                }
            }
        }
        let kind = if let Some(t) = top {
            t
        } else if !items.is_empty() || (blanks && customs.is_empty()) {
            FilterKind::Values { items, blanks }
        } else if let Some(a) = customs.first().cloned() {
            FilterKind::Custom { and: cs.first().is_some_and(|c| c.and), a, b: customs.get(1).cloned() }
        } else {
            continue;
        };
        f.cols.push(ColumnFilter { col, kind });
    }
    wb.sheets[si].filter = Some(f);
}

fn filter_cond(field: u32, op: &str, value: &str, number: bool) -> String {
    format!(r#"<table:filter-condition table:field-number="{field}"{} table:operator="{}" table:value="{}"/>"#, if number { r#" table:data-type="number""# } else { "" }, esc(op), esc(value))
}

fn custom_cond(field: u32, op: CritOp, v: &str) -> String {
    let inner = |p: &str, s: &str| p.strip_prefix(s).map(str::to_string);
    let (name, value) = match op {
        CritOp::Eq | CritOp::Ne => {
            let ne = op == CritOp::Ne;
            if v.len() > 1 && v.starts_with('*') && v.ends_with('*') {
                (if ne { "does-not-contain" } else { "contains" }, v[1..v.len() - 1].to_string())
            } else if let Some(x) = v.strip_suffix('*').filter(|x| !x.contains(['*', '?'])) {
                (if ne { "does-not-begin-with" } else { "begins-with" }, x.to_string())
            } else if let Some(x) = inner(v, "*").filter(|x| !x.contains(['*', '?'])) {
                (if ne { "does-not-end-with" } else { "ends-with" }, x)
            } else if v.is_empty() {
                (if ne { "!empty" } else { "empty" }, String::new())
            } else {
                (if ne { "!=" } else { "=" }, v.to_string())
            }
        }
        CritOp::Lt => ("<", v.to_string()),
        CritOp::Gt => (">", v.to_string()),
        CritOp::Le => ("<=", v.to_string()),
        CritOp::Ge => (">=", v.to_string()),
    };
    let number = value.parse::<f64>().is_ok() && !name.contains("with") && !name.contains("contain");
    filter_cond(field, name, &value, number)
}

/// `<table:database-ranges>` holding the autofilters.
pub(super) fn write_database_ranges(wb: &Workbook) -> String {
    let mut s = String::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        // Tables are named database ranges (without their totals row).
        for t in &sh.tables {
            let f = sh.filter.as_ref().filter(|f| t.filter_button && f.range == t.filter_range());
            let _ = write!(s, r#"<table:database-range table:name="{}" table:target-range-address="{}" table:contains-header="{}" table:display-filter-buttons="{}""#, esc(&t.name), esc(&range_addr(&sh.name, &t.filter_range())), t.header, t.filter_button && t.header);
            match f {
                Some(f) => write_filter_conds(&mut s, f),
                None => s.push_str("/>"),
            }
        }
        let Some(f) = &sh.filter else { continue };
        if sh.tables.iter().any(|t| t.filter_button && f.range == t.filter_range()) {
            continue;
        }
        let _ = write!(s, r#"<table:database-range table:name="__Anonymous_Sheet_DB__{si}" table:target-range-address="{}" table:display-filter-buttons="true""#, esc(&range_addr(&sh.name, &f.range)));
        write_filter_conds(&mut s, f);
    }
    if s.is_empty() { s } else { format!("<table:database-ranges>{s}</table:database-ranges>") }
}

/// The filter conditions of a database range and its closing tag.
fn write_filter_conds(s: &mut String, f: &AutoFilter) {
    {
        let mut conds = String::new();
        for c in &f.cols {
            let field = c.col.saturating_sub(f.range.start.col);
            match &c.kind {
                FilterKind::Values { items, blanks } => {
                    // LibreOffice keeps one condition per column: blanks are
                    // an empty set item.
                    match items.first() {
                        Some(first) => {
                            let _ = write!(conds, r#"<table:filter-condition table:field-number="{field}" table:operator="=" table:value="{}">"#, esc(first));
                            for i in items {
                                let _ = write!(conds, r#"<table:filter-set-item table:value="{}"/>"#, esc(i));
                            }
                            if *blanks {
                                conds.push_str(r#"<table:filter-set-item table:value=""/>"#);
                            }
                            conds.push_str("</table:filter-condition>");
                        }
                        None if *blanks => conds.push_str(&filter_cond(field, "empty", "", false)),
                        None => {}
                    }
                }
                FilterKind::Custom { and, a, b } => {
                    let x = custom_cond(field, a.0, &a.1);
                    match b {
                        Some(b) if !*and => {
                            let _ = write!(conds, "<table:filter-or>{x}{}</table:filter-or>", custom_cond(field, b.0, &b.1));
                        }
                        Some(b) => {
                            conds.push_str(&x);
                            conds.push_str(&custom_cond(field, b.0, &b.1));
                        }
                        None => conds.push_str(&x),
                    }
                }
                FilterKind::Top { top, percent, n } => {
                    let op = format!("{} {}", if *top { "top" } else { "bottom" }, if *percent { "percent" } else { "values" });
                    conds.push_str(&filter_cond(field, &op, &n.to_string(), true));
                }
                // No ODF equivalent.
                FilterKind::Average { .. } | FilterKind::Color { .. } => {}
            }
        }
        if conds.is_empty() {
            s.push_str("/>");
        } else {
            let _ = write!(s, "><table:filter><table:filter-and>{conds}</table:filter-and></table:filter></table:database-range>");
        }
    }
}

// ---------------------------------------------------------------------------
// Conditional formats

fn read_cfvo(t: &Tag) -> Cfvo {
    let val = t.attr("calcext:value").map(|v| v.into_owned()).unwrap_or_default();
    let n = val.parse::<f64>().unwrap_or(0.0);
    match t.attr("calcext:type").as_deref() {
        Some("maximum") | Some("auto-maximum") => Cfvo::Max,
        Some("number") => Cfvo::Num(n),
        Some("percent") => Cfvo::Percent(n),
        Some("percentile") => Cfvo::Percentile(n),
        Some("formula") => Cfvo::Formula(odf_to_canonical(&val)),
        _ => Cfvo::Min,
    }
}

fn cfvo_attrs(c: &Cfvo) -> String {
    let (ty, v) = match c {
        Cfvo::Min => ("minimum", "0".to_string()),
        Cfvo::Max => ("maximum", "0".to_string()),
        Cfvo::Num(n) => ("number", n.to_string()),
        Cfvo::Percent(n) => ("percent", n.to_string()),
        Cfvo::Percentile(n) => ("percentile", n.to_string()),
        Cfvo::Formula(f) => ("formula", to_odf(f)),
    };
    format!(r#" calcext:value="{}" calcext:type="{ty}""#, esc(&v))
}

fn period_name(p: Period) -> &'static str {
    match p {
        Period::Yesterday => "yesterday",
        Period::Today => "today",
        Period::Tomorrow => "tomorrow",
        Period::Last7Days => "last-7-days",
        Period::LastWeek => "last-week",
        Period::ThisWeek => "this-week",
        Period::NextWeek => "next-week",
        Period::LastMonth => "last-month",
        Period::ThisMonth => "this-month",
        Period::NextMonth => "next-month",
    }
}

fn cf_style(st: &Styles, name: &str) -> CfStyle {
    let c = st.resolve(name);
    CfStyle {
        color: c.color,
        fill: c.fill,
        bold: c.bold.then_some(true),
        italic: c.italic.then_some(true),
        underline: c.underline.then_some(true),
        strike: c.strike.then_some(true),
    }
}

fn parse_cf_value(v: &str) -> Option<CfKind> {
    let v = v.trim();
    let num = |x: &str| x.trim().parse::<u32>().unwrap_or(10);
    Some(if let Some(inner) = call(v, "formula-is") {
        let f = odf_to_canonical(inner);
        let compact: String = f.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.starts_with("LEN(TRIM(") && compact.ends_with("))=0") {
            CfKind::Blanks(true)
        } else if compact.starts_with("LEN(TRIM(") && compact.ends_with("))>0") {
            CfKind::Blanks(false)
        } else {
            CfKind::Expression(f)
        }
    } else if let Some(inner) = call(v, "between") {
        let (f1, f2) = two_args(inner);
        CfKind::CellIs { op: CmpOp::Between, f1, f2 }
    } else if let Some(inner) = call(v, "not-between") {
        let (f1, f2) = two_args(inner);
        CfKind::CellIs { op: CmpOp::NotBetween, f1, f2 }
    } else if let Some(inner) = call(v, "begins-with") {
        CfKind::Text { op: TextOp::BeginsWith, text: unquote(inner) }
    } else if let Some(inner) = call(v, "ends-with") {
        CfKind::Text { op: TextOp::EndsWith, text: unquote(inner) }
    } else if let Some(inner) = call(v, "contains-text") {
        CfKind::Text { op: TextOp::Contains, text: unquote(inner) }
    } else if let Some(inner) = call(v, "not-contains-text") {
        CfKind::Text { op: TextOp::NotContains, text: unquote(inner) }
    } else if let Some(inner) = call(v, "top-elements") {
        CfKind::Top { bottom: false, percent: false, rank: num(inner) }
    } else if let Some(inner) = call(v, "bottom-elements") {
        CfKind::Top { bottom: true, percent: false, rank: num(inner) }
    } else if let Some(inner) = call(v, "top-percent") {
        CfKind::Top { bottom: false, percent: true, rank: num(inner) }
    } else if let Some(inner) = call(v, "bottom-percent") {
        CfKind::Top { bottom: true, percent: true, rank: num(inner) }
    } else {
        match v {
            "is-error" => CfKind::Errors(true),
            "is-no-error" => CfKind::Errors(false),
            "duplicate" => CfKind::Duplicates(true),
            "unique" => CfKind::Duplicates(false),
            "above-average" => CfKind::Average { below: false, equal: false },
            "below-average" => CfKind::Average { below: true, equal: false },
            "above-equal-average" => CfKind::Average { below: false, equal: true },
            "below-equal-average" => CfKind::Average { below: true, equal: true },
            _ => {
                let (op, rest) = op_prefix(v)?;
                CfKind::CellIs { op, f1: odf_to_canonical(rest), f2: String::new() }
            }
        }
    })
}

/// Reads `calcext:conditional-formats` (start tag consumed) into the sheet.
/// `left`: the ranges the sheet's rules may still copy (from
/// `MAX_CF_RANGES`); rules past them, or of no range, are left out.
pub(super) fn read_cond_formats(r: &mut Reader, st: &Styles, sh: &mut Sheet, left: &mut usize) {
    let mut depth = 1;
    let mut ranges: Vec<Range> = Vec::new();
    let mut share = |ranges: &Vec<Range>| {
        if ranges.is_empty() || ranges.len() > *left {
            return None;
        }
        *left -= ranges.len();
        Some(ranges.clone())
    };
    // Graphic rule being read: kind and entries.
    let mut graphic: Option<(CfKind, Vec<Cfvo>, Vec<u32>, bool)> = None;
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => {
                if !empty {
                    depth += 1;
                }
                match t.name {
                    "calcext:conditional-format" => ranges = parse_ranges(&t.attr("calcext:target-range-address").unwrap_or_default()).1,
                    "calcext:condition" => {
                        if let Some(kind) = t.attr("calcext:value").and_then(|v| parse_cf_value(&v)) {
                            let style = t.attr("calcext:apply-style-name").map(|n| cf_style(st, &n)).unwrap_or_default();
                            if let Some(ranges) = share(&ranges) {
                                sh.cond_formats.push(CfRule { ranges, kind, style, stop: false });
                            }
                        }
                    }
                    "calcext:date-is" => {
                        let p = t.attr("calcext:date").map(|v| v.into_owned()).unwrap_or_default();
                        if let Some(p) = Period::ALL.into_iter().find(|x| period_name(*x) == p) {
                            let style = t.attr("calcext:style").map(|n| cf_style(st, &n)).unwrap_or_default();
                            if let Some(ranges) = share(&ranges) {
                                sh.cond_formats.push(CfRule { ranges, kind: CfKind::Period(p), style, stop: false });
                            }
                        }
                    }
                    "calcext:color-scale" => graphic = Some((CfKind::ColorScale(Vec::new()), Vec::new(), Vec::new(), true)),
                    "calcext:color-scale-entry" => {
                        if let Some(g) = &mut graphic {
                            g.1.push(read_cfvo(&t));
                            g.2.push(t.attr("calcext:color").and_then(|v| parse_hex_color(&v)).unwrap_or(0));
                        }
                    }
                    "calcext:data-bar" => {
                        let color = t.attr("calcext:positive-color").and_then(|v| parse_hex_color(&v)).unwrap_or(0x638EC6);
                        graphic = Some((CfKind::DataBar { color, min: Cfvo::Min, max: Cfvo::Max }, Vec::new(), Vec::new(), true));
                    }
                    "calcext:icon-set" => {
                        let set = t.attr("calcext:icon-set-type").map(|v| v.into_owned()).unwrap_or_else(|| "3TrafficLights1".into());
                        graphic = Some((CfKind::IconSet { set, reverse: false, show_value: true, points: Vec::new() }, Vec::new(), Vec::new(), true));
                    }
                    "calcext:formatting-entry" => {
                        if let Some(g) = &mut graphic {
                            if t.attr("calcext:show-value").is_some_and(|v| v == "false") {
                                g.3 = false;
                            }
                            g.1.push(read_cfvo(&t));
                        }
                    }
                    _ => {}
                }
            }
            Ev::End(n) => {
                if matches!(n, "calcext:color-scale" | "calcext:data-bar" | "calcext:icon-set") {
                    if let Some((kind, cfvos, colors, show)) = graphic.take() {
                        let kind = match kind {
                            CfKind::ColorScale(_) => CfKind::ColorScale(cfvos.into_iter().zip(colors).collect()),
                            CfKind::DataBar { color, .. } => {
                                let mut it = cfvos.into_iter();
                                CfKind::DataBar { color, min: it.next().unwrap_or(Cfvo::Min), max: it.next().unwrap_or(Cfvo::Max) }
                            }
                            CfKind::IconSet { set, reverse, .. } => CfKind::IconSet { set, reverse, show_value: show, points: cfvos.into_iter().skip(1).collect() },
                            k => k,
                        };
                        if let Some(ranges) = share(&ranges) {
                            sh.cond_formats.push(CfRule { ranges, kind, style: CfStyle::default(), stop: false });
                        }
                    }
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
}

/// Distinct formats used by conditional formatting, written as common
/// styles in `styles.xml`.
pub(super) fn cf_styles(wb: &Workbook) -> Vec<CfStyle> {
    let mut out: Vec<CfStyle> = Vec::new();
    for sh in &wb.sheets {
        for r in &sh.cond_formats {
            if !r.kind.is_graphic() && !out.contains(&r.style) {
                out.push(r.style.clone());
            }
        }
    }
    out
}

pub(super) fn write_cf_styles(styles: &[CfStyle]) -> String {
    let mut s = String::new();
    for (i, st) in styles.iter().enumerate() {
        let _ = write!(s, r#"<style:style style:name="ApexCf{}" style:family="table-cell" style:parent-style-name="Default">"#, i + 1);
        if let Some(f) = st.fill {
            let _ = write!(s, r#"<style:table-cell-properties fo:background-color="{}"/>"#, hex(f));
        }
        let mut text = String::new();
        if st.bold == Some(true) {
            text.push_str(r#" fo:font-weight="bold""#);
        }
        if st.italic == Some(true) {
            text.push_str(r#" fo:font-style="italic""#);
        }
        if st.underline == Some(true) {
            text.push_str(r#" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color""#);
        }
        if st.strike == Some(true) {
            text.push_str(r#" style:text-line-through-style="solid""#);
        }
        if let Some(c) = st.color {
            let _ = write!(text, r#" fo:color="{}""#, hex(c));
        }
        if !text.is_empty() {
            let _ = write!(s, "<style:text-properties{text}/>");
        }
        s.push_str("</style:style>");
    }
    s
}

/// `<calcext:conditional-formats>` for one sheet (inside its table).
pub(super) fn write_cond_formats(sh: &Sheet, styles: &[CfStyle]) -> String {
    if sh.cond_formats.is_empty() {
        return String::new();
    }
    let mut s = String::from("<calcext:conditional-formats>");
    for rule in &sh.cond_formats {
        let _ = write!(s, r#"<calcext:conditional-format calcext:target-range-address="{}">"#, esc(&ranges_addr(&sh.name, &rule.ranges)));
        let style = format!("ApexCf{}", styles.iter().position(|x| *x == rule.style).unwrap_or(0) + 1);
        let base = esc(&cell_addr(&sh.name, rule.origin())).into_owned();
        let a = rule.origin().to_a1();
        let value = match &rule.kind {
            CfKind::CellIs { op, f1, f2 } => Some(match op {
                CmpOp::Between => format!("between({},{})", to_odf(f1), to_odf(f2)),
                CmpOp::NotBetween => format!("not-between({},{})", to_odf(f1), to_odf(f2)),
                op => format!("{}{}", odf_op(*op), to_odf(f1)),
            }),
            CfKind::Expression(f) => Some(format!("formula-is({})", to_odf(f))),
            CfKind::Text { op, text } => {
                let q = format!("\"{}\"", text.replace('"', "\"\""));
                Some(match op {
                    TextOp::Contains => format!("contains-text({q})"),
                    TextOp::NotContains => format!("not-contains-text({q})"),
                    TextOp::BeginsWith => format!("begins-with({q})"),
                    TextOp::EndsWith => format!("ends-with({q})"),
                })
            }
            CfKind::Blanks(b) => Some(format!("formula-is({})", to_odf(&format!("LEN(TRIM({a})){}0", if *b { "=" } else { ">" })))),
            CfKind::Errors(b) => Some(if *b { "is-error" } else { "is-no-error" }.to_string()),
            CfKind::Duplicates(d) => Some(if *d { "duplicate" } else { "unique" }.to_string()),
            CfKind::Top { bottom, percent, rank } => Some(format!("{}-{}({rank})", if *bottom { "bottom" } else { "top" }, if *percent { "percent" } else { "elements" })),
            CfKind::Average { below, equal } => Some(format!("{}{}-average", if *below { "below" } else { "above" }, if *equal { "-equal" } else { "" })),
            CfKind::Period(p) => {
                let _ = write!(s, r#"<calcext:date-is calcext:style="{style}" calcext:date="{}"/>"#, period_name(*p));
                None
            }
            CfKind::ColorScale(stops) => {
                s.push_str("<calcext:color-scale>");
                for (c, col) in stops {
                    let _ = write!(s, r#"<calcext:color-scale-entry{} calcext:color="{}"/>"#, cfvo_attrs(c), hex(*col));
                }
                s.push_str("</calcext:color-scale>");
                None
            }
            CfKind::DataBar { color, min, max } => {
                let _ = write!(
                    s,
                    r##"<calcext:data-bar calcext:min-length="10" calcext:max-length="90" calcext:negative-color="#ff0000" calcext:axis-position="none" calcext:positive-color="{}" calcext:axis-color="#000000"><calcext:formatting-entry{}/><calcext:formatting-entry{}/></calcext:data-bar>"##,
                    hex(*color),
                    cfvo_attrs(min),
                    cfvo_attrs(max)
                );
                None
            }
            CfKind::IconSet { set, show_value, points, .. } => {
                let _ = write!(s, r#"<calcext:icon-set calcext:icon-set-type="{}"><calcext:formatting-entry{} calcext:value="0" calcext:type="percent"/>"#, esc(set), if *show_value { "" } else { r#" calcext:show-value="false""# });
                for c in points {
                    let _ = write!(s, "<calcext:formatting-entry{}/>", cfvo_attrs(c));
                }
                s.push_str("</calcext:icon-set>");
                None
            }
        };
        if let Some(v) = value {
            let _ = write!(s, r#"<calcext:condition calcext:apply-style-name="{style}" calcext:value="{}" calcext:base-cell-address="{base}"/>"#, esc(&v));
        }
        s.push_str("</calcext:conditional-format>");
    }
    s.push_str("</calcext:conditional-formats>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(split_top("a(1,2),b,\"c,d\"", &[',']), vec!["a(1,2)", "b", "\"c,d\""]);
        let (sh, r) = parse_ranges("Sheet1.A2:Sheet1.A4 'My s'.C2");
        assert_eq!(sh.as_deref(), Some("Sheet1"));
        assert_eq!(r, vec![Range::parse("A2:A4").unwrap(), Range::parse("C2").unwrap()]);
        let rects = vec![Range::parse("B2:C2").unwrap(), Range::parse("B3:C3").unwrap(), Range::parse("D2:D3").unwrap(), Range::parse("B5").unwrap()];
        assert_eq!(merge_rects(rects), vec![Range::parse("B2:D3").unwrap(), Range::parse("B5").unwrap()]);
        let mut v = Validation::new(vec![]);
        parse_condition("cell-content-is-whole-number() and cell-content-is-between(1,20)", &mut v);
        assert_eq!((v.kind, v.op, v.f1.as_str(), v.f2.as_str()), (ValKind::Whole, CmpOp::Between, "1", "20"));
        let mut v = Validation::new(vec![]);
        parse_condition("cell-content-is-in-list(\"Sí\";\"No\")", &mut v);
        assert_eq!(v.literal_items().unwrap(), vec!["Sí", "No"]);
        let mut v = Validation::new(vec![]);
        parse_condition("cell-content-text-length()<=[.$B$1]", &mut v);
        assert_eq!((v.kind, v.op, v.f1.as_str()), (ValKind::TextLength, CmpOp::LessEq, "$B$1"));
        assert_eq!(parse_cf_value("!=[.$B$1]"), Some(CfKind::CellIs { op: CmpOp::NotEqual, f1: "$B$1".into(), f2: String::new() }));
    }
}

// ---------------------------------------------------------------------------
// Defined names

/// A `<table:named-range>` or `<table:named-expression>`.
pub(super) fn read_named(t: &Tag<'_>, scope: Option<String>) -> Option<apex_sheet::DefinedName> {
    let name = t.attr("table:name")?.into_owned();
    let expr = if t.name == "table:named-range" {
        format!("[{}]", t.attr("table:cell-range-address")?)
    } else {
        strip_ns(t.attr("table:expression")?.as_ref()).trim_start_matches('=').to_string()
    };
    let mut e = parse_canonical(&super::odf_to_canonical(&format!("of:={expr}"))).ok()?;
    // Relative references count from the base cell; the model counts from A1.
    if e.has_relative_refs() {
        if let Some(base) = t.attr("table:base-cell-address").and_then(|b| parse_canonical(&super::odf_to_canonical(&format!("of:=[{b}]"))).ok()) {
            if let apex_sheet::Expr::Ref { addr, .. } = base {
                e.offset_wrap(-(addr.row as i64), -(addr.col as i64));
            }
        }
    }
    let mut d = apex_sheet::DefinedName::new(&name, scope, e);
    // LibreOffice's hidden names.
    d.hidden = t.attr("loext:hidden").is_some_and(|v| v == "true") || t.attr("table:range-usable-as").is_some_and(|v| v.split_whitespace().any(|x| x == "hidden"));
    Some(d)
}

/// `<table:named-expressions>` for the names of `scope` (None: the
/// workbook's), with `extra` elements.
pub(super) fn write_named(wb: &Workbook, scope: Option<&str>, extra: &str) -> String {
    let mut s = String::new();
    for d in wb.names.iter().filter(|d| d.scope.as_deref() == scope) {
        let first = wb.sheets.first().map_or("Sheet1", |s| s.name.as_str());
        let base = format!("${}.$A$1", odf_sheet(scope.unwrap_or(first)));
        let odf = d.expr.to_odf();
        let body = odf.trim_start_matches("of:=");
        // Plain references are named ranges, the rest named expressions.
        match body.strip_prefix('[').and_then(|b| b.strip_suffix(']')).filter(|b| !b.contains(['[', ']']) && d.range().is_some()) {
            Some(addr) => {
                let hidden = if d.hidden { r#" table:range-usable-as="hidden""# } else { "" };
                let _ = write!(s, r#"<table:named-range table:name="{}" table:base-cell-address="{}" table:cell-range-address="{}"{hidden}/>"#, esc(&d.name), esc(&base), esc(addr));
            }
            None => {
                let hidden = if d.hidden { r#" loext:hidden="true""# } else { "" };
                let _ = write!(s, r#"<table:named-expression table:name="{}" table:base-cell-address="{}" table:expression="{}"{hidden}/>"#, esc(&d.name), esc(&base), esc(&odf));
            }
        }
    }
    s.push_str(extra);
    if s.is_empty() { s } else { format!("<table:named-expressions>{s}</table:named-expressions>") }
}

pub(super) fn odf_sheet(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}
