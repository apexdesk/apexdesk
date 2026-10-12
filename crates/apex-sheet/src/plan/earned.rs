//! Earned value at the status date: planned value (what the baseline
//! meant to have done by then), earned value (the budget of what is done)
//! and actual cost, their variances and indices, and the estimate at
//! completion. Summaries add up their subtasks.
//!
//! Without a baseline the current plan stands in for it. The actual cost
//! is the task's cost in proportion to its % complete; the curves spread
//! the progress evenly over the days worked up to the status date.

use super::calendar::WorkTime;

/// A task for the earned value (days are serial days).
#[derive(Clone, Debug, PartialEq)]
pub struct EvTask {
    /// Budget at completion: the baseline cost (or the cost).
    pub bac: f64,
    /// The planned first and last day (baseline, or scheduled).
    pub plan_start: i64,
    pub plan_finish: i64,
    /// Scheduled (or actual) first and last day.
    pub start: i64,
    pub finish: i64,
    /// 0..=1.
    pub percent: f64,
    /// Current cost.
    pub cost: f64,
    pub summary: bool,
    pub parent: Option<usize>,
}

/// Planned value, earned value and actual cost of a task.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ev {
    pub pv: f64,
    pub ev: f64,
    pub ac: f64,
    pub bac: f64,
}

impl Ev {
    /// Schedule variance.
    pub fn sv(&self) -> f64 {
        self.ev - self.pv
    }

    /// Cost variance.
    pub fn cv(&self) -> f64 {
        self.ev - self.ac
    }

    /// Schedule performance index (None before anything was planned).
    pub fn spi(&self) -> Option<f64> {
        (self.pv.abs() > 1e-9).then(|| self.ev / self.pv)
    }

    /// Cost performance index (None before anything cost).
    pub fn cpi(&self) -> Option<f64> {
        (self.ac.abs() > 1e-9).then(|| self.ev / self.ac)
    }

    /// Estimate at completion: what is spent plus the rest at the cost
    /// performance so far.
    pub fn eac(&self) -> f64 {
        match self.cpi() {
            Some(c) if c > 1e-9 => self.ac + (self.bac - self.ev) / c,
            _ => self.ac + (self.bac - self.ev),
        }
    }

    /// Variance at completion.
    pub fn vac(&self) -> f64 {
        self.bac - self.eac()
    }
}

/// Working days from `a` to `b`, both included.
fn days(w: &WorkTime, a: i64, b: i64) -> f64 {
    if b < a {
        return 0.0;
    }
    let n = (a..=b).filter(|d| w.working(*d)).count() as f64;
    // A span of days off still counts as one day of work.
    n.max(1.0)
}

/// The part of `a..=b` done by the end of `day` (0..=1).
pub fn fraction(w: &WorkTime, a: i64, b: i64, day: i64) -> f64 {
    if day < a {
        0.0
    } else if day >= b {
        1.0
    } else {
        let done = (a..=day).filter(|d| w.working(*d)).count() as f64;
        (done / days(w, a, b)).clamp(0.0, 1.0)
    }
}

/// The earned value of each task at the end of `status`.
pub fn earned(tasks: &[EvTask], w: &WorkTime, status: i64) -> Vec<Ev> {
    at(tasks, w, status, status)
}

/// The values as they were at the end of `day`, knowing the progress at
/// `status` (days after it keep the progress of the status date).
fn at(tasks: &[EvTask], w: &WorkTime, status: i64, day: i64) -> Vec<Ev> {
    let mut out = vec![Ev::default(); tasks.len()];
    for (i, t) in tasks.iter().enumerate() {
        if t.summary {
            continue;
        }
        let pv = t.bac * fraction(w, t.plan_start, t.plan_finish, day);
        // Progress spread over the days worked up to the status date.
        let done_by = if day >= status { 1.0 } else { fraction(w, t.start, t.finish.min(status), day) };
        let p = t.percent.clamp(0.0, 1.0) * done_by;
        out[i] = Ev { pv, ev: t.bac * p, ac: t.cost * p, bac: t.bac };
    }
    for i in (0..tasks.len()).rev() {
        if let Some(p) = tasks[i].parent {
            let c = out[i];
            let s = &mut out[p];
            s.pv += c.pv;
            s.ev += c.ev;
            s.ac += c.ac;
            s.bac += c.bac;
        }
    }
    out
}

/// The whole project (the top-level tasks).
pub fn total(tasks: &[EvTask], ev: &[Ev]) -> Ev {
    let mut s = Ev::default();
    for (t, e) in tasks.iter().zip(ev) {
        if t.parent.is_none() {
            s.pv += e.pv;
            s.ev += e.ev;
            s.ac += e.ac;
            s.bac += e.bac;
        }
    }
    s
}

/// The three curves of the project: (day, planned value, earned value,
/// actual cost) every `step` days from the first planned day to the last
/// (earned value and actual cost stop at the status date).
pub fn curves(tasks: &[EvTask], w: &WorkTime, status: i64, step: i64) -> Vec<(i64, f64, Option<f64>, Option<f64>)> {
    let leaves = tasks.iter().filter(|t| !t.summary);
    let first = leaves.clone().map(|t| t.plan_start.min(t.start)).min();
    let last = leaves.map(|t| t.plan_finish.max(t.finish)).max();
    let (Some(first), Some(last)) = (first, last) else { return Vec::new() };
    let step = step.max(1);
    let mut out = Vec::new();
    let mut day = first - 1;
    loop {
        let e = total(tasks, &at(tasks, w, status, day));
        let past = day <= status;
        out.push((day, e.pv, past.then_some(e.ev), past.then_some(e.ac)));
        if day >= last || out.len() > 2000 {
            break;
        }
        // The status date is always a point.
        let next = day + step;
        day = if day < status && next > status { status } else { next.min(last) };
    }
    out
}
