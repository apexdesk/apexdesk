//! Planning > Views: other pictures of a plan in the panel beside the
//! table, instead of the Gantt: the network diagram (the critical path in
//! red), the team planner (a lane per resource; dragging a task to another
//! lane reassigns it), a month calendar, and the work per period of each
//! task (Task Usage) or each resource (Resource Usage).

use std::collections::BTreeMap;

use apex_sheet::format::{date_to_serial, serial_to_date};
use apex_sheet::plan::calendar::weekday;
use apex_sheet::workbook::{PlanView, TableId};
use eframe::egui::Shape;

use super::gantt::{Axis, BAR, CRITICAL, LINK, Look, OVER, SUMMARY, Scale, TODAY, galley, month_names, paint_scale, short_month};
use super::plan_ui::date_text;
use super::*;

/// What the panel beside a plan shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum ViewKind {
    #[default]
    Gantt,
    Network,
    Team,
    Calendar,
    TaskUsage,
    ResourceUsage,
}

impl ViewKind {
    pub const ALL: [ViewKind; 6] = [ViewKind::Gantt, ViewKind::Network, ViewKind::Team, ViewKind::Calendar, ViewKind::TaskUsage, ViewKind::ResourceUsage];

    pub fn key(self) -> &'static str {
        match self {
            ViewKind::Gantt => "plan.view.gantt",
            ViewKind::Network => "plan.view.network",
            ViewKind::Team => "plan.view.team",
            ViewKind::Calendar => "plan.view.calendar",
            ViewKind::TaskUsage => "plan.view.task_usage",
            ViewKind::ResourceUsage => "plan.view.resource_usage",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            ViewKind::Gantt => Icon::ChartBar,
            ViewKind::Network => Icon::Link,
            ViewKind::Team => Icon::Group,
            ViewKind::Calendar => Icon::Calendar,
            ViewKind::TaskUsage => Icon::Table,
            ViewKind::ResourceUsage => Icon::ChartColumn,
        }
    }
}

/// The state of the views.
#[derive(Default)]
pub(super) struct ViewsState {
    pub kind: ViewKind,
    /// The calendar's month (None: the project start's).
    pub month: Option<(i64, u32)>,
    /// Team Planner: the task being dragged and the lane it comes from.
    drag: Option<(usize, usize)>,
}

// --- layouts (no painting) ---------------------------------------------------------

/// Network diagram: the column and row of each task that is not a
/// summary. A task's column is one after its latest predecessor's.
pub(super) fn network_layout(v: &PlanView) -> Vec<(usize, usize, usize)> {
    let n = v.schedule.tasks.len();
    let leaf = |i: usize| i < n && !v.schedule.tasks[i].summary;
    let mut rank = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for i in (0..n).filter(|&i| leaf(i)) {
            for l in &v.input[i].links {
                if leaf(l.pred) && rank[l.pred] + 1 > rank[i] && rank[l.pred] < n {
                    rank[i] = rank[l.pred] + 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut rows: BTreeMap<usize, usize> = BTreeMap::new();
    (0..n)
        .filter(|&i| leaf(i))
        .map(|i| {
            let r = rows.entry(rank[i]).or_insert(0);
            *r += 1;
            (i, rank[i], *r - 1)
        })
        .collect()
}

/// A lane of the team planner: a resource (None: unassigned tasks) and its
/// tasks with the sub-row each one takes, so that none overlaps.
#[derive(Debug, PartialEq)]
pub(super) struct TeamLane {
    pub name: Option<String>,
    pub tasks: Vec<(usize, usize)>,
    pub rows: usize,
}

/// Puts `tasks` (index, first day, last day) in rows without overlaps.
fn pack(mut tasks: Vec<(usize, i64, i64)>) -> (Vec<(usize, usize)>, usize) {
    tasks.sort_by_key(|t| (t.1, t.0));
    let mut ends: Vec<i64> = Vec::new();
    let mut out = Vec::new();
    for (i, a, b) in tasks {
        let row = match ends.iter().position(|e| *e < a) {
            Some(r) => r,
            None => {
                ends.push(i64::MIN);
                ends.len() - 1
            }
        };
        ends[row] = b;
        out.push((i, row));
    }
    out.sort();
    (out, ends.len().max(1))
}

/// The team planner's lanes: the plan's resources and the names its
/// tasks use, then the tasks nobody does.
pub(super) fn team_lanes(v: &PlanView) -> Vec<TeamLane> {
    let mut names: Vec<String> = v.resources.iter().map(|r| r.name.clone()).collect();
    for list in &v.assignments {
        for a in list {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&a.name)) {
                names.push(a.name.clone());
            }
        }
    }
    let leaf = |i: usize| !v.schedule.tasks[i].summary;
    let span = |i: usize| (i, v.schedule.tasks[i].start_day, v.schedule.tasks[i].finish_day);
    let mut out: Vec<TeamLane> = names
        .into_iter()
        .map(|name| {
            let tasks = (0..v.schedule.tasks.len()).filter(|&i| leaf(i) && v.assignments.get(i).is_some_and(|l| l.iter().any(|a| a.name.eq_ignore_ascii_case(&name)))).map(span).collect();
            let (tasks, rows) = pack(tasks);
            TeamLane { name: Some(name), tasks, rows }
        })
        .collect();
    let free = (0..v.schedule.tasks.len()).filter(|&i| leaf(i) && v.assignments.get(i).is_none_or(|l| l.is_empty())).map(span).collect();
    let (tasks, rows) = pack(free);
    out.push(TeamLane { name: None, tasks, rows });
    out
}

/// The Mondays of the weeks that show month `m` of year `y`.
pub(super) fn month_weeks(y: i64, m: u32) -> Vec<i64> {
    let first = date_to_serial(y, m as i64, 1) as i64;
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let last = date_to_serial(ny, nm as i64, 1) as i64 - 1;
    let mut monday = first - weekday(first) as i64;
    let mut out = Vec::new();
    while monday <= last {
        out.push(monday);
        monday += 7;
    }
    out
}

/// The tasks in the week from `monday`: task, first and last weekday
/// shown (0..7) and the row each takes.
pub(super) fn week_bars(v: &PlanView, monday: i64) -> (Vec<(usize, usize, usize, usize)>, usize) {
    let tasks: Vec<(usize, i64, i64)> = (0..v.schedule.tasks.len())
        .filter(|&i| !v.schedule.tasks[i].summary)
        .map(|i| (i, v.schedule.tasks[i].start_day, v.schedule.tasks[i].finish_day.max(v.schedule.tasks[i].start_day)))
        .filter(|t| t.1 <= monday + 6 && t.2 >= monday)
        .collect();
    let spans: BTreeMap<usize, (i64, i64)> = tasks.iter().map(|t| (t.0, (t.1, t.2))).collect();
    let (rows, n) = pack(tasks);
    let bars = rows.into_iter().map(|(i, r)| (i, (spans[&i].0 - monday).max(0) as usize, (spans[&i].1 - monday).min(6) as usize, r)).collect();
    (bars, if spans.is_empty() { 0 } else { n })
}

/// The first day of the period of `day` on scale `s` (days, weeks from
/// Monday, months, quarters).
pub(super) fn period_start(day: i64, s: Scale) -> i64 {
    match s {
        Scale::Day => day,
        Scale::Week => day - weekday(day) as i64,
        Scale::Month | Scale::Quarter => {
            let (y, m, _) = serial_to_date(day as f64);
            let m = if s == Scale::Quarter { (m - 1) / 3 * 3 + 1 } else { m };
            date_to_serial(y, m as i64, 1) as i64
        }
    }
}

fn next_period(p: i64, s: Scale) -> i64 {
    match s {
        Scale::Day => p + 1,
        Scale::Week => p + 7,
        Scale::Month | Scale::Quarter => {
            let (y, m, _) = serial_to_date(p as f64);
            let k = if s == Scale::Quarter { 3 } else { 1 };
            let m = m as i64 - 1 + k;
            date_to_serial(y + m / 12, m % 12 + 1, 1) as i64
        }
    }
}

/// Hours of work per period.
#[derive(Debug, Default)]
pub(super) struct Usage {
    pub periods: Vec<i64>,
    /// By (task, resource), hours per period.
    pub work: BTreeMap<(usize, usize), BTreeMap<i64, f64>>,
    /// Resources overloaded on some day of a period.
    pub over: std::collections::BTreeSet<(usize, i64)>,
}

impl Usage {
    /// Hours of `pairs` in period `p`.
    pub fn sum(&self, pairs: impl Iterator<Item = (usize, usize)>, p: i64) -> f64 {
        pairs.filter_map(|k| self.work.get(&k).and_then(|m| m.get(&p))).sum()
    }
}

pub(super) fn usage(v: &PlanView, s: Scale) -> Usage {
    let hpd = v.plan.hours_per_day();
    let mut u = Usage::default();
    let (mut lo, mut hi) = (i64::MAX, i64::MIN);
    for (res, days) in v.loads().iter().enumerate() {
        let cap = v.resources.get(res).map_or(1.0, |r| r.capacity);
        for (&day, list) in days {
            let p = period_start(day, s);
            (lo, hi) = (lo.min(p), hi.max(p));
            if list.iter().map(|x| x.1).sum::<f64>() > cap + 1e-6 {
                u.over.insert((res, p));
            }
            for &(task, units) in list {
                *u.work.entry((task, res)).or_default().entry(p).or_default() += units * hpd;
            }
        }
    }
    let mut p = lo;
    while p <= hi && u.periods.len() < 2000 {
        u.periods.push(p);
        p = next_period(p, s);
    }
    u
}

/// Hours as the usage views show them.
fn hours(h: f64) -> String {
    if h.abs() < 0.005 {
        String::new()
    } else if (h - h.round()).abs() < 0.05 {
        format!("{:.0}h", h)
    } else {
        format!("{:.1}h", h)
    }
}

// --- painting ----------------------------------------------------------------------

const ROW: f32 = 22.0;

impl SheetView {
    /// The panel beside the plan when it shows a view other than the Gantt.
    pub(super) fn plan_view_ui(&mut self, ui: &mut egui::Ui, id: TableId, v: &PlanView) {
        let look = Look::paper(theme::header_bg(ui.visuals()));
        let rect = ui.max_rect();
        ui.expand_to_include_rect(rect);
        ui.painter().rect_filled(rect, 0.0, look.bg);
        ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), 26.0)), 0.0, look.header);
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.add_space(6.0);
            ui.label(RichText::new(tr(self.plan.views.kind.key())).strong().color(look.head_text));
            if self.plan.views.kind == ViewKind::Calendar {
                self.month_nav(ui, v);
            }
        });
        egui::ScrollArea::both().id_salt(("plan_view", self.uid)).auto_shrink([false, false]).show(ui, |ui| match self.plan.views.kind {
            ViewKind::Network => self.network_ui(ui, v, &look),
            ViewKind::Team => self.team_ui(ui, id, v, &look),
            ViewKind::Calendar => self.calendar_ui(ui, v, &look),
            ViewKind::TaskUsage => self.usage_ui(ui, v, &look, false),
            ViewKind::ResourceUsage => self.usage_ui(ui, v, &look, true),
            ViewKind::Gantt => {}
        });
    }

    fn network_ui(&mut self, ui: &mut egui::Ui, v: &PlanView, look: &Look) {
        let layout = network_layout(v);
        let (w, h, gx, gy) = (176.0, 70.0, 44.0, 18.0);
        let cols = layout.iter().map(|x| x.1 + 1).max().unwrap_or(0);
        let rows = layout.iter().map(|x| x.2 + 1).max().unwrap_or(0);
        let size = vec2(cols as f32 * (w + gx) + gx, rows as f32 * (h + gy) + gy);
        let (rect, _) = ui.allocate_exact_size(size.max(vec2(10.0, 10.0)), Sense::hover());
        let ctx = ui.ctx().clone();
        let at: BTreeMap<usize, Rect> = layout.iter().map(|&(i, c, r)| (i, Rect::from_min_size(rect.min + vec2(gx + c as f32 * (w + gx), gy + r as f32 * (h + gy)), vec2(w, h)))).collect();
        let p = ui.painter().clone();
        // Links under the boxes.
        for (&i, to) in &at {
            for l in &v.input[i].links {
                let Some(from) = at.get(&l.pred) else { continue };
                let red = v.schedule.tasks[i].critical && v.schedule.tasks[l.pred].critical;
                let color = if red { CRITICAL } else { LINK };
                let (a, b) = (from.right_center(), to.left_center());
                let mid = if b.x > a.x { (a.x + b.x) / 2.0 } else { a.x + gx / 2.0 };
                let pts = vec![a, pos2(mid, a.y), pos2(mid, b.y), b];
                p.add(Shape::line(pts, Stroke::new(if red { 2.0 } else { 1.2 }, color)));
                p.add(Shape::convex_polygon(vec![b, pos2(b.x - 7.0, b.y - 4.0), pos2(b.x - 7.0, b.y + 4.0)], color, Stroke::NONE));
            }
        }
        let names = month_names();
        let short = |d: i64| {
            let (_, m, dd) = serial_to_date(d as f64);
            format!("{dd} {}", short_month(&names, m))
        };
        for (&i, r) in &at {
            let o = &v.schedule.tasks[i];
            let color = if o.critical { CRITICAL } else { BAR };
            p.rect_filled(*r, 4.0, Color32::WHITE);
            p.rect_stroke(*r, 4.0, Stroke::new(if o.critical { 2.2 } else { 1.2 }, color), StrokeKind::Inside);
            let milestone = v.input[i].duration <= 0.0;
            let head = Rect::from_min_size(r.min, vec2(r.width(), 20.0));
            p.rect_filled(head.shrink(1.0), egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, color.gamma_multiply(0.15));
            let title = format!("{}{}  {}", if milestone { "◆ " } else { "" }, v.ids.get(i).copied().unwrap_or(i + 1), v.names.get(i).cloned().unwrap_or_default());
            let g = galley(&ctx, title, 12.0, look.text);
            p.with_clip_rect(r.shrink(3.0)).galley(pos2(r.left() + 6.0, r.top() + 3.0), g, look.text);
            let lines = [format!("{}  {}", tr("plan.net_start"), short(o.start_day)), format!("{}  {}", tr("plan.net_finish"), short(o.finish_day)), format!("{}  {} · {:.0}%", tr("plan.net_duration"), format!("{}", (o.duration * 100.0).round() / 100.0), o.percent * 100.0)];
            for (k, s) in lines.into_iter().enumerate() {
                let g = galley(&ctx, s, 10.5, look.weak);
                p.with_clip_rect(r.shrink(3.0)).galley(pos2(r.left() + 6.0, r.top() + 23.0 + k as f32 * 15.0), g, look.weak);
            }
        }
        if at.is_empty() {
            ui.label(tr("plan.view_empty"));
        }
    }

    /// The axis of the views with a timeline, from the plan's first day.
    fn view_axis(v: &PlanView, x0: f32, scale: Scale) -> (Axis, f32) {
        let (first, last) = v.schedule.tasks.iter().fold((v.plan.start, v.plan.start), |(a, b), o| (a.min(o.start), b.max(o.finish)));
        let px = scale.px();
        let left = first.floor() - 2.0 * scale.days_per_unit().min(7.0);
        let right = last.ceil() + 4.0 * scale.days_per_unit().min(7.0);
        (Axis { x0, left, px }, ((right - left) as f32 * px).max(200.0))
    }

    fn team_ui(&mut self, ui: &mut egui::Ui, id: TableId, v: &PlanView, look: &Look) {
        let lanes = team_lanes(v);
        let label_w = 130.0;
        let scale = self.plan.scale;
        let heights: Vec<f32> = lanes.iter().map(|l| l.rows as f32 * ROW + 8.0).collect();
        let head = HEADER_H;
        let origin = ui.cursor().min;
        let (ax, tw) = Self::view_axis(v, origin.x + label_w, scale);
        let size = vec2(label_w + tw, head + heights.iter().sum::<f32>());
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let mut shapes = Vec::new();
        paint_scale(&ctx, &mut shapes, &ax, Rect::from_min_max(pos2(rect.left() + label_w, rect.top()), pos2(rect.right(), rect.top() + head)), scale, false, look, 1.0);
        p.extend(shapes);
        // Lanes, and where each task's bar is.
        let mut y = rect.top() + head;
        let mut tops = Vec::new();
        let mut bars: Vec<(usize, usize, Rect)> = Vec::new();
        for (k, lane) in lanes.iter().enumerate() {
            let band = Rect::from_min_size(pos2(rect.left(), y), vec2(size.x, heights[k]));
            tops.push((band.top(), band.bottom()));
            if k % 2 == 1 {
                p.rect_filled(band, 0.0, look.nonwork);
            }
            p.hline(band.x_range(), band.bottom(), Stroke::new(1.0, look.grid));
            let name = lane.name.clone().unwrap_or_else(|| tr("plan.team_unassigned").to_string());
            let g = galley(&ctx, name, 12.0, if lane.name.is_some() { look.text } else { look.weak });
            p.with_clip_rect(Rect::from_min_size(band.min, vec2(label_w - 4.0, band.height()))).galley(pos2(band.left() + 6.0, band.top() + 5.0), g, look.text);
            for &(i, row) in &lane.tasks {
                let o = &v.schedule.tasks[i];
                let (x0, x1) = (ax.x(o.start_day as f64), ax.x(o.finish_day as f64 + 1.0));
                let top = band.top() + 4.0 + row as f32 * ROW;
                bars.push((i, k, Rect::from_min_max(pos2(x0, top + 2.0), pos2(x1.max(x0 + 8.0), top + ROW - 3.0))));
            }
            y += heights[k];
        }
        p.vline(rect.left() + label_w, rect.y_range(), Stroke::new(1.0, look.grid));
        // Input: drag a bar up or down to another lane.
        let pointer = ui.ctx().pointer_latest_pos();
        for &(i, k, r) in &bars {
            let resp = ui.interact(r, Id::new(("team_bar", self.uid, i, k)), Sense::drag());
            if resp.drag_started() {
                self.plan.views.drag = Some((i, k));
            }
            if resp.hovered() && self.plan.views.drag.is_none() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
            if resp.drag_stopped() && self.plan.views.drag == Some((i, k)) {
                self.plan.views.drag = None;
                let to = pointer.and_then(|pt| tops.iter().position(|(a, b)| pt.y >= *a && pt.y < *b));
                if let Some(to) = to.filter(|t| *t != k) {
                    self.settle_edit();
                    self.wb.plan_reassign(id, i, lanes[k].name.as_deref(), lanes[to].name.as_deref());
                }
            }
        }
        for &(i, k, r) in &bars {
            let o = &v.schedule.tasks[i];
            let dragging = self.plan.views.drag == Some((i, k));
            let color = if o.critical { CRITICAL } else { BAR };
            p.rect_filled(r, 3.0, if dragging { color.gamma_multiply(0.35) } else { color });
            let g = galley(&ctx, v.names.get(i).cloned().unwrap_or_default(), 11.0, Color32::WHITE);
            p.with_clip_rect(r.shrink(2.0)).galley(pos2(r.left() + 4.0, r.center().y - g.size().y / 2.0), g, Color32::WHITE);
            if dragging && let Some(pt) = pointer {
                let ghost = r.translate(vec2(0.0, pt.y - r.center().y));
                p.rect_stroke(ghost, 3.0, Stroke::new(2.0, color), StrokeKind::Inside);
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
        }
        let today = apex_sheet::format::now_serial().floor();
        let x = ax.x(today);
        if x > rect.left() + label_w && x < rect.right() {
            p.vline(x, (rect.top() + head)..=rect.bottom(), Stroke::new(1.5, TODAY));
        }
    }

    fn month_nav(&mut self, ui: &mut egui::Ui, v: &PlanView) {
        let (y, m) = self.plan.views.month.unwrap_or_else(|| {
            let (y, m, _) = serial_to_date(v.schedule.start.min(v.plan.start).max(1.0));
            (y, m)
        });
        let names = month_names();
        if ui.small_button("◀").on_hover_text(tr("plan.cal_prev")).clicked() {
            self.plan.views.month = Some(if m == 1 { (y - 1, 12) } else { (y, m - 1) });
        }
        ui.label(format!("{} {y}", names.get(m as usize - 1).cloned().unwrap_or_default()));
        if ui.small_button("▶").on_hover_text(tr("plan.cal_next")).clicked() {
            self.plan.views.month = Some(if m == 12 { (y + 1, 1) } else { (y, m + 1) });
        }
        if ui.small_button(tr("plan.today")).clicked() {
            let (y, m, _) = serial_to_date(apex_sheet::format::now_serial());
            self.plan.views.month = Some((y, m));
        }
        if self.plan.views.month.is_none() {
            self.plan.views.month = Some((y, m));
        }
    }

    fn calendar_ui(&mut self, ui: &mut egui::Ui, v: &PlanView, look: &Look) {
        let Some((y, m)) = self.plan.views.month else { return };
        let weeks = month_weeks(y, m);
        let avail = if ui.available_width().is_finite() { ui.available_width().min(ui.clip_rect().width()) } else { ui.clip_rect().width() };
        let cw = ((avail - 14.0) / 7.0).max(60.0);
        let bars: Vec<_> = weeks.iter().map(|&w| week_bars(v, w)).collect();
        let heights: Vec<f32> = bars.iter().map(|b| (22.0 + b.1 as f32 * 18.0 + 6.0).max(84.0)).collect();
        let size = vec2(cw * 7.0, 22.0 + heights.iter().sum::<f32>());
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let days: Vec<String> = tr("field.days").split('|').map(str::to_string).collect();
        for d in 0..7 {
            let r = Rect::from_min_size(rect.min + vec2(d as f32 * cw, 0.0), vec2(cw, 22.0));
            p.rect_filled(r, 0.0, look.header);
            let name: String = days.get((d + 1) % 7).map(|s| s.chars().take(3).collect()).unwrap_or_default();
            let g = galley(&ctx, name, 11.5, look.head_text);
            p.galley(pos2(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0), g, look.head_text);
        }
        let cal = v.plan.calendar();
        let today = apex_sheet::format::now_serial().floor() as i64;
        let mut top = rect.top() + 22.0;
        for (k, &monday) in weeks.iter().enumerate() {
            let h = heights[k];
            for d in 0..7 {
                let day = monday + d as i64;
                let r = Rect::from_min_size(pos2(rect.left() + d as f32 * cw, top), vec2(cw, h));
                let (_, dm, dd) = serial_to_date(day as f64);
                if !cal.is_working(day, None) {
                    p.rect_filled(r, 0.0, look.nonwork);
                }
                p.rect_stroke(r, 0.0, Stroke::new(1.0, look.grid), StrokeKind::Inside);
                let color = if dm != m { look.grid } else if day == today { TODAY } else { look.weak };
                let g = galley(&ctx, dd.to_string(), 11.0, color);
                p.galley(pos2(r.right() - g.size().x - 5.0, r.top() + 3.0), g, color);
            }
            for &(i, a, b, row) in &bars[k].0 {
                let o = &v.schedule.tasks[i];
                let r = Rect::from_min_max(pos2(rect.left() + a as f32 * cw + 3.0, top + 20.0 + row as f32 * 18.0), pos2(rect.left() + (b + 1) as f32 * cw - 3.0, top + 20.0 + row as f32 * 18.0 + 15.0));
                let color = if v.input[i].duration <= 0.0 { SUMMARY } else if o.critical { CRITICAL } else { BAR };
                p.rect_filled(r, 3.0, color);
                let text = if v.input[i].duration <= 0.0 { format!("◆ {}", v.names[i]) } else { v.names[i].clone() };
                let g = galley(&ctx, text, 10.5, Color32::WHITE);
                p.with_clip_rect(r.shrink(1.0)).galley(pos2(r.left() + 4.0, r.center().y - g.size().y / 2.0), g, Color32::WHITE);
            }
            top += h;
        }
    }

    fn usage_ui(&mut self, ui: &mut egui::Ui, v: &PlanView, look: &Look, by_resource: bool) {
        let scale = self.plan.scale;
        let u = usage(v, scale);
        let names = month_names();
        let period_name = |p: i64| match scale {
            Scale::Day | Scale::Week => date_text(&self.wb, p as f64),
            Scale::Month | Scale::Quarter => {
                let (y, m, _) = serial_to_date(p as f64);
                format!("{} {y}", short_month(&names, m))
            }
        };
        // Rows: label, level, bold, the (task, resource) pairs they add up,
        // and the resource whose overloads they show.
        type Row = (String, u8, bool, Vec<(usize, usize)>, Option<usize>);
        let mut rows: Vec<Row> = Vec::new();
        let pairs: Vec<(usize, usize)> = u.work.keys().copied().collect();
        let res_name = |r: usize| v.resources.get(r).map_or_else(|| format!("#{r}"), |x| x.name.clone());
        if by_resource {
            for r in 0..v.resources.len() {
                let mine: Vec<(usize, usize)> = pairs.iter().filter(|p| p.1 == r).copied().collect();
                rows.push((res_name(r), 0, true, mine.clone(), Some(r)));
                for (t, _) in mine {
                    rows.push((v.names[t].clone(), 1, false, vec![(t, r)], None));
                }
            }
        } else {
            for (t, o) in v.schedule.tasks.iter().enumerate() {
                // A summary adds up the tasks under it.
                let under = |p: &(usize, usize)| {
                    let mut k = Some(p.0);
                    while let Some(x) = k {
                        if x == t {
                            return true;
                        }
                        k = v.schedule.tasks[x].parent;
                    }
                    false
                };
                let mine: Vec<(usize, usize)> = pairs.iter().filter(|p| under(p)).copied().collect();
                rows.push((v.names[t].clone(), o.level, o.summary, mine.clone(), None));
                if !o.summary {
                    for (_, r) in mine {
                        rows.push((res_name(r), o.level + 1, false, vec![(t, r)], None));
                    }
                }
            }
        }
        let (label_w, cw) = (220.0, 58.0);
        let size = vec2(label_w + cw * u.periods.len() as f32, ROW * (rows.len() + 1) as f32);
        let (rect, _) = ui.allocate_exact_size(size.max(vec2(label_w, ROW)), Sense::hover());
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let clip = ui.clip_rect();
        let head = Rect::from_min_size(rect.min, vec2(size.x, ROW));
        p.rect_filled(head, 0.0, look.header);
        for (k, &per) in u.periods.iter().enumerate() {
            let r = Rect::from_min_size(pos2(rect.left() + label_w + k as f32 * cw, rect.top()), vec2(cw, ROW));
            if r.intersects(clip) {
                let g = galley(&ctx, period_name(per), 10.5, look.head_text);
                p.with_clip_rect(r).galley(pos2(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0), g, look.head_text);
            }
        }
        let g = galley(&ctx, tr(if by_resource { "plan.usage_resource" } else { "plan.usage_task" }).to_string(), 11.5, look.head_text);
        p.galley(pos2(rect.left() + 6.0, rect.top() + 4.0), g, look.head_text);
        for (n, (label, level, bold, sum, res)) in rows.iter().enumerate() {
            let y = rect.top() + ROW * (n + 1) as f32;
            let band = Rect::from_min_size(pos2(rect.left(), y), vec2(size.x, ROW));
            if !band.intersects(clip) {
                continue;
            }
            if *bold {
                p.rect_filled(band, 0.0, look.nonwork);
            }
            p.hline(band.x_range(), band.bottom(), Stroke::new(0.5, look.grid));
            let g = galley(&ctx, label.clone(), if *bold { 12.0 } else { 11.5 }, look.text);
            p.with_clip_rect(Rect::from_min_size(band.min, vec2(label_w - 4.0, ROW))).galley(pos2(band.left() + 6.0 + *level as f32 * 12.0, band.center().y - g.size().y / 2.0), g, look.text);
            for (k, &per) in u.periods.iter().enumerate() {
                let r = Rect::from_min_size(pos2(rect.left() + label_w + k as f32 * cw, y), vec2(cw, ROW));
                if !r.intersects(clip) {
                    continue;
                }
                let h = u.sum(sum.iter().copied(), per);
                let over = res.is_some_and(|r| u.over.contains(&(r, per)));
                let color = if over { OVER } else if *bold { look.text } else { look.weak };
                let g = galley(&ctx, hours(h), 10.5, color);
                p.galley(pos2(r.right() - g.size().x - 5.0, r.center().y - g.size().y / 2.0), g, color);
            }
        }
        p.vline(rect.left() + label_w, rect.y_range(), Stroke::new(1.0, look.grid));
        if rows.is_empty() {
            ui.label(tr("plan.view_empty"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::workbook::NewTask;

    fn plan() -> (Workbook, TableId) {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::EN;
        // Mon 5 Oct 2026. A (3d), B (2d, after A), C (1d, after A), D (after B and C).
        let tasks = [NewTask::new("A", 0, 3.0, ""), NewTask::new("B", 0, 2.0, "1"), NewTask::new("C", 0, 1.0, "1"), NewTask::new("D", 0, 1.0, "2;3")];
        let id = wb.insert_plan(0, CellRef::new(0, 0), date_to_serial(2026, 10, 5), &tasks).unwrap();
        wb.set_input(0, CellRef::parse("G2").unwrap(), "Ana");
        wb.set_input(0, CellRef::parse("G3").unwrap(), "Ana");
        wb.set_input(0, CellRef::parse("G4").unwrap(), "Ana");
        (wb, id)
    }

    #[test]
    fn network_columns_follow_links() {
        let (wb, id) = plan();
        let v = wb.plan_view(id).unwrap();
        let l = network_layout(&v);
        assert_eq!(l, vec![(0, 0, 0), (1, 1, 0), (2, 1, 1), (3, 2, 0)]);
    }

    #[test]
    fn team_lanes_pack_overlaps() {
        let (wb, id) = plan();
        let v = wb.plan_view(id).unwrap();
        let lanes = team_lanes(&v);
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[0].name.as_deref(), Some("Ana"));
        // B and C overlap (both after A): two rows.
        assert_eq!(lanes[0].tasks, vec![(0, 0), (1, 0), (2, 1)]);
        assert_eq!(lanes[0].rows, 2);
        assert_eq!(lanes[1], TeamLane { name: None, tasks: vec![(3, 0)], rows: 1 });
    }

    #[test]
    fn calendar_weeks_and_bars() {
        // October 2026 starts on a Thursday and ends on a Saturday.
        let w = month_weeks(2026, 10);
        assert_eq!(w.len(), 5);
        assert_eq!(w[0], date_to_serial(2026, 9, 28) as i64);
        let (wb, id) = plan();
        let v = wb.plan_view(id).unwrap();
        let (bars, rows) = week_bars(&v, date_to_serial(2026, 10, 5) as i64);
        assert_eq!(rows, 2);
        assert!(bars.contains(&(0, 0, 2, 0)));
        assert_eq!(week_bars(&v, date_to_serial(2026, 11, 2) as i64).1, 0);
    }

    #[test]
    fn usage_by_period() {
        assert_eq!(period_start(date_to_serial(2026, 10, 8) as i64, Scale::Week), date_to_serial(2026, 10, 5) as i64);
        assert_eq!(period_start(date_to_serial(2026, 11, 20) as i64, Scale::Quarter), date_to_serial(2026, 10, 1) as i64);
        assert_eq!(next_period(date_to_serial(2026, 12, 1) as i64, Scale::Month), date_to_serial(2027, 1, 1) as i64);
        let (wb, id) = plan();
        let v = wb.plan_view(id).unwrap();
        let u = usage(&v, Scale::Week);
        // A: 3 days of 8 hours in the first week.
        let week = date_to_serial(2026, 10, 5) as i64;
        assert_eq!(u.sum([(0, 0)].into_iter(), week), 24.0);
        // Ana does B and C on the same days: overloaded that week.
        assert!(u.over.contains(&(0, week)));
        assert_eq!(hours(7.5), "7.5h");
        assert_eq!(hours(0.0), "");
    }

    /// Every view draws beside the plan.
    #[test]
    fn views_draw() {
        let (wb, _) = plan();
        let mut v = SheetView::new(wb);
        v.select(CellRef::new(2, 1), false);
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut shell = crate::common::Shell { cmd: None, show_formula_bar: true, file_name: "plan".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None };
        for kind in ViewKind::ALL {
            v.plan.views.kind = kind;
            for _ in 0..2 {
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
                let mut out = ctx.run_ui(input, |ui| v.ui(ui, &mut shell));
                out.textures_delta.clear();
            }
        }
        assert!(v.plan.views.month.is_some());
    }
}
