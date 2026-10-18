//! Planning > Reports: a new sheet with plain cells and charts made from a
//! task plan (overview, earned value, late tasks, milestones, remaining
//! work, overloads, costs), which the user can edit and print like any
//! other. Each report is one undo step.
//!
//! The texts come from the caller (`t` maps `plan.rep.*` keys to the
//! interface language); column names are the plan's field names.

use super::plan::{field_name, money_format};
use super::{Op, PlanView, TableId, Workbook};
use crate::addr::{CellRef, Range};
use crate::chart::{Chart, ChartType, DataRef, LegendPos, Series};
use crate::plan::level::{loads, overloaded};
use crate::plan::{Field, ResKind};
use crate::sheet::{Cell, CellStyle, Content, Sheet};
use crate::value::Value;

/// The reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanReport {
    /// The project at a glance, with charts (a dashboard).
    Overview,
    /// Planned value, earned value and actual cost, and their curves.
    EarnedValue,
    LateTasks,
    Milestones,
    RemainingWork,
    Overload,
    Costs,
}

impl PlanReport {
    pub const ALL: [PlanReport; 7] = [PlanReport::Overview, PlanReport::EarnedValue, PlanReport::LateTasks, PlanReport::Milestones, PlanReport::RemainingWork, PlanReport::Overload, PlanReport::Costs];

    /// The i18n key of its name (and of the new sheet's name).
    pub fn key(self) -> &'static str {
        match self {
            PlanReport::Overview => "plan.rep.overview",
            PlanReport::EarnedValue => "plan.rep.earned",
            PlanReport::LateTasks => "plan.rep.late",
            PlanReport::Milestones => "plan.rep.milestones",
            PlanReport::RemainingWork => "plan.rep.remaining",
            PlanReport::Overload => "plan.rep.overload",
            PlanReport::Costs => "plan.rep.costs",
        }
    }
}

/// Cell styles of a report.
struct Styles {
    title: u32,
    head: u32,
    label: u32,
    date: u32,
    money: u32,
    pct: u32,
    num: u32,
    index: u32,
}

/// A report sheet being written.
struct Rep {
    sh: Sheet,
    st: Styles,
    row: u32,
    charts: Vec<Chart>,
}

impl Rep {
    fn put(&mut self, row: u32, col: u32, v: Value, style: u32) {
        let content = if matches!(v, Value::Empty) { Content::Empty } else { Content::Value(v) };
        self.sh.put(CellRef::new(row, col), Some(Cell::new(content, style)));
    }

    fn text(&mut self, row: u32, col: u32, s: &str, style: u32) {
        self.put(row, col, Value::text(s), style);
    }

    /// A row of column names, then `rows` below it; returns the range of
    /// the whole block.
    fn block(&mut self, heads: &[String], rows: &[Vec<(Value, u32)>]) -> Range {
        let r0 = self.row;
        for (c, h) in heads.iter().enumerate() {
            self.text(r0, c as u32, h, self.st.head);
        }
        for (k, r) in rows.iter().enumerate() {
            for (c, (v, s)) in r.iter().enumerate() {
                self.put(r0 + 1 + k as u32, c as u32, v.clone(), *s);
            }
        }
        self.row = r0 + 2 + rows.len() as u32;
        Range::new(CellRef::new(r0, 0), CellRef::new(r0 + rows.len().max(1) as u32, heads.len().saturating_sub(1) as u32))
    }

    /// Label and value pairs, one a row.
    fn pairs(&mut self, list: &[(String, Value, u32)]) {
        for (l, v, s) in list {
            let r = self.row;
            self.text(r, 0, l, self.st.label);
            self.put(r, 1, v.clone(), *s);
            self.row += 1;
        }
        self.row += 1;
    }

    /// A chart of `values` columns (by `categories`) of a block, to the
    /// right of the data at `top`.
    #[allow(clippy::too_many_arguments)]
    fn chart(&mut self, kind: ChartType, title: &str, block: Range, cat_col: u32, value_cols: &[u32], top: u32, left: u32) {
        let name = self.sh.name.clone();
        let (r0, r1) = (block.start.row, block.end.row);
        if r1 <= r0 {
            return;
        }
        let mut c = Chart::new(kind, CellRef::new(top, left), 480.0, 280.0);
        c.title = title.to_string();
        c.show_title = true;
        c.legend = if value_cols.len() > 1 { LegendPos::Bottom } else { LegendPos::None };
        for &vc in value_cols {
            c.series.push(Series {
                name: DataRef::range(&name, Range::new(CellRef::new(r0, vc), CellRef::new(r0, vc))),
                categories: DataRef::range(&name, Range::new(CellRef::new(r0 + 1, cat_col), CellRef::new(r1, cat_col))),
                values: DataRef::range(&name, Range::new(CellRef::new(r0 + 1, vc), CellRef::new(r1, vc))),
                ..Default::default()
            });
        }
        c.id = self.charts.len() as u32 + 1;
        c.name = format!("Chart {}", c.id);
        c.sanitize();
        self.charts.push(c);
    }
}

fn num(x: f64) -> Value {
    Value::Number((x * 100.0).round() / 100.0)
}

fn day(d: i64) -> Value {
    Value::Number(d as f64)
}

impl PlanView {
    /// The daily load of each resource (see [`crate::plan::level::loads`]).
    pub fn loads(&self) -> crate::plan::level::Load {
        let task_cals: Vec<Vec<usize>> = self.input.iter().map(|t| t.calendars.clone()).collect();
        loads(&self.schedule, &self.plan.calendars, &task_cals, &self.uses, self.resources.len())
    }

    /// Hours of work of each resource on each task (task, resource, hours).
    pub fn assignment_work(&self) -> Vec<(usize, usize, f64)> {
        let hpd = self.plan.hours_per_day();
        self.uses.iter().filter(|u| !self.schedule.tasks[u.task].summary).map(|u| (u.task, u.res, self.schedule.tasks[u.task].duration * hpd * u.units)).collect()
    }

    /// Leaf tasks that are late at the status date: finishing after their
    /// baseline, or due by then and not done.
    pub fn late_tasks(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&i| {
                let o = &self.schedule.tasks[i];
                !o.summary && o.percent < 1.0 && (self.baseline[i].is_some_and(|b| o.finish_day > b.1) || o.finish_day <= self.status)
            })
            .collect()
    }

    /// Milestones from the status date on, by date.
    pub fn next_milestones(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.rows.len()).filter(|&i| !self.schedule.tasks[i].summary && self.input[i].duration <= 0.0 && self.schedule.tasks[i].finish_day >= self.status).collect();
        v.sort_by_key(|&i| self.schedule.tasks[i].finish_day);
        v
    }
}

impl Workbook {
    /// Writes report `kind` of plan `id` on a new sheet after the plan's.
    /// Returns the sheet's index.
    pub fn plan_report(&mut self, id: TableId, kind: PlanReport, t: &dyn Fn(&str) -> String) -> Option<usize> {
        let v = self.plan_view(id)?;
        let base = t(kind.key());
        let name = if self.sheet_index(&base).is_none() { base.clone() } else { self.unique_sheet_name(&format!("{base} ")) };
        let index = self.sheets.len();
        let bold = |size: u16| CellStyle { bold: true, font_size: size, ..Default::default() };
        let date_fmt = self.locale.short_date_code().to_string();
        let money = money_format(&self.locale);
        let st = Styles {
            title: self.styles.intern(CellStyle { color: Some(0x1F3E7A), ..bold(160) }),
            head: self.styles.intern(CellStyle { fill: Some(0xDCE6F2), ..bold(0) }),
            label: self.styles.intern(bold(0)),
            date: self.styles.intern(CellStyle { num_fmt: date_fmt.into(), ..Default::default() }),
            money: self.styles.intern(CellStyle { num_fmt: money.into(), ..Default::default() }),
            pct: self.styles.intern(CellStyle { num_fmt: "0%".into(), ..Default::default() }),
            num: self.styles.intern(CellStyle { num_fmt: "0.##".into(), ..Default::default() }),
            index: self.styles.intern(CellStyle { num_fmt: "0.00".into(), ..Default::default() }),
        };
        let mut r = Rep { sh: Sheet::new(name), st, row: 0, charts: Vec::new() };
        let plan_name = self.table(id).map(|t| t.name.clone()).unwrap_or_default();
        r.text(0, 0, &format!("{} – {}", base, plan_name), r.st.title);
        r.text(1, 0, &t("plan.rep.status"), r.st.label);
        r.put(1, 1, day(v.status), r.st.date);
        r.row = 3;
        match kind {
            PlanReport::Overview => overview(&mut r, &v, t),
            PlanReport::EarnedValue => earned(&mut r, &v, t),
            PlanReport::LateTasks => late(&mut r, &v, t),
            PlanReport::Milestones => milestones(&mut r, &v, t),
            PlanReport::RemainingWork => remaining(&mut r, &v, t),
            PlanReport::Overload => overload(&mut r, &v, t),
            PlanReport::Costs => costs(&mut r, &v, t),
        }
        r.sh.col_widths.insert(0, 190.0);
        for c in 1..8 {
            r.sh.col_widths.insert(c, 96.0);
        }
        let mut sh = r.sh;
        sh.charts = r.charts;
        let at = (id.0 + 1).min(index);
        self.execute_as(Op::AddSheet { index: at, sheet: Box::new(sh) }, "undo.plan_report");
        Some(at)
    }
}

fn overview(r: &mut Rep, v: &PlanView, t: &dyn Fn(&str) -> String) {
    let s = &r.st;
    let (m, d, p, n, i) = (s.money, s.date, s.pct, s.num, s.index);
    let tops: Vec<usize> = (0..v.rows.len()).filter(|&k| v.schedule.tasks[k].parent.is_none()).collect();
    let (first, last) = tops.iter().fold((i64::MAX, i64::MIN), |(a, b), &k| (a.min(v.schedule.tasks[k].start_day), b.max(v.schedule.tasks[k].finish_day)));
    let (first, last) = if tops.is_empty() { (v.plan.start as i64, v.plan.start as i64) } else { (first, last) };
    let w = v.work_time();
    let dur = w.work(first as f64, last as f64 + 1.0);
    let weight: f64 = tops.iter().map(|&k| v.schedule.tasks[k].duration.max(0.0)).sum();
    let done = if weight > 0.0 { tops.iter().map(|&k| v.schedule.tasks[k].duration.max(0.0) * v.schedule.tasks[k].percent).sum::<f64>() / weight } else { 0.0 };
    let leaves = (0..v.rows.len()).filter(|&k| !v.schedule.tasks[k].summary).count();
    let miles = (0..v.rows.len()).filter(|&k| !v.schedule.tasks[k].summary && v.input[k].duration <= 0.0).count();
    let e = v.total_ev();
    let base: Option<f64> = v.baseline_cost.iter().zip(&v.schedule.tasks).filter(|(_, o)| o.parent.is_none()).map(|(b, _)| *b).sum();
    let mut list = vec![
        (field_name(Field::Start), day(first), d),
        (field_name(Field::Finish), day(last), d),
        (field_name(Field::Duration), num(dur), n),
        (field_name(Field::Percent), num(done), p),
        (t("plan.rep.tasks"), Value::Number(leaves as f64), n),
        (t("plan.rep.milestones_n"), Value::Number(miles as f64), n),
        (t("plan.rep.late_n"), Value::Number(v.late_tasks().len() as f64), n),
        (field_name(Field::Work), num(v.work.iter().zip(&v.schedule.tasks).filter(|(_, o)| o.parent.is_none()).map(|(w, _)| w).sum()), n),
        (field_name(Field::Cost), num(v.total_cost()), m),
    ];
    if let Some(b) = base {
        list.push((field_name(Field::BaselineCost), num(b), m));
    }
    list.extend([(field_name(Field::PlannedValue), num(e.pv), m), (field_name(Field::EarnedValue), num(e.ev), m), (field_name(Field::ActualCost), num(e.ac), m)]);
    list.push((field_name(Field::Spi), e.spi().map_or(Value::Empty, num), i));
    list.push((field_name(Field::Cpi), e.cpi().map_or(Value::Empty, num), i));
    list.push((field_name(Field::Eac), num(e.eac()), m));
    r.pairs(&list);
    // The top-level tasks: % complete and cost.
    let heads = [field_name(Field::Name), field_name(Field::Percent), field_name(Field::Cost), field_name(Field::Finish)];
    let rows: Vec<Vec<(Value, u32)>> = tops.iter().map(|&k| vec![(Value::text(v.names[k].as_str()), 0), (num(v.schedule.tasks[k].percent), p), (num(v.cost[k]), m), (day(v.schedule.tasks[k].finish_day), d)]).collect();
    let b = r.block(&heads, &rows);
    r.chart(ChartType::Bar, &field_name(Field::Percent), b, 0, &[1], 3, 5);
    r.chart(ChartType::Pie, &field_name(Field::Cost), b, 0, &[2], 3 + 15, 5);
}

fn earned(r: &mut Rep, v: &PlanView, t: &dyn Fn(&str) -> String) {
    let s = &r.st;
    let (m, d, i) = (s.money, s.date, s.index);
    let e = v.total_ev();
    r.pairs(&[
        (t("plan.rep.bac"), num(e.bac), m),
        (field_name(Field::PlannedValue), num(e.pv), m),
        (field_name(Field::EarnedValue), num(e.ev), m),
        (field_name(Field::ActualCost), num(e.ac), m),
        (field_name(Field::ScheduleVarianceEv), num(e.sv()), m),
        (field_name(Field::CostVarianceEv), num(e.cv()), m),
        (field_name(Field::Spi), e.spi().map_or(Value::Empty, num), i),
        (field_name(Field::Cpi), e.cpi().map_or(Value::Empty, num), i),
        (field_name(Field::Eac), num(e.eac()), m),
        (field_name(Field::Vac), num(e.vac()), m),
    ]);
    // Weekly points (daily for short plans).
    let span = v.ev_tasks.iter().filter(|x| !x.summary).map(|x| x.plan_finish.max(x.finish) - x.plan_start.min(x.start)).max().unwrap_or(0);
    let step = if span > 120 { 7 } else if span > 40 { 2 } else { 1 };
    let curves = v.ev_curves(step);
    let heads = [t("plan.rep.date"), field_name(Field::PlannedValue), field_name(Field::EarnedValue), field_name(Field::ActualCost)];
    let opt = |x: Option<f64>| x.map_or((Value::Empty, m), |x| (num(x), m));
    let rows: Vec<Vec<(Value, u32)>> = curves.iter().map(|c| vec![(day(c.0), d), (num(c.1), m), opt(c.2), opt(c.3)]).collect();
    let b = r.block(&heads, &rows);
    r.chart(ChartType::Line, &t("plan.rep.earned"), b, 0, &[1, 2, 3], 3, 5);
}

fn task_row(v: &PlanView, k: usize, s: &Styles) -> Vec<(Value, u32)> {
    let o = &v.schedule.tasks[k];
    vec![(Value::text(o.wbs.as_str()), 0), (Value::text(v.names[k].as_str()), 0), (day(o.start_day), s.date), (day(o.finish_day), s.date)]
}

fn late(r: &mut Rep, v: &PlanView, t: &dyn Fn(&str) -> String) {
    let w = v.work_time();
    let heads = [field_name(Field::Wbs), field_name(Field::Name), field_name(Field::Start), field_name(Field::Finish), field_name(Field::BaselineFinish), t("plan.rep.days_late"), field_name(Field::Percent), field_name(Field::Resources)];
    let rows: Vec<Vec<(Value, u32)>> = v
        .late_tasks()
        .into_iter()
        .map(|k| {
            let o = &v.schedule.tasks[k];
            let mut row = task_row(v, k, &r.st);
            let (bf, late) = match v.baseline[k] {
                Some((_, bf)) => (day(bf), w.work(bf as f64 + 1.0, o.finish_day as f64 + 1.0).max(0.0)),
                None => (Value::Empty, w.work(o.finish_day as f64 + 1.0, v.status as f64 + 1.0).max(0.0)),
            };
            let names: Vec<&str> = v.assignments[k].iter().map(|a| a.name.as_str()).collect();
            row.extend([(bf, r.st.date), (num(late), r.st.num), (num(o.percent), r.st.pct), (Value::text(names.join(", ").as_str()), 0)]);
            row
        })
        .collect();
    if rows.is_empty() {
        let row = r.row;
        r.text(row, 0, &t("plan.rep.none_late"), 0);
        return;
    }
    let b = r.block(&heads, &rows);
    r.chart(ChartType::Bar, &t("plan.rep.days_late"), b, 1, &[5], 3, 9);
}

fn milestones(r: &mut Rep, v: &PlanView, t: &dyn Fn(&str) -> String) {
    let heads = [field_name(Field::Wbs), field_name(Field::Name), field_name(Field::Finish), t("plan.rep.days_to_go"), t("plan.critical_task")];
    let rows: Vec<Vec<(Value, u32)>> = v
        .next_milestones()
        .into_iter()
        .map(|k| {
            let o = &v.schedule.tasks[k];
            let crit = if o.critical { t("plan.rep.yes") } else { String::new() };
            vec![(Value::text(o.wbs.as_str()), 0), (Value::text(v.names[k].as_str()), 0), (day(o.finish_day), r.st.date), (Value::Number((o.finish_day - v.status) as f64), r.st.num), (Value::text(crit.as_str()), 0)]
        })
        .collect();
    if rows.is_empty() {
        let row = r.row;
        r.text(row, 0, &t("plan.rep.none_milestones"), 0);
        return;
    }
    r.block(&heads, &rows);
}

fn remaining(r: &mut Rep, v: &PlanView, t: &dyn Fn(&str) -> String) {
    // By resource.
    let mut per: Vec<(f64, f64, f64)> = vec![(0.0, 0.0, 0.0); v.resources.len()];
    for (task, res, h) in v.assignment_work() {
        let left = 1.0 - v.schedule.tasks[task].percent.clamp(0.0, 1.0);
        per[res].0 += h;
        per[res].1 += h * left;
        per[res].2 += h * left * v.resources[res].rate;
    }
    let heads = [field_name(Field::ResName), field_name(Field::Work), t("plan.rep.remaining_work"), t("plan.rep.remaining_cost")];
    let rows: Vec<Vec<(Value, u32)>> = v.resources.iter().zip(&per).filter(|(x, _)| x.kind == ResKind::Work).map(|(x, p)| vec![(Value::text(x.name.as_str()), 0), (num(p.0), r.st.num), (num(p.1), r.st.num), (num(p.2), r.st.money)]).collect();
    if rows.is_empty() {
        let row = r.row;
        r.text(row, 0, &t("plan.rep.no_resources"), 0);
    } else {
        let b = r.block(&heads, &rows);
        r.chart(ChartType::Column, &t("plan.rep.remaining_work"), b, 0, &[1, 2], 3, 5);
    }
    // By task.
    let heads = [field_name(Field::Wbs), field_name(Field::Name), field_name(Field::Work), t("plan.rep.remaining_work"), field_name(Field::Percent)];
    let rows: Vec<Vec<(Value, u32)>> = (0..v.rows.len())
        .filter(|&k| !v.schedule.tasks[k].summary && v.schedule.tasks[k].percent < 1.0 && v.work[k] > 0.0)
        .map(|k| {
            let o = &v.schedule.tasks[k];
            vec![(Value::text(o.wbs.as_str()), 0), (Value::text(v.names[k].as_str()), 0), (num(v.work[k]), r.st.num), (num(v.work[k] * (1.0 - o.percent)), r.st.num), (num(o.percent), r.st.pct)]
        })
        .collect();
    if !rows.is_empty() {
        r.block(&heads, &rows);
    }
}

fn overload(r: &mut Rep, v: &PlanView, t: &dyn Fn(&str) -> String) {
    let load = v.loads();
    let heads = [field_name(Field::ResName), field_name(Field::ResCapacity), t("plan.rep.peak"), t("plan.rep.over_days"), t("plan.rep.first_over")];
    let mut rows = Vec::new();
    for (ri, res) in v.resources.iter().enumerate() {
        if res.kind != ResKind::Work {
            continue;
        }
        let peak = load[ri].values().map(|l| l.iter().map(|x| x.1).sum::<f64>()).fold(0.0, f64::max);
        let over = overloaded(&load[ri], res.capacity);
        let first = over.first().map_or(Value::Empty, |d| day(*d));
        rows.push(vec![(Value::text(res.name.as_str()), 0), (num(res.capacity), r.st.pct), (num(peak), r.st.pct), (Value::Number(over.len() as f64), r.st.num), (first, r.st.date)]);
    }
    if rows.is_empty() {
        let row = r.row;
        r.text(row, 0, &t("plan.rep.no_resources"), 0);
        return;
    }
    let b = r.block(&heads, &rows);
    r.chart(ChartType::Column, &t("plan.rep.over_days"), b, 0, &[3], 3, 6);
    // The overloaded days, with who works on them.
    let heads = [field_name(Field::ResName), t("plan.rep.date"), t("plan.rep.load"), field_name(Field::Name)];
    let mut rows = Vec::new();
    for (ri, res) in v.resources.iter().enumerate() {
        if res.kind != ResKind::Work {
            continue;
        }
        for d in overloaded(&load[ri], res.capacity) {
            let list = &load[ri][&d];
            let units: f64 = list.iter().map(|x| x.1).sum();
            let names: Vec<&str> = list.iter().map(|x| v.names[x.0].as_str()).collect();
            rows.push(vec![(Value::text(res.name.as_str()), 0), (day(d), r.st.date), (num(units), r.st.pct), (Value::text(names.join(", ").as_str()), 0)]);
        }
    }
    if !rows.is_empty() {
        r.block(&heads, &rows);
    }
}

fn costs(r: &mut Rep, v: &PlanView, t: &dyn Fn(&str) -> String) {
    let m = r.st.money;
    let e = v.total_ev();
    r.pairs(&[(field_name(Field::Cost), num(v.total_cost()), m), (t("plan.rep.bac"), num(e.bac), m), (field_name(Field::ActualCost), num(e.ac), m), (t("plan.rep.remaining_cost"), num(v.total_cost() - e.ac), m)]);
    let heads = [field_name(Field::Wbs), field_name(Field::Name), field_name(Field::Cost), field_name(Field::BaselineCost), field_name(Field::CostVariance), field_name(Field::ActualCost), t("plan.rep.remaining_cost")];
    let rows: Vec<Vec<(Value, u32)>> = (0..v.rows.len())
        .filter(|&k| v.schedule.tasks[k].level <= 1)
        .map(|k| {
            let o = &v.schedule.tasks[k];
            let b = v.baseline_cost[k];
            let (bv, var) = match b {
                Some(b) => (num(b), num(v.cost[k] - b)),
                None => (Value::Empty, Value::Empty),
            };
            let name = if o.level > 0 { format!("   {}", v.names[k]) } else { v.names[k].clone() };
            vec![(Value::text(o.wbs.as_str()), 0), (Value::text(name.as_str()), 0), (num(v.cost[k]), m), (bv, m), (var, m), (num(v.ev[k].ac), m), (num(v.cost[k] - v.ev[k].ac), m)]
        })
        .collect();
    let b = r.block(&heads, &rows);
    let cols: &[u32] = if v.baseline_cost.iter().any(|b| b.is_some()) { &[2, 3] } else { &[2] };
    r.chart(ChartType::Column, &field_name(Field::Cost), b, 1, cols, 3, 8);
}
