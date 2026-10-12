//! The Gantt chart beside a plan table: a bar per task on the row of the
//! table, summaries, milestones, links, the critical path in red, the
//! baseline, today's line and the resource usage. Its rows scroll with the
//! grid. Bars are dragged to move a task, their end to stretch it, and
//! up or down onto another task to link them.
//!
//! The painting works on plain shapes so that the export (`gantt_export`)
//! draws the same chart.

use std::sync::Arc;

use apex_sheet::format::{date_to_serial, serial_to_date};
use apex_sheet::plan::calendar::weekday;
use apex_sheet::plan::{Calendar, DepType, Field, ResKind, level};
use apex_sheet::workbook::{PlanView, TableId};
use eframe::egui::{Galley, Shape};

use super::*;

/// The time unit of the Gantt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scale {
    Day,
    Week,
    Month,
    Quarter,
}

impl Scale {
    pub const ALL: [Scale; 4] = [Scale::Day, Scale::Week, Scale::Month, Scale::Quarter];

    pub fn key(self) -> &'static str {
        match self {
            Scale::Day => "plan.zoom_day",
            Scale::Week => "plan.zoom_week",
            Scale::Month => "plan.zoom_month",
            Scale::Quarter => "plan.zoom_quarter",
        }
    }

    /// Pixels per day at 100 %.
    pub fn px(self) -> f32 {
        match self {
            Scale::Day => 28.0,
            Scale::Week => 11.0,
            Scale::Month => 3.4,
            Scale::Quarter => 1.2,
        }
    }

    pub fn days_per_unit(self) -> f64 {
        match self {
            Scale::Day => 1.0,
            Scale::Week => 7.0,
            Scale::Month => 30.4,
            Scale::Quarter => 91.3,
        }
    }

    /// The next finer (`d` < 0) or coarser scale.
    pub fn step(self, d: i32) -> Scale {
        let i = Scale::ALL.iter().position(|s| *s == self).unwrap_or(1) as i32;
        Scale::ALL[(i + d).clamp(0, 3) as usize]
    }
}

/// Where the grid put its rows on the last frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RowsGeo {
    pub sheet: usize,
    /// Screen top of the first row (under the column headers).
    pub origin_y: f32,
    pub z: f32,
    pub scroll_y: f32,
    pub fr: u32,
    /// Height of the frozen rows (sheet units).
    pub fy: f32,
    /// Offset of the first frozen row shown (sheet units).
    pub oy: f32,
    pub bottom: f32,
}

impl RowsGeo {
    /// The screen span of row `r` (None when hidden or scrolled under the
    /// frozen rows).
    pub fn span(&self, sh: &Sheet, r: u32) -> Option<(f32, f32)> {
        let h = sh.row_height(r) * self.z;
        if h <= 0.0 {
            return None;
        }
        if r < self.fr && sh.row_y(r) < self.oy {
            return None;
        }
        let y = if r < self.fr { self.origin_y + (sh.row_y(r) - self.oy) * self.z } else { self.origin_y + (sh.row_y(r) - self.scroll_y) * self.z };
        if r >= self.fr && y + h <= self.origin_y + self.fy * self.z {
            return None;
        }
        Some((y, y + h))
    }
}

/// Days to screen x.
#[derive(Clone, Copy, Debug)]
pub(super) struct Axis {
    pub x0: f32,
    /// The day at `x0`.
    pub left: f64,
    /// Pixels per day.
    pub px: f32,
}

impl Axis {
    pub fn x(&self, day: f64) -> f32 {
        self.x0 + ((day - self.left) as f32) * self.px
    }

    pub fn day(&self, x: f32) -> f64 {
        self.left + ((x - self.x0) / self.px) as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DragKind {
    /// Pressed on a bar, not yet moved enough to tell.
    Pending,
    Move,
    Stretch,
    Link,
    /// The time scale: scrolls the dates.
    Pan,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct GanttDrag {
    pub kind: DragKind,
    pub task: usize,
    pub from: Pos2,
    pub to: Pos2,
    pub left0: f64,
}

/// A task row of the chart: task index and vertical span.
#[derive(Clone, Copy, Debug)]
pub(super) struct Lane {
    pub task: usize,
    pub y0: f32,
    pub y1: f32,
}

/// Colors of the chart.
#[derive(Clone, Copy)]
pub(super) struct Look {
    pub bg: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub grid: Color32,
    pub header: Color32,
    /// Text on the header strip, which follows the window's theme.
    pub head_text: Color32,
    pub nonwork: Color32,
}

impl Look {
    pub fn paper(header: Color32) -> Look {
        Look { bg: theme::Paper::BG, text: theme::Paper::TEXT, weak: Color32::from_gray(110), grid: theme::Paper::GRID, header, head_text: if header.intensity() < 0.5 { Color32::from_gray(225) } else { theme::Paper::TEXT }, nonwork: Color32::from_rgb(0xF1, 0xF2, 0xF5) }
    }
}

pub(super) const BAR: Color32 = Color32::from_rgb(0x44, 0x72, 0xC4);
pub(super) const BAR_DONE: Color32 = Color32::from_rgb(0x1F, 0x3E, 0x7A);
pub(super) const MANUAL: Color32 = Color32::from_rgb(0x2A, 0x9D, 0x8F);
pub(super) const CRITICAL: Color32 = Color32::from_rgb(0xD9, 0x3B, 0x3B);
const CRITICAL_DONE: Color32 = Color32::from_rgb(0x8E, 0x1B, 0x1B);
pub(super) const SUMMARY: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
const BASELINE: Color32 = Color32::from_rgb(0xA8, 0xAD, 0xB5);
pub(super) const LINK: Color32 = Color32::from_rgb(0x5A, 0x6A, 0x85);
pub(super) const TODAY: Color32 = Color32::from_rgb(0xE8, 0x8A, 0x1A);
pub(super) const OVER: Color32 = Color32::from_rgb(0xD9, 0x3B, 0x3B);
const LOAD: Color32 = Color32::from_rgb(0x6C, 0x8E, 0xD1);

/// A laid out text.
pub(super) fn galley(ctx: &egui::Context, text: String, size: f32, color: Color32) -> Arc<Galley> {
    ctx.fonts_mut(|f| f.layout_no_wrap(text, FontId::proportional(size), color))
}

pub(super) fn month_names() -> Vec<String> {
    tr("field.months").split('|').map(|m| m.to_string()).collect()
}

/// A short month name ("Sep"): three letters of alphabetic names.
pub(super) fn short_month(names: &[String], m: u32) -> String {
    let n = names.get(m as usize - 1).cloned().unwrap_or_else(|| m.to_string());
    if n.chars().all(|c| c.is_alphabetic()) && n.chars().count() > 4 { n.chars().take(3).collect() } else { n }
}

/// Unit boundaries from `a` to `b` (serial days) of `scale`, and of the
/// coarser tier above it, with their labels.
fn units(scale: Scale, a: f64, b: f64, upper: bool) -> Vec<(f64, String)> {
    let months = month_names();
    let (ya, ma, _) = serial_to_date(a);
    let mut out = Vec::new();
    let tier = match (scale, upper) {
        (Scale::Day, false) => 0,
        (Scale::Week, false) => 1,
        (Scale::Day | Scale::Week, true) | (Scale::Month, false) => 2,
        (Scale::Quarter, false) => 3,
        (Scale::Month | Scale::Quarter, true) => 4,
    };
    match tier {
        0 | 1 => {
            let mut d = a.floor() - 1.0;
            if tier == 1 {
                d -= weekday(d as i64) as f64;
            }
            let step = if tier == 0 { 1.0 } else { 7.0 };
            while d <= b && out.len() < 5000 {
                out.push((d, serial_to_date(d).2.to_string()));
                d += step;
            }
        }
        2 | 3 => {
            let (mut y, mut m) = (ya, ma as i64);
            if tier == 3 {
                m -= (m - 1) % 3;
            }
            loop {
                let d = date_to_serial(y, m, 1);
                if d > b || out.len() > 5000 {
                    break;
                }
                let label = if tier == 2 {
                    if upper { format!("{} {y}", months.get(m as usize - 1).cloned().unwrap_or_default()) } else { short_month(&months, m as u32) }
                } else {
                    trf("plan.quarter_short", &[("n", &((m - 1) / 3 + 1))])
                };
                out.push((d, label));
                m += if tier == 2 { 1 } else { 3 };
                if m > 12 {
                    m -= 12;
                    y += 1;
                }
            }
        }
        _ => {
            let mut y = ya;
            loop {
                let d = date_to_serial(y, 1, 1);
                if d > b || out.len() > 500 {
                    break;
                }
                out.push((d, y.to_string()));
                y += 1;
            }
        }
    }
    out
}

/// The time scale in `rect`: the coarser tier if `upper`, else the finer.
pub(super) fn paint_scale(ctx: &egui::Context, shapes: &mut Vec<Shape>, ax: &Axis, rect: Rect, scale: Scale, upper: bool, look: &Look, z: f32) {
    shapes.push(Shape::rect_filled(rect, 0.0, look.header));
    let (a, b) = (ax.day(rect.left()), ax.day(rect.right()));
    let list = units(scale, a, b, upper);
    for (k, (d, label)) in list.iter().enumerate() {
        let x = ax.x(*d);
        let next = list.get(k + 1).map_or(rect.right() + 200.0, |n| ax.x(n.0));
        shapes.push(Shape::line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, look.grid)));
        let g = galley(ctx, label.clone(), 11.0 * z, look.head_text);
        if g.size().x + 4.0 > next - x {
            continue;
        }
        // Centered in its unit, or kept in view when the unit is wider.
        let mut tx = (x + next) / 2.0 - g.size().x / 2.0;
        if !upper || next - x > rect.width() {
            tx = tx.max(rect.left() + 3.0).min(next - g.size().x - 3.0);
        }
        let ty = rect.center().y - g.size().y / 2.0;
        shapes.push(Shape::galley(pos2(tx, ty), g, look.head_text));
    }
    shapes.push(Shape::line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, look.grid)));
}

/// The whole bar of task `i`: its left and right x.
fn bar_x(v: &PlanView, ax: &Axis, i: usize) -> (f32, f32) {
    let o = &v.schedule.tasks[i];
    let (a, b) = (ax.x(o.start), ax.x(o.finish));
    (a, b.max(a + 2.0))
}

fn is_milestone(v: &PlanView, i: usize) -> bool {
    let o = &v.schedule.tasks[i];
    !o.summary && o.duration <= 0.0
}

/// The bar rectangle of task `i` in `lane` (the diamond's box for a
/// milestone).
pub(super) fn bar_rect(v: &PlanView, ax: &Axis, lane: &Lane) -> Rect {
    let h = lane.y1 - lane.y0;
    let (a, b) = bar_x(v, ax, lane.task);
    if is_milestone(v, lane.task) {
        let r = h * 0.32;
        let c = pos2(a, (lane.y0 + lane.y1) / 2.0);
        return Rect::from_center_size(c, vec2(2.0 * r, 2.0 * r));
    }
    let baseline = v.baseline.get(lane.task).is_some_and(|b| b.is_some());
    let (top, bottom) = if v.schedule.tasks[lane.task].summary { (0.3, 0.55) } else if baseline { (0.18, 0.62) } else { (0.24, 0.76) };
    Rect::from_min_max(pos2(a, lane.y0 + h * top), pos2(b, lane.y0 + h * bottom))
}

/// Everything under the scale: shading, bars, links and labels.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_body(ctx: &egui::Context, shapes: &mut Vec<Shape>, v: &PlanView, ax: &Axis, lanes: &[Lane], rect: Rect, scale: Scale, look: &Look, z: f32, today: f64) {
    shapes.push(Shape::rect_filled(rect, 0.0, look.bg));
    let cal: Calendar = v.plan.calendar();
    let (a, b) = (ax.day(rect.left()).floor(), ax.day(rect.right()).ceil());
    // Nonworking days.
    if matches!(scale, Scale::Day | Scale::Week) {
        let mut d = a;
        while d <= b {
            if !cal.is_working(d as i64, None) {
                shapes.push(Shape::rect_filled(Rect::from_x_y_ranges(ax.x(d)..=ax.x(d + 1.0), rect.y_range()), 0.0, look.nonwork));
            }
            d += 1.0;
        }
    }
    for (d, _) in units(scale, a, b, false) {
        let x = ax.x(d);
        shapes.push(Shape::line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, look.grid.gamma_multiply(0.6))));
    }
    // Row lines, like the grid's.
    for l in lanes {
        shapes.push(Shape::line_segment([pos2(rect.left(), l.y1), pos2(rect.right(), l.y1)], Stroke::new(1.0, look.grid.gamma_multiply(0.5))));
    }
    let x = ax.x(today);
    if rect.x_range().contains(x) {
        shapes.extend(Shape::dashed_line(&[pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.5 * z.max(0.5), TODAY), 5.0 * z, 3.0 * z));
    }
    let lane_of: std::collections::HashMap<usize, Lane> = lanes.iter().map(|l| (l.task, *l)).collect();
    // Baselines.
    for l in lanes {
        if let Some(Some((bs, bf))) = v.baseline.get(l.task) {
            let h = l.y1 - l.y0;
            let r = Rect::from_min_max(pos2(ax.x(*bs as f64), l.y0 + h * 0.66), pos2(ax.x(*bf as f64 + 1.0), l.y0 + h * 0.82));
            shapes.push(Shape::rect_filled(r, 1.0, BASELINE));
        }
    }
    // Links under the bars.
    for (i, t) in v.input.iter().enumerate() {
        let Some(to) = lane_of.get(&i) else { continue };
        for link in &t.links {
            let Some(from) = lane_of.get(&link.pred) else { continue };
            let red = v.schedule.tasks[i].critical && v.schedule.tasks[link.pred].critical;
            link_shapes(shapes, v, ax, from, to, link.kind, if red { CRITICAL } else { LINK }, z);
        }
    }
    for l in lanes {
        let i = l.task;
        let o = &v.schedule.tasks[i];
        let r = bar_rect(v, ax, l);
        if r.right() < rect.left() - 400.0 || r.left() > rect.right() {
            continue;
        }
        if is_milestone(v, i) {
            let c = r.center();
            let s = r.width() / 2.0;
            let color = if o.critical { CRITICAL } else { SUMMARY };
            shapes.push(Shape::convex_polygon(vec![pos2(c.x, c.y - s), pos2(c.x + s, c.y), pos2(c.x, c.y + s), pos2(c.x - s, c.y)], color, Stroke::NONE));
        } else if o.summary {
            let color = if o.critical { CRITICAL_DONE } else { SUMMARY };
            shapes.push(Shape::rect_filled(r, 0.0, color));
            let t = r.height() * 1.1;
            for x in [r.left(), r.right()] {
                let dx = if x == r.left() { t * 0.7 } else { -t * 0.7 };
                shapes.push(Shape::convex_polygon(vec![pos2(x, r.top()), pos2(x + dx, r.top()), pos2(x, r.bottom() + t * 0.6)].into_iter().collect::<Vec<_>>(), color, Stroke::NONE));
            }
        } else {
            let (fill, done) = if v.input[i].manual {
                (MANUAL, MANUAL.gamma_multiply(0.6))
            } else if o.critical {
                (CRITICAL, CRITICAL_DONE)
            } else {
                (BAR, BAR_DONE)
            };
            shapes.push(Shape::rect_filled(r, 2.0 * z, fill));
            if o.percent > 0.0 {
                let w = r.width() * o.percent.clamp(0.0, 1.0) as f32;
                let h = r.height() * 0.36;
                shapes.push(Shape::rect_filled(Rect::from_min_size(pos2(r.left(), r.center().y - h / 2.0), vec2(w, h)), 0.0, done));
            }
        }
        // Resource names after the bar.
        let names: Vec<String> = v.assignments.get(i).map(|l| l.iter().map(|a| match a.units { Some(u) if u > 0.0 && u < 1.0 => format!("{} [{:.0}%]", a.name, u * 100.0), _ => a.name.clone() }).collect()).unwrap_or_default();
        if !names.is_empty() {
            let g = galley(ctx, names.join(", "), 10.5 * z, look.weak);
            let right = if is_milestone(v, i) { r.right() } else { bar_x(v, ax, i).1 };
            shapes.push(Shape::galley(pos2(right + 5.0 * z, (l.y0 + l.y1) / 2.0 - g.size().y / 2.0), g, look.weak));
        }
    }
}

/// An elbow arrow between two bars for a link of `kind`.
#[allow(clippy::too_many_arguments)]
fn link_shapes(shapes: &mut Vec<Shape>, v: &PlanView, ax: &Axis, from: &Lane, to: &Lane, kind: DepType, color: Color32, z: f32) {
    let (fa, fb) = bar_x(v, ax, from.task);
    let (ta, tb) = bar_x(v, ax, to.task);
    let (fy, ty) = ((from.y0 + from.y1) / 2.0, (to.y0 + to.y1) / 2.0);
    let gap = 7.0 * z;
    let (sx, tx, into_start) = match kind {
        DepType::FS => (fb, ta, true),
        DepType::SS => (fa, ta, true),
        DepType::FF => (fb, tb, false),
        DepType::SF => (fa, tb, false),
    };
    let from_end = matches!(kind, DepType::FS | DepType::FF);
    let out_x = if from_end { sx + gap } else { sx - gap };
    let half = |l: &Lane| (l.y1 - l.y0) / 2.0;
    let pts = if into_start {
        if tx >= out_x + gap && from_end || !from_end && tx >= out_x {
            vec![pos2(sx, fy), pos2(out_x.min(tx - gap).max(if from_end { sx } else { f32::MIN }), fy), pos2(out_x.min(tx - gap).max(if from_end { sx } else { f32::MIN }), ty), pos2(tx, ty)]
        } else {
            // Around: between the rows, then into the start.
            let mid = if ty > fy { to.y0 } else { to.y1 };
            let mid = if (mid - fy).abs() < half(from) { fy + (ty - fy).signum() * half(from) } else { mid };
            vec![pos2(sx, fy), pos2(out_x, fy), pos2(out_x, mid), pos2(tx - gap, mid), pos2(tx - gap, ty), pos2(tx, ty)]
        }
    } else {
        let cx = if from_end { sx.max(tx) + gap } else { (sx - gap).max(tx + gap) };
        vec![pos2(sx, fy), pos2(cx, fy), pos2(cx, ty), pos2(tx, ty)]
    };
    let stroke = Stroke::new(1.2 * z.max(0.6), color);
    shapes.push(Shape::line(pts.clone(), stroke));
    // The arrowhead, along the last segment.
    let (p, q) = (pts[pts.len() - 2], pts[pts.len() - 1]);
    let dir = (q - p).normalized();
    if dir.is_finite() && dir != Vec2::ZERO {
        let s = 4.5 * z.max(0.6);
        let n = vec2(-dir.y, dir.x);
        shapes.push(Shape::convex_polygon(vec![q, q - dir * s * 1.4 + n * s, q - dir * s * 1.4 - n * s], color, Stroke::NONE));
    }
}

/// The resource usage: a row per work resource with its daily load, in
/// red where it goes over the resource's units.
pub(super) fn paint_load(ctx: &egui::Context, shapes: &mut Vec<Shape>, v: &PlanView, ax: &Axis, rect: Rect, look: &Look, z: f32) {
    shapes.push(Shape::rect_filled(rect, 0.0, look.bg));
    shapes.push(Shape::line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, look.grid)));
    let task_cals: Vec<Vec<usize>> = v.input.iter().map(|t| t.calendars.clone()).collect();
    let load = level::loads(&v.schedule, &v.plan.calendars, &task_cals, &v.uses, v.resources.len());
    let row_h = 22.0 * z;
    let label_w = 110.0 * z;
    let title = galley(ctx, tr("plan.load").to_string(), 11.0 * z, look.text);
    shapes.push(Shape::galley(pos2(rect.left() + 4.0, rect.top() + 3.0), title, look.text));
    let mut y = rect.top() + row_h;
    for (ri, res) in v.resources.iter().enumerate() {
        if res.kind != ResKind::Work || y > rect.bottom() {
            continue;
        }
        let cap = res.capacity.max(1e-9);
        let lane = Rect::from_min_max(pos2(rect.left(), y), pos2(rect.right(), y + row_h));
        let over: std::collections::HashSet<i64> = level::overloaded(&load[ri], cap).into_iter().collect();
        for (day, list) in &load[ri] {
            let x = ax.x(*day as f64);
            if x > rect.right() || x + ax.px < rect.left() + label_w {
                continue;
            }
            let units: f64 = list.iter().map(|x| x.1).sum();
            let frac = (units / cap).min(2.0) as f32 / 2.0;
            let h = (row_h - 4.0) * frac.max(0.05);
            let color = if over.contains(day) { OVER } else { LOAD };
            let r = Rect::from_min_max(pos2(x.max(rect.left() + label_w) + 0.5, lane.bottom() - 2.0 - h), pos2(x + ax.px - 0.5, lane.bottom() - 2.0));
            shapes.push(Shape::rect_filled(r, 0.0, color));
        }
        // The 100 % line.
        let full = lane.bottom() - 2.0 - (row_h - 4.0) / 2.0;
        shapes.extend(Shape::dashed_line(&[pos2(rect.left() + label_w, full), pos2(rect.right(), full)], Stroke::new(1.0, look.grid), 3.0, 3.0));
        shapes.push(Shape::rect_filled(Rect::from_min_max(lane.min, pos2(lane.left() + label_w, lane.bottom())), 0.0, look.header));
        let color = if over.is_empty() { look.text } else { OVER };
        let name = galley(ctx, res.name.clone(), 11.0 * z, color);
        shapes.push(Shape::galley(pos2(lane.left() + 4.0, lane.center().y - name.size().y / 2.0), name, color));
        shapes.push(Shape::line_segment([lane.left_bottom(), lane.right_bottom()], Stroke::new(1.0, look.grid.gamma_multiply(0.6))));
        y += row_h;
    }
}

/// The height the resource usage needs.
pub(super) fn load_height(v: &PlanView, z: f32) -> f32 {
    let n = v.resources.iter().filter(|r| r.kind == ResKind::Work).count().max(1);
    22.0 * z * (n as f32 + 1.0) + 2.0
}

impl SheetView {
    /// The Gantt panel beside the grid when the sheet has a task plan.
    pub(super) fn gantt_panel(&mut self, ui: &mut egui::Ui) {
        if self.plan.hidden || self.sh().view == ViewMode::PageLayout {
            return;
        }
        let Some(id) = self.shown_plan() else {
            self.plan.drag = None;
            return;
        };
        let Some(v) = self.plan_view_cached(id) else { return };
        let width = ui.available_width();
        egui::Panel::right(Id::new(("gantt", self.uid)))
            .resizable(true)
            .default_size((width * 0.42).max(200.0))
            .min_size(160.0)
            .max_size((width - 200.0).max(160.0))
            .frame(egui::Frame::new().fill(theme::header_bg(ui.visuals())))
            .show(ui, |ui| match self.plan.views.kind {
                super::plan_views::ViewKind::Gantt => self.gantt_ui(ui, id, &v),
                _ => self.plan_view_ui(ui, id, &v),
            });
    }

    fn gantt_axis(&self, v: &PlanView, x0: f32, z: f32) -> Axis {
        let px = self.plan.scale.px() * z;
        let left = self.plan.left.unwrap_or_else(|| v.schedule.start.min(v.plan.start).floor() - (24.0 / px as f64).ceil());
        Axis { x0, left, px }
    }

    /// The lanes of the tasks (all of them: links reach rows out of view).
    fn gantt_lanes(&self, v: &PlanView, geo: &RowsGeo) -> Vec<Lane> {
        let sh = &self.wb.sheets[v.id.0];
        v.rows.iter().enumerate().filter_map(|(i, &r)| geo.span(sh, r).map(|(y0, y1)| Lane { task: i, y0, y1 })).collect()
    }

    fn gantt_ui(&mut self, ui: &mut egui::Ui, id: TableId, v: &PlanView) {
        let rect = ui.max_rect();
        // The panel keeps the size of what it holds: claim all of it, also
        // on the first frame, before the grid has placed its rows.
        ui.expand_to_include_rect(rect);
        let Some(geo) = self.plan.rows.filter(|g| g.sheet == self.sheet) else {
            ui.ctx().request_repaint();
            return;
        };
        let z = geo.z;
        let head = Rect::from_min_max(pos2(rect.left(), geo.origin_y - HEADER_H * z), pos2(rect.right(), geo.origin_y));
        let mut body = Rect::from_min_max(pos2(rect.left(), geo.origin_y), pos2(rect.right(), geo.bottom.max(geo.origin_y)));
        let load_rect = self.plan.load.then(|| {
            let h = load_height(v, z).min(body.height() * 0.5);
            let r = Rect::from_min_max(pos2(body.left(), body.bottom() - h), body.max);
            body.max.y = r.top();
            r
        });
        let mut ax = self.gantt_axis(v, rect.left(), z);
        let look = Look::paper(theme::header_bg(ui.visuals()));
        let ctx = ui.ctx().clone();

        // Input first, so that the picture follows it this frame.
        let resp = ui.interact(Rect::from_min_max(head.min, body.max), Id::new(("gantt_area", self.uid)), Sense::click_and_drag());
        let lanes = self.gantt_lanes(v, &geo);
        self.gantt_input(ui, &resp, id, v, &mut ax, &lanes, head, body);

        let mut shapes = Vec::new();
        let sh = &self.wb.sheets[id.0];
        // The finer tier on the table's header row, when it shows.
        let header_row = self.wb.table(id).filter(|t| t.header).and_then(|t| geo.span(sh, t.range.start.row)).filter(|(y0, y1)| *y1 > body.top() && *y0 < body.bottom());
        paint_body(&ctx, &mut shapes, v, &ax, &lanes, body, self.plan.scale, &look, z, apex_sheet::format::now_serial().floor());
        match header_row {
            Some((y0, y1)) => {
                paint_scale(&ctx, &mut shapes, &ax, head, self.plan.scale, true, &look, z);
                paint_scale(&ctx, &mut shapes, &ax, Rect::from_min_max(pos2(body.left(), y0), pos2(body.right(), y1)), self.plan.scale, false, &look, z);
            }
            None => paint_scale(&ctx, &mut shapes, &ax, head, self.plan.scale, false, &look, z),
        }
        if let Some(d) = self.plan.drag {
            self.drag_preview(&mut shapes, v, &ax, &lanes, &d, z);
        }
        if let Some(r) = load_rect {
            paint_load(&ctx, &mut shapes, v, &ax, r, &look, z);
        }
        let painter = ui.painter_at(Rect::from_min_max(head.min, load_rect.map_or(body.max, |r| r.max)));
        painter.extend(shapes);
        painter.line_segment([pos2(rect.left(), head.top()), pos2(rect.left(), rect.bottom())], Stroke::new(1.0, look.grid));

        // Details of the task under the pointer.
        if self.plan.drag.is_none()
            && let Some(p) = resp.hover_pos()
            && let Some(l) = lanes.iter().find(|l| bar_rect(v, &ax, l).expand(3.0).contains(p))
        {
            let i = l.task;
            resp.on_hover_ui_at_pointer(|ui| self.task_tooltip(ui, v, i));
        }
    }

    fn task_tooltip(&self, ui: &mut egui::Ui, v: &PlanView, i: usize) {
        let o = &v.schedule.tasks[i];
        let wb = &self.wb;
        ui.label(RichText::new(format!("{}  {}", o.wbs, v.names[i])).strong());
        egui::Grid::new("gantt_tip").num_columns(2).spacing(vec2(10.0, 2.0)).show(ui, |ui| {
            let fname = apex_sheet::workbook::field_name;
            ui.label(fname(Field::Start));
            ui.label(super::plan_ui::date_text(wb, o.start_day as f64));
            ui.end_row();
            ui.label(fname(Field::Finish));
            ui.label(super::plan_ui::date_text(wb, o.finish_day as f64));
            ui.end_row();
            ui.label(fname(Field::Duration));
            ui.label(trf("plan.days", &[("n", &((o.duration * 100.0).round() / 100.0))]));
            ui.end_row();
            ui.label(fname(Field::Slack));
            ui.label(trf("plan.days", &[("n", &((o.slack * 100.0).round() / 100.0))]));
            ui.end_row();
            ui.label(fname(Field::Percent));
            ui.label(format!("{:.0}%", o.percent * 100.0));
            ui.end_row();
        });
        if o.critical {
            ui.label(RichText::new(tr("plan.critical_task")).color(CRITICAL));
        }
    }

    fn drag_preview(&self, shapes: &mut Vec<Shape>, v: &PlanView, ax: &Axis, lanes: &[Lane], d: &GanttDrag, z: f32) {
        let Some(l) = lanes.iter().find(|l| l.task == d.task) else { return };
        let r = bar_rect(v, ax, l);
        let stroke = Stroke::new(1.5 * z.max(0.6), Color32::from_rgb(0x10, 0x10, 0x10));
        match d.kind {
            DragKind::Move => {
                let days = ((d.to.x - d.from.x) / ax.px).round();
                let g = r.translate(vec2(days * ax.px, 0.0));
                shapes.push(Shape::rect_stroke(g, 2.0, stroke, StrokeKind::Middle));
            }
            DragKind::Stretch => {
                let end = ax.x(ax.day(d.to.x).floor() + 1.0).max(r.left() + 2.0);
                shapes.push(Shape::rect_stroke(Rect::from_min_max(r.min, pos2(end, r.bottom())), 2.0, stroke, StrokeKind::Middle));
            }
            DragKind::Link => {
                shapes.extend(Shape::dashed_line(&[r.center(), d.to], stroke, 4.0, 3.0));
                if let Some(t) = lanes.iter().find(|x| x.y0 <= d.to.y && d.to.y < x.y1 && x.task != d.task) {
                    shapes.push(Shape::rect_stroke(bar_rect(v, ax, t).expand(2.0), 2.0, stroke, StrokeKind::Outside));
                }
            }
            DragKind::Pending | DragKind::Pan => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn gantt_input(&mut self, ui: &mut egui::Ui, resp: &egui::Response, id: TableId, v: &PlanView, ax: &mut Axis, lanes: &[Lane], head: Rect, body: Rect) {
        // Wheel: rows scroll with the grid; Shift or a sideways wheel
        // scrolls the dates; Ctrl changes the time unit.
        if resp.hovered() {
            let (delta, zoom, shift) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift));
            if zoom != 1.0 {
                let s = self.plan.scale.step(if zoom > 1.0 { -1 } else { 1 });
                if s != self.plan.scale {
                    // Keep the day under the pointer in place.
                    let at_x = resp.hover_pos().map_or(ax.x0, |p| p.x);
                    let at = ax.day(at_x);
                    self.plan.scale = s;
                    self.plan.left = Some(at - ((at_x - ax.x0) / (s.px() * self.zoom)) as f64);
                    *ax = self.gantt_axis(v, ax.x0, self.zoom);
                }
            } else if delta != Vec2::ZERO {
                let (dx, dy) = if shift && delta.x == 0.0 { (delta.y, 0.0) } else { (delta.x, delta.y) };
                if dy != 0.0 {
                    self.scroll.y = (self.scroll.y - dy / self.zoom).max(0.0);
                }
                if dx != 0.0 {
                    ax.left -= (dx / ax.px) as f64;
                    self.plan.left = Some(ax.left);
                }
            }
        }
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        // A drag starts where the button went down, not a few pixels later
        // where egui calls it a drag: a bar's edge is only 5 px wide.
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        {
            let hit = lanes.iter().find(|l| bar_rect(v, ax, l).expand(3.0).contains(p));
            self.plan.drag = if head.contains(p) {
                Some(GanttDrag { kind: DragKind::Pan, task: 0, from: p, to: p, left0: ax.left })
            } else if let Some(l) = hit {
                let r = bar_rect(v, ax, l);
                let o = &v.schedule.tasks[l.task];
                let kind = if !o.summary && !is_milestone(v, l.task) && (p.x - r.right()).abs() <= 5.0 { DragKind::Stretch } else { DragKind::Pending };
                Some(GanttDrag { kind, task: l.task, from: p, to: p, left0: ax.left })
            } else {
                Some(GanttDrag { kind: DragKind::Pan, task: 0, from: p, to: p, left0: ax.left })
            };
        }
        if let (Some(d), Some(p)) = (self.plan.drag.as_mut(), pointer) {
            d.to = p;
            let delta = p - d.from;
            if d.kind == DragKind::Pending && delta.length() > 4.0 {
                let summary = v.schedule.tasks[d.task].summary;
                d.kind = if delta.y.abs() > delta.x.abs() { DragKind::Link } else if summary { DragKind::Pan } else { DragKind::Move };
                d.left0 = ax.left;
            }
            if d.kind == DragKind::Pan {
                ax.left = d.left0 - ((p.x - d.from.x) / ax.px) as f64;
                self.plan.left = Some(ax.left);
            }
            match d.kind {
                DragKind::Stretch => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                DragKind::Move | DragKind::Pan => ui.ctx().set_cursor_icon(CursorIcon::Grabbing),
                DragKind::Link => ui.ctx().set_cursor_icon(CursorIcon::Alias),
                DragKind::Pending => {}
            }
        } else if let Some(p) = resp.hover_pos()
            && let Some(l) = lanes.iter().find(|l| bar_rect(v, ax, l).expand(3.0).contains(p))
        {
            let r = bar_rect(v, ax, l);
            let o = &v.schedule.tasks[l.task];
            let icon = if !o.summary && !is_milestone(v, l.task) && (p.x - r.right()).abs() <= 5.0 { CursorIcon::ResizeHorizontal } else { CursorIcon::Grab };
            ui.ctx().set_cursor_icon(icon);
        }
        if resp.drag_stopped()
            && let Some(d) = self.plan.drag.take()
        {
            self.gantt_drop(id, v, ax, lanes, &d);
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && body.contains(p)
            && let Some(l) = lanes.iter().find(|l| l.y0 <= p.y && p.y < l.y1)
        {
            self.settle_edit();
            self.cursor = CellRef::new(v.rows[l.task], v.col(Field::Name).unwrap_or(self.cursor.col));
            self.anchor = self.cursor;
            self.focus_grid = true;
        }
    }

    /// Applies a finished drag.
    fn gantt_drop(&mut self, id: TableId, v: &PlanView, ax: &Axis, lanes: &[Lane], d: &GanttDrag) {
        let row = v.rows[d.task];
        let o = &v.schedule.tasks[d.task];
        self.settle_edit();
        match d.kind {
            DragKind::Move => {
                let days = ((d.to.x - d.from.x) / ax.px).round() as f64;
                if days != 0.0 {
                    self.wb.plan_move_task(id, row, o.start_day as f64 + days);
                }
            }
            DragKind::Stretch => {
                let end = ax.day(d.to.x).floor();
                if end as i64 != o.finish_day {
                    let days = if end < o.start_day as f64 { 0.0 } else { Workbook::plan_days_between(v, o.start_day as f64, end) };
                    self.wb.plan_set_duration(id, row, days);
                }
            }
            DragKind::Link => {
                if let Some(t) = lanes.iter().find(|x| x.y0 <= d.to.y && d.to.y < x.y1 && x.task != d.task) {
                    self.wb.plan_add_link(id, row, v.rows[t.task], DepType::FS);
                }
            }
            DragKind::Pending | DragKind::Pan => {}
        }
    }

    /// Scrolls the dates to the task at the cursor.
    pub(super) fn gantt_scroll_to_task(&mut self, id: TableId) {
        let Some(v) = self.plan_view_cached(id) else { return };
        let i = v.task_at_row(self.cursor.row).unwrap_or(0);
        if let Some(o) = v.schedule.tasks.get(i) {
            self.plan.left = Some(o.start.floor() - 3.0 * self.plan.scale.days_per_unit());
            self.plan.hidden = false;
        }
    }

    /// Remembers where the grid put its rows (for the Gantt).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn note_rows_geo(&mut self, origin: Pos2, view: Rect, z: f32, scroll_y: f32, fr: u32, fy: f32, oy: f32) {
        self.plan.rows = Some(RowsGeo { sheet: self.sheet, origin_y: origin.y, z, scroll_y, fr, fy, oy, bottom: view.max.y });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_units() {
        let a = date_to_serial(2026, 9, 28);
        let days = units(Scale::Day, a, a + 6.0, false);
        assert_eq!(days.len(), 8);
        assert_eq!(days[1], (a, "28".to_string()));
        let weeks = units(Scale::Week, a + 2.0, a + 20.0, false);
        // Mondays only.
        assert!(weeks.iter().all(|(d, _)| weekday(*d as i64) == 0));
        let months = units(Scale::Month, a, a + 70.0, false);
        // September to December.
        assert_eq!(months.len(), 4);
        assert_eq!(months[1].0, date_to_serial(2026, 10, 1));
        let q = units(Scale::Quarter, a, a + 200.0, false);
        assert_eq!(q[0].0, date_to_serial(2026, 7, 1));
        let years = units(Scale::Quarter, a, a + 400.0, true);
        assert_eq!(years.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["2026", "2027"]);
        assert_eq!(Scale::Day.step(-1), Scale::Day);
        assert_eq!(Scale::Week.step(1), Scale::Month);
    }

    #[test]
    fn rows_follow_the_grid() {
        let sh = Sheet::new("S");
        let g = RowsGeo { sheet: 0, origin_y: 100.0, z: 1.0, scroll_y: 0.0, fr: 0, fy: 0.0, oy: 0.0, bottom: 500.0 };
        let (y0, y1) = g.span(&sh, 2).unwrap();
        assert_eq!(y0, 100.0 + sh.row_y(2));
        assert_eq!(y1 - y0, sh.row_height(2));
        let scrolled = RowsGeo { scroll_y: sh.row_y(3), fr: 1, fy: sh.row_y(1), ..g };
        assert!(scrolled.span(&sh, 2).is_none());
        assert_eq!(scrolled.span(&sh, 0).unwrap().0, 100.0);
        assert_eq!(scrolled.span(&sh, 4).unwrap().0, 100.0 + sh.row_height(3));
        let ax = Axis { x0: 10.0, left: 100.0, px: 5.0 };
        assert_eq!(ax.x(102.0), 20.0);
        assert_eq!(ax.day(20.0), 102.0);
    }
}
