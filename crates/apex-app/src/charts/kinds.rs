//! The chart types without the usual category and value axes: radar,
//! surface and contour, treemap, sunburst, funnel, box and whisker, 3-D pie,
//! and the data of a Pareto chart (drawn as columns with a line).

use apex_sheet::chart::{Axis, Series, SeriesData};

use super::*;

// --- Pareto --------------------------------------------------------------------------

/// A Pareto chart as what it draws: its categories added up and sorted
/// from the largest, as columns, and the cumulative percent as a line on
/// a secondary axis from 0 to 100 %.
pub(super) fn pareto(chart: &Chart, data: &ChartData) -> (Chart, ChartData) {
    let mut c = chart.clone();
    c.kind = ChartType::Column;
    c.view3d = false;
    c.grouping = Grouping::Clustered;
    c.markers = false;
    let Some(first) = data.series.first() else { return (c, data.clone()) };
    let mut pts: Vec<(String, f64)> = Vec::new();
    for (j, v) in first.values.iter().enumerate() {
        let name = data.categories.get(j).cloned().unwrap_or_default();
        let v = v.unwrap_or(0.0).max(0.0);
        match pts.iter_mut().find(|p| p.0 == name) {
            Some(p) => p.1 += v,
            None => pts.push((name, v)),
        }
    }
    pts.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = pts.iter().map(|p| p.1).sum();
    let mut run = 0.0;
    let cum: Vec<Option<f64>> = pts
        .iter()
        .map(|p| {
            run += p.1;
            Some(if total > 0.0 { run / total } else { 0.0 })
        })
        .collect();
    let bars = Series { kind: None, secondary: false, ..c.series.first().cloned().unwrap_or_default() };
    let line = Series { kind: Some(ChartType::Line), secondary: true, marker: Marker::None, color: c.series.get(1).and_then(|s| s.color), ..Default::default() };
    c.series = vec![bars, line];
    c.y2_axis = Axis { min: Some(0.0), max: Some(1.0), format: "0%".into(), title: chart.y2_axis.title.clone(), ..Default::default() };
    let n = pts.len();
    let cats: Vec<String> = pts.iter().map(|p| p.0.clone()).collect();
    let xs: Vec<Option<f64>> = (1..=n).map(|i| Some(i as f64)).collect();
    let d = ChartData {
        series: vec![
            SeriesData { name: first.name.clone(), values: pts.iter().map(|p| Some(p.1)).collect(), xs: xs.clone(), categories: cats.clone(), sizes: Vec::new(), ..Default::default() },
            SeriesData { name: tr("chart.cumulative").into(), values: cum, xs, categories: cats.clone(), sizes: Vec::new(), ..Default::default() },
        ],
        levels: cats.iter().map(|c| vec![c.clone()]).collect(),
        categories: cats,
        value_format: data.value_format.clone(),
        cat_format: data.cat_format.clone(),
    };
    (c, d)
}

// --- shared pieces ---------------------------------------------------------------------

/// Color of category `j` in the charts that color each one (a color set
/// on the point, else the next automatic color).
pub(super) fn category_color(chart: &Chart, j: usize) -> Color32 {
    match chart.series.first().and_then(|s| s.point_colors.iter().find(|(k, _)| *k as usize == j)) {
        Some((_, c)) => rgb(*c),
        None => auto_color(chart, j),
    }
}

/// Aligns text placed at angle `a` (clockwise from the top) outside a circle.
fn outside_align(a: f32) -> Align2 {
    let (s, c) = (a.sin(), a.cos());
    let h = if s > 0.2 {
        egui::Align::Min
    } else if s < -0.2 {
        egui::Align::Max
    } else {
        egui::Align::Center
    };
    let v = if c > 0.2 {
        egui::Align::Max
    } else if c < -0.2 {
        egui::Align::Min
    } else {
        egui::Align::Center
    };
    Align2([h, v])
}

/// A ring sector (a slice when `r0` is 0) as one mesh, with smooth edges.
fn sector(c: &mut Canvas, center: Pos2, r0: f32, r1: f32, a0: f32, sweep: f32, col: Color32) {
    if sweep <= 0.0 || r1 <= r0 {
        return;
    }
    let steps = ((sweep / 0.05).ceil() as usize).clamp(1, 1440);
    let at = |r: f32, k: usize| arc_point(center, r, a0 + sweep * k as f32 / steps as f32);
    let mut mesh = egui::epaint::Mesh::default();
    for k in 0..=steps {
        mesh.colored_vertex(at(r1, k), col);
        mesh.colored_vertex(if r0 > 0.0 { at(r0, k) } else { center }, col);
    }
    for k in 0..steps as u32 {
        let i = 2 * k;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    c.out.push(Shape::mesh(mesh));
    c.line((0..=steps).map(|k| at(r1, k)).collect(), Stroke::new(0.8, col));
}

/// A hierarchy of categories (treemap and sunburst).
#[derive(Default)]
struct Node {
    name: String,
    value: f64,
    children: Vec<Node>,
    /// Point of a leaf (for its color and label).
    point: Option<usize>,
}

impl Node {
    fn depth(&self) -> usize {
        self.children.iter().map(|c| c.depth() + 1).max().unwrap_or(0)
    }
}

/// The tree of the points with a positive value, by their category levels.
fn tree(data: &ChartData, values: &[Option<f64>]) -> Node {
    let mut root = Node::default();
    for (j, v) in values.iter().enumerate() {
        let v = v.unwrap_or(0.0);
        if v <= 0.0 || !v.is_finite() {
            continue;
        }
        let path: Vec<String> = match data.levels.get(j) {
            Some(p) if !p.is_empty() => p.clone(),
            _ => vec![data.categories.get(j).cloned().unwrap_or_default()],
        };
        let mut node = &mut root;
        node.value += v;
        for (k, name) in path.iter().enumerate() {
            let leaf = k + 1 == path.len();
            // Consecutive points of the same parent share it; a leaf is
            // always its own.
            let found = node.children.iter().rposition(|c| !leaf && c.name == *name && c.point.is_none());
            let i = match found {
                Some(i) => i,
                None => {
                    node.children.push(Node { name: name.clone(), ..Default::default() });
                    node.children.len() - 1
                }
            };
            node = &mut node.children[i];
            node.value += v;
            if leaf {
                node.point = Some(j);
            }
        }
    }
    root
}

/// The first level of the categories (legend of a treemap or sunburst).
pub(super) fn top_groups(data: &ChartData) -> Vec<String> {
    let values = data.series.first().map(|s| s.values.clone()).unwrap_or_default();
    tree(data, &values).children.into_iter().map(|n| n.name).collect()
}

/// Squarified layout of `sizes` (largest first works best) in `r`.
fn squarify(sizes: &[f64], r: Rect) -> Vec<Rect> {
    let total: f64 = sizes.iter().sum();
    let mut out = vec![Rect::NOTHING; sizes.len()];
    if total <= 0.0 || r.width() <= 0.0 || r.height() <= 0.0 {
        return out;
    }
    let scale = (r.width() * r.height()) as f64 / total;
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|a, b| sizes[*b].total_cmp(&sizes[*a]));
    let mut rest = r;
    let mut i = 0;
    while i < order.len() {
        let short = rest.width().min(rest.height()) as f64;
        let worst = |row: &[usize]| -> f64 {
            let s: f64 = row.iter().map(|&k| sizes[k] * scale).sum();
            let (mx, mn) = row.iter().map(|&k| sizes[k] * scale).fold((0.0f64, f64::INFINITY), |(a, b), v| (a.max(v), b.min(v)));
            if s <= 0.0 || mn <= 0.0 {
                return f64::INFINITY;
            }
            (short * short * mx / (s * s)).max(s * s / (short * short * mn))
        };
        let mut j = i + 1;
        while j < order.len() && worst(&order[i..=j]) <= worst(&order[i..j]) {
            j += 1;
        }
        let row = &order[i..j];
        let s: f64 = row.iter().map(|&k| sizes[k] * scale).sum();
        let last = j >= order.len();
        if rest.width() >= rest.height() {
            let w = if last { rest.width() } else { (s / rest.height() as f64) as f32 };
            let mut y = rest.min.y;
            for (n, &k) in row.iter().enumerate() {
                let h = if n + 1 == row.len() { rest.max.y - y } else { (sizes[k] * scale / w as f64) as f32 };
                out[k] = Rect::from_min_size(pos2(rest.min.x, y), vec2(w, h));
                y += h;
            }
            rest.min.x += w;
        } else {
            let h = if last { rest.height() } else { (s / rest.width() as f64) as f32 };
            let mut x = rest.min.x;
            for (n, &k) in row.iter().enumerate() {
                let w = if n + 1 == row.len() { rest.max.x - x } else { (sizes[k] * scale / h as f64) as f32 };
                out[k] = Rect::from_min_size(pos2(x, rest.min.y), vec2(w, h));
                x += w;
            }
            rest.min.y += h;
        }
        i = j;
    }
    out
}

/// Text color that reads on `bg`.
fn on(bg: Color32) -> Color32 {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if l > 150.0 { Color32::from_gray(40) } else { Color32::WHITE }
}

/// The label of a leaf: its category and, as the labels say, its value.
fn leaf_label(c: &Canvas, chart: &Chart, name: &str, v: f64, fmt: &NumFormat) -> String {
    if chart.labels.value { format!("{name}\n{}", c.num(v, fmt)) } else { name.to_string() }
}

// --- treemap -----------------------------------------------------------------------------

pub(super) fn treemap(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], area: Rect) {
    let Some(vals) = values.first() else { return };
    let root = tree(data, vals);
    let fmt = format_for("", &data.value_format);
    let sizes: Vec<f64> = root.children.iter().map(|n| n.value).collect();
    let rects = squarify(&sizes, area);
    let sep = c.look.bg;
    let label_h = c.text_size("Ag", LABEL_PX).y;
    let z = c.z;
    for (k, (node, r)) in root.children.iter().zip(rects).enumerate() {
        let col = c.look.fill(category_color(chart, k));
        if !r.is_positive() {
            continue;
        }
        let mut inner = r;
        if !node.children.is_empty() {
            // A parent: a banner with its name over its leaves.
            c.rect(r, col, None);
            if r.height() > label_h * 3.0 && r.width() > 20.0 * z {
                let t = elide(c, &node.name, r.width() - 8.0 * z);
                c.text(r.min + vec2(4.0 * z, 2.0 * z), Align2::LEFT_TOP, &t, LABEL_PX, true, on(col));
                inner.min.y += label_h + 4.0 * z;
            }
            treemap_level(c, chart, node, inner, col, &fmt, 1);
        } else {
            c.rect(r, col, None);
            leaf_text(c, chart, &node.name, node.value, r, col, &fmt);
        }
        c.out.push(Shape::rect_stroke(r, 0.0, Stroke::new(1.5 * z, sep), egui::StrokeKind::Inside));
    }
}

fn treemap_level(c: &mut Canvas, chart: &Chart, node: &Node, r: Rect, col: Color32, fmt: &NumFormat, depth: usize) {
    let sizes: Vec<f64> = node.children.iter().map(|n| n.value).collect();
    let z = c.z;
    for (child, cr) in node.children.iter().zip(squarify(&sizes, r)) {
        if !cr.is_positive() {
            continue;
        }
        if child.children.is_empty() || depth > 12 {
            leaf_text(c, chart, &child.name, child.value, cr, col, fmt);
        } else {
            treemap_level(c, chart, child, cr, col, fmt, depth + 1);
        }
        c.out.push(Shape::rect_stroke(cr, 0.0, Stroke::new(0.75 * z, c.look.bg), egui::StrokeKind::Inside));
    }
}

fn leaf_text(c: &mut Canvas, chart: &Chart, name: &str, v: f64, r: Rect, col: Color32, fmt: &NumFormat) {
    let z = c.z;
    if r.width() < 24.0 * z || r.height() < 14.0 * z {
        return;
    }
    let text = leaf_label(c, chart, name, v, fmt);
    let lines: Vec<String> = text.lines().map(|l| elide(c, l, r.width() - 8.0 * z)).collect();
    let mut y = r.min.y + 3.0 * z;
    for l in lines {
        let h = c.text_size(&l, LABEL_PX).y;
        if y + h > r.max.y - 2.0 * z {
            break;
        }
        c.text(pos2(r.min.x + 4.0 * z, y), Align2::LEFT_TOP, &l, LABEL_PX, false, on(col));
        y += h;
    }
}

// --- sunburst ----------------------------------------------------------------------------

pub(super) fn sunburst(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], area: Rect) {
    let Some(vals) = values.first() else { return };
    let root = tree(data, vals);
    let depth = root.depth().max(1);
    let radius = area.width().min(area.height()) / 2.0 * 0.95;
    if radius < 4.0 || root.value <= 0.0 {
        return;
    }
    let hole = radius * 0.12;
    let ring = (radius - hole) / depth as f32;
    let center = area.center();
    let fmt = format_for("", &data.value_format);
    let mut labels: Vec<(Pos2, String, Color32, f32, f32)> = Vec::new();
    let mut seps: Vec<(f32, f32, f32)> = Vec::new();
    let tau = std::f32::consts::TAU;
    #[allow(clippy::too_many_arguments)]
    fn walk(c: &mut Canvas, chart: &Chart, node: &Node, a0: f32, sweep: f32, level: usize, base: Option<Color32>, k: usize, g: &mut (Pos2, f32, f32, &NumFormat, &mut Vec<(Pos2, String, Color32, f32, f32)>, &mut Vec<(f32, f32, f32)>)) {
        let (center, hole, ring) = (g.0, g.1, g.2);
        let mut a = a0;
        for (i, child) in node.children.iter().enumerate() {
            let s = if node.value > 0.0 { (child.value / node.value) as f32 * sweep } else { 0.0 };
            if s <= 0.0 {
                continue;
            }
            let top = base.is_none();
            let col = match base {
                None => c.look.fill(category_color(chart, k + i)),
                Some(b) => mix(b, Color32::WHITE, 0.18),
            };
            let (r0, r1) = (hole + ring * level as f32, hole + ring * (level + 1) as f32);
            sector(c, center, r0, r1, a, s, col);
            g.5.push((a, r0, r1));
            let text = if child.children.is_empty() { leaf_label(c, chart, &child.name, child.value, g.3) } else { child.name.clone() };
            g.4.push((arc_point(center, (r0 + r1) / 2.0, a + s / 2.0), text, col, (r0 + r1) / 2.0 * s, ring));
            walk(c, chart, child, a, s, level + 1, Some(col), if top { i } else { k }, g);
            a += s;
        }
    }
    let mut g = (center, hole, ring, &fmt, &mut labels, &mut seps);
    walk(c, chart, &root, (chart.first_angle as f32).to_radians(), tau, 0, None, 0, &mut g);
    let sep = c.look.bg;
    for (a, r0, r1) in seps {
        c.line(vec![arc_point(center, r0, a), arc_point(center, r1, a)], Stroke::new(1.0 * c.z, sep));
    }
    for d in 0..=depth {
        let r = hole + ring * d as f32;
        c.out.push(Shape::circle_stroke(center, r, Stroke::new(1.0 * c.z, sep)));
    }
    // Names that fit their piece.
    for (at, t, col, arc, ring) in labels {
        let size = t.lines().map(|l| c.text_size(l, LABEL_PX)).fold(Vec2::ZERO, |a, s| vec2(a.x.max(s.x), a.y + s.y));
        if size.x < arc * 0.9 && size.y < ring * 0.9 && size.x < ring * 1.6 {
            let g = c.galley(&t, LABEL_PX, false, on(col));
            let r = Rect::from_center_size(at, g.size());
            c.out.push(Shape::galley(r.min, g, on(col)));
        }
    }
}

// --- funnel ------------------------------------------------------------------------------

pub(super) fn funnel(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let Some(vals) = values.first() else { return };
    let n = vals.len();
    if n == 0 {
        return;
    }
    let z = c.z;
    let gap = 6.0 * z;
    let fmt = format_for(&chart.y_axis.format, &data.value_format);
    let names: Vec<String> = (0..n).map(|j| cats.get(j).cloned().unwrap_or_default()).collect();
    let left = if chart.x_axis.visible { names.iter().map(|t| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max).min(area.width() * 0.35) } else { 0.0 };
    let plot = Rect::from_min_max(pos2(area.min.x + left + if left > 0.0 { gap } else { 0.0 }, area.min.y), area.max);
    let most = vals.iter().flatten().fold(0.0f64, |a, v| a.max(v.abs()));
    let band = plot.height() / n as f32;
    let h = band / (1.0 + chart.gap as f32 / 100.0);
    for (j, v) in vals.iter().enumerate() {
        let y = plot.min.y + band * j as f32 + band / 2.0;
        if chart.x_axis.visible {
            let t = elide(c, &names[j], left);
            let color = c.look.text;
            c.text(pos2(plot.min.x - gap, y), Align2::RIGHT_CENTER, &t, LABEL_PX, false, color);
        }
        let Some(v) = v else { continue };
        let w = if most > 0.0 { (v.abs() / most) as f32 * plot.width() } else { 0.0 };
        let r = Rect::from_center_size(pos2(plot.center().x, y), vec2(w.max(1.0), h));
        let col = c.look.fill(point_color(chart, 0, j));
        c.rect(r, col, c.look.outline);
        if chart.labels.any() {
            let t = label_text(c, chart, data, cats, 0, j, *v, &fmt);
            let tc = if c.text_size(&t, LABEL_PX).x < r.width() - 4.0 * z { on(col) } else { c.look.text };
            c.text(r.center(), Align2::CENTER_CENTER, &t, LABEL_PX, false, tc);
        }
    }
}

// --- Marimekko ---------------------------------------------------------------------------

/// Columns as wide as their total (the first series), split into segments
/// (the other series) that fill the height as shares of the column. With one
/// series the columns are equally wide.
pub(super) fn mekko(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let Some(first) = values.first() else { return };
    let (widths, segs): (Vec<f64>, &[Vec<Option<f64>>]) = if values.len() > 1 { (first.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect(), &values[1..]) } else { (vec![1.0; first.len()], values) };
    let n = widths.len();
    let total: f64 = widths.iter().sum();
    if n == 0 || total <= 0.0 {
        return;
    }
    let z = c.z;
    let gap = 4.0 * z;
    let text = c.look.text;
    let axis_w = c.text_size("100%", LABEL_PX).x + gap;
    let cat_h = c.text_size("Ag", LABEL_PX).y + gap;
    let plot = Rect::from_min_max(pos2(area.min.x + axis_w, area.min.y + cat_h * 0.5), pos2(area.max.x, area.max.y - cat_h));
    if plot.width() < 10.0 || plot.height() < 10.0 {
        return;
    }
    for k in 0..=4 {
        let y = plot.max.y - plot.height() * k as f32 / 4.0;
        c.line(vec![pos2(plot.min.x, y), pos2(plot.max.x, y)], Stroke::new(0.75 * z, c.look.grid));
        c.text(pos2(plot.min.x - gap, y), Align2::RIGHT_CENTER, &format!("{}%", k * 25), LABEL_PX, false, text);
    }
    let mut x = plot.min.x;
    for (j, w) in widths.iter().enumerate() {
        let cw = (*w / total) as f32 * plot.width();
        if cw <= 0.0 {
            continue;
        }
        let sum: f64 = segs.iter().map(|s| s.get(j).copied().flatten().unwrap_or(0.0).max(0.0)).sum();
        let mut y = plot.max.y;
        if sum > 0.0 {
            for (i, s) in segs.iter().enumerate() {
                let v = s.get(j).copied().flatten().unwrap_or(0.0).max(0.0);
                if v <= 0.0 {
                    continue;
                }
                let h = (v / sum) as f32 * plot.height();
                let r = Rect::from_min_max(pos2(x, y - h), pos2(x + cw, y));
                let col = c.look.fill(series_color(chart, i + usize::from(values.len() > 1)));
                c.rect(r, col, c.look.outline.or(Some(c.look.bg)));
                if chart.labels.any() {
                    let t = format!("{:.0}%", v / sum * 100.0);
                    if c.text_size(&t, LABEL_PX).x < r.width() - 2.0 * z && c.text_size(&t, LABEL_PX).y < r.height() {
                        c.text(r.center(), Align2::CENTER_CENTER, &t, LABEL_PX, false, on(col));
                    }
                }
                y -= h;
            }
        }
        let name = elide(c, cats.get(j).map_or("", |s| s.as_str()), cw - 2.0 * z);
        c.text(pos2(x + cw / 2.0, plot.max.y + gap), Align2::CENTER_TOP, &name, LABEL_PX, false, text);
        x += cw;
    }
    let _ = data;
}

// --- timeline ----------------------------------------------------------------------------

/// Events (date serial, label) of a timeline sorted by date; blank dates
/// are left out.
pub(super) fn timeline_events(values: &[Option<f64>], cats: &[String]) -> Vec<(f64, String)> {
    let mut ev: Vec<(f64, String)> = values.iter().enumerate().filter_map(|(j, v)| Some(((*v).filter(|v| v.is_finite())?, cats.get(j).cloned().unwrap_or_default()))).collect();
    ev.sort_by(|a, b| a.0.total_cmp(&b.0));
    ev
}

/// Milestones along a horizontal axis, alternating above and below it on
/// stems of two lengths so that neighbours don't overlap.
pub(super) fn timeline(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let Some(vals) = values.first() else { return };
    let ev = timeline_events(vals, cats);
    if ev.is_empty() {
        return;
    }
    let z = c.z;
    let fmt = {
        let f = format_for(&chart.y_axis.format, &data.value_format);
        if f.is_date() { f } else { NumFormat::parse("yyyy-mm-dd") }
    };
    let (lo, hi) = (ev[0].0, ev[ev.len() - 1].0);
    let margin = (area.width() * 0.08).min(60.0 * z);
    let (x0, x1) = (area.min.x + margin, area.max.x - margin);
    let axis_y = area.center().y;
    let text = c.look.text;
    c.line(vec![pos2(area.min.x, axis_y), pos2(area.max.x, axis_y)], Stroke::new(1.5 * z, c.look.axis));
    let half = area.height() / 2.0;
    for (i, (d, label)) in ev.iter().enumerate() {
        let x = if hi > lo { x0 + ((d - lo) / (hi - lo)) as f32 * (x1 - x0) } else { area.center().x };
        let above = i % 2 == 0;
        let stem = half * if (i / 2) % 2 == 0 { 0.4 } else { 0.68 };
        let sign = if above { -1.0 } else { 1.0 };
        let col = series_color(chart, 0);
        let end = pos2(x, axis_y + sign * stem);
        c.line(vec![pos2(x, axis_y), end], Stroke::new(1.0 * z, c.look.grid));
        c.marker(pos2(x, axis_y), Marker::Circle, 9.0 * z, col);
        let room = (area.width() / (ev.len() as f32 / 2.0).max(1.0)).clamp(70.0 * z, 200.0 * z);
        let day = c.num(*d, &fmt);
        let name = elide(c, label, room);
        let (a_name, a_day) = if above { (Align2::CENTER_BOTTOM, Align2::CENTER_BOTTOM) } else { (Align2::CENTER_TOP, Align2::CENTER_TOP) };
        let line_h = c.text_size("Ag", LABEL_PX).y;
        let gap = 2.0 * z;
        if above {
            c.text(pos2(x, end.y - gap), a_day, &day, LABEL_PX, false, c.look.axis);
            c.text(pos2(x, end.y - gap - line_h), a_name, &name, LABEL_PX, true, text);
        } else {
            c.text(pos2(x, end.y + gap), a_name, &name, LABEL_PX, true, text);
            c.text(pos2(x, end.y + gap + line_h), a_day, &day, LABEL_PX, false, c.look.axis);
        }
    }
}

// --- box and whisker ---------------------------------------------------------------------

/// Quartile `p` (0.25, 0.5, 0.75) of sorted values, exclusive method
/// (Excel's default), falling back to the ends for few values.
fn quartile(xs: &[f64], p: f64) -> f64 {
    let n = xs.len();
    if n == 1 {
        return xs[0];
    }
    let h = (p * (n as f64 + 1.0)).clamp(1.0, n as f64);
    let lo = h.floor() as usize;
    let f = h - lo as f64;
    let a = xs[lo - 1];
    let b = xs[lo.min(n - 1)];
    a + (b - a) * f
}

/// The five numbers, the mean and the outliers of a set of values.
#[derive(Debug, PartialEq)]
struct BoxStats {
    q1: f64,
    median: f64,
    q3: f64,
    low: f64,
    high: f64,
    mean: f64,
    outliers: Vec<f64>,
}

fn box_stats(mut xs: Vec<f64>) -> Option<BoxStats> {
    xs.retain(|v| v.is_finite());
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    let (q1, median, q3) = (quartile(&xs, 0.25), quartile(&xs, 0.5), quartile(&xs, 0.75));
    let iqr = q3 - q1;
    let (lf, hf) = (q1 - 1.5 * iqr, q3 + 1.5 * iqr);
    let inside: Vec<f64> = xs.iter().copied().filter(|v| *v >= lf && *v <= hf).collect();
    let low = inside.first().copied().unwrap_or(q1);
    let high = inside.last().copied().unwrap_or(q3);
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    let outliers = xs.into_iter().filter(|v| *v < lf || *v > hf).collect();
    Some(BoxStats { q1, median, q3, low, high, mean, outliers })
}

pub(super) fn box_whisker(c: &mut Canvas, chart: &Chart, data: &ChartData, area: Rect) {
    let z = c.z;
    let look = c.look;
    // Points group by their category; without categories, one group.
    let by_cat = chart.series.iter().any(|s| !s.categories.is_empty());
    let mut groups: Vec<String> = Vec::new();
    let mut sets: Vec<Vec<Vec<f64>>> = Vec::new();
    for (i, s) in data.series.iter().enumerate() {
        for (j, v) in s.values.iter().enumerate() {
            let Some(v) = v else { continue };
            let g = if by_cat { s.categories.get(j).or(data.categories.get(j)).cloned().unwrap_or_default() } else { String::new() };
            let k = match groups.iter().position(|x| *x == g) {
                Some(k) => k,
                None => {
                    groups.push(g);
                    sets.push(vec![Vec::new(); data.series.len()]);
                    groups.len() - 1
                }
            };
            sets[k][i].push(*v);
        }
    }
    if groups.is_empty() {
        return;
    }
    let stats: Vec<Vec<Option<BoxStats>>> = sets.into_iter().map(|g| g.into_iter().map(box_stats).collect()).collect();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for s in stats.iter().flatten().flatten() {
        lo = lo.min(s.low).min(s.outliers.first().copied().unwrap_or(s.low));
        hi = hi.max(s.high).max(s.outliers.last().copied().unwrap_or(s.high));
    }
    if lo > hi {
        return;
    }
    let gap = 4.0 * z;
    let label_h = c.text_size("0", LABEL_PX).y;
    let ticks = ((area.height() / (36.0 * z)).floor() as usize).clamp(2, 12);
    let scale = apex_sheet::chart::scale_for(lo, hi, ticks, &chart.y_axis);
    let fmt = format_for(&chart.y_axis.format, &data.value_format);
    let vlabels: Vec<(f64, String)> = if chart.y_axis.visible { scale.ticks().into_iter().map(|v| (v, c.num(v, &fmt))).collect() } else { Vec::new() };
    let left = vlabels.iter().map(|(_, t)| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max);
    let mut plot = area;
    plot.min.x += left + gap;
    plot.min.y += label_h / 2.0;
    if by_cat && chart.x_axis.visible {
        plot.max.y -= label_h + gap;
    }
    if plot.width() < 4.0 || plot.height() < 4.0 {
        return;
    }
    c.area(plot, &chart.plot, look.plot_bg, None);
    let m = Map { scale, a: plot.max.y, b: plot.min.y, reverse: chart.y_axis.reverse };
    if chart.y_axis.major_grid {
        for (v, _) in &vlabels {
            c.line(vec![pos2(plot.min.x, m.at(*v)), pos2(plot.max.x, m.at(*v))], Stroke::new(0.75 * z, look.grid));
        }
    }
    for (v, t) in &vlabels {
        c.text(pos2(plot.min.x - gap, m.at(*v)), Align2::RIGHT_CENTER, t, LABEL_PX, false, look.text);
    }
    let band = plot.width() / groups.len() as f32;
    let k = data.series.len().max(1);
    let gapf = chart.gap as f32 / 100.0;
    let w = band / (k as f32 + gapf);
    let dark = mix(look.text, Color32::BLACK, 0.3);
    for (gi, row) in stats.iter().enumerate() {
        let x0 = plot.min.x + band * gi as f32 + w * gapf / 2.0;
        if by_cat && chart.x_axis.visible {
            let t = elide(c, &groups[gi], band);
            c.text(pos2(plot.min.x + band * (gi as f32 + 0.5), plot.max.y + gap), Align2::CENTER_TOP, &t, LABEL_PX, false, look.text);
        }
        for (si, st) in row.iter().enumerate() {
            let Some(st) = st else { continue };
            let col = look.fill(series_color(chart, si));
            let edge = mix(col, Color32::BLACK, 0.35);
            let x = x0 + w * si as f32;
            let r = Rect::from_x_y_ranges(x + w * 0.1..=x + w * 0.9, m.at(st.q3).min(m.at(st.q1))..=m.at(st.q3).max(m.at(st.q1)));
            let cx = r.center().x;
            let s = Stroke::new(1.0 * z, edge);
            c.line(vec![pos2(cx, m.at(st.high)), pos2(cx, r.min.y)], s);
            c.line(vec![pos2(cx, r.max.y), pos2(cx, m.at(st.low))], s);
            let cap = r.width() * 0.25;
            c.line(vec![pos2(cx - cap, m.at(st.high)), pos2(cx + cap, m.at(st.high))], s);
            c.line(vec![pos2(cx - cap, m.at(st.low)), pos2(cx + cap, m.at(st.low))], s);
            c.rect(r, col, Some(edge));
            c.line(vec![pos2(r.min.x, m.at(st.median)), pos2(r.max.x, m.at(st.median))], Stroke::new(1.5 * z, edge));
            if chart.show_mean {
                let at = pos2(cx, m.at(st.mean));
                let d = 3.5 * z;
                let ms = Stroke::new(1.5 * z, on(col));
                c.line(vec![at - vec2(d, d), at + vec2(d, d)], ms);
                c.line(vec![at + vec2(-d, d), at + vec2(d, -d)], ms);
            }
            if chart.show_outliers {
                for v in &st.outliers {
                    c.out.push(Shape::circle_stroke(pos2(cx, m.at(*v)), 3.0 * z, Stroke::new(1.0 * z, edge)));
                }
            }
            if chart.labels.value {
                c.text(pos2(r.max.x + 2.0 * z, m.at(st.median)), Align2::LEFT_CENTER, &c.num(st.median, &fmt), LABEL_PX, false, dark);
            }
        }
    }
    c.line(vec![plot.left_bottom(), plot.right_bottom()], Stroke::new(0.75 * z, look.axis));
}

// --- radar -------------------------------------------------------------------------------

pub(super) fn radar(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let z = c.z;
    let look = c.look;
    let n = values.iter().map(|v| v.len()).max().unwrap_or(0);
    if n == 0 {
        return;
    }
    let gap = 5.0 * z;
    let label_h = c.text_size("Ag", LABEL_PX).y;
    let widest = cats.iter().take(n).map(|t| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max).min(area.width() * 0.25);
    let radius = ((area.width() / 2.0 - widest - gap).min(area.height() / 2.0 - label_h - gap)).max(4.0);
    let center = area.center();
    let (mut lo, mut hi) = (0.0f64, f64::NEG_INFINITY);
    for v in values.iter().flatten().flatten() {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    if hi <= lo {
        hi = lo + 1.0;
    }
    let scale = apex_sheet::chart::scale_for(lo, hi, 5, &chart.y_axis);
    let fmt = format_for(&chart.y_axis.format, &data.value_format);
    let step = std::f32::consts::TAU / n as f32;
    let at = |j: usize, v: f64| arc_point(center, radius * scale.frac(v).clamp(0.0, 1.2) as f32, step * j as f32);
    // Web: rings at the ticks and a spoke per category.
    let grid = Stroke::new(0.75 * z, look.grid);
    if chart.y_axis.major_grid {
        for v in scale.ticks() {
            let mut ring: Vec<Pos2> = (0..n).map(|j| at(j, v)).collect();
            ring.push(ring[0]);
            c.line(ring, grid);
        }
    }
    for j in 0..n {
        c.line(vec![center, arc_point(center, radius, step * j as f32)], grid);
        if chart.x_axis.visible {
            let a = step * j as f32;
            let t = cats.get(j).cloned().unwrap_or_default();
            let color = look.text;
            c.text(arc_point(center, radius + gap, a), outside_align(a), &t, LABEL_PX, false, color);
        }
    }
    if chart.y_axis.visible {
        for v in scale.ticks() {
            let t = c.num(v, &fmt);
            let color = look.text;
            c.text(at(0, v) - vec2(gap, 0.0), Align2::RIGHT_CENTER, &t, LABEL_PX, false, color);
        }
    }
    for (i, vals) in values.iter().enumerate() {
        let col = series_color(chart, i);
        let pts: Vec<Option<Pos2>> = (0..n).map(|j| vals.get(j).copied().flatten().map(|v| at(j, v))).collect();
        if chart.filled {
            let fill = look.fill(col).gamma_multiply(0.85);
            for j in 0..n {
                let (Some(a), Some(b)) = (pts[j], pts[(j + 1) % n]) else { continue };
                c.poly(vec![center, a, b], fill);
            }
        }
        let width = chart.series.get(i).and_then(|s| s.line_width).map_or(look.line_width, |w| w * 1.333) * z;
        for mut run in runs(&pts, chart.blanks == Blanks::Span) {
            if run.len() == n.min(pts.iter().flatten().count()) && pts.iter().all(|p| p.is_some()) {
                run.push(run[0]);
            }
            c.line(run, Stroke::new(if chart.filled { 1.0 * z } else { width }, if chart.filled { mix(col, Color32::BLACK, 0.2) } else { col }));
        }
        if chart.markers && !chart.filled {
            let m = series_marker(chart, i);
            for p in pts.iter().flatten() {
                c.marker(*p, m, 6.0 * z, col);
            }
        }
        if chart.labels.any() {
            for (j, p) in pts.iter().enumerate() {
                let (Some(p), Some(v)) = (p, vals.get(j).copied().flatten()) else { continue };
                let t = label_text(c, chart, data, cats, i, j, v, &fmt);
                let color = look.text;
                c.text(*p - vec2(0.0, 4.0 * z), Align2::CENTER_BOTTOM, &t, LABEL_PX, false, color);
            }
        }
    }
}

// --- surface and contour -----------------------------------------------------------------

/// The value bands of a surface chart (its legend): from tick to tick of
/// the value axis.
pub(super) fn surface_bands(chart: &Chart, data: &ChartData) -> Vec<(f64, f64)> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in data.series.iter().flat_map(|s| s.values.iter().flatten()) {
        if v.is_finite() {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
    }
    if lo > hi {
        return Vec::new();
    }
    let scale = apex_sheet::chart::scale_for(lo, hi, 6, &chart.y_axis);
    let t = scale.ticks();
    t.windows(2).take(24).map(|w| (w[0], w[1])).collect()
}

/// Color of value band `k`.
pub(super) fn band_color(chart: &Chart, k: usize) -> Color32 {
    auto_color(chart, k)
}

fn band_of(bands: &[(f64, f64)], v: f64) -> usize {
    bands.iter().position(|(_, b)| v <= *b).unwrap_or(bands.len().saturating_sub(1))
}

pub(super) fn surface(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let z = c.z;
    let look = c.look;
    let cols = values.iter().map(|v| v.len()).max().unwrap_or(0);
    if values.is_empty() || cols < 2 {
        return;
    }
    // Series are the rows of the grid (one row is drawn as two).
    let mut grid: Vec<Vec<f64>> = values.iter().map(|r| (0..cols).map(|j| r.get(j).copied().flatten().unwrap_or(0.0)).collect()).collect();
    if grid.len() == 1 {
        grid.push(grid[0].clone());
    }
    let rows = grid.len();
    let bands = surface_bands(chart, data);
    if bands.is_empty() {
        return;
    }
    let (lo, hi) = (bands[0].0, bands[bands.len() - 1].1);
    let frac = |v: f64| if hi > lo { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 } else { 0.5 };
    let fmt = format_for(&chart.y_axis.format, &data.value_format);
    let gap = 4.0 * z;
    let label_h = c.text_size("0", LABEL_PX).y;
    let names: Vec<String> = data.series.iter().map(|s| s.name.clone()).collect();
    let color_of = |c: &Canvas, v: f64| c.look.fill(band_color(chart, band_of(&bands, v)));
    if !chart.view3d {
        // Contour: the grid seen from above, colored by band.
        let left = names.iter().map(|t| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max).min(area.width() * 0.3);
        let plot = Rect::from_min_max(pos2(area.min.x + left + gap, area.min.y), pos2(area.max.x, area.max.y - label_h - gap));
        if plot.width() < 4.0 || plot.height() < 4.0 {
            return;
        }
        let sub = ((20000 / (cols * rows).max(1)) as f32).sqrt().clamp(1.0, 8.0) as usize;
        let (cw, rh) = (plot.width() / (cols - 1) as f32 / sub as f32, plot.height() / (rows - 1) as f32 / sub as f32);
        for s in 0..rows - 1 {
            for j in 0..cols - 1 {
                for a in 0..sub {
                    for b in 0..sub {
                        let (u, v) = ((b as f64 + 0.5) / sub as f64, (a as f64 + 0.5) / sub as f64);
                        let val = grid[s][j] * (1.0 - u) * (1.0 - v) + grid[s][j + 1] * u * (1.0 - v) + grid[s + 1][j] * (1.0 - u) * v + grid[s + 1][j + 1] * u * v;
                        let x = plot.min.x + (j * sub + b) as f32 * cw;
                        let y = plot.max.y - (s * sub + a + 1) as f32 * rh;
                        let col = color_of(c, val);
                        c.out.push(Shape::rect_filled(Rect::from_min_size(pos2(x, y), vec2(cw + 0.5, rh + 0.5)), 0.0, col));
                    }
                }
            }
        }
        c.out.push(Shape::rect_stroke(plot, 0.0, Stroke::new(0.75 * z, look.axis), egui::StrokeKind::Middle));
        let step = ((label_h * 1.2) / (plot.height() / (rows - 1) as f32)).ceil().max(1.0) as usize;
        for (s, t) in names.iter().enumerate().step_by(step) {
            c.text(pos2(plot.min.x - gap, plot.max.y - plot.height() * s as f32 / (rows - 1) as f32), Align2::RIGHT_CENTER, t, LABEL_PX, false, look.text);
        }
        let widest = cats.iter().take(cols).map(|t| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max) + 6.0 * z;
        let step = (widest / (plot.width() / (cols - 1) as f32)).ceil().max(1.0) as usize;
        for (j, t) in cats.iter().take(cols).enumerate().step_by(step) {
            c.text(pos2(plot.min.x + plot.width() * j as f32 / (cols - 1) as f32, plot.max.y + gap), Align2::CENTER_TOP, t, LABEL_PX, false, look.text);
        }
        return;
    }
    // 3-D: categories run right and a little down, series go back and to
    // the right, values up.
    let vl: Vec<(f64, String)> = if chart.y_axis.visible { std::iter::once(lo).chain(bands.iter().map(|b| b.1)).map(|v| (v, c.num(v, &fmt))).collect() } else { Vec::new() };
    let left = vl.iter().map(|(_, t)| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max) + gap;
    let right = names.iter().map(|t| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max).min(area.width() * 0.25) + gap;
    let (w, h) = (area.width() - left - right, area.height() - label_h - gap);
    if w < 10.0 || h < 10.0 {
        return;
    }
    let ax = vec2(w * 0.62, h * 0.16);
    let bx = vec2(w * 0.38, -h * 0.22);
    let up = h * 0.58;
    let o = pos2(area.min.x + left, area.min.y + h * 0.22 + up + label_h / 2.0);
    let p = |j: f32, s: f32, v: f64| o + ax * (j / (cols - 1) as f32) + bx * (s / (rows - 1) as f32) - vec2(0.0, up * frac(v));
    let wall = mix(look.grid, look.bg, 0.5);
    let floor = [o, o + ax, o + ax + bx, o + bx];
    c.poly(floor.to_vec(), wall);
    let back = [o + bx, o + ax + bx, o + ax + bx - vec2(0.0, up), o + bx - vec2(0.0, up)];
    c.poly(back.to_vec(), mix(look.grid, look.bg, 0.7));
    let side = [o, o + bx, o + bx - vec2(0.0, up), o - vec2(0.0, up)];
    c.poly(side.to_vec(), mix(look.grid, look.bg, 0.6));
    let gs = Stroke::new(0.75 * z, look.grid);
    for (v, t) in &vl {
        let y = up * frac(*v);
        c.line(vec![o - vec2(0.0, y), o + bx - vec2(0.0, y), o + ax + bx - vec2(0.0, y)], gs);
        c.text(o - vec2(gap, y), Align2::RIGHT_CENTER, t, LABEL_PX, false, look.text);
    }
    // Back to front: the far rows first.
    let wire = Stroke::new(0.6 * z, Color32::from_black_alpha(90));
    for s in (0..rows - 1).rev() {
        for j in 0..cols - 1 {
            let q = [p(j as f32, s as f32, grid[s][j]), p(j as f32 + 1.0, s as f32, grid[s][j + 1]), p(j as f32 + 1.0, s as f32 + 1.0, grid[s + 1][j + 1]), p(j as f32, s as f32 + 1.0, grid[s + 1][j])];
            let mean = (grid[s][j] + grid[s][j + 1] + grid[s + 1][j + 1] + grid[s + 1][j]) / 4.0;
            let col = color_of(c, mean);
            c.poly(vec![q[0], q[1], q[2]], col);
            c.poly(vec![q[0], q[2], q[3]], col);
            c.line(vec![q[0], q[1], q[2], q[3], q[0]], wire);
        }
    }
    let widest = cats.iter().take(cols).map(|t| c.text_size(t, LABEL_PX).x).fold(0.0, f32::max) + 6.0 * z;
    let step = (widest / (ax.x / (cols - 1) as f32)).ceil().max(1.0) as usize;
    for (j, t) in cats.iter().take(cols).enumerate().step_by(step) {
        c.text(o + ax * (j as f32 / (cols - 1) as f32) + vec2(0.0, gap), Align2::CENTER_TOP, t, LABEL_PX, false, look.text);
    }
    let step = ((label_h * 1.1) / (-bx.y / (rows - 1) as f32)).ceil().max(1.0) as usize;
    for (s, t) in names.iter().enumerate().take(rows).step_by(step) {
        c.text(o + ax + bx * (s as f32 / (rows - 1) as f32) + vec2(gap, 0.0), Align2::LEFT_CENTER, t, LABEL_PX, false, look.text);
    }
}

// --- 3-D pie -----------------------------------------------------------------------------

pub(super) fn pie3d(c: &mut Canvas, chart: &Chart, data: &ChartData, values: &[Vec<Option<f64>>], cats: &[String], area: Rect) {
    let Some(vals) = values.first() else { return };
    let vals: Vec<f64> = vals.iter().map(|v| v.unwrap_or(0.0).abs()).collect();
    let total: f64 = vals.iter().sum();
    if total <= 0.0 {
        return;
    }
    let z = c.z;
    let tilt = 0.55;
    let s0 = chart.series.first();
    let most = s0.map_or(0, |s| s.explode.max(s.point_explode.iter().map(|p| p.1).max().unwrap_or(0))) as f32 / 100.0;
    let radius = (area.width() / 2.0).min(area.height() / (2.0 * tilt + 0.36)) * 0.92 / (1.0 + most);
    if radius < 2.0 {
        return;
    }
    let thick = radius * 0.18;
    let center = area.center() - vec2(0.0, thick / 2.0);
    let proj = |c0: Pos2, r: f32, a: f32| c0 + vec2(a.sin() * r, -a.cos() * r * tilt);
    let tau = std::f32::consts::TAU;
    let mut a = (chart.first_angle as f32).to_radians();
    let mut slices = Vec::new();
    for (j, v) in vals.iter().enumerate() {
        let sweep = (*v / total) as f32 * tau;
        if sweep <= 0.0 {
            continue;
        }
        let pulled = s0.map_or(0, |s| s.explosion(j)) as f32 / 100.0 * radius;
        let mid = a + sweep / 2.0;
        let c0 = center + vec2(mid.sin() * pulled, -mid.cos() * pulled * tilt);
        slices.push((j, a, sweep, c0, c.look.fill(point_color(chart, 0, j))));
        a += sweep;
    }
    // The sides facing the viewer, then the tops.
    let down = vec2(0.0, thick);
    for &(_, a0, sweep, c0, col) in &slices {
        // The cut faces of a slice pulled out.
        if c0 != center {
            for edge in [a0, a0 + sweep] {
                let rim = proj(c0, radius, edge);
                if (rim.x - c0.x).abs() > 0.5 {
                    c.poly(vec![c0, rim, rim + down, c0 + down], mix(col, Color32::BLACK, 0.2));
                }
            }
        }
        // The rim in front (angles past a quarter turn from the top), as one
        // mesh so no seams show.
        let steps = ((sweep / 0.05).ceil() as usize).clamp(1, 1440);
        let dark = mix(col, Color32::BLACK, 0.3);
        let mut mesh = egui::epaint::Mesh::default();
        for k in 0..=steps {
            let t = a0 + sweep * k as f32 / steps as f32;
            let q = proj(c0, radius, t);
            mesh.colored_vertex(q, dark);
            mesh.colored_vertex(q + down, dark);
            if k > 0 {
                let mid = t - sweep / steps as f32 / 2.0;
                if mid.cos() < 0.02 {
                    let i = 2 * k as u32;
                    mesh.add_triangle(i - 2, i - 1, i);
                    mesh.add_triangle(i - 1, i + 1, i);
                }
            }
        }
        if !mesh.indices.is_empty() {
            c.out.push(Shape::mesh(mesh));
        }
    }
    let fmt = format_for("", &data.value_format);
    let pct = NumFormat::parse("0%");
    let sep = c.look.outline.unwrap_or(c.look.bg);
    let mut labels = Vec::new();
    for &(j, a0, sweep, c0, col) in &slices {
        let steps = ((sweep / 0.05).ceil() as usize).clamp(1, 1440);
        let mut mesh = egui::epaint::Mesh::default();
        mesh.colored_vertex(c0, col);
        let rim: Vec<Pos2> = (0..=steps).map(|k| proj(c0, radius, a0 + sweep * k as f32 / steps as f32)).collect();
        for q in &rim {
            mesh.colored_vertex(*q, col);
        }
        for k in 0..steps as u32 {
            mesh.add_triangle(0, k + 1, k + 2);
        }
        c.out.push(Shape::mesh(mesh));
        c.line(rim, Stroke::new(0.8, col));
        if slices.len() > 1 {
            c.line(vec![c0, proj(c0, radius, a0)], Stroke::new(1.0 * z, sep));
        }
        if chart.labels.any() {
            let mut parts: Vec<String> = super::cell_label(chart, data, 0, j).into_iter().collect();
            if chart.labels.series {
                parts.push(data.series.first().map_or(String::new(), |s| s.name.clone()));
            }
            if chart.labels.category {
                parts.push(cats.get(j).cloned().unwrap_or_default());
            }
            if chart.labels.value {
                parts.push(c.num(values[0][j].unwrap_or(0.0), &fmt));
            }
            if chart.labels.percent {
                parts.push(c.num(vals[j] / total, &pct));
            }
            labels.push((proj(c0, radius * 0.62, a0 + sweep / 2.0), parts.join(", ")));
        }
    }
    let text = Color32::from_gray(30);
    for (at, t) in labels {
        let g = c.galley(&t, LABEL_PX, false, text);
        let r = Rect::from_center_size(at, g.size());
        c.out.push(Shape::rect_filled(r.expand(1.5 * z), 2.0 * z, Color32::from_white_alpha(200)));
        c.out.push(Shape::galley(r.min, g, text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartiles_and_boxes() {
        let s = box_stats(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0]).unwrap();
        assert_eq!((s.q1, s.median, s.q3), (2.25, 4.5, 6.75));
        assert_eq!(s.outliers, vec![100.0]);
        assert_eq!((s.low, s.high), (1.0, 7.0));
        assert!(box_stats(vec![]).is_none());
        let one = box_stats(vec![3.0]).unwrap();
        assert_eq!((one.q1, one.q3), (3.0, 3.0));
    }

    #[test]
    fn timeline_events_sorted() {
        let ev = timeline_events(&[Some(30.0), None, Some(10.0)], &["c".into(), "b".into(), "a".into()]);
        assert_eq!(ev, vec![(10.0, "a".to_string()), (30.0, "c".to_string())]);
    }

    #[test]
    fn squarified_rects_fill_the_area() {
        let r = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 200.0));
        let rects = squarify(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], r);
        let total: f32 = rects.iter().map(|r| r.area()).sum();
        assert!((total - r.area()).abs() < 1.0, "{total}");
        for x in &rects {
            assert!(r.expand(0.5).contains_rect(*x), "{x:?}");
        }
        assert!((rects[0].area() / r.area() - 6.0 / 24.0).abs() < 0.01);
    }

    #[test]
    fn category_tree() {
        let d = ChartData {
            levels: vec![vec!["A".into(), "x".into()], vec!["A".into(), "y".into()], vec!["B".into()]],
            categories: vec!["x".into(), "y".into(), "B".into()],
            ..Default::default()
        };
        let t = tree(&d, &[Some(1.0), Some(2.0), Some(4.0)]);
        assert_eq!(t.value, 7.0);
        assert_eq!(t.children.len(), 2);
        assert_eq!(t.children[0].value, 3.0);
        assert_eq!(t.children[0].children.len(), 2);
        assert_eq!(t.depth(), 2);
    }

    #[test]
    fn pareto_sorts_and_accumulates() {
        let mut chart = Chart::new(ChartType::Pareto, apex_sheet::CellRef::new(0, 0), 400.0, 300.0);
        chart.series.push(Default::default());
        let d = ChartData {
            series: vec![SeriesData { name: "S".into(), values: vec![Some(1.0), Some(5.0), Some(4.0)], ..Default::default() }],
            categories: vec!["a".into(), "b".into(), "c".into()],
            ..Default::default()
        };
        let (c, p) = pareto(&chart, &d);
        assert_eq!(c.kind, ChartType::Column);
        assert_eq!(p.categories, ["b", "c", "a"]);
        assert_eq!(p.series[1].values, [Some(0.5), Some(0.9), Some(1.0)]);
        assert!(c.has_secondary());
    }
}

/// Renders every chart type to PNG files in `$APEX_CHART_OUT` (a visual
/// check: `APEX_CHART_OUT=dir cargo test -p apex-app gallery -- --ignored`).
#[cfg(test)]
mod gallery {
    use apex_sheet::{CellRef, Range, Workbook};

    use super::*;

    fn book(rows: &[&[&str]]) -> Workbook {
        let mut wb = Workbook::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                if !v.is_empty() {
                    wb.set_input(0, CellRef::new(r as u32, c as u32), v);
                }
            }
        }
        wb
    }

    #[test]
    #[ignore]
    fn gallery() {
        let Ok(dir) = std::env::var("APEX_CHART_OUT") else { return };
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let sales: &[&[&str]] = &[&["", "Norte", "Sur", "Este"], &["Ene", "10", "14", "8"], &["Feb", "16", "11", "12"], &["Mar", "12", "18", "15"], &["Abr", "20", "15", "9"], &["May", "17", "22", "13"]];
        let tree: &[&[&str]] = &[&["Región", "Producto", "Ventas"], &["Norte", "Pan", "30"], &["", "Leche", "22"], &["", "Queso", "12"], &["Sur", "Pan", "25"], &["", "Fruta", "40"], &["Este", "Pan", "18"], &["", "Leche", "9"], &["Oeste", "", "15"]];
        let stock: &[&[&str]] = &[&["", "Apertura", "Máximo", "Mínimo", "Cierre"], &["L", "44", "55", "40", "50"], &["M", "50", "57", "48", "52"], &["X", "52", "53", "41", "43"], &["J", "43", "49", "39", "47"], &["V", "47", "60", "46", "58"]];
        let bubble: &[&[&str]] = &[&["X", "Y", "Tamaño"], &["1", "10", "5"], &["2", "14", "20"], &["3", "9", "8"], &["4", "18", "40"], &["5", "12", "12"]];
        let grid: &[&[&str]] = &[&["", "0", "1", "2", "3", "4", "5"], &["A", "1", "3", "5", "6", "4", "2"], &["B", "2", "5", "8", "9", "6", "3"], &["C", "3", "6", "10", "11", "8", "4"], &["D", "2", "4", "7", "8", "5", "3"], &["E", "1", "2", "4", "5", "3", "1"]];
        let boxes: &[&[&str]] = &[&["Curso", "Nota A", "Nota B"], &["1º", "5", "6"], &["1º", "7", "8"], &["1º", "6", "5"], &["1º", "9", "7"], &["1º", "2", "6"], &["2º", "4", "3"], &["2º", "6", "5"], &["2º", "8", "9"], &["2º", "7", "6"], &["2º", "5", "10"]];
        let funnel: &[&[&str]] = &[&["Etapa", "Clientes"], &["Visitas", "5000"], &["Registros", "2300"], &["Pruebas", "1100"], &["Compras", "480"], &["Renovaciones", "210"]];
        let pareto: &[&[&str]] = &[&["Causa", "Quejas"], &["Retraso", "42"], &["Precio", "18"], &["Calidad", "31"], &["Atención", "9"], &["Otros", "5"]];
        let mekko: &[&[&str]] = &[&["Region", "Mercado", "A", "B", "C"], &["Norte", "500", "40", "35", "25"], &["Sur", "300", "20", "50", "30"], &["Este", "150", "60", "10", "30"], &["Oeste", "50", "30", "30", "40"]];
        let timeline: &[&[&str]] = &[&["Fecha", "Hito"], &["45292", "Inicio"], &["45350", "Prototipo"], &["45420", "Beta"], &["45500", "Lanzamiento"], &["45650", "Version 2"]];
        let cases: Vec<(&str, ChartType, bool, &[&[&str]])> = vec![
            ("column3d", ChartType::Column, true, sales),
            ("bar3d", ChartType::Bar, true, sales),
            ("line3d", ChartType::Line, true, sales),
            ("area3d", ChartType::Area, true, sales),
            ("pie3d", ChartType::Pie, true, sales),
            ("radar", ChartType::Radar, false, sales),
            ("radar_filled", ChartType::Radar, true, sales),
            ("bubble", ChartType::Bubble, false, bubble),
            ("stock", ChartType::Stock, false, stock),
            ("surface", ChartType::Surface, true, grid),
            ("contour", ChartType::Surface, false, grid),
            ("treemap", ChartType::Treemap, false, tree),
            ("sunburst", ChartType::Sunburst, false, tree),
            ("boxwhisker", ChartType::BoxWhisker, false, boxes),
            ("funnel", ChartType::Funnel, false, funnel),
            ("pareto", ChartType::Pareto, false, pareto),
            ("mekko", ChartType::Mekko, false, mekko),
            ("timeline", ChartType::Timeline, false, timeline),
        ];
        for (name, kind, flag, rows) in cases {
            let wb = book(rows);
            let range = Range::new(CellRef::new(0, 0), CellRef::new(rows.len() as u32 - 1, rows[0].len() as u32 - 1));
            let mut c = wb.chart_from_range(0, range, kind, None);
            c.view3d = flag && kind.has_3d() || (kind == ChartType::Surface && flag);
            c.filled = flag && kind == ChartType::Radar;
            c.title = name.into();
            let data = wb.chart_data(0, &c);
            let img = super::super::render(&ctx, &c, &data, 480.0, 300.0, 1.0, &Locale::EN);
            apex_fs::write(format!("{dir}/{name}.png"), super::super::encode_png(&img).unwrap()).unwrap();
        }
        // Chart elements: error bars, data tables, labels from cells.
        use apex_sheet::chart::{ErrorAmount, ErrorBars, ErrorDir};
        let extras: Vec<(&str, ChartType, &[&[&str]], fn(&mut Chart))> = vec![
            ("err_column_table", ChartType::Column, sales, |c| {
                c.data_table = true;
                for s in &mut c.series {
                    s.error_bars = Some(ErrorBars { amount: ErrorAmount::Percent(10.0), ..Default::default() });
                }
            }),
            ("err_line_cells", ChartType::Line, sales, |c| {
                c.labels.cells = true;
                c.series[0].label_range = c.series[2].values.clone();
                c.series[1].error_bars = Some(ErrorBars { amount: ErrorAmount::StdDev(1.0), dir: ErrorDir::Both, cap: false });
            }),
            ("err_scatter", ChartType::Scatter, bubble, |c| {
                c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::Fixed(2.0), dir: ErrorDir::Plus, cap: true });
            }),
            ("err_bar", ChartType::Bar, sales, |c| {
                c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::Custom { plus: c.series[2].values.clone(), minus: c.series[1].values.clone() }, ..Default::default() });
            }),
            ("table_stacked", ChartType::Column, sales, |c| {
                c.grouping = apex_sheet::chart::Grouping::Stacked;
                c.data_table = true;
                c.table_keys = false;
                c.x_axis.title = "Mes".into();
                c.y_axis.title = "Ventas".into();
            }),
        ];
        for (name, kind, rows, f) in extras {
            let wb = book(rows);
            let range = Range::new(CellRef::new(0, 0), CellRef::new(rows.len() as u32 - 1, rows[0].len() as u32 - 1));
            let mut c = wb.chart_from_range(0, range, kind, None);
            c.title = name.into();
            f(&mut c);
            let data = wb.chart_data(0, &c);
            let img = super::super::render(&ctx, &c, &data, 480.0, 300.0, 1.0, &Locale::EN);
            apex_fs::write(format!("{dir}/{name}.png"), super::super::encode_png(&img).unwrap()).unwrap();
        }
    }
}
