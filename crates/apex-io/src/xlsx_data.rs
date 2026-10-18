//! xlsx parts for sheet data tools: autofilter, data validation,
//! conditional formatting and the differential formats (`dxf`) they use.

use std::fmt::Write as _;

use apex_sheet::data::{AutoFilter, CfKind, CfRule, CfStyle, Cfvo, CmpOp, ColumnFilter, ErrorStyle, FilterKind, Period, TextOp, ValKind, Validation};
use apex_sheet::functions::CritOp;
use apex_sheet::{CellRef, Range, Sheet, Workbook};

use crate::parse_hex_color;
use crate::xml::{Ev, Reader, Tag, esc};

fn flag(t: &Tag, key: &str, default: bool) -> bool {
    t.attr(key).map_or(default, |v| matches!(v.as_ref(), "1" | "true"))
}

fn color_of(t: &Tag) -> Option<u32> {
    t.attr("rgb").and_then(|v| parse_hex_color(&v))
}

fn argb(c: u32) -> String {
    format!("FF{:06X}", c & 0xFF_FFFF)
}

/// Validations a sheet keeps: each cell edited looks them over.
pub(crate) const MAX_VALIDATIONS: usize = 16_384;

/// Ranges of a `sqref` ("A1:B2 D4").
fn sqref(s: &str) -> Vec<Range> {
    s.split_whitespace().filter_map(|p| Range::parse(p).or_else(|| CellRef::parse(p).map(Range::single))).collect()
}

fn sqref_text(v: &[Range]) -> String {
    v.iter().map(|r| if r.is_single() { r.start.to_a1() } else { r.to_a1() }).collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Reading

/// Differential formats of `styles.xml`.
pub fn read_dxfs(xml: &str) -> Vec<CfStyle> {
    let mut out = Vec::new();
    let Some(start) = xml.find("<dxfs") else { return out };
    let mut r = Reader::new(&xml[start..]);
    let mut cur: Option<CfStyle> = None;
    let mut in_font = false;
    let mut in_fill = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::End(n) if crate::xml::local(n) == "dxfs" => break,
            Ev::Start(t) if t.local() == "dxf" => cur = Some(CfStyle::default()),
            Ev::Empty(t) if t.local() == "dxf" => out.push(CfStyle::default()),
            Ev::End(n) => match crate::xml::local(n) {
                "dxf" => out.extend(cur.take()),
                "font" => in_font = false,
                "fill" => in_fill = false,
                _ => {}
            },
            Ev::Start(t) | Ev::Empty(t) => {
                let Some(s) = &mut cur else { continue };
                match t.local() {
                    "font" => in_font = true,
                    "fill" => in_fill = true,
                    "b" if in_font => s.bold = Some(flag(&t, "val", true)),
                    "i" if in_font => s.italic = Some(flag(&t, "val", true)),
                    "strike" if in_font => s.strike = Some(flag(&t, "val", true)),
                    "u" if in_font => s.underline = Some(t.attr("val").is_none_or(|v| v != "none")),
                    "color" if in_font => s.color = color_of(&t),
                    // Solid dxf fills keep their color in bgColor.
                    "bgColor" if in_fill => s.fill = color_of(&t).or(s.fill),
                    "fgColor" if in_fill && s.fill.is_none() => s.fill = color_of(&t),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

fn crit_op(s: Option<&str>) -> CritOp {
    match s {
        Some("lessThan") => CritOp::Lt,
        Some("lessThanOrEqual") => CritOp::Le,
        Some("notEqual") => CritOp::Ne,
        Some("greaterThanOrEqual") => CritOp::Ge,
        Some("greaterThan") => CritOp::Gt,
        _ => CritOp::Eq,
    }
}

fn crit_name(o: CritOp) -> Option<&'static str> {
    match o {
        CritOp::Eq => None,
        CritOp::Ne => Some("notEqual"),
        CritOp::Lt => Some("lessThan"),
        CritOp::Le => Some("lessThanOrEqual"),
        CritOp::Gt => Some("greaterThan"),
        CritOp::Ge => Some("greaterThanOrEqual"),
    }
}

fn cfvo(t: &Tag) -> Cfvo {
    let val = t.attr("val").map(|v| v.into_owned()).unwrap_or_default();
    let num = val.parse::<f64>().unwrap_or(0.0);
    match t.attr("type").as_deref() {
        Some("min") => Cfvo::Min,
        Some("max") => Cfvo::Max,
        Some("num") => match val.parse::<f64>() {
            Ok(n) => Cfvo::Num(n),
            Err(_) => Cfvo::Formula(val),
        },
        Some("percent") => Cfvo::Percent(num),
        Some("percentile") => Cfvo::Percentile(num),
        Some("formula") => Cfvo::Formula(val),
        _ => Cfvo::Min,
    }
}

/// Rule being read, with its raw attributes until its children are known.
struct RawRule {
    ranges: Vec<Range>,
    kind: String,
    op: String,
    text: String,
    period: String,
    priority: i64,
    stop: bool,
    style: CfStyle,
    formulas: Vec<String>,
    rank: u32,
    percent: bool,
    bottom: bool,
    above: bool,
    equal: bool,
    cfvos: Vec<Cfvo>,
    colors: Vec<u32>,
    icon_set: String,
    reverse: bool,
    show_value: bool,
}

impl RawRule {
    fn build(self) -> Option<(i64, CfRule)> {
        let f = |i: usize| self.formulas.get(i).cloned().unwrap_or_default();
        let kind = match self.kind.as_str() {
            "cellIs" => CfKind::CellIs { op: CmpOp::from_xlsx(&self.op), f1: f(0), f2: f(1) },
            "expression" => CfKind::Expression(f(0)),
            "containsText" | "notContainsText" | "beginsWith" | "endsWith" => CfKind::Text {
                op: match self.kind.as_str() {
                    "notContainsText" => TextOp::NotContains,
                    "beginsWith" => TextOp::BeginsWith,
                    "endsWith" => TextOp::EndsWith,
                    _ => TextOp::Contains,
                },
                text: self.text.clone(),
            },
            "containsBlanks" => CfKind::Blanks(true),
            "notContainsBlanks" => CfKind::Blanks(false),
            "containsErrors" => CfKind::Errors(true),
            "notContainsErrors" => CfKind::Errors(false),
            "duplicateValues" => CfKind::Duplicates(true),
            "uniqueValues" => CfKind::Duplicates(false),
            "top10" => CfKind::Top { bottom: self.bottom, percent: self.percent, rank: self.rank },
            "aboveAverage" => CfKind::Average { below: !self.above, equal: self.equal },
            "timePeriod" => CfKind::Period(Period::from_xlsx(&self.period)),
            "colorScale" if self.cfvos.len() >= 2 && self.colors.len() == self.cfvos.len() => CfKind::ColorScale(self.cfvos.iter().cloned().zip(self.colors.iter().copied()).collect()),
            "dataBar" if self.cfvos.len() >= 2 => {
                CfKind::DataBar { color: self.colors.first().copied().unwrap_or(0x638EC6), min: self.cfvos[0].clone(), max: self.cfvos[1].clone() }
            }
            "iconSet" => CfKind::IconSet {
                set: if self.icon_set.is_empty() { "3TrafficLights1".into() } else { self.icon_set.clone() },
                reverse: self.reverse,
                show_value: self.show_value,
                points: self.cfvos.iter().skip(1).cloned().collect(),
            },
            _ => return None,
        };
        Some((self.priority, CfRule { ranges: self.ranges, kind, style: self.style, stop: self.stop }))
    }
}

/// Reads the parts after `</sheetData>`: autofilter, conditional formats
/// and data validations.
pub fn read_tools(xml: &str, sh: &mut Sheet, dxfs: &[CfStyle]) {
    let Some(start) = xml.find("</sheetData>").or_else(|| xml.find("<sheetData/>")) else { return };
    let mut r = Reader::new(&xml[start..]);
    let mut rules: Vec<(i64, CfRule)> = Vec::new();
    let mut ranges: Vec<Range> = Vec::new();
    let mut rule: Option<RawRule> = None;
    let mut dv: Option<Validation> = None;
    let mut filter: Option<AutoFilter> = None;
    let mut col: Option<ColumnFilter> = None;
    let mut customs: Vec<(CritOp, String)> = Vec::new();
    let mut custom_and = false;
    let mut seq = 0i64;
    // Each rule copies its ranges: so many in all, the rest dropped.
    let mut cf_left = crate::MAX_CF_RANGES;
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "autoFilter" => {
                    filter = t.attr("ref").and_then(|v| Range::parse(&v)).map(AutoFilter::new);
                }
                "filterColumn" if !empty => {
                    if let Some(f) = &filter {
                        let id: u32 = t.attr("colId").and_then(|v| v.parse().ok()).unwrap_or(0);
                        col = Some(ColumnFilter { col: f.range.start.col.saturating_add(id), kind: FilterKind::Values { items: Vec::new(), blanks: false } });
                    }
                }
                "filters" => {
                    if let Some(c) = &mut col {
                        c.kind = FilterKind::Values { items: Vec::new(), blanks: flag(&t, "blank", false) };
                    }
                }
                "filter" => {
                    if let Some(ColumnFilter { kind: FilterKind::Values { items, .. }, .. }) = &mut col {
                        items.extend(t.attr("val").map(|v| v.into_owned()));
                    }
                }
                "customFilters" => {
                    custom_and = flag(&t, "and", false);
                    customs.clear();
                }
                "customFilter" => customs.push((crit_op(t.attr("operator").as_deref()), t.attr("val").map(|v| v.into_owned()).unwrap_or_default())),
                "top10" => {
                    if let Some(c) = &mut col {
                        c.kind = FilterKind::Top { top: flag(&t, "top", true), percent: flag(&t, "percent", false), n: t.attr("val").and_then(|v| v.parse().ok()).unwrap_or(10.0) };
                    }
                }
                "dynamicFilter" => {
                    if let Some(c) = &mut col {
                        match t.attr("type").as_deref() {
                            Some("aboveAverage") => c.kind = FilterKind::Average { above: true },
                            Some("belowAverage") => c.kind = FilterKind::Average { above: false },
                            _ => {}
                        }
                    }
                }
                "colorFilter" => {
                    if let Some(c) = &mut col {
                        let font = !flag(&t, "cellColor", true);
                        let d = t.attr("dxfId").and_then(|v| v.parse::<usize>().ok()).and_then(|i| dxfs.get(i));
                        c.kind = FilterKind::Color { font, color: d.and_then(|d| if font { d.color } else { d.fill }) };
                    }
                }
                "sortCondition" => {
                    if let (Some(f), Some(rg)) = (&mut filter, t.attr("ref").and_then(|v| Range::parse(&v).or_else(|| CellRef::parse(&v).map(Range::single)))) {
                        f.sort = Some((rg.start.col, !flag(&t, "descending", false)));
                    }
                }
                "conditionalFormatting" => ranges = t.attr("sqref").map(|v| sqref(&v)).unwrap_or_default(),
                // (A rule over no cells counts for nothing: it isn't kept.)
                "cfRule" if ranges.is_empty() || ranges.len() > cf_left => rule = None,
                "cfRule" => {
                    cf_left -= ranges.len();
                    seq += 1;
                    let style = t.attr("dxfId").and_then(|v| v.parse::<usize>().ok()).and_then(|i| dxfs.get(i)).cloned().unwrap_or_default();
                    let raw = RawRule {
                        ranges: ranges.clone(),
                        kind: t.attr("type").map(|v| v.into_owned()).unwrap_or_default(),
                        op: t.attr("operator").map(|v| v.into_owned()).unwrap_or_default(),
                        text: t.attr("text").map(|v| v.into_owned()).unwrap_or_default(),
                        period: t.attr("timePeriod").map(|v| v.into_owned()).unwrap_or_default(),
                        priority: t.attr("priority").and_then(|v| v.parse().ok()).unwrap_or(1000 + seq),
                        stop: flag(&t, "stopIfTrue", false),
                        style,
                        formulas: Vec::new(),
                        rank: t.attr("rank").and_then(|v| v.parse().ok()).unwrap_or(10),
                        percent: flag(&t, "percent", false),
                        bottom: flag(&t, "bottom", false),
                        above: flag(&t, "aboveAverage", true),
                        equal: flag(&t, "equalAverage", false),
                        cfvos: Vec::new(),
                        colors: Vec::new(),
                        icon_set: String::new(),
                        reverse: false,
                        show_value: true,
                    };
                    if empty {
                        rules.extend(raw.build());
                    } else {
                        rule = Some(raw);
                    }
                }
                "formula" if rule.is_some() => {
                    let text = r.text();
                    if let Some(x) = &mut rule {
                        x.formulas.push(text);
                    }
                }
                "cfvo" => {
                    if let Some(x) = &mut rule {
                        x.cfvos.push(cfvo(&t));
                    }
                }
                "color" => {
                    if let (Some(x), Some(c)) = (&mut rule, color_of(&t)) {
                        x.colors.push(c);
                    }
                }
                "iconSet" => {
                    if let Some(x) = &mut rule {
                        x.icon_set = t.attr("iconSet").map(|v| v.into_owned()).unwrap_or_else(|| "3TrafficLights1".into());
                        x.reverse = flag(&t, "reverse", false);
                        x.show_value = flag(&t, "showValue", true);
                    }
                }
                "dataValidation" => {
                    let mut v = Validation::new(t.attr("sqref").map(|v| sqref(&v)).unwrap_or_default());
                    v.kind = ValKind::from_xlsx(t.attr("type").as_deref().unwrap_or("none"));
                    v.op = CmpOp::from_xlsx(t.attr("operator").as_deref().unwrap_or("between"));
                    v.allow_blank = flag(&t, "allowBlank", false);
                    // showDropDown="1" *hides* the in-cell arrow.
                    v.dropdown = !flag(&t, "showDropDown", false);
                    v.show_input = flag(&t, "showInputMessage", false);
                    v.show_error = flag(&t, "showErrorMessage", false);
                    v.error_style = match t.attr("errorStyle").as_deref() {
                        Some("warning") => ErrorStyle::Warning,
                        Some("information") => ErrorStyle::Info,
                        _ => ErrorStyle::Stop,
                    };
                    v.input_title = t.attr("promptTitle").map(|v| v.into_owned()).unwrap_or_default();
                    v.input_msg = t.attr("prompt").map(|v| v.into_owned()).unwrap_or_default();
                    v.error_title = t.attr("errorTitle").map(|v| v.into_owned()).unwrap_or_default();
                    v.error_msg = t.attr("error").map(|v| v.into_owned()).unwrap_or_default();
                    if empty {
                        if !v.ranges.is_empty() && sh.validations.len() < MAX_VALIDATIONS {
                            sh.validations.push(v);
                        }
                    } else {
                        dv = Some(v);
                    }
                }
                "formula1" if dv.is_some() => {
                    let text = r.text();
                    if let Some(v) = &mut dv {
                        v.f1 = text;
                    }
                }
                "formula2" if dv.is_some() => {
                    let text = r.text();
                    if let Some(v) = &mut dv {
                        v.f2 = text;
                    }
                }
                // Excel 2010 extension rules keep their formulas in xm:f.
                "f" if dv.is_some() => {
                    let text = r.text();
                    if let Some(v) = &mut dv {
                        if v.f1.is_empty() { v.f1 = text } else { v.f2 = text }
                    }
                }
                "sqref" if dv.is_some() => {
                    let text = r.text();
                    if let Some(v) = &mut dv {
                        v.ranges = sqref(&text);
                    }
                }
                _ => {}
            },
            Ev::End(n) => match crate::xml::local(n) {
                "filterColumn" => {
                    if let (Some(f), Some(mut c)) = (&mut filter, col.take()) {
                        if !customs.is_empty() {
                            let mut it = customs.drain(..);
                            let a = it.next().unwrap();
                            c.kind = FilterKind::Custom { and: custom_and, a, b: it.next() };
                        }
                        f.cols.push(c);
                    }
                }
                "cfRule" => {
                    if let Some(x) = rule.take().and_then(RawRule::build) {
                        rules.push(x);
                    }
                }
                "dataValidation" => {
                    if let Some(v) = dv.take() {
                        // Each cell edited looks them over.
                        if !v.ranges.is_empty() && sh.validations.len() < MAX_VALIDATIONS {
                            sh.validations.push(v);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    rules.sort_by_key(|x| x.0);
    sh.cond_formats.extend(rules.into_iter().map(|x| x.1));
    if filter.is_some() {
        sh.filter = filter;
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Every differential format the workbook needs, deduplicated.
pub fn collect_dxfs(wb: &Workbook) -> Vec<CfStyle> {
    let mut out: Vec<CfStyle> = Vec::new();
    let mut add = |s: CfStyle| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    for sh in &wb.sheets {
        for r in &sh.cond_formats {
            if !r.kind.is_graphic() {
                add(r.style.clone());
            }
        }
        if let Some(f) = &sh.filter {
            for c in &f.cols {
                if let FilterKind::Color { font, color } = c.kind {
                    add(color_dxf(font, color));
                }
            }
        }
    }
    out
}

fn color_dxf(font: bool, color: Option<u32>) -> CfStyle {
    if font { CfStyle { color, ..Default::default() } } else { CfStyle { fill: color, ..Default::default() } }
}

pub fn write_dxfs(dxfs: &[CfStyle]) -> String {
    if dxfs.is_empty() {
        return String::new();
    }
    let mut s = format!(r#"<dxfs count="{}">"#, dxfs.len());
    for d in dxfs {
        s.push_str("<dxf>");
        let font = d.bold.is_some() || d.italic.is_some() || d.strike.is_some() || d.underline.is_some() || d.color.is_some();
        if font {
            s.push_str("<font>");
            for (v, tag) in [(d.bold, "b"), (d.italic, "i"), (d.strike, "strike")] {
                if let Some(v) = v {
                    let _ = write!(s, r#"<{tag}{}/>"#, if v { "" } else { r#" val="0""# });
                }
            }
            if let Some(u) = d.underline {
                s.push_str(if u { "<u/>" } else { r#"<u val="none"/>"# });
            }
            if let Some(c) = d.color {
                let _ = write!(s, r#"<color rgb="{}"/>"#, argb(c));
            }
            s.push_str("</font>");
        }
        if let Some(c) = d.fill {
            let _ = write!(s, r#"<fill><patternFill patternType="solid"><fgColor rgb="{0}"/><bgColor rgb="{0}"/></patternFill></fill>"#, argb(c));
        }
        s.push_str("</dxf>");
    }
    s.push_str("</dxfs>");
    s
}

fn dxf_id(dxfs: &[CfStyle], s: &CfStyle) -> usize {
    dxfs.iter().position(|d| d == s).unwrap_or(0)
}

/// `<autoFilter>`, written between `</sheetData>` and `<mergeCells>`.
pub fn write_autofilter(s: &mut String, sh: &Sheet, dxfs: &[CfStyle]) {
    let Some(f) = &sh.filter else { return };
    // A table's filter is written in the table part.
    if crate::xlsx_table::filter_in_table(sh) {
        return;
    }
    write_filter(s, f, dxfs);
}

/// An `<autoFilter>` element.
pub fn write_filter(s: &mut String, f: &AutoFilter, dxfs: &[CfStyle]) {
    let _ = write!(s, r#"<autoFilter ref="{}""#, f.range.to_a1());
    if f.cols.is_empty() && f.sort.is_none() {
        s.push_str("/>");
        return;
    }
    s.push('>');
    for c in &f.cols {
        let _ = write!(s, r#"<filterColumn colId="{}">"#, c.col.saturating_sub(f.range.start.col));
        match &c.kind {
            FilterKind::Values { items, blanks } => {
                s.push_str(if *blanks { r#"<filters blank="1">"# } else { "<filters>" });
                for i in items {
                    let _ = write!(s, r#"<filter val="{}"/>"#, esc(i));
                }
                s.push_str("</filters>");
            }
            FilterKind::Custom { and, a, b } => {
                s.push_str(if *and { r#"<customFilters and="1">"# } else { "<customFilters>" });
                for (op, val) in std::iter::once(a).chain(b.iter()) {
                    match crit_name(*op) {
                        Some(o) => {
                            let _ = write!(s, r#"<customFilter operator="{o}" val="{}"/>"#, esc(val));
                        }
                        None => {
                            let _ = write!(s, r#"<customFilter val="{}"/>"#, esc(val));
                        }
                    }
                }
                s.push_str("</customFilters>");
            }
            FilterKind::Top { top, percent, n } => {
                let _ = write!(s, r#"<top10{}{} val="{}"/>"#, if *top { "" } else { r#" top="0""# }, if *percent { r#" percent="1""# } else { "" }, n);
            }
            FilterKind::Average { above } => {
                let _ = write!(s, r#"<dynamicFilter type="{}"/>"#, if *above { "aboveAverage" } else { "belowAverage" });
            }
            FilterKind::Color { font, color } => {
                let _ = write!(s, r#"<colorFilter dxfId="{}"{}/>"#, dxf_id(dxfs, &color_dxf(*font, *color)), if *font { r#" cellColor="0""# } else { "" });
            }
        }
        s.push_str("</filterColumn>");
    }
    if let (Some((col, asc)), Some(data)) = (f.sort, f.data_rows()) {
        let key = Range::new(CellRef::new(data.start.row, col), CellRef::new(data.end.row, col));
        let _ = write!(s, r#"<sortState ref="{}"><sortCondition{} ref="{}"/></sortState>"#, data.to_a1(), if asc { "" } else { r#" descending="1""# }, key.to_a1());
    }
    s.push_str("</autoFilter>");
}

fn cfvo_xml(c: &Cfvo) -> String {
    match c {
        Cfvo::Min => r#"<cfvo type="min"/>"#.into(),
        Cfvo::Max => r#"<cfvo type="max"/>"#.into(),
        Cfvo::Num(n) => format!(r#"<cfvo type="num" val="{n}"/>"#),
        Cfvo::Percent(n) => format!(r#"<cfvo type="percent" val="{n}"/>"#),
        Cfvo::Percentile(n) => format!(r#"<cfvo type="percentile" val="{n}"/>"#),
        Cfvo::Formula(f) => format!(r#"<cfvo type="formula" val="{}"/>"#, esc(f)),
    }
}

fn period_formula(p: Period, a: &str) -> String {
    let d = format!("FLOOR({a},1)");
    let rd = format!("ROUNDDOWN({a},0)");
    match p {
        Period::Today => format!("{d}=TODAY()"),
        Period::Yesterday => format!("{d}=TODAY()-1"),
        Period::Tomorrow => format!("{d}=TODAY()+1"),
        Period::Last7Days => format!("AND(TODAY()-{d}<=6,{d}<=TODAY())"),
        Period::ThisWeek => format!("AND(TODAY()-{rd}<=WEEKDAY(TODAY())-1,{rd}-TODAY()<=7-WEEKDAY(TODAY()))"),
        Period::LastWeek => format!("AND(TODAY()-{rd}>=(WEEKDAY(TODAY())),TODAY()-{rd}<(WEEKDAY(TODAY())+7))"),
        Period::NextWeek => format!("AND({rd}-TODAY()>(7-WEEKDAY(TODAY())),{rd}-TODAY()<(15-WEEKDAY(TODAY())))"),
        Period::ThisMonth => format!("AND(MONTH({a})=MONTH(TODAY()),YEAR({a})=YEAR(TODAY()))"),
        Period::LastMonth => format!("AND(MONTH({a})=MONTH(EDATE(TODAY(),0-1)),YEAR({a})=YEAR(EDATE(TODAY(),0-1)))"),
        Period::NextMonth => format!("AND(MONTH({a})=MONTH(EDATE(TODAY(),0+1)),YEAR({a})=YEAR(EDATE(TODAY(),0+1)))"),
    }
}

fn quoted(t: &str) -> String {
    format!("\"{}\"", t.replace('"', "\"\""))
}

/// `<conditionalFormatting>` and `<dataValidations>`, after `<mergeCells>`.
pub fn write_cf_dv(s: &mut String, sh: &Sheet, dxfs: &[CfStyle]) {
    for (i, rule) in sh.cond_formats.iter().enumerate() {
        let a = rule.origin().to_a1();
        let _ = write!(s, r#"<conditionalFormatting sqref="{}">"#, sqref_text(&rule.ranges));
        let dxf = if rule.kind.is_graphic() { String::new() } else { format!(r#" dxfId="{}""#, dxf_id(dxfs, &rule.style)) };
        let stop = if rule.stop { r#" stopIfTrue="1""# } else { "" };
        let p = i + 1;
        let f = |x: &str| format!("<formula>{}</formula>", esc(x));
        match &rule.kind {
            CfKind::CellIs { op, f1, f2 } => {
                let _ = write!(s, r#"<cfRule type="cellIs"{dxf} priority="{p}"{stop} operator="{}">{}"#, op.xlsx(), f(f1));
                if op.two_args() {
                    s.push_str(&f(f2));
                }
                s.push_str("</cfRule>");
            }
            CfKind::Expression(e) => {
                let _ = write!(s, r#"<cfRule type="expression"{dxf} priority="{p}"{stop}>{}</cfRule>"#, f(e));
            }
            CfKind::Text { op, text } => {
                let q = quoted(text);
                let (ty, oper, formula) = match op {
                    TextOp::Contains => ("containsText", "containsText", format!("NOT(ISERROR(SEARCH({q},{a})))")),
                    TextOp::NotContains => ("notContainsText", "notContains", format!("ISERROR(SEARCH({q},{a}))")),
                    TextOp::BeginsWith => ("beginsWith", "beginsWith", format!("LEFT({a},LEN({q}))={q}")),
                    TextOp::EndsWith => ("endsWith", "endsWith", format!("RIGHT({a},LEN({q}))={q}")),
                };
                let _ = write!(s, r#"<cfRule type="{ty}"{dxf} priority="{p}"{stop} operator="{oper}" text="{}">{}</cfRule>"#, esc(text), f(&formula));
            }
            CfKind::Blanks(b) => {
                let (ty, formula) = if *b { ("containsBlanks", format!("LEN(TRIM({a}))=0")) } else { ("notContainsBlanks", format!("LEN(TRIM({a}))>0")) };
                let _ = write!(s, r#"<cfRule type="{ty}"{dxf} priority="{p}"{stop}>{}</cfRule>"#, f(&formula));
            }
            CfKind::Errors(b) => {
                let (ty, formula) = if *b { ("containsErrors", format!("ISERROR({a})")) } else { ("notContainsErrors", format!("NOT(ISERROR({a}))")) };
                let _ = write!(s, r#"<cfRule type="{ty}"{dxf} priority="{p}"{stop}>{}</cfRule>"#, f(&formula));
            }
            CfKind::Duplicates(d) => {
                let _ = write!(s, r#"<cfRule type="{}"{dxf} priority="{p}"{stop}/>"#, if *d { "duplicateValues" } else { "uniqueValues" });
            }
            CfKind::Top { bottom, percent, rank } => {
                let _ = write!(s, r#"<cfRule type="top10"{dxf} priority="{p}"{stop}{}{} rank="{rank}"/>"#, if *percent { r#" percent="1""# } else { "" }, if *bottom { r#" bottom="1""# } else { "" });
            }
            CfKind::Average { below, equal } => {
                let _ = write!(s, r#"<cfRule type="aboveAverage"{dxf} priority="{p}"{stop}{}{}/>"#, if *below { r#" aboveAverage="0""# } else { "" }, if *equal { r#" equalAverage="1""# } else { "" });
            }
            CfKind::Period(per) => {
                let _ = write!(s, r#"<cfRule type="timePeriod"{dxf} priority="{p}"{stop} timePeriod="{}">{}</cfRule>"#, per.xlsx(), f(&period_formula(*per, &a)));
            }
            CfKind::ColorScale(stops) => {
                let _ = write!(s, r#"<cfRule type="colorScale" priority="{p}"{stop}><colorScale>"#);
                for (c, _) in stops {
                    s.push_str(&cfvo_xml(c));
                }
                for (_, col) in stops {
                    let _ = write!(s, r#"<color rgb="{}"/>"#, argb(*col));
                }
                s.push_str("</colorScale></cfRule>");
            }
            CfKind::DataBar { color, min, max } => {
                let _ = write!(s, r#"<cfRule type="dataBar" priority="{p}"{stop}><dataBar>{}{}<color rgb="{}"/></dataBar></cfRule>"#, cfvo_xml(min), cfvo_xml(max), argb(*color));
            }
            CfKind::IconSet { set, reverse, show_value, points } => {
                let _ = write!(
                    s,
                    r#"<cfRule type="iconSet" priority="{p}"{stop}><iconSet iconSet="{}"{}{}><cfvo type="percent" val="0"/>"#,
                    esc(set),
                    if *show_value { "" } else { r#" showValue="0""# },
                    if *reverse { r#" reverse="1""# } else { "" }
                );
                for c in points {
                    s.push_str(&cfvo_xml(c));
                }
                s.push_str("</iconSet></cfRule>");
            }
        }
        s.push_str("</conditionalFormatting>");
    }
    if sh.validations.is_empty() {
        return;
    }
    let _ = write!(s, r#"<dataValidations count="{}">"#, sh.validations.len());
    for v in &sh.validations {
        s.push_str("<dataValidation");
        if v.kind != ValKind::Any {
            let _ = write!(s, r#" type="{}""#, v.kind.xlsx());
        }
        match v.error_style {
            ErrorStyle::Stop => {}
            ErrorStyle::Warning => s.push_str(r#" errorStyle="warning""#),
            ErrorStyle::Info => s.push_str(r#" errorStyle="information""#),
        }
        if v.kind.has_op() && v.op != CmpOp::Between {
            let _ = write!(s, r#" operator="{}""#, v.op.xlsx());
        }
        for (on, attr) in [(v.allow_blank, "allowBlank"), (v.kind == ValKind::List && !v.dropdown, "showDropDown"), (v.show_input, "showInputMessage"), (v.show_error, "showErrorMessage")] {
            if on {
                let _ = write!(s, r#" {attr}="1""#);
            }
        }
        for (text, attr) in [(&v.error_title, "errorTitle"), (&v.error_msg, "error"), (&v.input_title, "promptTitle"), (&v.input_msg, "prompt")] {
            if !text.is_empty() {
                let _ = write!(s, r#" {attr}="{}""#, esc(text));
            }
        }
        let _ = write!(s, r#" sqref="{}">"#, sqref_text(&v.ranges));
        if !v.f1.is_empty() {
            let _ = write!(s, "<formula1>{}</formula1>", esc(&v.f1));
        }
        if v.kind.has_op() && v.op.two_args() && !v.f2.is_empty() {
            let _ = write!(s, "<formula2>{}</formula2>", esc(&v.f2));
        }
        s.push_str("</dataValidation>");
    }
    s.push_str("</dataValidations>");
}

/// `_xlnm._FilterDatabase` names for `workbook.xml` (the inside of
/// `<definedNames>`).
pub fn filter_names(wb: &Workbook) -> String {
    let mut s = String::new();
    for (i, sh) in wb.sheets.iter().enumerate() {
        if let Some(f) = sh.filter.as_ref().filter(|_| !crate::xlsx_table::filter_in_table(sh)) {
            let r = f.range;
            let _ = write!(
                s,
                r#"<definedName name="_xlnm._FilterDatabase" localSheetId="{i}" hidden="1">'{}'!${}${}:${}${}</definedName>"#,
                esc(&sh.name.replace('\'', "''")),
                apex_sheet::addr::col_name(r.start.col),
                r.start.row + 1,
                apex_sheet::addr::col_name(r.end.col),
                r.end.row + 1
            );
        }
    }
    s
}

/// A workbook using every data tool, shared by the xlsx and ods tests.
#[cfg(test)]
pub(crate) mod sample {
    use super::*;
    use apex_sheet::Op;

    pub fn rg(s: &str) -> Range {
        Range::parse(s).unwrap()
    }

    pub fn workbook() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::EN;
        let rows = [["Name", "N", "Pick"], ["ana", "5", "Sí"], ["Bob", "30", "No"], ["cy", "7", ""]];
        let mut inputs = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, t) in row.iter().enumerate() {
                inputs.push((CellRef::new(r as u32, c as u32), t.to_string()));
            }
        }
        wb.set_inputs(0, inputs);
        let mut list = Validation::new(vec![rg("C2:C4")]);
        list.kind = ValKind::List;
        list.f1 = "\"Sí,No\"".into();
        list.input_title = "Pick".into();
        list.input_msg = "Choose one".into();
        list.error_style = ErrorStyle::Warning;
        list.error_msg = "Not in the list".into();
        let mut num = Validation::new(vec![rg("B2:B4")]);
        num.kind = ValKind::Whole;
        num.op = CmpOp::Between;
        num.f1 = "1".into();
        num.f2 = "20".into();
        num.show_input = false;
        let mut refs = Validation::new(vec![rg("D2")]);
        refs.kind = ValKind::List;
        refs.f1 = "$A$2:$A$4".into();
        wb.execute(Op::Validations { sheet: 0, list: vec![list, num, refs] });
        let red = CfStyle { color: Some(0x9C0006), fill: Some(0xFFC7CE), bold: Some(true), ..Default::default() };
        let rules = vec![
            CfRule { ranges: vec![rg("B2:B4")], kind: CfKind::CellIs { op: CmpOp::Greater, f1: "10".into(), f2: String::new() }, style: red.clone(), stop: true },
            CfRule { ranges: vec![rg("A2:A4")], kind: CfKind::Text { op: TextOp::BeginsWith, text: "b".into() }, style: CfStyle { italic: Some(true), ..Default::default() }, stop: false },
            CfRule { ranges: vec![rg("A2:A4"), rg("C2:C4")], kind: CfKind::Expression("$B2>6".into()), style: CfStyle { fill: Some(0xC6EFCE), ..Default::default() }, stop: false },
            CfRule { ranges: vec![rg("B2:B4")], kind: CfKind::DataBar { color: 0x638EC6, min: Cfvo::Min, max: Cfvo::Max }, style: CfStyle::default(), stop: false },
            CfRule { ranges: vec![rg("B2:B4")], kind: CfKind::ColorScale(vec![(Cfvo::Min, 0xF8696B), (Cfvo::Percentile(50.0), 0xFFEB84), (Cfvo::Max, 0x63BE7B)]), style: CfStyle::default(), stop: false },
            CfRule {
                ranges: vec![rg("B2:B4")],
                kind: CfKind::IconSet { set: "3Arrows".into(), reverse: true, show_value: false, points: vec![Cfvo::Percent(33.0), Cfvo::Percent(67.0)] },
                style: CfStyle::default(),
                stop: false,
            },
            CfRule { ranges: vec![rg("B2:B4")], kind: CfKind::Top { bottom: true, percent: false, rank: 2 }, style: red.clone(), stop: false },
            CfRule { ranges: vec![rg("B2:B4")], kind: CfKind::Average { below: false, equal: false }, style: red.clone(), stop: false },
            CfRule { ranges: vec![rg("A2:A4")], kind: CfKind::Duplicates(false), style: red.clone(), stop: false },
        ];
        wb.execute(Op::CondFormats { sheet: 0, list: rules });
        let mut f = AutoFilter::new(rg("A1:C4"));
        f.cols.push(ColumnFilter { col: 0, kind: FilterKind::Values { items: vec!["ana".into(), "cy".into()], blanks: true } });
        f.cols.push(ColumnFilter { col: 1, kind: FilterKind::Custom { and: true, a: (CritOp::Ge, "5".into()), b: Some((CritOp::Lt, "20".into())) } });
        wb.set_filter(0, Some(f), "undo.filter");
        // SUBTOTAL skips the filtered row; an outline with nested groups.
        wb.set_inputs(0, vec![(CellRef::new(9, 0), "=SUBTOTAL(109,B2:B4)".into()), (CellRef::new(12, 0), "detail".into())]);
        wb.group(0, true, 11, 13);
        wb.group(0, true, 12, 12);
        wb.group(0, false, 7, 8);
        wb.set_collapsed(0, true, &apex_sheet::Group { level: 2, start: 12, end: 12 }, true);
        wb.sheets[0].summary_below = false;
        wb
    }

    /// Checks what a format keeps of [`workbook`]. `full`: xlsx, which keeps
    /// every rule kind.
    pub fn check(orig: &Workbook, back: &Workbook, full: bool) {
        let (a, b) = (&orig.sheets[0], &back.sheets[0]);
        assert_eq!(b.validations, a.validations);
        assert_eq!(b.filter.as_ref().map(|f| f.range), a.filter.as_ref().map(|f| f.range));
        assert_eq!(b.filter.as_ref().unwrap().cols, a.filter.as_ref().unwrap().cols);
        assert_eq!(b.row_height(2), 0.0);
        assert_eq!(b.value(CellRef::new(9, 0)), &apex_sheet::Value::Number(12.0));
        assert_eq!(b.row_levels, a.row_levels);
        assert_eq!(b.col_levels, a.col_levels);
        assert_eq!(b.row_height(12), 0.0);
        if full {
            assert!(!b.summary_below && b.summary_right);
            assert_eq!(b.cond_formats, a.cond_formats);
        } else {
            assert_eq!(b.cond_formats.len(), a.cond_formats.len());
        }
    }
}
