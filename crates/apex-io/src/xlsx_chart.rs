//! Charts (`xl/charts/chartN.xml`, DrawingML charts, and the newer
//! `chartExN.xml` waterfall and histogram charts) and sparklines (the
//! worksheet's `x14:sparklineGroups` extension) in .xlsx.
//!
//! Charts are read into a small element tree: they are small and their
//! elements come in many orders.

use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::chart::{Axis, Bins, Blanks, BoxFormat, ErrorAmount, ErrorBars, ErrorDir, Fill, Grouping, Labels, LegendPos, Marker, MAX_POINTS, MAX_SERIES, Series, SparkAxis, SparkKind, TrendKind, Trendline};
use apex_sheet::formula::Expr;
use apex_sheet::{CellRef, Chart, ChartData, ChartType, DataRef, Sheet, SparkGroup, Sparkline, Value, Workbook};

use crate::xml::{Ev, HEADER, Reader, Tag, esc};

// ---------------------------------------------------------------------------
// Element tree

/// Deepest element kept and most elements read (hostile files).
const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 2_000_000;

#[derive(Default, Debug)]
struct Node {
    /// Local name (no prefix).
    name: String,
    /// Attributes by local name.
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
    text: String,
}

impl Node {
    fn from_tag(t: &Tag) -> Node {
        Node { name: t.local().to_string(), attrs: t.attrs().map(|(k, v)| (crate::xml::local(k).to_string(), v.into_owned())).collect(), ..Default::default() }
    }

    fn get(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find(|k| k.name == name)
    }

    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.kids.iter().filter(move |k| k.name == name)
    }

    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    fn val(&self) -> Option<&str> {
        self.attr("val")
    }

    fn path(&self, names: &[&str]) -> Option<&Node> {
        names.iter().try_fold(self, |n, name| n.get(name))
    }

    /// `val` of child `name` as a number.
    fn num(&self, name: &str) -> Option<f64> {
        self.get(name)?.val()?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
    }

    /// A boolean child (`val` defaults to true, as in the schema).
    fn flag(&self, name: &str) -> Option<bool> {
        self.get(name).map(|n| n.val().is_none_or(|v| v == "1" || v == "true"))
    }

    /// Text of the element and its descendants.
    fn all_text(&self) -> String {
        let mut s = self.text.clone();
        for k in &self.kids {
            s.push_str(&k.all_text());
        }
        s
    }

    /// First descendant named `name`.
    fn find(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find_map(|k| if k.name == name { Some(k) } else { k.find(name) })
    }
}

/// The tree of the element whose start tag `t` was just read.
fn subtree(r: &mut Reader, t: &Tag, empty: bool) -> Node {
    let mut root = Node::from_tag(t);
    if empty {
        return root;
    }
    let mut stack: Vec<Node> = Vec::new();
    let mut count = 0usize;
    loop {
        let top = stack.last_mut().unwrap_or(&mut root);
        match r.next() {
            Ev::Start(t) => {
                count += 1;
                if count > MAX_NODES || stack.len() >= MAX_DEPTH {
                    r.skip();
                } else {
                    stack.push(Node::from_tag(&t));
                }
            }
            Ev::Empty(t) => {
                count += 1;
                if count <= MAX_NODES {
                    top.kids.push(Node::from_tag(&t));
                }
            }
            Ev::Text(x) => top.text.push_str(&x),
            Ev::End(_) => match stack.pop() {
                Some(n) => stack.last_mut().unwrap_or(&mut root).kids.push(n),
                None => break,
            },
            Ev::Eof => {
                while let Some(n) = stack.pop() {
                    stack.last_mut().unwrap_or(&mut root).kids.push(n);
                }
                break;
            }
        }
    }
    flatten_alternates(&mut root);
    root
}

/// `mc:AlternateContent` becomes what it falls back to: the plain form.
fn flatten_alternates(n: &mut Node) {
    if n.kids.iter().any(|k| k.name == "AlternateContent") {
        let kids = std::mem::take(&mut n.kids);
        for k in kids {
            if k.name == "AlternateContent" {
                let pick = k.kids.iter().position(|c| c.name == "Fallback").or_else(|| k.kids.iter().position(|c| c.name == "Choice"));
                if let Some(i) = pick {
                    let mut k = k;
                    n.kids.append(&mut k.kids.swap_remove(i).kids);
                }
            } else {
                n.kids.push(k);
            }
        }
    }
    for k in &mut n.kids {
        flatten_alternates(k);
    }
}

fn dom(xml: &str) -> Option<Node> {
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Start(t) => return Some(subtree(&mut r, &t, false)),
            Ev::Empty(t) => return Some(subtree(&mut r, &t, true)),
            Ev::Eof => return None,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Colors

/// Color palettes the chart styles use, 10 colors each. Palette 0 is
/// Office's accent colors (what Excel shows by default).
pub(crate) fn palette(p: u8) -> [u32; 10] {
    apex_sheet::chart::palette(p)
}

thread_local! {
    /// Colors of the theme of the workbook being read (dk1, lt1, dk2, lt2,
    /// accent1..6, hlink, folHlink).
    static THEME: std::cell::Cell<[u32; 12]> = const { std::cell::Cell::new(OFFICE) };
}

const OFFICE: [u32; 12] = [0x000000, 0xFFFFFF, 0x44546A, 0xE7E6E6, 0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47, 0x0563C1, 0x954F72];

/// Scheme colors of the charts read next come from `theme` (None: Office).
pub fn set_theme(theme: Option<&apex_doc::Theme>) {
    let colors = theme.map_or(OFFICE, |t| std::array::from_fn(|i| t.color(i as u8)));
    THEME.with(|c| c.set(colors));
}

/// Theme colors by scheme name.
fn scheme(name: &str) -> u32 {
    let slot = match name {
        "dk1" | "tx1" => 0,
        "lt1" | "bg1" => 1,
        "dk2" | "tx2" => 2,
        "lt2" | "bg2" => 3,
        "accent1" => 4,
        "accent2" => 5,
        "accent3" => 6,
        "accent4" => 7,
        "accent5" => 8,
        "accent6" => 9,
        "hlink" => 10,
        "folHlink" => 11,
        _ => 0,
    };
    THEME.with(|c| c.get()[slot])
}

/// The accent colors of the theme, when not Office's.
fn theme_accents() -> Option<[u32; 6]> {
    let t = THEME.with(|c| c.get());
    (t != OFFICE).then(|| std::array::from_fn(|i| t[4 + i]))
}

fn hex(v: &str) -> Option<u32> {
    let v = v.trim();
    let v = if v.len() == 8 { &v[2..] } else { v };
    (v.len() == 6).then(|| u32::from_str_radix(v, 16).ok()).flatten()
}

/// Applies DrawingML color transforms (lumMod, lumOff, tint, shade).
fn transform(c: u32, n: &Node) -> u32 {
    let mut rgb = [(c >> 16 & 255) as f64 / 255.0, (c >> 8 & 255) as f64 / 255.0, (c & 255) as f64 / 255.0];
    let pct = |k: &Node| k.val().and_then(|v| v.parse::<f64>().ok()).map_or(1.0, |v| v / 100_000.0);
    let (mut lm, mut lo) = (1.0, 0.0);
    for k in &n.kids {
        match k.name.as_str() {
            "lumMod" => lm = pct(k),
            "lumOff" => lo = pct(k),
            "tint" => rgb = rgb.map(|x| x * pct(k) + (1.0 - pct(k))),
            "shade" => rgb = rgb.map(|x| x * pct(k)),
            _ => {}
        }
    }
    if lm != 1.0 || lo != 0.0 {
        // Luminance change in HSL.
        let (mx, mn) = (rgb.iter().cloned().fold(0.0, f64::max), rgb.iter().cloned().fold(1.0, f64::min));
        let l = (mx + mn) / 2.0;
        let nl = (l * lm + lo).clamp(0.0, 1.0);
        rgb = if l <= 0.0 {
            [nl; 3]
        } else if l >= 1.0 {
            [nl; 3]
        } else {
            // Scale the distance to gray, keeping hue and saturation.
            let s = if l < 0.5 { nl / l } else { (1.0 - nl) / (1.0 - l) };
            rgb.map(|x| (nl + (x - l) * s).clamp(0.0, 1.0))
        };
    }
    let b = rgb.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u32);
    b[0] << 16 | b[1] << 8 | b[2]
}

/// The color of a color element (srgbClr, schemeClr, sysClr, prstClr).
fn color_of(n: &Node) -> Option<u32> {
    let base = match n.name.as_str() {
        "srgbClr" => hex(n.val()?)?,
        "schemeClr" => scheme(n.val()?),
        "sysClr" => n.attr("lastClr").and_then(hex).unwrap_or(0),
        "scrgbClr" => {
            let p = |k: &str| n.attr(k).and_then(|v| v.parse::<f64>().ok()).map_or(0, |v| ((v / 100_000.0).clamp(0.0, 1.0) * 255.0) as u32);
            p("r") << 16 | p("g") << 8 | p("b")
        }
        "prstClr" => match n.val()? {
            "black" => 0,
            "white" => 0xFFFFFF,
            "red" => 0xFF0000,
            "green" => 0x008000,
            "blue" => 0x0000FF,
            "yellow" => 0xFFFF00,
            "gray" | "grey" => 0x808080,
            _ => 0,
        },
        _ => return None,
    };
    Some(transform(base, n))
}

/// Fill of a shape properties element: Some(None) = no fill.
fn fill_of(sp: &Node) -> Option<Option<u32>> {
    for k in &sp.kids {
        match k.name.as_str() {
            "noFill" => return Some(None),
            "solidFill" => return Some(k.kids.iter().find_map(color_of)),
            "gradFill" => return Some(k.find("gs").and_then(|g| g.kids.iter().find_map(color_of))),
            "pattFill" => return Some(k.get("fgClr").and_then(|g| g.kids.iter().find_map(color_of))),
            _ => {}
        }
    }
    None
}

/// A gradient or pattern fill of a shape properties element.
fn fancy_of(sp: &Node) -> Option<Fill> {
    for k in &sp.kids {
        match k.name.as_str() {
            "gradFill" => {
                let stops: Vec<(f32, u32)> = k.get("gsLst")?.all("gs").filter_map(|g| Some((g.attr("pos")?.parse::<f32>().ok()? / 100_000.0, g.kids.iter().find_map(color_of)?))).collect();
                if stops.is_empty() {
                    return None;
                }
                let radial = k.get("path").is_some();
                let angle = k.get("lin").and_then(|l| l.attr("ang")).and_then(|a| a.parse::<f32>().ok()).map_or(if radial { 0.0 } else { 90.0 }, |a| a / 60_000.0);
                let mut f = Fill::Gradient { stops, angle, radial };
                if let Fill::Gradient { stops, .. } = &mut f {
                    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                }
                return Some(f);
            }
            "pattFill" => {
                let c = |n: &str, d: u32| k.get(n).and_then(|g| g.kids.iter().find_map(color_of)).unwrap_or(d);
                return Some(Fill::Pattern { preset: k.attr("prst").unwrap_or("pct5").to_string(), fg: c("fgClr", 0), bg: c("bgClr", 0xFFFFFF) });
            }
            "noFill" | "solidFill" => return None,
            _ => {}
        }
    }
    None
}

/// Chart or plot area look from its `spPr` and `txPr`.
fn read_box(sp: Option<&Node>, tx: Option<&Node>) -> BoxFormat {
    let mut b = BoxFormat::default();
    if let Some(sp) = sp {
        match fill_of(sp) {
            Some(None) => b.no_fill = true,
            Some(Some(c)) => b.fill = Some(c),
            None => {}
        }
        b.fancy = fancy_of(sp);
        match sp.get("ln").and_then(fill_of) {
            Some(None) => b.no_border = true,
            Some(Some(c)) => b.border = Some(c),
            None => {}
        }
    }
    if let Some(r) = tx.and_then(|t| t.find("defRPr")) {
        b.font_size = r.attr("sz").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 100.0);
        b.bold = r.attr("b").map(|v| v == "1" || v == "true");
        b.text_color = r.get("solidFill").and_then(|f| f.kids.iter().find_map(color_of));
        b.font = r.get("latin").and_then(|l| l.attr("typeface")).filter(|f| !f.is_empty() && !f.starts_with('+')).map(str::to_string);
    }
    b
}

// ---------------------------------------------------------------------------
// Reading charts

/// Cached values one chart keeps from its file, all series together.
const CHART_POINTS: usize = 1_000_000;

thread_local! {
    /// What is left of [`CHART_POINTS`] for the chart being read.
    static POINTS_LEFT: std::cell::Cell<usize> = const { std::cell::Cell::new(CHART_POINTS) };
}

fn cache_values(n: &Node, numeric: bool) -> Vec<Value> {
    let left = POINTS_LEFT.get();
    let count = n.get("ptCount").and_then(|p| p.val()).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0).min(MAX_POINTS).min(left);
    let mut out = vec![Value::Empty; count];
    let mut room = left - count;
    for pt in n.all("pt") {
        let Some(i) = pt.attr("idx").and_then(|v| v.trim().parse::<usize>().ok()).filter(|i| *i < MAX_POINTS) else { continue };
        let text = match pt.get("v") {
            Some(v) => v.all_text(),
            None => pt.text.clone(),
        };
        if i >= out.len() {
            if i + 1 - out.len() > room {
                continue;
            }
            room -= i + 1 - out.len();
            out.resize(i + 1, Value::Empty);
        }
        out[i] = match text.trim().parse::<f64>() {
            Ok(v) if numeric && v.is_finite() => Value::Number(v),
            _ if numeric && text.trim().is_empty() => Value::Empty,
            _ => Value::Text(Arc::from(text.as_str())),
        };
    }
    POINTS_LEFT.set(room);
    out
}

/// Whether the formula refers to cells.
fn has_refs(r: &DataRef) -> bool {
    let Some(f) = &r.formula else { return false };
    let mut e = (**f).clone();
    let mut any = false;
    e.visit_refs_mut(&mut |x| any |= matches!(x, Expr::Ref { .. } | Expr::Range { .. }));
    any
}

/// A data source element (tx, cat, val, xVal, yVal).
fn data_ref(n: Option<&Node>) -> DataRef {
    let Some(n) = n else { return DataRef::default() };
    for k in &n.kids {
        match k.name.as_str() {
            "strRef" | "numRef" | "multiLvlStrRef" => {
                let numeric = k.name == "numRef";
                let f = k.get("f").map(|f| f.all_text()).unwrap_or_default();
                let cache = match k.kids.iter().find(|c| c.name.ends_with("Cache")) {
                    // Several levels: the innermost (first) one.
                    Some(c) if c.name == "multiLvlStrCache" => c.get("lvl").map(|l| cache_values(l, false)).unwrap_or_default(),
                    Some(c) => cache_values(c, numeric),
                    None => Vec::new(),
                };
                let mut r = DataRef::parse(&f);
                if !f.trim().starts_with("_xlchart.") && !has_refs(&r) {
                    // LibreOffice writes "categories", "0"... for data of
                    // its own.
                    r.formula = None;
                }
                r.cache = cache;
                return r;
            }
            "strLit" => return DataRef::literal(cache_values(k, false)),
            "numLit" => return DataRef::literal(cache_values(k, true)),
            "v" => return DataRef::text(&k.all_text()),
            _ => {}
        }
    }
    DataRef::default()
}

/// Text of a title (`c:title`): rich text, or the cached text of a
/// reference.
fn title_text(t: &Node) -> String {
    let Some(tx) = t.get("tx") else { return String::new() };
    if let Some(rich) = tx.get("rich") {
        return rich.all("p").map(|p| p.kids.iter().filter(|r| r.name == "r" || r.name == "fld").filter_map(|r| r.get("t")).map(|t| t.all_text()).collect::<String>()).collect::<Vec<_>>().join("\n").trim_end().to_string();
    }
    if let Some(sr) = tx.get("strRef") {
        return sr.find("strCache").map(|c| cache_values(c, false).iter().filter_map(|v| if let Value::Text(t) = v { Some(t.to_string()) } else { None }).collect::<Vec<_>>().join(" ")).unwrap_or_default();
    }
    tx.get("v").map(|v| v.all_text()).unwrap_or_default()
}

fn read_axis(n: &Node) -> Axis {
    let mut a = Axis::default();
    if let Some(s) = n.get("scaling") {
        a.min = s.num("min");
        a.max = s.num("max");
        a.log_base = s.num("logBase");
        a.reverse = s.get("orientation").and_then(|o| o.val()) == Some("maxMin");
    }
    a.visible = !n.flag("delete").unwrap_or(false);
    a.major_grid = n.get("majorGridlines").is_some();
    a.minor_grid = n.get("minorGridlines").is_some();
    a.major = n.num("majorUnit");
    a.minor = n.num("minorUnit");
    if let Some(t) = n.get("title") {
        a.title = title_text(t);
    }
    if let Some(f) = n.get("numFmt") {
        let linked = f.attr("sourceLinked").is_some_and(|v| v == "1" || v == "true");
        let code = f.attr("formatCode").unwrap_or_default();
        if !linked && !code.eq_ignore_ascii_case("general") {
            a.format = code.to_string();
        }
    }
    a
}

fn group_kind(g: &Node) -> Option<ChartType> {
    Some(match g.name.as_str() {
        "barChart" | "bar3DChart" => {
            if g.get("barDir").and_then(|d| d.val()) == Some("bar") {
                ChartType::Bar
            } else {
                ChartType::Column
            }
        }
        "lineChart" | "line3DChart" => ChartType::Line,
        "areaChart" | "area3DChart" => ChartType::Area,
        "pieChart" | "pie3DChart" | "ofPieChart" => ChartType::Pie,
        "doughnutChart" => ChartType::Doughnut,
        "scatterChart" => ChartType::Scatter,
        "radarChart" => ChartType::Radar,
        "bubbleChart" => ChartType::Bubble,
        "stockChart" => ChartType::Stock,
        "surfaceChart" | "surface3DChart" => ChartType::Surface,
        _ => return None,
    })
}

fn marker_of(n: &Node) -> Marker {
    match n.path(&["marker", "symbol"]).and_then(|s| s.val()) {
        None => Marker::Auto,
        Some("none") => Marker::None,
        Some("square") => Marker::Square,
        Some("diamond") => Marker::Diamond,
        Some("triangle") => Marker::Triangle,
        Some(_) => Marker::Circle,
    }
}

fn read_labels(n: Option<&Node>, l: &mut Labels) {
    let Some(n) = n else { return };
    if n.flag("delete") == Some(true) {
        return;
    }
    let on = |k: &str| n.flag(k).unwrap_or(false);
    l.value |= on("showVal");
    l.percent |= on("showPercent");
    l.category |= on("showCatName");
    l.series |= on("showSerName");
    l.cells |= n.find("showDataLabelsRange").and_then(|x| x.val()).is_some_and(|v| v == "1" || v == "true");
}

/// Error bars of a series (the vertical ones of a scatter chart).
fn read_error_bars(ser: &Node) -> Option<ErrorBars> {
    let e = ser.all("errBars").find(|e| e.get("errDir").and_then(|d| d.val()) != Some("x"))?;
    let v = e.num("val").filter(|v| v.is_finite()).unwrap_or(1.0).abs();
    let amount = match e.get("errValType").and_then(|t| t.val()) {
        Some("percentage") => ErrorAmount::Percent(v),
        Some("stdDev") => ErrorAmount::StdDev(v),
        Some("stdErr") => ErrorAmount::StdErr,
        Some("cust") => ErrorAmount::Custom { plus: data_ref(e.get("plus")), minus: data_ref(e.get("minus")) },
        _ => ErrorAmount::Fixed(v),
    };
    let dir = match e.get("errBarType").and_then(|t| t.val()) {
        Some("plus") => ErrorDir::Plus,
        Some("minus") => ErrorDir::Minus,
        _ => ErrorDir::Both,
    };
    Some(ErrorBars { amount, dir, cap: !e.flag("noEndCap").unwrap_or(false) })
}

/// The label range of a series (Excel 2013's "value from cells").
fn read_label_range(ser: &Node) -> DataRef {
    let Some(r) = ser.find("datalabelsRange") else { return DataRef::default() };
    let f = r.get("f").map(|f| f.all_text()).unwrap_or_default();
    let mut d = DataRef::parse(&f);
    if !has_refs(&d) {
        return DataRef::default();
    }
    d.cache = r.get("dlblRangeCache").map(|c| cache_values(c, false)).unwrap_or_default();
    d
}

fn read_series(ser: &Node, kind: ChartType, labels: &mut Labels) -> Series {
    let mut s = Series {
        name: data_ref(ser.get("tx")),
        categories: data_ref(ser.get("cat").or_else(|| ser.get("xVal"))),
        values: data_ref(ser.get("val").or_else(|| ser.get("yVal"))),
        sizes: data_ref(ser.get("bubbleSize")),
        ..Default::default()
    };
    let lined = matches!(kind, ChartType::Line | ChartType::Scatter | ChartType::Radar);
    if let Some(sp) = ser.get("spPr") {
        let fill = fill_of(sp);
        let ln = sp.get("ln");
        let ln_fill = ln.and_then(fill_of);
        if let Some(w) = ln.and_then(|l| l.attr("w")).and_then(|w| w.parse::<f32>().ok()) {
            s.line_width = Some(w / 12_700.0);
        }
        if lined {
            s.color = ln_fill.flatten().or(fill.flatten());
            // 3D lines are ribbons, drawn with their fill.
            s.no_line = ln_fill == Some(None) && fill.flatten().is_none();
        } else {
            s.color = fill.flatten();
            s.no_fill = fill == Some(None);
            s.fancy = fancy_of(sp);
            s.line_color = ln_fill.flatten();
            s.no_line = ln_fill == Some(None);
        }
    }
    s.marker = marker_of(ser);
    s.smooth = ser.flag("smooth").unwrap_or(false);
    if let Some(t) = ser.get("trendline") {
        let n = |k: &str| t.num(k).unwrap_or(2.0).clamp(2.0, 255.0) as u8;
        let kind = match t.get("trendlineType").and_then(|v| v.val()) {
            Some("exp") => Some(TrendKind::Exponential),
            Some("log") => Some(TrendKind::Logarithmic),
            Some("poly") => Some(TrendKind::Polynomial(n("order").min(6))),
            Some("power") => Some(TrendKind::Power),
            Some("movingAvg") => Some(TrendKind::MovingAverage(n("period"))),
            Some("linear") | None => Some(TrendKind::Linear),
            Some(_) => None,
        };
        s.trendline = kind.map(|kind| Trendline { kind, show_equation: t.flag("dispEq").unwrap_or(false), show_r2: t.flag("dispRSqr").unwrap_or(false) });
    }
    s.explode = ser.num("explosion").unwrap_or(0.0).clamp(0.0, 255.0) as u8;
    for p in ser.all("dPt") {
        if let (Some(i), Some(e)) = (p.num("idx"), p.num("explosion")) {
            let e = e.clamp(0.0, 255.0) as u8;
            if e != s.explode && (0.0..MAX_POINTS as f64).contains(&i) {
                s.point_explode.push((i as u32, e));
            }
        }
        let (Some(i), Some(Some(c))) = (p.num("idx"), p.get("spPr").and_then(fill_of)) else { continue };
        if (0.0..MAX_POINTS as f64).contains(&i) {
            s.point_colors.push((i as u32, c));
        }
    }
    read_labels(ser.get("dLbls"), labels);
    s.error_bars = read_error_bars(ser);
    s.label_range = read_label_range(ser);
    if kind == ChartType::Stock {
        // Their lines are hidden: the chart draws its own.
        s.no_line = false;
        s.line_width = None;
        s.marker = Marker::Auto;
    }
    s
}

/// A chart from the xml of a `chartN.xml` part (placed at A1).
pub fn read_chart(xml: &str) -> Option<Chart> {
    POINTS_LEFT.set(CHART_POINTS);
    let chart = read_chart_inner(xml);
    POINTS_LEFT.set(CHART_POINTS);
    chart
}

fn read_chart_inner(xml: &str) -> Option<Chart> {
    let root = dom(xml)?;
    let ch = root.get("chart")?;
    let plot = ch.get("plotArea")?;
    let groups: Vec<(&Node, ChartType)> = plot.kids.iter().filter_map(|g| Some((g, group_kind(g)?))).collect();
    let (main, kind) = *groups.first()?;
    let mut chart = Chart::new(kind, CellRef::new(0, 0), 480.0, 288.0);
    if let Some(v) = root.num("style") {
        // Excel's 48 styles come in rows of 8 color variations; the row is
        // the look (c14 styles are 100 + the same number).
        let v = if v > 100.0 { v - 100.0 } else { v };
        chart.style = (((v.clamp(1.0, 48.0) as u8) - 1) / 8).min(apex_sheet::chart::STYLES - 1);
    }
    chart.grouping = match main.get("grouping").and_then(|g| g.val()) {
        Some("stacked") => Grouping::Stacked,
        Some("percentStacked") => Grouping::Percent,
        _ => Grouping::Clustered,
    };
    chart.vary_colors = main.flag("varyColors").unwrap_or(false);
    if let Some(v) = main.num("gapWidth") {
        chart.gap = v.clamp(0.0, 500.0) as u16;
    }
    if let Some(v) = main.num("overlap") {
        // Stacked columns always overlap fully; the value only matters
        // side by side.
        chart.overlap = if chart.grouping == Grouping::Clustered { v.clamp(-100.0, 100.0) as i8 } else { 0 };
    }
    if let Some(v) = main.num("firstSliceAng") {
        chart.first_angle = v.clamp(0.0, 359.0) as u16;
    }
    if let Some(v) = main.num("holeSize") {
        chart.hole = v.clamp(10.0, 90.0) as u8;
    }
    chart.view3d = main.name.ends_with("3DChart");
    if chart.view3d && kind == ChartType::Pie {
        if let Some(v) = ch.get("view3D").and_then(|v| v.num("rotY")) {
            chart.first_angle = v.clamp(0.0, 359.0) as u16;
        }
    }
    chart.filled = main.get("radarStyle").and_then(|r| r.val()) == Some("filled");
    if let Some(v) = main.num("bubbleScale") {
        chart.bubble_scale = v.clamp(1.0, 300.0) as u16;
    }
    let mut labels = Labels::default();
    read_labels(main.get("dLbls"), &mut labels);
    let ax_ids = |g: &Node| -> Vec<String> { g.all("axId").filter_map(|a| a.val().map(str::to_string)).collect() };
    let main_ids = ax_ids(main);
    let axis = |id: Option<&String>| -> Option<&Node> {
        let id = id?;
        plot.kids.iter().find(|a| a.name.ends_with("Ax") && a.get("axId").and_then(|x| x.val()) == Some(id.as_str()))
    };
    let mut secondary_val = None;
    for (gi, (g, k)) in groups.iter().enumerate() {
        read_labels(g.get("dLbls"), &mut labels);
        let ids = ax_ids(g);
        let secondary = gi > 0 && kind.has_axes() && !ids.is_empty() && ids != main_ids;
        if secondary && secondary_val.is_none() {
            secondary_val = axis(ids.get(1));
        }
        let group_markers = g.flag("marker").unwrap_or(true);
        for ser in g.all("ser") {
            if chart.series.len() >= MAX_SERIES {
                break;
            }
            let mut s = read_series(ser, *k, &mut labels);
            if gi > 0 && (*k != kind || secondary) {
                s.kind = Some(*k);
            }
            s.secondary = secondary;
            if *k == ChartType::Line && !group_markers && s.marker == Marker::Auto {
                s.marker = Marker::None;
            }
            chart.series.push(s);
        }
        if gi == 0 && *k == ChartType::Scatter {
            let style = g.get("scatterStyle").and_then(|s| s.val()).unwrap_or("marker");
            chart.smooth = style.starts_with("smooth");
            chart.lines = style != "marker";
        }
    }
    // Markers and lines are the chart's when every series agrees.
    let n = chart.series.len();
    let main_series = |c: &Chart| c.series.iter().filter(|s| s.kind.is_none()).count();
    if matches!(kind, ChartType::Line | ChartType::Scatter | ChartType::Radar) && main_series(&chart) > 0 {
        let none = chart.series.iter().filter(|s| s.kind.is_none()).all(|s| s.marker == Marker::None);
        chart.markers = !none;
        if kind == ChartType::Scatter && chart.series.iter().filter(|s| s.kind.is_none()).all(|s| s.no_line) {
            chart.lines = false;
        }
        for s in chart.series.iter_mut().filter(|s| s.kind.is_none()) {
            if none {
                s.marker = Marker::Auto;
            }
            if kind == ChartType::Scatter && !chart.lines {
                s.no_line = false;
            }
        }
        if n > 0 && chart.series.iter().all(|s| s.smooth) {
            chart.smooth = true;
        }
        if chart.smooth {
            for s in &mut chart.series {
                s.smooth = false;
            }
        }
    }
    if chart.view3d && kind == ChartType::Line {
        // 3-D lines are ribbons, without markers.
        chart.markers = false;
    }
    chart.labels = labels;
    if let Some(t) = plot.get("dTable") {
        chart.data_table = true;
        chart.table_keys = t.flag("showKeys").unwrap_or(true);
    }
    if kind.has_axes() {
        if let Some(a) = axis(main_ids.first()) {
            chart.x_axis = read_axis(a);
        }
        if let Some(a) = axis(main_ids.get(1)) {
            chart.y_axis = read_axis(a);
        }
        if let Some(a) = secondary_val {
            chart.y2_axis = read_axis(a);
        }
    }
    chart.show_title = ch.get("title").is_some();
    if let Some(t) = ch.get("title") {
        chart.title = title_text(t);
    }
    chart.legend = match ch.get("legend") {
        None => LegendPos::None,
        Some(l) => match l.get("legendPos").and_then(|p| p.val()) {
            Some("l") => LegendPos::Left,
            Some("t") => LegendPos::Top,
            Some("b") => LegendPos::Bottom,
            _ => LegendPos::Right,
        },
    };
    chart.visible_only = ch.flag("plotVisOnly").unwrap_or(true);
    chart.blanks = match ch.get("dispBlanksAs").map(|b| b.val().unwrap_or("zero")) {
        Some("zero") => Blanks::Zero,
        Some("span") => Blanks::Span,
        _ => Blanks::Gap,
    };
    chart.area = read_box(root.get("spPr"), root.get("txPr"));
    chart.plot = read_box(plot.get("spPr"), None);
    detect_palette(&mut chart);
    theme_colors(&mut chart);
    if let Some(e) = root.get("extLst").and_then(|l| l.all("ext").find(|e| e.attr("uri") == Some(APEX_EXT))).and_then(|e| e.get("chart")) {
        apply_emulation(&mut chart, e);
    }
    chart.sanitize();
    Some(chart)
}

/// Colors that are those of one of our palettes (for the series' and
/// slices' positions) become automatic again, remembering the palette.
pub(crate) fn detect_palette(chart: &mut Chart) {
    let vary = chart.vary_colors;
    let score = |p: u8| -> usize {
        let pal = palette(p);
        chart.series.iter().enumerate().filter(|(i, s)| s.color == Some(pal[i % 10])).count() + if vary { chart.series.iter().flat_map(|s| &s.point_colors).filter(|(i, c)| *c == pal[*i as usize % 10]).count() } else { 0 }
    };
    let best = (0..apex_sheet::chart::PALETTES).max_by_key(|&p| (score(p), std::cmp::Reverse(p))).unwrap_or(0);
    let pal = palette(best);
    chart.palette = best;
    for (i, s) in chart.series.iter_mut().enumerate() {
        if s.color == Some(pal[i % 10]) && s.fancy.is_none() {
            s.color = None;
        }
        if vary {
            s.point_colors.retain(|(i, c)| *c != pal[*i as usize % 10]);
        }
    }
}

/// Automatic colors of a workbook with another theme than Office are its
/// accents.
fn theme_colors(chart: &mut Chart) {
    if chart.palette == 0 {
        chart.accents = theme_accents();
    }
}

/// A waterfall or histogram chart from the xml of a `chartExN.xml` part;
/// None for the other kinds of the newer charts.
pub fn read_chartex(xml: &str) -> Option<Chart> {
    POINTS_LEFT.set(CHART_POINTS);
    let chart = read_chartex_inner(xml);
    POINTS_LEFT.set(CHART_POINTS);
    chart
}

fn read_chartex_inner(xml: &str) -> Option<Chart> {
    let root = dom(xml)?;
    let ch = root.get("chart")?;
    let region = ch.path(&["plotArea", "plotAreaRegion"])?;
    let all: Vec<&Node> = region.all("series").collect();
    let ser = *all.first()?;
    let kind = match ser.attr("layoutId")? {
        "waterfall" => ChartType::Waterfall,
        "clusteredColumn" if all.iter().any(|s| s.attr("layoutId") == Some("paretoLine")) => ChartType::Pareto,
        "clusteredColumn" => ChartType::Histogram,
        "treemap" => ChartType::Treemap,
        "sunburst" => ChartType::Sunburst,
        "boxWhisker" => ChartType::BoxWhisker,
        "funnel" => ChartType::Funnel,
        _ => return None,
    };
    let mut chart = Chart::new(kind, CellRef::new(0, 0), 480.0, 288.0);
    let dim = |ser: &Node, ty: &str| -> DataRef {
        let data_id = ser.get("dataId").and_then(|d| d.val()).unwrap_or("0");
        let data = root.get("chartData").and_then(|d| d.all("data").find(|x| x.attr("id").unwrap_or("0") == data_id));
        let Some(d) = data.and_then(|d| d.kids.iter().find(|k| (k.name == "numDim" || k.name == "strDim") && k.attr("type") == Some(ty))) else { return DataRef::default() };
        let f = d.get("f").map(|f| f.all_text()).unwrap_or_default();
        let mut r = DataRef::parse(&f);
        r.cache = d.get("lvl").map(|l| {
            // Out of the chart's budget of points, as `cache_values`.
            let left = POINTS_LEFT.get();
            let count = l.attr("ptCount").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(MAX_POINTS).min(left);
            let mut v = vec![Value::Empty; count];
            let mut room = left - count;
            for pt in l.all("pt") {
                let Some(i) = pt.attr("idx").and_then(|x| x.parse::<usize>().ok()).filter(|i| *i < MAX_POINTS) else { continue };
                if i >= v.len() {
                    if i + 1 - v.len() > room {
                        continue;
                    }
                    room -= i + 1 - v.len();
                    v.resize(i + 1, Value::Empty);
                }
                v[i] = match (d.name.as_str(), pt.text.trim().parse::<f64>()) {
                    ("numDim", Ok(n)) if n.is_finite() => Value::Number(n),
                    _ => Value::Text(Arc::from(pt.text.as_str())),
                };
            }
            POINTS_LEFT.set(room);
            v
        }).unwrap_or_default();
        r
    };
    let tx = |n: Option<&Node>| -> Option<DataRef> {
        let td = n?.path(&["tx", "txData"])?;
        let v = td.get("v").map(|v| v.all_text()).unwrap_or_default();
        Some(match td.get("f") {
            Some(f) => {
                let mut r = DataRef::parse(&f.all_text());
                r.cache = vec![Value::Text(Arc::from(v.as_str()))];
                r
            }
            None => DataRef::text(&v),
        })
    };
    for sr in all.iter().filter(|s| s.attr("layoutId") != Some("paretoLine")).take(MAX_SERIES) {
        let values = if matches!(kind, ChartType::Treemap | ChartType::Sunburst) { dim(sr, "size") } else { dim(sr, "val") };
        let values = if values.is_empty() { dim(sr, "val") } else { values };
        let mut s = Series { values, categories: dim(sr, "cat"), name: tx(Some(sr)).unwrap_or_default(), ..Default::default() };
        if let Some(sp) = sr.get("spPr") {
            s.color = fill_of(sp).flatten();
        }
        chart.series.push(s);
    }
    if let Some(v) = ser.path(&["layoutPr", "visibility"]) {
        let on = |k: &str| v.attr(k).is_none_or(|v| v == "1" || v == "true");
        chart.show_mean = on("meanMarker");
        chart.show_outliers = on("outliers");
    }
    if let Some(lp) = ser.get("layoutPr") {
        if let Some(st) = lp.get("subtotals") {
            chart.totals = st.all("idx").filter_map(|i| i.val()?.parse::<u32>().ok()).filter(|i| (*i as usize) < MAX_POINTS).collect();
        }
        if let Some(b) = lp.get("binning") {
            let v = |k: &str| b.attr(k).or_else(|| b.get(k).and_then(|n| n.val())).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
            chart.bins = match (v("binSize"), v("binCount")) {
                (Some(w), _) => Bins::Width(w),
                (None, Some(n)) => Bins::Count(n as u32),
                _ => Bins::Auto,
            };
        }
    }
    if let Some(vis) = ser.path(&["dataLabels", "visibility"]) {
        let on = |k: &str| vis.attr(k).is_some_and(|v| v == "1" || v == "true");
        // Treemaps and sunbursts always show their names.
        let tree = matches!(kind, ChartType::Treemap | ChartType::Sunburst);
        chart.labels = Labels { value: on("value"), category: on("categoryName") && !tree, series: on("seriesName"), percent: false, cells: false };
    } else if ser.get("dataLabels").is_some() {
        chart.labels.value = true;
    }
    chart.show_title = ch.get("title").is_some();
    chart.title = ch.get("title").and_then(|t| tx(Some(t))).map(|r| r.cache.iter().map(|v| if let Value::Text(t) = v { t.to_string() } else { String::new() }).collect()).unwrap_or_default();
    chart.legend = match ch.get("legend").map(|l| l.attr("pos").unwrap_or("t")) {
        None => LegendPos::None,
        Some("l") => LegendPos::Left,
        Some("r") => LegendPos::Right,
        Some("b") => LegendPos::Bottom,
        Some(_) => LegendPos::Top,
    };
    for ax in ch.path(&["plotArea"]).map(|p| p.all("axis").collect::<Vec<_>>()).unwrap_or_default() {
        // The percent axis of a Pareto chart is fixed.
        if ax.get("units").is_some() {
            continue;
        }
        let a = if ax.get("valScaling").is_some() { &mut chart.y_axis } else { &mut chart.x_axis };
        a.visible = !ax.attr("hidden").is_some_and(|v| v == "1" || v == "true");
        a.major_grid = ax.get("majorGridlines").is_some();
        a.minor_grid = ax.get("minorGridlines").is_some();
        if let Some(t) = ax.get("title").and_then(|t| tx(Some(t))) {
            a.title = t.cache.iter().map(|v| if let Value::Text(t) = v { t.to_string() } else { String::new() }).collect();
        }
        if let Some(sc) = ax.get("valScaling") {
            let n = |k: &str| sc.attr(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
            a.min = n("min");
            a.max = n("max");
            a.major = n("majorUnit");
            a.minor = n("minorUnit");
        }
        if let Some(g) = ax.get("catScaling").and_then(|c| c.attr("gapWidth")).and_then(|v| v.parse::<f64>().ok()) {
            chart.gap = (g * 100.0).clamp(0.0, 500.0) as u16;
        }
    }
    chart.sanitize();
    Some(chart)
}

/// Excel refers to the data of the newer charts through defined names
/// (`_xlchart.v1.0`); they become the references they name.
pub fn resolve_names(wb: &mut Workbook, names: &[(String, String)]) {
    if names.is_empty() {
        return;
    }
    // The first of names alike.
    let mut by_name: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    for (n, t) in names {
        by_name.entry(n.to_ascii_lowercase()).or_insert(t.as_str());
    }
    for sh in &mut wb.sheets {
        for r in sh.charts.iter_mut().flat_map(|c| c.series.iter_mut()).flat_map(|s| [&mut s.name, &mut s.categories, &mut s.values]) {
            let Some(text) = r.to_canonical() else { continue };
            if let Some(target) = by_name.get(&text.to_ascii_lowercase()) {
                let cache = std::mem::take(&mut r.cache);
                *r = DataRef::parse(target);
                r.cache = cache;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Writing charts

const C_NS: &str = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
const AX_CAT: u32 = 500_001;
const AX_VAL: u32 = 500_002;
const AX_CAT2: u32 = 500_003;
const AX_VAL2: u32 = 500_004;
/// The series (depth) axis of 3-D line, area and surface charts.
const AX_SER: u32 = 500_005;

fn num(v: f64) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

/// The formula of a data reference with the sheet named: Excel wants every
/// chart reference qualified.
fn qualified(r: &DataRef, own: &str) -> Option<String> {
    let f = r.formula.as_ref()?;
    let mut e = (**f).clone();
    e.visit_refs_mut(&mut |x| {
        if let Expr::Ref { sheet, .. } | Expr::Range { sheet, .. } | Expr::SpillRef { sheet, .. } = x {
            if sheet.is_none() {
                *sheet = Some(Arc::from(own));
            }
        }
    });
    Some(e.to_xlsx())
}

fn str_pts(x: &mut String, v: &[String]) {
    let _ = write!(x, r#"<c:ptCount val="{}"/>"#, v.len());
    for (i, t) in v.iter().enumerate() {
        let _ = write!(x, r#"<c:pt idx="{i}"><c:v>{}</c:v></c:pt>"#, esc(t));
    }
}

fn num_pts(x: &mut String, v: &[Option<f64>], fmt: &str) {
    let _ = write!(x, r#"<c:formatCode>{}</c:formatCode><c:ptCount val="{}"/>"#, esc(fmt), v.len());
    for (i, n) in v.iter().enumerate() {
        if let Some(n) = n {
            let _ = write!(x, r#"<c:pt idx="{i}"><c:v>{}</c:v></c:pt>"#, num(*n));
        }
    }
}

/// A string source (`c:tx`, `c:cat`) with `f` and the shown texts.
fn str_source(x: &mut String, tag: &str, f: Option<String>, texts: &[String]) {
    let _ = write!(x, "<c:{tag}>");
    match f {
        Some(f) => {
            let _ = write!(x, "<c:strRef><c:f>{}</c:f><c:strCache>", esc(&f));
            str_pts(x, texts);
            x.push_str("</c:strCache></c:strRef>");
        }
        None => {
            x.push_str("<c:strLit>");
            str_pts(x, texts);
            x.push_str("</c:strLit>");
        }
    }
    let _ = write!(x, "</c:{tag}>");
}

fn num_source(x: &mut String, tag: &str, f: Option<String>, v: &[Option<f64>], fmt: &str) {
    let _ = write!(x, "<c:{tag}>");
    match f {
        Some(f) => {
            let _ = write!(x, "<c:numRef><c:f>{}</c:f><c:numCache>", esc(&f));
            num_pts(x, v, fmt);
            x.push_str("</c:numCache></c:numRef>");
        }
        None => {
            x.push_str("<c:numLit>");
            num_pts(x, v, fmt);
            x.push_str("</c:numLit>");
        }
    }
    let _ = write!(x, "</c:{tag}>");
}

fn solid(c: u32) -> String {
    format!(r#"<a:solidFill><a:srgbClr val="{c:06X}"/></a:solidFill>"#)
}

/// A gradient or pattern fill.
fn fancy_xml(f: &Fill) -> String {
    match f {
        Fill::Gradient { stops, angle, radial } => {
            let mut x = String::from(r#"<a:gradFill rotWithShape="1"><a:gsLst>"#);
            for (pos, c) in stops {
                let _ = write!(x, r#"<a:gs pos="{}"><a:srgbClr val="{c:06X}"/></a:gs>"#, (pos * 100_000.0).round() as i64);
            }
            x.push_str("</a:gsLst>");
            if *radial {
                x.push_str(r#"<a:path path="circle"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path>"#);
            } else {
                let _ = write!(x, r#"<a:lin ang="{}" scaled="0"/>"#, (angle.rem_euclid(360.0) * 60_000.0).round() as i64);
            }
            x.push_str("</a:gradFill>");
            x
        }
        Fill::Pattern { preset, fg, bg } => format!(r#"<a:pattFill prst="{}"><a:fgClr><a:srgbClr val="{fg:06X}"/></a:fgClr><a:bgClr><a:srgbClr val="{bg:06X}"/></a:bgClr></a:pattFill>"#, esc(preset)),
    }
}

/// `c:spPr` (and for the chart area `c:txPr`) of a chart or plot area.
fn box_xml(x: &mut String, b: &BoxFormat, text: bool) {
    let fill = match (&b.fancy, b.fill, b.no_fill) {
        (Some(f), _, _) => Some(fancy_xml(f)),
        (_, Some(c), _) => Some(solid(c)),
        (_, _, true) => Some("<a:noFill/>".to_string()),
        _ => None,
    };
    let line = match (b.border, b.no_border) {
        (Some(c), _) => Some(format!(r#"<a:ln w="9525">{}</a:ln>"#, solid(c))),
        (_, true) => Some("<a:ln><a:noFill/></a:ln>".to_string()),
        _ => None,
    };
    if fill.is_some() || line.is_some() {
        let _ = write!(x, "<c:spPr>{}{}</c:spPr>", fill.unwrap_or_default(), line.unwrap_or_default());
    }
    if text && (b.font_size.is_some() || b.bold.is_some() || b.text_color.is_some() || b.font.is_some()) {
        x.push_str("<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr");
        if let Some(sz) = b.font_size {
            let _ = write!(x, r#" sz="{}""#, (sz * 100.0).round() as i64);
        }
        if let Some(bold) = b.bold {
            let _ = write!(x, r#" b="{}""#, bold as u8);
        }
        x.push('>');
        if let Some(c) = b.text_color {
            x.push_str(&solid(c));
        }
        if let Some(f) = &b.font {
            let _ = write!(x, r#"<a:latin typeface="{}"/>"#, esc(f));
        }
        x.push_str(r#"</a:defRPr></a:pPr><a:endParaRPr lang="en-US"/></a:p></c:txPr>"#);
    }
}

fn rich(x: &mut String, text: &str, vertical: bool) {
    x.push_str("<c:tx><c:rich>");
    x.push_str(if vertical { r#"<a:bodyPr rot="-5400000" vert="horz"/>"# } else { "<a:bodyPr/>" });
    x.push_str("<a:lstStyle/>");
    for line in text.split('\n') {
        let _ = write!(x, "<a:p><a:r><a:t>{}</a:t></a:r></a:p>", esc(line));
    }
    x.push_str("</c:rich></c:tx>");
}

fn title_xml(x: &mut String, text: &str, vertical: bool) {
    x.push_str("<c:title>");
    if !text.is_empty() {
        rich(x, text, vertical);
    }
    x.push_str(r#"<c:overlay val="0"/></c:title>"#);
}

const C15: &str = "http://schemas.microsoft.com/office/drawing/2012/chart";

fn labels_xml(x: &mut String, l: &Labels) {
    if !l.any() {
        return;
    }
    let b = |v: bool| v as u8;
    let _ = write!(
        x,
        r#"<c:dLbls><c:showLegendKey val="0"/><c:showVal val="{}"/><c:showCatName val="{}"/><c:showSerName val="{}"/><c:showPercent val="{}"/><c:showBubbleSize val="0"/>"#,
        b(l.value),
        b(l.category),
        b(l.series),
        b(l.percent)
    );
    if l.cells {
        let _ = write!(x, r#"<c:extLst><c:ext uri="{{CE6537A1-D6FC-4f65-9D91-7224C49458BB}}" xmlns:c15="{C15}"><c15:showDataLabelsRange val="1"/></c:ext></c:extLst>"#);
    }
    x.push_str("</c:dLbls>");
}

fn error_bars_xml(x: &mut String, e: &ErrorBars, d: &apex_sheet::chart::SeriesData, own: &str, xy: bool) {
    x.push_str("<c:errBars>");
    if xy {
        x.push_str(r#"<c:errDir val="y"/>"#);
    }
    let dir = match e.dir {
        ErrorDir::Both => "both",
        ErrorDir::Plus => "plus",
        ErrorDir::Minus => "minus",
    };
    let (ty, v) = match &e.amount {
        ErrorAmount::Fixed(v) => ("fixedVal", Some(*v)),
        ErrorAmount::Percent(v) => ("percentage", Some(*v)),
        ErrorAmount::StdDev(v) => ("stdDev", Some(*v)),
        ErrorAmount::StdErr => ("stdErr", None),
        ErrorAmount::Custom { .. } => ("cust", None),
    };
    let _ = write!(x, r#"<c:errBarType val="{dir}"/><c:errValType val="{ty}"/><c:noEndCap val="{}"/>"#, !e.cap as u8);
    if let ErrorAmount::Custom { plus, minus } = &e.amount {
        num_source(x, "plus", qualified(plus, own), &d.err_plus, "General");
        num_source(x, "minus", qualified(minus, own), &d.err_minus, "General");
    }
    if let Some(v) = v {
        let _ = write!(x, r#"<c:val val="{}"/>"#, num(v));
    }
    x.push_str(r#"<c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="595959"/></a:solidFill></a:ln></c:spPr></c:errBars>"#);
}

fn marker_xml(x: &mut String, m: Marker, shown: bool, color: u32) {
    let sym = match m {
        _ if !shown => "none",
        Marker::Auto => return,
        Marker::None => "none",
        Marker::Circle => "circle",
        Marker::Square => "square",
        Marker::Diamond => "diamond",
        Marker::Triangle => "triangle",
    };
    if sym == "none" {
        x.push_str(r#"<c:marker><c:symbol val="none"/></c:marker>"#);
    } else {
        let _ = write!(x, r#"<c:marker><c:symbol val="{sym}"/><c:size val="5"/><c:spPr>{}<a:ln>{}</a:ln></c:spPr></c:marker>"#, solid(color), solid(color));
    }
}

fn trend_xml(x: &mut String, t: &Trendline, color: u32) {
    let _ = write!(x, r#"<c:trendline><c:spPr><a:ln w="19050">{}<a:prstDash val="sysDot"/></a:ln></c:spPr>"#, solid(color));
    match t.kind {
        TrendKind::Linear => x.push_str(r#"<c:trendlineType val="linear"/>"#),
        TrendKind::Exponential => x.push_str(r#"<c:trendlineType val="exp"/>"#),
        TrendKind::Logarithmic => x.push_str(r#"<c:trendlineType val="log"/>"#),
        TrendKind::Power => x.push_str(r#"<c:trendlineType val="power"/>"#),
        TrendKind::Polynomial(n) => {
            let _ = write!(x, r#"<c:trendlineType val="poly"/><c:order val="{}"/>"#, n.clamp(2, 6));
        }
        TrendKind::MovingAverage(n) => {
            let _ = write!(x, r#"<c:trendlineType val="movingAvg"/><c:period val="{}"/>"#, n.max(2));
        }
    }
    let _ = write!(x, r#"<c:dispRSqr val="{}"/><c:dispEq val="{}"/></c:trendline>"#, t.show_r2 as u8, t.show_equation as u8);
}

/// One `c:ser` of a group drawn as `kind`.
fn series_xml(x: &mut String, chart: &Chart, data: &ChartData, own: &str, i: usize, kind: ChartType) {
    let s = &chart.series[i];
    let Some(d) = data.series.get(i) else { return };
    let pal = chart.colors();
    let color = s.color.unwrap_or(pal[i % 10]);
    let _ = write!(x, r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/>"#);
    if let Some(f) = qualified(&s.name, own) {
        str_source(x, "tx", Some(f), std::slice::from_ref(&d.name));
    } else if !s.name.is_empty() {
        let _ = write!(x, "<c:tx><c:v>{}</c:v></c:tx>", esc(&d.name));
    }
    // 3-D lines are ribbons with a fill; stock series hide their lines.
    let lined = (matches!(kind, ChartType::Line | ChartType::Scatter) && !(kind == ChartType::Line && chart.view3d)) || (kind == ChartType::Radar && !chart.filled);
    let width = s.line_width.map(|w| format!(r#" w="{}""#, (w * 12_700.0).round() as i64)).unwrap_or_default();
    x.push_str("<c:spPr>");
    if kind == ChartType::Stock {
        x.push_str(r#"<a:ln w="19050"><a:noFill/></a:ln>"#);
    } else if lined {
        let hide = s.no_line || (kind == ChartType::Scatter && s.kind.is_none() && !chart.lines);
        if hide {
            x.push_str("<a:ln><a:noFill/></a:ln>");
        } else {
            let _ = write!(x, r#"<a:ln{width} cap="rnd">{}<a:round/></a:ln>"#, solid(color));
        }
    } else {
        x.push_str(&match &s.fancy {
            _ if s.no_fill => "<a:noFill/>".to_string(),
            Some(f) => fancy_xml(f),
            None => solid(color),
        });
        if s.no_line {
            x.push_str("<a:ln><a:noFill/></a:ln>");
        } else if let Some(lc) = s.line_color {
            let _ = write!(x, "<a:ln{width}>{}</a:ln>", solid(lc));
        } else if !width.is_empty() {
            let _ = write!(x, "<a:ln{width}/>");
        }
    }
    x.push_str("</c:spPr>");
    if kind.is_round() && s.explode > 0 {
        let _ = write!(x, r#"<c:explosion val="{}"/>"#, s.explode);
    }
    let main = s.kind.is_none() || s.kind == Some(chart.kind);
    match kind {
        ChartType::Line if !chart.view3d => marker_xml(x, s.marker, !main || chart.markers, color),
        ChartType::Scatter => marker_xml(x, s.marker, chart.markers, color),
        ChartType::Radar => marker_xml(x, s.marker, chart.markers && !chart.filled, color),
        ChartType::Stock => x.push_str(r#"<c:marker><c:symbol val="none"/></c:marker>"#),
        ChartType::Column | ChartType::Bar | ChartType::Bubble => x.push_str(r#"<c:invertIfNegative val="0"/>"#),
        _ => {}
    }
    // Colors of single points: every slice of a pie with varied colors.
    let round = kind.is_round();
    let points = if round && chart.vary_colors { d.values.len().min(1000) } else { 0 };
    let mut dpts: Vec<(u32, u32)> = (0..points as u32).map(|p| (p, s.point_colors.iter().find(|(i, _)| *i == p).map_or(pal[p as usize % 10], |c| c.1))).collect();
    for pc in &s.point_colors {
        if !dpts.iter().any(|(i, _)| *i == pc.0) {
            dpts.push(*pc);
        }
    }
    if round {
        for pe in &s.point_explode {
            if !dpts.iter().any(|(i, _)| *i == pe.0) {
                dpts.push((pe.0, s.color.unwrap_or(pal[pe.0 as usize % 10])));
            }
        }
    }
    dpts.sort_by_key(|p| p.0);
    for (p, c) in dpts {
        let _ = write!(x, r#"<c:dPt><c:idx val="{p}"/>"#);
        if matches!(kind, ChartType::Column | ChartType::Bar) {
            x.push_str(r#"<c:invertIfNegative val="0"/>"#);
        }
        if !lined && !matches!(kind, ChartType::Surface | ChartType::Stock | ChartType::Radar | ChartType::Line) {
            x.push_str(r#"<c:bubble3D val="0"/>"#);
        }
        if let Some(e) = s.point_explode.iter().find(|e| e.0 == p).filter(|_| round) {
            let _ = write!(x, r#"<c:explosion val="{}"/>"#, e.1);
        }
        let _ = write!(x, "<c:spPr>{}</c:spPr></c:dPt>", if lined { format!("<a:ln>{}</a:ln>", solid(c)) } else { solid(c) });
    }
    // Labels from cells are the series' own labels.
    let label_f = qualified(&s.label_range, own).filter(|_| chart.labels.cells);
    if label_f.is_some() {
        labels_xml(x, &chart.labels);
    }
    if let (Some(t), false) = (&s.trendline, round) {
        trend_xml(x, t, color);
    }
    if let (Some(e), false) = (&s.error_bars, round || matches!(kind, ChartType::Radar | ChartType::Stock | ChartType::Surface)) {
        error_bars_xml(x, e, d, own, kind.is_xy());
    }
    let cat_f = qualified(&s.categories, own);
    if kind == ChartType::Bubble {
        if !s.categories.is_empty() {
            num_source(x, "xVal", cat_f, &d.xs, &data.cat_format);
        }
        num_source(x, "yVal", qualified(&s.values, own), &d.values, &data.value_format);
        num_source(x, "bubbleSize", qualified(&s.sizes, own), &d.sizes, "General");
        x.push_str(r#"<c:bubble3D val="0"/>"#);
    } else if kind == ChartType::Scatter {
        if !s.categories.is_empty() {
            if d.categories.iter().all(|t| t.is_empty() || !d.xs.is_empty()) && d.xs.len() == d.categories.len() && d.xs.iter().any(|v| v.is_some()) && chart.series_kind(s) == ChartType::Scatter && !s.categories.cache.iter().any(|v| matches!(v, Value::Text(_))) {
                num_source(x, "xVal", cat_f, &d.xs, &data.cat_format);
            } else {
                str_source(x, "xVal", cat_f, &d.categories);
            }
        }
        num_source(x, "yVal", qualified(&s.values, own), &d.values, &data.value_format);
    } else {
        if !s.categories.is_empty() {
            str_source(x, "cat", cat_f, &d.categories);
        }
        num_source(x, "val", qualified(&s.values, own), &d.values, &data.value_format);
    }
    if matches!(kind, ChartType::Line | ChartType::Scatter) {
        let _ = write!(x, r#"<c:smooth val="{}"/>"#, (s.smooth || chart.smooth) as u8);
    }
    if let Some(f) = label_f {
        let _ = write!(x, r#"<c:extLst><c:ext uri="{{02D57815-91ED-43cb-92C2-25804820EDAC}}" xmlns:c15="{C15}"><c15:datalabelsRange><c15:f>{}</c15:f><c15:dlblRangeCache>"#, esc(&f));
        str_pts(x, &d.label_texts);
        x.push_str("</c15:dlblRangeCache></c15:datalabelsRange></c:ext></c:extLst>");
    }
    x.push_str("</c:ser>");
}

fn axis_scaling(x: &mut String, a: &Axis, values: bool) {
    x.push_str("<c:scaling>");
    if let (Some(b), true) = (a.log_base, values) {
        let _ = write!(x, r#"<c:logBase val="{}"/>"#, num(b));
    }
    let _ = write!(x, r#"<c:orientation val="{}"/>"#, if a.reverse { "maxMin" } else { "minMax" });
    if values {
        if let Some(v) = a.max {
            let _ = write!(x, r#"<c:max val="{}"/>"#, num(v));
        }
        if let Some(v) = a.min {
            let _ = write!(x, r#"<c:min val="{}"/>"#, num(v));
        }
    }
    x.push_str("</c:scaling>");
}

#[allow(clippy::too_many_arguments)]
fn axis_xml(x: &mut String, tag: &str, id: u32, cross: u32, a: &Axis, pos: &str, deleted: bool, crosses: &str, linked_fmt: &str, between: Option<&str>) {
    let _ = write!(x, r#"<c:{tag}><c:axId val="{id}"/>"#);
    axis_scaling(x, a, tag == "valAx");
    let _ = write!(x, r#"<c:delete val="{}"/><c:axPos val="{pos}"/>"#, (deleted || !a.visible) as u8);
    // With their lines: LibreOffice hides gridlines without.
    if a.major_grid {
        x.push_str(r#"<c:majorGridlines><c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></a:ln></c:spPr></c:majorGridlines>"#);
    }
    if a.minor_grid {
        x.push_str(r#"<c:minorGridlines><c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="F2F2F2"/></a:solidFill></a:ln></c:spPr></c:minorGridlines>"#);
    }
    if !a.title.is_empty() {
        title_xml(x, &a.title, pos == "l" || pos == "r");
    }
    if a.format.is_empty() {
        let _ = write!(x, r#"<c:numFmt formatCode="{}" sourceLinked="1"/>"#, esc(linked_fmt));
    } else {
        let _ = write!(x, r#"<c:numFmt formatCode="{}" sourceLinked="0"/>"#, esc(&a.format));
    }
    let _ = write!(x, r#"<c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="{cross}"/><c:crosses val="{crosses}"/>"#);
    if tag == "catAx" {
        x.push_str(r#"<c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/>"#);
    } else {
        if let Some(b) = between {
            let _ = write!(x, r#"<c:crossBetween val="{b}"/>"#);
        }
        if let Some(v) = a.major {
            let _ = write!(x, r#"<c:majorUnit val="{}"/>"#, num(v));
        }
        if let Some(v) = a.minor {
            let _ = write!(x, r#"<c:minorUnit val="{}"/>"#, num(v));
        }
    }
    let _ = write!(x, "</c:{tag}>");
}

/// The chart space around a plot area, with extensions `ext`.
fn chart_space(chart: &Chart, plot: &str, title: &str, ext: &str) -> String {
    let mut x = String::from(HEADER);
    let _ = write!(x, r#"<c:chartSpace {C_NS}><c:date1904 val="0"/><c:roundedCorners val="0"/><c:style val="{}"/><c:chart>"#, chart.style as u32 * 8 + 2);
    if chart.show_title {
        title_xml(&mut x, title, false);
    }
    let _ = write!(x, r#"<c:autoTitleDeleted val="{}"/>"#, (!chart.show_title) as u8);
    if chart.view3d || chart.kind == ChartType::Surface {
        let (rx, ry, right) = match chart.kind {
            ChartType::Surface if !chart.view3d => (90, 0, 0),
            ChartType::Pie => (30, chart.first_angle, 0),
            ChartType::Surface => (15, 20, 0),
            _ => (15, 20, 1),
        };
        let _ = write!(x, r#"<c:view3D><c:rotX val="{rx}"/><c:rotY val="{ry}"/><c:rAngAx val="{right}"/>{}</c:view3D>"#, if rx == 90 { r#"<c:perspective val="0"/>"# } else { "" });
    }
    x.push_str(plot);
    let pos = match chart.legend {
        LegendPos::None => None,
        LegendPos::Right => Some("r"),
        LegendPos::Left => Some("l"),
        LegendPos::Top => Some("t"),
        LegendPos::Bottom => Some("b"),
    };
    if let Some(p) = pos {
        let _ = write!(x, r#"<c:legend><c:legendPos val="{p}"/>"#);
        if chart.kind == ChartType::Waterfall {
            // The emulation's invisible base.
            x.push_str(r#"<c:legendEntry><c:idx val="0"/><c:delete val="1"/></c:legendEntry>"#);
        }
        x.push_str(r#"<c:overlay val="0"/></c:legend>"#);
    }
    let blanks = match chart.blanks {
        Blanks::Gap => "gap",
        Blanks::Zero => "zero",
        Blanks::Span => "span",
    };
    let _ = write!(x, r#"<c:plotVisOnly val="{}"/><c:dispBlanksAs val="{blanks}"/></c:chart>"#, chart.visible_only as u8);
    box_xml(&mut x, &chart.area, true);
    let _ = write!(x, "{ext}</c:chartSpace>");
    x
}

/// The `chartN.xml` part of a chart. Waterfall and histogram charts are
/// emulated with columns (for readers without the newer charts), with
/// what they are in an extension of ours.
pub fn write_chart(wb: &Workbook, sheet: usize, chart: &Chart) -> String {
    let data = wb.chart_data(sheet, chart);
    let own = wb.sheets[sheet].name.as_str();
    let mut p = String::from("<c:plotArea><c:layout/>");
    let mut ext = String::new();
    match chart.kind {
        k if k.is_chartex() || k.is_apex_only() => {
            emulated(&mut p, chart, &data);
            emulation_ext(&mut ext, chart, own);
        }
        _ => plot_xml(&mut p, chart, &data, own),
    }
    if chart.data_table && !chart.kind.is_chartex() && !chart.kind.is_apex_only() {
        let _ = write!(p, r#"<c:dTable><c:showHorzBorder val="1"/><c:showVertBorder val="1"/><c:showOutline val="1"/><c:showKeys val="{}"/></c:dTable>"#, chart.table_keys as u8);
    }
    box_xml(&mut p, &chart.plot, false);
    p.push_str("</c:plotArea>");
    chart_space(chart, &p, &chart.title, &ext)
}

const APEX_EXT: &str = "{3C5E1B0A-7A0D-4E57-9A7C-41504558434B}";

/// What an emulated chart is: `<c:extLst>` with an `apex:chart` element.
fn emulation_ext(x: &mut String, chart: &Chart, own: &str) {
    let _ = write!(x, r#"<c:extLst><c:ext uri="{APEX_EXT}" xmlns:apex="urn:apexdesk:chart"><apex:chart kind="{}""#, chart.kind.key());
    if let Some(s) = chart.series.first() {
        for (k, r) in [("name", &s.name), ("categories", &s.categories), ("values", &s.values)] {
            if let Some(f) = qualified(r, own) {
                let _ = write!(x, r#" {k}="{}""#, esc(&f));
            } else if let (true, Some(Value::Text(t))) = (k == "name", r.cache.first()) {
                let _ = write!(x, r#" nameText="{}""#, esc(t));
            }
        }
        if let Some(c) = s.color {
            let _ = write!(x, r#" color="{c:06X}""#);
        }
    }
    if !chart.totals.is_empty() {
        let t: Vec<String> = chart.totals.iter().map(|t| t.to_string()).collect();
        let _ = write!(x, r#" totals="{}""#, t.join(" "));
    }
    match chart.bins {
        Bins::Auto => {}
        Bins::Width(w) => {
            let _ = write!(x, r#" binWidth="{}""#, num(w));
        }
        Bins::Count(n) => {
            let _ = write!(x, r#" binCount="{n}""#);
        }
    }
    if !chart.show_mean {
        x.push_str(r#" showMean="0""#);
    }
    if !chart.show_outliers {
        x.push_str(r#" showOutliers="0""#);
    }
    x.push('>');
    // The other series (box and whisker charts have several).
    for s in chart.series.iter().skip(1) {
        x.push_str("<apex:ser");
        for (k, r) in [("name", &s.name), ("categories", &s.categories), ("values", &s.values)] {
            if let Some(f) = qualified(r, own) {
                let _ = write!(x, r#" {k}="{}""#, esc(&f));
            }
        }
        if let Some(c) = s.color {
            let _ = write!(x, r#" color="{c:06X}""#);
        }
        x.push_str("/>");
    }
    x.push_str("</apex:chart></c:ext></c:extLst>");
}

/// Makes a chart read from an emulation what it emulates.
fn apply_emulation(chart: &mut Chart, e: &Node) {
    let Some(kind) = e.attr("kind").and_then(|k| ChartType::ALL.into_iter().find(|t| t.key() == k && (t.is_chartex() || t.is_apex_only()))) else { return };
    let r = |k: &str| e.attr(k).map(DataRef::parse).unwrap_or_default();
    let mut s = Series { name: r("name"), categories: r("categories"), values: r("values"), ..Default::default() };
    if s.name.is_empty() {
        if let Some(t) = e.attr("nameText") {
            s.name = DataRef::text(t);
        }
    }
    s.color = e.attr("color").and_then(hex);
    chart.kind = kind;
    chart.grouping = Grouping::Clustered;
    (chart.markers, chart.lines, chart.smooth) = (false, false, false);
    chart.overlap = 0;
    chart.palette = 0;
    chart.vary_colors = false;
    chart.series = vec![s];
    for n in e.all("ser").take(MAX_SERIES) {
        let r = |k: &str| n.attr(k).map(DataRef::parse).unwrap_or_default();
        chart.series.push(Series { name: r("name"), categories: r("categories"), values: r("values"), color: n.attr("color").and_then(hex), ..Default::default() });
    }
    chart.show_mean = e.attr("showMean") != Some("0");
    chart.show_outliers = e.attr("showOutliers") != Some("0");
    chart.totals = e.attr("totals").map(|t| t.split_whitespace().filter_map(|v| v.parse::<u32>().ok()).filter(|v| (*v as usize) < MAX_POINTS).collect()).unwrap_or_default();
    let n = |k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
    chart.bins = match (n("binWidth"), n("binCount")) {
        (Some(w), _) => Bins::Width(w),
        (None, Some(c)) => Bins::Count(c as u32),
        _ => Bins::Auto,
    };
}

fn plot_xml(p: &mut String, chart: &Chart, data: &ChartData, own: &str) {
    // Chart groups: (kind, secondary, series), the main one first.
    let mut groups: Vec<(ChartType, bool, Vec<usize>)> = vec![(chart.kind, false, Vec::new())];
    let two_axes = chart.has_secondary() && chart.kind.can_stack();
    for (i, s) in chart.series.iter().enumerate() {
        let k = if chart.kind.is_round() || chart.kind.is_xy() || !chart.kind.can_stack() { chart.kind } else { chart.series_kind(s) };
        let sec = two_axes && s.secondary;
        match groups.iter_mut().find(|g| g.0 == k && g.1 == sec) {
            Some(g) => g.2.push(i),
            None => groups.push((k, sec, vec![i])),
        }
    }
    if groups[0].2.is_empty() && groups.len() > 1 {
        groups.remove(0);
    }
    let bar = chart.kind == ChartType::Bar;
    for (k, sec, list) in &groups {
        let (cat, val) = if *sec { (AX_CAT2, AX_VAL2) } else { (AX_CAT, AX_VAL) };
        let grouping = if *k == chart.kind { chart.grouping } else { Grouping::Clustered };
        let g = |std: &'static str| match grouping {
            Grouping::Clustered => std,
            Grouping::Stacked => "stacked",
            Grouping::Percent => "percentStacked",
        };
        let vary = chart.vary_colors as u8;
        let d3 = chart.view3d && *k == chart.kind;
        match k {
            ChartType::Column | ChartType::Bar if d3 => {
                let _ = write!(p, r#"<c:bar3DChart><c:barDir val="{}"/><c:grouping val="{}"/><c:varyColors val="{vary}"/>"#, if *k == ChartType::Bar { "bar" } else { "col" }, g("clustered"));
            }
            ChartType::Line if d3 => {
                let _ = write!(p, r#"<c:line3DChart><c:grouping val="{}"/><c:varyColors val="0"/>"#, g("standard"));
            }
            ChartType::Area if d3 => {
                let _ = write!(p, r#"<c:area3DChart><c:grouping val="{}"/><c:varyColors val="0"/>"#, g("standard"));
            }
            ChartType::Pie if d3 => {
                let _ = write!(p, r#"<c:pie3DChart><c:varyColors val="{vary}"/>"#);
            }
            ChartType::Radar => {
                let _ = write!(p, r#"<c:radarChart><c:radarStyle val="{}"/><c:varyColors val="0"/>"#, if chart.filled { "filled" } else { "marker" });
            }
            ChartType::Bubble => p.push_str(r#"<c:bubbleChart><c:varyColors val="0"/>"#),
            ChartType::Stock => p.push_str("<c:stockChart>"),
            ChartType::Surface => {
                let _ = write!(p, r#"<c:{}><c:wireframe val="0"/>"#, if chart.view3d { "surface3DChart" } else { "surfaceChart" });
            }
            ChartType::Column | ChartType::Bar => {
                let _ = write!(p, r#"<c:barChart><c:barDir val="{}"/><c:grouping val="{}"/><c:varyColors val="{vary}"/>"#, if *k == ChartType::Bar { "bar" } else { "col" }, g("clustered"));
            }
            ChartType::Line => {
                let _ = write!(p, r#"<c:lineChart><c:grouping val="{}"/><c:varyColors val="0"/>"#, g("standard"));
            }
            ChartType::Area => {
                let _ = write!(p, r#"<c:areaChart><c:grouping val="{}"/><c:varyColors val="0"/>"#, g("standard"));
            }
            ChartType::Pie => {
                let _ = write!(p, r#"<c:pieChart><c:varyColors val="{vary}"/>"#);
            }
            ChartType::Doughnut => {
                let _ = write!(p, r#"<c:doughnutChart><c:varyColors val="{vary}"/>"#);
            }
            _ => {
                let _ = write!(p, r#"<c:scatterChart><c:scatterStyle val="{}"/><c:varyColors val="0"/>"#, if chart.smooth { "smoothMarker" } else { "lineMarker" });
            }
        }
        for &i in list {
            series_xml(p, chart, data, own, i, *k);
        }
        labels_xml(p, &chart.labels);
        match k {
            ChartType::Column | ChartType::Bar if d3 => {
                let _ = write!(p, r#"<c:gapWidth val="{}"/><c:shape val="box"/><c:axId val="{cat}"/><c:axId val="{val}"/><c:axId val="0"/></c:bar3DChart>"#, chart.gap);
            }
            ChartType::Line if d3 => {
                let _ = write!(p, r#"<c:axId val="{cat}"/><c:axId val="{val}"/><c:axId val="{AX_SER}"/></c:line3DChart>"#);
            }
            ChartType::Area if d3 => {
                let _ = write!(p, r#"<c:axId val="{cat}"/><c:axId val="{val}"/><c:axId val="{AX_SER}"/></c:area3DChart>"#);
            }
            ChartType::Pie if d3 => p.push_str("</c:pie3DChart>"),
            ChartType::Radar => {
                let _ = write!(p, r#"<c:axId val="{cat}"/><c:axId val="{val}"/></c:radarChart>"#);
            }
            ChartType::Bubble => {
                let _ = write!(p, r#"<c:bubbleScale val="{}"/><c:showNegBubbles val="0"/><c:axId val="{cat}"/><c:axId val="{val}"/></c:bubbleChart>"#, chart.bubble_scale);
            }
            ChartType::Stock => {
                p.push_str("<c:hiLowLines/>");
                if list.len() >= 4 {
                    p.push_str(r#"<c:upDownBars><c:gapWidth val="150"/><c:upBars/><c:downBars/></c:upDownBars>"#);
                }
                let _ = write!(p, r#"<c:axId val="{cat}"/><c:axId val="{val}"/></c:stockChart>"#);
            }
            ChartType::Surface => {
                let _ = write!(p, r#"<c:axId val="{cat}"/><c:axId val="{val}"/><c:axId val="{AX_SER}"/></c:{}>"#, if chart.view3d { "surface3DChart" } else { "surfaceChart" });
            }
            ChartType::Column | ChartType::Bar => {
                let overlap = if grouping == Grouping::Clustered { chart.overlap as i32 } else { 100 };
                let _ = write!(p, r#"<c:gapWidth val="{}"/><c:overlap val="{overlap}"/><c:axId val="{cat}"/><c:axId val="{val}"/></c:barChart>"#, chart.gap);
            }
            ChartType::Line => {
                let _ = write!(p, r#"<c:marker val="1"/><c:axId val="{cat}"/><c:axId val="{val}"/></c:lineChart>"#);
            }
            ChartType::Area => {
                let _ = write!(p, r#"<c:axId val="{cat}"/><c:axId val="{val}"/></c:areaChart>"#);
            }
            ChartType::Pie => {
                let _ = write!(p, r#"<c:firstSliceAng val="{}"/></c:pieChart>"#, chart.first_angle);
            }
            ChartType::Doughnut => {
                let _ = write!(p, r#"<c:firstSliceAng val="{}"/><c:holeSize val="{}"/></c:doughnutChart>"#, chart.first_angle, chart.hole);
            }
            _ => {
                let _ = write!(p, r#"<c:axId val="{cat}"/><c:axId val="{val}"/></c:scatterChart>"#);
            }
        }
    }
    if chart.kind.is_round() {
        return;
    }
    let (cpos, vpos) = if bar { ("l", "b") } else { ("b", "l") };
    let between = if matches!(chart.kind, ChartType::Area) { "midCat" } else { "between" };
    if chart.kind.is_xy() {
        axis_xml(p, "valAx", AX_CAT, AX_VAL, &chart.x_axis, "b", false, "autoZero", &data.cat_format, Some("midCat"));
        axis_xml(p, "valAx", AX_VAL, AX_CAT, &chart.y_axis, "l", false, "autoZero", &data.value_format, Some("midCat"));
        return;
    }
    axis_xml(p, "catAx", AX_CAT, AX_VAL, &chart.x_axis, cpos, false, "autoZero", &data.cat_format, None);
    axis_xml(p, "valAx", AX_VAL, AX_CAT, &chart.y_axis, vpos, false, "autoZero", &data.value_format, Some(between));
    let depth = chart.kind == ChartType::Surface || (chart.view3d && matches!(chart.kind, ChartType::Line | ChartType::Area));
    if depth {
        let _ = write!(p, r#"<c:serAx><c:axId val="{AX_SER}"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="{}"/><c:axPos val="b"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="{AX_VAL}"/><c:crosses val="autoZero"/></c:serAx>"#, (chart.kind != ChartType::Surface) as u8);
    }
    if groups.iter().any(|g| g.1) {
        let (cpos2, vpos2) = if bar { ("r", "t") } else { ("t", "r") };
        axis_xml(p, "valAx", AX_VAL2, AX_CAT2, &chart.y2_axis, vpos2, false, "max", &data.value_format, Some(between));
        axis_xml(p, "catAx", AX_CAT2, AX_VAL2, &Axis::default(), cpos2, true, "autoZero", &data.cat_format, None);
    }
}

/// Short text of a number for bin labels.
fn short(v: f64) -> String {
    let s = format!("{:.4}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Waterfall as stacked columns over an invisible base, histogram as
/// columns of the bin counts; the data are literal.
fn emulated(p: &mut String, chart: &Chart, data: &ChartData) {
    let pal = chart.colors();
    let Some(d) = data.series.first() else { return };
    match chart.kind {
        ChartType::Mekko => return emulated_mekko(p, chart, data),
        ChartType::Timeline => return emulated_timeline(p, chart, data),
        _ => {}
    }
    if !matches!(chart.kind, ChartType::Waterfall | ChartType::Histogram) {
        return emulated_plain(p, chart, data);
    }
    let (cats, series): (Vec<String>, Vec<(String, Vec<Option<f64>>, Vec<(u32, u32)>, bool)>) = if chart.kind == ChartType::Histogram {
        let vals: Vec<f64> = data.series.iter().flat_map(|s| s.values.iter().flatten().copied()).collect();
        let bins = apex_sheet::chart::histogram(&vals, chart.bins);
        let cats = bins.iter().enumerate().map(|(i, (a, b, _))| format!("{}{}, {}]", if i == 0 { "[" } else { "(" }, short(*a), short(*b))).collect();
        (cats, vec![(d.name.clone(), bins.iter().map(|b| Some(b.2 as f64)).collect(), Vec::new(), true)])
    } else {
        let (mut base, mut delta, mut colors) = (Vec::new(), Vec::new(), Vec::new());
        let mut run = 0.0;
        for (i, v) in d.values.iter().enumerate() {
            let v = v.unwrap_or(0.0);
            let total = chart.totals.contains(&(i as u32));
            let (a, b) = if total { (0.0, v) } else { (run, run + v) };
            run = b;
            base.push(Some(a.min(b)));
            delta.push(Some((b - a).abs()));
            colors.push((i as u32, if total { pal[2] } else if v < 0.0 { pal[1] } else { pal[0] }));
        }
        (data.categories.clone(), vec![(String::new(), base, Vec::new(), false), (d.name.clone(), delta, colors, true)])
    };
    let stacked = series.len() > 1;
    let _ = write!(p, r#"<c:barChart><c:barDir val="col"/><c:grouping val="{}"/><c:varyColors val="0"/>"#, if stacked { "stacked" } else { "clustered" });
    for (i, (name, vals, colors, shown)) in series.iter().enumerate() {
        let _ = write!(p, r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>{}</c:v></c:tx><c:spPr>{}</c:spPr><c:invertIfNegative val="0"/>"#, esc(name), if *shown { solid(chart.series[0].color.unwrap_or(pal[0])) } else { "<a:noFill/>".into() });
        for (pi, c) in colors {
            let _ = write!(p, r#"<c:dPt><c:idx val="{pi}"/><c:invertIfNegative val="0"/><c:bubble3D val="0"/><c:spPr>{}</c:spPr></c:dPt>"#, solid(*c));
        }
        if *shown {
            labels_xml(p, &chart.labels);
        }
        str_source(p, "cat", None, &cats);
        num_source(p, "val", None, vals, "General");
        p.push_str("</c:ser>");
    }
    let _ = write!(p, r#"<c:gapWidth val="{}"/>"#, chart.gap);
    if stacked {
        p.push_str(r#"<c:overlap val="100"/>"#);
    }
    let _ = write!(p, r#"<c:axId val="{AX_CAT}"/><c:axId val="{AX_VAL}"/></c:barChart>"#);
    axis_xml(p, "catAx", AX_CAT, AX_VAL, &chart.x_axis, "b", false, "autoZero", "General", None);
    axis_xml(p, "valAx", AX_VAL, AX_CAT, &chart.y_axis, "l", false, "autoZero", &data.value_format, Some("between"));
}

/// A Marimekko chart as 100% stacked columns of its segments (the widths
/// are only in our extension).
fn emulated_mekko(p: &mut String, chart: &Chart, data: &ChartData) {
    let pal = chart.colors();
    let skip = usize::from(data.series.len() > 1);
    p.push_str(r#"<c:barChart><c:barDir val="col"/><c:grouping val="percentStacked"/><c:varyColors val="0"/>"#);
    for (i, d) in data.series.iter().enumerate().skip(skip) {
        let color = chart.series.get(i).and_then(|s| s.color).unwrap_or(pal[i % 10]);
        let _ = write!(p, r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>{}</c:v></c:tx><c:spPr>{}</c:spPr><c:invertIfNegative val="0"/>"#, esc(&d.name), solid(color));
        str_source(p, "cat", None, &data.categories);
        num_source(p, "val", None, &d.values, "General");
        p.push_str("</c:ser>");
    }
    let _ = write!(p, r#"<c:gapWidth val="{}"/><c:overlap val="100"/><c:axId val="{AX_CAT}"/><c:axId val="{AX_VAL}"/></c:barChart>"#, chart.gap);
    axis_xml(p, "catAx", AX_CAT, AX_VAL, &chart.x_axis, "b", false, "autoZero", "General", None);
    axis_xml(p, "valAx", AX_VAL, AX_CAT, &chart.y_axis, "l", false, "autoZero", "0%", Some("between"));
}

/// A timeline as a scatter chart of the dates on a line (the labels are
/// only in our extension).
fn emulated_timeline(p: &mut String, chart: &Chart, data: &ChartData) {
    let pal = chart.colors();
    p.push_str(r#"<c:scatterChart><c:scatterStyle val="lineMarker"/><c:varyColors val="0"/>"#);
    if let Some(d) = data.series.first() {
        let color = chart.series.first().and_then(|s| s.color).unwrap_or(pal[0]);
        let _ = write!(p, r#"<c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>{}</c:v></c:tx><c:spPr><a:ln w="19050"><a:noFill/></a:ln></c:spPr>"#, esc(&d.name));
        marker_xml(p, Marker::Circle, true, color);
        num_source(p, "xVal", None, &d.values, &data.value_format);
        num_source(p, "yVal", None, &vec![Some(0.0); d.values.len()], "General");
        p.push_str("</c:ser>");
    }
    let _ = write!(p, r#"<c:axId val="{AX_CAT}"/><c:axId val="{AX_VAL}"/></c:scatterChart>"#);
    axis_xml(p, "valAx", AX_CAT, AX_VAL, &chart.x_axis, "b", false, "autoZero", &data.value_format, Some("midCat"));
    axis_xml(p, "valAx", AX_VAL, AX_CAT, &chart.y_axis, "l", false, "autoZero", "General", Some("midCat"));
}

/// The newer charts other than waterfall and histogram as plain columns
/// (bars for a funnel) of their values; the data are literal.
fn emulated_plain(p: &mut String, chart: &Chart, data: &ChartData) {
    let pal = chart.colors();
    let bar = chart.kind == ChartType::Funnel;
    let count = if chart.kind == ChartType::BoxWhisker { data.series.len() } else { data.series.len().min(1) };
    let _ = write!(p, r#"<c:barChart><c:barDir val="{}"/><c:grouping val="clustered"/><c:varyColors val="0"/>"#, if bar { "bar" } else { "col" });
    for (i, d) in data.series.iter().enumerate().take(count) {
        let color = chart.series.get(i).and_then(|s| s.color).unwrap_or(pal[i % 10]);
        let _ = write!(p, r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>{}</c:v></c:tx><c:spPr>{}</c:spPr><c:invertIfNegative val="0"/>"#, esc(&d.name), solid(color));
        labels_xml(p, &chart.labels);
        str_source(p, "cat", None, &data.categories);
        num_source(p, "val", None, &d.values, "General");
        p.push_str("</c:ser>");
    }
    let _ = write!(p, r#"<c:gapWidth val="{}"/><c:axId val="{AX_CAT}"/><c:axId val="{AX_VAL}"/></c:barChart>"#, chart.gap);
    let (cpos, vpos) = if bar { ("l", "b") } else { ("b", "l") };
    let hidden = !matches!(chart.kind, ChartType::Treemap | ChartType::Sunburst);
    axis_xml(p, "catAx", AX_CAT, AX_VAL, &chart.x_axis, cpos, !hidden, "autoZero", "General", None);
    axis_xml(p, "valAx", AX_VAL, AX_CAT, &chart.y_axis, vpos, !hidden || bar, "autoZero", &data.value_format, Some("between"));
}

/// The `chartExN.xml` part of a waterfall or histogram chart.
/// Waterfall and histogram charts as such, for Excel, besides their
/// emulation. Their ranges go through hidden names (`_xlchart.v1.N`, added
/// to `names`) as Excel writes them.
pub fn write_chartex(wb: &Workbook, sheet: usize, chart: &Chart, names: &mut Vec<String>) -> Option<String> {
    let mut x = chartex_xml(wb, sheet, chart)?;
    let own = wb.sheets[sheet].name.as_str();
    for s in chart.series.iter().take(chartex_series(chart)) {
        for r in [&s.categories, &s.values, &s.name] {
            if let Some(f) = qualified(r, own) {
                let from = format!("<cx:f>{}</cx:f>", esc(&f));
                if x.contains(&from) {
                    let to = format!("<cx:f>_xlchart.v1.{}</cx:f>", names.len());
                    x = x.replacen(&from, &to, 1);
                    names.push(f);
                }
            }
        }
    }
    Some(x)
}

/// Series a newer chart writes: all of a box and whisker chart, else one.
fn chartex_series(chart: &Chart) -> usize {
    if chart.kind == ChartType::BoxWhisker { chart.series.len() } else { chart.series.len().min(1) }
}

/// The prefix and namespace the newer chart requires. LibreOffice (26.2)
/// knows cx1, cx2 and cx4 but draws waterfalls and histograms as lines:
/// with cx3 it takes the emulation, and Excel the newer chart.
pub const CHARTEX_REQUIRES: (&str, &str) = ("cx3", "http://schemas.microsoft.com/office/drawing/2016/5/9/chartex");

fn chartex_xml(wb: &Workbook, sheet: usize, chart: &Chart) -> Option<String> {
    if !chart.kind.is_chartex() {
        return None;
    }
    let kind = chart.kind;
    let data = wb.chart_data(sheet, chart);
    let own = wb.sheets[sheet].name.as_str();
    let count = chartex_series(chart).min(data.series.len());
    if count == 0 {
        return None;
    }
    let tree = matches!(kind, ChartType::Treemap | ChartType::Sunburst);
    let mut x = String::from(HEADER);
    x.push_str(r#"<cx:chartSpace xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData>"#);
    for i in 0..count {
        let (s, d) = (&chart.series[i], &data.series[i]);
        let _ = write!(x, r#"<cx:data id="{i}">"#);
        if kind != ChartType::Histogram && !s.categories.is_empty() {
            x.push_str(r#"<cx:strDim type="cat">"#);
            if let Some(f) = qualified(&s.categories, own) {
                let _ = write!(x, "<cx:f>{}</cx:f>", esc(&f));
            }
            // Levels of a hierarchy go from the leaves out.
            let depth = if tree { data.levels.iter().map(|l| l.len()).max().unwrap_or(1).max(1) } else { 1 };
            for level in (0..depth).rev() {
                let n = d.values.len();
                let _ = write!(x, r#"<cx:lvl ptCount="{n}">"#);
                for j in 0..n {
                    let t = if tree { data.levels.get(j).and_then(|l| l.get(level)).cloned().unwrap_or_default() } else { d.categories.get(j).or(data.categories.get(j)).cloned().unwrap_or_default() };
                    let _ = write!(x, r#"<cx:pt idx="{j}">{}</cx:pt>"#, esc(&t));
                }
                x.push_str("</cx:lvl>");
            }
            x.push_str("</cx:strDim>");
        }
        let _ = write!(x, r#"<cx:numDim type="{}">"#, if tree { "size" } else { "val" });
        if let Some(f) = qualified(&s.values, own) {
            let _ = write!(x, "<cx:f>{}</cx:f>", esc(&f));
        }
        let _ = write!(x, r#"<cx:lvl ptCount="{}" formatCode="{}">"#, d.values.len(), esc(&data.value_format));
        for (j, v) in d.values.iter().enumerate() {
            if let Some(v) = v {
                let _ = write!(x, r#"<cx:pt idx="{j}">{}</cx:pt>"#, num(*v));
            }
        }
        x.push_str("</cx:lvl></cx:numDim></cx:data>");
    }
    x.push_str("</cx:chartData><cx:chart>");
    let tx = |x: &mut String, f: Option<String>, v: &str| {
        x.push_str("<cx:tx><cx:txData>");
        if let Some(f) = f {
            let _ = write!(x, "<cx:f>{}</cx:f>", esc(&f));
        }
        let _ = write!(x, "<cx:v>{}</cx:v></cx:txData></cx:tx>", esc(v));
    };
    if chart.show_title {
        x.push_str(r#"<cx:title pos="t" align="ctr" overlay="0">"#);
        if !chart.title.is_empty() {
            tx(&mut x, None, &chart.title);
        }
        x.push_str("</cx:title>");
    }
    let layout = match kind {
        ChartType::Waterfall => "waterfall",
        ChartType::Treemap => "treemap",
        ChartType::Sunburst => "sunburst",
        ChartType::BoxWhisker => "boxWhisker",
        ChartType::Funnel => "funnel",
        _ => "clusteredColumn",
    };
    x.push_str("<cx:plotArea><cx:plotAreaRegion>");
    for i in 0..count {
        let (s, d) = (&chart.series[i], &data.series[i]);
        let _ = write!(x, r#"<cx:series layoutId="{layout}" uniqueId="{{6F2A1B3C-0000-4000-8000-0000000{:05}}}" formatIdx="{i}">"#, (chart.id % 1000) * 100 + i as u32 % 100);
        if !s.name.is_empty() {
            tx(&mut x, qualified(&s.name, own), &d.name);
        }
        if let Some(c) = s.color {
            let _ = write!(x, "<cx:spPr>{}</cx:spPr>", solid(c));
        }
        let l = &chart.labels;
        if l.value || l.category || l.series || tree {
            let pos = if tree || kind == ChartType::Funnel { "ctr" } else { "outEnd" };
            let _ = write!(x, r#"<cx:dataLabels pos="{pos}"><cx:visibility seriesName="{}" categoryName="{}" value="{}"/></cx:dataLabels>"#, l.series as u8, (l.category || tree) as u8, l.value as u8);
        }
        let _ = write!(x, r#"<cx:dataId val="{i}"/>"#);
        match kind {
            ChartType::Waterfall => {
                x.push_str("<cx:layoutPr>");
                if !chart.totals.is_empty() {
                    x.push_str("<cx:subtotals>");
                    for t in &chart.totals {
                        let _ = write!(x, r#"<cx:idx val="{t}"/>"#);
                    }
                    x.push_str("</cx:subtotals>");
                }
                x.push_str("</cx:layoutPr>");
            }
            ChartType::Histogram => {
                x.push_str("<cx:layoutPr>");
                match chart.bins {
                    Bins::Width(w) => {
                        let _ = write!(x, r#"<cx:binning intervalClosed="r"><cx:binSize val="{}"/></cx:binning>"#, num(w));
                    }
                    Bins::Count(n) => {
                        let _ = write!(x, r#"<cx:binning intervalClosed="r"><cx:binCount val="{n}"/></cx:binning>"#);
                    }
                    Bins::Auto => x.push_str(r#"<cx:binning intervalClosed="r"/>"#),
                }
                x.push_str("</cx:layoutPr>");
            }
            ChartType::Pareto => x.push_str(r#"<cx:layoutPr><cx:aggregation/></cx:layoutPr><cx:axisId val="0"/><cx:axisId val="1"/>"#),
            ChartType::Treemap => x.push_str(r#"<cx:layoutPr><cx:parentLabelLayout val="banner"/></cx:layoutPr>"#),
            ChartType::BoxWhisker => {
                let _ = write!(x, r#"<cx:layoutPr><cx:visibility meanLine="0" meanMarker="{}" nonoutliers="0" outliers="{}"/><cx:statistics quartileMethod="exclusive"/></cx:layoutPr>"#, chart.show_mean as u8, chart.show_outliers as u8);
            }
            _ => {}
        }
        x.push_str("</cx:series>");
    }
    if kind == ChartType::Pareto {
        let _ = write!(x, r#"<cx:series layoutId="paretoLine" ownerIdx="0" uniqueId="{{6F2A1B3C-0000-4000-8000-0000001{:05}}}" formatIdx="1"><cx:axisId val="2"/></cx:series>"#, chart.id % 100_000);
    }
    x.push_str("</cx:plotAreaRegion>");
    let axis_title = |x: &mut String, a: &Axis| {
        if !a.title.is_empty() {
            x.push_str("<cx:title>");
            tx(x, None, &a.title);
            x.push_str("</cx:title>");
        }
        if a.major_grid {
            x.push_str("<cx:majorGridlines/>");
        }
        if a.minor_grid {
            x.push_str("<cx:minorGridlines/>");
        }
        x.push_str("<cx:tickLabels/>");
    };
    let hidden = |a: &Axis| if a.visible { "" } else { r#" hidden="1""# };
    if !tree {
        let _ = write!(x, r#"<cx:axis id="0"{}><cx:catScaling gapWidth="{}"/>"#, hidden(&chart.x_axis), num(chart.gap as f64 / 100.0));
        axis_title(&mut x, &chart.x_axis);
        x.push_str("</cx:axis>");
    }
    if !tree && kind != ChartType::Funnel {
        let _ = write!(x, r#"<cx:axis id="1"{}><cx:valScaling"#, hidden(&chart.y_axis));
        let y = &chart.y_axis;
        for (k, v) in [("max", y.max), ("min", y.min), ("majorUnit", y.major), ("minorUnit", y.minor)] {
            if let Some(v) = v {
                let _ = write!(x, r#" {k}="{}""#, num(v));
            }
        }
        x.push_str("/>");
        axis_title(&mut x, y);
        x.push_str("</cx:axis>");
    }
    if kind == ChartType::Pareto {
        x.push_str(r#"<cx:axis id="2"><cx:valScaling max="1" min="0"/><cx:units unit="percentage"/><cx:tickLabels/></cx:axis>"#);
    }
    x.push_str("</cx:plotArea>");
    let pos = match chart.legend {
        LegendPos::None => None,
        LegendPos::Right => Some("r"),
        LegendPos::Left => Some("l"),
        LegendPos::Top => Some("t"),
        LegendPos::Bottom => Some("b"),
    };
    if let Some(p) = pos {
        let _ = write!(x, r#"<cx:legend pos="{p}" align="ctr" overlay="0"/>"#);
    }
    x.push_str("</cx:chart></cx:chartSpace>");
    Some(x)
}

// ---------------------------------------------------------------------------
// Sparklines

const SPARK_URI: &str = "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}";

/// Sparkline groups of a worksheet (from its xml), added to `sh`.
pub fn read_sparklines(xml: &str, sh: &mut Sheet) {
    let tail = xml.rfind("</sheetData>").map_or(xml, |i| &xml[i..]);
    if !tail.contains("sparklineGroup") {
        return;
    }
    let mut r = Reader::new(tail);
    loop {
        let (t, empty) = match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            _ => continue,
        };
        if t.local() != "sparklineGroup" {
            continue;
        }
        let n = subtree(&mut r, &t, empty);
        if let Some(g) = spark_group(&n) {
            sh.sparklines.push(g);
        }
    }
}

fn spark_group(n: &Node) -> Option<SparkGroup> {
    let mut g = SparkGroup::default();
    let on = |k: &str| n.attr(k).is_some_and(|v| v == "1" || v == "true");
    g.kind = match n.attr("type") {
        Some("column") => SparkKind::Column,
        Some("stacked") => SparkKind::WinLoss,
        _ => SparkKind::Line,
    };
    g.blanks = match n.attr("displayEmptyCellsAs") {
        Some("gap") => Blanks::Gap,
        Some("span") => Blanks::Span,
        _ => Blanks::Zero,
    };
    g.markers = on("markers");
    g.high = on("high");
    g.low = on("low");
    g.first = on("first");
    g.last = on("last");
    g.negative = on("negative");
    g.show_axis = on("displayXAxis");
    g.right_to_left = on("rightToLeft");
    g.visible_only = !on("displayHidden");
    if let Some(w) = n.attr("lineWeight").and_then(|v| v.parse::<f32>().ok()) {
        g.line_weight = w;
    }
    let manual = |k: &str| n.attr(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    let axis = |ty: Option<&str>, v: Option<f64>| match ty {
        Some("group") => SparkAxis::Group,
        Some("custom") => SparkAxis::Custom(v.unwrap_or(0.0)),
        _ => SparkAxis::Individual,
    };
    g.min = axis(n.attr("minAxisType"), manual("manualMin"));
    g.max = axis(n.attr("maxAxisType"), manual("manualMax"));
    let color = |k: &str, def: u32| -> u32 {
        let Some(c) = n.get(k) else { return def };
        if let Some(v) = c.attr("rgb").and_then(hex) {
            return v;
        }
        match c.attr("theme").and_then(|t| t.parse::<u32>().ok()) {
            Some(0) => 0xFFFFFF,
            Some(1) => 0,
            Some(2) => 0xE7E6E6,
            Some(3) => 0x44546A,
            Some(t @ 4..=9) => palette(0)[t as usize - 4],
            _ => def,
        }
    };
    let d = SparkGroup::default();
    g.color = color("colorSeries", d.color);
    g.negative_color = color("colorNegative", d.negative_color);
    g.marker_color = color("colorMarkers", d.marker_color);
    g.first_color = color("colorFirst", d.first_color);
    g.last_color = color("colorLast", d.last_color);
    g.high_color = color("colorHigh", d.high_color);
    g.low_color = color("colorLow", d.low_color);
    for s in n.get("sparklines").map(|s| s.all("sparkline").collect::<Vec<_>>()).unwrap_or_default() {
        if g.sparklines.len() >= MAX_POINTS {
            break;
        }
        let Some(cell) = s.get("sqref").and_then(|q| CellRef::parse(&q.all_text().replace('$', ""))) else { continue };
        let data = s.get("f").map(|f| DataRef::parse(&f.all_text())).unwrap_or_default();
        g.sparklines.push(Sparkline { cell, data });
    }
    g.sanitize();
    (!g.sparklines.is_empty()).then_some(g)
}

/// The worksheet's `extLst` with its sparkline groups (the last child of
/// `<worksheet>`), if it has any.
pub fn write_sparklines(s: &mut String, sh: &Sheet) {
    if sh.sparklines.iter().all(|g| g.sparklines.is_empty()) {
        return;
    }
    let _ = write!(s, r#"<extLst><ext uri="{SPARK_URI}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:sparklineGroups xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main">"#);
    for g in &sh.sparklines {
        if g.sparklines.is_empty() {
            continue;
        }
        s.push_str("<x14:sparklineGroup");
        for (k, a) in [("manualMax", g.max), ("manualMin", g.min)] {
            if let SparkAxis::Custom(v) = a {
                let _ = write!(s, r#" {k}="{}""#, num(v));
            }
        }
        let _ = write!(s, r#" lineWeight="{}""#, num(g.line_weight as f64));
        match g.kind {
            SparkKind::Line => {}
            SparkKind::Column => s.push_str(r#" type="column""#),
            SparkKind::WinLoss => s.push_str(r#" type="stacked""#),
        }
        let blanks = match g.blanks {
            Blanks::Gap => "gap",
            Blanks::Zero => "zero",
            Blanks::Span => "span",
        };
        let _ = write!(s, r#" displayEmptyCellsAs="{blanks}""#);
        for (k, on) in [("markers", g.markers), ("high", g.high), ("low", g.low), ("first", g.first), ("last", g.last), ("negative", g.negative), ("displayXAxis", g.show_axis), ("displayHidden", !g.visible_only)] {
            if on {
                let _ = write!(s, r#" {k}="1""#);
            }
        }
        for (k, a) in [("minAxisType", g.min), ("maxAxisType", g.max)] {
            match a {
                SparkAxis::Individual => {}
                SparkAxis::Group => {
                    let _ = write!(s, r#" {k}="group""#);
                }
                SparkAxis::Custom(_) => {
                    let _ = write!(s, r#" {k}="custom""#);
                }
            }
        }
        if g.right_to_left {
            s.push_str(r#" rightToLeft="1""#);
        }
        s.push('>');
        for (k, c) in [
            ("colorSeries", g.color),
            ("colorNegative", g.negative_color),
            ("colorAxis", 0),
            ("colorMarkers", g.marker_color),
            ("colorFirst", g.first_color),
            ("colorLast", g.last_color),
            ("colorHigh", g.high_color),
            ("colorLow", g.low_color),
        ] {
            let _ = write!(s, r#"<x14:{k} rgb="FF{:06X}"/>"#, c & 0xFF_FFFF);
        }
        s.push_str("<x14:sparklines>");
        for sp in &g.sparklines {
            s.push_str("<x14:sparkline>");
            if let Some(f) = qualified(&sp.data, &sh.name) {
                let _ = write!(s, "<xm:f>{}</xm:f>", esc(&f));
            }
            let _ = write!(s, "<xm:sqref>{}</xm:sqref></x14:sparkline>", sp.cell.to_a1());
        }
        s.push_str("</x14:sparklines></x14:sparklineGroup>");
    }
    s.push_str("</x14:sparklineGroups></ext></extLst>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::chart::{SparkAxis, SparkKind};
    use apex_sheet::{Op, Range};

    fn book() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::EN;
        let rows = [["", "North", "South", "West"], ["Jan", "10", "4", "7"], ["Feb", "20", "-5", "8"], ["Mar", "15", "=B4/3", "9"], ["Apr", "30", "6", "1"]];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                if !v.is_empty() {
                    wb.set_input(0, CellRef::new(r as u32, c as u32), v);
                }
            }
        }
        wb
    }

    fn all_charts(wb: &Workbook) -> Vec<Chart> {
        let r = Range::parse("A1:D5").unwrap();
        let mut out = Vec::new();
        let mut add = |mut c: Chart, f: &dyn Fn(&mut Chart)| {
            c.from = CellRef::new(6 + out.len() as u32 * 16, 1);
            f(&mut c);
            out.push(c);
        };
        let base = |k| wb.chart_from_range(0, r, k, None);
        add(base(ChartType::Column), &|c| {
            c.title = "Sales".into();
            c.y_axis.title = "Units".into();
            c.x_axis.title = "Month".into();
            c.labels.value = true;
            c.series[1].color = Some(0x112233);
            c.series[0].line_color = Some(0x445566);
            c.series[0].line_width = Some(1.5);
            c.gap = 80;
            c.overlap = -20;
        });
        add(base(ChartType::Column), &|c| {
            c.grouping = Grouping::Stacked;
            c.legend = LegendPos::Bottom;
            c.palette = 3;
        });
        add(base(ChartType::Bar), &|c| {
            c.grouping = Grouping::Percent;
            c.legend = LegendPos::Top;
            c.y_axis.reverse = true;
        });
        add(base(ChartType::Line), &|c| {
            c.markers = true;
            c.series[0].marker = Marker::Diamond;
            c.series[1].trendline = Some(Trendline { kind: TrendKind::Polynomial(3), show_equation: true, show_r2: true });
            c.y_axis.min = Some(-10.0);
            c.y_axis.max = Some(40.0);
            c.y_axis.major = Some(10.0);
            c.y_axis.minor = Some(2.0);
            c.y_axis.minor_grid = true;
            c.y_axis.format = "0.0".into();
            c.blanks = Blanks::Span;
        });
        add(base(ChartType::Area), &|c| {
            c.grouping = Grouping::Stacked;
            c.legend = LegendPos::Left;
            c.show_title = false;
        });
        add(wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Pie, None), &|c| {
            c.first_angle = 90;
            c.labels = Labels { percent: true, category: true, ..Default::default() };
            c.series[0].point_colors = vec![(2, 0xABCDEF)];
            c.title = "Two\nlines".into();
        });
        add(base(ChartType::Doughnut), &|c| {
            c.hole = 60;
            c.style = 3;
        });
        add(wb.chart_from_range(0, Range::parse("B2:D5").unwrap(), ChartType::Scatter, None), &|c| {
            c.lines = true;
            c.smooth = true;
            c.x_axis.log_base = Some(10.0);
            c.x_axis.major_grid = true;
            c.series[0].trendline = Some(Trendline { kind: TrendKind::Linear, show_equation: false, show_r2: false });
        });
        add(wb.chart_from_range(0, Range::parse("B2:D5").unwrap(), ChartType::Scatter, None), &|c| {
            c.lines = false;
            c.markers = true;
        });
        add(base(ChartType::Column), &|c| {
            c.make_combo();
            c.y2_axis.title = "West".into();
            c.y2_axis.max = Some(12.0);
        });
        add(wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Waterfall, None), &|c| {
            c.totals = vec![3];
            c.title = "Bridge".into();
            c.labels.value = true;
        });
        add(wb.chart_from_range(0, Range::parse("B2:B5").unwrap(), ChartType::Histogram, None), &|c| c.bins = Bins::Width(5.0));
        add(wb.chart_from_range(0, Range::parse("B2:B5").unwrap(), ChartType::Histogram, None), &|c| c.bins = Bins::Count(3));
        add(base(ChartType::Pie), &|c| {
            c.series[0].explode = 10;
            c.series[0].point_explode = vec![(2, 30)];
            c.series[0].fancy = Some(Fill::Gradient { stops: vec![(0.0, 0x112233), (1.0, 0xAABBCC)], angle: 45.0, radial: false });
            c.series[0].color = Some(0x112233);
        });
        add(base(ChartType::Column), &|c| {
            c.series[0].fancy = Some(Fill::Pattern { preset: "ltUpDiag".into(), fg: 0x4472C4, bg: 0xFFFFFF });
            c.series[0].color = Some(0x4472C4);
            c.series[1].fancy = Some(Fill::Gradient { stops: vec![(0.0, 0xFF0000), (0.5, 0x00FF00), (1.0, 0x0000FF)], angle: 0.0, radial: true });
            c.series[1].color = Some(0xFF0000);
            c.area = BoxFormat { fill: Some(0xFFF2CC), border: Some(0x7F6000), text_color: Some(0x7F6000), font_size: Some(12.0), font: Some("Georgia".into()), bold: Some(true), ..Default::default() };
            c.plot = BoxFormat { fancy: Some(Fill::Gradient { stops: vec![(0.0, 0xFFFFFF), (1.0, 0xDDEBF7)], angle: 90.0, radial: false }), fill: Some(0xFFFFFF), no_border: true, ..Default::default() };
        });
        // The other types and 3-D variants.
        add(base(ChartType::Column), &|c| {
            c.view3d = true;
            c.grouping = Grouping::Stacked;
        });
        add(base(ChartType::Bar), &|c| c.view3d = true);
        add(base(ChartType::Line), &|c| c.view3d = true);
        add(base(ChartType::Area), &|c| c.view3d = true);
        add(wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Pie, None), &|c| {
            c.view3d = true;
            c.first_angle = 40;
        });
        add(base(ChartType::Radar), &|c| c.markers = true);
        add(base(ChartType::Radar), &|c| c.filled = true);
        add(wb.chart_from_range(0, Range::parse("B2:D5").unwrap(), ChartType::Bubble, None), &|c| c.bubble_scale = 150);
        add(base(ChartType::Stock), &|_| {});
        add(base(ChartType::Surface), &|_| {});
        add(base(ChartType::Surface), &|c| c.view3d = false);
        add(wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Treemap, None), &|_| {});
        add(wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Sunburst, None), &|c| c.labels.value = true);
        add(base(ChartType::BoxWhisker), &|c| c.show_outliers = false);
        add(wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Funnel, None), &|_| {});
        add(wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Pareto, None), &|_| {});
        add(base(ChartType::Mekko), &|_| {});
        add(wb.chart_from_range(0, Range::parse("B1:C5").unwrap(), ChartType::Timeline, None), &|_| {});
        // Error bars, data tables and labels from cells.
        add(base(ChartType::Column), &|c| {
            c.data_table = true;
            c.table_keys = false;
            c.labels.cells = true;
            c.labels.value = true;
            c.series[0].label_range = c.series[0].categories.clone();
            c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::Percent(10.0), ..Default::default() });
            c.series[1].error_bars = Some(ErrorBars { amount: ErrorAmount::Custom { plus: c.series[2].values.clone(), minus: c.series[0].values.clone() }, dir: ErrorDir::Both, cap: true });
        });
        add(base(ChartType::Line), &|c| {
            c.data_table = true;
            c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::StdDev(1.5), dir: ErrorDir::Minus, cap: true });
            c.series[1].error_bars = Some(ErrorBars { amount: ErrorAmount::StdErr, dir: ErrorDir::Both, cap: false });
        });
        add(wb.chart_from_range(0, Range::parse("B1:C5").unwrap(), ChartType::Scatter, None), &|c| {
            c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::Fixed(2.0), dir: ErrorDir::Plus, cap: false });
        });
        out
    }

    /// The chart as read back: placement from the original, caches dropped.
    fn normalized(mut back: Chart, orig: &Chart) -> Chart {
        back.from = orig.from;
        back.dx = orig.dx;
        back.dy = orig.dy;
        back.width = orig.width;
        back.height = orig.height;
        back.id = orig.id;
        back.source = orig.source.clone();
        back.by_rows = orig.by_rows;
        for s in &mut back.series {
            let custom = match &mut s.error_bars {
                Some(ErrorBars { amount: ErrorAmount::Custom { plus, minus }, .. }) => vec![plus, minus],
                _ => Vec::new(),
            };
            for r in [&mut s.name, &mut s.categories, &mut s.values, &mut s.sizes, &mut s.label_range].into_iter().chain(custom) {
                if r.formula.is_some() {
                    r.cache.clear();
                }
            }
        }
        back
    }

    #[test]
    fn charts_roundtrip() {
        let mut wb = book();
        for c in all_charts(&wb) {
            wb.add_chart(0, c);
        }
        let bytes = crate::xlsx::write(&wb);
        let back = crate::xlsx::read(&bytes).unwrap();
        assert_eq!(back.sheets[0].charts.len(), wb.sheets[0].charts.len());
        for (a, b) in wb.sheets[0].charts.iter().zip(&back.sheets[0].charts) {
            let b = normalized(b.clone(), a);
            let (wa, wb2) = (a.width, b.width);
            assert!((wa - wb2).abs() < 1.0);
            assert_eq!(format!("{b:?}"), format!("{a:?}"), "{:?}", a.kind);
        }
        // Placement survives within a pixel.
        let (a, b) = (&wb.sheets[0].charts[3], &back.sheets[0].charts[3]);
        assert_eq!(a.from, b.from);
        assert!((a.width - b.width).abs() < 1.0 && (a.height - b.height).abs() < 1.0, "{} {}", b.width, b.height);
        // Data from the cells: the caches hold the values at save time.
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let ct = z.read_string("[Content_Types].xml").unwrap();
        assert!(ct.contains("drawingml.chart+xml"));
        let x = z.read_string("xl/charts/chart1.xml").unwrap();
        assert!(x.contains("<c:f>Sheet1!$B$2:$B$5</c:f>") && x.contains("<c:v>30</c:v>"), "{x}");
        // Saved again: the same.
        let again = crate::xlsx::read(&crate::xlsx::write(&back)).unwrap();
        for (a, b) in back.sheets[0].charts.iter().zip(&again.sheets[0].charts) {
            assert_eq!(normalized(b.clone(), a), normalized(a.clone(), a));
        }
    }

    /// Waterfall and histogram charts as Excel 2016 saves them: the newer
    /// chart (data through defined names) with a plain one to fall back to.
    #[test]
    fn chartex_charts() {
        let mut wb = book();
        let charts: Vec<Chart> = all_charts(&wb).into_iter().filter(|c| matches!(c.kind, ChartType::Waterfall | ChartType::Histogram)).collect();
        assert_eq!(charts.len(), 3);
        for c in all_charts(&wb).iter().filter(|c| c.kind.is_chartex()) {
            let x = chartex_xml(&wb, 0, c).unwrap();
            let back = normalized(read_chartex(&x).unwrap(), c);
            assert_eq!(&back, c, "{:?}", c.kind);
        }
        wb.add_chart(0, charts[0].clone());
        let bytes = crate::xlsx::write(&wb);
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let ex = chartex_xml(&wb, 0, &charts[0]).unwrap();
        let ex = ex.replace("<cx:f>Sheet1!$A$2:$A$5</cx:f>", "<cx:f>_xlchart.v1.0</cx:f>").replace("<cx:f>Sheet1!$B$2:$B$5</cx:f>", "<cx:f>_xlchart.v1.1</cx:f>");
        let mut w = crate::zip::ZipWriter::new();
        for n in z.names() {
            let mut data = String::from_utf8(z.read(n).unwrap()).unwrap_or_default();
            if n == "xl/workbook.xml" {
                data = data.replace("</sheets>", r#"</sheets><definedNames><definedName name="_xlchart.v1.0" hidden="1">Sheet1!$A$2:$A$5</definedName><definedName name="_xlchart.v1.1" hidden="1">Sheet1!$B$2:$B$5</definedName></definedNames>"#);
            }
            if n == "xl/drawings/_rels/drawing1.xml.rels" {
                data = data.replace("</Relationships>", r#"<Relationship Id="rId99" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="../charts/chartEx1.xml"/></Relationships>"#);
            }
            if n == "xl/drawings/drawing1.xml" {
                let (a, b) = (data.find("<xdr:graphicFrame").unwrap(), data.find("</xdr:graphicFrame>").unwrap() + 19);
                let plain = data[a..b].to_string();
                // A picture-less frame for Excel 2016, as Excel writes it.
                let newer = r#"<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="9" name="Newer"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" r:id="rId99"/></a:graphicData></a:graphic></xdr:graphicFrame>"#;
                let alt = format!(r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" Requires="cx1">{newer}</mc:Choice><mc:Fallback>{plain}</mc:Fallback></mc:AlternateContent>"#);
                data.replace_range(a..b, &alt);
            }
            if n == "[Content_Types].xml" {
                data = data.replace("</Types>", r#"<Override PartName="/xl/charts/chartEx1.xml" ContentType="application/vnd.ms-office.chartex+xml"/></Types>"#);
            }
            w.add(n, data.as_bytes(), true);
        }
        w.add("xl/charts/chartEx1.xml", ex.as_bytes(), true);
        let back = crate::xlsx::read(&w.finish()).unwrap();
        let c = &back.sheets[0].charts[0];
        assert_eq!(c.name, "Newer");
        assert_eq!(c.series[0].values.to_canonical().as_deref(), Some("Sheet1!$B$2:$B$5"));
        assert_eq!(c.series[0].categories.to_canonical().as_deref(), Some("Sheet1!$A$2:$A$5"));
        let mut c = c.clone();
        c.name = charts[0].name.clone();
        assert_eq!(normalized(c, &charts[0]), charts[0]);
        assert_eq!(back.sheets[0].charts[0].from, charts[0].from);
    }

    /// APEX_DUMP=dir saves the newer and the richer charts, to check them
    /// in other programs.
    #[test]
    fn dump_charts() {
        let Some(d) = std::env::var_os("APEX_DUMP") else { return };
        let mut wb = book();
        let all = all_charts(&wb);
        let n = all.len();
        let picked = all.into_iter().enumerate().filter(|(k, c)| matches!(c.kind, ChartType::Waterfall | ChartType::Histogram) || *k + 2 >= n);
        for (m, (_, mut c)) in picked.enumerate() {
            c.from = CellRef::new(6 + (m as u32 / 3) * 10, 1 + (m as u32 % 3) * 5);
            (c.width, c.height) = (420.0, 210.0);
            wb.add_chart(0, c);
        }
        let _ = std::fs::write(std::path::Path::new(&d).join("charts.xlsx"), crate::xlsx::write(&wb));
    }

    #[test]
    fn caches_follow_cells() {
        let mut wb = book();
        let c = wb.chart_from_range(0, Range::parse("A1:B5").unwrap(), ChartType::Column, None);
        wb.add_chart(0, c);
        wb.set_input(0, CellRef::new(1, 1), "1234");
        wb.execute(Op::InsertRows { sheet: 0, at: 0, count: 1 });
        let bytes = crate::xlsx::write(&wb);
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let x = z.read_string("xl/charts/chart1.xml").unwrap();
        assert!(x.contains("<c:f>Sheet1!$B$3:$B$6</c:f>") && x.contains("<c:v>1234</c:v>"), "{x}");
        // A reference to another workbook keeps its cached values.
        let x = x.replace("<c:f>Sheet1!$B$3:$B$6</c:f>", "<c:f>[1]Other!$B$3:$B$6</c:f>");
        let c = read_chart(&x).unwrap();
        let d = wb.chart_data(0, &c);
        assert_eq!(d.series[0].values[0], Some(1234.0));
    }

    #[test]
    fn sparklines_roundtrip() {
        let mut wb = book();
        let mut groups = Vec::new();
        for (i, kind) in [SparkKind::Line, SparkKind::Column, SparkKind::WinLoss].into_iter().enumerate() {
            let row = i as u32 + 1;
            groups.push(SparkGroup {
                kind,
                color: 0x123456 + i as u32,
                markers: i == 0,
                high: true,
                low: i == 1,
                first: true,
                last: i == 2,
                negative: true,
                line_weight: 1.25,
                min: if i == 0 { SparkAxis::Custom(-3.5) } else { SparkAxis::Group },
                max: if i == 1 { SparkAxis::Custom(40.0) } else { SparkAxis::Individual },
                show_axis: i == 2,
                blanks: [Blanks::Gap, Blanks::Zero, Blanks::Span][i],
                right_to_left: i == 1,
                visible_only: i != 0,
                sparklines: vec![Sparkline { cell: CellRef::new(row, 5), data: DataRef::parse(&format!("B{}:D{}", row + 1, row + 1)) }],
                ..Default::default()
            });
        }
        for g in groups {
            wb.add_sparklines(0, g);
        }
        let bytes = crate::xlsx::write(&wb);
        let sheet = crate::zip::ZipReader::new(&bytes).unwrap().read_string("xl/worksheets/sheet1.xml").unwrap();
        assert!(sheet.ends_with("</extLst></worksheet>"), "{sheet}");
        let back = crate::xlsx::read(&bytes).unwrap();
        let (a, b) = (&wb.sheets[0].sparklines, &back.sheets[0].sparklines);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            let mut y = y.clone();
            // Written qualified with the sheet.
            assert_eq!(y.sparklines[0].data.to_canonical().unwrap(), format!("Sheet1!{}", x.sparklines[0].data.to_canonical().unwrap()));
            y.sparklines[0].data = x.sparklines[0].data.clone();
            assert_eq!(&y, x);
        }
    }

    #[test]
    fn hostile_charts() {
        let bad = [
            "",
            "<c:chartSpace/>",
            "<c:chartSpace><c:chart><c:plotArea><c:barChart><c:ser><c:val><c:numRef><c:f>((</c:f><c:numCache><c:ptCount val=\"99999999999\"/><c:pt idx=\"-1\"><c:v>x</c:v></c:pt><c:pt idx=\"99999999\"><c:v>1</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser><c:gapWidth val=\"NaN\"/><c:overlap val=\"1e9\"/></c:barChart><c:valAx><c:scaling><c:min val=\"inf\"/><c:logBase val=\"-2\"/></c:scaling></c:valAx></c:plotArea></c:chart></c:chartSpace>",
            "<c:chartSpace><c:chart><c:plotArea><c:pieChart><c:holeSize val=\"900\"/><c:ser><c:dPt><c:idx val=\"1e30\"/></c:dPt></c:ser></c:pieChart></c:plotArea></c:chart></c:chartSpace>",
            "<cx:chartSpace><cx:chart><cx:plotArea><cx:plotAreaRegion><cx:series layoutId=\"clusteredColumn\"><cx:layoutPr><cx:binning><cx:binCount val=\"-4\"/></cx:binning></cx:layoutPr></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>",
        ];
        for x in bad {
            if let Some(c) = read_chart(x) {
                let _ = Workbook::new().chart_data(0, &c);
            }
            let _ = read_chartex(x);
        }
        // Deep nesting doesn't overflow the stack.
        let deep = "<a>".repeat(100_000);
        assert!(dom(&deep).is_some());
        let c = read_chart(bad[2]).unwrap();
        assert!(c.series[0].values.cache.len() <= MAX_POINTS);
        // A package whose chart parts are gone or broken.
        let mut wb = book();
        for c in all_charts(&wb) {
            wb.add_chart(0, c);
        }
        let bytes = crate::xlsx::write(&wb);
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let mut w = crate::zip::ZipWriter::new();
        for n in z.names() {
            let mut data = z.read(n).unwrap();
            if n == "xl/charts/chart2.xml" {
                continue;
            }
            if n == "xl/charts/chart3.xml" {
                data.truncate(data.len() / 2);
            }
            if n.ends_with("drawing1.xml.rels") {
                data = String::from_utf8(data).unwrap().replace("chart4.xml", "../../../nothing.xml").into_bytes();
            }
            w.add(n, &data, true);
        }
        let back = crate::xlsx::read(&w.finish()).unwrap();
        // Charts 2, 3 and 4 are lost.
        assert!(back.sheets[0].charts.len() >= wb.sheets[0].charts.len() - 3);
    }

    #[test]
    fn theme_colors_and_transforms() {
        let x = r#"<c:chartSpace><c:chart><c:plotArea><c:lineChart><c:ser><c:spPr><a:ln><a:solidFill><a:schemeClr val="accent2"><a:lumMod val="75000"/></a:schemeClr></a:solidFill></a:ln></c:spPr><c:marker><c:symbol val="none"/></c:marker></c:ser><c:ser><c:spPr><a:ln><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></c:spPr><c:marker><c:symbol val="none"/></c:marker></c:ser></c:lineChart></c:plotArea><c:legend/></c:chart></c:chartSpace>"#;
        let c = read_chart(x).unwrap();
        assert_eq!(c.kind, ChartType::Line);
        assert!(!c.markers);
        let col = c.series[0].color.unwrap();
        // Darker orange.
        assert!(col >> 16 < 0xED && col >> 16 > 0x90, "{col:06X}");
        assert_eq!(c.series[1].color, Some(0xFF0000));
        assert_eq!(c.legend, LegendPos::Right);
        assert!(!c.show_title);
    }

    /// Reads the real files named in `APEX_F9_REAL` (a folder): charts and
    /// sparklines, saved and read again.
    #[test]
    #[ignore]
    fn real_files() {
        let Ok(dir) = std::env::var("APEX_F9_REAL") else { return };
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "xlsx" || e == "xlsm")).collect();
        names.sort();
        for p in names {
            let Ok(bytes) = std::fs::read(&p) else { continue };
            if !String::from_utf8_lossy(&bytes).contains("xl/charts/") && !String::from_utf8_lossy(&bytes).contains("sparkline") {
                continue;
            }
            let Ok(wb) = crate::xlsx::read(&bytes) else { continue };
            let n: usize = wb.sheets.iter().map(|s| s.charts.len()).sum();
            let sp: usize = wb.sheets.iter().map(|s| s.sparklines.iter().map(|g| g.sparklines.len()).sum::<usize>()).sum();
            let kinds: Vec<String> = wb.sheets.iter().flat_map(|s| s.charts.iter().map(|c| format!("{}x{}", c.kind.key(), c.series.len()))).collect();
            let back = crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap();
            let n2: usize = back.sheets.iter().map(|s| s.charts.len()).sum();
            let sp2: usize = back.sheets.iter().map(|s| s.sparklines.iter().map(|g| g.sparklines.len()).sum::<usize>()).sum();
            println!("{}: {n} charts {kinds:?}, {sp} sparklines -> {n2}, {sp2}", p.file_name().unwrap().to_string_lossy());
            assert_eq!((n, sp), (n2, sp2), "{}", p.display());
        }
    }

    /// Writes sample workbooks to `APEX_F9_OUT` for checking in other
    /// programs.
    #[test]
    #[ignore]
    fn samples() {
        let Ok(dir) = std::env::var("APEX_F9_OUT") else { return };
        let mut wb = book();
        for c in all_charts(&wb) {
            wb.add_chart(0, c);
        }
        let g = SparkGroup { markers: true, high: true, low: true, negative: true, sparklines: (1..5).map(|r| Sparkline { cell: CellRef::new(r, 5), data: DataRef::parse(&format!("B{}:D{}", r + 1, r + 1)) }).collect(), ..Default::default() };
        wb.add_sparklines(0, g);
        let g = SparkGroup { kind: SparkKind::Column, negative: true, sparklines: vec![Sparkline { cell: CellRef::new(1, 6), data: DataRef::parse("B2:B5") }], ..Default::default() };
        wb.add_sparklines(0, g);
        let g = SparkGroup { kind: SparkKind::WinLoss, sparklines: vec![Sparkline { cell: CellRef::new(2, 6), data: DataRef::parse("C2:C5") }], ..Default::default() };
        wb.add_sparklines(0, g);
        std::fs::write(format!("{dir}/charts.xlsx"), crate::xlsx::write(&wb)).unwrap();
        std::fs::write(format!("{dir}/charts.ods"), crate::ods::write(&wb)).unwrap();
    }

    /// Writes a forecast sheet (APEX_F9_OUT) to compare the FORECAST.ETS
    /// values with other programs.
    #[test]
    #[ignore]
    fn forecast_sample() {
        let Ok(dir) = std::env::var("APEX_F9_OUT") else { return };
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::format::Locale::EN;
        wb.set_input(0, CellRef::new(0, 0), "Month");
        wb.set_input(0, CellRef::new(0, 1), "Sales");
        for i in 0..36u32 {
            let s = [0.0, 5.0, 9.0, 4.0, -2.0, -6.0, -3.0, 2.0, 6.0, 10.0, 4.0, -1.0][i as usize % 12];
            let noise = ((i * 7919) % 13) as f64 / 4.0 - 1.5;
            wb.set_input(0, CellRef::new(i + 1, 0), &(i + 1).to_string());
            wb.set_input(0, CellRef::new(i + 1, 1), &(100.0 + 1.5 * i as f64 + s + noise).to_string());
        }
        let r = Range::new(CellRef::new(0, 0), CellRef::new(36, 1));
        let src = wb.forecast_source(0, &r, apex_sheet::analysis::Lang::En).unwrap();
        let o = apex_sheet::workbook::ForecastOptions { end: 48.0, statistics: true, ..Default::default() };
        wb.forecast_sheet(&src, &o, apex_sheet::analysis::Lang::En).unwrap();
        std::fs::write(format!("{dir}/forecast.xlsx"), crate::xlsx::write(&wb)).unwrap();
        std::fs::write(format!("{dir}/forecast.ods"), crate::ods::write(&wb)).unwrap();
        let sh = &wb.sheets[1];
        for row in 36..49 {
            eprintln!("{} {:?} {:?} {:?}", row + 1, sh.value(CellRef::new(row, 2)), sh.value(CellRef::new(row, 3)), sh.value(CellRef::new(row, 4)));
        }
        for row in 1..8 {
            eprintln!("stat {:?}", sh.value(CellRef::new(row, 7)));
        }
    }
}
