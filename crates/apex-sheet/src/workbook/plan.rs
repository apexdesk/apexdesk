//! Plan tables in the workbook (see [`crate::plan`]): the tasks read from
//! the cells, and the scheduler's dates written back after every edit, in
//! the same undo step as the edit.
//!
//! What typing means in a plan: a duration, predecessors or constraint are
//! inputs; a start typed on an automatic task becomes a "start no earlier
//! than" constraint; a finish typed on one changes its duration. Manual
//! tasks keep the dates typed. The outline is the indent of the names.

use std::sync::RwLock;

use super::{Op, TableId, Workbook};
use crate::addr::CellRef;
use crate::plan::calendar::{END, WorkTime};
use crate::plan::level::Use;
use crate::plan::parse::{self, Assignment};
use crate::plan::earned::{self, Ev, EvTask};
use crate::plan::{Constraint, Field, Plan, PlanKind, ResKind, Schedule, TaskIn, Warning, schedule};
use crate::sheet::{Cell, Content};
use crate::value::Value;

/// Column names in the interface language (the app sets them).
static NAMES: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Sets the column names new plan columns get (in `Field::ALL` order).
pub fn set_field_names(names: Vec<String>) {
    if let Ok(mut n) = NAMES.write() {
        *n = names;
    }
}

/// The name a new column of `f` gets.
pub fn field_name(f: Field) -> String {
    let i = Field::ALL.iter().position(|x| *x == f).unwrap_or(0);
    NAMES.read().ok().and_then(|n| n.get(i).filter(|s| !s.is_empty()).cloned()).unwrap_or_else(|| f.english().to_string())
}

/// The field a column header names (English or the interface language).
pub fn field_of_header(h: &str) -> Option<Field> {
    let h = h.trim().to_lowercase();
    if h.is_empty() {
        return None;
    }
    let names = NAMES.read().ok().map(|n| n.clone()).unwrap_or_default();
    for (i, f) in Field::ALL.iter().enumerate() {
        if f.english().to_lowercase() == h || names.get(i).is_some_and(|n| n.to_lowercase() == h) || f.key() == h {
            return Some(*f);
        }
    }
    // Common synonyms.
    Some(match h.as_str() {
        "name" | "task name" | "nombre" | "tarea" | "nombre de tarea" | "nom" | "tâche" | "vorgang" | "aufgabe" | "nome" | "attività" | "tarefa" => Field::Name,
        "duración" | "durée" | "dauer" | "durata" | "duração" | "days" | "días" => Field::Duration,
        "inicio" | "comienzo" | "début" | "anfang" | "beginn" | "inizio" | "início" | "begin" => Field::Start,
        "fin" | "end" | "ende" | "fine" | "término" | "fim" => Field::Finish,
        "predecesoras" | "prédécesseurs" | "vorgänger" | "predecessori" | "predecessoras" | "depends on" => Field::Predecessors,
        "recursos" | "ressources" | "ressourcen" | "risorse" | "assigned to" | "owner" | "responsable" => Field::Resources,
        "% completado" | "% complete" | "progress" | "progreso" | "avance" | "% achevé" | "% abgeschlossen" | "% completamento" | "% concluída" | "%" => Field::Percent,
        "notas" | "notes" | "notizen" | "note" => Field::Notes,
        "edt" | "wbs" | "psp" | "nº" | "no." | "#" => Field::Wbs,
        _ => return None,
    })
}

/// A resource as the plan sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resource {
    pub name: String,
    pub kind: ResKind,
    /// Maximum units (1.0 = one full-time person).
    pub capacity: f64,
    /// Its calendar (index into the plan's calendars).
    pub calendar: Option<usize>,
    /// Cost per hour of work (per unit for materials).
    pub rate: f64,
    pub per_use: f64,
}

/// A plan table read from its cells and scheduled.
#[derive(Clone, Debug)]
pub struct PlanView {
    pub id: TableId,
    pub plan: Plan,
    /// Sheet column of each field in the table.
    pub cols: Vec<(Field, u32)>,
    /// Sheet row of each task (blank rows are not tasks).
    pub rows: Vec<u32>,
    /// The id of each task: its position in the table, 1-based.
    pub ids: Vec<usize>,
    pub names: Vec<String>,
    pub input: Vec<TaskIn>,
    pub schedule: Schedule,
    /// Warnings, by task position (id - 1), not task index.
    pub warnings: Vec<Warning>,
    pub resources: Vec<Resource>,
    pub uses: Vec<Use>,
    pub assignments: Vec<Vec<Assignment>>,
    /// Hours of work and cost of each task (summaries: of their subtasks).
    pub work: Vec<f64>,
    pub cost: Vec<f64>,
    pub fixed_cost: Vec<f64>,
    pub priority: Vec<f64>,
    /// Baseline start and finish days, and cost.
    pub baseline: Vec<Option<(i64, i64)>>,
    pub baseline_cost: Vec<Option<f64>>,
    /// Predecessor items that could not be read, per task.
    pub bad_links: Vec<Vec<String>>,
    /// The status date (the plan's, else today) and the earned value of
    /// each task at its end.
    pub status: i64,
    pub ev_tasks: Vec<EvTask>,
    pub ev: Vec<Ev>,
}

impl PlanView {
    pub fn col(&self, f: Field) -> Option<u32> {
        self.cols.iter().find(|x| x.0 == f).map(|x| x.1)
    }

    /// The task shown in sheet row `row`.
    pub fn task_at_row(&self, row: u32) -> Option<usize> {
        self.rows.iter().position(|r| *r == row)
    }

    /// The task with id `id`.
    pub fn task_of_id(&self, id: usize) -> Option<usize> {
        self.ids.iter().position(|x| *x == id)
    }

    /// Cost of the whole project (the top-level tasks).
    pub fn total_cost(&self) -> f64 {
        self.schedule.tasks.iter().zip(&self.cost).filter(|(t, _)| t.parent.is_none()).map(|(_, c)| c).sum()
    }

    pub fn work_time(&self) -> WorkTime<'_> {
        WorkTime::new(self.plan.calendars.first().expect("a plan has a calendar"))
    }

    /// Earned value of the whole project.
    pub fn total_ev(&self) -> Ev {
        earned::total(&self.ev_tasks, &self.ev)
    }

    /// The curves of planned value, earned value and actual cost.
    pub fn ev_curves(&self, step: i64) -> Vec<(i64, f64, Option<f64>, Option<f64>)> {
        earned::curves(&self.ev_tasks, &self.work_time(), self.status, step)
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Bool(b) => Some(*b as u8 as f64),
        Value::Text(t) => t.trim().replace(',', ".").parse().ok(),
        _ => None,
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(t) => t.to_string(),
        Value::Number(n) if *n == n.trunc() && n.abs() < 1e15 => format!("{}", *n as i64),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => if *b { "TRUE".into() } else { "FALSE".into() },
        _ => String::new(),
    }
}

fn is_manual(s: &str) -> bool {
    let s = s.trim().to_lowercase();
    s.starts_with("man") || s.starts_with('m') && s.len() <= 2 || s == "手動" || s == "手动" || s == "수동"
}

/// Percent in 0..=1 (50 and 0.5 both mean half).
fn percent(v: &Value) -> f64 {
    let p = match v {
        Value::Text(t) => t.trim().trim_end_matches('%').trim().replace(',', ".").parse::<f64>().map(|p| p / 100.0).unwrap_or(0.0),
        v => num(v).unwrap_or(0.0),
    };
    let p = if p > 1.0 { p / 100.0 } else { p };
    p.clamp(0.0, 1.0)
}

impl Workbook {
    /// The plan tables (tasks and resources) of the workbook.
    pub fn plan_tables(&self) -> Vec<TableId> {
        let mut out = Vec::new();
        for (s, sh) in self.sheets.iter().enumerate() {
            for (i, t) in sh.tables.iter().enumerate() {
                if t.plan.is_some() {
                    out.push((s, i));
                }
            }
        }
        out
    }

    /// The task plan whose table covers cell `c` of `sheet`.
    pub fn plan_at(&self, sheet: usize, c: CellRef) -> Option<TableId> {
        let id = self.table_at(sheet, c)?;
        self.table(id)?.plan.as_ref().filter(|p| p.kind == PlanKind::Tasks).map(|_| id)
    }

    /// The task plans (not resource sheets).
    pub fn task_plans(&self) -> Vec<TableId> {
        self.plan_tables().into_iter().filter(|id| self.table(*id).and_then(|t| t.plan.as_ref()).is_some_and(|p| p.kind == PlanKind::Tasks)).collect()
    }

    fn plan_cell(&self, sheet: usize, row: u32, col: Option<u32>) -> &Value {
        static EMPTY: Value = Value::Empty;
        match col {
            Some(c) => self.sheets[sheet].value(CellRef::new(row, c)),
            None => &EMPTY,
        }
    }

    /// The resources of a task plan: its resource table's rows, then the
    /// names only its assignments give (full-time work resources).
    fn plan_resources(&self, plan: &Plan, used: &[Vec<Assignment>]) -> Vec<Resource> {
        let mut out: Vec<Resource> = Vec::new();
        if let Some(rid) = (!plan.resources.is_empty()).then(|| self.find_table(&plan.resources)).flatten()
            && let Some(t) = self.table(rid)
        {
            let col = |f: Field| t.columns.iter().position(|c| c.field == Some(f)).map(|i| t.col_at(i));
            let (d0, d1) = t.data_rows();
            for r in d0..=d1 {
                let name = text(self.plan_cell(rid.0, r, col(Field::ResName))).trim().to_string();
                if name.is_empty() || out.iter().any(|x| x.name.to_lowercase() == name.to_lowercase()) {
                    continue;
                }
                let kind = ResKind::parse(&text(self.plan_cell(rid.0, r, col(Field::ResType))));
                let cap = self.plan_cell(rid.0, r, col(Field::ResCapacity));
                let capacity = if matches!(cap, Value::Empty) { 1.0 } else { percent_or_units(cap) };
                let calendar = plan.calendar_index(&text(self.plan_cell(rid.0, r, col(Field::ResCalendar)))).filter(|i| *i > 0);
                let rate = num(self.plan_cell(rid.0, r, col(Field::ResRate))).unwrap_or(0.0);
                let per_use = num(self.plan_cell(rid.0, r, col(Field::ResPerUse))).unwrap_or(0.0);
                out.push(Resource { name, kind, capacity, calendar, rate, per_use });
            }
        }
        for list in used {
            for a in list {
                if !out.iter().any(|x| x.name.to_lowercase() == a.name.to_lowercase()) {
                    out.push(Resource { name: a.name.clone(), kind: ResKind::Work, capacity: 1.0, calendar: None, rate: 0.0, per_use: 0.0 });
                }
            }
        }
        out
    }

    /// Reads and schedules the task plan `id`. `fresh_duration`: tasks
    /// whose duration was just typed (a manual task's finish then follows
    /// it).
    pub fn plan_view(&self, id: TableId) -> Option<PlanView> {
        self.plan_view_with(id, &[])
    }

    fn plan_view_with(&self, id: TableId, fresh_duration: &[u32]) -> Option<PlanView> {
        let t = self.table(id)?;
        let plan = t.plan.as_deref()?.clone();
        if plan.kind != PlanKind::Tasks {
            return None;
        }
        let sheet = id.0;
        let cols: Vec<(Field, u32)> = t.columns.iter().enumerate().filter_map(|(i, c)| c.field.map(|f| (f, t.col_at(i)))).collect();
        let col = |f: Field| cols.iter().find(|x| x.0 == f).map(|x| x.1);
        let hpd = plan.hours_per_day();
        let (d0, d1) = t.data_rows();
        // Tasks: the rows with a name or a duration.
        let mut rows = Vec::new();
        let mut ids = Vec::new();
        for (p, r) in (d0..=d1).enumerate() {
            let name = text(self.plan_cell(sheet, r, col(Field::Name)));
            let dur = self.plan_cell(sheet, r, col(Field::Duration));
            if !name.trim().is_empty() || !matches!(dur, Value::Empty) {
                rows.push(r);
                ids.push(p + 1);
            }
        }
        let index_of_id = |id: usize| ids.iter().position(|x| *x == id);
        let n = rows.len();
        let mut names = Vec::with_capacity(n);
        let mut input = Vec::with_capacity(n);
        let mut assignments = Vec::with_capacity(n);
        let mut fixed_cost = Vec::with_capacity(n);
        let mut priority = Vec::with_capacity(n);
        let mut baseline = Vec::with_capacity(n);
        let mut baseline_cost = Vec::with_capacity(n);
        let mut bad_links = Vec::with_capacity(n);
        let mut warnings = Vec::new();
        for (k, &r) in rows.iter().enumerate() {
            let v = |f: Field| self.plan_cell(sheet, r, col(f));
            let date = |f: Field| num(v(f)).filter(|d| *d > 0.0).map(f64::floor);
            names.push(text(v(Field::Name)));
            let level = col(Field::Name).and_then(|c| self.sheets[sheet].get(CellRef::new(r, c))).map_or(0, |c| self.styles.get(c.style).indent);
            let duration = match v(Field::Duration) {
                Value::Number(d) if d.is_finite() && *d >= 0.0 => d.min(100_000.0),
                Value::Text(s) => parse::duration(s, hpd).unwrap_or(1.0),
                _ => 1.0,
            };
            let (links, bad) = parse::links(&text(v(Field::Predecessors)), hpd);
            let mut ok = Vec::new();
            for l in links {
                match index_of_id(l.pred + 1) {
                    Some(i) => ok.push(crate::plan::Link { pred: i, ..l }),
                    None => warnings.push(Warning::BadLink { task: ids[k] - 1, pred: l.pred }),
                }
            }
            bad_links.push(bad);
            let manual = is_manual(&text(v(Field::Mode)));
            let kind = Constraint::parse_kind(&text(v(Field::Constraint))).unwrap_or(0);
            let constraint = match (kind, date(Field::ConstraintDate)) {
                (0 | 1, _) => Constraint::from_index(kind, 0.0),
                (k, Some(d)) => Constraint::from_index(k, d),
                _ => Constraint::Asap,
            };
            let fresh = fresh_duration.contains(&r);
            input.push(TaskIn {
                level,
                duration,
                manual,
                start: if manual { date(Field::Start) } else { None },
                finish: if manual && !fresh { date(Field::Finish) } else { None },
                links: ok,
                constraint,
                percent: percent(v(Field::Percent)),
                actual_start: date(Field::ActualStart),
                actual_finish: date(Field::ActualFinish),
                calendars: Vec::new(),
                delay: num(v(Field::LevelDelay)).unwrap_or(0.0).max(0.0),
            });
            assignments.push(parse::assignments(&text(v(Field::Resources))));
            fixed_cost.push(num(v(Field::FixedCost)).unwrap_or(0.0));
            priority.push(num(v(Field::Priority)).unwrap_or(500.0));
            baseline.push(match (date(Field::BaselineStart), date(Field::BaselineFinish)) {
                (Some(a), Some(b)) => Some((a as i64, b as i64)),
                _ => None,
            });
            baseline_cost.push(num(v(Field::BaselineCost)));
        }
        let resources = self.plan_resources(&plan, &assignments);
        let res_of = |name: &str| resources.iter().position(|x| x.name.to_lowercase() == name.to_lowercase());
        let mut uses = Vec::new();
        for (i, list) in assignments.iter().enumerate() {
            for a in list {
                let Some(r) = res_of(&a.name) else { continue };
                let res = &resources[r];
                if res.kind == ResKind::Work {
                    uses.push(Use { task: i, res: r, units: a.units.unwrap_or(1.0) });
                    if let Some(c) = res.calendar
                        && !input[i].calendars.contains(&c)
                    {
                        input[i].calendars.push(c);
                    }
                }
            }
        }
        let mut sched = schedule(&input, &plan.calendars, plan.start);
        for w in &sched.warnings {
            warnings.push(match w {
                Warning::Cycle(v) => Warning::Cycle(v.iter().map(|i| ids[*i] - 1).collect()),
                Warning::BadLink { task, pred } => Warning::BadLink { task: ids[*task] - 1, pred: ids.get(*pred).map_or(*pred, |x| x - 1) },
                Warning::SummaryLink { task, pred } => Warning::SummaryLink { task: ids[*task] - 1, pred: ids[*pred] - 1 },
                Warning::LinkConflict { task, pred } => Warning::LinkConflict { task: ids[*task] - 1, pred: ids[*pred] - 1 },
                Warning::ConstraintMissed { task } => Warning::ConstraintMissed { task: ids[*task] - 1 },
            });
        }
        sched.warnings.clear();
        // Work and cost, then the roll-up of summaries (deepest first).
        let mut work = vec![0.0; n];
        let mut cost = vec![0.0; n];
        for i in 0..n {
            if sched.tasks[i].summary {
                continue;
            }
            let dur = sched.tasks[i].duration;
            let mut c = fixed_cost[i];
            for a in &assignments[i] {
                let Some(r) = res_of(&a.name) else { continue };
                let res = &resources[r];
                match res.kind {
                    ResKind::Work => {
                        let h = dur * hpd * a.units.unwrap_or(1.0);
                        work[i] += h;
                        c += h * res.rate + res.per_use;
                    }
                    ResKind::Material => c += a.units.unwrap_or(1.0) * res.rate + res.per_use,
                    ResKind::Cost => c += a.units.unwrap_or(0.0) + res.per_use,
                }
            }
            cost[i] = c;
        }
        for i in (0..n).rev() {
            if let Some(p) = sched.tasks[i].parent {
                work[p] += work[i];
                cost[p] += cost[i];
            }
        }
        for i in 0..n {
            if sched.tasks[i].summary {
                cost[i] += fixed_cost[i];
            }
        }
        // Earned value: the baseline (or the plan as it is) against the
        // progress at the status date.
        let status = plan.status.unwrap_or_else(|| crate::format::now_serial()).floor() as i64;
        let ev_tasks: Vec<EvTask> = (0..n)
            .map(|i| {
                let o = &sched.tasks[i];
                let (ps, pf) = baseline[i].unwrap_or((o.start_day, o.finish_day));
                let bac = if o.summary { 0.0 } else { baseline_cost[i].unwrap_or(cost[i]) };
                EvTask { bac, plan_start: ps, plan_finish: pf, start: o.start_day, finish: o.finish_day, percent: o.percent, cost: cost[i], summary: o.summary, parent: o.parent }
            })
            .collect();
        let ev = earned::earned(&ev_tasks, &WorkTime::new(&plan.calendars[0]), status);
        Some(PlanView { id, plan, cols, rows, ids, names, input, schedule: sched, warnings, resources, uses, assignments, work, cost, fixed_cost, priority, baseline, baseline_cost, bad_links, status, ev_tasks, ev })
    }

    /// Whether an op may change what a plan shows.
    pub(super) fn plan_touch(op: &Op) -> bool {
        match op {
            Op::SetCells { .. } | Op::InsertRows { .. } | Op::DeleteRows { .. } | Op::InsertCols { .. } | Op::DeleteCols { .. } | Op::InsertRowsAt { .. } | Op::DeleteRowsAt { .. } | Op::Tables { .. } => true,
            Op::Batch(v) => v.iter().any(Self::plan_touch),
            _ => false,
        }
    }

    /// Whether an op sets a plan table (a new plan has no plan before it).
    pub(super) fn op_makes_plan(op: &Op) -> bool {
        match op {
            Op::Tables { list, .. } => list.iter().any(|t| t.plan.is_some()),
            Op::Batch(v) => v.iter().any(Self::op_makes_plan),
            _ => false,
        }
    }

    fn edited_cells(op: &Op, out: &mut Vec<(usize, CellRef)>) {
        match op {
            Op::SetCells { sheet, cells } => out.extend(cells.iter().map(|(c, _)| (*sheet, *c))),
            Op::Batch(v) => v.iter().for_each(|o| Self::edited_cells(o, out)),
            _ => {}
        }
    }

    /// After an edit: turns what was typed in the plans into inputs and
    /// writes the new dates. Returns the inverse ops, last first.
    pub(super) fn plan_follow(&mut self, op: &Op) -> Vec<Op> {
        if !Self::plan_touch(op) {
            return Vec::new();
        }
        let plans = self.task_plans();
        if plans.is_empty() {
            return Vec::new();
        }
        let mut edited = Vec::new();
        Self::edited_cells(op, &mut edited);
        let mut inv = Vec::new();
        for id in plans {
            let mine: Vec<CellRef> = edited.iter().filter(|(s, _)| *s == id.0).map(|(_, c)| *c).collect();
            let mut fresh = Vec::new();
            self.plan_inputs(id, &mine, &mut fresh, &mut inv);
            if let Some(o) = self.plan_outputs(id, &fresh) {
                inv.push(self.apply(o));
            }
            if let Some(o) = self.plan_custom_outputs(id) {
                inv.push(self.apply(o));
            }
        }
        inv.reverse();
        inv
    }

    /// Start and finish typed on tasks: constraints and durations. The
    /// changes are applied; their inverses go to `inv`.
    fn plan_inputs(&mut self, id: TableId, edited: &[CellRef], fresh: &mut Vec<u32>, inv: &mut Vec<Op>) {
        if edited.is_empty() {
            return;
        }
        let Some(view) = self.plan_view(id) else { return };
        let (sc, fc, dc) = (view.col(Field::Start), view.col(Field::Finish), view.col(Field::Duration));
        let mut starts = Vec::new();
        let mut cells = Vec::new();
        let w = view.work_time();
        for c in edited {
            let Some(i) = view.task_at_row(c.row) else { continue };
            let t = &view.input[i];
            let out = &view.schedule.tasks[i];
            if Some(c.col) == dc {
                fresh.push(c.row);
            }
            if out.summary || t.actual_start.is_some() {
                continue;
            }
            let v = self.sheets[id.0].value(*c).clone();
            let Some(day) = num(&v).filter(|d| *d > 0.0).map(f64::floor) else { continue };
            if Some(c.col) == sc && !t.manual && day as i64 != out.start_day {
                starts.push((c.row, day));
            } else if Some(c.col) == fc && !t.manual && day as i64 != out.finish_day && t.duration > 0.0 {
                if day >= out.start_day as f64
                    && let Some(dc) = dc
                {
                    let d = (w.work(out.start, w.norm_end(day + END)) * 1000.0).round() / 1000.0;
                    cells.extend(self.plan_put(id.0, CellRef::new(c.row, dc), Value::Number(d), None, None));
                }
            } else if Some(c.col) == fc && t.manual {
                // A manual task's finish sets its duration.
                if let Some(s) = t.start
                    && day >= s
                    && let Some(dc) = dc
                {
                    let d = (w.work(w.norm_start(s), w.norm_end(day + END)) * 1000.0).round() / 1000.0;
                    cells.extend(self.plan_put(id.0, CellRef::new(c.row, dc), Value::Number(d), None, None));
                }
            }
        }
        if !cells.is_empty() {
            inv.push(self.apply(Op::SetCells { sheet: id.0, cells }));
        }
        if starts.is_empty() {
            return;
        }
        // Starts become constraints: the columns first.
        let (t, added) = self.plan_with_fields(id, &[Field::Constraint, Field::ConstraintDate]);
        if let Some(o) = added {
            inv.push(self.apply(o));
        }
        let kc = t.columns.iter().position(|c| c.field == Some(Field::Constraint)).map(|i| t.col_at(i));
        let dc2 = t.columns.iter().position(|c| c.field == Some(Field::ConstraintDate)).map(|i| t.col_at(i));
        if let (Some(kc), Some(dc2)) = (kc, dc2) {
            let fmt = self.locale.short_date_code();
            let mut cells = Vec::new();
            for (row, day) in starts {
                cells.extend(self.plan_put(id.0, CellRef::new(row, kc), Value::text("SNET"), None, None));
                cells.extend(self.plan_put(id.0, CellRef::new(row, dc2), Value::Number(day), Some(fmt), None));
            }
            if !cells.is_empty() {
                inv.push(self.apply(Op::SetCells { sheet: id.0, cells }));
            }
        }
    }

    /// The table `id` with columns for `fields` it lacks, and the op that
    /// adds them (at its right, header cells included). The table is
    /// returned as it will be.
    pub(crate) fn plan_with_fields(&mut self, id: TableId, fields: &[Field]) -> (crate::table::Table, Option<Op>) {
        let t = &self.sheets[id.0].tables[id.1];
        let missing: Vec<(String, Option<Field>)> = fields.iter().copied().filter(|f| !t.columns.iter().any(|c| c.field == Some(*f))).map(|f| (field_name(f), Some(f))).collect();
        self.plan_with_columns(id, &missing)
    }

    /// The table `id` with new columns at its right (name, field; names
    /// taken get a number), and the op that adds them.
    pub(crate) fn plan_with_columns(&mut self, id: TableId, missing: &[(String, Option<Field>)]) -> (crate::table::Table, Option<Op>) {
        let t = self.sheets[id.0].tables[id.1].clone();
        if missing.is_empty() {
            return (t, None);
        }
        let mut nt = t.clone();
        let mut ops = Vec::new();
        let first = t.range.end.col + 1;
        let k = missing.len() as u32;
        // Room at the right: shift what is there.
        let busy = self.sheets[id.0].tables.iter().any(|o| o.range != t.range && o.range.start.col <= first + k && o.range.end.col >= first && o.range.start.row <= t.range.end.row && o.range.end.row >= t.range.start.row)
            || (t.range.start.row..=t.range.end.row).any(|r| (first..first + k).any(|c| !matches!(self.sheets[id.0].value(CellRef::new(r, c)), Value::Empty)));
        if busy {
            ops.push(Op::InsertCols { sheet: id.0, at: first, count: k });
        }
        let mut cells = Vec::new();
        for (j, (base, f)) in missing.iter().enumerate() {
            let mut name = base.clone();
            let mut n = 2;
            while nt.column_index(&name).is_some() {
                name = format!("{base}{n}");
                n += 1;
            }
            let mut c = crate::table::TableColumn::new(name.clone());
            c.field = *f;
            nt.columns.push(c);
            if t.header {
                let at = CellRef::new(t.range.start.row, first + j as u32);
                cells.push((at, Cell::with_content(None, Content::Value(Value::text(name.as_str())))));
            }
        }
        nt.range.end.col += k;
        let mut list = self.sheets[id.0].tables.clone();
        // Other tables move right with the inserted columns.
        if busy {
            for (i, o) in list.iter_mut().enumerate() {
                if i != id.1 {
                    o.shift(false, first, k as i64);
                }
            }
        }
        list[id.1] = nt.clone();
        ops.push(Op::Tables { sheet: id.0, list });
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet: id.0, cells });
        }
        (nt, Some(Op::Batch(ops)))
    }

    /// A cell change that puts `v` (with number format `fmt` unless the
    /// cell already has one of that kind, and bold on or off) — None when
    /// the cell already shows it or holds a formula.
    pub(crate) fn plan_put(&mut self, sheet: usize, c: CellRef, v: Value, fmt: Option<&str>, bold: Option<bool>) -> Option<(CellRef, Option<Cell>)> {
        let old = self.sheets[sheet].get(c);
        if old.is_some_and(|x| x.formula().is_some()) {
            return None;
        }
        let old_style = old.map_or(0, |x| x.style);
        let mut st = self.styles.get(old_style).clone();
        if let Some(f) = fmt {
            let cur = self.styles.format(old_style);
            let want_date = crate::NumFormat::parse(f).is_date();
            let ok = if want_date { cur.is_date() } else { &*st.num_fmt == f };
            if !ok {
                st.num_fmt = f.into();
            }
        }
        if let Some(b) = bold {
            st.bold = b;
        }
        let style = if *self.styles.get(old_style) == st { old_style } else { self.styles.intern(st) };
        let content = if matches!(v, Value::Empty) { Content::Empty } else { Content::Value(v) };
        let same_value = old.map_or(matches!(content, Content::Empty), |x| match (&x.content, &content) {
            (Content::Value(Value::Number(a)), Content::Value(Value::Number(b))) => (a - b).abs() < 1e-9,
            (a, b) => a == b,
        });
        if same_value && style == old_style {
            return None;
        }
        let mut cell = Cell::with_content(old, content);
        match &mut cell {
            Some(x) => x.style = style,
            None if style != 0 => cell = Some(Cell::new(Content::Empty, style)),
            None => {}
        }
        Some((c, cell))
    }

    /// The scheduler's results written to the cells.
    fn plan_outputs(&mut self, id: TableId, fresh: &[u32]) -> Option<Op> {
        let view = self.plan_view_with(id, fresh)?;
        let sheet = id.0;
        let date_fmt = self.locale.short_date_code();
        let money = money_format(&self.locale);
        let mut cells = Vec::new();
        let w = view.work_time();
        for (i, &row) in view.rows.iter().enumerate() {
            let out = &view.schedule.tasks[i];
            let t = &view.input[i];
            let at = |f: Field| view.col(f).map(|c| CellRef::new(row, c));
            let mut put = |wb: &mut Workbook, f: Field, v: Value, fmt: Option<&str>, bold: Option<bool>| {
                if let Some(c) = at(f) {
                    cells.extend(wb.plan_put(sheet, c, v, fmt, bold));
                }
            };
            put(self, Field::Id, Value::Number(view.ids[i] as f64), None, None);
            put(self, Field::Wbs, Value::text(out.wbs.as_str()), Some("@"), None);
            // Summaries turn bold; other names keep the bold they were given
            // (outdenting takes a summary's off).
            put(self, Field::Name, Value::text(view.names[i].as_str()), None, out.summary.then_some(true));
            // Durations: numbers (typed text such as "3d" becomes one).
            let dur = if out.summary || t.actual_finish.is_some() { round(out.duration, 2) } else { round(t.duration, 3) };
            put(self, Field::Duration, Value::Number(dur), None, None);
            put(self, Field::Start, Value::Number(out.start_day as f64), Some(date_fmt), None);
            put(self, Field::Finish, Value::Number(out.finish_day as f64), Some(date_fmt), None);
            if out.summary || t.actual_finish.is_some() {
                let p = if out.summary { round(out.percent, 4) } else { 1.0 };
                put(self, Field::Percent, Value::Number(p), Some("0%"), None);
            } else if let Some(c) = at(Field::Percent) {
                // Typed as 50: 50%.
                let v = self.sheets[sheet].value(c).clone();
                if !matches!(v, Value::Empty) {
                    put(self, Field::Percent, Value::Number(t.percent), Some("0%"), None);
                }
            }
            if let Some(c) = at(Field::Predecessors) {
                // A lone id typed becomes text (a list is text anyway).
                if let Value::Number(n) = self.sheets[sheet].value(c).clone() {
                    put(self, Field::Predecessors, Value::text(text(&Value::Number(n)).as_str()), Some("@"), None);
                }
            }
            put(self, Field::Work, Value::Number(round(view.work[i], 2)), None, None);
            put(self, Field::Cost, Value::Number(round(view.cost[i], 2)), Some(&money), None);
            put(self, Field::Slack, Value::Number(round(out.slack, 2)), None, None);
            if let Some((bs, bf)) = view.baseline[i] {
                let sv = w.work(bs as f64, out.start_day as f64);
                let fv = w.work(bf as f64, out.finish_day as f64);
                put(self, Field::StartVariance, Value::Number(round(sv, 2)), None, None);
                put(self, Field::FinishVariance, Value::Number(round(fv, 2)), None, None);
            } else {
                put(self, Field::StartVariance, Value::Empty, None, None);
                put(self, Field::FinishVariance, Value::Empty, None, None);
            }
            match view.baseline_cost[i] {
                Some(b) => put(self, Field::CostVariance, Value::Number(round(view.cost[i] - b, 2)), Some(&money), None),
                None => put(self, Field::CostVariance, Value::Empty, None, None),
            }
            let e = view.ev[i];
            let m = |x: f64| Value::Number(round(x, 2));
            let index = |x: Option<f64>| x.map_or(Value::Empty, |x| Value::Number(round(x, 2)));
            put(self, Field::PlannedValue, m(e.pv), Some(&money), None);
            put(self, Field::EarnedValue, m(e.ev), Some(&money), None);
            put(self, Field::ActualCost, m(e.ac), Some(&money), None);
            put(self, Field::ScheduleVarianceEv, m(e.sv()), Some(&money), None);
            put(self, Field::CostVarianceEv, m(e.cv()), Some(&money), None);
            put(self, Field::Spi, index(e.spi()), Some("0.00"), None);
            put(self, Field::Cpi, index(e.cpi()), Some("0.00"), None);
            put(self, Field::Eac, m(e.eac()), Some(&money), None);
            put(self, Field::Vac, m(e.vac()), Some(&money), None);
        }
        // Rows that are no tasks (blank) show nothing computed.
        if let Some(t) = self.table(id) {
            let (d0, d1) = t.data_rows();
            for r in (d0..=d1).filter(|r| !view.rows.contains(r)) {
                for f in Field::ALL.into_iter().filter(|f| f.computed() || matches!(f, Field::Start | Field::Finish)) {
                    if let Some(c) = view.col(f) {
                        cells.extend(self.plan_put(sheet, CellRef::new(r, c), Value::Empty, None, None));
                    }
                }
            }
        }
        (!cells.is_empty()).then_some(Op::SetCells { sheet, cells })
    }
}

/// Percent (50%) or units (0.5, 2) of a capacity cell.
fn percent_or_units(v: &Value) -> f64 {
    match v {
        Value::Text(t) if t.trim().ends_with('%') => t.trim().trim_end_matches('%').trim().replace(',', ".").parse::<f64>().map_or(1.0, |p| p / 100.0),
        v => num(v).unwrap_or(1.0).max(0.0),
    }
}

fn round(x: f64, digits: i32) -> f64 {
    let k = 10f64.powi(digits);
    (x * k).round() / k
}

/// Number format of money in the workbook's locale.
pub(crate) fn money_format(l: &crate::Locale) -> String {
    let sym = if l.currency.is_empty() { String::new() } else { format!("\"{}\"", l.currency) };
    if l.currency_after { format!("#,##0.00 {sym}") } else { format!("{sym}#,##0.00") }
}
