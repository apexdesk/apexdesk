//! Charts and sparklines in .ods.
//!
//! A chart is an embedded object: a `draw:frame` holding a `draw:object`
//! whose `Object N/content.xml` has an `office:chart`. Its data are cell
//! range addresses of the spreadsheet (`Sheet1.B2:Sheet1.B6`), with a copy
//! of the values in a local table. Sparklines are LibreOffice's
//! `calcext:sparkline-groups` at the end of a `table:table`.
//!
//! ODF has no waterfall or histogram: they are written as column charts of
//! their source data, and the frame's description (`svg:desc`) keeps what
//! they really are so they come back.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::chart::{Axis, Bins, Blanks, Chart, ChartType, DataRef, ErrorAmount, ErrorBars, ErrorDir, Grouping, MAX_SERIES, LegendPos, Marker, Series, SparkAxis, SparkGroup, SparkKind, Sparkline, TrendKind, Trendline};
use apex_sheet::{Anchor, CellRef, MAX_COLS, MAX_ROWS, Sheet, Value, Workbook};

use crate::xml::{Ev, HEADER, Reader, Tag, esc};
use crate::zip::ZipReader;
use crate::{parse_hex_color, parse_length_pt};

/// Charts read from one sheet at most (hostile files).
const MAX_CHARTS: usize = 1000;
/// Rows written to a chart's local table at most.
const MAX_LOCAL_ROWS: usize = 20_000;
/// Mark of our own settings in a frame's description.
const DESC_MARK: &str = "apexdesk:chart";
const CHART_MIME: &str = "application/vnd.oasis.opendocument.chart";

/// The automatic color of series (or slice) `i`, as the painter picks it.
fn auto_color(c: &Chart, i: usize) -> u32 {
    let p = c.colors();
    p[i % p.len()]
}

// ---------------------------------------------------------------------------
// Range addresses

/// An ODF range address list ("Sheet1.$B$2:.$B$6 Sheet1.C2") as canonical
/// reference text (first range only).
pub(super) fn odf_to_ref(addr: &str) -> Option<String> {
    let first = split_list(addr).into_iter().next()?;
    // A sheetless end ("$B$6") would lose its column '$' as a sheet's.
    let (mut fixed, mut part, mut q) = (String::new(), String::new(), false);
    for c in first.chars().chain(std::iter::once(':')) {
        match c {
            '\'' => q = !q,
            ':' if !q => {
                if !fixed.is_empty() {
                    fixed.push(':');
                    if !part.contains('.') {
                        fixed.push('.');
                    }
                }
                fixed.push_str(&part);
                part.clear();
                continue;
            }
            _ => {}
        }
        part.push(c);
    }
    let r = super::convert_ref(&fixed);
    (!r.is_empty() && !r.contains("#REF!")).then_some(r)
}

/// Splits on spaces outside quotes.
fn split_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut q) = (String::new(), false);
    for c in s.chars() {
        match c {
            '\'' => {
                q = !q;
                cur.push(c);
            }
            ' ' if !q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn odf_sheet(name: &str) -> String {
    let plain = name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()) && !name.is_empty();
    if plain { name.to_string() } else { format!("'{}'", name.replace('\'', "''")) }
}

/// The ODF address of a reference (a cell or range; `own` is the sheet of
/// unqualified references). None for anything else.
pub(super) fn ref_to_odf(r: &DataRef, own: &str) -> Option<String> {
    let (sheet, range) = r.area()?;
    let s = odf_sheet(sheet.as_deref().unwrap_or(own));
    let one = |c: CellRef| format!("${}${}", apex_sheet::addr::col_name(c.col), c.row + 1);
    Some(if range.is_single() { format!("{s}.{}", one(range.start)) } else { format!("{s}.{}:{s}.{}", one(range.start), one(range.end)) })
}

fn data_ref(addr: Option<std::borrow::Cow<str>>) -> DataRef {
    addr.and_then(|a| odf_to_ref(&a)).map(|r| DataRef::parse(&r)).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Reading

/// Properties of one automatic style: every attribute of its chart,
/// graphic and text properties, and its number style.
#[derive(Default, Clone)]
struct Props(HashMap<String, String>);

impl Props {
    fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str)
    }

    fn is(&self, k: &str, v: &str) -> bool {
        self.get(k) == Some(v)
    }

    fn num(&self, k: &str) -> Option<f64> {
        self.get(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
    }

    fn color(&self, k: &str) -> Option<u32> {
        self.get(k).and_then(parse_hex_color)
    }

    /// A ODF 1.3 `chart:` attribute or its older `loext:` form.
    fn ext(&self, name: &str) -> Option<&str> {
        self.get(&format!("chart:{name}")).or_else(|| self.get(&format!("loext:{name}")))
    }
}

fn read_styles(xml: &str) -> (HashMap<String, Props>, HashMap<String, String>) {
    let mut styles: HashMap<String, Props> = HashMap::new();
    let mut numbers = HashMap::new();
    let mut r = Reader::new(xml);
    let mut cur: Option<(String, Props)> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "office:body" => break,
            Ev::Start(t) if t.name == "style:style" => {
                let mut p = Props::default();
                if let Some(d) = t.attr("style:data-style-name") {
                    p.0.insert("style:data-style-name".into(), d.into_owned());
                }
                cur = Some((t.attr("style:name").unwrap_or_default().into_owned(), p));
            }
            Ev::Start(t) | Ev::Empty(t) if matches!(t.name, "style:chart-properties" | "style:graphic-properties" | "style:text-properties") => {
                if let Some((_, p)) = &mut cur {
                    for (k, v) in t.attrs() {
                        p.0.insert(k.to_string(), v.into_owned());
                    }
                }
            }
            Ev::Start(t) if t.name.starts_with("number:") && t.name.ends_with("-style") => {
                let name = t.attr("style:name").unwrap_or_default().into_owned();
                let code = super::read_number_style(&mut r, crate::xml::local(t.name), &t);
                numbers.insert(name, code);
            }
            Ev::End("style:style") => {
                if let Some((n, p)) = cur.take() {
                    styles.insert(n, p);
                }
            }
            _ => {}
        }
    }
    (styles, numbers)
}

#[derive(Default)]
struct RawAxis {
    dim: String,
    name: String,
    style: String,
    title: Option<String>,
    categories: Option<String>,
    major_grid: bool,
    minor_grid: bool,
}

#[derive(Default)]
struct RawSeries {
    style: String,
    values: Option<String>,
    label_cell: Option<String>,
    label_string: Option<String>,
    class: Option<String>,
    secondary: bool,
    domain: Option<String>,
    /// A second domain (the x values of a bubble chart).
    domain2: Option<String>,
    /// Data points: (repeat count, style).
    points: Vec<(u32, String)>,
    regression: Option<(String, bool, bool)>,
    /// Style of the (vertical) error indicator.
    error_style: Option<String>,
}

#[derive(Default)]
struct RawChart {
    class: String,
    width: f32,
    height: f32,
    title: Option<String>,
    legend: Option<String>,
    plot_style: String,
    /// Styles of the chart area and of the wall (plot area).
    style: String,
    wall_style: String,
    axes: Vec<RawAxis>,
    series: Vec<RawSeries>,
    /// The local table: rows of cells.
    table: Vec<Vec<Value>>,
    /// Style of the data table (LibreOffice's).
    data_table: Option<String>,
}

fn parse_body(xml: &str) -> RawChart {
    let mut c = RawChart::default();
    let mut r = Reader::new(xml);
    let mut in_body = false;
    let mut axis: Option<RawAxis> = None;
    let mut series: Option<RawSeries> = None;
    let mut in_table = false;
    let mut row: Option<Vec<Value>> = None;
    let mut cell: Option<(Value, String)> = None;
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.name {
                "office:body" => in_body = true,
                _ if !in_body => {}
                "chart:chart" => {
                    c.class = t.attr("chart:class").unwrap_or_default().into_owned();
                    c.style = t.attr("chart:style-name").unwrap_or_default().into_owned();
                    c.width = t.attr("svg:width").and_then(|v| parse_length_pt(&v)).map_or(0.0, |p| p / 0.75);
                    c.height = t.attr("svg:height").and_then(|v| parse_length_pt(&v)).map_or(0.0, |p| p / 0.75);
                }
                "chart:title" if !empty => {
                    let text = r.text();
                    match &mut axis {
                        Some(a) => a.title = Some(text),
                        None if !in_table => c.title = Some(text),
                        None => {}
                    }
                }
                "chart:legend" => c.legend = Some(t.attr("chart:legend-position").unwrap_or_else(|| "end".into()).into_owned()),
                "chart:plot-area" => c.plot_style = t.attr("chart:style-name").unwrap_or_default().into_owned(),
                "chart:axis" => {
                    let a = RawAxis {
                        dim: t.attr("chart:dimension").unwrap_or_default().into_owned(),
                        name: t.attr("chart:name").unwrap_or_default().into_owned(),
                        style: t.attr("chart:style-name").unwrap_or_default().into_owned(),
                        ..Default::default()
                    };
                    if empty {
                        c.axes.push(a);
                    } else {
                        axis = Some(a);
                    }
                }
                "chart:categories" => {
                    if let Some(a) = &mut axis {
                        a.categories = t.attr("table:cell-range-address").map(|v| v.into_owned());
                    }
                }
                "chart:grid" => {
                    if let Some(a) = &mut axis {
                        if t.attr("chart:class").is_some_and(|v| v == "minor") {
                            a.minor_grid = true;
                        } else {
                            a.major_grid = true;
                        }
                    }
                }
                "chart:series" => {
                    let s = RawSeries {
                        style: t.attr("chart:style-name").unwrap_or_default().into_owned(),
                        values: t.attr("chart:values-cell-range-address").map(|v| v.into_owned()),
                        label_cell: t.attr("chart:label-cell-address").map(|v| v.into_owned()),
                        label_string: t.attr("loext:label-string").map(|v| v.into_owned()),
                        class: t.attr("chart:class").map(|v| v.into_owned()),
                        secondary: t.attr("chart:attached-axis").is_some_and(|v| v == "secondary-y"),
                        ..Default::default()
                    };
                    if empty {
                        c.series.push(s);
                    } else {
                        series = Some(s);
                    }
                }
                "chart:domain" => {
                    if let Some(s) = &mut series {
                        let a = t.attr("table:cell-range-address").map(|v| v.into_owned());
                        if s.domain.is_none() {
                            s.domain = a;
                        } else if s.domain2.is_none() {
                            s.domain2 = a;
                        }
                    }
                }
                "chart:wall" => c.wall_style = t.attr("chart:style-name").unwrap_or_default().into_owned(),
                "loext:data-table" => c.data_table = Some(t.attr("chart:style-name").unwrap_or_default().into_owned()),
                "chart:error-indicator" => {
                    if let Some(s) = &mut series {
                        if t.attr("chart:dimension").is_none_or(|d| d != "x") {
                            s.error_style = Some(t.attr("chart:style-name").unwrap_or_default().into_owned());
                        }
                    }
                }
                "chart:data-point" => {
                    if let Some(s) = &mut series {
                        let n = t.attr("chart:repeated").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 1_000_000);
                        s.points.push((n, t.attr("chart:style-name").unwrap_or_default().into_owned()));
                    }
                }
                "chart:regression-curve" => {
                    if let Some(s) = &mut series {
                        s.regression = Some((t.attr("chart:style-name").unwrap_or_default().into_owned(), false, false));
                    }
                }
                "chart:equation" => {
                    if let Some((_, eq, r2)) = series.as_mut().and_then(|s| s.regression.as_mut()) {
                        *eq = t.attr("chart:display-equation").is_some_and(|v| v == "true");
                        *r2 = t.attr("chart:display-r-square").is_some_and(|v| v == "true");
                    }
                }
                "table:table" => in_table = true,
                "table:table-row" if in_table => {
                    if empty {
                        c.table.push(Vec::new());
                    } else {
                        row = Some(Vec::new());
                    }
                }
                "table:table-cell" if in_table => {
                    let v = match t.attr("office:value-type").as_deref() {
                        Some("float" | "percentage" | "currency") => t.attr("office:value").and_then(|v| v.trim().parse::<f64>().ok()).map_or(Value::Empty, Value::Number),
                        _ => Value::Empty,
                    };
                    let string = t.attr("office:value-type").is_some_and(|v| v == "string");
                    if empty {
                        if let Some(rw) = &mut row {
                            rw.push(v);
                        }
                    } else {
                        cell = Some((v, if string { "\u{1}".into() } else { String::new() }));
                    }
                }
                "text:p" if cell.is_some() && !empty => {
                    let txt = r.text();
                    if let Some((_, s)) = &mut cell {
                        if s.starts_with('\u{1}') {
                            s.push_str(&txt);
                        }
                    }
                }
                // Our text is in text:p only.
                "draw:g" if cell.is_some() && !empty => r.skip(),
                _ => {}
            },
            Ev::End(n) => match n {
                "chart:axis" => {
                    if let Some(a) = axis.take() {
                        c.axes.push(a);
                    }
                }
                "chart:series" => {
                    if let Some(s) = series.take() {
                        c.series.push(s);
                    }
                }
                "table:table" => in_table = false,
                "table:table-row" => {
                    if let Some(rw) = row.take() {
                        if c.table.len() < apex_sheet::chart::MAX_POINTS {
                            c.table.push(rw);
                        }
                    }
                }
                "table:table-cell" => {
                    if let (Some((v, s)), Some(rw)) = (cell.take(), &mut row) {
                        let v = match s.strip_prefix('\u{1}') {
                            Some(txt) if matches!(v, Value::Empty) => Value::Text(Arc::from(txt)),
                            _ => v,
                        };
                        rw.push(v);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    c
}

/// Our settings kept in a frame description: "apexdesk:chart kind=waterfall
/// totals=2,5 ...".
#[derive(Default, Debug, PartialEq)]
struct Extra {
    kind: Option<ChartType>,
    style: u8,
    palette: u8,
    hole: Option<u8>,
    bins: Bins,
    totals: Vec<u32>,
    mean: Option<bool>,
    outliers: Option<bool>,
    bubble_scale: Option<u16>,
    /// Series whose error bars have no caps.
    nocap: Vec<usize>,
    /// Label ranges of the series (ODF addresses).
    label_ranges: Vec<(usize, String)>,
    label_cells: bool,
}

/// A value in the frame description, which splits at spaces and commas.
fn enc(v: &str) -> String {
    v.replace('%', "%25").replace(' ', "%20").replace(',', "%2C")
}

fn dec(v: &str) -> String {
    v.replace("%20", " ").replace("%2C", ",").replace("%25", "%")
}

/// Whether a frame description holds our settings.
pub(super) fn is_ours(desc: &str) -> bool {
    desc.trim().starts_with(DESC_MARK)
}

fn parse_extra(desc: &str) -> Option<Extra> {
    let rest = desc.trim().strip_prefix(DESC_MARK)?;
    let mut e = Extra::default();
    for kv in rest.split_whitespace() {
        let Some((k, v)) = kv.split_once('=') else { continue };
        match k {
            "kind" => e.kind = ChartType::ALL.into_iter().find(|t| t.key() == v),
            "style" => e.style = v.parse().unwrap_or(0),
            "palette" => e.palette = v.parse().unwrap_or(0),
            "hole" => e.hole = v.parse().ok(),
            "bins" => {
                e.bins = match v.split_once(':') {
                    Some(("w", x)) => x.parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0).map_or(Bins::Auto, Bins::Width),
                    Some(("n", x)) => x.parse::<u32>().ok().map_or(Bins::Auto, Bins::Count),
                    _ => Bins::Auto,
                }
            }
            "totals" => e.totals = v.split(',').filter_map(|x| x.parse().ok()).take(10_000).collect(),
            "mean" => e.mean = Some(v == "1"),
            "outliers" => e.outliers = Some(v == "1"),
            "bubblescale" => e.bubble_scale = v.parse().ok(),
            "nocap" => e.nocap = v.split(',').filter_map(|x| x.parse().ok()).take(MAX_SERIES).collect(),
            "cells" => e.label_cells = v == "1",
            "lbl" => {
                e.label_ranges = v.split(',').filter_map(|x| x.split_once(':')).filter_map(|(i, r)| Some((i.parse().ok()?, dec(r)))).take(MAX_SERIES).collect();
            }
            _ => {}
        }
    }
    Some(e)
}

fn write_extra(c: &Chart, sheet_name: &str) -> Option<String> {
    // Kinds ODF has no class for are written as columns.
    let special = c.kind.is_chartex() || c.kind.is_apex_only() || c.kind == ChartType::Surface;
    let scale = c.kind == ChartType::Bubble && c.bubble_scale != 100;
    let nocap: Vec<String> = c.series.iter().enumerate().filter(|(_, s)| s.error_bars.as_ref().is_some_and(|e| !e.cap)).map(|(i, _)| i.to_string()).collect();
    let own = sheet_name;
    let lbl: Vec<String> = c.series.iter().enumerate().filter_map(|(i, s)| Some(format!("{i}:{}", enc(&ref_to_odf(&s.label_range, own)?)))).collect();
    if !special && !scale && nocap.is_empty() && lbl.is_empty() && !c.labels.cells && c.style == 0 && c.palette == 0 && (c.kind != ChartType::Doughnut || c.hole == 50) {
        return None;
    }
    let mut s = String::from(DESC_MARK);
    if special {
        let _ = write!(s, " kind={}", c.kind.key());
    }
    if c.style != 0 {
        let _ = write!(s, " style={}", c.style);
    }
    if c.palette != 0 {
        let _ = write!(s, " palette={}", c.palette);
    }
    if c.kind == ChartType::Doughnut && c.hole != 50 {
        let _ = write!(s, " hole={}", c.hole);
    }
    match c.bins {
        Bins::Width(w) if c.kind == ChartType::Histogram => {
            let _ = write!(s, " bins=w:{w}");
        }
        Bins::Count(n) if c.kind == ChartType::Histogram => {
            let _ = write!(s, " bins=n:{n}");
        }
        _ => {}
    }
    if c.kind == ChartType::BoxWhisker {
        let _ = write!(s, " mean={} outliers={}", c.show_mean as u8, c.show_outliers as u8);
    }
    if scale {
        let _ = write!(s, " bubblescale={}", c.bubble_scale);
    }
    if !nocap.is_empty() {
        let _ = write!(s, " nocap={}", nocap.join(","));
    }
    if c.labels.cells {
        s.push_str(" cells=1");
    }
    if !lbl.is_empty() {
        let _ = write!(s, " lbl={}", lbl.join(","));
    }
    if c.kind == ChartType::Waterfall && !c.totals.is_empty() {
        let _ = write!(s, " totals={}", c.totals.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(","));
    }
    Some(s)
}

fn kind_of(class: &str) -> Option<ChartType> {
    Some(match class.trim_start_matches("chart:") {
        "bar" => ChartType::Column,
        "line" => ChartType::Line,
        "radar" | "filled-radar" => ChartType::Radar,
        "stock" => ChartType::Stock,
        "surface" => ChartType::Surface,
        "area" => ChartType::Area,
        "circle" => ChartType::Pie,
        "ring" => ChartType::Doughnut,
        "scatter" => ChartType::Scatter,
        "bubble" => ChartType::Bubble,
        _ => return None,
    })
}

/// Error bars from the properties of an error indicator (or, in older
/// files, of the series).
fn error_bars_of(p: &Props) -> Option<ErrorBars> {
    let (upper, lower) = (!p.is("chart:error-upper-indicator", "false"), !p.is("chart:error-lower-indicator", "false"));
    let dir = match (upper, lower) {
        (true, false) => ErrorDir::Plus,
        (false, true) => ErrorDir::Minus,
        (true, true) => ErrorDir::Both,
        (false, false) => return None,
    };
    let v = |k: &str, d: f64| p.num(k).filter(|v| v.is_finite()).map_or(d, f64::abs);
    let amount = match p.get("chart:error-category")? {
        "constant" => ErrorAmount::Fixed(v("chart:error-upper-limit", v("chart:error-lower-limit", 1.0))),
        "percentage" => ErrorAmount::Percent(v("chart:error-percentage", 5.0)),
        "standard-deviation" => ErrorAmount::StdDev(v("loext:std-weight", 1.0)),
        "standard-error" => ErrorAmount::StdErr,
        "cell-range" => ErrorAmount::Custom { plus: data_ref(p.get("chart:error-upper-range").map(Into::into)), minus: data_ref(p.get("chart:error-lower-range").map(Into::into)) },
        _ => return None,
    };
    Some(ErrorBars { amount, dir, cap: true })
}

fn marker_of(p: &Props) -> Option<Marker> {
    Some(match p.get("chart:symbol-type")? {
        "none" => Marker::None,
        "automatic" => Marker::Auto,
        "named-symbol" => match p.get("chart:symbol-name").unwrap_or("") {
            "square" => Marker::Square,
            "diamond" => Marker::Diamond,
            "arrow-up" | "arrow-down" | "arrow-left" | "arrow-right" => Marker::Triangle,
            _ => Marker::Circle,
        },
        _ => Marker::Auto,
    })
}

fn read_axis(a: &RawAxis, styles: &HashMap<String, Props>, numbers: &HashMap<String, String>, base: Axis) -> Axis {
    let p = styles.get(&a.style).cloned().unwrap_or_default();
    let mut x = base;
    x.title = a.title.clone().unwrap_or_default();
    x.min = p.num("chart:minimum");
    x.max = p.num("chart:maximum");
    x.major = p.num("chart:interval-major").filter(|v| *v > 0.0);
    x.minor = match (x.major, p.num("chart:interval-minor-divisor")) {
        (Some(m), Some(d)) if d >= 1.0 && d != 5.0 => Some(m / d),
        _ => None,
    };
    if p.is("chart:logarithmic", "true") {
        x.log_base = Some(10.0);
    }
    x.reverse = p.is("chart:reverse-direction", "true");
    x.visible = !p.is("chart:display-label", "false");
    x.major_grid = a.major_grid;
    x.minor_grid = a.minor_grid;
    if !p.is("chart:link-data-style-to-source", "true") {
        if let Some(code) = p.get("style:data-style-name").and_then(|n| numbers.get(n)) {
            if !code.eq_ignore_ascii_case("general") && code != "0" {
                x.format = code.clone();
            }
        }
    }
    x
}

/// The chart in `Object N/content.xml` of the object at `href` (None if
/// missing or not a chart). `desc` is the frame's description.
/// Chart area (with its text) or wall look; what we write by default is
/// left automatic.
fn read_box(p: &Props, text: bool) -> apex_sheet::chart::BoxFormat {
    let mut b = apex_sheet::chart::BoxFormat::default();
    if p.is("draw:fill", "none") {
        // The wall is unfilled by default.
        b.no_fill = text;
    } else if let Some(c) = p.color("draw:fill-color").filter(|c| !(text && *c == 0xFFFFFF)) {
        b.fill = Some(c);
    }
    if p.is("draw:stroke", "none") {
        b.no_border = text;
    } else if let Some(c) = p.color("svg:stroke-color").filter(|c| *c != 0xD9D9D9) {
        b.border = Some(c);
    }
    if text {
        b.text_color = p.color("fo:color");
        b.font_size = p.get("fo:font-size").and_then(parse_length_pt).filter(|v| *v > 0.0);
        b.bold = p.get("fo:font-weight").map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
        b.font = p.get("fo:font-family").or_else(|| p.get("style:font-name")).map(|f| f.trim_matches('\'').to_string());
    }
    b
}

pub(super) fn read_object(z: &ZipReader, href: &str, desc: &str) -> Option<Chart> {
    let dir = href.trim().trim_start_matches("./").trim_end_matches('/');
    if dir.is_empty() || dir.contains("://") || dir.starts_with('/') || dir.starts_with("..") {
        return None;
    }
    let xml = z.read_string(&format!("{dir}/content.xml")).ok()?;
    read_content(&xml, desc)
}

/// A chart from the package of a chart object (an odt's), reading its
/// local table.
pub(crate) fn read_package(data: &[u8]) -> Option<Chart> {
    let z = ZipReader::new(data).ok()?;
    if z.read_string("mimetype").ok()?.trim() != CHART_MIME {
        return None;
    }
    read_content(&z.read_string("content.xml").ok()?, "")
}

/// The content and styles of a chart object in a text document: its
/// data in its local table (ranges of "local-table"), not in a sheet.
pub(crate) fn object_in_document(wb: &Workbook, si: usize, c: &Chart) -> (String, String) {
    let own = crate::chart_object::standalone(wb, si, c, "local-table");
    let (content, _) = object_content(&own, 0, &own.sheets[0].charts[0]);
    let content = content.replace(r#" xlink:href=".." xlink:type="simple""#, "").replace("&apos;local-table&apos;.", "local-table.");
    (content, format!("{HEADER}{OBJECT_STYLES}"))
}

fn read_content(xml: &str, desc: &str) -> Option<Chart> {
    let raw = parse_body(xml);
    let extra = parse_extra(desc);
    let (styles, numbers) = read_styles(xml);
    let none = Props::default();
    let style = |n: &str| styles.get(n).unwrap_or(&none);
    let plot = style(&raw.plot_style);
    let mut kind = kind_of(&raw.class)?;
    if kind == ChartType::Column && plot.is("chart:vertical", "true") {
        kind = ChartType::Bar;
    }
    let mut c = Chart::new(kind, CellRef::new(0, 0), raw.width.max(8.0), raw.height.max(8.0));
    c.grouping = if plot.is("chart:percentage", "true") {
        Grouping::Percent
    } else if plot.is("chart:stacked", "true") {
        Grouping::Stacked
    } else {
        Grouping::Clustered
    };
    if !kind.can_stack() {
        c.grouping = Grouping::Clustered;
    }
    c.smooth = plot.get("chart:interpolation").is_some_and(|v| v != "none");
    if matches!(kind, ChartType::Line | ChartType::Radar) {
        c.markers = plot.get("chart:symbol-type").is_some_and(|v| v != "none");
    }
    c.filled = kind == ChartType::Radar && raw.class.ends_with("filled-radar");
    let view3d = kind.has_3d() && plot.is("chart:three-dimensional", "true");
    c.view3d = view3d;
    c.blanks = match plot.get("chart:treat-empty-cells") {
        Some("use-zero") => Blanks::Zero,
        Some("ignore") => Blanks::Span,
        _ => Blanks::Gap,
    };
    c.visible_only = !plot.is("chart:include-hidden-cells", "true");
    c.title = raw.title.clone().unwrap_or_default();
    c.show_title = raw.title.is_some();
    c.legend = match raw.legend.as_deref() {
        None => LegendPos::None,
        Some("start" | "top-start" | "bottom-start") => LegendPos::Left,
        Some("top") => LegendPos::Top,
        Some("bottom") => LegendPos::Bottom,
        Some(_) => LegendPos::Right,
    };
    // Bar spacing sits on an axis style (LibreOffice) or the plot area.
    let spacing = |k: &str| raw.axes.iter().map(|a| style(&a.style)).chain([plot]).find_map(|p| p.num(k));
    if let Some(g) = spacing("chart:gap-width") {
        c.gap = g.clamp(0.0, 500.0) as u16;
    }
    if let Some(o) = spacing("chart:overlap") {
        c.overlap = o.clamp(-100.0, 100.0) as i8;
    }
    let angle = plot.num("chart:angle-offset").or_else(|| raw.series.first().and_then(|s| style(&s.style).num("chart:angle-offset")));
    if let Some(a) = angle {
        // ODF counts counterclockwise from three o'clock.
        c.first_angle = (90.0 - a).rem_euclid(360.0) as u16;
    }
    let mut cats = DataRef::default();
    for a in &raw.axes {
        match (a.dim.as_str(), a.name.as_str()) {
            ("x", n) if n != "secondary-x" => {
                c.x_axis = read_axis(a, &styles, &numbers, Axis::default());
                if let Some(r) = &a.categories {
                    cats = data_ref(Some(r.into()));
                }
            }
            ("y", "secondary-y") => c.y2_axis = read_axis(a, &styles, &numbers, Axis::default()),
            ("y", _) => c.y_axis = read_axis(a, &styles, &numbers, Axis::default()),
            _ => {}
        }
    }
    let n_series = raw.series.len().min(apex_sheet::chart::MAX_SERIES);
    // The local table: header row with names, then a category and a value
    // per series in each row (when it matches the series).
    let table_ok = raw.table.len() > 1 && raw.table.iter().all(|r| r.len() == n_series + 1);
    let filled = c.filled;
    let line_like = |k: ChartType| matches!(k, ChartType::Line | ChartType::Scatter | ChartType::Stock) || k == ChartType::Radar && !filled;
    let fill_of = |rs: &RawSeries| {
        let p = style(&rs.style);
        let sk = rs.class.as_deref().and_then(kind_of).unwrap_or(kind);
        if line_like(sk) { p.color("svg:stroke-color").or_else(|| p.color("draw:fill-color")) } else { p.color("draw:fill-color") }
    };
    // Automatic colors: ours, or LibreOffice's when every color is one of them.
    c.palette = match extra.as_ref().map_or(0, |e| e.palette) {
        0 => {
            let lo = apex_sheet::chart::palette(5);
            let used: Vec<(usize, u32)> = if kind.is_round() {
                let mut k = 0usize;
                let mut v = Vec::new();
                for (n, st) in raw.series.first().map_or(&[][..], |s| &s.points[..]).iter().take(apex_sheet::chart::MAX_POINTS) {
                    if let Some(col) = style(st).color("draw:fill-color") {
                        v.push((k, col));
                    }
                    k = k.saturating_add(*n as usize);
                }
                v
            } else {
                raw.series.iter().take(n_series).enumerate().filter_map(|(i, rs)| Some((i, fill_of(rs)?))).collect()
            };
            if !used.is_empty() && used.iter().all(|&(i, col)| col == lo[i % 10]) { 5 } else { 0 }
        }
        p => p,
    };
    let pal = apex_sheet::chart::palette(c.palette);
    let auto = |i: usize| pal[i % pal.len()];
    for (i, rs) in raw.series.iter().take(n_series).enumerate() {
        let p = style(&rs.style);
        let mut s = Series { values: data_ref(rs.values.as_deref().map(Into::into)), ..Default::default() };
        s.explode = p.num("chart:pie-offset").unwrap_or(0.0).clamp(0.0, 255.0) as u8;
        if table_ok {
            s.values.cache = raw.table[1..].iter().map(|r| r[i + 1].clone()).collect();
        }
        if let Some(l) = &rs.label_cell {
            s.name = data_ref(Some(l.into()));
        }
        if s.name.is_empty() {
            if let Some(l) = &rs.label_string {
                let l = l.trim();
                s.name = DataRef::text(l.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(l));
            }
        }
        if table_ok && s.name.formula.is_some() {
            s.name.cache = vec![raw.table[0][i + 1].clone()];
        }
        s.categories = if kind.is_xy() { data_ref(rs.domain.as_deref().map(Into::into)) } else { cats.clone() };
        // Bubbles: sizes as the values, then the y and the x domains.
        if kind == ChartType::Bubble {
            s.sizes = std::mem::take(&mut s.values);
            s.values = std::mem::take(&mut s.categories);
            s.categories = data_ref(rs.domain2.as_deref().map(Into::into));
        }
        if table_ok && s.categories.formula.is_some() && !kind.is_xy() {
            s.categories.cache = raw.table[1..].iter().map(|r| r[0].clone()).collect();
        }
        let own = rs.class.as_deref().and_then(kind_of).map(|k| if k == ChartType::Column && kind == ChartType::Bar { ChartType::Bar } else { k });
        if let Some(k) = own.filter(|k| *k != kind && matches!(k, ChartType::Column | ChartType::Bar | ChartType::Line | ChartType::Area)) {
            s.kind = Some(k);
        }
        s.secondary = rs.secondary && kind.has_axes();
        let sk = s.kind.unwrap_or(kind);
        let stroke_none = p.is("draw:stroke", "none");
        let color = if line_like(sk) { p.color("svg:stroke-color").or_else(|| p.color("draw:fill-color")) } else { p.color("draw:fill-color") };
        s.color = color.filter(|&c| c != auto(i));
        if kind.is_round() {
            s.color = None;
        }
        // LibreOffice leaves out draw:stroke="solid", its default.
        if !line_like(sk) && !stroke_none {
            s.line_color = p.color("svg:stroke-color");
        }
        if let Some(w) = p.get("svg:stroke-width").and_then(parse_length_pt).filter(|w| w.is_finite() && *w > 0.0) {
            if line_like(sk) || s.line_color.is_some() {
                s.line_width = Some((w * 100.0).round() / 100.0);
            }
        }
        // A pie's series fill is under its slices' (xlsx imports say none).
        s.no_fill = p.is("draw:fill", "none") && !line_like(sk) && !kind.is_round();
        if sk == ChartType::Line {
            s.no_line = stroke_none;
        }
        if kind == ChartType::Scatter && i == 0 {
            c.lines = !stroke_none;
        }
        if let Some(m) = marker_of(&p) {
            s.marker = m;
        }
        if kind == ChartType::Scatter && i == 0 {
            c.markers = s.marker != Marker::None;
        }
        // No markers on the chart is written as none on each series.
        if s.marker == Marker::None && !c.markers {
            s.marker = Marker::Auto;
        }
        // Labels are one setting for the chart here: any series turns them on.
        match p.get("chart:data-label-number") {
            Some("value") => c.labels.value = true,
            Some("percentage") => c.labels.percent = true,
            Some("value-and-percentage") => {
                c.labels.value = true;
                c.labels.percent = true;
            }
            _ => {}
        }
        if p.is("chart:data-label-text", "true") {
            c.labels.category = true;
        }
        if p.ext("data-label-series") == Some("true") {
            c.labels.series = true;
        }
        if let Some((st, eq, r2)) = &rs.regression {
            let rp = style(st);
            let kind = match rp.get("chart:regression-type") {
                Some("linear") => Some(TrendKind::Linear),
                Some("exponential") => Some(TrendKind::Exponential),
                Some("logarithmic") => Some(TrendKind::Logarithmic),
                Some("power") => Some(TrendKind::Power),
                Some("polynomial") => Some(TrendKind::Polynomial(rp.ext("regression-max-degree").and_then(|v| v.trim().parse::<f64>().ok()).map_or(2, |v| v.clamp(2.0, 6.0) as u8))),
                Some("moving-average") => Some(TrendKind::MovingAverage(rp.ext("regression-period").and_then(|v| v.trim().parse::<f64>().ok()).map_or(2, |v| v.clamp(2.0, 255.0) as u8))),
                _ => None,
            };
            s.trendline = kind.map(|kind| Trendline { kind, show_equation: *eq, show_r2: *r2 });
        }
        s.error_bars = match &rs.error_style {
            Some(st) => error_bars_of(&style(st)),
            None => error_bars_of(&p),
        };
        if extra.as_ref().is_some_and(|e| e.nocap.contains(&i)) {
            if let Some(e) = &mut s.error_bars {
                e.cap = false;
            }
        }
        if let Some((_, r)) = extra.as_ref().and_then(|e| e.label_ranges.iter().find(|(k, _)| *k == i)) {
            s.label_range = data_ref(Some(r.as_str().into()));
        }
        // Colors of single points (pie slices) that are not the default.
        // Up to the points a series has, and a pie's worth of slices.
        let mut idx = 0u32;
        for (n, st) in &rs.points {
            if idx as usize >= apex_sheet::chart::MAX_POINTS || s.point_explode.len() + s.point_colors.len() >= 10_000 {
                break;
            }
            if let Some(e) = style(st).num("chart:pie-offset").map(|e| e.clamp(0.0, 255.0) as u8).filter(|e| *e != s.explode && kind.is_round()) {
                for k in idx..idx.saturating_add(*n).min(idx + 10_000) {
                    s.point_explode.push((k, e));
                }
            }
            if !st.is_empty() {
                if let Some(col) = style(st).color("draw:fill-color") {
                    for k in idx..idx.saturating_add(*n).min(idx + 10_000) {
                        let default = if kind.is_round() { auto(k as usize) } else { s.color.unwrap_or(auto(i)) };
                        if col != default {
                            s.point_colors.push((k, col));
                        }
                    }
                }
            }
            idx = idx.saturating_add(*n);
        }
        c.series.push(s);
    }
    if let Some(st) = &raw.data_table {
        c.data_table = true;
        c.table_keys = style(st).is("loext:show-keys", "true");
    }
    c.labels.cells = extra.as_ref().is_some_and(|e| e.label_cells);
    c.area = read_box(&style(&raw.style), true);
    c.plot = read_box(&style(&raw.wall_style), false);
    if let Some(e) = extra {
        if let Some(k) = e.kind {
            c.set_kind(k);
        }
        c.style = e.style;
        if let Some(h) = e.hole {
            c.hole = h;
        }
        c.bins = e.bins;
        c.totals = e.totals;
        if let Some(m) = e.mean {
            c.show_mean = m;
        }
        if let Some(o) = e.outliers {
            c.show_outliers = o;
        }
        if let Some(b) = e.bubble_scale {
            c.bubble_scale = b;
        }
    }
    if c.kind == ChartType::Surface {
        c.view3d = view3d;
    }
    c.sanitize();
    Some(c)
}

/// Places a chart read from a frame (lengths in LibreOffice pixels) and
/// adds it to its sheet.
#[allow(clippy::too_many_arguments)]
pub(super) fn place(sh: &mut Sheet, mut c: Chart, name: String, cell: Option<CellRef>, resize: bool, x: f32, y: f32, w: f32, h: f32) {
    if sh.charts.len() >= MAX_CHARTS {
        return;
    }
    let (sx, sy) = (super::images::SX, super::images::SY);
    c.name = name;
    if w > 0.0 && h > 0.0 {
        c.width = w;
        c.height = h;
    }
    c.anchor = match (cell, resize) {
        (None, _) => Anchor::Absolute,
        (Some(_), true) => Anchor::TwoCell,
        (Some(_), false) => Anchor::OneCell,
    };
    let (ox, oy) = cell.map_or((0.0, 0.0), |c| (sh.col_x(c.col) / sx, sh.row_y(c.row) / sy));
    c.set_pos(sh, (ox + x) * sx, (oy + y) * sy);
    if let Some(at) = cell {
        c.from = at;
        c.dx = x * sx;
        c.dy = y * sy;
    }
    c.sanitize();
    c.id = sh.charts.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    if c.name.is_empty() {
        c.name = format!("Chart {}", c.id);
    }
    sh.charts.push(c);
}

// ---------------------------------------------------------------------------
// Writing

/// Automatic styles of a chart object being written.
#[derive(Default)]
struct StyleSet {
    xml: String,
    n: usize,
}

impl StyleSet {
    fn add(&mut self, chart: &str, graphic: &str, text: &str, data_style: Option<&str>) -> String {
        self.n += 1;
        let name = format!("ch{}", self.n);
        let _ = write!(self.xml, r#"<style:style style:name="{name}" style:family="chart""#);
        if let Some(d) = data_style {
            let _ = write!(self.xml, r#" style:data-style-name="{d}""#);
        }
        self.xml.push('>');
        if !chart.is_empty() {
            let _ = write!(self.xml, "<style:chart-properties{chart}/>");
        }
        if !graphic.is_empty() {
            let _ = write!(self.xml, "<style:graphic-properties{graphic}/>");
        }
        if !text.is_empty() {
            let _ = write!(self.xml, "<style:text-properties{text}/>");
        }
        self.xml.push_str("</style:style>");
        name
    }
}

fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xFF_FFFF)
}

fn cm(px: f32) -> String {
    format!("{:.3}cm", px * 2.54 / 96.0)
}

fn pt(v: f32) -> String {
    format!("{:.3}cm", v * 2.54 / 72.0)
}

fn num(v: f64) -> String {
    let s = format!("{v}");
    if s.contains('e') { format!("{v:.12}") } else { s }
}

fn axis_xml(out: &mut String, st: &mut StyleSet, a: &Axis, dim: &str, name: &str, cats: Option<&str>, extra: &str) {
    let data_style = if a.format.is_empty() { None } else { super::write_number_style(&format!("N{}", st.n + 1), &a.format) };
    let mut props = String::new();
    let _ = write!(props, r#" chart:display-label="{}" chart:logarithmic="{}" chart:reverse-direction="{}""#, a.visible, a.log_base.is_some(), a.reverse);
    if let Some(v) = a.min {
        let _ = write!(props, r#" chart:minimum="{}""#, num(v));
    }
    if let Some(v) = a.max {
        let _ = write!(props, r#" chart:maximum="{}""#, num(v));
    }
    if let Some(m) = a.major {
        let _ = write!(props, r#" chart:interval-major="{}""#, num(m));
        if let Some(n) = a.minor.filter(|n| *n > 0.0) {
            let d = (m / n).round();
            if (1.0..=1000.0).contains(&d) {
                let _ = write!(props, r#" chart:interval-minor-divisor="{}""#, d as u32);
            }
        }
    }
    let ds_name = data_style.as_ref().map(|_| format!("N{}", st.n + 1));
    let _ = write!(props, r#" chart:link-data-style-to-source="{}""#, ds_name.is_none());
    props.push_str(extra);
    if let Some(d) = &data_style {
        st.xml.push_str(d);
    }
    let graphic = if a.visible { r##" svg:stroke-color="#b3b3b3""## } else { r#" draw:stroke="none""# };
    let s = st.add(&props, graphic, "", ds_name.as_deref());
    let _ = write!(out, r#"<chart:axis chart:dimension="{dim}" chart:name="{name}" chart:style-name="{s}">"#);
    if !a.title.is_empty() {
        let rot = if dim == "y" { r#" style:rotation-angle="90""# } else { "" };
        let ts = st.add(rot, "", "", None);
        let _ = write!(out, r#"<chart:title chart:style-name="{ts}"><text:p>{}</text:p></chart:title>"#, esc(&a.title));
    }
    if let Some(c) = cats {
        let _ = write!(out, r#"<chart:categories table:cell-range-address="{}"/>"#, esc(c));
    }
    for (on, class) in [(a.major_grid, "major"), (a.minor_grid, "minor")] {
        if on {
            let g = st.add("", r##" svg:stroke-color="#d9d9d9""##, "", None);
            let _ = write!(out, r#"<chart:grid chart:style-name="{g}" chart:class="{class}"/>"#);
        }
    }
    out.push_str("</chart:axis>");
}

/// The class of a chart or series: a filled radar has its own.
fn chart_class(c: &Chart, k: ChartType) -> &'static str {
    if k == ChartType::Radar && c.filled { "chart:filled-radar" } else { class_of(k) }
}

fn class_of(k: ChartType) -> &'static str {
    match k {
        ChartType::Line => "chart:line",
        ChartType::Radar => "chart:radar",
        ChartType::Bubble => "chart:bubble",
        ChartType::Stock => "chart:stock",
        ChartType::Area => "chart:area",
        ChartType::Pie => "chart:circle",
        ChartType::Doughnut => "chart:ring",
        ChartType::Scatter => "chart:scatter",
        _ => "chart:bar",
    }
}

fn symbol(m: Marker) -> &'static str {
    match m {
        Marker::Auto => r#" chart:symbol-type="automatic""#,
        Marker::None => r#" chart:symbol-type="none""#,
        Marker::Circle => r#" chart:symbol-type="named-symbol" chart:symbol-name="circle""#,
        Marker::Square => r#" chart:symbol-type="named-symbol" chart:symbol-name="square""#,
        Marker::Diamond => r#" chart:symbol-type="named-symbol" chart:symbol-name="diamond""#,
        Marker::Triangle => r#" chart:symbol-type="named-symbol" chart:symbol-name="arrow-up""#,
    }
}

/// `Object N/content.xml` of a chart; also returns the addresses it uses.
fn object_content(wb: &Workbook, si: usize, c: &Chart) -> (String, Vec<String>) {
    let own = wb.sheets[si].name.as_str();
    let data = wb.chart_data(si, c);
    let mut st = StyleSet::default();
    let mut used = Vec::new();
    let mut body = String::new();
    // The link to the container ("..") makes the ranges the sheet's, not the local table's.
    let _ = write!(body, r#"<chart:chart svg:width="{}" svg:height="{}" xlink:href=".." xlink:type="simple" chart:class="{}""#, cm(c.width), cm(c.height), chart_class(c, c.kind));
    let cs = {
        let a = &c.area;
        let fill = a.fancy.as_ref().map(|f| f.main_color()).or(a.fill);
        let mut g = match (fill, a.no_fill) {
            (Some(f), _) => format!(r#" draw:fill="solid" draw:fill-color="{}""#, hex(f)),
            (None, true) => r#" draw:fill="none""#.to_string(),
            _ => r##" draw:fill-color="#ffffff""##.to_string(),
        };
        match (a.border, a.no_border) {
            (Some(b), _) => {
                let _ = write!(g, r#" draw:stroke="solid" svg:stroke-color="{}""#, hex(b));
            }
            (None, true) => g.push_str(r#" draw:stroke="none""#),
            _ => g.push_str(r##" draw:stroke="solid" svg:stroke-color="#d9d9d9""##),
        }
        let mut t = String::new();
        if let Some(col) = a.text_color {
            let _ = write!(t, r#" fo:color="{}""#, hex(col));
        }
        if let Some(sz) = a.font_size {
            let _ = write!(t, r#" fo:font-size="{}pt""#, sz);
        }
        if let Some(b) = a.bold {
            t.push_str(if b { r#" fo:font-weight="bold""# } else { r#" fo:font-weight="normal""# });
        }
        if let Some(f) = &a.font {
            let _ = write!(t, r#" fo:font-family="{}""#, esc(f));
        }
        st.add("", &g, &t, None)
    };
    let _ = write!(body, r#" chart:style-name="{cs}">"#);
    if c.show_title && !c.title.is_empty() {
        let ts = st.add("", "", r#" fo:font-size="14pt""#, None);
        let _ = write!(body, r#"<chart:title chart:style-name="{ts}"><text:p>{}</text:p></chart:title>"#, esc(&c.title));
    }
    let legend = match c.legend {
        LegendPos::None => None,
        LegendPos::Right => Some("end"),
        LegendPos::Left => Some("start"),
        LegendPos::Top => Some("top"),
        LegendPos::Bottom => Some("bottom"),
    };
    if let Some(pos) = legend {
        let ls = st.add("", r#" draw:stroke="none" draw:fill="none""#, "", None);
        let _ = write!(body, r#"<chart:legend chart:legend-position="{pos}" style:legend-expansion="high" chart:style-name="{ls}"/>"#);
    }
    // LibreOffice's data table (before the plot area).
    if c.data_table && !c.kind.is_xy() && c.kind.has_axes() {
        let ts = st.add(&format!(r#" loext:show-horizontal-border="true" loext:show-vertical-border="true" loext:show-outline="true" loext:show-keys="{}""#, c.table_keys), r##" svg:stroke-color="#8b8b8b""##, "", None);
        let _ = write!(body, r#"<loext:data-table chart:style-name="{ts}"/>"#);
    }
    // Plot area settings.
    let mut pa = String::new();
    if c.kind == ChartType::Bar {
        pa.push_str(r#" chart:vertical="true""#);
    }
    match c.grouping {
        Grouping::Stacked if c.kind.can_stack() => pa.push_str(r#" chart:stacked="true""#),
        Grouping::Percent if c.kind.can_stack() => pa.push_str(r#" chart:percentage="true""#),
        _ => {}
    }
    if c.view3d && c.kind.has_3d() {
        pa.push_str(r#" chart:three-dimensional="true""#);
    }
    if matches!(c.kind, ChartType::Line | ChartType::Scatter | ChartType::Radar) {
        pa.push_str(if c.markers { r#" chart:symbol-type="automatic""# } else { r#" chart:symbol-type="none""# });
    }
    if c.smooth && matches!(c.kind, ChartType::Line | ChartType::Scatter) {
        pa.push_str(r#" chart:interpolation="cubic-spline""#);
    }
    if c.kind.is_round() {
        let _ = write!(pa, r#" chart:angle-offset="{}""#, (90 - c.first_angle as i32).rem_euclid(360));
    }
    let _ = write!(
        pa,
        r#" chart:treat-empty-cells="{}" chart:include-hidden-cells="{}""#,
        match c.blanks {
            Blanks::Gap => "leave-gap",
            Blanks::Zero => "use-zero",
            Blanks::Span => "ignore",
        },
        !c.visible_only
    );
    let pas = st.add(&pa, "", "", None);
    let _ = write!(body, r#"<chart:plot-area chart:style-name="{pas}">"#);
    let cat_addr = c.series.iter().find_map(|s| ref_to_odf(&s.categories, own));
    let bars = format!(r#" chart:gap-width="{}" chart:overlap="{}""#, c.gap, c.overlap);
    let cats = if c.kind.is_xy() { None } else { cat_addr.as_deref() };
    if let Some(a) = cats {
        used.push(a.to_string());
    }
    axis_xml(&mut body, &mut st, &c.x_axis, "x", "primary-x", cats, "");
    let y = if c.kind.is_round() { Axis { visible: false, major_grid: false, ..c.y_axis.clone() } } else { c.y_axis.clone() };
    axis_xml(&mut body, &mut st, &y, "y", "primary-y", None, &bars);
    if c.has_secondary() {
        axis_xml(&mut body, &mut st, &c.y2_axis, "y", "secondary-y", None, &bars);
    }
    for (i, s) in c.series.iter().enumerate() {
        let sk = c.series_kind(s);
        let line_like = matches!(sk, ChartType::Line | ChartType::Scatter | ChartType::Stock) || sk == ChartType::Radar && !c.filled;
        let color = s.color.unwrap_or(auto_color(c, i));
        let mut g = String::new();
        let mut p = String::new();
        if line_like {
            let _ = write!(g, r#" svg:stroke-color="{}" draw:fill-color="{}""#, hex(color), hex(color));
            let hide = if sk == ChartType::Scatter { !c.lines } else { s.no_line };
            g.push_str(if hide { r#" draw:stroke="none""# } else { r#" draw:stroke="solid""# });
            if let Some(w) = s.line_width {
                let _ = write!(g, r#" svg:stroke-width="{}""#, pt(w));
            }
            let m = match (s.marker, sk, c.markers) {
                (Marker::Auto, ChartType::Line | ChartType::Scatter | ChartType::Radar, false) | (Marker::Auto, ChartType::Stock, _) => Marker::None,
                (m, _, _) => m,
            };
            p.push_str(symbol(m));
        } else {
            if s.no_fill {
                g.push_str(r#" draw:fill="none""#);
            }
            let _ = write!(g, r#" draw:fill-color="{}""#, hex(color));
            match s.line_color {
                Some(l) => {
                    let _ = write!(g, r#" draw:stroke="solid" svg:stroke-color="{}""#, hex(l));
                    if let Some(w) = s.line_width {
                        let _ = write!(g, r#" svg:stroke-width="{}""#, pt(w));
                    }
                }
                None => g.push_str(r#" draw:stroke="none""#),
            }
        }
        let l = c.labels;
        let number = match (l.value, l.percent) {
            (true, true) => Some("value-and-percentage"),
            (true, false) => Some("value"),
            (false, true) => Some("percentage"),
            _ => None,
        };
        if let Some(n) = number {
            let _ = write!(p, r#" chart:data-label-number="{n}""#);
        }
        if l.category {
            p.push_str(r#" chart:data-label-text="true""#);
        }
        if l.series {
            p.push_str(r#" chart:data-label-series="true""#);
        }
        if c.kind.is_round() && s.explode > 0 {
            let _ = write!(p, r#" chart:pie-offset="{}""#, s.explode);
        }
        let ss = st.add(&p, &g, "", None);
        let _ = write!(body, r#"<chart:series chart:style-name="{ss}""#);
        // A bubble's values are its sizes (the y values are a domain).
        let values = if c.kind == ChartType::Bubble { &s.sizes } else { &s.values };
        if let Some(a) = ref_to_odf(values, own) {
            let _ = write!(body, r#" chart:values-cell-range-address="{}""#, esc(&a));
            used.push(a);
        }
        match ref_to_odf(&s.name, own) {
            Some(a) => {
                let _ = write!(body, r#" chart:label-cell-address="{}""#, esc(&a));
                used.push(a);
            }
            None => {
                if let Some(n) = data.series.get(i).map(|d| d.name.as_str()).filter(|_| !s.name.is_empty()) {
                    let _ = write!(body, r#" loext:label-string="{}""#, esc(&format!("\"{n}\"")));
                }
            }
        }
        // LibreOffice drops ring series: a doughnut's are circles.
        let _ = write!(body, r#" chart:class="{}""#, chart_class(c, match sk {
            ChartType::Bar => ChartType::Column,
            ChartType::Doughnut => ChartType::Pie,
            k => k,
        }));
        if s.secondary && c.has_secondary() {
            body.push_str(r#" chart:attached-axis="secondary-y""#);
        }
        body.push('>');
        if c.kind.is_xy() {
            let domains = if c.kind == ChartType::Bubble { vec![&s.values, &s.categories] } else { vec![&s.categories] };
            for d in domains {
                if let Some(a) = ref_to_odf(d, own) {
                    let _ = write!(body, r#"<chart:domain table:cell-range-address="{}"/>"#, esc(&a));
                    used.push(a);
                }
            }
        }
        if let Some(t) = &s.trendline {
            let (ty, extra) = match t.kind {
                TrendKind::Linear => ("linear", String::new()),
                TrendKind::Exponential => ("exponential", String::new()),
                TrendKind::Logarithmic => ("logarithmic", String::new()),
                TrendKind::Power => ("power", String::new()),
                TrendKind::Polynomial(n) => ("polynomial", format!(r#" chart:regression-max-degree="{n}""#)),
                TrendKind::MovingAverage(n) => ("moving-average", format!(r#" chart:regression-period="{n}""#)),
            };
            let rs = st.add(&format!(r#" chart:regression-type="{ty}"{extra}"#), &format!(r#" svg:stroke-color="{}""#, hex(color)), "", None);
            let _ = write!(body, r#"<chart:regression-curve chart:style-name="{rs}">"#);
            if t.show_equation || t.show_r2 {
                let _ = write!(body, r#"<chart:equation chart:display-equation="{}" chart:display-r-square="{}"/>"#, t.show_equation, t.show_r2);
            }
            body.push_str("</chart:regression-curve>");
        }
        if let (Some(e), false) = (&s.error_bars, c.kind.is_round()) {
            let mut ep = String::new();
            let (up, low) = (e.dir != ErrorDir::Minus, e.dir != ErrorDir::Plus);
            let _ = write!(ep, r#" chart:error-upper-indicator="{up}" chart:error-lower-indicator="{low}""#);
            match &e.amount {
                ErrorAmount::Fixed(v) => {
                    let _ = write!(ep, r#" chart:error-category="constant" chart:error-upper-limit="{}" chart:error-lower-limit="{}""#, num(*v), num(*v));
                }
                ErrorAmount::Percent(v) => {
                    let _ = write!(ep, r#" chart:error-category="percentage" chart:error-percentage="{}""#, num(*v));
                }
                ErrorAmount::StdDev(v) => {
                    let _ = write!(ep, r#" chart:error-category="standard-deviation" loext:std-weight="{}""#, num(*v));
                }
                ErrorAmount::StdErr => ep.push_str(r#" chart:error-category="standard-error""#),
                ErrorAmount::Custom { plus, minus } => {
                    ep.push_str(r#" chart:error-category="cell-range""#);
                    for (k, r) in [("upper", plus), ("lower", minus)] {
                        if let Some(a) = ref_to_odf(r, own) {
                            let _ = write!(ep, r#" chart:error-{k}-range="{}""#, esc(&a));
                            used.push(a);
                        }
                    }
                }
            }
            // Solid, or they take the series' border (often none).
            let es = st.add(&ep, r##" draw:stroke="solid" svg:stroke-width="0.026cm" svg:stroke-color="#595959""##, "", None);
            let _ = write!(body, r#"<chart:error-indicator chart:style-name="{es}" chart:dimension="y"/>"#);
        }
        // Slices take a color each; other points only when set.
        let points = data.series.get(i).map_or(0, |d| d.values.len()).min(10_000);
        if c.kind.is_round() || c.vary_colors && i == 0 && c.series.len() == 1 && !line_like && !c.kind.is_xy() {
            // (The first set for a point counts; points may be many.)
            let mut colors = std::collections::HashMap::new();
            for p in &s.point_colors {
                colors.entry(p.0).or_insert(p.1);
            }
            let mut explode = std::collections::HashMap::new();
            for p in &s.point_explode {
                explode.entry(p.0).or_insert(p.1);
            }
            for k in 0..points {
                let col = colors.get(&(k as u32)).map_or(auto_color(c, k), |p| *p);
                let off = explode.get(&(k as u32)).filter(|_| c.kind.is_round()).map_or(String::new(), |p| format!(r#" chart:pie-offset="{}""#, p));
                let ps = st.add(&off, &format!(r#" draw:fill="solid" draw:fill-color="{}""#, hex(col)), "", None);
                let _ = write!(body, r#"<chart:data-point chart:style-name="{ps}"/>"#);
            }
        } else if !s.point_colors.is_empty() {
            let mut at = 0u32;
            let mut pts: Vec<(u32, u32)> = s.point_colors.clone();
            pts.sort_by_key(|p| p.0);
            pts.dedup_by_key(|p| p.0);
            for (k, col) in pts {
                if k > at {
                    let _ = write!(body, r#"<chart:data-point chart:repeated="{}"/>"#, k - at);
                }
                let ps = st.add("", &format!(r#" draw:fill="solid" draw:fill-color="{}""#, hex(col)), "", None);
                let _ = write!(body, r#"<chart:data-point chart:style-name="{ps}"/>"#);
                at = k + 1;
            }
        }
        body.push_str("</chart:series>");
    }
    let wall = match c.plot.fancy.as_ref().map(|f| f.main_color()).or(c.plot.fill) {
        Some(f) => st.add("", &format!(r#" draw:fill="solid" draw:fill-color="{}"{}"#, hex(f), c.plot.border.map_or(String::new(), |b| format!(r#" draw:stroke="solid" svg:stroke-color="{}""#, hex(b)))), "", None),
        None => st.add("", r#" draw:fill="none""#, "", None),
    };
    let _ = write!(body, r#"<chart:wall chart:style-name="{wall}"/></chart:plot-area>"#);
    // Local table: categories and one column per series.
    let n = data.series.len();
    let _ = write!(
        body,
        r#"<table:table table:name="local-table"><table:table-header-columns><table:table-column/></table:table-header-columns><table:table-columns><table:table-column table:number-columns-repeated="{}"/></table:table-columns><table:table-header-rows><table:table-row><table:table-cell><text:p/></table:table-cell>"#,
        n.max(1)
    );
    for s in &data.series {
        let _ = write!(body, r#"<table:table-cell office:value-type="string"><text:p>{}</text:p></table:table-cell>"#, esc(&s.name));
    }
    body.push_str("</table:table-row></table:table-header-rows><table:table-rows>");
    for (k, cat) in data.categories.iter().enumerate().take(MAX_LOCAL_ROWS) {
        let _ = write!(body, r#"<table:table-row><table:table-cell office:value-type="string"><text:p>{}</text:p></table:table-cell>"#, esc(cat));
        for s in &data.series {
            match s.values.get(k).copied().flatten() {
                Some(v) => {
                    let _ = write!(body, r#"<table:table-cell office:value-type="float" office:value="{}"><text:p>{}</text:p></table:table-cell>"#, num(v), num(v));
                }
                None => body.push_str("<table:table-cell/>"),
            }
        }
        body.push_str("</table:table-row>");
    }
    body.push_str("</table:table-rows></table:table></chart:chart>");
    let mut x = String::from(HEADER);
    x.push_str(r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:chart="urn:oasis:names:tc:opendocument:xmlns:chart:1.0" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" office:version="1.3"><office:automatic-styles>"#);
    x.push_str(&st.xml);
    x.push_str("</office:automatic-styles><office:body><office:chart>");
    x.push_str(&body);
    x.push_str("</office:chart></office:body></office:document-content>");
    used.dedup();
    (x, used)
}

const OBJECT_STYLES: &str = r#"<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3"><office:styles/></office:document-styles>"#;

/// The frame of a chart and the parts of its object (added to `pics`).
/// Returns the anchor cell (None: anchored to the sheet) and the frame.
pub(super) fn frame(wb: &Workbook, si: usize, c: &Chart, z: usize, pics: &mut super::images::Pictures) -> (Option<CellRef>, String) {
    use super::images::{SX, SY, cell_address, len};
    let sh = &wb.sheets[si];
    pics.objects += 1;
    let dir = format!("Object {}", pics.objects);
    let (content, used) = object_content(wb, si, c);
    pics.files.push((format!("{dir}/"), Arc::from(&b""[..]), CHART_MIME));
    pics.files.push((format!("{dir}/content.xml"), Arc::from(content.into_bytes()), "text/xml"));
    pics.files.push((format!("{dir}/styles.xml"), Arc::from(format!("{HEADER}{OBJECT_STYLES}").into_bytes()), "text/xml"));
    let (px, py) = c.pos(sh);
    let (lx, ly) = (px / SX, py / SY);
    let cell = (c.anchor != Anchor::Absolute).then_some(c.from);
    let mut x = String::from("<draw:frame");
    if let (Some(_), Anchor::TwoCell) = (cell, c.anchor) {
        let (ex, ey) = ((lx + c.width) * SX, (ly + c.height) * SY);
        let end = CellRef::new(sh.row_at(ey.max(0.0)).min(MAX_ROWS - 1), sh.col_at(ex.max(0.0)).min(MAX_COLS - 1));
        let _ = write!(
            x,
            r#" table:end-cell-address="{}" table:end-x="{}" table:end-y="{}""#,
            esc(&cell_address(&sh.name, end)),
            len((ex - sh.col_x(end.col)).max(0.0) / SX),
            len((ey - sh.row_y(end.row)).max(0.0) / SY)
        );
    }
    let (ox, oy) = if cell.is_some() { (c.dx / SX, c.dy / SY) } else { (lx, ly) };
    let _ = write!(x, r#" draw:z-index="{z}" draw:name="{}" svg:width="{}" svg:height="{}" svg:x="{}" svg:y="{}">"#, esc(&c.name), len(c.width), len(c.height), len(ox), len(oy));
    // LibreOffice writes an empty loext:p here; ODF allows no text:p.
    let _ = write!(x, r#"<draw:object draw:notify-on-update-of-ranges="{}" xlink:href="./{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"><loext:p/></draw:object>"#, esc(&used.join(" ")), esc(&dir));
    if !c.alt.is_empty() {
        let _ = write!(x, "<svg:title>{}</svg:title>", esc(&c.alt));
    }
    if let Some(d) = write_extra(c, &sh.name) {
        let _ = write!(x, "<svg:desc>{}</svg:desc>", esc(&d));
    }
    x.push_str("</draw:frame>");
    (cell, x)
}

// ---------------------------------------------------------------------------
// Sparklines

fn cell_of(addr: &str) -> Option<CellRef> {
    let r = super::convert_ref(addr.trim());
    let a = r.rsplit_once('!').map_or(r.as_str(), |x| x.1).replace('$', "");
    CellRef::parse(&a)
}

/// Reads `calcext:sparkline-groups` whose start was just returned.
pub(super) fn read_sparklines(r: &mut Reader, sh: &mut Sheet) {
    let mut depth = 1;
    let mut group: Option<SparkGroup> = None;
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::End(n) => {
                if n == "calcext:sparkline-group" {
                    if let Some(mut g) = group.take() {
                        if !g.sparklines.is_empty() {
                            g.sanitize();
                            sh.sparklines.push(g);
                        }
                    }
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                if !empty {
                    depth += 1;
                }
                match t.name {
                    "calcext:sparkline-group" => {
                        let g = read_group(&t);
                        if empty {
                            continue;
                        }
                        group = Some(g);
                    }
                    "calcext:sparkline" => {
                        if let (Some(g), Some(cell)) = (&mut group, t.attr("calcext:cell-address").and_then(|a| cell_of(&a))) {
                            if g.sparklines.len() < 1_000_000 {
                                g.sparklines.push(Sparkline { cell, data: data_ref(t.attr("calcext:data-range")) });
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn read_group(t: &Tag) -> SparkGroup {
    let mut g = SparkGroup::default();
    let flag = |k: &str| t.attr(k).is_some_and(|v| v == "true");
    g.kind = match t.attr("calcext:type").as_deref() {
        Some("column") => SparkKind::Column,
        Some("stacked") => SparkKind::WinLoss,
        _ => SparkKind::Line,
    };
    if let Some(w) = t.attr("calcext:line-width").and_then(|v| parse_length_pt(&v)) {
        g.line_weight = w;
    }
    g.blanks = match t.attr("calcext:display-empty-cells-as").as_deref() {
        Some("zero") => Blanks::Zero,
        Some("span") => Blanks::Span,
        _ => Blanks::Gap,
    };
    g.markers = flag("calcext:markers");
    g.high = flag("calcext:high");
    g.low = flag("calcext:low");
    g.first = flag("calcext:first");
    g.last = flag("calcext:last");
    g.negative = flag("calcext:negative");
    g.show_axis = flag("calcext:display-x-axis");
    g.visible_only = !flag("calcext:display-hidden");
    g.right_to_left = flag("calcext:right-to-left");
    let axis = |ty: &str, val: &str| match t.attr(ty).as_deref() {
        Some("group") => SparkAxis::Group,
        Some("custom") => t.attr(val).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).map_or(SparkAxis::Individual, SparkAxis::Custom),
        _ => SparkAxis::Individual,
    };
    g.min = axis("calcext:min-axis-type", "calcext:manual-min");
    g.max = axis("calcext:max-axis-type", "calcext:manual-max");
    let color = |k: &str, d: u32| t.attr(k).and_then(|v| parse_hex_color(&v)).unwrap_or(d);
    g.color = color("calcext:color-series", g.color);
    g.negative_color = color("calcext:color-negative", g.negative_color);
    g.marker_color = color("calcext:color-markers", g.marker_color);
    g.high_color = color("calcext:color-high", g.high_color);
    g.low_color = color("calcext:color-low", g.low_color);
    g.first_color = color("calcext:color-first", g.first_color);
    g.last_color = color("calcext:color-last", g.last_color);
    g
}

/// The `calcext:sparkline-groups` of a sheet (empty without sparklines).
pub(super) fn write_sparklines(sh: &Sheet) -> String {
    if sh.sparklines.is_empty() {
        return String::new();
    }
    let mut x = String::from("<calcext:sparkline-groups>");
    for (i, g) in sh.sparklines.iter().enumerate() {
        let _ = write!(
            x,
            r#"<calcext:sparkline-group calcext:id="{{{:08x}-0000-4000-8000-000000000000}}" calcext:type="{}" calcext:line-width="{}pt" calcext:display-empty-cells-as="{}""#,
            i + 1,
            match g.kind {
                SparkKind::Line => "line",
                SparkKind::Column => "column",
                SparkKind::WinLoss => "stacked",
            },
            g.line_weight,
            match g.blanks {
                Blanks::Gap => "gap",
                Blanks::Zero => "zero",
                Blanks::Span => "span",
            }
        );
        for (on, k) in [
            (g.markers, "markers"),
            (g.high, "high"),
            (g.low, "low"),
            (g.first, "first"),
            (g.last, "last"),
            (g.negative, "negative"),
            (g.show_axis, "display-x-axis"),
            (!g.visible_only, "display-hidden"),
            (g.right_to_left, "right-to-left"),
        ] {
            if on {
                let _ = write!(x, r#" calcext:{k}="true""#);
            }
        }
        for (a, ty, val) in [(g.min, "min-axis-type", "manual-min"), (g.max, "max-axis-type", "manual-max")] {
            match a {
                SparkAxis::Individual => {
                    let _ = write!(x, r#" calcext:{ty}="individual""#);
                }
                SparkAxis::Group => {
                    let _ = write!(x, r#" calcext:{ty}="group""#);
                }
                SparkAxis::Custom(v) => {
                    let _ = write!(x, r#" calcext:{ty}="custom" calcext:{val}="{}""#, num(v));
                }
            }
        }
        for (k, c) in [
            ("series", g.color),
            ("negative", g.negative_color),
            ("markers", g.marker_color),
            ("first", g.first_color),
            ("last", g.last_color),
            ("high", g.high_color),
            ("low", g.low_color),
        ] {
            let _ = write!(x, r#" calcext:color-{k}="{}""#, hex(c));
        }
        x.push_str("><calcext:sparklines>");
        for s in &g.sparklines {
            let _ = write!(
                x,
                r#"<calcext:sparkline calcext:cell-address="{}" calcext:data-range="{}"/>"#,
                esc(&format!("{}.{}", odf_sheet(&sh.name), s.cell.to_a1())),
                esc(&ref_to_odf(&s.data, &sh.name).unwrap_or_default())
            );
        }
        x.push_str("</calcext:sparklines></calcext:sparkline-group>");
    }
    x.push_str("</calcext:sparkline-groups>");
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::chart::Labels;
    use apex_sheet::{Op, Range};

    fn book() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::EN;
        let rows = [["", "North", "South", "West"], ["Jan", "10", "4", "-3"], ["Feb", "20", "5", "7"], ["Mar", "15", "6", "2"], ["Apr", "8", "9", "11"]];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                if !v.is_empty() {
                    wb.set_input(0, CellRef::new(r as u32, c as u32), v);
                }
            }
        }
        wb.execute(Op::RenameSheet { index: 0, name: "My Data".into() });
        wb
    }

    fn charts(wb: &Workbook) -> Vec<Chart> {
        let r = Range::parse("A1:D5").unwrap();
        let mk = |k: ChartType| wb.chart_from_range(0, r, k, None);
        let mut v = Vec::new();
        for g in [Grouping::Clustered, Grouping::Stacked, Grouping::Percent] {
            let mut c = mk(ChartType::Column);
            c.grouping = g;
            v.push(c);
        }
        let mut c = mk(ChartType::Bar);
        c.title = "Sales".into();
        c.legend = LegendPos::Bottom;
        c.gap = 80;
        c.overlap = 20;
        c.series[0].color = Some(0x112233);
        c.series[1].line_color = Some(0x445566);
        c.series[1].line_width = Some(1.5);
        v.push(c);
        let mut c = mk(ChartType::Line);
        c.markers = true;
        c.series[0].marker = Marker::Diamond;
        c.series[1].trendline = Some(Trendline { kind: TrendKind::Polynomial(3), show_equation: true, show_r2: false });
        c.series[2].trendline = Some(Trendline { kind: TrendKind::MovingAverage(2), show_equation: false, show_r2: false });
        c.x_axis.title = "Month".into();
        c.y_axis = Axis { title: "Units".into(), min: Some(-5.0), max: Some(30.0), major: Some(5.0), minor: Some(2.5), format: "0.0".into(), reverse: true, major_grid: true, minor_grid: true, ..Axis::default() };
        c.legend = LegendPos::Left;
        c.labels = Labels { value: true, ..Default::default() };
        v.push(c);
        let mut c = mk(ChartType::Area);
        c.grouping = Grouping::Stacked;
        c.legend = LegendPos::Top;
        c.blanks = Blanks::Zero;
        v.push(c);
        let mut c = wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Pie, None);
        c.first_angle = 45;
        c.series[0].point_colors = vec![(1, 0xFF0000)];
        c.series[0].explode = 5;
        c.series[0].point_explode = vec![(2, 25)];
        c.labels = Labels { percent: true, category: true, ..Default::default() };
        c.area = apex_sheet::chart::BoxFormat { fill: Some(0xFFF2CC), border: Some(0x7F6000), text_color: Some(0x7F6000), font_size: Some(12.0), bold: Some(true), font: Some("Georgia".into()), ..Default::default() };
        c.plot = apex_sheet::chart::BoxFormat { fill: Some(0xDDEBF7), ..Default::default() };
        v.push(c);
        let mut c = wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Doughnut, None);
        c.hole = 70;
        c.legend = LegendPos::None;
        v.push(c);
        let mut c = wb.chart_from_range(0, Range::parse("B1:C5").unwrap(), ChartType::Scatter, None);
        c.lines = true;
        c.smooth = true;
        c.y_axis.log_base = Some(10.0);
        c.series[0].trendline = Some(Trendline { kind: TrendKind::Linear, show_equation: true, show_r2: true });
        v.push(c);
        let mut c = mk(ChartType::Column);
        c.make_combo();
        c.y2_axis.title = "Right".into();
        v.push(c);
        let mut c = wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Waterfall, None);
        c.totals = vec![3];
        v.push(c);
        let mut c = wb.chart_from_range(0, Range::parse("B2:D5").unwrap(), ChartType::Histogram, Some(true));
        c.bins = Bins::Width(2.5);
        c.style = 3;
        c.palette = 2;
        v.push(c);
        v
    }

    pub(crate) fn sample() -> Workbook {
        let mut wb = book();
        let list = charts(&wb);
        for (i, mut c) in list.into_iter().enumerate() {
            c.from = CellRef::new(8 + (i as u32 % 4) * 16, 1 + (i as u32 / 4) * 8);
            c.dx = 5.0;
            c.dy = 3.0;
            c.anchor = [Anchor::TwoCell, Anchor::OneCell, Anchor::Absolute][i % 3];
            if i % 2 == 0 {
                c.alt = format!("Chart {i} of <sales> & costs");
            }
            wb.add_chart(0, c);
        }
        let g = SparkGroup {
            kind: SparkKind::Column,
            high: true,
            negative: true,
            min: SparkAxis::Custom(-2.0),
            max: SparkAxis::Group,
            color: 0x00B050,
            sparklines: (1..5).map(|r| Sparkline { cell: CellRef::new(r, 5), data: DataRef::parse(&format!("'My Data'!$B${}:$D${}", r + 1, r + 1)) }).collect(),
            ..Default::default()
        };
        wb.add_sparklines(0, g);
        wb.add_sparklines(0, SparkGroup { kind: SparkKind::WinLoss, markers: true, show_axis: true, line_weight: 1.25, sparklines: vec![Sparkline { cell: CellRef::new(6, 5), data: DataRef::parse("'My Data'!$B$2:$B$5") }], ..Default::default() });
        wb
    }

    fn same(a: &Chart, b: &Chart, sa: &Sheet, sb: &Sheet) {
        let ctx = format!("{:?}", a.kind);
        assert_eq!((a.kind, a.grouping, a.anchor), (b.kind, b.grouping, b.anchor), "{ctx}");
        assert_eq!((&a.title, a.legend, a.labels, a.blanks), (&b.title, b.legend, b.labels, b.blanks), "{ctx}");
        assert_eq!((a.style, a.palette, &a.totals, a.bins), (b.style, b.palette, &b.totals, b.bins), "{ctx}");
        if a.kind.is_round() {
            assert_eq!((a.first_angle, a.hole), (b.first_angle, b.hole), "{ctx}");
        }
        if matches!(a.kind, ChartType::Column | ChartType::Bar) {
            assert_eq!((a.gap, a.overlap), (b.gap, b.overlap), "{ctx}");
        }
        if matches!(a.kind, ChartType::Line | ChartType::Scatter) {
            assert_eq!((a.markers, a.smooth), (b.markers, b.smooth), "{ctx}");
        }
        if a.kind == ChartType::Scatter {
            assert_eq!(a.lines, b.lines);
        }
        if !a.kind.is_round() {
            assert_eq!(a.x_axis, b.x_axis, "{ctx}");
            assert_eq!(a.y_axis, b.y_axis, "{ctx}");
        }
        if a.has_secondary() {
            assert_eq!(a.y2_axis.title, b.y2_axis.title);
        }
        assert_eq!(a.series.len(), b.series.len(), "{ctx}");
        for (x, y) in a.series.iter().zip(&b.series) {
            assert_eq!(x.values.to_canonical(), y.values.to_canonical(), "{ctx}");
            assert_eq!(x.name.to_canonical(), y.name.to_canonical(), "{ctx}");
            assert_eq!(x.categories.to_canonical(), y.categories.to_canonical(), "{ctx}");
            assert_eq!((x.color, x.line_color, x.line_width, x.marker), (y.color, y.line_color, y.line_width, y.marker), "{ctx}");
            assert_eq!((x.kind, x.secondary, x.trendline, &x.point_colors), (y.kind, y.secondary, y.trendline, &y.point_colors), "{ctx}");
            assert_eq!((x.explode, &x.point_explode), (y.explode, &y.point_explode), "{ctx}");
        }
        assert_eq!((&a.area, &a.plot), (&b.area, &b.plot), "{ctx}");
        let (pa, pb) = (a.pos(sa), b.pos(sb));
        assert!((pa.0 - pb.0).abs() < 1.0 && (pa.1 - pb.1).abs() < 1.0, "{ctx} {pa:?} {pb:?}");
        assert!((a.width - b.width).abs() < 1.0 && (a.height - b.height).abs() < 1.0, "{ctx}");
        assert_eq!(a.name, b.name);
        assert_eq!(a.alt, b.alt, "{ctx}");
    }

    #[test]
    fn more_kinds_roundtrip() {
        let mut wb = book();
        let r = Range::parse("A1:D5").unwrap();
        let mut list = Vec::new();
        for k in [ChartType::Radar, ChartType::Stock, ChartType::Surface, ChartType::Treemap, ChartType::Sunburst, ChartType::BoxWhisker, ChartType::Funnel, ChartType::Pareto, ChartType::Mekko, ChartType::Timeline] {
            list.push(wb.chart_from_range(0, r, k, None));
        }
        let mut c = wb.chart_from_range(0, r, ChartType::Radar, None);
        c.filled = true;
        list.push(c);
        let mut c = wb.chart_from_range(0, r, ChartType::Column, None);
        c.view3d = true;
        list.push(c);
        let mut c = wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Pie, None);
        c.view3d = true;
        list.push(c);
        let mut c = wb.chart_from_range(0, r, ChartType::Surface, None);
        c.view3d = false;
        list.push(c);
        let mut c = wb.chart_from_range(0, r, ChartType::BoxWhisker, None);
        c.show_mean = false;
        c.show_outliers = false;
        list.push(c);
        let mut c = wb.chart_from_range(0, Range::parse("B1:D5").unwrap(), ChartType::Bubble, None);
        c.bubble_scale = 60;
        list.push(c);
        for (i, mut c) in list.into_iter().enumerate() {
            c.from = CellRef::new(8 + i as u32 * 16, 1);
            wb.add_chart(0, c);
        }
        let bytes = crate::ods::write(&wb);
        let back = crate::ods::read(&bytes).unwrap();
        let (a, b) = (&wb.sheets[0].charts, &back.sheets[0].charts);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            let ctx = format!("{:?}", x.kind);
            assert_eq!((x.kind, x.view3d, x.filled, x.show_mean, x.show_outliers, x.bubble_scale), (y.kind, y.view3d, y.filled, y.show_mean, y.show_outliers, y.bubble_scale), "{ctx}");
            assert_eq!(x.series.len(), y.series.len(), "{ctx}");
            for (p, q) in x.series.iter().zip(&y.series) {
                assert_eq!(p.values.to_canonical(), q.values.to_canonical(), "{ctx}");
                assert_eq!(p.sizes.to_canonical(), q.sizes.to_canonical(), "{ctx}");
                assert_eq!(p.categories.to_canonical(), q.categories.to_canonical(), "{ctx}");
            }
        }
        let bubble = b.last().unwrap();
        assert!(!bubble.series[0].sizes.is_empty());
    }

    #[test]
    fn elements_roundtrip() {
        use apex_sheet::chart::{ErrorAmount, ErrorBars, ErrorDir};
        let mut wb = book();
        let r = Range::parse("A1:D5").unwrap();
        let mut c = wb.chart_from_range(0, r, ChartType::Column, None);
        c.data_table = true;
        c.table_keys = false;
        c.labels.cells = true;
        c.series[0].label_range = c.series[0].categories.clone();
        c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::Percent(10.0), ..Default::default() });
        c.series[1].error_bars = Some(ErrorBars { amount: ErrorAmount::Custom { plus: c.series[2].values.clone(), minus: c.series[0].values.clone() }, dir: ErrorDir::Plus, cap: false });
        c.series[2].error_bars = Some(ErrorBars { amount: ErrorAmount::StdDev(2.0), dir: ErrorDir::Minus, cap: true });
        wb.add_chart(0, c);
        let mut c = wb.chart_from_range(0, Range::parse("B1:C5").unwrap(), ChartType::Scatter, None);
        c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::Fixed(1.5), ..Default::default() });
        c.from = CellRef::new(30, 1);
        wb.add_chart(0, c);
        let bytes = crate::ods::write(&wb);
        // APEX_DUMP=dir saves the file, to check it in other programs.
        if let Some(d) = std::env::var_os("APEX_DUMP") {
            let _ = std::fs::write(std::path::Path::new(&d).join("elements.ods"), &bytes);
        }
        let back = crate::ods::read(&bytes).unwrap();
        for (x, y) in wb.sheets[0].charts.iter().zip(&back.sheets[0].charts) {
            assert_eq!((x.data_table, x.table_keys, x.labels), (y.data_table, y.table_keys, y.labels));
            for (p, q) in x.series.iter().zip(&y.series) {
                let canon = |e: &Option<ErrorBars>| {
                    e.as_ref().map(|e| {
                        let amount = match &e.amount {
                            ErrorAmount::Custom { plus, minus } => format!("{:?} {:?}", plus.to_canonical(), minus.to_canonical()),
                            a => format!("{a:?}"),
                        };
                        (e.dir, e.cap, amount)
                    })
                };
                assert_eq!(canon(&p.error_bars), canon(&q.error_bars));
                assert_eq!(p.label_range.to_canonical(), q.label_range.to_canonical());
            }
        }
    }

    #[test]
    fn charts_roundtrip() {
        let wb = sample();
        let bytes = crate::ods::write(&wb);
        let back = crate::ods::read(&bytes).unwrap();
        let (a, b) = (&wb.sheets[0], &back.sheets[0]);
        assert_eq!(a.charts.len(), b.charts.len());
        for (x, y) in a.charts.iter().zip(&b.charts) {
            same(x, y, a, b);
        }
        // Data come through the references.
        let d = back.chart_data(0, &b.charts[0]);
        assert_eq!(d.categories, ["Jan", "Feb", "Mar", "Apr"]);
        assert_eq!(d.series[2].values[0], Some(-3.0));
        assert_eq!(d.series[0].name, "North");
        assert_eq!(a.sparklines, b.sparklines);
        let z = ZipReader::new(&bytes).unwrap();
        let manifest = z.read_string("META-INF/manifest.xml").unwrap();
        assert!(manifest.contains(r#"manifest:full-path="Object 1/" manifest:media-type="application/vnd.oasis.opendocument.chart""#));
        assert_eq!(a.charts.len(), 12);
        assert!(z.read_string("Object 12/content.xml").is_ok());
        assert!(z.read_string("Object 13/content.xml").is_err());
        // Saved again: the same.
        let again = crate::ods::read(&crate::ods::write(&back)).unwrap();
        for (x, y) in b.charts.iter().zip(&again.sheets[0].charts) {
            same(x, y, b, &again.sheets[0]);
        }
    }

    #[test]
    fn range_addresses() {
        assert_eq!(odf_to_ref("Sheet1.$B$2:.$B$6").unwrap(), "Sheet1!$B$2:$B$6");
        assert_eq!(odf_to_ref("'My Sheet'.B2:'My Sheet'.B6 Sheet1.C1").unwrap(), "'My Sheet'!B2:B6");
        assert_eq!(odf_to_ref("$Sheet1.$B$2:$B$6").unwrap(), "Sheet1!$B$2:$B$6");
        assert_eq!(odf_to_ref("'O''Neil'.A1").unwrap(), "'O''Neil'!A1");
        assert!(odf_to_ref("").is_none());
        assert!(odf_to_ref("Sheet1.#REF!").is_none());
        let r = DataRef::parse("'My Sheet'!$B$2:$B$6");
        assert_eq!(ref_to_odf(&r, "x").unwrap(), "'My Sheet'.$B$2:'My Sheet'.$B$6");
        assert_eq!(ref_to_odf(&DataRef::parse("C3"), "Own").unwrap(), "Own.$C$3");
        assert_eq!(ref_to_odf(&DataRef::parse("'O''Neil'!A1:A2"), "x").unwrap(), "'O''Neil'.$A$1:'O''Neil'.$A$2");
        assert!(ref_to_odf(&DataRef::text("x"), "x").is_none());
        assert_eq!(parse_extra("apexdesk:chart kind=histogram bins=n:7 style=2"), Some(Extra { kind: Some(ChartType::Histogram), style: 2, bins: Bins::Count(7), ..Default::default() }));
        assert_eq!(parse_extra("a picture"), None);
        let e = parse_extra("apexdesk:chart bins=w:-1 totals=a,2 hole=x").unwrap();
        assert_eq!((e.bins, e.totals, e.hole), (Bins::Auto, vec![2], None));
    }

    /// A LibreOffice-written line chart (trimmed).
    const LO_OBJECT: &str = r##"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" office:version="1.4"><office:automatic-styles><number:number-style style:name="N0"><number:number number:min-integer-digits="1"/></number:number-style><style:style style:name="ch3" style:family="chart"><style:chart-properties chart:symbol-type="automatic" chart:include-hidden-cells="false" chart:treat-empty-cells="leave-gap"/></style:style><style:style style:name="ch5" style:family="chart" style:data-style-name="N0"><style:chart-properties chart:display-label="true" chart:logarithmic="false" chart:interval-minor-divisor="5" chart:reverse-direction="false" chart:link-data-style-to-source="false"/></style:style><style:style style:name="ch7" style:family="chart"><style:chart-properties chart:symbol-type="automatic"/><style:graphic-properties svg:stroke-width="0.079cm" svg:stroke-color="#4a7ebb" draw:fill-color="#4a7ebb"/></style:style><style:style style:name="ch8" style:family="chart"><style:graphic-properties draw:fill-color="#be4b48"/></style:style></office:automatic-styles><office:body><office:chart><chart:chart svg:width="14.277cm" svg:height="8.07cm" xlink:href=".." chart:class="chart:line" chart:style-name="ch1"><chart:legend chart:legend-position="end" svg:x="11.24cm" svg:y="3.54cm" chart:style-name="ch2"/><chart:plot-area chart:style-name="ch3"><chart:axis chart:dimension="x" chart:name="primary-x" chart:style-name="ch4"><chart:title><text:p><text:span>Year</text:span></text:p></chart:title><chart:categories table:cell-range-address="Sheet1.C1:Sheet1.C6"/></chart:axis><chart:axis chart:dimension="y" chart:name="primary-y" chart:style-name="ch5"><chart:grid chart:style-name="ch6" chart:class="major"/></chart:axis><chart:series chart:style-name="ch7" chart:values-cell-range-address="Sheet1.A1:Sheet1.A6" loext:label-string="&quot;1st Column&quot;" chart:class="chart:line"><chart:data-point chart:repeated="6"/></chart:series><chart:series chart:style-name="ch8" chart:values-cell-range-address="Sheet1.B1:Sheet1.B6" chart:label-cell-address="Sheet1.B1:Sheet1.B1" chart:class="chart:bar"><chart:data-point chart:repeated="6"/></chart:series><chart:wall chart:style-name="ch9"/></chart:plot-area><table:table table:name="local-table"><table:table-header-rows><table:table-row><table:table-cell><text:p/></table:table-cell><table:table-cell office:value-type="string"><text:p>1st Column</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>2nd</text:p></table:table-cell></table:table-row></table:table-header-rows><table:table-rows><table:table-row><table:table-cell office:value-type="string"><text:p>1</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p><draw:g><svg:desc>Sheet1.A1:Sheet1.A6</svg:desc></draw:g></table:table-cell><table:table-cell office:value-type="float" office:value="10"><text:p>10</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell office:value-type="string"><text:p>2</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="2"><text:p>2</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="12"><text:p>12</text:p></table:table-cell></table:table-row></table:table-rows></table:table></chart:chart></office:chart></office:body></office:document-content>"##;

    fn ods_with(frame: &str, object: Option<&str>) -> Vec<u8> {
        let content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"><office:body><office:spreadsheet><table:table table:name="Sheet2"><table:table-row><table:table-cell/><table:table-cell>{frame}</table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#
        );
        let mut w = crate::zip::ZipWriter::new();
        w.add("mimetype", b"application/vnd.oasis.opendocument.spreadsheet", false);
        w.add("content.xml", content.as_bytes(), true);
        if let Some(o) = object {
            w.add("Object 1/content.xml", o.as_bytes(), true);
        }
        w.finish()
    }

    const LO_FRAME: &str = r#"<draw:frame table:end-cell-address="Sheet2.I16" table:end-x="0.687cm" table:end-y="0.184cm" draw:z-index="0" draw:name="Chart 1" svg:width="14.276cm" svg:height="8.069cm" svg:x="0cm" svg:y="0.053cm"><draw:object draw:notify-on-update-of-ranges="Sheet1.A1:Sheet1.A6" xlink:href="./Object 1" xlink:type="simple"><loext:p/></draw:object><draw:image xlink:href="./ObjectReplacements/Object 1"/></draw:frame>"#;

    #[test]
    fn libreoffice_objects() {
        let wb = crate::ods::read(&ods_with(LO_FRAME, Some(LO_OBJECT))).unwrap();
        let sh = &wb.sheets[0];
        assert_eq!(sh.charts.len(), 1);
        assert!(sh.images.is_empty());
        let c = &sh.charts[0];
        assert_eq!((c.kind, c.anchor, c.from, c.name.as_str()), (ChartType::Line, Anchor::TwoCell, CellRef::new(0, 1), "Chart 1"));
        assert!(c.markers && c.y_axis.major_grid && c.legend == LegendPos::Right && !c.show_title);
        assert_eq!(c.x_axis.title, "Year");
        assert!((c.width - 14.276 / 2.54 * 96.0).abs() < 0.5);
        assert_eq!(c.series.len(), 2);
        assert_eq!(c.series[0].values.to_canonical().unwrap(), "Sheet1!A1:A6");
        assert_eq!(c.series[0].categories.to_canonical().unwrap(), "Sheet1!C1:C6");
        assert_eq!(c.series[0].color, Some(0x4a7ebb));
        assert_eq!(c.series[1].kind, Some(ChartType::Column));
        // Sheet1 is not in this file: the local table's values are used.
        let d = wb.chart_data(0, c);
        assert_eq!(d.series[0].name, "1st Column");
        assert_eq!(d.series[0].values, [Some(1.0), Some(2.0)]);
        assert_eq!(d.series[1].values, [Some(10.0), Some(12.0)]);
        assert_eq!(d.categories, ["1", "2"]);
        // LibreOffice's own default colors are its palette.
        let lo = LO_OBJECT.replace("#4a7ebb", "#004586").replace("#be4b48", "#ff420e");
        let wb = crate::ods::read(&ods_with(LO_FRAME, Some(&lo))).unwrap();
        let c = &wb.sheets[0].charts[0];
        assert_eq!((c.palette, c.series[0].color, c.series[1].color), (5, None, None));
    }

    #[test]
    fn hostile_objects() {
        // Missing object part, broken hrefs, junk numbers: no chart, no panic.
        let junk_obj = LO_OBJECT.replace("\"6\"", "\"-5\"").replace("chart:line\"", "chart:bogus\"");
        let cases: Vec<(String, Option<String>)> = vec![
            (LO_FRAME.into(), None),
            (LO_FRAME.replace("./Object 1", "../../etc"), Some(LO_OBJECT.into())),
            (LO_FRAME.replace("./Object 1", "http://x/"), Some(LO_OBJECT.into())),
            (LO_FRAME.replace("14.276cm", "NaNcm").replace("0.053cm", "-1e40cm"), Some(junk_obj)),
            (LO_FRAME.into(), Some("<office:document-content><office:body><office:chart><chart:chart chart:class=\"chart:bar\"><chart:plot-area><chart:series chart:values-cell-range-address=\"::..\"><chart:data-point chart:repeated=\"99999999999\" chart:style-name=\"x\"/>".into())),
            (LO_FRAME.into(), Some("<<<<".into())),
        ];
        for (frame, obj) in &cases {
            let wb = crate::ods::read(&ods_with(frame, obj.as_deref())).unwrap();
            for c in &wb.sheets[0].charts {
                let _ = wb.chart_data(0, c);
                assert!(c.width.is_finite() && c.width >= 1.0);
            }
            let _ = crate::ods::write(&wb);
        }
        // Sparklines with junk.
        let sp = r#"<calcext:sparkline-groups><calcext:sparkline-group calcext:type="x" calcext:line-width="NaNpt" calcext:min-axis-type="custom" calcext:manual-min="zz"><calcext:sparklines><calcext:sparkline calcext:cell-address="junk" calcext:data-range=""/><calcext:sparkline calcext:cell-address="S.A1" calcext:data-range="S.B1:S.C1"/></calcext:sparklines></calcext:sparkline-group><calcext:sparkline-group/></calcext:sparkline-groups>"#;
        let content = format!(r#"<office:document-content><office:body><office:spreadsheet><table:table table:name="S"><table:table-row><table:table-cell/></table:table-row>{sp}</table:table></office:spreadsheet></office:body></office:document-content>"#);
        let mut w = crate::zip::ZipWriter::new();
        w.add("content.xml", content.as_bytes(), true);
        let wb = crate::ods::read(&w.finish()).unwrap();
        let g = &wb.sheets[0].sparklines;
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].sparklines.len(), g[0].min, g[0].line_weight), (1, SparkAxis::Individual, 0.75));
    }
}
