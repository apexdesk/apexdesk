//! Drawing by hand (the Draw tab): the tools and their settings, strokes
//! taken from the mouse, a pen or a finger (with the pressure when the
//! device gives one), painting ink, and what the eraser and the lasso
//! reach. The document and sheet views place the strokes (see
//! doc_view/ink_ui.rs and sheet_view/ink_ui.rs).

use std::borrow::Cow;
use std::sync::Arc;

use apex_doc::{Ink, InkStroke, InkTool, Shape};
use eframe::egui::{self, Color32, Event, Painter, Pos2, Rect, Stroke, pos2, vec2};

use crate::i18n::tr;
use crate::icons::Icon;
use crate::toolstrip::{self, Body};

/// Widths offered for each tool, in points.
const PEN_WIDTHS: [f32; 6] = [0.5, 1.0, 1.5, 2.25, 3.5, 5.0];
const MARKER_WIDTHS: [f32; 5] = [6.0, 8.0, 10.0, 14.0, 20.0];
/// Colors offered next to the color menu.
const PEN_COLORS: [u32; 6] = [0x000000, 0xE71224, 0x0070C0, 0x00A550, 0x7030A0, 0xFF6600];
const MARKER_COLORS: [u32; 5] = [0xFFFF00, 0x00FF00, 0x00B0F0, 0xFF66CC, 0xFFC000];

/// What the pointer does on the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Draw(InkTool),
    /// Erases whole strokes.
    Eraser,
    /// Picks the strokes drawn around.
    Lasso,
}

/// Color (0xRRGGBB) and width (points) of a tool.
#[derive(Clone, Copy, Debug)]
pub struct Pen {
    pub color: u32,
    pub width: f32,
}

/// The stroke being drawn, in the view's units.
struct Live {
    points: Vec<Pos2>,
    pressure: Vec<f32>,
    /// The device gave a pressure.
    force: bool,
}

pub struct InkState {
    /// None: the pointer selects and types as usual.
    pub mode: Option<Mode>,
    /// Settings of the pen, the pencil and the highlighter.
    pub pens: [Pen; 3],
    live: Option<Live>,
    lasso: Vec<Pos2>,
    /// The eraser is down.
    erasing: bool,
    /// Strokes the eraser went over in this drag: ink (by address) and
    /// stroke index; they go when it is lifted.
    erased: Vec<(usize, usize)>,
}

impl Default for InkState {
    fn default() -> Self {
        let pen = |t: InkTool| Pen { color: t.default_color(), width: t.default_width() };
        InkState { mode: None, pens: InkTool::ALL.map(pen), live: None, lasso: Vec::new(), erasing: false, erased: Vec::new() }
    }
}

fn tool_index(t: InkTool) -> usize {
    InkTool::ALL.iter().position(|x| *x == t).unwrap_or(0)
}

/// The key of an ink object for the eraser: where its strokes live.
pub fn ink_key(ink: &Arc<Ink>) -> usize {
    Arc::as_ptr(ink) as usize
}

impl InkState {
    pub fn active(&self) -> bool {
        self.mode.is_some()
    }

    /// The pointer is drawing, erasing or drawing a lasso.
    pub fn busy(&self) -> bool {
        self.live.is_some() || !self.lasso.is_empty() || self.erasing
    }

    pub fn pen(&self, t: InkTool) -> Pen {
        self.pens[tool_index(t)]
    }

    pub fn set_mode(&mut self, m: Option<Mode>) {
        self.mode = m;
        self.cancel();
    }

    /// Drops what the pointer was doing (Escape, another tool).
    pub fn cancel(&mut self) {
        self.live = None;
        self.lasso.clear();
        self.erasing = false;
        self.erased.clear();
    }

    // --- strokes -------------------------------------------------------------------------

    pub fn begin(&mut self, p: Pos2, force: Option<f32>) {
        self.live = Some(Live { points: vec![p], pressure: vec![force.unwrap_or(1.0)], force: force.is_some() });
    }

    pub fn drawing(&self) -> bool {
        self.live.is_some()
    }

    pub fn extend(&mut self, pts: &[Pos2], force: Option<f32>) {
        let Some(l) = &mut self.live else { return };
        for &p in pts {
            if !p.is_finite() || l.points.len() >= apex_doc::ink::MAX_POINTS * 4 {
                continue;
            }
            if l.points.last().is_some_and(|q| (*q - p).length_sq() < 0.01) {
                continue;
            }
            l.points.push(p);
            l.pressure.push(force.unwrap_or(1.0));
            l.force |= force.is_some();
        }
    }

    /// The finished stroke with points in the view's units, smoothed;
    /// `unit`: view units per point.
    pub fn finish(&mut self, unit: f32) -> Option<InkStroke> {
        let l = self.live.take()?;
        let Mode::Draw(tool) = self.mode? else { return None };
        let pen = self.pen(tool);
        let pts: Vec<(f32, f32)> = l.points.iter().map(|p| (p.x, p.y)).collect();
        let pressure = if l.force { l.pressure } else { Vec::new() };
        let (points, pressure) = apex_doc::ink::smooth(&pts, &pressure, 0.3 * unit.max(0.01));
        if points.is_empty() {
            return None;
        }
        Some(InkStroke { tool, color: pen.color, width: pen.width, points, pressure })
    }

    /// Paints the stroke being drawn; `to_screen` places view points.
    pub fn paint_live(&self, p: &Painter, to_screen: impl Fn(Pos2) -> Pos2, scale: f32) {
        if let (Some(l), Some(Mode::Draw(tool))) = (&self.live, self.mode) {
            let pen = self.pen(tool);
            let mut s = InkStroke { tool, color: pen.color, width: pen.width, points: l.points.iter().map(|q| (q.x, q.y)).collect(), pressure: Vec::new() };
            if l.force {
                s.pressure = l.pressure.clone();
            }
            let pts: Vec<Pos2> = l.points.iter().map(|q| to_screen(*q)).collect();
            paint_stroke(p, &s, &pts, scale);
        }
        if self.lasso.len() > 1 {
            let pts: Vec<Pos2> = self.lasso.iter().map(|q| to_screen(*q)).collect();
            let mut out = Vec::new();
            dashed_loop(&pts, &mut out);
            p.extend(out);
        }
    }

    // --- lasso ---------------------------------------------------------------------------

    pub fn lasso_push(&mut self, p: Pos2) {
        if p.is_finite() && self.lasso.last().is_none_or(|q| (*q - p).length_sq() > 1.0) && self.lasso.len() < 100_000 {
            self.lasso.push(p);
        }
    }

    pub fn lassoing(&self) -> bool {
        !self.lasso.is_empty()
    }

    pub fn take_lasso(&mut self) -> Vec<Pos2> {
        std::mem::take(&mut self.lasso)
    }

    // --- eraser --------------------------------------------------------------------------

    pub fn erase_begin(&mut self) {
        self.erasing = true;
        self.erased.clear();
    }

    pub fn erasing(&self) -> bool {
        self.erasing
    }

    /// Marks strokes of `ink` as erased.
    pub fn erase(&mut self, ink: &Arc<Ink>, strokes: &[usize]) {
        let k = ink_key(ink);
        for &i in strokes {
            if !self.erased.contains(&(k, i)) {
                self.erased.push((k, i));
            }
        }
    }

    /// Ends the eraser's drag: the strokes it went over, by ink.
    pub fn take_erased(&mut self) -> Vec<(usize, Vec<usize>)> {
        self.erasing = false;
        let mut out: Vec<(usize, Vec<usize>)> = Vec::new();
        for (k, i) in std::mem::take(&mut self.erased) {
            match out.iter_mut().find(|(x, _)| *x == k) {
                Some((_, v)) => v.push(i),
                None => out.push((k, vec![i])),
            }
        }
        out
    }

    /// The shape as shown while the eraser is down: without the strokes
    /// it went over.
    pub fn shown<'a>(&self, s: &'a Shape) -> Cow<'a, Shape> {
        let Some(ink) = s.ink.as_ref().filter(|_| !self.erased.is_empty()) else { return Cow::Borrowed(s) };
        let k = ink_key(ink);
        if !self.erased.iter().any(|(x, _)| *x == k) {
            return Cow::Borrowed(s);
        }
        let mut out = s.clone();
        out.ink = Some(Arc::new(without(ink, self.erased.iter().filter(|(x, _)| *x == k).map(|(_, i)| *i).collect())));
        Cow::Owned(out)
    }
}

/// `ink` without the strokes at `gone`.
pub fn without(ink: &Ink, gone: Vec<usize>) -> Ink {
    Ink::new(ink.strokes.iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, s)| s.clone()).collect())
}

/// Pointer positions (screen) of this frame, oldest first, and the pen or
/// touch pressure when the device gave one.
pub fn trail(ctx: &egui::Context) -> (Vec<Pos2>, Option<f32>) {
    ctx.input(|i| {
        let mut pts = Vec::new();
        let mut force = None;
        for e in &i.events {
            match e {
                Event::PointerMoved(p) => pts.push(*p),
                Event::Touch { pos, force: Some(f), .. } if f.is_finite() && *f > 0.0 => {
                    force = Some(f.clamp(0.0, 1.0));
                    pts.push(*pos);
                }
                _ => {}
            }
        }
        if pts.is_empty() {
            pts.extend(i.pointer.interact_pos());
        }
        (pts, force)
    })
}

// --- geometry ---------------------------------------------------------------------------

/// The strokes of an ink shape drawn in `rect` (before turning), in the
/// same space as `rect`, flipped and turned as the shape says.
pub fn placed(shape: &Shape, ink: &Ink, rect: Rect) -> Vec<Vec<Pos2>> {
    let c = rect.center();
    let (sn, cs) = shape.rotation.to_radians().sin_cos();
    let at = |(x, y): (f32, f32)| {
        let x = if shape.flip_h { 1.0 - x } else { x };
        let y = if shape.flip_v { 1.0 - y } else { y };
        let d = pos2(rect.min.x + x * rect.width(), rect.min.y + y * rect.height()) - c;
        if shape.rotation == 0.0 { c + d } else { c + vec2(d.x * cs - d.y * sn, d.x * sn + d.y * cs) }
    };
    ink.strokes.iter().map(|s| s.points.iter().map(|&q| at(q)).collect()).collect()
}

fn seg_dist(q: Pos2, a: Pos2, b: Pos2) -> f32 {
    apex_doc::ink::seg_dist((q.x, q.y), (a.x, a.y), (b.x, b.y))
}

/// Strokes of the shape's ink within `tol` of `q` (plus half their width
/// at `scale` units per point).
pub fn strokes_near(shape: &Shape, rect: Rect, q: Pos2, tol: f32, scale: f32) -> Vec<usize> {
    let Some(ink) = &shape.ink else { return Vec::new() };
    if !rect.expand(tol + 200.0 * scale).contains(q) && shape.rotation == 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, (pts, s)) in placed(shape, ink, rect).iter().zip(&ink.strokes).enumerate() {
        let reach = tol + s.width * scale / 2.0;
        let d = match pts.len() {
            0 => f32::INFINITY,
            1 => (pts[0] - q).length(),
            _ => pts.windows(2).map(|w| seg_dist(q, w[0], w[1])).fold(f32::INFINITY, f32::min),
        };
        if d <= reach {
            out.push(i);
        }
    }
    out
}

/// Strokes of the shape's ink the lasso goes around (most of their points
/// inside it).
pub fn strokes_in(shape: &Shape, rect: Rect, lasso: &[Pos2]) -> Vec<usize> {
    let Some(ink) = &shape.ink else { return Vec::new() };
    if lasso.len() < 3 {
        return Vec::new();
    }
    let poly: Vec<(f32, f32)> = lasso.iter().map(|p| (p.x, p.y)).collect();
    placed(shape, ink, rect)
        .iter()
        .enumerate()
        .filter(|(_, pts)| !pts.is_empty() && pts.iter().filter(|p| apex_doc::ink::inside(&poly, (p.x, p.y))).count() * 10 >= pts.len() * 7)
        .map(|(i, _)| i)
        .collect()
}

/// The strokes of an ink shape in box `rect` with absolute points (in the
/// space of `rect`), turned and flipped as it is shown.
pub fn absolute(shape: &Shape, ink: &Ink, rect: Rect, which: &[usize]) -> Vec<InkStroke> {
    placed(shape, ink, rect)
        .into_iter()
        .zip(&ink.strokes)
        .enumerate()
        .filter(|(i, _)| which.contains(i))
        .map(|(_, (pts, s))| InkStroke { points: pts.iter().map(|p| (p.x, p.y)).collect(), ..s.clone() })
        .collect()
}

// --- painting ---------------------------------------------------------------------------

fn color(s: &InkStroke) -> Color32 {
    let c = s.color;
    Color32::from_rgba_unmultiplied((c >> 16) as u8, (c >> 8) as u8, c as u8, (s.tool.alpha() * 255.0).round() as u8)
}

/// Paints one stroke through screen points `pts`; `scale`: screen pixels
/// per point.
pub fn paint_stroke(p: &Painter, s: &InkStroke, pts: &[Pos2], scale: f32) {
    let col = color(s);
    let w = (s.width * scale).max(0.6);
    match pts.len() {
        0 => {}
        1 => {
            if s.tool == InkTool::Highlighter {
                p.rect_filled(Rect::from_center_size(pts[0], vec2(w * 0.4, w)), 0.0, col);
            } else {
                p.circle_filled(pts[0], w / 2.0, col);
            }
        }
        _ if s.has_pressure() && s.tool == InkTool::Pen => {
            // Width changing along the stroke: round joints hide the seams.
            let mut out = Vec::with_capacity(pts.len() * 2);
            for (i, seg) in pts.windows(2).enumerate() {
                let wi = ((s.width_at(i) + s.width_at(i + 1)) / 2.0 * scale).max(0.6);
                out.push(egui::Shape::line_segment([seg[0], seg[1]], Stroke::new(wi, col)));
                out.push(egui::Shape::circle_filled(seg[1], wi / 2.0, col));
            }
            out.push(egui::Shape::circle_filled(pts[0], (s.width_at(0) * scale).max(0.6) / 2.0, col));
            p.extend(out);
        }
        _ => {
            let w = if s.has_pressure() { (s.pressure.iter().sum::<f32>() / s.pressure.len() as f32 * 0.65 + 0.35) * w } else { w };
            p.add(egui::Shape::line(pts.to_vec(), Stroke::new(w, col)));
            if s.tool == InkTool::Pen {
                // Round ends.
                p.circle_filled(pts[0], w / 2.0, col);
                p.circle_filled(pts[pts.len() - 1], w / 2.0, col);
            }
        }
    }
}

/// Paints the ink of a shape drawn in `rect` (screen, before turning).
pub fn paint(p: &Painter, rect: Rect, shape: &Shape, ink: &Ink, scale: f32) {
    // Highlighters first, so that what is written stays on top.
    let lines = placed(shape, ink, rect);
    for marker in [true, false] {
        for (pts, s) in lines.iter().zip(&ink.strokes) {
            if (s.tool == InkTool::Highlighter) == marker {
                paint_stroke(p, s, pts, scale);
            }
        }
    }
}

fn dashed_loop(pts: &[Pos2], out: &mut Vec<egui::Shape>) {
    let dark = Stroke::new(1.0, Color32::from_gray(40));
    let light = Stroke::new(1.0, Color32::WHITE);
    let mut run = 0.0f32;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let len = (b - a).length();
        out.push(egui::Shape::line_segment([a, b], if (run / 4.0) as i64 % 2 == 0 { dark } else { light }));
        run += len;
    }
}

// --- the Draw tab -----------------------------------------------------------------------

/// What the Draw tab asked for besides changing the tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabAction {
    /// Selects every ink object (to move or delete them).
    SelectAll,
}

/// The groups of the Draw tab, shared by both views.
/// `select_all`: offer to select every ink object (views that pick
/// several shapes at once), with whether there is any.
pub fn tab(b: &mut Body, st: &mut InkState, select_all: Option<bool>) -> Option<TabAction> {
    let a = b.accent();
    let mut action = None;
    b.group("ink.g_tools", Icon::Pen, |ui| {
        if toolstrip::large(ui, Icon::Select, tr("ink.select"), tr("ink.select_tip"), a, st.mode.is_none()).clicked() {
            st.set_mode(None);
        }
        // Each tool, pressed again, lets go (back to selecting).
        let on = st.mode == Some(Mode::Lasso);
        if toolstrip::large(ui, Icon::Lasso, tr("ink.lasso"), tr("ink.lasso_tip"), a, on).clicked() {
            st.set_mode(if on { None } else { Some(Mode::Lasso) });
        }
        for (t, icon) in [(InkTool::Pen, Icon::Pen), (InkTool::Pencil, Icon::Pencil), (InkTool::Highlighter, Icon::Highlight)] {
            let on = st.mode == Some(Mode::Draw(t));
            if toolstrip::large(ui, icon, tr(t.key()), "", a, on).clicked() {
                st.set_mode(if on { None } else { Some(Mode::Draw(t)) });
            }
        }
        let on = st.mode == Some(Mode::Eraser);
        if toolstrip::large(ui, Icon::Eraser, tr("ink.eraser"), tr("ink.eraser_tip"), a, on).clicked() {
            st.set_mode(if on { None } else { Some(Mode::Eraser) });
        }
    });
    // The pen whose settings show: the one in use, else the pen.
    let tool = match st.mode {
        Some(Mode::Draw(t)) => t,
        _ => InkTool::Pen,
    };
    let i = tool_index(tool);
    b.group("ink.g_pen", Icon::Fill, |ui| {
        let pen = st.pens[i];
        toolstrip::column(ui, |ui| {
            ui.horizontal(|ui| {
                let colors: &[u32] = if tool == InkTool::Highlighter { &MARKER_COLORS } else { &PEN_COLORS };
                for &c in colors {
                    if color_swatch(ui, c, c == pen.color).clicked() {
                        st.pens[i].color = c;
                        st.mode = Some(Mode::Draw(tool));
                    }
                }
            });
            toolstrip::small_menu(ui, Some(Icon::Fill), tr("ink.color"), "", a, |ui| {
                if let Some(c) = crate::widgets::color_menu(ui, tr("ink.default_color"), Some(pen.color)) {
                    st.pens[i].color = c.unwrap_or(tool.default_color());
                    st.mode = Some(Mode::Draw(tool));
                    ui.close();
                }
            });
            toolstrip::small_menu(ui, Some(Icon::LineSpacing), &format!("{} ({})", tr("ink.width"), fmt_pt(pen.width)), "", a, |ui| {
                let widths: &[f32] = if tool == InkTool::Highlighter { &MARKER_WIDTHS } else { &PEN_WIDTHS };
                for &w in widths {
                    if ui.selectable_label((pen.width - w).abs() < 0.01, fmt_pt(w)).clicked() {
                        st.pens[i].width = w;
                        st.mode = Some(Mode::Draw(tool));
                        ui.close();
                    }
                }
            });
        });
    });
    if let Some(any) = select_all {
        b.group("ink.g_edit", Icon::Select, |ui| {
            if toolstrip::enabled(ui, any, tr("ink.no_ink"), |ui| toolstrip::large(ui, Icon::Select, tr("ink.select_all"), "", a, false)).clicked() {
                st.set_mode(None);
                action = Some(TabAction::SelectAll);
            }
        });
    }
    action
}

fn fmt_pt(w: f32) -> String {
    let v = format!("{w:.2}");
    format!("{} pt", v.trim_end_matches('0').trim_end_matches('.'))
}

fn color_swatch(ui: &mut egui::Ui, c: u32, current: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::ColorButton, &format!("#{c:06X}"), Some(current));
    ui.painter().circle_filled(rect.center(), 7.0, crate::theme::rgb(c));
    let ring = if current || resp.hovered() { Stroke::new(2.0, crate::theme::ACCENT) } else { Stroke::new(1.0, Color32::from_gray(150)) };
    ui.painter().circle_stroke(rect.center(), 8.0, ring);
    resp.on_hover_text(format!("#{c:06X}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(points: Vec<(f32, f32)>) -> Shape {
        Shape::ink(Ink::new(vec![InkStroke { points, ..InkStroke::new(InkTool::Pen, 0, 2.0) }]))
    }

    #[test]
    fn strokes_are_placed_turned_and_found() {
        let s = shape(vec![(0.0, 0.5), (1.0, 0.5)]);
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0));
        let pts = placed(&s, s.ink.as_ref().unwrap(), rect);
        assert_eq!(pts[0], vec![pos2(100.0, 150.0), pos2(300.0, 150.0)]);
        assert_eq!(strokes_near(&s, rect, pos2(200.0, 153.0), 2.0, 1.0), vec![0]);
        assert!(strokes_near(&s, rect, pos2(200.0, 160.0), 2.0, 1.0).is_empty());
        // Turned a quarter: the line stands up.
        let mut t = s.clone();
        t.rotation = 90.0;
        let pts = placed(&t, t.ink.as_ref().unwrap(), rect);
        assert!((pts[0][0].x - 200.0).abs() < 0.01 && (pts[0][0].y - 50.0).abs() < 0.01);
        let lasso = [pos2(150.0, 0.0), pos2(250.0, 0.0), pos2(250.0, 300.0), pos2(150.0, 300.0)];
        assert_eq!(strokes_in(&t, rect, &lasso), vec![0]);
        assert!(strokes_in(&s, rect, &lasso).is_empty());
    }

    #[test]
    fn a_stroke_is_smoothed_and_keeps_its_pen() {
        let mut st = InkState::default();
        st.set_mode(Some(Mode::Draw(InkTool::Highlighter)));
        st.pens[2].color = 0x00FF00;
        st.begin(pos2(0.0, 0.0), None);
        let pts: Vec<Pos2> = (1..50).map(|i| pos2(i as f32, if i % 2 == 0 { 0.2 } else { -0.2 })).collect();
        st.extend(&pts, None);
        let s = st.finish(1.0).unwrap();
        assert!(!st.drawing());
        assert_eq!((s.tool, s.color, s.width), (InkTool::Highlighter, 0x00FF00, 10.0));
        assert!(s.points.len() < 10 && s.pressure.is_empty());
        // With a pressure from the device, it stays with the points.
        st.set_mode(Some(Mode::Draw(InkTool::Pen)));
        st.begin(pos2(0.0, 0.0), Some(0.2));
        st.extend(&[pos2(10.0, 10.0), pos2(20.0, 0.0)], Some(0.9));
        let s = st.finish(1.0).unwrap();
        assert!(s.has_pressure() && s.pressure[0] < 0.5);
    }

    #[test]
    fn erased_strokes_hide_until_the_eraser_lifts() {
        let mut st = InkState::default();
        let s = Shape::ink(Ink::new(vec![InkStroke { points: vec![(0.0, 0.0), (1.0, 1.0)], ..Default::default() }, InkStroke { points: vec![(0.0, 1.0), (1.0, 0.0)], ..Default::default() }]));
        let ink = s.ink.clone().unwrap();
        st.erase_begin();
        st.erase(&ink, &[1]);
        st.erase(&ink, &[1]);
        assert_eq!(st.shown(&s).ink.as_ref().unwrap().strokes.len(), 1);
        assert_eq!(st.take_erased(), vec![(ink_key(&ink), vec![1])]);
        assert!(!st.erasing() && matches!(st.shown(&s), Cow::Borrowed(_)));
    }
}
