//! Charts over the sheet (painting from cached data, selection handles,
//! moving and resizing, keys, clipboard, Insert > Chart and the chart
//! editor) and sparklines in cells (painting and their dialog).
//!
//! The hooks live in `images_ui.rs`: charts are floating objects like
//! pictures and share their entry points.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use apex_sheet::chart::{Axis, Bins, Blanks, Chart, ChartData, ChartType, DataRef, ErrorAmount, ErrorBars, ErrorDir, Grouping, LegendPos, Marker, Series, SparkAxis, SparkGroup, SparkKind, Sparkline, TrendKind, Trendline};
use apex_sheet::formula::{self, Expr};
use apex_sheet::{CellRef, Range, Sheet};
use eframe::egui::{self, Color32, CursorIcon, Event, Key, Modifiers, Pos2, Rect, RichText, Shape, Stroke, Vec2, pos2, vec2};

use super::{Geo, SheetView};
use crate::app::Notice;
use crate::charts;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme;
use crate::widgets::{self, DialogResult};

const HANDLE: f32 = crate::touch::size(7.0);
const GRAB: f32 = 6.0;
const MIN_SIZE: f32 = 40.0;

#[derive(Default)]
pub(super) struct ChartsState {
    /// Selected chart: sheet and id.
    pub(super) selected: Option<(usize, u32)>,
    /// A chart being dragged or edited, painted instead of the stored one.
    preview: Option<Chart>,
    drag: Option<ChartDrag>,
    menu: bool,
    editor: Option<Editor>,
    spark: Option<SparkDlg>,
    /// Last copied chart and the pixel hash of its picture on the clipboard.
    clip: Option<(Chart, u64)>,
    hovering: bool,
    cache: RefCell<Cache>,
}

impl ChartsState {
    pub(super) fn deselect(&mut self) {
        self.selected = None;
        self.menu = false;
    }

    /// No chart is being dragged.
    pub(super) fn idle(&self) -> bool {
        self.drag.is_none()
    }
}

#[derive(Default)]
struct Cache {
    /// Chart data by sheet and chart id.
    data: HashMap<(usize, u32), DataEntry>,
    /// Shapes last built for a chart on screen.
    shapes: HashMap<(usize, u32), ShapeEntry>,
    /// Sparkline values of the shown sheet: (revision, sheet) and per group,
    /// per sparkline.
    sparks: Option<((u64, usize), Vec<(Option<(f64, f64)>, Vec<Vec<Option<f64>>>)>)>,
}

struct DataEntry {
    revision: u64,
    /// Computed for the chart as stored in the workbook (not a preview):
    /// then the revision alone tells it is current.
    stored: bool,
    /// What the data depends on.
    key: DataKey,
    data: Arc<ChartData>,
    /// Bumped when the data change (shape caches key on it).
    version: u64,
}

#[derive(PartialEq)]
struct DataKey {
    series: Vec<Series>,
    kind: ChartType,
    visible_only: bool,
    blanks: Blanks,
}

impl DataKey {
    fn of(c: &Chart) -> DataKey {
        DataKey { series: c.series.clone(), kind: c.kind, visible_only: c.visible_only, blanks: c.blanks }
    }

    fn matches(&self, c: &Chart) -> bool {
        self.kind == c.kind && self.visible_only == c.visible_only && self.blanks == c.blanks && self.series == c.series
    }
}

struct ShapeEntry {
    revision: u64,
    stored: bool,
    rect: Rect,
    z: f32,
    ppp: f32,
    version: u64,
    fonts: u64,
    chart: Chart,
    /// Everything but the text, tessellated once (thousands of points
    /// cost nothing on later frames), and the text.
    mesh: Arc<egui::epaint::Mesh>,
    texts: Vec<Shape>,
}

/// Tessellates the non-text shapes of a chart into one mesh.
fn freeze(ctx: &egui::Context, shapes: Vec<Shape>, ppp: f32) -> (egui::epaint::Mesh, Vec<Shape>) {
    let (texts, rest): (Vec<Shape>, Vec<Shape>) = shapes.into_iter().partition(|s| matches!(s, Shape::Text(_)));
    let clipped = rest.into_iter().map(|shape| egui::epaint::ClippedShape { clip_rect: Rect::EVERYTHING, shape }).collect();
    let mut mesh = egui::epaint::Mesh::default();
    for prim in ctx.tessellate(clipped, ppp) {
        if let egui::epaint::Primitive::Mesh(m) = prim.primitive {
            if m.texture_id == mesh.texture_id {
                mesh.append(m);
            }
        }
    }
    (mesh, texts)
}

/// Chart types offered for inserting: label, icon, type, grouping and the
/// variant flags of `insert_chart` (bit 0 markers, 1 lines, 4 3-D, 5
/// filled).
pub(crate) const CHART_TYPES: [(&str, Icon, ChartType, Grouping, u8); 36] = [
    ("chart.t.column", Icon::ChartColumn, ChartType::Column, Grouping::Clustered, 0),
    ("chart.t.column_stacked", Icon::ChartColumn, ChartType::Column, Grouping::Stacked, 0),
    ("chart.t.column_percent", Icon::ChartColumn, ChartType::Column, Grouping::Percent, 0),
    ("chart.t.column_3d", Icon::ChartColumn, ChartType::Column, Grouping::Clustered, 16),
    ("chart.t.bar", Icon::ChartBar, ChartType::Bar, Grouping::Clustered, 0),
    ("chart.t.bar_stacked", Icon::ChartBar, ChartType::Bar, Grouping::Stacked, 0),
    ("chart.t.bar_percent", Icon::ChartBar, ChartType::Bar, Grouping::Percent, 0),
    ("chart.t.bar_3d", Icon::ChartBar, ChartType::Bar, Grouping::Clustered, 16),
    ("chart.t.line", Icon::ChartLine, ChartType::Line, Grouping::Clustered, 0),
    ("chart.t.line_markers", Icon::ChartLine, ChartType::Line, Grouping::Clustered, 1),
    ("chart.t.line_3d", Icon::ChartLine, ChartType::Line, Grouping::Clustered, 16),
    ("chart.t.area", Icon::ChartArea, ChartType::Area, Grouping::Clustered, 0),
    ("chart.t.area_stacked", Icon::ChartArea, ChartType::Area, Grouping::Stacked, 0),
    ("chart.t.area_3d", Icon::ChartArea, ChartType::Area, Grouping::Clustered, 16),
    ("chart.t.pie", Icon::ChartPie, ChartType::Pie, Grouping::Clustered, 0),
    ("chart.t.pie_3d", Icon::ChartPie, ChartType::Pie, Grouping::Clustered, 16),
    ("chart.t.doughnut", Icon::ChartDoughnut, ChartType::Doughnut, Grouping::Clustered, 0),
    ("chart.t.scatter", Icon::ChartScatter, ChartType::Scatter, Grouping::Clustered, 0),
    ("chart.t.scatter_lines", Icon::ChartScatter, ChartType::Scatter, Grouping::Clustered, 2),
    ("chart.t.bubble", Icon::ChartBubble, ChartType::Bubble, Grouping::Clustered, 0),
    ("chart.t.radar", Icon::ChartRadar, ChartType::Radar, Grouping::Clustered, 0),
    ("chart.t.radar_markers", Icon::ChartRadar, ChartType::Radar, Grouping::Clustered, 1),
    ("chart.t.radar_filled", Icon::ChartRadar, ChartType::Radar, Grouping::Clustered, 32),
    ("chart.t.stock", Icon::ChartStock, ChartType::Stock, Grouping::Clustered, 0),
    ("chart.t.surface", Icon::ChartSurface, ChartType::Surface, Grouping::Clustered, 16),
    ("chart.t.contour", Icon::ChartSurface, ChartType::Surface, Grouping::Clustered, 0),
    ("chart.t.treemap", Icon::ChartTreemap, ChartType::Treemap, Grouping::Clustered, 0),
    ("chart.t.sunburst", Icon::ChartSunburst, ChartType::Sunburst, Grouping::Clustered, 0),
    ("chart.t.histogram", Icon::ChartHistogram, ChartType::Histogram, Grouping::Clustered, 0),
    ("chart.t.pareto", Icon::ChartPareto, ChartType::Pareto, Grouping::Clustered, 0),
    ("chart.t.boxwhisker", Icon::ChartBox, ChartType::BoxWhisker, Grouping::Clustered, 0),
    ("chart.t.waterfall", Icon::ChartWaterfall, ChartType::Waterfall, Grouping::Clustered, 0),
    ("chart.t.funnel", Icon::ChartFunnel, ChartType::Funnel, Grouping::Clustered, 0),
    ("chart.t.mekko", Icon::ChartMekko, ChartType::Mekko, Grouping::Clustered, 0),
    ("chart.t.timeline", Icon::ChartTimeline, ChartType::Timeline, Grouping::Clustered, 0),
    ("chart.t.combo", Icon::ChartCombo, ChartType::Column, Grouping::Clustered, 0),
];

/// The Insert Chart menu: families of `CHART_TYPES`.
const CHART_MENU: [(&str, std::ops::Range<usize>); 13] = [
    ("chart.type.column", 0..4),
    ("chart.type.bar", 4..8),
    ("chart.type.line", 8..11),
    ("chart.type.area", 11..14),
    ("chart.type.pie", 14..17),
    ("chart.family.scatter", 17..20),
    ("chart.type.radar", 20..23),
    ("chart.family.stock_surface", 23..26),
    ("chart.family.hierarchy", 26..28),
    ("chart.family.statistic", 28..31),
    ("chart.family.flow", 31..33),
    ("chart.family.business", 33..35),
    ("chart.v.combo", 35..36),
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Move,
    Resize(i8, i8),
}

struct ChartDrag {
    handle: Handle,
    start: Pos2,
    orig: Chart,
    moved: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Type,
    Data,
    Elements,
    Axes,
    Style,
}

struct Editor {
    sheet: usize,
    chart: Chart,
    tab: Tab,
    /// The data range being typed and the series fields, as the user
    /// edits them (display syntax).
    range: String,
    series: usize,
    /// Name, values, categories, bubble sizes, label range, and the plus
    /// and minus ranges of custom error bars.
    fields: [String; 7],
    fields_for: Option<usize>,
    error: Option<String>,
    /// Totals of a waterfall as typed ("3, 7").
    totals: String,
    fresh: bool,
}

struct SparkDlg {
    sheet: usize,
    /// The group being edited (index), or a new one.
    editing: Option<usize>,
    group: SparkGroup,
    data: String,
    location: String,
    error: Option<String>,
    fresh: bool,
}

/// Screen rectangle of a chart in a pane.
fn screen_rect(g: &Geo, pane: (bool, bool), sh: &Sheet, c: &Chart) -> Rect {
    let [x, y, w, h] = c.rect(sh);
    Rect::from_min_size(g.point_to_screen(sh, pane.0, pane.1, x, y), vec2(w, h) * g.z)
}

fn pane_at(g: &Geo, p: Pos2) -> (bool, bool) {
    (g.fc > 0 && p.x < g.origin.x + g.fx * g.z, g.fr > 0 && p.y < g.origin.y + g.fy * g.z)
}

fn home_pane(g: &Geo, sh: &Sheet, c: &Chart) -> (bool, bool) {
    let (x, y) = c.pos(sh);
    (g.fc > 0 && x < g.ox + g.fx, g.fr > 0 && y < g.oy + g.fy)
}

fn handles(r: Rect) -> Vec<(Handle, Pos2)> {
    let mut v = Vec::with_capacity(8);
    for sy in -1i8..=1 {
        for sx in -1i8..=1 {
            if sx != 0 || sy != 0 {
                let x = r.center().x + sx as f32 * r.width() / 2.0;
                let y = r.center().y + sy as f32 * r.height() / 2.0;
                v.push((Handle::Resize(sx, sy), pos2(x, y)));
            }
        }
    }
    v
}

fn resize_cursor(sx: i8, sy: i8) -> CursorIcon {
    match (sx, sy) {
        (0, _) => CursorIcon::ResizeVertical,
        (_, 0) => CursorIcon::ResizeHorizontal,
        (a, b) if a == b => CursorIcon::ResizeNwSe,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// `orig` resized by dragging handle (sx, sy) by `d` sheet pixels; the
/// opposite side stays put.
fn resized(orig: &Chart, sh: &Sheet, sx: i8, sy: i8, d: Vec2, keep: bool) -> Chart {
    let [x, y, w, h] = orig.rect(sh);
    let mut nw = if sx != 0 { (w + sx as f32 * d.x).max(MIN_SIZE) } else { w };
    let mut nh = if sy != 0 { (h + sy as f32 * d.y).max(MIN_SIZE) } else { h };
    if keep && sx != 0 && sy != 0 {
        let k = (nw / w).max(nh / h);
        (nw, nh) = (w * k, h * k);
    }
    let nx = if sx < 0 { x + w - nw } else { x };
    let ny = if sy < 0 { y + h - nh } else { y };
    let mut c = orig.clone();
    c.width = nw;
    c.height = nh;
    c.set_pos(sh, nx, ny);
    c
}

/// A reference typed by the user, qualified with `sheet` when it names no
/// sheet (chart references in files always name theirs).
fn parse_ref(text: &str, sheet: &str, loc: &apex_sheet::Locale) -> Result<DataRef, String> {
    let t = text.trim();
    if t.is_empty() || t == "=" {
        return Ok(DataRef::default());
    }
    let mut e = formula::parse(t, loc).map_err(|e| e.msg)?;
    let own: Arc<str> = Arc::from(sheet);
    e.visit_refs_mut(&mut |r| {
        if let Expr::Ref { sheet: s @ None, .. } | Expr::Range { sheet: s @ None, .. } = r {
            *s = Some(own.clone());
        }
    });
    // A literal name ("Sales") is text, not an unknown name.
    if let Expr::Name(n) = &e {
        return Ok(DataRef::text(n));
    }
    Ok(DataRef { formula: Some(Arc::new(e)), cache: Vec::new() })
}

fn ref_text(r: &DataRef, loc: &apex_sheet::Locale) -> String {
    match &r.formula {
        Some(_) => r.to_display(loc),
        None => r.cache.iter().map(|v| match v {
            apex_sheet::Value::Text(t) => t.to_string(),
            apex_sheet::Value::Number(n) => apex_sheet::format::general_number(*n, loc.decimal),
            _ => String::new(),
        }).collect::<Vec<_>>().join(if loc.decimal == ',' { ";" } else { "," }),
    }
}

impl SheetView {
    pub(super) fn selected_chart(&self) -> Option<&Chart> {
        let (s, id) = self.images.charts.selected?;
        (s == self.sheet).then(|| self.sh().charts.iter().find(|c| c.id == id)).flatten()
    }

    pub(super) fn chart_selected(&self) -> bool {
        self.selected_chart().is_some()
    }

    pub(super) fn chart_hovered(&self) -> bool {
        self.images.charts.hovering
    }

    fn shown_chart<'a>(&'a self, c: &'a Chart) -> &'a Chart {
        let st = &self.images.charts;
        match &st.preview {
            Some(p) if p.id == c.id && st.selected.is_some_and(|(s, _)| s == self.sheet) => p,
            _ => c,
        }
    }

    /// The data of a chart of the current sheet, computed once per change.
    /// `stored`: `c` is the chart in the workbook, not a preview.
    pub(super) fn chart_data(&self, c: &Chart, stored: bool) -> (Arc<ChartData>, u64) {
        let key = (self.sheet, c.id);
        let rev = self.wb.revision;
        let mut cache = self.images.charts.cache.borrow_mut();
        if let Some(e) = cache.data.get(&key) {
            if e.revision == rev && ((stored && e.stored) || e.key.matches(c)) {
                return (e.data.clone(), e.version);
            }
        }
        let data = Arc::new(self.wb.chart_data(self.sheet, c));
        let version = cache.data.get(&key).map_or(1, |e| e.version + 1);
        // Same numbers after an unrelated edit: keep the shapes.
        let version = match cache.data.get(&key) {
            Some(e) if *e.data == *data => e.version,
            _ => version,
        };
        cache.data.insert(key, DataEntry { revision: rev, stored, key: DataKey::of(c), data: data.clone(), version });
        (data, version)
    }

    // --- painting ------------------------------------------------------------------

    /// Charts of the current sheet that show in `clip` of one pane.
    /// `print`: paint afresh (pages), not from the screen cache.
    pub(super) fn paint_charts_in(&self, p: &egui::Painter, g: &Geo, pane: (bool, bool), clip: Rect, print: bool) {
        // The charts' formulas share one budget, as one formula.
        apex_sheet::functions::shared_budget(2, 1 << 27, || self.paint_charts_now(p, g, pane, clip, print));
    }

    fn paint_charts_now(&self, p: &egui::Painter, g: &Geo, pane: (bool, bool), clip: Rect, print: bool) {
        let sh = self.sh();
        for stored in &sh.charts {
            let c = self.shown_chart(stored);
            let r = screen_rect(g, pane, sh, c);
            if !r.intersects(clip) {
                continue;
            }
            let stored_now = std::ptr::eq(c, stored);
            let (data, version) = self.chart_data(c, stored_now);
            if print {
                charts::paint(p, r, c, &data, g.z, &self.wb.locale);
                continue;
            }
            let key = (self.sheet, c.id);
            let mut cache = self.images.charts.cache.borrow_mut();
            let rev = self.wb.revision;
            let ppp = p.ctx().pixels_per_point();
            let fonts = crate::fonts::generation();
            let fresh = cache.shapes.get(&key).is_some_and(|e| e.rect.size() == r.size() && e.z == g.z && e.ppp == ppp && e.version == version && e.fonts == fonts && ((stored_now && e.stored && e.revision == rev) || e.chart == *c));
            if !fresh {
                let shapes = charts::build(p.ctx(), r, c, &data, g.z, &self.wb.locale);
                let (mesh, texts) = freeze(p.ctx(), shapes, ppp);
                cache.shapes.insert(key, ShapeEntry { revision: rev, stored: stored_now, rect: r, z: g.z, ppp, version, fonts, chart: c.clone(), mesh: Arc::new(mesh), texts });
            }
            if let Some(e) = cache.shapes.get_mut(&key) {
                // Scrolled: move what was built.
                let d = r.min - e.rect.min;
                if d != Vec2::ZERO {
                    Arc::make_mut(&mut e.mesh).translate(d);
                    for t in &mut e.texts {
                        t.translate(d);
                    }
                    e.rect = r;
                }
                p.add(Shape::Mesh(e.mesh.clone()));
                p.extend(e.texts.iter().cloned());
            }
        }
    }

    /// Sparklines in the cells of one pane.
    pub(super) fn paint_sparklines_in(&self, p: &egui::Painter, g: &Geo, clip: Rect) {
        let sh = self.sh();
        if sh.sparklines.is_empty() {
            return;
        }
        let key = (self.wb.revision, self.sheet);
        let mut cache = self.images.charts.cache.borrow_mut();
        if cache.sparks.as_ref().is_none_or(|(k, v)| *k != key || v.len() != sh.sparklines.len()) {
            let groups = sh
                .sparklines
                .iter()
                .map(|gr| {
                    let vals: Vec<Vec<Option<f64>>> = gr.sparklines.iter().map(|s| self.wb.sparkline_data(self.sheet, gr, s)).collect();
                    (charts::spark_range(&vals), vals)
                })
                .collect();
            cache.sparks = Some((key, groups));
        }
        let Some((_, groups)) = &cache.sparks else { return };
        for (gr, (range, vals)) in sh.sparklines.iter().zip(groups) {
            for (s, v) in gr.sparklines.iter().zip(vals) {
                let r = g.cell_rect(sh, s.cell);
                if r.width() < 2.0 || r.height() < 2.0 || !r.intersects(clip) {
                    continue;
                }
                charts::paint_sparkline(p, r, gr, v, *range, g.z);
            }
        }
    }

    pub(super) fn paint_chart_handles(&self, painter: &egui::Painter, g: &Geo) {
        let Some(stored) = self.selected_chart() else { return };
        let sh = self.sh();
        let c = self.shown_chart(stored);
        let r = screen_rect(g, home_pane(g, sh, c), sh, c);
        let p = painter.with_clip_rect(g.view);
        let accent = theme::SHEET_ACCENT;
        p.rect_stroke(r, 0.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
        crate::touch::grip(&p, r, crate::touch::reach(GRAB), Some((r.right_bottom(), r.left_top())));
        for (_, at) in handles(r) {
            p.rect(Rect::from_center_size(at, Vec2::splat(HANDLE)), 1.0, Color32::WHITE, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
        }
    }

    // --- pointer -------------------------------------------------------------------

    fn chart_hit(&self, g: &Geo, p: Pos2) -> Option<(u32, Handle)> {
        let sh = self.sh();
        if let Some(stored) = self.selected_chart() {
            let c = self.shown_chart(stored);
            let r = screen_rect(g, home_pane(g, sh, c), sh, c);
            if let Some(h) = crate::touch::nearest(handles(r), p, GRAB) {
                return Some((c.id, h));
            }
        }
        let pane = pane_at(g, p);
        for stored in sh.charts.iter().rev() {
            let c = self.shown_chart(stored);
            if screen_rect(g, pane, sh, c).contains(p) {
                return Some((c.id, Handle::Move));
            }
        }
        None
    }

    /// Mouse handling for charts; true when it took the event.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn chart_pointer(&mut self, ui: &egui::Ui, resp: &egui::Response, g: &Geo, p: Pos2, pressed: bool, down: bool, mods: Modifiers) -> bool {
        if let Some(d) = &mut self.images.charts.drag {
            if !d.moved && (p - d.start).length() > 3.0 {
                d.moved = true;
            }
            let (handle, start, moved) = (d.handle, d.start, d.moved);
            if down {
                if moved {
                    let orig = d.orig.clone();
                    let delta = (p - start) / g.z;
                    let sh = self.sh();
                    let c = match handle {
                        Handle::Move => {
                            let mut d = delta;
                            if mods.shift {
                                if d.x.abs() > d.y.abs() {
                                    d.y = 0.0;
                                } else {
                                    d.x = 0.0;
                                }
                            }
                            let mut c = orig.clone();
                            let (x, y) = orig.pos(sh);
                            c.set_pos(sh, x + d.x, y + d.y);
                            c
                        }
                        Handle::Resize(sx, sy) => resized(&orig, sh, sx, sy, delta, mods.shift),
                    };
                    self.images.charts.preview = Some(c);
                    ui.ctx().request_repaint();
                }
                return true;
            }
            let d = self.images.charts.drag.take();
            if let (Some(c), true) = (self.images.charts.preview.take(), moved) {
                self.wb.set_chart(self.sheet, c);
            } else if d.is_some() && resp.double_clicked() {
                self.open_chart_editor(Tab::Type);
            }
            return true;
        }
        let target = if resp.hovered() && g.view.contains(p) && !self.formula_ref_point() { self.chart_hit(g, p) } else { None };
        self.images.charts.hovering = target.is_some();
        if let Some((_, h)) = target {
            ui.ctx().set_cursor_icon(match h {
                Handle::Move => CursorIcon::Move,
                Handle::Resize(sx, sy) => resize_cursor(sx, sy),
            });
        }
        if pressed && resp.hovered() {
            let Some((id, handle)) = target else {
                self.images.charts.selected = None;
                return false;
            };
            if self.edit.is_some() && !self.settle_edit() {
                return true;
            }
            resp.request_focus();
            self.images.charts.selected = Some((self.sheet, id));
            self.images.deselect_picture();
            self.marquee = false;
            if let Some(orig) = self.sh().charts.iter().find(|c| c.id == id).cloned() {
                self.images.charts.drag = Some(ChartDrag { handle, start: p, orig, moved: false });
            }
            return true;
        }
        false
    }

    pub(super) fn chart_menu_hit(&mut self, g: &Geo, p: Pos2) -> bool {
        let hit = if g.view.contains(p) && !self.formula_ref_point() { self.chart_hit(g, p) } else { None };
        self.images.charts.menu = hit.is_some();
        if let Some((id, _)) = hit {
            self.images.charts.selected = Some((self.sheet, id));
            self.images.deselect_picture();
        }
        self.images.charts.menu
    }

    pub(super) fn chart_menu_open(&self) -> bool {
        self.images.charts.menu && self.chart_selected()
    }

    /// The Chart Design tab of the toolstrip.
    pub(super) fn chart_tab(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.type", Icon::Chart, |ui| {
            if toolstrip::large(ui, Icon::ChartColumn, tr("chart.change_type"), "", a, false).clicked() {
                self.open_chart_editor(Tab::Type);
            }
            toolstrip::large_menu(ui, Icon::Chart, tr("chart.templates"), tr("chart.templates_tip"), a, |ui| self.chart_template_items(ui, true));
            if toolstrip::large(ui, Icon::Save, tr("chart.save_template"), tr("chart.save_template_tip"), a, false).clicked() {
                self.save_chart_template();
            }
        });
        b.group("toolstrip.g.data", Icon::Table, |ui| {
            if toolstrip::large(ui, Icon::Table, tr("chart.select_data"), "", a, false).clicked() {
                self.open_chart_editor(Tab::Data);
            }
            if toolstrip::large(ui, Icon::Orientation, tr("chart.switch"), "", a, false).clicked()
                && let Some(mut c) = self.selected_chart().cloned()
            {
                let by_rows = !c.by_rows;
                self.wb.rebuild_series(self.sheet, &mut c, by_rows);
                self.wb.set_chart(self.sheet, c);
            }
        });
        b.group("toolstrip.g.chart_layouts", Icon::Chart, |ui| {
            if toolstrip::large(ui, Icon::Chart, tr("chart.elements"), "", a, false).clicked() {
                self.open_chart_editor(Tab::Elements);
            }
            if toolstrip::large(ui, Icon::ChartLine, tr("chart.axes"), "", a, false).clicked() {
                self.open_chart_editor(Tab::Axes);
            }
            if toolstrip::large(ui, Icon::Styles, tr("chart.style"), "", a, false).clicked() {
                self.open_chart_editor(Tab::Style);
            }
        });
        b.group("toolstrip.g.arrange", Icon::Chart, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Copy), tr("chart.copy_picture"), "", a, false).clicked() {
                    self.copy_chart(ui.ctx(), false);
                }
                if toolstrip::small(ui, Some(Icon::Save), &format!("{}…", tr("chart.save_picture")), tr("chart.save_picture_tip"), a, false).clicked() {
                    self.save_chart_picture(ui.ctx());
                }
                if toolstrip::small(ui, Some(Icon::Eraser), tr("chart.delete"), "", a, false).clicked() {
                    self.delete_chart();
                }
            });
        });
    }

    pub(super) fn chart_context_menu(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(tr("edit.cut")).shortcut_text("Ctrl+X")).clicked() {
            self.copy_chart(ui.ctx(), true);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("edit.copy")).shortcut_text("Ctrl+C")).clicked() {
            self.copy_chart(ui.ctx(), false);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("chart.delete")).shortcut_text("Del")).clicked() {
            self.delete_chart();
            ui.close();
        }
        ui.separator();
        for (tab, key) in [(Tab::Type, "chart.change_type"), (Tab::Data, "chart.select_data"), (Tab::Elements, "chart.elements"), (Tab::Axes, "chart.axes"), (Tab::Style, "chart.style")] {
            if ui.button(format!("{}…", tr(key))).clicked() {
                self.open_chart_editor(tab);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(tr("chart.switch")).clicked() {
            if let Some(mut c) = self.selected_chart().cloned() {
                let by_rows = !c.by_rows;
                self.wb.rebuild_series(self.sheet, &mut c, by_rows);
                self.wb.set_chart(self.sheet, c);
            }
            ui.close();
        }
        if ui.button(tr("chart.copy_picture")).clicked() {
            self.copy_chart(ui.ctx(), false);
            ui.close();
        }
        if ui.button(format!("{}…", tr("chart.save_picture"))).clicked() {
            ui.close();
            self.save_chart_picture(ui.ctx());
        }
        if ui.button(format!("{}…", tr("chart.save_template"))).clicked() {
            ui.close();
            self.save_chart_template();
        }
    }

    // --- keyboard ------------------------------------------------------------------

    /// Keys for the selected chart. True when the grid must not see them.
    pub(super) fn chart_keys(&mut self, ui: &egui::Ui, events: &[Event]) -> bool {
        if self.images.charts.editor.is_some() || self.images.charts.spark.is_some() {
            return true;
        }
        let Some(c) = self.selected_chart().cloned() else { return false };
        for ev in events {
            match ev {
                Event::Copy => self.copy_chart(ui.ctx(), false),
                Event::Cut => self.copy_chart(ui.ctx(), true),
                Event::Paste(_) => {
                    self.images.charts.selected = None;
                    return false;
                }
                Event::Key { key, pressed: true, modifiers: m, .. } => {
                    let step = if m.shift { 10.0 } else { 1.0 };
                    let nudge = match key {
                        Key::ArrowLeft => Some(vec2(-step, 0.0)),
                        Key::ArrowRight => Some(vec2(step, 0.0)),
                        Key::ArrowUp => Some(vec2(0.0, -step)),
                        Key::ArrowDown => Some(vec2(0.0, step)),
                        _ => None,
                    };
                    if let Some(d) = nudge {
                        let mut moved = c.clone();
                        let (x, y) = c.pos(self.sh());
                        moved.set_pos(self.sh(), x + d.x, y + d.y);
                        self.wb.set_chart(self.sheet, moved);
                        continue;
                    }
                    match key {
                        Key::Delete | Key::Backspace => self.delete_chart(),
                        Key::Escape => self.images.charts.selected = None,
                        Key::Z if m.command && m.shift => self.redo(),
                        Key::Z if m.command => self.undo(),
                        Key::Y if m.command => self.redo(),
                        Key::Enter => self.open_chart_editor(Tab::Type),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        true
    }

    fn delete_chart(&mut self) {
        if let Some((s, id)) = self.images.charts.selected.take() {
            if s == self.sheet {
                self.wb.remove_chart(self.sheet, id);
            }
        }
    }

    // --- clipboard -----------------------------------------------------------------

    /// Copies the selected chart: a picture on the system clipboard (and in
    /// the app's own, for documents) and the chart itself for the sheet.
    pub(super) fn copy_chart(&mut self, ctx: &egui::Context, cut: bool) {
        let Some(c) = self.selected_chart().cloned() else { return };
        let (data, _) = self.chart_data(&c, true);
        // Twice the sheet size for a sharp picture.
        let img = charts::render(ctx, &c, &data, c.width, c.height, 2.0, &self.wb.locale);
        let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
        let hash = super::images_ui::pixel_hash(&bytes);
        charts::set_app_clip(charts::encode_png(&img).map(Arc::from));
        // For documents: the chart with a copy of its data.
        let book = apex_io::chart_object::book(&apex_io::chart_object::standalone(&self.wb, self.sheet, &c, "Sheet1"));
        charts::set_chart_clip(Some((hash, book, (c.width * 0.75, c.height * 0.75))));
        ctx.copy_image(img);
        self.images.charts.clip = Some((c, hash));
        if cut {
            self.delete_chart();
        }
    }

    /// Our own chart coming back from the clipboard: pasted as a chart.
    pub(super) fn paste_chart_if_ours(&mut self, bytes: &[u8]) -> bool {
        let Some((c, hash)) = self.images.charts.clip.clone() else { return false };
        let Some(d) = crate::images::decode(bytes) else { return false };
        if super::images_ui::pixel_hash(&d.rgba) != hash {
            return false;
        }
        self.settle_edit();
        let mut c = c;
        let (x, y) = (self.sh().col_x(self.cursor.col), self.sh().row_y(self.cursor.row));
        c.set_pos(self.sh(), x, y);
        c.name.clear();
        let id = self.wb.add_chart(self.sheet, c);
        self.images.charts.selected = Some((self.sheet, id));
        true
    }

    // --- inserting -----------------------------------------------------------------

    /// The chart types to insert, each with its icon.
    pub(super) fn chart_type_items(&mut self, ui: &mut egui::Ui) {
        let pick = chart_type_menu(ui);
        ui.separator();
        crate::widgets::menu_button(ui, tr("chart.templates"), |ui| self.chart_template_items(ui, false));
        if let Some(i) = pick {
            let (key, _, kind, grouping, flags) = CHART_TYPES[i];
            self.insert_chart(kind, grouping, flags, key == "chart.t.combo");
        }
    }

    /// The Sparklines group of the Insert tab.
    pub(super) fn sparkline_buttons(&mut self, ui: &mut egui::Ui, a: Color32) {
        use crate::toolstrip;
        toolstrip::column(ui, |ui| {
            for (key, kind) in [("spark.line", SparkKind::Line), ("spark.column", SparkKind::Column), ("spark.winloss", SparkKind::WinLoss)] {
                if toolstrip::small(ui, Some(Icon::Sparkline), tr(key), tr("spark.insert"), a, false).clicked() {
                    self.open_sparkline_dialog(Some(kind));
                }
            }
        });
        if self.sparkline_at_cursor().is_some() {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Sparkline), &format!("{}…", tr("spark.edit")), "", a, false).clicked() {
                    self.open_sparkline_dialog(None);
                }
            });
        }
    }

    /// A chart of the selection (or of the data around the cursor).
    pub(super) fn insert_chart(&mut self, kind: ChartType, grouping: Grouping, flags: u8, combo: bool) {
        if !self.settle_edit() {
            return;
        }
        let mut sel = self.sel();
        if sel.is_single() {
            sel = self.current_region(sel.start);
        }
        let used = self.wb.clamp_to_used(self.sheet, &sel);
        if used.is_none() || sel.cell_count() > 5_000_000 {
            self.notices.push(Notice::info(tr("chart.no_data")));
            return;
        }
        let mut c = self.wb.chart_from_range(self.sheet, sel, kind, None);
        apply_type(&mut c, grouping, flags, combo);
        // Next to the data, in view, at Excel's default size.
        let sh = self.sh();
        let (x, y) = (sh.col_x(sel.end.col + 1) + 16.0, sh.row_y(sel.start.row));
        let (vx, vy) = (self.scroll.x, self.scroll.y);
        let (vw, vh) = self.view_size;
        let x = if x + c.width > vx + vw { (vx + vw - c.width - 16.0).max(vx) } else { x };
        let y = y.clamp(vy, (vy + vh - c.height - 8.0).max(vy));
        c.set_pos(sh, x, y);
        let id = self.wb.add_chart(self.sheet, c);
        self.images.charts.selected = Some((self.sheet, id));
        self.images.deselect_picture();
    }

    /// The block of filled cells around `c` (Excel's current region).
    fn current_region(&self, c: CellRef) -> Range {
        let sh = self.sh();
        let filled = |r: u32, col: u32| !matches!(sh.value(CellRef::new(r, col)), apex_sheet::Value::Empty);
        let mut r = Range::single(c);
        loop {
            let mut grown = r;
            let (r0, r1, c0, c1) = (r.start.row, r.end.row, r.start.col, r.end.col);
            if r0 > 0 && (c0.saturating_sub(1)..=c1 + 1).any(|col| filled(r0 - 1, col)) {
                grown.start.row -= 1;
            }
            if r1 + 1 < apex_sheet::MAX_ROWS && (c0.saturating_sub(1)..=c1 + 1).any(|col| filled(r1 + 1, col)) {
                grown.end.row += 1;
            }
            if c0 > 0 && (r0.saturating_sub(1)..=r1 + 1).any(|row| filled(row, c0 - 1)) {
                grown.start.col -= 1;
            }
            if c1 + 1 < apex_sheet::MAX_COLS && (r0.saturating_sub(1)..=r1 + 1).any(|row| filled(row, c1 + 1)) {
                grown.end.col += 1;
            }
            if grown == r || grown.cell_count() > 1_000_000 {
                return r;
            }
            r = grown;
        }
    }

    // --- editor --------------------------------------------------------------------

    fn open_chart_editor(&mut self, tab: Tab) {
        let Some(c) = self.selected_chart().cloned() else { return };
        let loc = self.wb.locale;
        let range = c.source.as_ref().map(|s| s.to_display(&loc)).or_else(|| series_block(&c).map(|r| r.to_display(&loc))).unwrap_or_default();
        let totals = c.totals.iter().map(|t| (t + 1).to_string()).collect::<Vec<_>>().join(if loc.decimal == ',' { "; " } else { ", " });
        self.images.charts.preview = Some(c.clone());
        self.images.charts.editor = Some(Editor { sheet: self.sheet, chart: c, tab, range, series: 0, fields: Default::default(), fields_for: None, error: None, totals, fresh: true });
    }

    pub(super) fn chart_dialogs(&mut self, ctx: &egui::Context) {
        self.chart_editor(ctx);
        self.sparkline_dialog(ctx);
    }

    fn chart_editor(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.images.charts.editor.take() else { return };
        if ed.sheet >= self.wb.sheets.len() {
            self.images.charts.preview = None;
            return;
        }
        let sheet_name = self.wb.sheets[ed.sheet].name.clone();
        let wb = &self.wb;
        let data = wb.chart_data(ed.sheet, &ed.chart);
        let mut res = editor_ui(ctx, &mut ed, wb, &sheet_name, &data);
        // The key or click that opened it must not close it.
        if std::mem::take(&mut ed.fresh) && res != DialogResult::None && !ctx.input(|i| i.key_pressed(Key::Escape)) {
            res = DialogResult::None;
        }
        match res {
            DialogResult::None => {
                self.images.charts.preview = Some(ed.chart.clone());
                self.images.charts.editor = Some(ed);
            }
            DialogResult::Ok => {
                self.images.charts.preview = None;
                let sheet = ed.sheet;
                self.wb.set_chart(sheet, ed.chart);
            }
            DialogResult::Cancel => self.images.charts.preview = None,
        }
    }

    /// Drops a selection that no longer exists (undo, another sheet).
    pub(super) fn charts_frame(&mut self) {
        let st = &mut self.images.charts;
        if st.selected.is_some() && !st.selected.is_some_and(|(s, id)| s == self.sheet && self.wb.sheets.get(s).is_some_and(|sh| sh.charts.iter().any(|c| c.id == id))) {
            st.selected = None;
            st.drag = None;
            if st.editor.is_none() {
                st.preview = None;
            }
        }
        // Forget caches of charts that are gone.
        let cache = st.cache.get_mut();
        if cache.data.len() > 64 || cache.shapes.len() > 64 {
            let live: std::collections::HashSet<(usize, u32)> = self.wb.sheets.iter().enumerate().flat_map(|(s, sh)| sh.charts.iter().map(move |c| (s, c.id))).collect();
            cache.data.retain(|k, _| live.contains(k));
            cache.shapes.retain(|k, _| live.contains(k));
        }
    }

    // --- sparklines ----------------------------------------------------------------

    fn sparkline_at_cursor(&self) -> Option<usize> {
        self.sh().sparklines.iter().position(|g| g.sparklines.iter().any(|s| s.cell == self.cursor))
    }

    fn open_sparkline_dialog(&mut self, kind: Option<SparkKind>) {
        let loc = self.wb.locale;
        let name = self.sh().name.clone();
        match (kind, self.sparkline_at_cursor()) {
            (None, Some(i)) => {
                let group = self.sh().sparklines[i].clone();
                let data = group.sparklines.first().map(|s| s.data.to_display(&loc)).unwrap_or_default();
                let cells: Vec<CellRef> = group.sparklines.iter().map(|s| s.cell).collect();
                let location = bounding(&cells).map(|r| r.to_a1()).unwrap_or_default();
                self.images.charts.spark = Some(SparkDlg { sheet: self.sheet, editing: Some(i), group, data, location, error: None, fresh: true });
            }
            (k, _) => {
                let sel = self.sel();
                // Data: the selection; location: the column right of it.
                let loc_cell = Range::new(CellRef::new(sel.start.row, (sel.end.col + 1).min(apex_sheet::MAX_COLS - 1)), CellRef::new(sel.end.row, (sel.end.col + 1).min(apex_sheet::MAX_COLS - 1)));
                let data = if sel.is_single() { String::new() } else { format!("{}!{}", apex_sheet::chart::quote_sheet(&name), sel.to_a1()) };
                let group = SparkGroup { kind: k.unwrap_or_default(), ..Default::default() };
                self.images.charts.spark = Some(SparkDlg { sheet: self.sheet, editing: None, group, data, location: if sel.is_single() { String::new() } else { loc_cell.to_a1() }, error: None, fresh: true });
            }
        }
    }

    fn sparkline_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.images.charts.spark.take() else { return };
        let mut clear = false;
        let mut res = widgets::dialog(ctx, "sparklines", &tr("spark.title"), 440.0, |ui| clear = spark_ui(ui, &mut d));
        // The key or click that opened it must not close it.
        if std::mem::take(&mut d.fresh) && res != DialogResult::None && !ctx.input(|i| i.key_pressed(Key::Escape)) {
            res = DialogResult::None;
        }
        if clear {
            if let Some(i) = d.editing {
                let mut list = self.wb.sheets[d.sheet].sparklines.clone();
                if i < list.len() {
                    list.remove(i);
                    self.wb.set_sparklines(d.sheet, list);
                }
            }
            return;
        }
        match res {
            DialogResult::None => self.images.charts.spark = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                if d.sheet >= self.wb.sheets.len() {
                    return;
                }
                match self.build_sparklines(&d) {
                    Ok(group) => match d.editing {
                        Some(i) => {
                            let mut list = self.wb.sheets[d.sheet].sparklines.clone();
                            if i < list.len() {
                                list[i] = group;
                            }
                            self.wb.set_sparklines(d.sheet, list);
                        }
                        None => self.wb.add_sparklines(d.sheet, group),
                    },
                    Err(e) => {
                        d.error = Some(e);
                        self.images.charts.spark = Some(d);
                    }
                }
            }
        }
    }

    /// The group the dialog describes: one sparkline per location cell,
    /// each with the matching row (or column) of the data range.
    fn build_sparklines(&self, d: &SparkDlg) -> Result<SparkGroup, String> {
        let name = self.wb.sheets[d.sheet].name.clone();
        let data = parse_ref(&d.data, &name, &self.wb.locale).map_err(|_| tr("spark.bad_data").to_string())?;
        let Some((dsheet, drange)) = data.area() else { return Err(tr("spark.bad_data").into()) };
        let dsheet = dsheet.map(|s| s.to_string()).unwrap_or(name.clone());
        let loc = Range::parse(d.location.trim().trim_start_matches('=').trim_start_matches('$')).or_else(|| Range::parse(&d.location.replace('$', ""))).ok_or_else(|| tr("spark.bad_location").to_string())?;
        if loc.rows() != 1 && loc.cols() != 1 {
            return Err(tr("spark.bad_location").into());
        }
        let n = loc.rows().max(loc.cols());
        let mut group = d.group.clone();
        group.sparklines.clear();
        let cells: Vec<CellRef> = if loc.cols() == 1 { (loc.start.row..=loc.end.row).map(|r| CellRef::new(r, loc.start.col)).collect() } else { (loc.start.col..=loc.end.col).map(|c| CellRef::new(loc.start.row, c)).collect() };
        // Rows of the data go with a column of cells, and so on.
        let by_rows = if n == 1 { drange.rows() == 1 || drange.cols() != 1 } else if loc.cols() == 1 { drange.rows() == n } else { drange.cols() != n };
        let lines = if by_rows { drange.rows() } else { drange.cols() };
        if n > 1 && lines != n {
            return Err(tr("spark.mismatch").into());
        }
        for (i, cell) in cells.into_iter().enumerate() {
            let i = i as u32;
            let r = if n == 1 {
                drange
            } else if by_rows {
                Range::new(CellRef::new(drange.start.row + i, drange.start.col), CellRef::new(drange.start.row + i, drange.end.col))
            } else {
                Range::new(CellRef::new(drange.start.row, drange.start.col + i), CellRef::new(drange.end.row, drange.start.col + i))
            };
            group.sparklines.push(Sparkline { cell, data: DataRef::parse(&format!("{}!{}", apex_sheet::chart::quote_sheet(&dsheet), r.to_a1())) });
        }
        Ok(group)
    }
}

/// The block that the series of a chart read from files cover (names,
/// values and categories on one sheet), shown as its data range.
fn series_block(c: &Chart) -> Option<DataRef> {
    let mut sheet: Option<Option<Arc<str>>> = None;
    let mut cells = Vec::new();
    for s in &c.series {
        for r in [&s.name, &s.values, &s.categories] {
            if r.formula.is_none() {
                continue;
            }
            let (sh, area) = r.area()?;
            if sheet.get_or_insert_with(|| sh.clone()) != &sh {
                return None;
            }
            cells.push(area.start);
            cells.push(area.end);
        }
    }
    let block = bounding(&cells)?;
    let name = sheet.flatten()?;
    Some(DataRef::range(&name, block))
}

fn bounding(cells: &[CellRef]) -> Option<Range> {
    let first = *cells.first()?;
    Some(cells.iter().fold(Range::single(first), |r, c| Range::new(CellRef::new(r.start.row.min(c.row), r.start.col.min(c.col)), CellRef::new(r.end.row.max(c.row), r.end.col.max(c.col)))))
}

// --- editor panels -------------------------------------------------------------------------

/// Pages of the chart editor, in order.
const PAGES: [(Tab, Icon, &str); 5] = [
    (Tab::Type, Icon::Chart, "chart.tab_type"),
    (Tab::Data, Icon::Table, "chart.tab_data"),
    (Tab::Elements, Icon::Caption, "chart.tab_elements"),
    (Tab::Axes, Icon::Gridlines, "chart.tab_axes"),
    (Tab::Style, Icon::Styles, "chart.tab_style"),
];

/// Width of a page of the editor, and of the preview beside it.
const PAGE_W: f32 = 500.0;
const PREVIEW_W: f32 = 300.0;

const LABEL_W: f32 = widgets::FORM_LABEL_W;

fn editor_ui(ctx: &egui::Context, ed: &mut Editor, wb: &apex_sheet::Workbook, sheet_name: &str, data: &ChartData) -> DialogResult {
    let pages: Vec<(Icon, &'static str)> = PAGES.iter().map(|(_, i, k)| (*i, *k)).collect();
    let mut page = PAGES.iter().position(|(t, ..)| *t == ed.tab).unwrap_or(0);
    // The page edits the chart, the preview beside it shows it.
    let cell = RefCell::new(&mut *ed);
    let res = widgets::nav_dialog_aside(
        ctx,
        "chart-editor",
        tr("chart.editor"),
        &pages,
        &mut page,
        vec2(PAGE_W, 500.0),
        PREVIEW_W,
        |ui, i| {
            let ed = &mut *cell.borrow_mut();
            match PAGES[i].0 {
                Tab::Type => type_ui(ui, ed, data),
                Tab::Data => data_ui(ui, ed, wb, sheet_name),
                Tab::Elements => elements_ui(ui, ed, data, &wb.locale, sheet_name),
                Tab::Axes => axes_ui(ui, &mut ed.chart),
                Tab::Style => style_ui(ui, ed, data),
            }
        },
        |ui| preview_ui(ui, &cell.borrow().chart, data),
    );
    drop(cell);
    ed.tab = PAGES[page].0;
    res
}

/// The chart as it is being edited.
fn preview_ui(ui: &mut egui::Ui, c: &Chart, data: &ChartData) {
    widgets::section(ui, tr("chart.preview"));
    let (rect, _) = ui.allocate_exact_size(vec2(PREVIEW_W, PREVIEW_W * 0.72), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, Color32::WHITE);
    charts::paint(&ui.painter_at(rect.shrink(1.0)), rect.shrink(1.0), c, data, 0.6, &apex_sheet::Locale::EN);
    ui.painter().rect_stroke(rect, 4.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    ui.add_space(6.0);
    ui.label(RichText::new(tr("chart.preview_hint")).small().weak());
}

/// A variant of a chart type: its name, type, grouping and flags (bit 0
/// markers, 1 lines, 2 smooth, 4 3-D, 5 filled; 9 a combo chart).
type Variant = (&'static str, ChartType, Grouping, u8);

/// Chart families: name, icon and variants.
const FAMILIES: [(&str, Icon, &[Variant]); 15] = [
    ("chart.type.column", Icon::ChartColumn, &[("chart.v.clustered", ChartType::Column, Grouping::Clustered, 0), ("chart.v.stacked", ChartType::Column, Grouping::Stacked, 0), ("chart.v.percent", ChartType::Column, Grouping::Percent, 0), ("chart.v.clustered_3d", ChartType::Column, Grouping::Clustered, 16), ("chart.v.stacked_3d", ChartType::Column, Grouping::Stacked, 16), ("chart.v.percent_3d", ChartType::Column, Grouping::Percent, 16), ("chart.v.combo", ChartType::Column, Grouping::Clustered, 9)]),
    ("chart.type.bar", Icon::ChartBar, &[("chart.v.clustered", ChartType::Bar, Grouping::Clustered, 0), ("chart.v.stacked", ChartType::Bar, Grouping::Stacked, 0), ("chart.v.percent", ChartType::Bar, Grouping::Percent, 0), ("chart.v.clustered_3d", ChartType::Bar, Grouping::Clustered, 16), ("chart.v.stacked_3d", ChartType::Bar, Grouping::Stacked, 16), ("chart.v.percent_3d", ChartType::Bar, Grouping::Percent, 16)]),
    ("chart.type.line", Icon::ChartLine, &[("chart.v.line", ChartType::Line, Grouping::Clustered, 0), ("chart.v.markers", ChartType::Line, Grouping::Clustered, 1), ("chart.v.stacked", ChartType::Line, Grouping::Stacked, 0), ("chart.v.percent", ChartType::Line, Grouping::Percent, 0), ("chart.v.line_3d", ChartType::Line, Grouping::Clustered, 16)]),
    ("chart.type.area", Icon::ChartArea, &[("chart.v.area", ChartType::Area, Grouping::Clustered, 0), ("chart.v.stacked", ChartType::Area, Grouping::Stacked, 0), ("chart.v.percent", ChartType::Area, Grouping::Percent, 0), ("chart.v.area_3d", ChartType::Area, Grouping::Clustered, 16), ("chart.v.stacked_3d", ChartType::Area, Grouping::Stacked, 16)]),
    ("chart.type.pie", Icon::ChartPie, &[("chart.type.pie", ChartType::Pie, Grouping::Clustered, 0), ("chart.v.pie_3d", ChartType::Pie, Grouping::Clustered, 16), ("chart.type.doughnut", ChartType::Doughnut, Grouping::Clustered, 0)]),
    ("chart.family.scatter", Icon::ChartScatter, &[("chart.v.markers_only", ChartType::Scatter, Grouping::Clustered, 1), ("chart.v.lines_markers", ChartType::Scatter, Grouping::Clustered, 3), ("chart.v.lines_only", ChartType::Scatter, Grouping::Clustered, 2), ("chart.v.smooth", ChartType::Scatter, Grouping::Clustered, 7), ("chart.type.bubble", ChartType::Bubble, Grouping::Clustered, 0)]),
    ("chart.type.radar", Icon::ChartRadar, &[("chart.type.radar", ChartType::Radar, Grouping::Clustered, 0), ("chart.v.markers", ChartType::Radar, Grouping::Clustered, 1), ("chart.v.filled", ChartType::Radar, Grouping::Clustered, 32)]),
    ("chart.type.stock", Icon::ChartStock, &[("chart.type.stock", ChartType::Stock, Grouping::Clustered, 0)]),
    ("chart.type.surface", Icon::ChartSurface, &[("chart.v.surface_3d", ChartType::Surface, Grouping::Clustered, 16), ("chart.v.contour", ChartType::Surface, Grouping::Clustered, 0)]),
    ("chart.family.hierarchy", Icon::ChartTreemap, &[("chart.type.treemap", ChartType::Treemap, Grouping::Clustered, 0), ("chart.type.sunburst", ChartType::Sunburst, Grouping::Clustered, 0)]),
    ("chart.family.statistic", Icon::ChartHistogram, &[("chart.type.histogram", ChartType::Histogram, Grouping::Clustered, 0), ("chart.type.pareto", ChartType::Pareto, Grouping::Clustered, 0), ("chart.type.boxwhisker", ChartType::BoxWhisker, Grouping::Clustered, 0)]),
    ("chart.type.waterfall", Icon::ChartWaterfall, &[("chart.type.waterfall", ChartType::Waterfall, Grouping::Clustered, 0)]),
    ("chart.type.funnel", Icon::ChartFunnel, &[("chart.type.funnel", ChartType::Funnel, Grouping::Clustered, 0)]),
    ("chart.type.mekko", Icon::ChartMekko, &[("chart.type.mekko", ChartType::Mekko, Grouping::Clustered, 0)]),
    ("chart.type.timeline", Icon::ChartTimeline, &[("chart.type.timeline", ChartType::Timeline, Grouping::Clustered, 0)]),
];

fn is_variant(c: &Chart, &(_, kind, grouping, flags): &Variant) -> bool {
    let combo = flags == 9;
    c.kind == kind
        && c.grouping == grouping
        && combo == c.is_combo()
        && match kind {
            ChartType::Line => c.markers == (flags & 1 != 0),
            ChartType::Scatter => c.markers == (flags & 1 != 0) && c.lines == (flags & 2 != 0) && c.smooth == (flags & 4 != 0),
            ChartType::Radar => c.markers == (flags & 1 != 0) && c.filled == (flags & 32 != 0),
            _ => true,
        }
        && (!kind.has_3d() || c.view3d == (flags & 16 != 0))
}

fn apply_variant(c: &mut Chart, &(_, kind, grouping, flags): &Variant) {
    c.set_kind(kind);
    c.grouping = grouping;
    if flags == 9 {
        c.make_combo();
    } else {
        for s in &mut c.series {
            s.kind = None;
            s.secondary = false;
        }
    }
    match kind {
        ChartType::Line => c.markers = flags & 1 != 0,
        ChartType::Scatter => {
            c.markers = flags & 1 != 0;
            c.lines = flags & 2 != 0;
            c.smooth = flags & 4 != 0;
        }
        ChartType::Radar => {
            c.markers = flags & 1 != 0;
            c.filled = flags & 32 != 0;
        }
        _ => {}
    }
    if kind.has_3d() {
        c.view3d = flags & 16 != 0;
    }
}

/// A tile with a chart family's icon and name.
fn family_tile(ui: &mut egui::Ui, icon: Icon, label: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(66.0, 62.0), egui::Sense::click());
    widgets::name(&resp, egui::WidgetType::SelectableLabel, label, Some(on));
    let v = ui.visuals();
    if on {
        ui.painter().rect_filled(rect, 5.0, v.selection.bg_fill.gamma_multiply(0.3));
        ui.painter().rect_stroke(rect, 5.0, Stroke::new(1.5, theme::SHEET_ACCENT), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, v.widgets.hovered.weak_bg_fill);
    }
    crate::icons::paint_accent(ui.painter(), Rect::from_center_size(pos2(rect.center().x, rect.min.y + 20.0), vec2(26.0, 26.0)), icon, v.text_color(), theme::ACCENT);
    let p = ui.painter().with_clip_rect(rect);
    p.text(pos2(rect.center().x, rect.max.y - 13.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.5), v.text_color());
    resp.on_hover_text(label)
}

/// A thumbnail of `sample` (the chart itself, simplified) with a caption.
fn chart_tile(ui: &mut egui::Ui, sample: &Chart, data: &ChartData, size: Vec2, caption: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size + vec2(0.0, if caption.is_empty() { 0.0 } else { 20.0 }), egui::Sense::click());
    widgets::name(&resp, egui::WidgetType::SelectableLabel, caption, Some(on));
    let img = Rect::from_min_size(rect.min, size);
    if ui.is_rect_visible(rect) {
        let mut s = sample.clone();
        s.legend = LegendPos::None;
        s.labels = Default::default();
        s.show_title = false;
        s.x_axis.title.clear();
        s.y_axis.title.clear();
        ui.painter().rect_filled(img, 3.0, Color32::WHITE);
        charts::paint(&ui.painter_at(img), img, &s, data, 0.35, &apex_sheet::Locale::EN);
        let v = ui.visuals();
        let stroke = if on {
            Stroke::new(2.5, theme::SHEET_ACCENT)
        } else if resp.hovered() {
            Stroke::new(1.5, v.widgets.hovered.bg_stroke.color)
        } else {
            v.widgets.noninteractive.bg_stroke
        };
        ui.painter().rect_stroke(img, 3.0, stroke, egui::StrokeKind::Outside);
        if !caption.is_empty() {
            let color = if on { v.strong_text_color() } else { v.text_color() };
            ui.painter().with_clip_rect(rect).text(pos2(rect.center().x, img.max.y + 10.0), egui::Align2::CENTER_CENTER, caption, egui::FontId::proportional(12.0), color);
        }
    }
    resp
}

fn type_ui(ui: &mut egui::Ui, ed: &mut Editor, data: &ChartData) {
    let c = &mut ed.chart;
    let family = FAMILIES.iter().position(|(_, _, vs)| vs.iter().any(|v| is_variant(c, v))).or_else(|| FAMILIES.iter().position(|(_, _, vs)| vs.iter().any(|v| v.1 == c.kind))).unwrap_or(0);
    widgets::section(ui, tr("chart.chart_type"));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (i, (key, icon, variants)) in FAMILIES.iter().enumerate() {
            if family_tile(ui, *icon, tr(key), i == family).clicked() && i != family {
                apply_variant(c, &variants[0]);
            }
        }
    });
    widgets::section(ui, tr("chart.variant"));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
        for v in FAMILIES[family].2 {
            let mut sample = c.clone();
            apply_variant(&mut sample, v);
            // A combo chart draws its last series another way: it takes two.
            let can = v.3 != 9 || c.series.len() > 1;
            if ui.add_enabled_ui(can, |ui| chart_tile(ui, &sample, data, vec2(112.0, 76.0), tr(v.0), is_variant(c, v))).inner.clicked() {
                apply_variant(c, v);
            }
        }
    });
    let has_options = matches!(c.kind, ChartType::Column | ChartType::Bar | ChartType::Histogram | ChartType::Waterfall | ChartType::Pie | ChartType::Doughnut | ChartType::Pareto | ChartType::Funnel | ChartType::BoxWhisker | ChartType::Bubble | ChartType::Sunburst);
    if !has_options {
        return;
    }
    widgets::section(ui, tr("chart.type_options"));
    widgets::form(ui, "chart_type_opts", |ui| {
        match c.kind {
            ChartType::Bubble => {
                widgets::form_row(ui, tr("chart.bubble_scale"), |ui| ui.add(egui::DragValue::new(&mut c.bubble_scale).range(1..=300).suffix(" %")));
            }
            ChartType::BoxWhisker => {
                widgets::form_row(ui, tr("chart.gap"), |ui| ui.add(egui::DragValue::new(&mut c.gap).range(0..=500).suffix(" %")));
                widgets::form_row(ui, "", |ui| ui.checkbox(&mut c.show_mean, tr("chart.show_mean")));
                widgets::form_row(ui, "", |ui| ui.checkbox(&mut c.show_outliers, tr("chart.show_outliers")));
            }
            ChartType::Sunburst => {
                widgets::form_row(ui, tr("chart.first_angle"), |ui| ui.add(egui::DragValue::new(&mut c.first_angle).range(0..=359).suffix("°")));
            }
            ChartType::Column | ChartType::Bar | ChartType::Histogram | ChartType::Waterfall | ChartType::Pareto | ChartType::Funnel => {
                widgets::form_row(ui, tr("chart.gap"), |ui| ui.add(egui::DragValue::new(&mut c.gap).range(0..=500).suffix(" %")));
                if matches!(c.kind, ChartType::Column | ChartType::Bar) && c.grouping == Grouping::Clustered {
                    widgets::form_row(ui, tr("chart.overlap"), |ui| ui.add(egui::DragValue::new(&mut c.overlap).range(-100..=100).suffix(" %")));
                }
            }
            ChartType::Pie | ChartType::Doughnut => {
                widgets::form_row(ui, tr("chart.first_angle"), |ui| ui.add(egui::DragValue::new(&mut c.first_angle).range(0..=359).suffix("°")));
                if c.kind == ChartType::Doughnut {
                    widgets::form_row(ui, tr("chart.hole"), |ui| ui.add(egui::DragValue::new(&mut c.hole).range(10..=90).suffix(" %")));
                }
            }
            _ => {}
        }
        if c.kind == ChartType::Histogram {
            widgets::form_row(ui, tr("chart.bins"), |ui| {
                ui.vertical(|ui| {
                    let mut mode = match c.bins {
                        Bins::Auto => 0,
                        Bins::Width(_) => 1,
                        Bins::Count(_) => 2,
                    };
                    for (i, key) in ["chart.bins_auto", "chart.bins_width", "chart.bins_count"].iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.radio_value(&mut mode, i, tr(key)).changed() {
                                c.bins = match i {
                                    1 => Bins::Width(1.0),
                                    2 => Bins::Count(5),
                                    _ => Bins::Auto,
                                };
                            }
                            match (&mut c.bins, i) {
                                (Bins::Width(w), 1) => {
                                    ui.add(egui::DragValue::new(w).range(1e-9..=1e12).speed(0.1));
                                }
                                (Bins::Count(n), 2) => {
                                    ui.add(egui::DragValue::new(n).range(1..=1000));
                                }
                                _ => {}
                            }
                        });
                    }
                });
            });
        }
        if c.kind == ChartType::Waterfall {
            widgets::form_row(ui, tr("chart.totals"), |ui| {
                if ui.add(egui::TextEdit::singleline(&mut ed.totals).desired_width(200.0).hint_text(tr("chart.totals_hint"))).changed() {
                    c.totals = ed.totals.split([',', ';', ' ']).filter_map(|t| t.trim().parse::<u32>().ok()).filter(|n| *n >= 1).map(|n| n - 1).collect();
                }
            });
        }
    });
}

fn data_ui(ui: &mut egui::Ui, ed: &mut Editor, wb: &apex_sheet::Workbook, sheet_name: &str) {
    let loc = wb.locale;
    let sheet = ed.sheet;
    widgets::section(ui, tr("chart.data_range"));
    ui.horizontal(|ui| {
        let apply = ui.add(egui::TextEdit::singleline(&mut ed.range).desired_width(PAGE_W - 110.0)).lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        // Off while the range typed is the one the chart has.
        let same = parse_ref(&ed.range, sheet_name, &loc).is_ok_and(|r| r.formula.is_some() && ed.chart.source.as_ref().is_some_and(|s| s.formula == r.formula));
        if ui.add_enabled(!same, egui::Button::new(tr("chart.apply"))).clicked() || apply {
            match parse_ref(&ed.range, sheet_name, &loc) {
                Ok(r) if r.area().is_some() => {
                    ed.chart.source = Some(r);
                    let by_rows = ed.chart.by_rows;
                    wb.rebuild_series(sheet, &mut ed.chart, by_rows);
                    ed.fields_for = None;
                    ed.error = None;
                }
                _ => ed.error = Some(tr("chart.bad_ref").into()),
            }
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(tr("chart.series_in"));
        let mut by_rows = ed.chart.by_rows;
        let a = ui.radio_value(&mut by_rows, false, tr("chart.columns")).changed();
        let b = ui.radio_value(&mut by_rows, true, tr("chart.rows")).changed();
        if (a || b) && ed.chart.source.is_some() {
            wb.rebuild_series(sheet, &mut ed.chart, by_rows);
            ed.fields_for = None;
        }
    });
    if let Some(e) = &ed.error {
        ui.colored_label(Color32::from_rgb(220, 60, 60), e);
    }
    widgets::section(ui, tr("chart.series"));
    let names: Vec<String> = wb.chart_data(sheet, &ed.chart).series.into_iter().map(|s| s.name).collect();
    ed.series = ed.series.min(ed.chart.series.len().saturating_sub(1));
    // The series as a list, with the buttons that change it beside.
    ui.horizontal_top(|ui| {
        egui::Frame::group(ui.style()).inner_margin(4.0).show(ui, |ui| {
            ui.set_width(PAGE_W - 170.0);
            egui::ScrollArea::vertical().id_salt("chart_series_list").max_height(112.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (i, n) in names.iter().enumerate() {
                    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::click());
                    widgets::name(&r, egui::WidgetType::SelectableLabel, n, Some(ed.series == i));
                    let v = ui.visuals();
                    if ed.series == i {
                        ui.painter().rect_filled(rect, 3.0, v.selection.bg_fill.gamma_multiply(0.45));
                    } else if r.hovered() {
                        ui.painter().rect_filled(rect, 3.0, v.widgets.hovered.weak_bg_fill);
                    }
                    ui.painter().rect_filled(Rect::from_center_size(pos2(rect.min.x + 14.0, rect.center().y), vec2(10.0, 10.0)), 2.0, charts::series_color(&ed.chart, i));
                    ui.painter().with_clip_rect(rect).text(pos2(rect.min.x + 28.0, rect.center().y), egui::Align2::LEFT_CENTER, n, egui::FontId::proportional(13.0), v.text_color());
                    if r.clicked() {
                        ed.series = i;
                    }
                }
                if names.is_empty() {
                    ui.label(RichText::new(tr("chart.no_series")).weak());
                }
                });
            });
        });
        ui.vertical(|ui| {
            let w = vec2(120.0, 24.0);
            if ui.add_enabled(ed.chart.series.len() < apex_sheet::chart::MAX_SERIES, egui::Button::new(tr("chart.add_series")).min_size(w)).clicked() {
                ed.chart.series.push(Series::default());
                ed.series = ed.chart.series.len() - 1;
            }
            if ui.add_enabled(!ed.chart.series.is_empty(), egui::Button::new(tr("chart.remove_series")).min_size(w)).clicked() {
                ed.chart.series.remove(ed.series);
                ed.series = ed.series.saturating_sub(1);
                ed.fields_for = None;
            }
            if ui.add_enabled(ed.series > 0, egui::Button::new(tr("chart.up")).min_size(w)).clicked() {
                ed.chart.series.swap(ed.series, ed.series - 1);
                ed.series -= 1;
            }
            if ui.add_enabled(ed.series + 1 < ed.chart.series.len(), egui::Button::new(tr("chart.down")).min_size(w)).clicked() {
                ed.chart.series.swap(ed.series, ed.series + 1);
                ed.series += 1;
            }
        });
    });
    if ed.chart.series.get(ed.series).is_some() {
        sync_fields(ed, &loc);
        ui.add_space(6.0);
        let cat_key = if ed.chart.kind.is_xy() { "chart.x_values" } else { "chart.categories" };
        let mut changed = [false; 5];
        let bubble = ed.chart.kind == ChartType::Bubble;
        let cells = ed.chart.labels.cells;
        widgets::form(ui, "chart_series_refs", |ui| {
            for (i, key) in ["chart.series_name", "chart.values", cat_key, "chart.bubble_sizes", "chart.label_range"].iter().enumerate() {
                if i == 3 && !bubble || i == 4 && !cells {
                    continue;
                }
                widgets::form_row(ui, tr(key), |ui| {
                    changed[i] = ui.add(egui::TextEdit::singleline(&mut ed.fields[i]).desired_width(250.0)).lost_focus();
                });
            }
        });
        for (i, ch) in changed.iter().enumerate() {
            if !ch {
                continue;
            }
            match parse_ref(&ed.fields[i], sheet_name, &loc) {
                Ok(r) => {
                    let s = &mut ed.chart.series[ed.series];
                    match i {
                        0 => s.name = r,
                        1 => s.values = r,
                        3 => s.sizes = r,
                        4 => s.label_range = r,
                        _ => {
                            // Categories are shared by all series, as in Excel.
                            for x in &mut ed.chart.series {
                                x.categories = r.clone();
                            }
                        }
                    }
                    ed.error = None;
                }
                Err(_) => ed.error = Some(tr("chart.bad_ref").into()),
            }
        }
    }
    widgets::section(ui, tr("chart.hidden_empty"));
    ui.checkbox(&mut ed.chart.visible_only, tr("chart.visible_only"));
    ui.add_space(4.0);
    ui.label(tr("chart.blanks"));
    ui.indent("blanks", |ui| {
        ui.radio_value(&mut ed.chart.blanks, Blanks::Gap, tr("chart.blanks_gap"));
        ui.radio_value(&mut ed.chart.blanks, Blanks::Zero, tr("chart.blanks_zero"));
        ui.radio_value(&mut ed.chart.blanks, Blanks::Span, tr("chart.blanks_span"));
    });
}

/// Loads the reference fields of the selected series.
fn sync_fields(ed: &mut Editor, loc: &apex_sheet::Locale) {
    let Some(s) = ed.chart.series.get(ed.series) else { return };
    if ed.fields_for == Some(ed.series) {
        return;
    }
    let (plus, minus) = match &s.error_bars {
        Some(ErrorBars { amount: ErrorAmount::Custom { plus, minus }, .. }) => (ref_text(plus, loc), ref_text(minus, loc)),
        _ => (String::new(), String::new()),
    };
    ed.fields = [ref_text(&s.name, loc), ref_text(&s.values, loc), ref_text(&s.categories, loc), ref_text(&s.sizes, loc), ref_text(&s.label_range, loc), plus, minus];
    ed.fields_for = Some(ed.series);
}

fn elements_ui(ui: &mut egui::Ui, ed: &mut Editor, data: &ChartData, loc: &apex_sheet::Locale, sheet_name: &str) {
    let c = &mut ed.chart;
    let field_w = PAGE_W - LABEL_W - 30.0;
    widgets::section(ui, tr("chart.title_legend"));
    widgets::form(ui, "chart_title_legend", |ui| {
        ui.checkbox(&mut c.show_title, tr("chart.title"));
        let auto = charts::shown_title(&Chart { title: String::new(), show_title: true, ..c.clone() }, data).unwrap_or_default();
        ui.add_enabled(c.show_title, egui::TextEdit::singleline(&mut c.title).desired_width(field_w).hint_text(auto));
        ui.end_row();
        widgets::form_row(ui, tr("chart.legend"), |ui| {
            egui::ComboBox::from_id_salt("chart_legend").width(160.0).selected_text(legend_name(c.legend)).show_ui(ui, |ui| {
                for l in [LegendPos::None, LegendPos::Right, LegendPos::Left, LegendPos::Top, LegendPos::Bottom] {
                    ui.selectable_value(&mut c.legend, l, legend_name(l));
                }
            });
        });
    });
    widgets::section(ui, tr("chart.labels"));
    egui::Grid::new("chart_labels").num_columns(2).min_col_width(LABEL_W).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.checkbox(&mut c.labels.value, tr("chart.label_value"));
        ui.checkbox(&mut c.labels.category, tr("chart.label_category"));
        ui.end_row();
        ui.checkbox(&mut c.labels.series, tr("chart.label_series"));
        if c.kind.is_round() {
            ui.checkbox(&mut c.labels.percent, tr("chart.label_percent"));
        }
        ui.end_row();
        ui.checkbox(&mut c.labels.cells, tr("chart.label_cells")).on_hover_text(tr("chart.label_cells_tip"));
        ui.end_row();
    });
    if c.kind.has_axes() && !c.kind.is_xy() && !matches!(c.kind, ChartType::Bar | ChartType::Radar | ChartType::Surface | ChartType::BoxWhisker | ChartType::Funnel) {
        widgets::section(ui, tr("chart.data_table"));
        ui.horizontal(|ui| {
            ui.checkbox(&mut c.data_table, tr("chart.data_table_show"));
            ui.add_enabled(c.data_table, egui::Checkbox::new(&mut c.table_keys, tr("chart.table_keys")));
        });
    }
    if c.kind.has_axes() {
        widgets::section(ui, tr("chart.gridlines"));
        egui::Grid::new("chart_grid").num_columns(2).min_col_width(LABEL_W).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.checkbox(&mut c.y_axis.major_grid, tr("chart.grid_value_major"));
            ui.checkbox(&mut c.y_axis.minor_grid, tr("chart.grid_value_minor"));
            ui.end_row();
            ui.checkbox(&mut c.x_axis.major_grid, tr("chart.grid_cat_major"));
            if c.kind == ChartType::Scatter {
                ui.checkbox(&mut c.x_axis.minor_grid, tr("chart.grid_cat_minor"));
            }
            ui.end_row();
        });
        widgets::section(ui, tr("chart.axis_titles"));
        widgets::form(ui, "chart_axis_titles", |ui| {
            widgets::form_row(ui, tr("chart.axis_title_x"), |ui| ui.add(egui::TextEdit::singleline(&mut c.x_axis.title).desired_width(field_w)));
            widgets::form_row(ui, tr("chart.axis_title_y"), |ui| ui.add(egui::TextEdit::singleline(&mut c.y_axis.title).desired_width(field_w)));
            if c.has_secondary() {
                widgets::form_row(ui, tr("chart.axis_title_y2"), |ui| ui.add(egui::TextEdit::singleline(&mut c.y2_axis.title).desired_width(field_w)));
            }
        });
    }
    if !matches!(c.kind, ChartType::Column | ChartType::Bar | ChartType::Line | ChartType::Area | ChartType::Scatter | ChartType::Bubble) || c.series.is_empty() {
        return;
    }
    ed.series = ed.series.min(c.series.len() - 1);
    widgets::section(ui, tr("chart.series_extras"));
    widgets::form(ui, "chart_extras_series", |ui| {
        widgets::form_row(ui, tr("chart.series"), |ui| {
            egui::ComboBox::from_id_salt("chart_trend_series").width(160.0).selected_text(data.series.get(ed.series).map_or(String::new(), |s| s.name.clone())).show_ui(ui, |ui| {
                for (i, s) in data.series.iter().enumerate() {
                    ui.selectable_value(&mut ed.series, i, &s.name);
                }
            });
        });
    });
    widgets::section(ui, tr("chart.trendline"));
    let kinds = [
        ("chart.trend_none", None),
        ("chart.trend_linear", Some(TrendKind::Linear)),
        ("chart.trend_exp", Some(TrendKind::Exponential)),
        ("chart.trend_log", Some(TrendKind::Logarithmic)),
        ("chart.trend_poly", Some(TrendKind::Polynomial(2))),
        ("chart.trend_power", Some(TrendKind::Power)),
        ("chart.trend_moving", Some(TrendKind::MovingAverage(2))),
    ];
    let same = |a: Option<TrendKind>, b: Option<TrendKind>| std::mem::discriminant(&a) == std::mem::discriminant(&b) && a.zip(b).is_none_or(|(x, y)| std::mem::discriminant(&x) == std::mem::discriminant(&y));
    widgets::form(ui, "chart_trend", |ui| {
        let s = &mut c.series[ed.series];
        widgets::form_row(ui, tr("chart.trend_type"), |ui| {
            let cur = kinds.iter().find(|(_, k)| same(s.trendline.map(|t| t.kind), *k)).map_or("", |(key, _)| tr(key));
            egui::ComboBox::from_id_salt("chart_trend_kind").width(160.0).selected_text(cur).show_ui(ui, |ui| {
                for (key, k) in kinds {
                    if ui.selectable_label(same(s.trendline.map(|t| t.kind), k), tr(key)).clicked() {
                        s.trendline = k.map(|kind| Trendline { kind, show_equation: s.trendline.is_some_and(|t| t.show_equation), show_r2: s.trendline.is_some_and(|t| t.show_r2) });
                    }
                }
            });
        });
        if let Some(t) = &mut s.trendline {
            match &mut t.kind {
                TrendKind::Polynomial(n) => {
                    widgets::form_row(ui, tr("chart.trend_order"), |ui| ui.add(egui::DragValue::new(n).range(2..=6)));
                }
                TrendKind::MovingAverage(n) => {
                    widgets::form_row(ui, tr("chart.trend_period"), |ui| ui.add(egui::DragValue::new(n).range(2..=255)));
                }
                _ => {}
            }
            if !matches!(t.kind, TrendKind::MovingAverage(_)) {
                ui.label("");
                ui.vertical(|ui| {
                    ui.checkbox(&mut t.show_equation, tr("chart.trend_equation"));
                    ui.checkbox(&mut t.show_r2, tr("chart.trend_r2"));
                });
                ui.end_row();
            }
        }
    });
    error_bars_ui(ui, ed, loc, sheet_name);
}

fn error_bars_ui(ui: &mut egui::Ui, ed: &mut Editor, loc: &apex_sheet::Locale, sheet_name: &str) {
    if ed.chart.kind == ChartType::Waterfall {
        return;
    }
    sync_fields(ed, loc);
    widgets::section(ui, tr("chart.error_bars"));
    let kinds: [(&str, Option<fn(&ErrorAmount) -> ErrorAmount>); 6] = [
        ("chart.err_none", None),
        ("chart.err_fixed", Some(|_| ErrorAmount::Fixed(1.0))),
        ("chart.err_percent", Some(|_| ErrorAmount::Percent(5.0))),
        ("chart.err_stddev", Some(|_| ErrorAmount::StdDev(1.0))),
        ("chart.err_stderr", Some(|_| ErrorAmount::StdErr)),
        ("chart.err_custom", Some(|_| ErrorAmount::Custom { plus: DataRef::default(), minus: DataRef::default() })),
    ];
    let key_of = |e: &Option<ErrorBars>| match e.as_ref().map(|e| &e.amount) {
        None => 0,
        Some(ErrorAmount::Fixed(_)) => 1,
        Some(ErrorAmount::Percent(_)) => 2,
        Some(ErrorAmount::StdDev(_)) => 3,
        Some(ErrorAmount::StdErr) => 4,
        Some(ErrorAmount::Custom { .. }) => 5,
    };
    let mut changed = [false; 2];
    let fields = &mut ed.fields;
    let s = &mut ed.chart.series[ed.series];
    widgets::form(ui, "chart_err", |ui| {
        widgets::form_row(ui, tr("chart.err_type"), |ui| {
            let cur = key_of(&s.error_bars);
            egui::ComboBox::from_id_salt("chart_err_kind").width(160.0).selected_text(tr(kinds[cur].0)).show_ui(ui, |ui| {
                for (k, (key, make)) in kinds.iter().enumerate() {
                    if ui.selectable_label(cur == k, tr(key)).clicked() && cur != k {
                        s.error_bars = make.map(|f| {
                            let old = s.error_bars.clone().unwrap_or_default();
                            ErrorBars { amount: f(&old.amount), ..old }
                        });
                    }
                }
            });
        });
        let Some(e) = &mut s.error_bars else { return };
        match &mut e.amount {
            ErrorAmount::Fixed(v) | ErrorAmount::StdDev(v) => {
                widgets::form_row(ui, tr("chart.err_amount"), |ui| ui.add(egui::DragValue::new(v).speed(0.1).range(0.0..=f64::MAX)));
            }
            ErrorAmount::Percent(v) => {
                widgets::form_row(ui, tr("chart.err_amount"), |ui| ui.add(egui::DragValue::new(v).speed(0.5).range(0.0..=f64::MAX).suffix(" %")));
            }
            ErrorAmount::StdErr => {}
            ErrorAmount::Custom { .. } => {
                for (k, key) in ["chart.err_plus_range", "chart.err_minus_range"].iter().enumerate() {
                    widgets::form_row(ui, tr(key), |ui| {
                        changed[k] = ui.add(egui::TextEdit::singleline(&mut fields[5 + k]).desired_width(250.0)).lost_focus();
                    });
                }
            }
        }
        widgets::form_row(ui, tr("chart.err_dir"), |ui| {
            for (d, key) in [(ErrorDir::Both, "chart.err_both"), (ErrorDir::Plus, "chart.err_plus"), (ErrorDir::Minus, "chart.err_minus")] {
                ui.radio_value(&mut e.dir, d, tr(key));
            }
        });
        ui.label("");
        ui.checkbox(&mut e.cap, tr("chart.err_cap"));
        ui.end_row();
    });
    for (k, ch) in changed.into_iter().enumerate() {
        if !ch {
            continue;
        }
        let text = ed.fields[5 + k].clone();
        let r = if text.trim().is_empty() { Ok(DataRef::default()) } else { parse_ref(&text, sheet_name, loc) };
        match (r, &mut ed.chart.series[ed.series].error_bars) {
            (Ok(r), Some(ErrorBars { amount: ErrorAmount::Custom { plus, minus }, .. })) => {
                *(if k == 0 { plus } else { minus }) = r;
                ed.error = None;
            }
            (Err(_), _) => ed.error = Some(tr("chart.bad_ref").into()),
            _ => {}
        }
    }
    if let Some(e) = &ed.error {
        ui.colored_label(Color32::from_rgb(220, 60, 60), e);
    }
}

fn legend_name(l: LegendPos) -> &'static str {
    tr(match l {
        LegendPos::None => "chart.legend_none",
        LegendPos::Right => "chart.legend_right",
        LegendPos::Left => "chart.legend_left",
        LegendPos::Top => "chart.legend_top",
        LegendPos::Bottom => "chart.legend_bottom",
    })
}

/// A value that is automatic (None) or fixed.
fn auto_value(ui: &mut egui::Ui, label: &str, v: &mut Option<f64>, default: f64) {
    widgets::form_row(ui, label, |ui| {
        let mut fixed = !v.is_some();
        if ui.checkbox(&mut fixed, tr("chart.auto")).changed() {
            *v = (!fixed).then_some(default);
        }
        if let Some(x) = v {
            let speed = (x.abs() * 0.01).max(0.01);
            ui.add(egui::DragValue::new(x).speed(speed));
        }
    });
}

fn axis_ui(ui: &mut egui::Ui, id: &str, a: &mut Axis, value: bool) {
    ui.checkbox(&mut a.visible, tr("chart.axis_visible"));
    ui.add_space(4.0);
    ui.add_enabled_ui(a.visible, |ui| {
        widgets::form(ui, id, |ui| {
            if value {
                auto_value(ui, tr("chart.min"), &mut a.min, 0.0);
                auto_value(ui, tr("chart.max"), &mut a.max, 100.0);
                auto_value(ui, tr("chart.major"), &mut a.major, 10.0);
                auto_value(ui, tr("chart.minor"), &mut a.minor, 2.0);
                widgets::form_row(ui, tr("chart.log"), |ui| {
                    let mut log = a.log_base.is_some();
                    if ui.checkbox(&mut log, tr("chart.log_base")).changed() {
                        a.log_base = log.then_some(10.0);
                    }
                    if let Some(b) = &mut a.log_base {
                        ui.add(egui::DragValue::new(b).range(2.0..=1000.0));
                    }
                });
            }
            widgets::form_row(ui, tr("chart.format"), |ui| ui.add(egui::TextEdit::singleline(&mut a.format).desired_width(200.0).hint_text(tr("chart.format_linked"))));
            ui.label("");
            ui.checkbox(&mut a.reverse, tr(if value { "chart.reverse_values" } else { "chart.reverse_cats" }));
            ui.end_row();
        });
    });
}

fn axes_ui(ui: &mut egui::Ui, c: &mut Chart) {
    if !c.kind.has_axes() {
        ui.add_space(8.0);
        ui.label(RichText::new(tr("chart.no_axes")).weak());
        return;
    }
    widgets::section(ui, tr("chart.value_axis"));
    axis_ui(ui, "chart_axis_y", &mut c.y_axis, true);
    widgets::section(ui, tr(if c.kind == ChartType::Scatter { "chart.x_axis" } else { "chart.category_axis" }));
    axis_ui(ui, "chart_axis_x", &mut c.x_axis, c.kind == ChartType::Scatter);
    if c.has_secondary() {
        widgets::section(ui, tr("chart.secondary_axis"));
        axis_ui(ui, "chart_axis_y2", &mut c.y2_axis, true);
    }
    c.sanitize();
}

fn style_ui(ui: &mut egui::Ui, ed: &mut Editor, data: &ChartData) {
    let c = &mut ed.chart;
    widgets::section(ui, tr("chart.styles"));
    // The gallery: the chart itself in each style.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for s in 0..apex_sheet::chart::STYLES {
            let mut sample = c.clone();
            sample.style = s;
            let on = c.style == s;
            if chart_tile(ui, &sample, data, vec2(112.0, 74.0), "", on).on_hover_text(format!("{} {}", tr("chart.style"), s + 1)).clicked() {
                c.style = s;
            }
        }
    });
    widgets::section(ui, tr("chart.palette"));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
        for p in 0..apex_sheet::chart::PALETTES {
            let (rect, resp) = ui.allocate_exact_size(vec2(108.0, 18.0), egui::Sense::click());
            for k in 0..6 {
                let w = rect.width() / 6.0;
                ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(w * k as f32, 0.0), vec2(w, rect.height())), 0.0, charts::palette_color(p, k));
            }
            if c.palette == p {
                ui.painter().rect_stroke(rect.expand(2.0), 2.0, Stroke::new(2.0, theme::SHEET_ACCENT), egui::StrokeKind::Outside);
            } else if resp.hovered() {
                ui.painter().rect_stroke(rect.expand(2.0), 2.0, Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color), egui::StrokeKind::Outside);
            }
            if resp.clicked() {
                c.palette = p;
                for s in &mut c.series {
                    s.color = None;
                }
            }
        }
    });
    if c.series.is_empty() {
        return;
    }
    widgets::section(ui, tr("chart.format_series"));
    ed.series = ed.series.min(c.series.len() - 1);
    let i = ed.series;
    let auto = charts::series_color(&Chart { series: Vec::new(), ..c.clone() }, i);
    let can_combo = matches!(c.kind, ChartType::Column | ChartType::Line | ChartType::Area | ChartType::Bar);
    let kind = c.kind;
    widgets::form(ui, "chart_series_fmt", |ui| {
        widgets::form_row(ui, tr("chart.series"), |ui| {
            egui::ComboBox::from_id_salt("chart_style_series").width(160.0).selected_text(data.series.get(ed.series).map_or(String::new(), |s| s.name.clone())).show_ui(ui, |ui| {
                for (i, s) in data.series.iter().enumerate() {
                    ui.selectable_value(&mut ed.series, i, &s.name);
                }
            });
        });
        let s = &mut c.series[i];
        widgets::form_row(ui, tr("chart.fill"), |ui| {
            let cur = s.color.map_or(auto, charts::rgb);
            ui.add_enabled_ui(!s.no_fill, |ui| {
                crate::widgets::menu_button(ui, RichText::new("      ").background_color(cur), |ui| {
                    if let Some(v) = widgets::color_menu(ui, &tr("chart.auto"), s.color) {
                        s.color = v;
                        ui.close();
                    }
                });
            });
            ui.checkbox(&mut s.no_fill, tr("chart.no_fill"));
        });
        widgets::form_row(ui, tr("chart.line"), |ui| {
            let cur = s.line_color.map_or(Color32::from_gray(128), charts::rgb);
            ui.add_enabled_ui(!s.no_line, |ui| {
                crate::widgets::menu_button(ui, RichText::new("      ").background_color(cur), |ui| {
                    if let Some(v) = widgets::color_menu(ui, &tr("chart.auto"), s.line_color) {
                        s.line_color = v;
                        ui.close();
                    }
                });
                let mut w = s.line_width.unwrap_or(0.0);
                if ui.add(egui::DragValue::new(&mut w).range(0.0..=20.0).speed(0.25).suffix(" pt")).changed() {
                    s.line_width = (w > 0.0).then_some(w);
                }
            });
            ui.checkbox(&mut s.no_line, tr("chart.no_line"));
        });
        if matches!(kind, ChartType::Line | ChartType::Scatter) || s.kind == Some(ChartType::Line) {
            widgets::form_row(ui, tr("chart.marker"), |ui| {
                egui::ComboBox::from_id_salt("chart_marker").width(160.0).selected_text(marker_name(s.marker)).show_ui(ui, |ui| {
                    for m in [Marker::Auto, Marker::None, Marker::Circle, Marker::Square, Marker::Diamond, Marker::Triangle] {
                        ui.selectable_value(&mut s.marker, m, marker_name(m));
                    }
                });
            });
            ui.label("");
            ui.checkbox(&mut s.smooth, tr("chart.smooth"));
            ui.end_row();
        }
        if can_combo {
            widgets::form_row(ui, tr("chart.series_type"), |ui| {
                let shown = s.kind.unwrap_or(kind);
                egui::ComboBox::from_id_salt("chart_series_kind").width(160.0).selected_text(tr(type_key(shown))).show_ui(ui, |ui| {
                    for k in [ChartType::Column, ChartType::Line, ChartType::Area] {
                        let k = if kind == ChartType::Bar && k == ChartType::Column { ChartType::Bar } else { k };
                        if ui.selectable_label(shown == k, tr(type_key(k))).clicked() {
                            s.kind = (k != kind).then_some(k);
                        }
                    }
                });
            });
            ui.label("");
            ui.checkbox(&mut s.secondary, tr("chart.on_secondary"));
            ui.end_row();
        }
    });
}

fn type_key(k: ChartType) -> &'static str {
    match k {
        ChartType::Column => "chart.type.column",
        ChartType::Bar => "chart.type.bar",
        ChartType::Line => "chart.type.line",
        ChartType::Area => "chart.type.area",
        ChartType::Pie => "chart.type.pie",
        ChartType::Doughnut => "chart.type.doughnut",
        ChartType::Scatter => "chart.type.scatter",
        ChartType::Waterfall => "chart.type.waterfall",
        ChartType::Histogram => "chart.type.histogram",
        ChartType::Radar => "chart.type.radar",
        ChartType::Bubble => "chart.type.bubble",
        ChartType::Stock => "chart.type.stock",
        ChartType::Surface => "chart.type.surface",
        ChartType::Treemap => "chart.type.treemap",
        ChartType::Sunburst => "chart.type.sunburst",
        ChartType::BoxWhisker => "chart.type.boxwhisker",
        ChartType::Funnel => "chart.type.funnel",
        ChartType::Pareto => "chart.type.pareto",
        ChartType::Mekko => "chart.type.mekko",
        ChartType::Timeline => "chart.type.timeline",
    }
}

fn marker_name(m: Marker) -> &'static str {
    tr(match m {
        Marker::Auto => "chart.auto",
        Marker::None => "chart.marker_none",
        Marker::Circle => "chart.marker_circle",
        Marker::Square => "chart.marker_square",
        Marker::Diamond => "chart.marker_diamond",
        Marker::Triangle => "chart.marker_triangle",
    })
}

/// The sparkline dialog; true when "Clear" was pressed.
fn spark_ui(ui: &mut egui::Ui, d: &mut SparkDlg) -> bool {
    let g = &mut d.group;
    let mut clear = false;
    egui::Grid::new("spark_where").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(tr("spark.data"));
        ui.add(egui::TextEdit::singleline(&mut d.data).desired_width(260.0));
        ui.end_row();
        ui.label(tr("spark.location"));
        ui.add(egui::TextEdit::singleline(&mut d.location).desired_width(260.0));
        ui.end_row();
        ui.label(tr("spark.type"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut g.kind, SparkKind::Line, tr("spark.line"));
            ui.radio_value(&mut g.kind, SparkKind::Column, tr("spark.column"));
            ui.radio_value(&mut g.kind, SparkKind::WinLoss, tr("spark.winloss"));
        });
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.label(RichText::new(tr("spark.show")).strong());
    let color_row = |ui: &mut egui::Ui, on: &mut bool, label: &str, color: &mut u32| {
        ui.horizontal(|ui| {
            ui.checkbox(on, label);
            crate::widgets::menu_button(ui, RichText::new("    ").background_color(charts::rgb(*color)), |ui| {
                if let Some(Some(v)) = widgets::color_menu(ui, &tr("chart.auto"), Some(*color)) {
                    *color = v;
                    ui.close();
                }
            });
        });
    };
    egui::Grid::new("spark_points").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        color_row(ui, &mut g.high, &tr("spark.high"), &mut g.high_color);
        color_row(ui, &mut g.low, &tr("spark.low"), &mut g.low_color);
        ui.end_row();
        color_row(ui, &mut g.first, &tr("spark.first"), &mut g.first_color);
        color_row(ui, &mut g.last, &tr("spark.last"), &mut g.last_color);
        ui.end_row();
        color_row(ui, &mut g.negative, &tr("spark.negative"), &mut g.negative_color);
        if g.kind == SparkKind::Line {
            color_row(ui, &mut g.markers, &tr("spark.markers"), &mut g.marker_color);
        }
        ui.end_row();
    });
    ui.add_space(6.0);
    egui::Grid::new("spark_look").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(tr("spark.color"));
        ui.horizontal(|ui| {
            crate::widgets::menu_button(ui, RichText::new("    ").background_color(charts::rgb(g.color)), |ui| {
                if let Some(Some(v)) = widgets::color_menu(ui, &tr("chart.auto"), Some(g.color)) {
                    g.color = v;
                    ui.close();
                }
            });
            if g.kind == SparkKind::Line {
                ui.add(egui::DragValue::new(&mut g.line_weight).range(0.25..=6.0).speed(0.25).suffix(" pt"));
            }
        });
        ui.end_row();
        for (label, a) in [(tr("spark.min"), &mut g.min), (tr("spark.max"), &mut g.max)] {
            ui.label(label);
            ui.horizontal(|ui| {
                let mut mode = match a {
                    SparkAxis::Individual => 0,
                    SparkAxis::Group => 1,
                    SparkAxis::Custom(_) => 2,
                };
                let before = mode;
                ui.radio_value(&mut mode, 0, tr("spark.axis_each"));
                ui.radio_value(&mut mode, 1, tr("spark.axis_group"));
                ui.radio_value(&mut mode, 2, tr("spark.axis_custom"));
                if mode != before {
                    *a = match mode {
                        1 => SparkAxis::Group,
                        2 => SparkAxis::Custom(0.0),
                        _ => SparkAxis::Individual,
                    };
                }
                if let SparkAxis::Custom(v) = a {
                    ui.add(egui::DragValue::new(v).speed(0.1));
                }
            });
            ui.end_row();
        }
        ui.label("");
        ui.checkbox(&mut g.show_axis, tr("spark.axis"));
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut g.right_to_left, tr("spark.rtl"));
        ui.end_row();
        ui.label(tr("chart.blanks"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut g.blanks, Blanks::Gap, tr("chart.blanks_gap"));
            ui.radio_value(&mut g.blanks, Blanks::Zero, tr("chart.blanks_zero"));
            if g.kind == SparkKind::Line {
                ui.radio_value(&mut g.blanks, Blanks::Span, tr("chart.blanks_span"));
            }
        });
        ui.end_row();
    });
    if let Some(e) = &d.error {
        ui.colored_label(Color32::from_rgb(200, 40, 40), e);
    }
    if d.editing.is_some() {
        ui.add_space(6.0);
        if ui.button(tr("spark.clear")).clicked() {
            clear = true;
        }
    }
    clear
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_and_resizing() {
        let loc = apex_sheet::Locale::EN;
        let r = parse_ref("=B2:B5", "My Data", &loc).unwrap();
        assert_eq!(r.to_canonical().unwrap(), "'My Data'!B2:B5");
        let r = parse_ref("Other!$A$1", "S", &loc).unwrap();
        assert_eq!(r.to_canonical().unwrap(), "Other!$A$1");
        assert!(parse_ref("", "S", &loc).unwrap().is_empty());
        assert!(parse_ref("=((", "S", &loc).is_err());
        let sh = Sheet::new("S");
        let c = Chart::new(ChartType::Column, CellRef::new(2, 2), 200.0, 100.0);
        let [x, y, _, _] = c.rect(&sh);
        let r = resized(&c, &sh, -1, -1, vec2(-20.0, -10.0), false);
        let [rx, ry, rw, rh] = r.rect(&sh);
        assert!((rw - 220.0).abs() < 1e-3 && (rh - 110.0).abs() < 1e-3);
        assert!((rx - (x - 20.0)).abs() < 1e-3 && (ry - (y - 10.0)).abs() < 1e-3);
        let r = resized(&c, &sh, 1, 0, vec2(-1000.0, 0.0), false);
        assert_eq!(r.width, MIN_SIZE);
        assert_eq!(bounding(&[CellRef::new(3, 1), CellRef::new(1, 4)]), Some(Range::new(CellRef::new(1, 1), CellRef::new(3, 4))));
    }
}

/// The Insert Chart menu's grid of chart types: the one picked (an index
/// of `CHART_TYPES`).
pub(crate) fn chart_type_menu(ui: &mut egui::Ui) -> Option<usize> {
    let mut pick = None;
    egui::Grid::new("chart_menu").num_columns(4).spacing([14.0, 10.0]).show(ui, |ui| {
        for (n, (family, range)) in CHART_MENU.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_min_width(180.0);
                ui.label(RichText::new(tr(family)).small().strong());
                for i in range.clone() {
                    let (key, icon, ..) = CHART_TYPES[i];
                    if crate::toolstrip::menu_item(ui, icon, tr(key)).clicked() {
                        pick = Some(i);
                        ui.close();
                    }
                }
            });
            if n % 4 == 3 {
                ui.end_row();
            }
        }
    });
    pick
}

/// Gives a new chart the variant chosen in the menu: its grouping and
/// flags (markers 1, lines 2, 3-D 16, filled 32), combo or not.
pub(crate) fn apply_type(c: &mut Chart, grouping: Grouping, flags: u8, combo: bool) {
    c.grouping = grouping;
    match c.kind {
        ChartType::Line => c.markers = flags & 1 != 0,
        ChartType::Scatter => c.lines = flags & 2 != 0,
        ChartType::Radar => {
            c.markers = flags & 1 != 0;
            c.filled = flags & 32 != 0;
        }
        _ => {}
    }
    if c.kind.has_3d() {
        c.view3d = flags & 16 != 0;
    }
    if combo {
        c.make_combo();
    }
}
