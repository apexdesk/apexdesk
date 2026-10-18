//! Resource load and leveling. Leveling only runs when the user asks
//! (never on its own): it puts off the tasks of lower priority until their
//! resources are free, as delays the user can see, undo or clear.

use std::collections::{BTreeMap, HashSet};

use super::calendar::{Calendar, TOL, WorkTime};
use super::engine::{Schedule, TaskIn, schedule};
use super::Constraint;

/// A resource working on a task (`units`: 1.0 = full time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Use {
    pub task: usize,
    pub res: usize,
    pub units: f64,
}

/// Per resource, the work of each day (in units: 1.0 = one full day) and
/// who does it.
pub type Load = Vec<BTreeMap<i64, Vec<(usize, f64)>>>;

/// The load of `resources` resources for a schedule.
pub fn loads(sched: &Schedule, cals: &[Calendar], task_cals: &[Vec<usize>], uses: &[Use], resources: usize) -> Load {
    let default_cal = Calendar::default();
    let base = cals.first().unwrap_or(&default_cal);
    let mut out: Load = vec![BTreeMap::new(); resources];
    for u in uses {
        let Some(t) = sched.tasks.get(u.task) else { continue };
        if t.summary || u.res >= resources || t.finish - t.start <= TOL || u.units <= 0.0 {
            continue;
        }
        let others: Vec<&Calendar> = task_cals.get(u.task).into_iter().flatten().filter_map(|&c| cals.get(c)).filter(|c| !std::ptr::eq(*c, base)).collect();
        let w = WorkTime { base, others: &others };
        for d in t.start_day..=t.finish_day {
            if !w.working(d) {
                continue;
            }
            let part = w.work(t.start.max(d as f64), t.finish.min(d as f64 + 1.0));
            if part > TOL {
                out[u.res].entry(d).or_default().push((u.task, part * u.units));
            }
        }
    }
    out
}

/// The days a resource works more than `capacity`.
pub fn overloaded(load: &BTreeMap<i64, Vec<(usize, f64)>>, capacity: f64) -> Vec<i64> {
    load.iter().filter(|(_, v)| v.iter().map(|x| x.1).sum::<f64>() > capacity + 1e-6).map(|(d, _)| *d).collect()
}

/// Leveling: the delay (working days) of each task that removes the
/// overloads it can. Tasks with a higher `priority` stay; then those with
/// less slack, the earlier ones and those listed first. Manual tasks,
/// started ones and those that must start or finish on a date never move.
pub fn level(tasks: &[TaskIn], cals: &[Calendar], start: f64, uses: &[Use], capacity: &[f64], priority: &[f64]) -> Vec<f64> {
    let mut work: Vec<TaskIn> = tasks.to_vec();
    for t in &mut work {
        t.delay = 0.0;
    }
    let task_cals: Vec<Vec<usize>> = work.iter().map(|t| t.calendars.clone()).collect();
    let fixed = |t: &TaskIn| t.manual || t.actual_start.is_some() || matches!(t.constraint, Constraint::Mso(_) | Constraint::Mfo(_));
    let prio = |i: usize| priority.get(i).copied().unwrap_or(500.0);
    let mut skip: HashSet<(usize, i64)> = HashSet::new();
    for _ in 0..5000 {
        let sched = schedule(&work, cals, start);
        let load = loads(&sched, cals, &task_cals, uses, capacity.len());
        // The first overloaded day of any resource.
        let mut first: Option<(i64, usize)> = None;
        for (r, l) in load.iter().enumerate() {
            if let Some(d) = overloaded(l, capacity[r]).into_iter().find(|d| !skip.contains(&(r, *d))) {
                if first.is_none_or(|(fd, _)| d < fd) {
                    first = Some((d, r));
                }
            }
        }
        let Some((day, r)) = first else { break };
        let mut on: Vec<(usize, f64)> = load[r][&day].clone();
        on.sort_by(|a, b| {
            let (ta, tb) = (&sched.tasks[a.0], &sched.tasks[b.0]);
            fixed(&work[b.0]).cmp(&fixed(&work[a.0]))
                .then(prio(b.0).total_cmp(&prio(a.0)))
                .then(ta.slack.total_cmp(&tb.slack))
                .then(ta.start.total_cmp(&tb.start))
                .then(a.0.cmp(&b.0))
        });
        let mut used = 0.0;
        let mut kept: Vec<usize> = Vec::new();
        let mut moved = None;
        for (t, amount) in &on {
            if used + amount <= capacity[r] + 1e-6 || kept.is_empty() || fixed(&work[*t]) {
                used += amount;
                kept.push(*t);
            } else {
                moved = Some(*t);
                break;
            }
        }
        let Some(t) = moved else {
            skip.insert((r, day));
            continue;
        };
        // Start again when the first kept task ends.
        let base = cals.first().cloned().unwrap_or_default();
        let w = WorkTime::new(&base);
        let free = kept.iter().map(|k| sched.tasks[*k].finish).fold(f64::INFINITY, f64::min);
        let s = sched.tasks[t].start;
        let mut step = if free > s + TOL { w.work(s, w.norm_start(free)) } else { 0.0 };
        if step <= TOL {
            step = 1.0;
        }
        work[t].delay += step;
    }
    work.iter().map(|t| t.delay).collect()
}
