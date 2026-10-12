//! Chart painting: turns a chart and its data into egui shapes (on screen,
//! and in printed pages and PDFs as vectors), sparklines in cells, and a
//! picture of a chart for the clipboard.
//!
//! Filled shapes are always convex pieces (egui fills only convex paths and
//! the PDF writer takes them as they are).

use std::sync::{Arc, Mutex};

use apex_sheet::chart::{Blanks, BoxFormat, Chart, ChartData, ChartType, Fill, Grouping, LegendPos, Marker, Scale, SparkAxis, SparkGroup, SparkKind, TrendKind};
use apex_sheet::{Locale, NumFormat};
use eframe::egui::{self, Align2, Color32, FontId, Galley, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

use crate::fonts::{self, Face};
use crate::i18n::tr;

mod kinds;

pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Palette color `i` (cycling, darker on each round like Office).
pub fn palette_color(palette: u8, i: usize) -> Color32 {
    colors_color(&apex_sheet::chart::palette(palette), i)
}

/// Automatic color `i` of a chart (its palette, or its theme's accents).
pub fn auto_color(chart: &Chart, i: usize) -> Color32 {
    colors_color(&chart.colors(), i)
}

fn colors_color(p: &[u32; 10], i: usize) -> Color32 {
    let c = rgb(p[i % p.len()]);
    let round = (i / p.len()).min(4) as f32;
    if round == 0.0 { c } else { mix(c, Color32::BLACK, round * 0.18) }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Fill color of series `i`.
pub fn series_color(chart: &Chart, i: usize) -> Color32 {
    match chart.series.get(i).and_then(|s| s.color) {
        Some(c) => rgb(c),
        None => auto_color(chart, i),
    }
}

/// Color of point `j` of series `i` (pie slices, varied columns).
pub fn point_color(chart: &Chart, i: usize, j: usize) -> Color32 {
    let s = chart.series.get(i);
    if let Some((_, c)) = s.and_then(|s| s.point_colors.iter().find(|(k, _)| *k as usize == j)) {
        return rgb(*c);
    }
    if chart.vary_colors && (chart.kind.is_round() || chart.series.len() == 1) {
        auto_color(chart, j)
    } else {
        series_color(chart, i)
    }
}

/// The look of a gallery style.
#[derive(Clone, Copy)]
pub struct Look {
    pub bg: Color32,
    pub plot_bg: Option<Color32>,
    pub border: Option<Color32>,
    pub text: Color32,
    pub grid: Color32,
    pub axis: Color32,
    /// Outline of columns, bars and slices (None: none; slices get the
    /// background color).
    pub outline: Option<Color32>,
    pub line_width: f32,
    /// Fills are lightened this much (0 = as they are).
    pub tint: f32,
}

pub fn look(style: u8) -> Look {
    let base = Look {
        bg: Color32::WHITE,
        plot_bg: None,
        border: Some(Color32::from_gray(217)),
        text: Color32::from_gray(89),
        grid: Color32::from_gray(217),
        axis: Color32::from_gray(191),
        outline: None,
        line_width: 2.25,
        tint: 0.0,
    };
    match style {
        // Light gray plot.
        1 => Look { plot_bg: Some(Color32::from_gray(242)), grid: Color32::WHITE, ..base },
        // Dark.
        2 => Look { bg: Color32::from_gray(64), border: None, text: Color32::from_gray(217), grid: Color32::from_gray(100), axis: Color32::from_gray(140), ..base },
        // Outlined, pale fills.
        3 => Look { outline: Some(Color32::from_gray(64)), tint: 0.55, line_width: 1.5, ..base },
        // Bold: thick lines, no border.
        4 => Look { border: None, line_width: 3.5, text: Color32::from_gray(64), ..base },
        // Soft: tinted fills, dotted feel.
        5 => Look { tint: 0.3, grid: Color32::from_gray(235), ..base },
        // Blue background.
        6 => Look { bg: rgb(0xDEEBF7), border: Some(rgb(0x9DC3E6)), grid: Color32::WHITE, text: rgb(0x1F4E79), ..base },
        // Black and white print.
        7 => Look { outline: Some(Color32::BLACK), text: Color32::BLACK, grid: Color32::from_gray(160), axis: Color32::BLACK, line_width: 1.5, ..base },
        _ => base,
    }
}

impl Look {
    fn fill(&self, c: Color32) -> Color32 {
        if self.tint > 0.0 { mix(c, Color32::WHITE, self.tint) } else { c }
    }
}

/// Font sizes in sheet pixels.
const TITLE_PX: f32 = 18.7;
const LABEL_PX: f32 = 12.0;
const AXIS_TITLE_PX: f32 = 13.3;

/// A chart being turned into shapes.
struct Canvas<'a> {
    ctx: &'a egui::Context,
    out: Vec<Shape>,
    z: f32,
    loc: &'a Locale,
    look: Look,
    /// The chart area's font, size (as a factor of the default 10 pt) and
    /// weight, applied to every text.
    family: Option<String>,
    fs: f32,
    bold: bool,
}

impl Canvas<'_> {
    fn font(&self, px: f32, bold: bool) -> FontId {
        let size = (px * self.z * self.fs).clamp(1.0, 400.0);
        match &self.family {
            Some(f) => fonts::font_for(Some(f), bold || self.bold, false, size).0,
            None => fonts::font(Face::Sans, bold || self.bold, size),
        }
    }

    /// A rectangle in a gradient or pattern.
    fn fancy_rect(&mut self, r: Rect, f: &Fill) {
        if r.width() <= 0.0 || r.height() <= 0.0 {
            return;
        }
        match f {
            Fill::Gradient { .. } => {
                let n = if r.width().max(r.height()) < 12.0 { 2 } else { 12 };
                let mut mesh = egui::epaint::Mesh::default();
                for yi in 0..=n {
                    for xi in 0..=n {
                        let p = pos2(r.min.x + r.width() * xi as f32 / n as f32, r.min.y + r.height() * yi as f32 / n as f32);
                        mesh.colored_vertex(p, fill_at(f, r, p));
                    }
                }
                let w = n as u32 + 1;
                for yi in 0..n as u32 {
                    for xi in 0..n as u32 {
                        let k = yi * w + xi;
                        mesh.add_triangle(k, k + 1, k + w);
                        mesh.add_triangle(k + 1, k + w + 1, k + w);
                    }
                }
                self.out.push(Shape::mesh(mesh));
            }
            Fill::Pattern { preset, fg, bg } => {
                self.out.push(Shape::rect_filled(r, 0.0, rgb(*bg)));
                let (dirs, spacing, width) = hatch_of(preset);
                if dirs.is_empty() {
                    self.out.push(Shape::rect_filled(r, 0.0, fill_at(f, r, r.center())));
                    return;
                }
                let stroke = Stroke::new(width * self.z.max(0.5), rgb(*fg));
                for d in dirs {
                    hatch(&mut self.out, r, d, spacing * self.z.max(0.5), stroke);
                }
            }
        }
    }

    /// A chart or plot area: fill (color, gradient or pattern) and border.
    fn area(&mut self, r: Rect, b: &BoxFormat, fill: Option<Color32>, border: Option<Color32>) {
        match (&b.fancy, b.fill, b.no_fill) {
            (Some(f), _, _) => self.fancy_rect(r, f),
            (_, Some(c), _) => self.out.push(Shape::rect_filled(r, 0.0, rgb(c))),
            (_, _, true) => {}
            _ => {
                if let Some(c) = fill {
                    self.out.push(Shape::rect_filled(r, 0.0, c));
                }
            }
        }
        let border = match (b.border, b.no_border) {
            (Some(c), _) => Some(rgb(c)),
            (_, true) => None,
            _ => border,
        };
        if let Some(c) = border {
            self.out.push(Shape::rect_stroke(r, 0.0, Stroke::new(0.75 * self.z, c), egui::StrokeKind::Inside));
        }
    }

    fn galley(&self, text: &str, px: f32, bold: bool, color: Color32) -> Arc<Galley> {
        let font = self.font(px, bold);
        self.ctx.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, color))
    }

    fn text_size(&self, text: &str, px: f32) -> Vec2 {
        self.galley(text, px, false, Color32::BLACK).size()
    }

    /// Text placed by `align` at `at`.
    fn text(&mut self, at: Pos2, align: Align2, text: &str, px: f32, bold: bool, color: Color32) -> Rect {
        let g = self.galley(text, px, bold, color);
        let r = align.anchor_size(at, g.size());
        self.out.push(Shape::galley(r.min, g, color));
        r
    }

    /// Text turned a quarter counter-clockwise, centered at `at`.
    fn text_up(&mut self, at: Pos2, text: &str, px: f32, color: Color32) {
        let g = self.galley(text, px, false, color);
        let s = g.size();
        // Rotating about the galley origin: after -90 degrees the text runs
        // upwards from its bottom-left.
        let pos = pos2(at.x - s.y / 2.0, at.y + s.x / 2.0);
        self.out.push(egui::epaint::TextShape::new(pos, g, color).with_angle(-std::f32::consts::FRAC_PI_2).into());
    }

    fn rect(&mut self, r: Rect, fill: Color32, stroke: Option<Color32>) {
        if r.width() <= 0.0 && r.height() <= 0.0 {
            return;
        }
        let s = stroke.map_or(Stroke::NONE, |c| Stroke::new(0.75 * self.z, c));
        self.out.push(Shape::rect_filled(r, 0.0, fill));
        if s.width > 0.0 {
            self.out.push(Shape::rect_stroke(r, 0.0, s, egui::StrokeKind::Middle));
        }
    }

    fn poly(&mut self, pts: Vec<Pos2>, fill: Color32) {
        if pts.len() >= 3 {
            self.out.push(Shape::convex_polygon(pts, fill, Stroke::NONE));
        }
    }

    fn line(&mut self, pts: Vec<Pos2>, stroke: Stroke) {
        if pts.len() >= 2 {
            self.out.push(Shape::line(pts, stroke));
        }
    }

    fn marker(&mut self, at: Pos2, m: Marker, size: f32, fill: Color32) {
        let r = size / 2.0;
        let edge = Stroke::new(0.75 * self.z, mix(fill, Color32::BLACK, 0.25));
        match m {
            Marker::None => {}
            Marker::Square => self.out.push(Shape::rect_filled(Rect::from_center_size(at, Vec2::splat(size)), 0.0, fill)),
            Marker::Diamond => self.out.push(Shape::convex_polygon(vec![at + vec2(0.0, -r), at + vec2(r, 0.0), at + vec2(0.0, r), at + vec2(-r, 0.0)], fill, edge)),
            Marker::Triangle => self.out.push(Shape::convex_polygon(vec![at + vec2(0.0, -r), at + vec2(r, r), at + vec2(-r, r)], fill, edge)),
            Marker::Auto | Marker::Circle => self.out.push(Shape::Circle(egui::epaint::CircleShape { center: at, radius: r, fill, stroke: edge })),
        }
    }

    fn num(&self, v: f64, fmt: &NumFormat) -> String {
        fmt.format_number(v, self.loc).0
    }
}

/// The color of a gradient or pattern at `p` of `bounds`.
fn fill_at(f: &Fill, b: Rect, p: Pos2) -> Color32 {
    match f {
        Fill::Gradient { stops, angle, radial } => {
            let t = if *radial {
                (p - b.center()).length() / (b.size().length() / 2.0).max(1e-3)
            } else {
                let a = angle.to_radians();
                let d = vec2(a.cos(), a.sin());
                let proj = |q: Pos2| (q - b.center()).dot(d);
                let ends = [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom()].map(proj);
                let (lo, hi) = (ends.iter().cloned().fold(f32::MAX, f32::min), ends.iter().cloned().fold(f32::MIN, f32::max));
                if hi > lo { (proj(p) - lo) / (hi - lo) } else { 0.0 }
            };
            rgb(Fill::at(stops, t.clamp(0.0, 1.0)))
        }
        Fill::Pattern { preset, fg, bg } => {
            // Where lines are not drawn: the share of the foreground.
            let share = preset.strip_prefix("pct").and_then(|p| p.parse::<f32>().ok()).map_or(0.5, |p| p / 100.0);
            mix(rgb(*bg), rgb(*fg), share)
        }
    }
}

/// Line directions, spacing and width of a pattern preset (no directions:
/// drawn as a blend).
fn hatch_of(preset: &str) -> (Vec<Vec2>, f32, f32) {
    let p = preset.to_ascii_lowercase();
    let spacing = if p.starts_with("lt") || p.starts_with("sm") || p.starts_with("nar") {
        4.0
    } else if p.starts_with("dk") || p.starts_with("wd") || p.starts_with("lg") {
        8.0
    } else {
        6.0
    };
    let width = if p.starts_with("dk") || p.starts_with("wd") { 2.0 } else { 1.0 };
    let (h, v, down, up) = (vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0).normalized(), vec2(1.0, -1.0).normalized());
    let dirs = if p.ends_with("horz") || p.ends_with("dashhorz") {
        vec![h]
    } else if p.ends_with("vert") {
        vec![v]
    } else if p.ends_with("dndiag") {
        vec![down]
    } else if p.ends_with("updiag") {
        vec![up]
    } else if p == "cross" || p.ends_with("grid") {
        vec![h, v]
    } else if p == "diagcross" || p.ends_with("check") || p == "opendmnd" || p == "soliddmnd" || p == "trellis" {
        vec![down, up]
    } else {
        Vec::new()
    };
    (dirs, spacing, width)
}

/// Parallel lines along `dir`, `spacing` apart, clipped to `r`.
fn hatch(out: &mut Vec<Shape>, r: Rect, dir: Vec2, spacing: f32, stroke: Stroke) {
    let n = vec2(-dir.y, dir.x);
    let c = r.center();
    let corners = [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()];
    let (lo, hi) = corners.iter().map(|q| (*q - c).dot(n)).fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
    let len = r.size().length();
    let mut s = (lo / spacing).ceil() * spacing;
    let mut count = 0;
    while s <= hi && count < 2000 {
        let (a, b) = (c + n * s - dir * len, c + n * s + dir * len);
        if let Some((p, q)) = clip(a, b, r) {
            out.push(Shape::line_segment([p, q], stroke));
        }
        s += spacing;
        count += 1;
    }
}

/// The part of segment `a`-`b` inside `r` (Liang-Barsky).
fn clip(a: Pos2, b: Pos2, r: Rect) -> Option<(Pos2, Pos2)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-d.x, a.x - r.min.x), (d.x, r.max.x - a.x), (-d.y, a.y - r.min.y), (d.y, r.max.y - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 < t1).then(|| (a + d * t0, a + d * t1))
}

/// Number format for axis and labels.
fn format_for(code: &str, fallback: &str) -> NumFormat {
    let c = if code.trim().is_empty() { fallback } else { code };
    NumFormat::parse(if c.trim().is_empty() { "General" } else { c })
}

/// The title shown (explicit, else the only series' name).
pub fn shown_title(chart: &Chart, data: &ChartData) -> Option<String> {
    if !chart.show_title {
        return None;
    }
    if !chart.title.trim().is_empty() {
        return Some(chart.title.clone());
    }
    match data.series.as_slice() {
        [one] if !chart.series.first().is_some_and(|s| s.name.is_empty()) => Some(one.name.clone()),
        _ => None,
    }
}

/// Values a series plots after histogram binning; the categories.
fn plotted(chart: &Chart, data: &ChartData) -> (Vec<Vec<Option<f64>>>, Vec<String>) {
    if chart.kind == ChartType::Histogram {
        let vals: Vec<f64> = data.series.first().map(|s| s.values.iter().flatten().copied().collect()).unwrap_or_default();
        let bins = apex_sheet::chart::histogram(&vals, chart.bins);
        let fmt = format_for("", &data.value_format);
        let loc = Locale::EN;
        let n = |v: f64| fmt.format_number((v * 1e6).round() / 1e6, &loc).0;
        let cats = bins.iter().enumerate().map(|(i, (a, b, _))| if i == 0 { format!("[{}, {}]", n(*a), n(*b)) } else { format!("({}, {}]", n(*a), n(*b)) }).collect();
        return (vec![bins.iter().map(|b| Some(b.2 as f64)).collect()], cats);
    }
    (data.series.iter().map(|s| s.values.clone()).collect(), data.categories.clone())
}

/// Builds the shapes of `chart` filling `rect`; `z` scales lines and text
/// (screen points per sheet pixel).
pub fn build(ctx: &egui::Context, rect: Rect, chart: &Chart, data: &ChartData, z: f32, loc: &Locale) -> Vec<Shape> {
    // A Pareto chart is sorted columns with a cumulative line.
    let pareto;
    let (chart, data) = if chart.kind == ChartType::Pareto {
        pareto = kinds::pareto(chart, data);
        (&pareto.0, &pareto.1)
    } else {
        (chart, data)
    };
    let mut look = look(chart.style);
    let a = &chart.area;
    if let Some(t) = a.text_color {
        look.text = rgb(t);
    }
    // Slice separators and label boxes follow the chart's background.
    if let Some(f) = a.fancy.as_ref().map(|f| f.main_color()).or(a.fill) {
        look.bg = rgb(f);
    }
    let mut c = Canvas { ctx, out: Vec::with_capacity(256), z, loc, look, family: a.font.clone(), fs: a.font_size.map_or(1.0, |s| (s / 10.0).clamp(0.2, 20.0)), bold: a.bold == Some(true) };
    let base = self::look(chart.style);
    c.area(rect, a, Some(base.bg), base.border);
    if rect.width() < 8.0 || rect.height() < 8.0 {
        return c.out;
    }
    let pad = 8.0 * z;
    let mut inner = rect.shrink(pad);
    if let Some(t) = shown_title(chart, data) {
        let r = c.text(pos2(rect.center().x, inner.min.y), Align2::CENTER_TOP, &t, TITLE_PX, false, look.text);
        inner.min.y = r.max.y + 6.0 * z;
    }
    let (values, cats) = plotted(chart, data);
    legend(&mut c, chart, data, &cats, &mut inner);
    if inner.width() < 10.0 || inner.height() < 10.0 {
        return c.out;
    }
    match chart.kind {
        ChartType::Pie if chart.view3d => kinds::pie3d(&mut c, chart, data, &values, &cats, inner),
        k if k.is_round() => round(&mut c, chart, data, &values, &cats, inner),
        ChartType::Radar => kinds::radar(&mut c, chart, data, &values, &cats, inner),
        ChartType::Surface => kinds::surface(&mut c, chart, data, &values, &cats, inner),
        ChartType::Treemap => kinds::treemap(&mut c, chart, data, &values, inner),
        ChartType::Sunburst => kinds::sunburst(&mut c, chart, data, &values, inner),
        ChartType::Funnel => kinds::funnel(&mut c, chart, data, &values, &cats, inner),
        ChartType::BoxWhisker => kinds::box_whisker(&mut c, chart, data, inner),
        ChartType::Mekko => kinds::mekko(&mut c, chart, data, &values, &cats, inner),
        ChartType::Timeline => kinds::timeline(&mut c, chart, data, &values, &cats, inner),
        _ => axes_chart(&mut c, chart, data, &values, &cats, inner),
    }
    c.out
}

/// Paints a chart (screen or print).
pub fn paint(p: &egui::Painter, rect: Rect, chart: &Chart, data: &ChartData, z: f32, loc: &Locale) {
    p.extend(build(p.ctx(), rect, chart, data, z, loc));
}

// --- legend -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Sym {
    Box,
    Line(Marker),
}

fn legend(c: &mut Canvas, chart: &Chart, data: &ChartData, cats: &[String], inner: &mut Rect) {
    // Stock charts are drawn in one color: their series need no key.
    if chart.legend == LegendPos::None || matches!(chart.kind, ChartType::Stock | ChartType::Timeline) {
        return;
    }
    let z = c.z;
    let mut items: Vec<(String, Color32, Sym)> = Vec::new();
    let per_point = chart.kind.is_round() || (chart.vary_colors && chart.series.len() == 1 && chart.kind != ChartType::Scatter && chart.kind != ChartType::Line);
    if chart.kind == ChartType::Surface {
        for (k, (a, b)) in kinds::surface_bands(chart, data).into_iter().enumerate() {
            let fmt = format_for(&chart.y_axis.format, &data.value_format);
            items.push((format!("{}-{}", c.num(a, &fmt), c.num(b, &fmt)), kinds::band_color(chart, k), Sym::Box));
        }
    } else if matches!(chart.kind, ChartType::Treemap | ChartType::Sunburst) {
        for (k, name) in kinds::top_groups(data).into_iter().enumerate().take(200) {
            items.push((name, kinds::category_color(chart, k), Sym::Box));
        }
    } else if chart.kind == ChartType::Waterfall {
        items.push((tr("chart.increase").into(), auto_color(chart, 0), Sym::Box));
        items.push((tr("chart.decrease").into(), auto_color(chart, 1), Sym::Box));
        if !chart.totals.is_empty() {
            items.push((tr("chart.total").into(), auto_color(chart, 2), Sym::Box));
        }
    } else if per_point {
        for (j, name) in cats.iter().enumerate().take(200) {
            items.push((name.clone(), point_color(chart, 0, j), Sym::Box));
        }
    } else {
        // A Marimekko's first series holds the widths, not a segment.
        let skip = usize::from(chart.kind == ChartType::Mekko && data.series.len() > 1);
        for (i, s) in data.series.iter().enumerate().skip(skip) {
            let kind = chart.series.get(i).map_or(chart.kind, |x| chart.series_kind(x));
            let sym = match kind {
                ChartType::Line => Sym::Line(if chart.markers { series_marker(chart, i) } else { Marker::None }),
                ChartType::Radar if !chart.filled => Sym::Line(if chart.markers { series_marker(chart, i) } else { Marker::None }),
                ChartType::Stock => Sym::Line(Marker::None),
                ChartType::Scatter if chart.lines => Sym::Line(if chart.markers { series_marker(chart, i) } else { Marker::None }),
                ChartType::Scatter => Sym::Line(series_marker(chart, i)),
                _ => Sym::Box,
            };
            items.push((s.name.clone(), series_color(chart, i), sym));
        }
    }
    if items.is_empty() {
        return;
    }
    let sym_w = 14.0 * z;
    let gap = 5.0 * z;
    let sizes: Vec<Vec2> = items.iter().map(|(t, _, _)| c.text_size(t, LABEL_PX)).collect();
    let line_h = sizes.iter().map(|s| s.y).fold(0.0, f32::max) + 4.0 * z;
    let draw = |c: &mut Canvas, at: Pos2, (t, col, sym): &(String, Color32, Sym)| {
        let mid = at + vec2(sym_w / 2.0, line_h / 2.0);
        match sym {
            Sym::Box => {
                let fill = c.look.fill(*col);
                c.rect(Rect::from_center_size(mid, Vec2::splat(7.0 * z)), fill, c.look.outline);
            }
            Sym::Line(m) => {
                c.line(vec![mid - vec2(sym_w / 2.0, 0.0), mid + vec2(sym_w / 2.0, 0.0)], Stroke::new(c.look.line_width * z * 0.8, *col));
                c.marker(mid, *m, 6.0 * z, *col);
            }
        }
        let text = c.look.text;
        c.text(pos2(at.x + sym_w + gap, mid.y), Align2::LEFT_CENTER, t, LABEL_PX, false, text);
    };
    match chart.legend {
        LegendPos::Right | LegendPos::Left => {
            let w = (sizes.iter().map(|s| s.x).fold(0.0, f32::max) + sym_w + gap).min(inner.width() * 0.4);
            let fit = ((inner.height() / line_h).floor() as usize).max(1).min(items.len());
            let total = fit as f32 * line_h;
            let x = if chart.legend == LegendPos::Right { inner.max.x - w } else { inner.min.x };
            let mut y = inner.center().y - total / 2.0;
            let room = w - sym_w - gap;
            for (t, col, sym) in items.iter().take(fit) {
                let t = elide(c, t, room);
                draw(c, pos2(x, y), &(t, *col, *sym));
                y += line_h;
            }
            if chart.legend == LegendPos::Right {
                inner.max.x -= w + 8.0 * z;
            } else {
                inner.min.x += w + 8.0 * z;
            }
        }
        LegendPos::Top | LegendPos::Bottom => {
            // Rows of entries, centered.
            let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
            let mut used = 0.0;
            for (i, s) in sizes.iter().enumerate() {
                let w = s.x + sym_w + gap + 12.0 * z;
                if used + w > inner.width() && !rows.last().is_some_and(|r| r.is_empty()) {
                    rows.push(Vec::new());
                    used = 0.0;
                }
                used += w;
                if let Some(r) = rows.last_mut() {
                    r.push(i);
                }
            }
            let max_rows = ((inner.height() * 0.3 / line_h).floor() as usize).max(1);
            rows.truncate(max_rows);
            let h = rows.len() as f32 * line_h;
            let mut y = if chart.legend == LegendPos::Top { inner.min.y } else { inner.max.y - h };
            for row in &rows {
                let width: f32 = row.iter().map(|&i| sizes[i].x + sym_w + gap + 12.0 * z).sum::<f32>() - 12.0 * z;
                let mut x = inner.center().x - width / 2.0;
                for &i in row {
                    draw(c, pos2(x, y), &items[i]);
                    x += sizes[i].x + sym_w + gap + 12.0 * z;
                }
                y += line_h;
            }
            if chart.legend == LegendPos::Top {
                inner.min.y += h + 6.0 * z;
            } else {
                inner.max.y -= h + 6.0 * z;
            }
        }
        LegendPos::None => {}
    }
}

/// `t` cut to `room` wide with an ellipsis.
fn elide(c: &Canvas, t: &str, room: f32) -> String {
    if c.text_size(t, LABEL_PX).x <= room {
        return t.to_string();
    }
    let chars: Vec<char> = t.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "…";
        if c.text_size(&s, LABEL_PX).x <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

fn series_marker(chart: &Chart, i: usize) -> Marker {
    match chart.series.get(i).map_or(Marker::Auto, |s| s.marker) {
        Marker::Auto => [Marker::Circle, Marker::Square, Marker::Diamond, Marker::Triangle][i % 4],
        m => m,
    }
}

// --- pie and doughnut -------------------------------------------------------------------

fn arc_point(center: Pos2, r: f32, a: f32) -> Pos2 {
    // `a` in radians clockwise from the top.
    center + vec2(a.sin() * r, -a.cos() * r)
}

fn round(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let z = c.z;
    let series: Vec<usize> = if chart.kind == ChartType::Pie { (0..values.len().min(1)).collect() } else { (0..values.len()).collect() };
    // Slices pulled out: the pie shrinks so they still fit.
    let most = series.iter().filter_map(|&si| chart.series.get(si)).map(|s| s.explode.max(s.point_explode.iter().map(|p| p.1).max().unwrap_or(0))).max().unwrap_or(0);
    let radius = area.width().min(area.height()) / 2.0 * 0.92 / (1.0 + most as f32 / 100.0);
    let center = area.center();
    if series.is_empty() || radius <= 1.0 {
        return;
    }
    let hole = if chart.kind == ChartType::Doughnut { radius * chart.hole as f32 / 100.0 } else { 0.0 };
    let ring = (radius - hole) / series.len() as f32;
    let start = (chart.first_angle as f32).to_radians();
    let tau = std::f32::consts::TAU;
    let fmt = format_for("", &data.value_format);
    let pct = NumFormat::parse("0%");
    let sep_color = c.look.outline.unwrap_or(c.look.bg);
    let mut labels: Vec<(Pos2, String)> = Vec::new();
    for (ri, &si) in series.iter().enumerate() {
        let vals: Vec<f64> = values[si].iter().map(|v| v.unwrap_or(0.0).abs()).collect();
        let total: f64 = vals.iter().sum();
        if total <= 0.0 {
            continue;
        }
        let (r0, r1) = (hole + ring * ri as f32, hole + ring * (ri + 1) as f32);
        let mut a = start;
        let mut bounds = Vec::new();
        for (j, v) in vals.iter().enumerate() {
            let sweep = (*v / total) as f32 * tau;
            if sweep <= 0.0 {
                continue;
            }
            let col = c.look.fill(point_color(chart, si, j));
            let sr = chart.series.get(si);
            let pulled = sr.map_or(0, |s| s.explosion(j)) as f32 / 100.0 * r1;
            let center = if pulled > 0.0 { arc_point(center, pulled, a + sweep / 2.0) } else { center };
            let fancy = sr.and_then(|s| s.fancy.as_ref()).filter(|_| !chart.series.get(si).is_some_and(|s| s.point_colors.iter().any(|p| p.0 as usize == j)));
            let bounds_r = Rect::from_center_size(center, Vec2::splat(2.0 * r1));
            let vc = |p: Pos2| fancy.map_or(col, |f| c.look.fill(fill_at(f, bounds_r, p)));
            // One mesh per slice (pieces of at most 3 degrees): separate
            // anti-aliased pieces would show seams. The arcs get a thin
            // stroke of the same color for smooth edges.
            let steps = ((sweep / 0.05).ceil() as usize).clamp(1, 1440);
            let outer: Vec<Pos2> = (0..=steps).map(|k| arc_point(center, r1, a + sweep * k as f32 / steps as f32)).collect();
            let mut mesh = egui::epaint::Mesh::default();
            if r0 <= 0.0 {
                mesh.colored_vertex(center, vc(center));
                for p in &outer {
                    mesh.colored_vertex(*p, vc(*p));
                }
                for k in 0..steps as u32 {
                    mesh.add_triangle(0, k + 1, k + 2);
                }
            } else {
                for (k, p) in outer.iter().enumerate() {
                    mesh.colored_vertex(*p, vc(*p));
                    let q = arc_point(center, r0, a + sweep * k as f32 / steps as f32);
                    mesh.colored_vertex(q, vc(q));
                }
                for k in 0..steps as u32 {
                    let i = 2 * k;
                    mesh.add_triangle(i, i + 1, i + 2);
                    mesh.add_triangle(i + 1, i + 3, i + 2);
                }
                let inner: Vec<Pos2> = (0..=steps).map(|k| arc_point(center, r0, a + sweep * k as f32 / steps as f32)).collect();
                c.out.push(Shape::mesh(mesh));
                mesh = egui::epaint::Mesh::default();
                if fancy.is_none() {
                    c.line(inner, Stroke::new(0.8, col));
                }
            }
            if !mesh.is_empty() {
                c.out.push(Shape::mesh(mesh));
            }
            if fancy.is_none() {
                c.line(outer, Stroke::new(0.8, col));
            }
            bounds.push((a, center));
            let mid = a + sweep / 2.0;
            if chart.labels.any() {
                let mut parts: Vec<String> = cell_label(chart, data, si, j).into_iter().collect();
                if chart.labels.series {
                    parts.push(data.series.get(si).map_or(String::new(), |s| s.name.clone()));
                }
                if chart.labels.category {
                    parts.push(cats.get(j).cloned().unwrap_or_default());
                }
                if chart.labels.value {
                    parts.push(c.num(values[si][j].unwrap_or(0.0), &fmt));
                }
                if chart.labels.percent {
                    parts.push(c.num(*v / total, &pct));
                }
                let rl = if r0 <= 0.0 { r1 * 0.65 } else { (r0 + r1) / 2.0 };
                labels.push((arc_point(center, rl, mid), parts.join(", ")));
            }
            a += sweep;
        }
        // Separators between slices.
        if vals.iter().filter(|v| **v > 0.0).count() > 1 {
            for (b, at) in bounds {
                c.line(vec![arc_point(at, r0, b), arc_point(at, r1, b)], Stroke::new(1.0 * z, sep_color));
            }
        }
    }
    let text = if c.look.bg == Color32::WHITE || c.look.tint > 0.0 { Color32::from_gray(30) } else { c.look.text };
    for (at, t) in labels {
        let g = c.galley(&t, LABEL_PX, false, text);
        let r = Rect::from_center_size(at, g.size());
        c.out.push(Shape::rect_filled(r.expand(1.5 * z), 2.0 * z, Color32::from_white_alpha(200)));
        c.out.push(Shape::galley(r.min, g, text));
    }
}

// --- charts with axes ---------------------------------------------------------------------

/// Where a series sits: its drawing type and its value axis.
#[derive(Clone, Copy, PartialEq)]
struct Role {
    kind: ChartType,
    secondary: bool,
}

/// A series' bars/points after stacking: start and end value per point.
struct Plotted {
    role: Role,
    spans: Vec<Option<(f64, f64)>>,
}

fn stack(chart: &Chart, values: &[Vec<Option<f64>>], roles: &[Role], n: usize) -> Vec<Plotted> {
    let mut out: Vec<Plotted> = roles.iter().map(|r| Plotted { role: *r, spans: vec![None; n] }).collect();
    if chart.kind == ChartType::Waterfall {
        let mut cum = 0.0;
        for (j, v) in values.first().map(|v| v.as_slice()).unwrap_or(&[]).iter().enumerate().take(n) {
            let v = v.unwrap_or(0.0);
            let span = if chart.totals.contains(&(j as u32)) {
                // A total shows the running sum up to here.
                (0.0, cum)
            } else {
                let s = (cum, cum + v);
                cum += v;
                s
            };
            if let Some(p) = out.first_mut() {
                p.spans[j] = Some(span);
            }
        }
        return out;
    }
    let stacked = matches!(chart.grouping, Grouping::Stacked | Grouping::Percent);
    // Series stack with the others of the same type and axis.
    let mut groups: Vec<Role> = Vec::new();
    for r in roles {
        if !groups.contains(r) {
            groups.push(*r);
        }
    }
    for g in groups {
        let members: Vec<usize> = (0..roles.len()).filter(|&i| roles[i] == g).collect();
        let can = stacked && g.kind.can_stack();
        // Lines and areas stack on a single running sum; columns and bars
        // stack negatives below zero apart (as Excel does).
        let signed = matches!(g.kind, ChartType::Line | ChartType::Area);
        for j in 0..n {
            let total: f64 = members.iter().map(|&i| values[i].get(j).copied().flatten().unwrap_or(0.0).abs()).sum();
            let (mut pos, mut neg) = (0.0, 0.0);
            for &i in &members {
                let Some(v) = values[i].get(j).copied().flatten() else { continue };
                let v = if can && chart.grouping == Grouping::Percent { if total > 0.0 { v / total } else { 0.0 } } else { v };
                out[i].spans[j] = Some(if !can {
                    (0.0, v)
                } else if v >= 0.0 || signed {
                    let s = (pos, pos + v);
                    pos += v;
                    s
                } else {
                    let s = (neg, neg + v);
                    neg += v;
                    s
                });
            }
        }
    }
    out
}

/// Maps values of one axis to screen positions.
#[derive(Clone, Copy)]
struct Map {
    scale: Scale,
    /// Screen coordinate of min and of max.
    a: f32,
    b: f32,
    reverse: bool,
}

impl Map {
    fn at(&self, v: f64) -> f32 {
        let f = self.scale.frac(v).clamp(-0.5, 1.5) as f32;
        let f = if self.reverse { 1.0 - f } else { f };
        self.a + (self.b - self.a) * f
    }

    /// Where bars start: zero, or the edge nearest to it.
    fn base(&self) -> f32 {
        let s = &self.scale;
        let v = if s.log.is_some() { s.min } else { 0f64.clamp(s.min, s.max) };
        self.at(v)
    }
}

fn axes_chart(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let z = c.z;
    let look = c.look;
    let n = values.iter().map(|v| v.len()).max().unwrap_or(0).max(cats.len().min(values.iter().map(|v| v.len()).max().unwrap_or(0)));
    let roles: Vec<Role> = (0..values.len())
        .map(|i| {
            let s = chart.series.get(i);
            let kind = s.map_or(chart.kind, |s| chart.series_kind(s));
            Role { kind, secondary: s.is_some_and(|s| s.secondary) && chart.has_secondary() }
        })
        .collect();
    let plotted = stack(chart, values, &roles, n);
    let scatter = chart.kind.is_xy();
    let horizontal = chart.kind == ChartType::Bar;
    let percent = chart.grouping == Grouping::Percent && chart.kind.can_stack();

    // Ends of the error bars, which the value axes take in.
    let err_ends: Vec<Vec<Option<(f64, f64)>>> = (0..plotted.len())
        .map(|i| match (chart.series.get(i).and_then(|s| s.error_bars.as_ref()), data.series.get(i), values.get(i)) {
            (Some(e), Some(d), Some(raw)) => e.ends(raw, &d.err_plus, &d.err_minus),
            _ => Vec::new(),
        })
        .collect();
    // Value ranges of both axes.
    let range = |secondary: bool| -> (f64, f64) {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for (i, p) in plotted.iter().enumerate() {
            if p.role.secondary != secondary {
                continue;
            }
            for (j, e) in err_ends[i].iter().enumerate() {
                let (Some((a, b)), Some(Some((_, top))), Some(Some(v))) = (e, p.spans.get(j), values[i].get(j)) else { continue };
                for x in [a + top - v, b + top - v] {
                    if x.is_finite() {
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
            }
        }
        for p in plotted.iter().filter(|p| p.role.secondary == secondary) {
            for (a, b) in p.spans.iter().flatten() {
                let bars = matches!(p.role.kind, ChartType::Column | ChartType::Bar | ChartType::Area | ChartType::Waterfall | ChartType::Histogram);
                for v in if bars { vec![*a, *b] } else { vec![*b] } {
                    if v.is_finite() {
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
            }
        }
        if lo > hi {
            (0.0, 1.0)
        } else if chart.kind == ChartType::Bubble {
            let pad = (hi - lo) * 0.15;
            (lo - pad, hi + pad)
        } else {
            (lo, hi)
        }
    };
    let secondary = roles.iter().any(|r| r.secondary);
    let (vlo, vhi) = range(false);
    let value_len = if horizontal { area.width() } else { area.height() };
    let ticks = ((value_len / (if horizontal { 70.0 } else { 36.0 } * z)).floor() as usize).clamp(2, 12);
    let yscale = apex_sheet::chart::scale_for(vlo, vhi, ticks, &chart.y_axis);
    let y2scale = secondary.then(|| {
        let (a, b) = range(true);
        apex_sheet::chart::scale_for(a, b, ticks, &chart.y2_axis)
    });
    let xscale = scatter.then(|| {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for s in &data.series {
            for x in s.xs.iter().flatten() {
                lo = lo.min(*x);
                hi = hi.max(*x);
            }
        }
        if lo > hi {
            (lo, hi) = (0.0, 1.0);
        }
        if chart.kind == ChartType::Bubble {
            // Room for the bubbles at the ends.
            let pad = ((hi - lo) * 0.12).max(0.5);
            (lo, hi) = (lo - pad, hi + pad);
        }
        let xt = ((area.width() / (70.0 * z)).floor() as usize).clamp(2, 12);
        apex_sheet::chart::scale_for(lo, hi, xt, &chart.x_axis)
    });

    let pct_fmt = NumFormat::parse("0%");
    let yfmt = if percent && chart.y_axis.format.is_empty() { pct_fmt.clone() } else { format_for(&chart.y_axis.format, if chart.kind == ChartType::Histogram { "General" } else { &data.value_format }) };
    let y2fmt = format_for(&chart.y2_axis.format, &data.value_format);
    let xfmt = format_for(&chart.x_axis.format, &data.cat_format);

    // Labels of the value axes and the category (or x) axis.
    let vlabels: Vec<(f64, String)> = if chart.y_axis.visible { yscale.ticks().into_iter().map(|v| (v, c.num(v, &yfmt))).collect() } else { Vec::new() };
    let v2labels: Vec<(f64, String)> = match (&y2scale, chart.y2_axis.visible) {
        (Some(s), true) => s.ticks().into_iter().map(|v| (v, c.num(v, &y2fmt))).collect(),
        _ => Vec::new(),
    };
    let xlabels: Vec<String> = match &xscale {
        Some(s) if chart.x_axis.visible => s.ticks().into_iter().map(|v| c.num(v, &xfmt)).collect(),
        Some(_) => Vec::new(),
        None if chart.x_axis.visible => cats.iter().take(n).cloned().collect(),
        None => Vec::new(),
    };
    let label_h = c.text_size("0", LABEL_PX).y;
    // Hundreds of categories (dates): measure only the longest few, laying
    // out every label costs milliseconds on each rebuild.
    let widest = |l: &mut dyn Iterator<Item = &String>, c: &Canvas| {
        let mut l: Vec<&String> = l.collect();
        if l.len() > 16 {
            l.select_nth_unstable_by_key(15, |t| std::cmp::Reverse(t.chars().count()));
            l.truncate(16);
        }
        l.into_iter().map(|t| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max)
    };
    let gap = 4.0 * z;

    let mut plot = area;
    // Axis titles.
    let ytitle = (!chart.y_axis.title.is_empty() && chart.y_axis.visible).then(|| chart.y_axis.title.clone());
    let y2title = (secondary && !chart.y2_axis.title.is_empty()).then(|| chart.y2_axis.title.clone());
    let xtitle = (!chart.x_axis.title.is_empty() && chart.x_axis.visible).then(|| chart.x_axis.title.clone());
    let title_h = c.text_size("Ag", AXIS_TITLE_PX).y;
    // In a bar chart the value axis is horizontal: the titles swap places.
    let (left_title, bottom_title) = if horizontal { (xtitle.clone(), ytitle.clone()) } else { (ytitle.clone(), xtitle.clone()) };
    if left_title.is_some() {
        plot.min.x += title_h + gap;
    }
    if bottom_title.is_some() {
        plot.max.y -= title_h + gap;
    }
    if y2title.is_some() {
        plot.max.x -= title_h + gap;
    }
    // Data table: under the plot, in place of the category labels.
    let table = chart.data_table && !horizontal && !scatter && n > 0 && n <= 500;
    let row_h = label_h * 1.4;
    let key_w = if chart.table_keys { 14.0 * z } else { 0.0 };
    let name_w = if table { data.series.iter().map(|s| c.text_size(&s.name, LABEL_PX).x).fold(0.0, f32::max).min(area.width() * 0.3) + key_w + 8.0 * z } else { 0.0 };
    let xlabels = if table { Vec::new() } else { xlabels };
    if table {
        plot.max.y -= row_h * (1 + values.len()) as f32;
    }
    // Room for the labels.
    let (left_w, bottom_h) = if horizontal {
        (widest(&mut xlabels.iter(), c), if vlabels.is_empty() { 0.0 } else { label_h })
    } else {
        (widest(&mut vlabels.iter().map(|(_, t)| t), c), if xlabels.is_empty() { 0.0 } else { label_h })
    };
    let left_w = left_w.min(area.width() * 0.35);
    if left_w > 0.0 {
        plot.min.x += left_w + gap;
    }
    if table {
        plot.min.x = plot.min.x.max(area.min.x + name_w);
    }
    // Rotated category labels need more room.
    let slot = if horizontal || scatter { f32::INFINITY } else { plot.width() / n.max(1) as f32 };
    let widest_cat = if horizontal || scatter { 0.0 } else { widest(&mut xlabels.iter(), c) };
    let rotate = !horizontal && !scatter && widest_cat > slot * 0.95 && n > 1;
    let bottom_h = if rotate { (widest_cat * 0.72 + label_h * 0.7).min(area.height() * 0.35) } else { bottom_h };
    if bottom_h > 0.0 {
        plot.max.y -= bottom_h + gap;
    }
    if !v2labels.is_empty() {
        plot.max.x -= widest(&mut v2labels.iter().map(|(_, t)| t), c) + gap;
    }
    plot.min.y += label_h / 2.0;
    plot.max.x -= if v2labels.is_empty() { 4.0 * z } else { 0.0 };
    // 3-D: the plot is a box; the front face keeps the axes, the back wall
    // is `d3` up and to the right.
    let d3 = if chart.view3d { (plot.width().min(plot.height()) * 0.08).clamp(4.0 * z, 24.0 * z) } else { 0.0 };
    plot.max.x -= d3;
    plot.min.y += d3;
    if plot.width() < 4.0 || plot.height() < 4.0 {
        return;
    }
    let deep = vec2(d3, -d3);
    if d3 > 0.0 {
        c.area(plot.translate(deep), &chart.plot, look.plot_bg, Some(look.grid));
        let wall = mix(look.grid, look.bg, 0.4);
        if horizontal {
            c.poly(vec![plot.left_bottom(), plot.left_top(), plot.left_top() + deep, plot.left_bottom() + deep], wall);
        } else {
            c.poly(vec![plot.left_bottom(), plot.right_bottom(), plot.right_bottom() + deep, plot.left_bottom() + deep], wall);
        }
    } else {
        c.area(plot, &chart.plot, look.plot_bg, None);
    }

    // Value maps and the category layout.
    let vmap = |s: Scale, rev: bool| if horizontal { Map { scale: s, a: plot.min.x, b: plot.max.x, reverse: rev } } else { Map { scale: s, a: plot.max.y, b: plot.min.y, reverse: rev } };
    let ym = vmap(yscale, chart.y_axis.reverse);
    let y2m = y2scale.map(|s| vmap(s, chart.y2_axis.reverse));
    let xm = xscale.map(|s| Map { scale: s, a: plot.min.x, b: plot.max.x, reverse: chart.x_axis.reverse });
    // Area charts alone put points on the edges of the plot.
    let edges = roles.iter().all(|r| r.kind == ChartType::Area) && n > 1 && !table;
    let cat_len = if horizontal { plot.height() } else { plot.width() };
    let band = if edges { cat_len / (n - 1) as f32 } else { cat_len / n.max(1) as f32 };
    // Bars go up from the bottom: the first category of a bar chart is at
    // the bottom, like Excel.
    let cat_pos = |j: usize| -> f32 {
        let k = if chart.x_axis.reverse != horizontal { n.saturating_sub(1).saturating_sub(j) } else { j };
        let off = if edges { band * k as f32 } else { band * (k as f32 + 0.5) };
        if horizontal { plot.min.y + off } else { plot.min.x + off }
    };
    let map_of = |r: Role| if r.secondary { y2m.unwrap_or(ym) } else { ym };

    // Gridlines.
    let grid = |c: &mut Canvas, v: f32, minor: bool| {
        let col = if minor { mix(look.grid, look.bg, 0.5) } else { look.grid };
        let s = Stroke::new(0.75 * z, col);
        if d3 > 0.0 {
            // Along the back wall and across the floor (or side wall).
            if horizontal {
                c.line(vec![pos2(v, plot.max.y), pos2(v, plot.max.y) + deep, pos2(v, plot.min.y) + deep], s);
            } else {
                c.line(vec![pos2(plot.min.x, v), pos2(plot.min.x, v) + deep, pos2(plot.max.x, v) + deep], s);
            }
        } else if horizontal {
            c.line(vec![pos2(v, plot.min.y), pos2(v, plot.max.y)], s);
        } else {
            c.line(vec![pos2(plot.min.x, v), pos2(plot.max.x, v)], s);
        }
    };
    if chart.y_axis.minor_grid {
        for v in yscale.minor_ticks().into_iter().take(500) {
            grid(c, ym.at(v), true);
        }
    }
    if chart.y_axis.major_grid {
        for v in yscale.ticks() {
            grid(c, ym.at(v), false);
        }
    }
    // Vertical gridlines of the category (or x) axis.
    let xgrid = |c: &mut Canvas, v: f32, minor: bool| {
        let col = if minor { mix(look.grid, look.bg, 0.5) } else { look.grid };
        let s = Stroke::new(0.75 * z, col);
        if horizontal {
            c.line(vec![pos2(plot.min.x, v), pos2(plot.max.x, v)], s);
        } else {
            c.line(vec![pos2(v, plot.min.y), pos2(v, plot.max.y)], s);
        }
    };
    match &xm {
        Some(m) => {
            if chart.x_axis.minor_grid {
                for v in m.scale.minor_ticks().into_iter().take(500) {
                    xgrid(c, m.at(v), true);
                }
            }
            if chart.x_axis.major_grid {
                for v in m.scale.ticks() {
                    xgrid(c, m.at(v), false);
                }
            }
        }
        None if chart.x_axis.major_grid && n > 0 && n <= 2000 => {
            let start = if horizontal { plot.min.y } else { plot.min.x };
            for k in 0..=n {
                xgrid(c, start + band * k as f32 - if edges { band / 2.0 } else { 0.0 }, false);
            }
        }
        None => {}
    }

    // Series: areas, then columns, then lines and points.
    let col_members: Vec<usize> = (0..plotted.len()).filter(|&i| matches!(roles[i].kind, ChartType::Column | ChartType::Bar | ChartType::Waterfall | ChartType::Histogram)).collect();
    let clustered = !matches!(chart.grouping, Grouping::Stacked | Grouping::Percent) || chart.kind == ChartType::Waterfall;
    // Columns side by side: one slot per series of each axis group.
    let slots: Vec<usize> = {
        let mut v = vec![0; plotted.len()];
        if clustered {
            for (k, &i) in col_members.iter().enumerate() {
                v[i] = k;
            }
        }
        v
    };
    let k = if clustered { col_members.len().max(1) } else { 1 };
    let gapf = chart.gap as f32 / 100.0;
    let ov = if clustered { chart.overlap as f32 / 100.0 } else { 0.0 };
    let bar_w = band / (k as f32 - (k as f32 - 1.0) * ov + gapf).max(0.01);

    // Where each value is drawn, for its error bars: the point and the
    // plotted (maybe stacked) value.
    let mut tips: Vec<Vec<Option<(Pos2, f64)>>> = if chart.series.iter().any(|s| s.error_bars.is_some()) { vec![vec![None; n]; plotted.len()] } else { vec![Vec::new(); plotted.len()] };
    let mut tip = |i: usize, j: usize, at: Pos2, b: f64| {
        if let Some(t) = tips.get_mut(i).and_then(|v| v.get_mut(j)) {
            *t = Some((at, b));
        }
    };
    // Areas.
    for (i, p) in plotted.iter().enumerate() {
        if p.role.kind != ChartType::Area {
            continue;
        }
        let m = map_of(p.role);
        let col = look.fill(series_color(chart, i));
        let base = m.base();
        let mut prev: Option<(f32, f32, f32)> = None;
        for j in 0..n {
            let Some((a, b)) = p.spans[j] else {
                if chart.blanks != Blanks::Span {
                    prev = None;
                }
                continue;
            };
            let x = cat_pos(j);
            tip(i, j, xy(horizontal, x, m.at(b)), b);
            let (lo, hi) = if a == 0.0 { (base, m.at(b)) } else { (m.at(a), m.at(b)) };
            if let Some((px, plo, phi)) = prev {
                if d3 > 0.0 {
                    // The top of the area going back, and its end.
                    let bd = deep * 0.7;
                    let (p0, p1) = (pos2(px, phi), pos2(x, hi));
                    c.poly(vec![p0, p1, p1 + bd, p0 + bd], mix(col, Color32::WHITE, 0.25));
                    if j + 1 == n || p.spans.get(j + 1).is_none_or(|s| s.is_none()) {
                        c.poly(vec![pos2(x, lo), p1, p1 + bd, pos2(x, lo) + bd], mix(col, Color32::BLACK, 0.25));
                    }
                }
                area_piece(c, horizontal, (px, plo, phi), (x, lo, hi), col);
            }
            prev = Some((x, lo, hi));
        }
        if let Some(lc) = chart.series.get(i).and_then(|s| s.line_color) {
            let pts: Vec<Pos2> = (0..n).filter_map(|j| p.spans[j].map(|(_, b)| xy(horizontal, cat_pos(j), m.at(b)))).collect();
            c.line(pts, Stroke::new(0.75 * z, rgb(lc)));
        }
    }
    // Columns and bars.
    let mut labels: Vec<(Pos2, Align2, String)> = Vec::new();
    let fmt_of = |r: Role| if r.secondary { &y2fmt } else { &yfmt };
    for &i in &col_members {
        let p = &plotted[i];
        let m = map_of(p.role);
        let s = chart.series.get(i);
        let no_fill = s.is_some_and(|s| s.no_fill);
        let outline = match s {
            Some(s) if s.no_line => None,
            Some(s) if s.line_color.is_some() => s.line_color.map(rgb),
            _ => look.outline,
        };
        let slot = slots[i];
        for j in 0..n {
            let Some((a, b)) = p.spans[j] else { continue };
            let center = cat_pos(j);
            let start = center - band / 2.0 + bar_w * gapf / 2.0 + slot as f32 * bar_w * (1.0 - ov);
            let (v0, v1) = if a == 0.0 && chart.kind != ChartType::Waterfall { (m.base(), m.at(b)) } else { (m.at(a), m.at(b)) };
            let r = if horizontal {
                // Bars stack their series up from the bottom, like the
                // categories.
                let bottom = center + band / 2.0 - bar_w * gapf / 2.0 - slot as f32 * bar_w * (1.0 - ov);
                Rect::from_x_y_ranges(v0.min(v1)..=v0.max(v1), bottom - bar_w..=bottom)
            } else {
                Rect::from_x_y_ranges(start..=start + bar_w, v0.min(v1)..=v0.max(v1))
            };
            // Thousands of categories: keep each bar a point thick so
            // they read as solid, not as faint hairlines.
            let r = if horizontal && r.height() < 1.0 {
                Rect::from_center_size(r.center(), vec2(r.width(), 1.0))
            } else if !horizontal && r.width() < 1.0 {
                Rect::from_center_size(r.center(), vec2(1.0, r.height()))
            } else {
                r
            };
            let r = r.intersect(plot.expand(1.0));
            if r.width() <= 0.0 || r.height() < 0.0 {
                continue;
            }
            let front = if d3 > 0.0 { r.translate(deep * 0.5) } else { r };
            tip(i, j, if horizontal { pos2(m.at(b), front.center().y) } else { pos2(front.center().x, m.at(b) + if d3 > 0.0 { -d3 * 0.5 } else { 0.0 }) }, b);
            let color = if chart.kind == ChartType::Waterfall {
                let total = chart.totals.contains(&(j as u32));
                auto_color(chart, if total { 2 } else if b >= a { 0 } else { 1 })
            } else {
                point_color(chart, i, j)
            };
            let fill = if no_fill { Color32::TRANSPARENT } else { look.fill(color) };
            if d3 > 0.0 {
                box3d(c, r.translate(deep * 0.15), deep * 0.7, fill, outline);
                if chart.labels.any() {
                    let v = values.get(i).and_then(|v| v.get(j).copied().flatten()).unwrap_or(b - a);
                    let text = label_text(c, chart, data, cats, i, j, v, fmt_of(p.role));
                    let r = r.translate(deep * 0.5);
                    let (at, al) = if horizontal { (pos2(r.max.x + 3.0 * z, r.center().y), Align2::LEFT_CENTER) } else { (pos2(r.center().x, r.min.y - 2.0 * z), Align2::CENTER_BOTTOM) };
                    labels.push((at, al, text));
                }
                continue;
            }
            match chart.series.get(i).and_then(|s| s.fancy.as_ref()).filter(|_| !no_fill && chart.kind != ChartType::Waterfall) {
                Some(f) => {
                    c.fancy_rect(r, f);
                    c.rect(r, Color32::TRANSPARENT, outline);
                }
                None => c.rect(r, fill, outline.or(if no_fill { Some(color) } else { None })),
            }
            if chart.labels.any() {
                let v = values.get(i).and_then(|v| v.get(j).copied().flatten()).unwrap_or(b - a);
                let text = label_text(c, chart, data, cats, i, j, v, fmt_of(p.role));
                let end = if horizontal { pos2(r.max.x + 3.0 * z, r.center().y) } else { pos2(r.center().x, r.min.y - 2.0 * z) };
                let neg = b < a;
                let (at, al) = match (horizontal, neg) {
                    (true, false) => (end, Align2::LEFT_CENTER),
                    (true, true) => (pos2(r.min.x - 3.0 * z, r.center().y), Align2::RIGHT_CENTER),
                    (false, false) => (end, Align2::CENTER_BOTTOM),
                    (false, true) => (pos2(r.center().x, r.max.y + 2.0 * z), Align2::CENTER_TOP),
                };
                labels.push((at, al, text));
            }
        }
        // Waterfall connectors.
        if chart.kind == ChartType::Waterfall && n > 1 {
            let s = Stroke::new(0.75 * z, look.axis);
            for j in 0..n - 1 {
                let (Some((_, b)), Some(_)) = (p.spans[j], p.spans[j + 1]) else { continue };
                let y = m.at(b);
                let x0 = cat_pos(j) - band / 2.0 + bar_w * gapf / 2.0 + bar_w;
                let x1 = cat_pos(j + 1) - band / 2.0 + bar_w * gapf / 2.0;
                c.line(vec![pos2(x0, y), pos2(x1, y)], s);
            }
        }
    }
    // Stock: high-low lines, close ticks or up-down bars.
    if chart.kind == ChartType::Stock && values.len() >= 3 {
        let (open, high, low, close) = if values.len() >= 4 { (Some(0), 1, 2, 3) } else { (None, 0, 1, 2) };
        let at = |i: usize, j: usize| values.get(i).and_then(|v| v.get(j).copied().flatten());
        let dark = mix(look.text, Color32::BLACK, 0.3);
        let tick = (band * 0.3).min(8.0 * z);
        for j in 0..n {
            let x = cat_pos(j);
            if let (Some(h), Some(l)) = (at(high, j), at(low, j)) {
                c.line(vec![pos2(x, ym.at(h)), pos2(x, ym.at(l))], Stroke::new(1.0 * z, dark));
            }
            match (open.and_then(|o| at(o, j)), at(close, j)) {
                (Some(o), Some(cl)) => {
                    let (y0, y1) = (ym.at(o), ym.at(cl));
                    let r = Rect::from_x_y_ranges(x - tick..=x + tick, y0.min(y1)..=y0.max(y1).max(y0.min(y1) + 1.0));
                    let fill = if cl >= o { look.bg } else { dark };
                    c.rect(r, fill, Some(dark));
                }
                (None, Some(cl)) => {
                    let y = ym.at(cl);
                    c.line(vec![pos2(x, y), pos2(x + tick, y)], Stroke::new(1.5 * z, dark));
                }
                _ => {}
            }
        }
    }
    // Bubbles, the largest first so the small ones stay visible.
    if chart.kind == ChartType::Bubble {
        let most = data.series.iter().flat_map(|s| s.sizes.iter().flatten()).fold(0.0f64, |a, b| a.max(b.abs()));
        let rmax = plot.width().min(plot.height()) * 0.125 * chart.bubble_scale as f32 / 100.0;
        let mut bubbles: Vec<(f32, Pos2, usize, usize)> = Vec::new();
        for (i, p) in plotted.iter().enumerate() {
            let m = map_of(p.role);
            let Some(d) = data.series.get(i) else { continue };
            for j in 0..n.min(5000) {
                let (Some((_, y)), Some(Some(x))) = (p.spans[j], d.xs.get(j)) else { continue };
                let size = d.sizes.get(j).copied().flatten().unwrap_or(0.0).abs();
                if most <= 0.0 || size <= 0.0 {
                    continue;
                }
                let r = rmax * (size / most).sqrt() as f32;
                let at = pos2(xm.map_or(cat_pos(j), |xm| xm.at(*x)), m.at(y));
                tip(i, j, at, y);
                bubbles.push((r, at, i, j));
            }
        }
        bubbles.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (r, at, i, j) in bubbles {
            let col = look.fill(point_color(chart, i, j));
            c.out.push(Shape::Circle(egui::epaint::CircleShape { center: at, radius: r.max(1.0), fill: col, stroke: Stroke::new(0.75 * z, mix(col, Color32::BLACK, 0.3)) }));
            if chart.labels.any() {
                let v = values.get(i).and_then(|v| v.get(j).copied().flatten()).unwrap_or(0.0);
                let text = label_text(c, chart, data, cats, i, j, v, fmt_of(Role { kind: ChartType::Bubble, secondary: false }));
                labels.push((at, Align2::CENTER_CENTER, text));
            }
        }
    }
    // Lines and scatter.
    for (i, p) in plotted.iter().enumerate() {
        if !matches!(p.role.kind, ChartType::Line | ChartType::Scatter) {
            continue;
        }
        let m = map_of(p.role);
        let s = chart.series.get(i);
        let col = series_color(chart, i);
        let lc = s.and_then(|s| s.line_color).map_or(col, rgb);
        let width = s.and_then(|s| s.line_width).map_or(look.line_width, |w| w * 1.333) * z;
        let smooth = s.is_some_and(|s| s.smooth) || chart.smooth;
        let pts: Vec<Option<Pos2>> = (0..n)
            .map(|j| {
                let (_, b) = p.spans[j]?;
                let v = m.at(b);
                Some(match &xm {
                    Some(xm) => {
                        let x = data.series.get(i)?.xs.get(j).copied().flatten()?;
                        pos2(xm.at(x), v)
                    }
                    None => xy(horizontal, cat_pos(j), v),
                })
            })
            .collect();
        for (j, q) in pts.iter().enumerate() {
            if let (Some(q), Some((_, b))) = (q, p.spans[j]) {
                tip(i, j, *q + if d3 > 0.0 && p.role.kind == ChartType::Line { deep * 0.5 } else { vec2(0.0, 0.0) }, b);
            }
        }
        let draw_line = !s.is_some_and(|s| s.no_line) && (p.role.kind == ChartType::Line || chart.lines);
        if d3 > 0.0 && p.role.kind == ChartType::Line {
            // A ribbon going back from the line.
            let bd = deep * 0.7;
            let (top, edge) = (mix(lc, Color32::WHITE, 0.2), mix(lc, Color32::BLACK, 0.25));
            for run in runs(&pts, chart.blanks == Blanks::Span) {
                for w in run.windows(2) {
                    let (p0, p1) = (w[0] + deep * 0.15, w[1] + deep * 0.15);
                    c.poly(vec![p0, p1, p1 + bd, p0 + bd], top);
                    c.line(vec![p0, p1], Stroke::new(0.75 * z, edge));
                }
            }
            continue;
        }
        if draw_line {
            for run in runs(&pts, chart.blanks == Blanks::Span) {
                let run = decimate_line(run, horizontal);
                let run = if smooth { smoothed(&run) } else { run };
                c.line(run, Stroke::new(width, lc));
            }
        }
        let marker = match s.map_or(Marker::Auto, |s| s.marker) {
            Marker::Auto if !chart.markers && (p.role.kind == ChartType::Line || chart.lines) => Marker::None,
            _ => series_marker(chart, i),
        };
        if marker != Marker::None {
            let fill = if s.is_some_and(|s| s.no_fill) { look.bg } else { col };
            let visible: Vec<Pos2> = pts.iter().flatten().copied().collect();
            // A marker on the same 1.5-pixel spot as one already drawn
            // adds nothing (large series stay cheap without losing points).
            let mut seen = std::collections::HashSet::new();
            let cell = 1.5 * z.max(0.1);
            for q in &visible {
                if plot.expand(2.0).contains(*q) && seen.insert(((q.x / cell) as i32, (q.y / cell) as i32)) {
                    c.marker(*q, marker, 6.0 * z, fill);
                }
            }
        }
        if chart.labels.any() {
            for (j, q) in pts.iter().enumerate() {
                let Some(q) = q else { continue };
                if n > 200 {
                    break;
                }
                let v = values.get(i).and_then(|v| v.get(j).copied().flatten()).unwrap_or(0.0);
                let text = label_text(c, chart, data, cats, i, j, v, fmt_of(p.role));
                labels.push((*q - vec2(0.0, 5.0 * z), Align2::CENTER_BOTTOM, text));
            }
        }
    }
    // Area labels.
    if chart.labels.any() {
        for (i, p) in plotted.iter().enumerate() {
            if p.role.kind != ChartType::Area || n > 200 {
                continue;
            }
            let m = map_of(p.role);
            for j in 0..n {
                let Some((a, b)) = p.spans[j] else { continue };
                let v = values.get(i).and_then(|v| v.get(j).copied().flatten()).unwrap_or(0.0);
                let text = label_text(c, chart, data, cats, i, j, v, fmt_of(p.role));
                labels.push((xy(horizontal, cat_pos(j), (m.at(a) + m.at(b)) / 2.0), Align2::CENTER_CENTER, text));
            }
        }
    }
    // Trendlines.
    for (i, s) in chart.series.iter().enumerate() {
        let Some(t) = s.trendline else { continue };
        let Some(p) = plotted.get(i) else { continue };
        let m = map_of(p.role);
        let ys: Vec<Option<f64>> = values.get(i).cloned().unwrap_or_default();
        let xs: Vec<Option<f64>> = match &xm {
            Some(_) => data.series.get(i).map(|d| d.xs.clone()).unwrap_or_default(),
            None => (1..=n).map(|k| Some(k as f64)).collect(),
        };
        let col = mix(series_color(chart, i), Color32::BLACK, 0.2);
        let stroke = Stroke::new(1.5 * z, col);
        let to_screen = |x: f64, y: f64| -> Pos2 {
            let v = m.at(y);
            match &xm {
                Some(xm) => pos2(xm.at(x), v),
                None => {
                    // Between categories: interpolate their positions.
                    let j = (x - 1.0).max(0.0);
                    let (a, b) = (cat_pos(j.floor() as usize), cat_pos((j.floor() as usize + 1).min(n.saturating_sub(1))));
                    let f = (j - j.floor()) as f32;
                    xy(horizontal, a + (b - a) * f, v)
                }
            }
        };
        let mut line: Vec<Pos2> = Vec::new();
        if let TrendKind::MovingAverage(k) = t.kind {
            let ma = apex_sheet::chart::moving_average(&ys, k as usize);
            for (x, y) in xs.iter().zip(ma) {
                if let (Some(x), Some(y)) = (x, y) {
                    line.push(to_screen(*x, y));
                }
            }
        } else if let Some(fit) = apex_sheet::chart::fit_trend(t.kind, &xs, &ys) {
            let (lo, hi) = xs.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(*x), b.max(*x)));
            if lo < hi {
                for k in 0..=80 {
                    let x = lo + (hi - lo) * k as f64 / 80.0;
                    if let Some(y) = fit.eval(x) {
                        let q = to_screen(x, y);
                        if plot.expand(1.0).contains(q) {
                            line.push(q);
                        }
                    }
                }
                let mut notes = Vec::new();
                if t.show_equation {
                    notes.push(fit.equation(c.loc));
                }
                if t.show_r2 {
                    let r2 = format!("{:.4}", fit.r2);
                    notes.push(format!("R² = {}", if c.loc.decimal == ',' { r2.replace('.', ",") } else { r2 }));
                }
                if !notes.is_empty() {
                    if let Some(last) = line.last() {
                        // Under the end of the line when it ends high, so
                        // the text stays in the plot.
                        let label = if last.y < plot.center().y { (*last + vec2(-4.0 * z, 10.0 * z), Align2::RIGHT_TOP) } else { (*last - vec2(4.0 * z, 6.0 * z), Align2::RIGHT_BOTTOM) };
                        labels.push((label.0, label.1, notes.join("\n")));
                    }
                }
            }
        }
        if line.len() >= 2 {
            c.out.extend(Shape::dashed_line(&line, stroke, 5.0 * z, 3.0 * z));
        }
    }

    // Error bars.
    for (i, s) in chart.series.iter().enumerate() {
        let (Some(e), Some(p), Some(raw)) = (&s.error_bars, plotted.get(i), values.get(i)) else { continue };
        let m = map_of(p.role);
        let col = mix(look.text, look.bg, 0.25);
        let stroke = Stroke::new(1.0 * z, col);
        let cap = 3.5 * z;
        for (j, ends) in err_ends[i].iter().copied().enumerate().take(5000) {
            let (Some((lo, hi)), Some(Some((at, b))), Some(Some(v))) = (ends, tips[i].get(j), raw.get(j)) else { continue };
            // Stacked values move the bar with the point.
            let shift = b - v;
            let (a0, a1) = (m.at(lo + shift), m.at(hi + shift));
            let (a0, a1) = if horizontal { (a0.clamp(plot.min.x, plot.max.x), a1.clamp(plot.min.x, plot.max.x)) } else { (a0.clamp(plot.min.y, plot.max.y), a1.clamp(plot.min.y, plot.max.y)) };
            let bar = |t: f32| if horizontal { pos2(t, at.y) } else { pos2(at.x, t) };
            c.line(vec![bar(a0), bar(a1)], stroke);
            if e.cap {
                let across = if horizontal { vec2(0.0, cap) } else { vec2(cap, 0.0) };
                for (t, on) in [(a0, e.dir != apex_sheet::chart::ErrorDir::Plus), (a1, e.dir != apex_sheet::chart::ErrorDir::Minus)] {
                    if on {
                        c.line(vec![bar(t) - across, bar(t) + across], stroke);
                    }
                }
            }
        }
    }

    // Axis lines: the category axis where the values cross zero.
    let axis_stroke = Stroke::new(0.75 * z, look.axis);
    if chart.x_axis.visible {
        let v = ym.base();
        if horizontal {
            c.line(vec![pos2(v, plot.min.y), pos2(v, plot.max.y)], axis_stroke);
        } else {
            c.line(vec![pos2(plot.min.x, v), pos2(plot.max.x, v)], axis_stroke);
        }
    }
    if scatter && chart.y_axis.visible {
        if let Some(xm) = &xm {
            let x = xm.base();
            c.line(vec![pos2(x, plot.min.y), pos2(x, plot.max.y)], axis_stroke);
        }
    }

    // Value axis labels.
    let text = look.text;
    for (v, t) in &vlabels {
        let at = ym.at(*v);
        if horizontal {
            c.text(pos2(at, plot.max.y + gap), Align2::CENTER_TOP, t, LABEL_PX, false, text);
        } else {
            c.text(pos2(plot.min.x - gap, at), Align2::RIGHT_CENTER, t, LABEL_PX, false, text);
        }
    }
    if let Some(m) = &y2m {
        for (v, t) in &v2labels {
            c.text(pos2(plot.max.x + gap, m.at(*v)), Align2::LEFT_CENTER, t, LABEL_PX, false, text);
        }
    }
    // Category (or x) labels, skipping some when crowded.
    match &xm {
        Some(m) => {
            for (v, t) in m.scale.ticks().into_iter().zip(&xlabels) {
                c.text(pos2(m.at(v), plot.max.y + gap), Align2::CENTER_TOP, t, LABEL_PX, false, text);
            }
        }
        None if !xlabels.is_empty() => {
            let each = if horizontal { label_h * 1.1 } else if rotate { label_h * 1.2 } else { widest_cat + 6.0 * z };
            let skip = ((each / band).ceil() as usize).max(1);
            for (j, t) in xlabels.iter().enumerate().step_by(skip) {
                let p = cat_pos(j);
                if horizontal {
                    c.text(pos2(plot.min.x - gap, p), Align2::RIGHT_CENTER, t, LABEL_PX, false, text);
                } else if rotate {
                    // 45 degrees, ending under the category.
                    let g = c.galley(t, LABEL_PX, false, text);
                    let w = g.size().x;
                    let a = -std::f32::consts::FRAC_PI_4;
                    let (s, co) = a.sin_cos();
                    let end = pos2(p, plot.max.y + gap);
                    let pos = end - vec2(w * co, w * s) + vec2(-label_h * 0.5 * s.abs(), 0.0);
                    c.out.push(egui::epaint::TextShape::new(pos, g, text).with_angle(a).into());
                } else {
                    c.text(pos2(p, plot.max.y + gap), Align2::CENTER_TOP, t, LABEL_PX, false, text);
                }
            }
        }
        None => {}
    }
    if table {
        data_table(c, chart, data, values, &roles, plot, name_w, row_h, key_w, band, &cat_pos, cats, [&yfmt, &y2fmt]);
    }
    // Axis titles.
    if let Some(t) = &left_title {
        c.text_up(pos2(area.min.x + title_h / 2.0, plot.center().y), t, AXIS_TITLE_PX, text);
    }
    if let Some(t) = &bottom_title {
        c.text(pos2(plot.center().x, area.max.y), Align2::CENTER_BOTTOM, t, AXIS_TITLE_PX, false, text);
    }
    if let Some(t) = &y2title {
        c.text_up(pos2(area.max.x - title_h / 2.0, plot.center().y), t, AXIS_TITLE_PX, text);
    }
    // Data labels last, on top.
    let ltext = if look.bg == Color32::WHITE || look.tint > 0.0 || look.plot_bg.is_some() { Color32::from_gray(64) } else { text };
    for (at, al, t) in labels.into_iter().take(5000) {
        c.text(at, al, &t, LABEL_PX, false, ltext);
    }
}

/// The data table under a category chart: the categories, then a row of
/// values per series.
#[allow(clippy::too_many_arguments)]
fn data_table(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], roles: &[Role], plot: Rect, name_w: f32, row_h: f32, key_w: f32, band: f32, cat_pos: &dyn Fn(usize) -> f32, cats: &[String], fmts: [&NumFormat; 2]) {
    let z = c.z;
    let look = c.look;
    let n = values.iter().map(|v| v.len()).max().unwrap_or(0);
    let top = plot.max.y;
    let bottom = top + row_h * (1 + values.len()) as f32;
    let x0 = plot.min.x - name_w;
    let rule = Stroke::new(0.75 * z, look.axis);
    c.line(vec![pos2(plot.min.x, top), pos2(plot.max.x, top)], rule);
    for r in 1..=values.len() {
        let y = top + row_h * r as f32;
        c.line(vec![pos2(x0, y), pos2(plot.max.x, y)], rule);
    }
    c.line(vec![pos2(x0, top + row_h), pos2(x0, bottom)], rule);
    for j in 0..=n {
        let x = if j < n { cat_pos(j) - band / 2.0 } else { cat_pos(n - 1) + band / 2.0 };
        c.line(vec![pos2(x, top), pos2(x, bottom)], rule);
    }
    let room = band - 4.0 * z;
    for j in 0..n {
        let t = elide(c, cats.get(j).map_or("", |s| s.as_str()), room);
        c.text(pos2(cat_pos(j), top + row_h / 2.0), Align2::CENTER_CENTER, &t, LABEL_PX, false, look.text);
    }
    for (i, vals) in values.iter().enumerate() {
        let y = top + row_h * (i as f32 + 1.5);
        let mut x = x0 + 4.0 * z;
        if key_w > 0.0 {
            let col = series_color(chart, i);
            let line = matches!(roles.get(i).map(|r| r.kind), Some(ChartType::Line));
            if line {
                c.line(vec![pos2(x, y), pos2(x + key_w - 4.0 * z, y)], Stroke::new(2.0 * z, col));
            } else {
                c.rect(Rect::from_center_size(pos2(x + (key_w - 4.0 * z) / 2.0, y), vec2(7.0 * z, 7.0 * z)), look.fill(col), None);
            }
            x += key_w;
        }
        let name = data.series.get(i).map_or("", |s| s.name.as_str());
        let t = elide(c, name, plot.min.x - x - 2.0 * z);
        c.text(pos2(x, y), Align2::LEFT_CENTER, &t, LABEL_PX, false, look.text);
        let fmt = if roles.get(i).is_some_and(|r| r.secondary) { fmts[1] } else { fmts[0] };
        for (j, v) in vals.iter().enumerate().take(n) {
            let Some(v) = v else { continue };
            let t = elide(c, &c.num(*v, fmt), room);
            c.text(pos2(cat_pos(j), y), Align2::CENTER_CENTER, &t, LABEL_PX, false, look.text);
        }
    }
}

/// A 3-D column: the front face `r`, and its top and side going `d` back.
fn box3d(c: &mut Canvas, r: Rect, d: Vec2, fill: Color32, outline: Option<Color32>) {
    if fill.a() > 0 {
        c.poly(vec![r.left_top(), r.right_top(), r.right_top() + d, r.left_top() + d], mix(fill, Color32::WHITE, 0.25));
        c.poly(vec![r.right_top(), r.right_bottom(), r.right_bottom() + d, r.right_top() + d], mix(fill, Color32::BLACK, 0.25));
    }
    c.rect(r, fill, outline);
    if let Some(o) = outline {
        c.line(vec![r.left_top(), r.left_top() + d, r.right_top() + d, r.right_bottom() + d, r.right_bottom()], Stroke::new(0.75 * c.z, o));
        c.line(vec![r.right_top(), r.right_top() + d], Stroke::new(0.75 * c.z, o));
    }
}

fn xy(horizontal: bool, cat: f32, val: f32) -> Pos2 {
    if horizontal { pos2(val, cat) } else { pos2(cat, val) }
}

/// One segment of an area: the band between (lo, hi) at two categories, in
/// convex pieces (split where it crosses its base).
fn area_piece(c: &mut Canvas, horizontal: bool, a: (f32, f32, f32), b: (f32, f32, f32), col: Color32) {
    let (x0, lo0, hi0) = a;
    let (x1, lo1, hi1) = b;
    let d0 = hi0 - lo0;
    let d1 = hi1 - lo1;
    if d0 * d1 < 0.0 {
        let t = d0 / (d0 - d1);
        let xm = x0 + (x1 - x0) * t;
        let ym = lo0 + (lo1 - lo0) * t;
        c.poly(vec![xy(horizontal, x0, lo0), xy(horizontal, x0, hi0), xy(horizontal, xm, ym)], col);
        c.poly(vec![xy(horizontal, xm, ym), xy(horizontal, x1, hi1), xy(horizontal, x1, lo1)], col);
    } else {
        c.poly(vec![xy(horizontal, x0, lo0), xy(horizontal, x0, hi0), xy(horizontal, x1, hi1), xy(horizontal, x1, lo1)], col);
    }
}

/// Unbroken runs of points (gaps split lines unless `span`).
fn runs(pts: &[Option<Pos2>], span: bool) -> Vec<Vec<Pos2>> {
    let mut out: Vec<Vec<Pos2>> = vec![Vec::new()];
    for p in pts {
        match p {
            Some(p) => {
                if let Some(r) = out.last_mut() {
                    r.push(*p);
                }
            }
            None if !span => {
                if out.last().is_some_and(|r| !r.is_empty()) {
                    out.push(Vec::new());
                }
            }
            None => {}
        }
    }
    out.retain(|r| r.len() >= 2);
    out
}

/// Keeps a line's look with fewer points: within each pixel column only
/// the first, lowest, highest and last points matter.
fn decimate_line(pts: Vec<Pos2>, horizontal: bool) -> Vec<Pos2> {
    if pts.len() < 1000 {
        return pts;
    }
    let key = |p: &Pos2| if horizontal { p.y.floor() } else { p.x.floor() };
    let val = |p: &Pos2| if horizontal { p.x } else { p.y };
    let mut out = Vec::with_capacity(pts.len() / 2);
    let mut i = 0;
    while i < pts.len() {
        let k = key(&pts[i]);
        let mut j = i;
        let (mut lo, mut hi) = (i, i);
        while j < pts.len() && key(&pts[j]) == k {
            if val(&pts[j]) < val(&pts[lo]) {
                lo = j;
            }
            if val(&pts[j]) > val(&pts[hi]) {
                hi = j;
            }
            j += 1;
        }
        let mut pick = vec![i, lo, hi, j - 1];
        pick.sort_unstable();
        pick.dedup();
        out.extend(pick.into_iter().map(|x| pts[x]));
        i = j;
    }
    out
}

/// Catmull-Rom curve through the points.
fn smoothed(pts: &[Pos2]) -> Vec<Pos2> {
    if pts.len() < 3 || pts.len() > 2000 {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(pts.len() * 8);
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        for k in 0..8 {
            let t = k as f32 / 8.0;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            out.push(pos2(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y)));
        }
    }
    if let Some(l) = pts.last() {
        out.push(*l);
    }
    out
}

#[allow(clippy::too_many_arguments)]
/// The text of a data label from the series' label range (empty cells show nothing).
fn cell_label(chart: &Chart, data: &ChartData, i: usize, j: usize) -> Option<String> {
    chart.labels.cells.then(|| data.series.get(i)?.label_texts.get(j).filter(|t| !t.is_empty()).cloned()).flatten()
}

fn label_text(c: &Canvas, chart: &Chart, data: &ChartData, cats: &[String], i: usize, j: usize, v: f64, fmt: &NumFormat) -> String {
    let mut parts: Vec<String> = cell_label(chart, data, i, j).into_iter().collect();
    if chart.labels.series {
        parts.push(data.series.get(i).map_or(String::new(), |s| s.name.clone()));
    }
    if chart.labels.category {
        parts.push(cats.get(j).cloned().unwrap_or_default());
    }
    if chart.labels.value || chart.labels.percent {
        parts.push(c.num(v, fmt));
    }
    parts.join(", ")
}

// --- sparklines -------------------------------------------------------------------------

/// Low and high value of a whole group (for shared axes).
pub fn spark_range(values: &[Vec<Option<f64>>]) -> Option<(f64, f64)> {
    let mut it = values.iter().flatten().flatten().copied();
    let first = it.next()?;
    Some(it.fold((first, first), |(a, b), v| (a.min(v), b.max(v))))
}

/// Paints one sparkline filling `rect`. `group` is the group's value range
/// (for shared axes).
pub fn paint_sparkline(p: &egui::Painter, rect: Rect, g: &SparkGroup, values: &[Option<f64>], group: Option<(f64, f64)>, z: f32) {
    let r = rect.shrink2(vec2(2.0 * z, 3.0 * z));
    if r.width() < 2.0 || r.height() < 2.0 {
        return;
    }
    let vals: Vec<Option<f64>> = if g.right_to_left { values.iter().rev().copied().collect() } else { values.to_vec() };
    let n = vals.len();
    let present: Vec<f64> = vals.iter().flatten().copied().collect();
    if present.is_empty() {
        return;
    }
    let (dlo, dhi) = present.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
    let pick = |a: SparkAxis, own: f64, grp: Option<f64>| match a {
        SparkAxis::Individual => own,
        SparkAxis::Group => grp.unwrap_or(own),
        SparkAxis::Custom(v) => v,
    };
    let mut lo = pick(g.min, dlo, group.map(|x| x.0));
    let mut hi = pick(g.max, dhi, group.map(|x| x.1));
    if g.kind == SparkKind::Column && g.min == SparkAxis::Individual {
        lo = lo.min(0.0);
    }
    if g.kind == SparkKind::Column && g.max == SparkAxis::Individual && hi < 0.0 {
        hi = 0.0;
    }
    if g.kind == SparkKind::WinLoss {
        (lo, hi) = (-1.0, 1.0);
    }
    if hi <= lo {
        // A custom minimum past the maximum shows them the other way.
        (lo, hi) = (hi.min(lo), hi.max(lo));
        let d = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        (lo, hi) = (lo - d, hi + d);
    }
    let y = |v: f64| r.max.y - ((v.max(lo).min(hi) - lo) / (hi - lo)) as f32 * r.height();
    let color = rgb(g.color);
    let (imax, imin) = {
        let mut a = (0, 0);
        let mut best = (f64::NEG_INFINITY, f64::INFINITY);
        for (i, v) in vals.iter().enumerate() {
            if let Some(v) = v {
                if *v > best.0 {
                    best.0 = *v;
                    a.0 = i;
                }
                if *v < best.1 {
                    best.1 = *v;
                    a.1 = i;
                }
            }
        }
        a
    };
    let first = vals.iter().position(Option::is_some).unwrap_or(0);
    let last = vals.iter().rposition(Option::is_some).unwrap_or(0);
    // Special points: the highest wins over the lowest, and so on (Excel's
    // order).
    let special = |i: usize, v: f64| -> Option<Color32> {
        if g.high && i == imax {
            Some(rgb(g.high_color))
        } else if g.low && i == imin {
            Some(rgb(g.low_color))
        } else if g.first && i == first {
            Some(rgb(g.first_color))
        } else if g.last && i == last {
            Some(rgb(g.last_color))
        } else if g.negative && v < 0.0 {
            Some(rgb(g.negative_color))
        } else {
            None
        }
    };
    let zero = y(0.0);
    if g.show_axis && lo < 0.0 && hi > 0.0 {
        p.line_segment([pos2(r.min.x, zero), pos2(r.max.x, zero)], Stroke::new(0.75 * z, Color32::BLACK));
    }
    match g.kind {
        SparkKind::Line => {
            let step = if n > 1 { r.width() / (n - 1) as f32 } else { 0.0 };
            let pts: Vec<Option<Pos2>> = vals.iter().enumerate().map(|(i, v)| v.map(|v| pos2(r.min.x + step * i as f32, y(v)))).collect();
            let w = (g.line_weight * 1.333 * z).max(0.75);
            for run in runs(&pts, g.blanks == Blanks::Span) {
                p.add(Shape::line(run, Stroke::new(w, color)));
            }
            for (i, v) in vals.iter().enumerate() {
                let (Some(v), Some(q)) = (v, pts[i]) else { continue };
                let c = special(i, *v).or(g.markers.then(|| rgb(g.marker_color)));
                if let Some(c) = c {
                    p.circle_filled(q, (w * 1.3).max(1.5 * z), c);
                }
            }
            if n == 1 {
                if let Some(q) = pts[0] {
                    p.circle_filled(q, 1.5 * z, color);
                }
            }
        }
        SparkKind::Column | SparkKind::WinLoss => {
            let band = r.width() / n.max(1) as f32;
            let bw = (band * 0.8).max(0.5);
            for (i, v) in vals.iter().enumerate() {
                let Some(v) = v else { continue };
                let v = if g.kind == SparkKind::WinLoss {
                    if *v > 0.0 {
                        1.0
                    } else if *v < 0.0 {
                        -1.0
                    } else {
                        continue;
                    }
                } else {
                    *v
                };
                let x = r.min.x + band * i as f32 + (band - bw) / 2.0;
                let base = if g.kind == SparkKind::WinLoss { r.center().y } else { zero.clamp(r.min.y, r.max.y) };
                let top = if g.kind == SparkKind::WinLoss { if v > 0.0 { r.min.y } else { r.max.y } } else { y(v) };
                let (a, b) = (base.min(top), base.max(top).max(base.min(top) + 0.75));
                let c = special(i, v).unwrap_or(if v < 0.0 && g.negative { rgb(g.negative_color) } else { color });
                p.rect_filled(Rect::from_x_y_ranges(x..=x + bw, a..=b), 0.0, c);
            }
        }
    }
}

// --- pictures of charts -----------------------------------------------------------------------

/// The last chart copied as a picture (PNG), for pastes that can't reach
/// the system clipboard.
static CLIP: Mutex<Option<Arc<[u8]>>> = Mutex::new(None);

pub fn set_app_clip(png: Option<Arc<[u8]>>) {
    if let Ok(mut c) = CLIP.lock() {
        *c = png;
    }
}

pub fn app_clip() -> Option<Arc<[u8]>> {
    CLIP.lock().ok().and_then(|c| c.clone())
}

/// The last chart copied from a sheet, as a chart object for documents:
/// the hash of its picture's pixels, its workbook and its size (points).
type ChartClip = (u64, Arc<[u8]>, (f32, f32));
static CHART_CLIP: Mutex<Option<ChartClip>> = Mutex::new(None);

pub fn set_chart_clip(clip: Option<ChartClip>) {
    if let Ok(mut c) = CHART_CLIP.lock() {
        *c = clip;
    }
}

/// The chart copied last (its workbook and size), when `picture` is the
/// picture it was copied as: a document pastes it as a live chart.
pub fn chart_clip_for(picture: &[u8]) -> Option<(Arc<[u8]>, (f32, f32))> {
    let (hash, book, size) = CHART_CLIP.lock().ok()?.clone()?;
    let d = crate::images::decode(picture)?;
    (crate::images::pixel_hash(&d.rgba) == hash).then_some((book, size))
}

/// Renders a chart to pixels, `w` x `h` sheet pixels at `scale` pixels
/// each, on white.
pub fn render(ctx: &egui::Context, chart: &Chart, data: &ChartData, w: f32, h: f32, scale: f32, loc: &Locale) -> egui::ColorImage {
    let (pw, ph) = ((w * scale).round().clamp(1.0, 4096.0) as usize, (h * scale).round().clamp(1.0, 4096.0) as usize);
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(pw as f32, ph as f32));
    let ppp = ctx.pixels_per_point();
    // Text is laid out for the screen's pixel density: lay it out in points
    // so the glyphs come out at `scale` pixels per sheet pixel.
    let shapes = build(ctx, Rect::from_min_size(Pos2::ZERO, rect.size() / ppp), chart, data, scale / ppp, loc);
    let clipped: Vec<egui::epaint::ClippedShape> = shapes.into_iter().map(|s| egui::epaint::ClippedShape { clip_rect: Rect::EVERYTHING, shape: s }).collect();
    let prims = ctx.tessellate(clipped, ppp);
    let atlas = ctx.fonts(|f| f.image());
    let mut img = egui::ColorImage::new([pw, ph], vec![Color32::WHITE; pw * ph]);
    for p in prims {
        if let egui::epaint::Primitive::Mesh(m) = p.primitive {
            if m.texture_id == egui::TextureId::default() {
                raster(&mut img, &m, &atlas, ppp);
            }
        }
    }
    img
}

/// Draws a mesh (positions in points, `ppp` pixels each) into `img`.
pub(crate) fn raster(img: &mut egui::ColorImage, m: &egui::Mesh, tex: &egui::ColorImage, ppp: f32) {
    let [w, h] = img.size;
    let [tw, th] = tex.size;
    if tw == 0 || th == 0 {
        return;
    }
    let sample = |uv: Pos2| -> [f32; 4] {
        let x = ((uv.x * tw as f32) as usize).min(tw - 1);
        let y = ((uv.y * th as f32) as usize).min(th - 1);
        let c = tex.pixels[y * tw + x];
        [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32].map(|v| v / 255.0)
    };
    for t in m.indices.chunks_exact(3) {
        let (Some(a), Some(b), Some(c)) = (m.vertices.get(t[0] as usize), m.vertices.get(t[1] as usize), m.vertices.get(t[2] as usize)) else { continue };
        let (pa, pb, pc) = (a.pos.to_vec2() * ppp, b.pos.to_vec2() * ppp, c.pos.to_vec2() * ppp);
        let area = (pb.x - pa.x) * (pc.y - pa.y) - (pc.x - pa.x) * (pb.y - pa.y);
        if area.abs() < 1e-6 {
            continue;
        }
        let x0 = pa.x.min(pb.x).min(pc.x).floor().max(0.0) as usize;
        let x1 = (pa.x.max(pb.x).max(pc.x).ceil().max(0.0) as usize).min(w);
        let y0 = pa.y.min(pb.y).min(pc.y).floor().max(0.0) as usize;
        let y1 = (pa.y.max(pb.y).max(pc.y).ceil().max(0.0) as usize).min(h);
        let col = |v: &egui::epaint::Vertex| [v.color.r() as f32, v.color.g() as f32, v.color.b() as f32, v.color.a() as f32].map(|x| x / 255.0);
        let (ca, cb, cc) = (col(a), col(b), col(c));
        for y in y0..y1 {
            for x in x0..x1 {
                let p = vec2(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((pb.x - p.x) * (pc.y - p.y) - (pc.x - p.x) * (pb.y - p.y)) / area;
                let w1 = ((pc.x - p.x) * (pa.y - p.y) - (pa.x - p.x) * (pc.y - p.y)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                let uv = pos2(a.uv.x * w0 + b.uv.x * w1 + c.uv.x * w2, a.uv.y * w0 + b.uv.y * w1 + c.uv.y * w2);
                let t = sample(uv);
                let s: [f32; 4] = std::array::from_fn(|k| (ca[k] * w0 + cb[k] * w1 + cc[k] * w2) * t[k]);
                let d = &mut img.pixels[y * w + x];
                let inv = 1.0 - s[3];
                let blend = |sv: f32, dv: u8| ((sv + dv as f32 / 255.0 * inv) * 255.0).round().clamp(0.0, 255.0) as u8;
                *d = Color32::from_rgba_premultiplied(blend(s[0], d.r()), blend(s[1], d.g()), blend(s[2], d.b()), blend(s[3], d.a()));
            }
        }
    }
}

/// PNG bytes of an opaque image.
pub fn encode_png(img: &egui::ColorImage) -> Option<Vec<u8>> {
    let [w, h] = img.size;
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w as u32, h as u32);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().ok()?;
        let data: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
        wr.write_image_data(&data).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::chart::{Axis, SeriesData};
    use apex_sheet::{CellRef, Chart};

    fn data(vals: &[&[f64]]) -> ChartData {
        ChartData {
            series: vals.iter().enumerate().map(|(i, v)| SeriesData { name: format!("S{i}"), values: v.iter().map(|x| Some(*x)).collect(), xs: (1..=v.len()).map(|k| Some(k as f64)).collect(), categories: Vec::new(), sizes: v.iter().map(|x| Some(x.abs())).collect(), err_plus: vec![Some(1.0); v.len()], err_minus: vec![Some(0.5); v.len()], label_texts: v.iter().map(|x| format!("L{x}")).collect() }).collect(),
            categories: (1..=vals.iter().map(|v| v.len()).max().unwrap_or(0)).map(|i| format!("C{i}")).collect(),
            value_format: "General".into(),
            cat_format: "General".into(),
            levels: Vec::new(),
        }
    }

    fn chart(kind: ChartType, n: usize) -> Chart {
        let mut c = Chart::new(kind, CellRef::new(0, 0), 400.0, 300.0);
        for _ in 0..n {
            c.series.push(Default::default());
        }
        c
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        ctx
    }

    #[test]
    fn every_type_builds_shapes() {
        let ctx = ctx();
        let d = data(&[&[1.0, -2.0, 3.0, 4.5], &[2.0, 2.0, 0.0, 1.0]]);
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
        for kind in ChartType::ALL {
            for (grouping, view3d) in [(Grouping::Clustered, false), (Grouping::Stacked, false), (Grouping::Percent, false), (Grouping::Clustered, true), (Grouping::Stacked, true)] {
                let mut c = chart(kind, 2);
                c.grouping = grouping;
                c.view3d = view3d && kind.has_3d();
                c.filled = view3d;
                c.labels.value = true;
                c.labels.percent = true;
                c.x_axis.title = "X".into();
                c.y_axis.title = "Y".into();
                c.series[0].trendline = Some(apex_sheet::chart::Trendline { kind: TrendKind::Linear, show_equation: true, show_r2: true });
                // Error bars, a data table and labels from cells.
                c.labels.cells = view3d;
                c.data_table = grouping == Grouping::Stacked;
                c.series[0].error_bars = Some(apex_sheet::chart::ErrorBars::default());
                c.series[1].error_bars = Some(apex_sheet::chart::ErrorBars { amount: apex_sheet::chart::ErrorAmount::Custom { plus: Default::default(), minus: Default::default() }, dir: apex_sheet::chart::ErrorDir::Plus, cap: true });
                let shapes = build(&ctx, rect, &c, &d, 1.0, &Locale::EN);
                assert!(shapes.len() > 5, "{kind:?}");
                // Everything stays inside the chart.
                for s in &shapes {
                    let b = s.visual_bounding_rect();
                    if b.is_positive() {
                        assert!(rect.expand(40.0).contains_rect(b), "{kind:?} {grouping:?} {view3d} {b:?}");
                    }
                }
            }
        }
        // Combo on a secondary axis, log axis, reversed, tiny, empty.
        let mut c = chart(ChartType::Column, 2);
        c.make_combo();
        c.y_axis = Axis { log_base: Some(10.0), reverse: true, ..Default::default() };
        assert!(build(&ctx, rect, &c, &d, 1.0, &Locale::EN).len() > 5);
        build(&ctx, Rect::from_min_size(Pos2::ZERO, vec2(3.0, 2.0)), &c, &d, 1.0, &Locale::EN);
        build(&ctx, rect, &chart(ChartType::Pie, 0), &ChartData::default(), 1.0, &Locale::EN);
        let nan = data(&[&[f64::NAN, f64::INFINITY]]);
        for kind in ChartType::ALL {
            build(&ctx, rect, &chart(kind, 1), &nan, 1.0, &Locale::EN);
        }
    }

    #[test]
    fn stacking_and_waterfall() {
        let c = chart(ChartType::Column, 2);
        let v = vec![vec![Some(1.0), Some(-2.0)], vec![Some(3.0), Some(-1.0)]];
        let r = [Role { kind: ChartType::Column, secondary: false }; 2];
        let mut s = c.clone();
        s.grouping = Grouping::Stacked;
        let p = stack(&s, &v, &r, 2);
        assert_eq!(p[1].spans[0], Some((1.0, 4.0)));
        assert_eq!(p[1].spans[1], Some((-2.0, -3.0)));
        let ra = [Role { kind: ChartType::Area, secondary: false }; 2];
        let p = stack(&s, &v, &ra, 2);
        assert_eq!(p[1].spans[1], Some((-2.0, -3.0)));
        let v2 = vec![vec![Some(5.0)], vec![Some(-2.0)]];
        assert_eq!(stack(&s, &v2, &ra, 1)[1].spans[0], Some((5.0, 3.0)));
        s.grouping = Grouping::Percent;
        let p = stack(&s, &v, &r, 2);
        assert_eq!(p[1].spans[0], Some((0.25, 1.0)));
        let mut w = chart(ChartType::Waterfall, 1);
        w.totals = vec![2];
        let p = stack(&w, &[vec![Some(5.0), Some(-2.0), None]], &r[..1], 3);
        assert_eq!(p[0].spans, [Some((0.0, 5.0)), Some((5.0, 3.0)), Some((0.0, 3.0))]);
    }

    #[test]
    fn lines_and_pictures() {
        let pts = [Some(pos2(0.0, 0.0)), Some(pos2(1.0, 1.0)), None, Some(pos2(3.0, 3.0)), Some(pos2(4.0, 4.0))];
        assert_eq!(runs(&pts, false).len(), 2);
        assert_eq!(runs(&pts, true).len(), 1);
        let many: Vec<Pos2> = (0..5000).map(|i| pos2(i as f32 / 50.0, (i % 7) as f32)).collect();
        let d = decimate_line(many, false);
        assert!(d.len() < 1000, "{}", d.len());
        let ctx = ctx();
        let mut c = chart(ChartType::Column, 1);
        c.title = "T".into();
        let img = render(&ctx, &c, &data(&[&[1.0, 3.0]]), 120.0, 80.0, 1.0, &Locale::EN);
        assert_eq!(img.size, [120, 80]);
        // Something other than white got drawn (the columns).
        let blue = palette_color(0, 0);
        assert!(img.pixels.iter().any(|p| p.r() < 100 && p.b() > 150 && (p.b() as i32 - blue.b() as i32).abs() < 30), "no column pixels");
        let png = encode_png(&img).unwrap();
        assert_eq!(crate::images::decode(&png).map(|d| (d.width, d.height)), Some((120, 80)));
    }
}
