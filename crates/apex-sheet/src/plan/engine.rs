//! The scheduler: dates of the tasks from their durations, links and
//! constraints (forward pass), late dates and slack (backward pass), the
//! critical path and the roll-up of summary tasks.
//!
//! Summary tasks are two nodes of the graph: one at their start, a bound
//! for their subtasks (what the summary's predecessors impose), and one at
//! their end, which gathers the subtasks (what the summary's successors
//! see). Links that close a cycle are left out and reported.

use super::calendar::{Calendar, END, TOL, WorkTime, split};
use super::{Constraint, DepType, Lag, Link};

/// A task as the scheduler sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskIn {
    /// Outline level (0 = top).
    pub level: u8,
    /// Working days (leaf tasks; 0 is a milestone).
    pub duration: f64,
    /// Manual tasks keep the dates they were given.
    pub manual: bool,
    /// Given dates (serial days): those of a manual task.
    pub start: Option<f64>,
    pub finish: Option<f64>,
    pub links: Vec<Link>,
    pub constraint: Constraint,
    /// 0..=1.
    pub percent: f64,
    pub actual_start: Option<f64>,
    pub actual_finish: Option<f64>,
    /// Resource calendars (indices into the calendar list, never 0) that
    /// narrow the working time.
    pub calendars: Vec<usize>,
    /// Working days the task was put off by leveling.
    pub delay: f64,
}

impl Default for TaskIn {
    fn default() -> Self {
        TaskIn { level: 0, duration: 1.0, manual: false, start: None, finish: None, links: Vec::new(), constraint: Constraint::Asap, percent: 0.0, actual_start: None, actual_finish: None, calendars: Vec::new(), delay: 0.0 }
    }
}

/// What the scheduler found for a task. Moments as in [`super::calendar`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskOut {
    pub summary: bool,
    /// Scheduled start and finish.
    pub start: f64,
    pub finish: f64,
    /// Late start and finish.
    pub late_start: f64,
    pub late_finish: f64,
    /// First and last working day.
    pub start_day: i64,
    pub finish_day: i64,
    /// Working days (a summary: from its start to its finish).
    pub duration: f64,
    /// Total and free slack, in working days.
    pub slack: f64,
    pub free_slack: f64,
    pub critical: bool,
    /// Rolled up for summaries (weighted by duration).
    pub percent: f64,
    /// Outline number: "1", "1.2", "1.2.3".
    pub wbs: String,
    /// Parent summary (task index).
    pub parent: Option<usize>,
    /// Outline level after normalization.
    pub level: u8,
}

/// Something the user should look at.
#[derive(Clone, Debug, PartialEq)]
pub enum Warning {
    /// Tasks whose links form a cycle (those links were ignored).
    Cycle(Vec<usize>),
    /// A predecessor that does not exist.
    BadLink { task: usize, pred: usize },
    /// A link between a summary and one of its subtasks, or a
    /// finish-to-finish / start-to-finish link into a summary (ignored).
    SummaryLink { task: usize, pred: usize },
    /// A link the task does not respect because of its constraint, its
    /// actual dates or because it is manual.
    LinkConflict { task: usize, pred: usize },
    /// The task finishes after its "finish no later than" / "start no
    /// later than" date... because its links push it.
    ConstraintMissed { task: usize },
}

impl Warning {
    /// The task the warning is about (the first of a cycle).
    pub fn task(&self) -> usize {
        match self {
            Warning::Cycle(v) => v.first().copied().unwrap_or(0),
            Warning::BadLink { task, .. } | Warning::SummaryLink { task, .. } | Warning::LinkConflict { task, .. } | Warning::ConstraintMissed { task } => *task,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schedule {
    pub tasks: Vec<TaskOut>,
    pub warnings: Vec<Warning>,
    /// Project start and finish (moments).
    pub start: f64,
    pub finish: f64,
}

/// A node's early and late dates.
#[derive(Clone, Copy, Debug)]
struct Node {
    es: f64,
    ef: f64,
    ls: f64,
    lf: f64,
}

/// Outline levels made valid (a task is at most one level below the one
/// above it), and each task's parent.
pub fn outline(levels: &[u8]) -> (Vec<u8>, Vec<Option<usize>>) {
    let mut out = Vec::with_capacity(levels.len());
    let mut parents = Vec::with_capacity(levels.len());
    let mut stack: Vec<usize> = Vec::new();
    for (i, &l) in levels.iter().enumerate() {
        let max = out.last().map_or(0, |p: &u8| p + 1);
        let l = l.min(max);
        while stack.len() > l as usize {
            stack.pop();
        }
        parents.push(stack.last().copied());
        stack.push(i);
        out.push(l);
    }
    (out, parents)
}

/// Outline numbers of normalized levels.
pub fn wbs(levels: &[u8]) -> Vec<String> {
    let mut counters: Vec<u32> = Vec::new();
    levels
        .iter()
        .map(|&l| {
            let l = l as usize;
            counters.truncate(l + 1);
            while counters.len() < l + 1 {
                counters.push(0);
            }
            counters[l] += 1;
            counters.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(".")
        })
        .collect()
}

pub fn schedule(tasks: &[TaskIn], calendars: &[Calendar], project_start: f64) -> Schedule {
    let n = tasks.len();
    let default_cal = Calendar::default();
    let base = calendars.first().unwrap_or(&default_cal);
    let (levels, parents) = outline(&tasks.iter().map(|t| t.level).collect::<Vec<_>>());
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in parents.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }
    let summary: Vec<bool> = children.iter().map(|c| !c.is_empty()).collect();
    let cal_refs: Vec<Vec<&Calendar>> = tasks.iter().map(|t| t.calendars.iter().filter_map(|&c| calendars.get(c)).filter(|c| !std::ptr::eq(*c, base)).collect()).collect();
    let wt = |i: usize| WorkTime { base, others: &cal_refs[i] };
    let pwt = WorkTime::new(base);
    let mut warnings = Vec::new();

    // Links that are usable: to existing tasks, not between a summary and
    // its own subtasks, no FF/SF into a summary.
    let ancestor = |a: usize, mut b: usize| {
        while let Some(p) = parents[b] {
            if p == a {
                return true;
            }
            b = p;
        }
        false
    };
    let mut links: Vec<(usize, Link)> = Vec::new();
    for (i, t) in tasks.iter().enumerate() {
        for l in &t.links {
            if l.pred >= n || l.pred == i {
                warnings.push(Warning::BadLink { task: i, pred: l.pred });
            } else if ancestor(l.pred, i) || ancestor(i, l.pred) || (summary[i] && matches!(l.kind, DepType::FF | DepType::SF)) {
                warnings.push(Warning::SummaryLink { task: i, pred: l.pred });
            } else {
                links.push((i, *l));
            }
        }
    }

    // Nodes: task i is node i (its start for a summary); a summary's end is
    // node n + i.
    let out_node = |i: usize| if summary[i] { n + i } else { i };
    let order = loop {
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); 2 * n];
        let mut indeg = vec![0usize; 2 * n];
        let mut add = |a: usize, b: usize, edges: &mut Vec<Vec<usize>>| {
            edges[a].push(b);
            indeg[b] += 1;
        };
        for &(i, l) in &links {
            add(out_node(l.pred), i, &mut edges);
        }
        for (i, p) in parents.iter().enumerate() {
            if let Some(p) = *p {
                add(p, i, &mut edges);
                add(out_node(i), n + p, &mut edges);
            }
        }
        let used = |v: usize| v < n || summary[v - n];
        let mut stack: Vec<usize> = (0..2 * n).filter(|&v| used(v) && indeg[v] == 0).collect();
        stack.reverse();
        let mut order = Vec::with_capacity(2 * n);
        while let Some(v) = stack.pop() {
            order.push(v);
            for &w in edges[v].iter().rev() {
                indeg[w] -= 1;
                if indeg[w] == 0 {
                    stack.push(w);
                }
            }
        }
        let total = (0..2 * n).filter(|&v| used(v)).count();
        if order.len() == total {
            break order;
        }
        // Nodes left: those in a cycle and those after one. Peel off the
        // ones that lead to no other node left: the rest are in cycles.
        let mut alive: Vec<bool> = (0..2 * n).map(|v| used(v) && indeg[v] > 0).collect();
        loop {
            let dead: Vec<usize> = (0..2 * n).filter(|&v| alive[v] && !edges[v].iter().any(|&w| alive[w])).collect();
            if dead.is_empty() {
                break;
            }
            for v in dead {
                alive[v] = false;
            }
        }
        let mut left = vec![false; n];
        for v in (0..2 * n).filter(|&v| alive[v]) {
            left[v % n] = true;
        }
        let before = links.len();
        links.retain(|(i, l)| !(left[*i] && left[l.pred]));
        let mut cyc: Vec<usize> = (0..n).filter(|&i| left[i]).collect();
        cyc.sort_unstable();
        warnings.push(Warning::Cycle(cyc));
        if links.len() == before {
            // Should not happen: parent edges alone make no cycle.
            break (0..2 * n).filter(|&v| used(v)).collect();
        }
    };
    let mut preds: Vec<Vec<Link>> = vec![Vec::new(); n];
    let mut succs: Vec<Vec<(usize, Link)>> = vec![Vec::new(); n];
    for &(i, l) in &links {
        preds[i].push(l);
        succs[l.pred].push((i, l));
    }

    let start0 = pwt.norm_start(project_start);
    let mut node = vec![Node { es: start0, ef: start0, ls: f64::INFINITY, lf: f64::INFINITY }; 2 * n];
    // The dates a predecessor's link asks of a task.
    let dur = |i: usize, node: &[Node]| if summary[i] { wt(i).work(node[n + i].es, node[n + i].ef) } else { tasks[i].duration };
    let lag_after = |w: &WorkTime, t: f64, lag: Lag, pred_dur: f64| match lag {
        Lag::Days(d) => w.add(t, d),
        Lag::Elapsed(d) => w.elapsed(t, d),
        Lag::Percent(p) => w.add(t, p / 100.0 * pred_dur),
    };
    let lag_before = |w: &WorkTime, t: f64, lag: Lag, pred_dur: f64| match lag {
        Lag::Days(d) => w.sub(t, d),
        Lag::Elapsed(d) => w.elapsed(t, -d),
        Lag::Percent(p) => w.sub(t, p / 100.0 * pred_dur),
    };
    // Earliest start a link allows (`d`: the task's duration).
    let link_bound = |i: usize, l: &Link, node: &[Node], d: f64| {
        let w = wt(i);
        let p = &node[out_node(l.pred)];
        let pd = dur(l.pred, node);
        match l.kind {
            DepType::FS => lag_after(&w, p.ef, l.lag, pd),
            DepType::SS => lag_after(&w, p.es, l.lag, pd),
            DepType::FF => w.sub(lag_after(&w, p.ef, l.lag, pd), d),
            DepType::SF => w.sub(lag_after(&w, p.es, l.lag, pd), d),
        }
    };

    // Forward pass.
    for &v in &order {
        if v >= n {
            // A summary's end gathers its subtasks.
            let i = v - n;
            let (mut es, mut ef) = (f64::INFINITY, f64::NEG_INFINITY);
            for &c in &children[i] {
                es = es.min(node[c].es);
                ef = ef.max(node[out_node(c)].ef);
            }
            node[v].es = es;
            node[v].ef = ef;
            continue;
        }
        let i = v;
        let t = &tasks[i];
        let w = wt(i);
        let d = if summary[i] { 0.0 } else { t.duration };
        let mut bound = start0;
        let mut from_links = f64::NEG_INFINITY;
        for l in &preds[i] {
            let b = link_bound(i, l, &node, d);
            from_links = from_links.max(b);
        }
        bound = bound.max(from_links);
        if let Some(p) = parents[i] {
            bound = bound.max(node[p].es);
        }
        if t.delay > 0.0 {
            bound = w.add(bound, t.delay);
        }
        let mut es = if d > 0.0 || summary[i] { w.norm_start(bound) } else if w.working(split(bound).0) { bound } else { w.norm_start(bound) };
        // Constraints (they win over links, as in Project).
        let start_of = |day: f64| w.norm_start(day.floor());
        let end_of = |day: f64| w.norm_end(day.floor() + END);
        match t.constraint {
            Constraint::Asap | Constraint::Alap => {}
            Constraint::Snet(day) => es = es.max(start_of(day)),
            Constraint::Fnet(day) => es = es.max(w.sub(end_of(day), d)),
            Constraint::Mso(day) => es = start_of(day),
            Constraint::Mfo(day) => es = w.sub(end_of(day), d),
            Constraint::Snlt(day) => es = es.min(start_of(day)),
            Constraint::Fnlt(day) => es = es.min(w.sub(end_of(day), d)),
        }
        let mut ef = w.add(es, d);
        if summary[i] {
            // Only a bound for the subtasks.
            node[i].es = es;
            node[i].ef = es;
            continue;
        }
        if t.manual {
            if let Some(s) = t.start {
                es = if d > 0.0 { w.norm_start(s.floor()) } else { s.floor() };
            }
            ef = match t.finish {
                Some(f) if d > 0.0 && f >= es.floor() => w.norm_end(f.floor() + END),
                _ => w.add(es, d),
            };
        }
        if let Some(a) = t.actual_start {
            es = w.norm_start(a.floor());
            ef = match t.actual_finish {
                Some(f) if f >= a => w.norm_end(f.floor() + END),
                _ => w.add(es, d),
            };
        }
        if t.manual || t.actual_start.is_some() || !matches!(t.constraint, Constraint::Asap | Constraint::Alap | Constraint::Snet(_) | Constraint::Fnet(_)) {
            for l in &preds[i] {
                if link_bound(i, l, &node, d) > es + TOL {
                    warnings.push(Warning::LinkConflict { task: i, pred: l.pred });
                }
            }
        }
        node[i].es = es;
        node[i].ef = ef;
    }
    let finish = (0..n).filter(|&i| !summary[i]).map(|i| node[i].ef).fold(start0, f64::max);

    // Backward pass.
    let d_of = |i: usize| if summary[i] { 0.0 } else { tasks[i].duration };
    for &v in order.iter().rev() {
        if v >= n {
            // A summary's end: bounded by its successors and its parent.
            let i = v - n;
            let mut lf = finish;
            for (s, l) in &succs[i] {
                lf = lf.min(bound_before(&wt(i), node[*s].ls, node[out_node(*s)].lf, l, dur(i, &node), &lag_before));
            }
            if let Some(p) = parents[i] {
                lf = lf.min(node[n + p].lf);
            }
            node[v].lf = lf;
            node[v].ls = lf;
            continue;
        }
        let i = v;
        let w = wt(i);
        if summary[i] {
            // A summary's start: its subtasks' earliest late start.
            let ls = children[i].iter().map(|&c| node[c].ls).fold(f64::INFINITY, f64::min);
            node[i].ls = ls;
            node[i].lf = node[n + i].lf;
            continue;
        }
        let d = tasks[i].duration;
        let mut lf = finish;
        for (s, l) in &succs[i] {
            lf = lf.min(bound_before(&w, node[*s].ls, node[out_node(*s)].lf, l, d, &lag_before));
        }
        if let Some(p) = parents[i] {
            lf = lf.min(node[n + p].lf);
        }
        let end_of = |day: f64| w.norm_end(day.floor() + END);
        match tasks[i].constraint {
            Constraint::Snlt(day) => lf = lf.min(w.add(w.norm_start(day.floor()), d)),
            Constraint::Fnlt(day) => lf = lf.min(end_of(day)),
            Constraint::Mso(_) | Constraint::Mfo(_) => lf = lf.min(node[i].ef),
            _ => {}
        }
        let ls = if d > 0.0 { w.sub(lf, d) } else { lf };
        node[i].lf = lf;
        node[i].ls = ls;
    }

    // As late as possible: into the free slack.
    for &v in order.iter().rev() {
        if v < n && !summary[v] && !tasks[v].manual && tasks[v].constraint == Constraint::Alap && tasks[v].actual_start.is_none() {
            let w = wt(v);
            let d = tasks[v].duration;
            let mut lf = finish;
            for (s, l) in &succs[v] {
                lf = lf.min(bound_before(&w, node[*s].es, node[out_node(*s)].ef, l, d, &lag_before));
            }
            if let Some(p) = parents[v] {
                lf = lf.min(node[n + p].lf);
            }
            if lf > node[v].ef + TOL {
                node[v].ef = lf;
                node[v].es = if d > 0.0 { w.sub(lf, d) } else { lf };
            }
        }
    }

    // Results.
    let wb = wbs(&levels);
    let mut out: Vec<TaskOut> = (0..n)
        .map(|i| {
            let w = wt(i);
            let (s, f) = (node[i].es, node[out_node(i)].ef);
            let milestone = !summary[i] && tasks[i].duration <= TOL;
            let (sd, fd) = if milestone { (split(s).0, split(s).0) } else { (w.start_day(s), w.finish_day(f).max(w.start_day(s))) };
            TaskOut {
                summary: summary[i],
                start: s,
                finish: f,
                late_start: node[i].ls,
                late_finish: node[out_node(i)].lf,
                start_day: sd,
                finish_day: fd,
                duration: if summary[i] || tasks[i].actual_finish.is_some() || (tasks[i].manual && tasks[i].finish.is_some()) { w.work(s, f) } else { tasks[i].duration },
                slack: if summary[i] { 0.0 } else { round(w.work(node[i].es, node[i].ls)) },
                free_slack: 0.0,
                critical: false,
                percent: tasks[i].percent.clamp(0.0, 1.0),
                wbs: wb[i].clone(),
                parent: parents[i],
                level: levels[i],
            }
        })
        .collect();
    // Free slack: how far a task can slip without moving a successor.
    for i in 0..n {
        if summary[i] {
            continue;
        }
        let mut fs = out[i].slack;
        for (s, l) in &succs[i] {
            let d = d_of(*s);
            let need = link_bound(*s, l, &node, d);
            fs = fs.min(round(wt(*s).work(need, node[*s].es).max(0.0)));
        }
        out[i].free_slack = fs.max(0.0);
        out[i].critical = out[i].slack <= TOL * 100.0;
    }
    // Summaries: slack, critical flag and progress of their subtasks
    // (deepest first).
    for i in (0..n).rev() {
        if !summary[i] {
            continue;
        }
        let kids = &children[i];
        out[i].slack = kids.iter().map(|&c| out[c].slack).fold(f64::INFINITY, f64::min);
        out[i].free_slack = kids.iter().map(|&c| out[c].free_slack).fold(f64::INFINITY, f64::min);
        out[i].critical = kids.iter().any(|&c| out[c].critical);
        let total: f64 = kids.iter().map(|&c| out[c].duration).sum();
        out[i].percent = if total > TOL { kids.iter().map(|&c| out[c].percent * out[c].duration).sum::<f64>() / total } else { kids.iter().map(|&c| out[c].percent).sum::<f64>() / kids.len() as f64 };
    }
    // Missed late constraints.
    for (i, t) in tasks.iter().enumerate() {
        if summary[i] {
            continue;
        }
        let w = wt(i);
        let missed = match t.constraint {
            Constraint::Snlt(day) => node[i].es > w.norm_start(day.floor()) + TOL,
            Constraint::Fnlt(day) => node[i].ef > w.norm_end(day.floor() + END) + TOL,
            _ => false,
        };
        if missed {
            warnings.push(Warning::ConstraintMissed { task: i });
        }
    }
    Schedule { tasks: out, warnings, start: start0, finish }
}

/// Latest finish a successor starting at `s_start` and finishing at
/// `s_finish` allows its predecessor (working in `w`, duration `d`).
fn bound_before(w: &WorkTime, s_start: f64, s_finish: f64, l: &Link, d: f64, lag_before: &dyn Fn(&WorkTime, f64, Lag, f64) -> f64) -> f64 {
    match l.kind {
        DepType::FS => w.norm_end(lag_before(w, s_start, l.lag, d)),
        DepType::SS => w.add(w.norm_start(lag_before(w, s_start, l.lag, d)), d),
        DepType::FF => w.norm_end(lag_before(w, s_finish, l.lag, d)),
        DepType::SF => w.add(w.norm_start(lag_before(w, s_finish, l.lag, d)), d),
    }
}

fn round(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}
