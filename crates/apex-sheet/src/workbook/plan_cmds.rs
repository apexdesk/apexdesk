//! Planning commands: insert a plan or turn a table into one, outline,
//! links, task mode, moving tasks (the Gantt), inserting and deleting
//! tasks (links renumbered), baseline, leveling and the resource sheet.
//! Each is one undo step; the dates follow through `plan_follow`.

use super::plan::{field_name, field_of_header, money_format};
use super::{Op, TableError, TableId, Workbook};
use crate::addr::{CellRef, Range};
use crate::plan::calendar::END;
use crate::plan::level::level;
use crate::plan::parse;
use crate::plan::{DepType, Field, Lag, Link, Plan, PlanKind};
use crate::sheet::{Cell, Content};
use crate::table::{Table, TableColumn};
use crate::value::Value;

/// A task of a new plan (templates).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewTask {
    pub name: String,
    pub level: u8,
    /// Working days (0: a milestone).
    pub duration: f64,
    /// Predecessor ids as typed ("2;3SS").
    pub predecessors: String,
    pub resources: String,
}

impl NewTask {
    pub fn new(name: impl Into<String>, level: u8, duration: f64, predecessors: &str) -> NewTask {
        NewTask { name: name.into(), level, duration, predecessors: predecessors.into(), resources: String::new() }
    }
}

impl Workbook {
    fn plan_table(&self, id: TableId) -> Option<&Table> {
        self.table(id).filter(|t| t.plan.as_ref().is_some_and(|p| p.kind == PlanKind::Tasks))
    }

    /// Adds the columns of `fields` that plan `id` lacks (an undo step of
    /// its own, for the caller to merge) and returns the table then.
    fn plan_ensure(&mut self, id: TableId, fields: &[Field], label: &'static str) -> Option<Table> {
        let (_, add) = self.plan_with_fields(id, fields);
        if let Some(o) = add {
            self.execute_as(o, label);
        }
        self.plan_table(id).cloned()
    }

    fn field_col(t: &Table, f: Field) -> Option<u32> {
        t.columns.iter().position(|c| c.field == Some(f)).map(|i| t.col_at(i))
    }

    fn check_plan_area(&self, sheet: usize, r: &Range) -> Result<(), TableError> {
        let sh = self.sheets.get(sheet).ok_or(TableError::BadRange)?;
        if sh.tables.iter().any(|t| t.range.intersect(r).is_some()) {
            return Err(TableError::Overlap);
        }
        if sh.merges.iter().any(|m| m.intersect(r).is_some()) {
            return Err(TableError::Merged);
        }
        Ok(())
    }

    /// Inserts a task plan at `at` (the header row), with `tasks` (a blank
    /// row when there are none), starting on `start` (a serial day). One
    /// undo step.
    pub fn insert_plan(&mut self, sheet: usize, at: CellRef, start: f64, tasks: &[NewTask]) -> Result<TableId, TableError> {
        let fields = Field::TASKS;
        let rows = tasks.len().max(1) as u32;
        let range = Range::new(at, CellRef::new(at.row + rows, at.col + fields.len() as u32 - 1));
        if range.end.row >= crate::MAX_ROWS || range.end.col >= crate::MAX_COLS {
            return Err(TableError::BadRange);
        }
        self.check_plan_area(sheet, &range)?;
        let names: Vec<String> = fields.iter().map(|f| field_name(*f)).collect();
        let names = Self::unique_names(names);
        let mut cells = Vec::new();
        let head = self.styles.intern(crate::CellStyle { bold: true, ..Default::default() });
        for (i, n) in names.iter().enumerate() {
            cells.push((CellRef::new(at.row, at.col + i as u32), Some(Cell::new(Content::Value(Value::text(n.as_str())), head))));
        }
        let col = |f: Field| at.col + fields.iter().position(|x| *x == f).unwrap_or(0) as u32;
        for (k, t) in tasks.iter().enumerate() {
            let r = at.row + 1 + k as u32;
            let style = if t.level > 0 { self.styles.intern(crate::CellStyle { indent: t.level, ..Default::default() }) } else { 0 };
            cells.push((CellRef::new(r, col(Field::Name)), Some(Cell::new(Content::Value(Value::text(t.name.as_str())), style))));
            cells.push((CellRef::new(r, col(Field::Duration)), Some(Cell::new(Content::Value(Value::Number(t.duration)), 0))));
            if !t.predecessors.is_empty() {
                cells.push((CellRef::new(r, col(Field::Predecessors)), Some(Cell::new(Content::Value(Value::text(t.predecessors.as_str())), 0))));
            }
            if !t.resources.is_empty() {
                cells.push((CellRef::new(r, col(Field::Resources)), Some(Cell::new(Content::Value(Value::text(t.resources.as_str())), 0))));
            }
        }
        let name = self.unique_table_name("Plan");
        let mut t = Table::new(self.next_table_id(), name, range, names);
        for (c, f) in t.columns.iter_mut().zip(fields) {
            c.field = Some(f);
        }
        t.filter_button = false;
        t.plan = Some(Box::new(Plan::tasks(start.floor())));
        let mut list = self.sheets[sheet].tables.clone();
        list.push(t);
        let widths: Vec<(u32, Option<f32>)> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let w = match f {
                    Field::Name => 30.0,
                    Field::Start | Field::Finish => 11.0,
                    Field::Predecessors | Field::Resources => 14.0,
                    Field::Notes => 24.0,
                    _ => 9.0,
                };
                (at.col + i as u32, Some(w * 7.0 + 12.0))
            })
            .collect();
        self.execute_as(Op::Batch(vec![Op::SetCells { sheet, cells }, Op::Tables { sheet, list }, Op::ColWidths { sheet, sizes: widths }]), "undo.plan_insert");
        Ok((sheet, self.sheets[sheet].tables.len() - 1))
    }

    /// Makes table `id` a task plan: its columns get roles by their names,
    /// and the ones a plan needs are added. One undo step.
    pub fn convert_to_plan(&mut self, id: TableId, start: f64) -> Result<(), TableError> {
        let t = self.table(id).ok_or(TableError::NotFound)?.clone();
        if t.plan.is_some() {
            return Ok(());
        }
        let mut nt = t.clone();
        let mut seen = Vec::new();
        for c in &mut nt.columns {
            let f = field_of_header(&c.name).filter(|f| !f.is_resource() && !seen.contains(f));
            if let Some(f) = f {
                seen.push(f);
            }
            c.field = f;
        }
        // The first text column names the tasks when no header says so.
        if !seen.contains(&Field::Name)
            && let Some(c) = nt.columns.iter_mut().find(|c| c.field.is_none())
        {
            c.field = Some(Field::Name);
        }
        nt.plan = Some(Box::new(Plan::tasks(start.floor())));
        nt.filter_button = false;
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1] = nt;
        let before = self.undo.len();
        let mut ops = vec![Op::Tables { sheet: id.0, list }];
        if self.sheets[id.0].filter.as_ref().is_some_and(|f| f.range == t.filter_range()) {
            ops.push(Op::Filter { sheet: id.0, filter: None });
        }
        self.execute_as(Op::Batch(ops), "undo.plan_insert");
        self.plan_ensure(id, &[Field::Duration, Field::Start, Field::Finish, Field::Predecessors], "undo.plan_insert");
        let n = self.undo.len() - before;
        self.merge_undo(n, "undo.plan_insert");
        Ok(())
    }

    /// Replaces a plan's settings (start, calendars, resource sheet).
    pub fn set_plan(&mut self, id: TableId, plan: Plan) {
        let Some(t) = self.table(id) else { return };
        if t.plan.as_deref() == Some(&plan) {
            return;
        }
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1].plan = Some(Box::new(plan));
        self.execute_as(Op::Tables { sheet: id.0, list }, "undo.plan_settings");
    }

    /// Indents (delta 1) or outdents (-1) the tasks in `rows`.
    pub fn plan_indent(&mut self, id: TableId, rows: &[u32], delta: i32) {
        let Some(t) = self.plan_table(id) else { return };
        let Some(nc) = Self::field_col(t, Field::Name) else { return };
        let (d0, d1) = t.data_rows();
        let level = |wb: &Self, r: u32| wb.sheets[id.0].get(CellRef::new(r, nc)).map_or(0, |c| wb.styles.get(c.style).indent);
        let before: Vec<u8> = (d0..=d1).map(|r| level(self, r)).collect();
        let mut after = before.clone();
        for &r in rows.iter().filter(|r| (d0..=d1).contains(*r)) {
            let i = (r - d0) as usize;
            after[i] = (after[i] as i32 + delta).clamp(0, 15) as u8;
        }
        // A task with deeper ones right below is a summary.
        let summary = |lv: &[u8], i: usize| lv.get(i + 1).is_some_and(|n| *n > lv[i]);
        let mut cells = Vec::new();
        for (i, r) in (d0..=d1).enumerate() {
            // Summaries that end lose the bold they were given for it.
            let unbold = summary(&before, i) && !summary(&after, i);
            if after[i] == before[i] && !unbold {
                continue;
            }
            let at = CellRef::new(r, nc);
            let old = self.sheets[id.0].get(at);
            let sid = old.map_or(0, |c| c.style);
            let mut st = self.styles.get(sid).clone();
            st.indent = after[i];
            if unbold {
                st.bold = false;
            }
            let style = self.styles.intern(st);
            let mut cell = old.cloned().unwrap_or_else(|| Cell::new(Content::Empty, 0));
            cell.style = style;
            cells.push((at, Some(cell)));
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_indent");
        }
    }

    /// The predecessor text of every task, changed by `f(position, links)`
    /// (positions 0-based): the cell changes.
    fn plan_edit_links(&mut self, id: TableId, mut f: impl FnMut(usize, &mut Vec<Link>)) -> Vec<(CellRef, Option<Cell>)> {
        let Some(t) = self.plan_table(id).cloned() else { return Vec::new() };
        let hpd = t.plan.as_ref().map_or(8.0, |p| p.hours_per_day());
        let Some(pc) = Self::field_col(&t, Field::Predecessors) else { return Vec::new() };
        let (d0, d1) = t.data_rows();
        let mut cells = Vec::new();
        for (p, r) in (d0..=d1).enumerate() {
            let at = CellRef::new(r, pc);
            let old = match self.sheets[id.0].value(at) {
                Value::Number(n) => format!("{n}"),
                Value::Text(s) => s.to_string(),
                _ => String::new(),
            };
            let (mut list, bad) = parse::links(&old, hpd);
            let before = list.clone();
            f(p, &mut list);
            if list == before {
                continue;
            }
            let mut s = parse::links_text(&list);
            for b in bad {
                if !s.is_empty() {
                    s.push(';');
                }
                s.push_str(&b);
            }
            let v = if s.is_empty() { Value::Empty } else { Value::text(s.as_str()) };
            cells.extend(self.plan_put(id.0, at, v, Some("@"), None));
        }
        cells
    }

    fn plan_position(&self, id: TableId, row: u32) -> Option<usize> {
        let t = self.table(id)?;
        let (d0, d1) = t.data_rows();
        (d0..=d1).contains(&row).then(|| (row - d0) as usize)
    }

    /// Links the tasks in `rows` one after the other (`kind`), or removes
    /// the links among them (`kind` None; one task: all its predecessors).
    pub fn plan_link(&mut self, id: TableId, rows: &[u32], kind: Option<DepType>) {
        let mut pos: Vec<usize> = rows.iter().filter_map(|r| self.plan_position(id, *r)).collect();
        pos.sort_unstable();
        pos.dedup();
        let cells = match kind {
            Some(k) => {
                if pos.len() < 2 {
                    return;
                }
                self.plan_edit_links(id, |p, list| {
                    if let Some(i) = pos.iter().position(|x| *x == p).filter(|i| *i > 0) {
                        let pred = pos[i - 1];
                        if !list.iter().any(|l| l.pred == pred) {
                            list.push(Link { pred, kind: k, lag: Lag::Days(0.0) });
                        }
                    }
                })
            }
            None if pos.len() == 1 => self.plan_edit_links(id, |p, list| {
                if p == pos[0] {
                    list.clear();
                }
            }),
            None => self.plan_edit_links(id, |p, list| {
                if pos.contains(&p) {
                    list.retain(|l| !pos.contains(&l.pred));
                }
            }),
        };
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_link");
        }
    }

    /// Adds a link from the task in row `from` to the one in row `to`
    /// (dragging in the Gantt).
    pub fn plan_add_link(&mut self, id: TableId, from: u32, to: u32, kind: DepType) {
        let (Some(a), Some(b)) = (self.plan_position(id, from), self.plan_position(id, to)) else { return };
        if a == b {
            return;
        }
        let cells = self.plan_edit_links(id, |p, list| {
            if p == b && !list.iter().any(|l| l.pred == a) {
                list.push(Link { pred: a, kind, lag: Lag::Days(0.0) });
            }
        });
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_link");
        }
    }

    /// Makes the tasks in `rows` manual or automatic.
    pub fn plan_set_mode(&mut self, id: TableId, rows: &[u32], manual: bool, manual_word: &str, auto_word: &str) {
        let before = self.undo.len();
        let Some(t) = self.plan_ensure(id, &[Field::Mode], "undo.plan_mode") else { return };
        let Some(mc) = Self::field_col(&t, Field::Mode) else { return };
        let (d0, d1) = t.data_rows();
        let mut cells = Vec::new();
        for &r in rows.iter().filter(|r| (d0..=d1).contains(*r)) {
            cells.extend(self.plan_put(id.0, CellRef::new(r, mc), Value::text(if manual { manual_word } else { auto_word }), None, None));
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_mode");
        }
        let n = self.undo.len() - before;
        self.merge_undo(n, "undo.plan_mode");
    }

    /// Moves the task in `row` to start on `day` (an automatic task gets a
    /// "start no earlier than" constraint; a manual one keeps its length).
    pub fn plan_move_task(&mut self, id: TableId, row: u32, day: f64) {
        let Some(view) = self.plan_view(id) else { return };
        let Some(i) = view.task_at_row(row) else { return };
        if view.schedule.tasks[i].summary {
            return;
        }
        let day = day.floor();
        let (Some(sc), fc) = (view.col(Field::Start), view.col(Field::Finish)) else { return };
        let fmt = self.locale.short_date_code();
        if view.input[i].manual {
            let w = view.work_time();
            let out = &view.schedule.tasks[i];
            let mut cells: Vec<_> = self.plan_put(id.0, CellRef::new(row, sc), Value::Number(day), Some(fmt), None).into_iter().collect();
            if let Some(fc) = fc {
                let f = if out.duration > 0.0 { w.finish_day(w.add(w.norm_start(day), out.duration)) as f64 } else { day };
                cells.extend(self.plan_put(id.0, CellRef::new(row, fc), Value::Number(f), Some(fmt), None));
            }
            if !cells.is_empty() {
                self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_move");
            }
        } else {
            // As if the start were typed.
            let cells: Vec<_> = self.plan_put(id.0, CellRef::new(row, sc), Value::Number(day), Some(fmt), None).into_iter().collect();
            if !cells.is_empty() {
                self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_move");
            }
        }
    }

    /// Sets the duration of the task in `row` (stretching it in the Gantt).
    pub fn plan_set_duration(&mut self, id: TableId, row: u32, days: f64) {
        let Some(view) = self.plan_view(id) else { return };
        let (Some(i), Some(dc)) = (view.task_at_row(row), view.col(Field::Duration)) else { return };
        if view.schedule.tasks[i].summary {
            return;
        }
        let d = (days.max(0.0) * 1000.0).round() / 1000.0;
        let mut cells: Vec<_> = self.plan_put(id.0, CellRef::new(row, dc), Value::Number(d), None, None).into_iter().collect();
        // A manual task's finish follows.
        if view.input[i].manual
            && let (Some(fc), Some(s)) = (view.col(Field::Finish), view.input[i].start)
        {
            let w = view.work_time();
            let f = if d > 0.0 { w.finish_day(w.add(w.norm_start(s), d)) as f64 } else { s };
            cells.extend(self.plan_put(id.0, CellRef::new(row, fc), Value::Number(f), Some(self.locale.short_date_code()), None));
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_duration");
        }
    }

    /// Inserts `count` blank tasks before the one in `row` (links keep
    /// pointing at the same tasks).
    pub fn plan_insert_tasks(&mut self, id: TableId, row: u32, count: u32) {
        let Some(p) = self.plan_position(id, row) else { return };
        let count = count.max(1);
        let links = self.plan_edit_links(id, |_, list| {
            for l in list.iter_mut() {
                if l.pred >= p {
                    l.pred += count as usize;
                }
            }
        });
        // The new texts go where the rows move to.
        let cells: Vec<(CellRef, Option<Cell>)> = links.into_iter().map(|(c, v)| (if c.row >= row { CellRef::new(c.row + count, c.col) } else { c }, v)).collect();
        let mut ops = vec![Op::InsertRows { sheet: id.0, at: row, count }];
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet: id.0, cells });
        }
        self.execute_as(Op::Batch(ops), "undo.plan_task_insert");
    }

    /// Deletes the tasks in `rows` (the links to them go too). All of them:
    /// the plan is left with one empty row, as a table keeps one data row.
    pub fn plan_delete_tasks(&mut self, id: TableId, rows: &[u32]) {
        let Some(t) = self.plan_table(id).cloned() else { return };
        let mut rows: Vec<u32> = rows.iter().copied().filter(|r| self.plan_position(id, *r).is_some()).collect();
        rows.sort_unstable();
        rows.dedup();
        let (d0, d1) = t.data_rows();
        if rows.is_empty() {
            return;
        }
        if rows.len() as u32 > d1 - d0 {
            let cells: Vec<(CellRef, Option<Cell>)> = (t.range.start.col..=t.range.end.col).map(|c| (CellRef::new(d0, c), None)).collect();
            let mut ops = vec![Op::SetCells { sheet: id.0, cells }];
            if d1 > d0 {
                ops.insert(0, Op::DeleteRowsAt { sheet: id.0, rows: (d0 + 1..=d1).collect() });
            }
            self.execute_as(Op::Batch(ops), "undo.plan_task_delete");
            return;
        }
        let gone: Vec<usize> = rows.iter().map(|r| (r - d0) as usize).collect();
        let links = self.plan_edit_links(id, |_, list| {
            list.retain(|l| !gone.contains(&l.pred));
            for l in list.iter_mut() {
                l.pred -= gone.iter().filter(|g| **g < l.pred).count();
            }
        });
        let cells: Vec<(CellRef, Option<Cell>)> = links
            .into_iter()
            .filter(|(c, _)| !rows.contains(&c.row))
            .map(|(c, v)| (CellRef::new(c.row - rows.iter().filter(|r| **r < c.row).count() as u32, c.col), v))
            .collect();
        let mut ops = vec![Op::DeleteRowsAt { sheet: id.0, rows: rows.clone() }];
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet: id.0, cells });
        }
        self.execute_as(Op::Batch(ops), "undo.plan_task_delete");
    }

    /// Saves the current dates and costs as the baseline (columns added
    /// when missing).
    pub fn plan_save_baseline(&mut self, id: TableId, today: f64) {
        let before = self.undo.len();
        let fields = [Field::BaselineStart, Field::BaselineFinish, Field::BaselineCost, Field::StartVariance, Field::FinishVariance, Field::CostVariance];
        let Some(t) = self.plan_ensure(id, &fields, "undo.plan_baseline") else { return };
        let Some(view) = self.plan_view(id) else { return };
        let fmt = self.locale.short_date_code();
        let money = money_format(&self.locale);
        let mut cells = Vec::new();
        let (bs, bf, bc) = (Self::field_col(&t, Field::BaselineStart), Self::field_col(&t, Field::BaselineFinish), Self::field_col(&t, Field::BaselineCost));
        for (i, &r) in view.rows.iter().enumerate() {
            let o = &view.schedule.tasks[i];
            if let Some(c) = bs {
                cells.extend(self.plan_put(id.0, CellRef::new(r, c), Value::Number(o.start_day as f64), Some(fmt), None));
            }
            if let Some(c) = bf {
                cells.extend(self.plan_put(id.0, CellRef::new(r, c), Value::Number(o.finish_day as f64), Some(fmt), None));
            }
            if let Some(c) = bc {
                cells.extend(self.plan_put(id.0, CellRef::new(r, c), Value::Number((view.cost[i] * 100.0).round() / 100.0), Some(&money), None));
            }
        }
        let mut list = self.sheets[id.0].tables.clone();
        if let Some(p) = list[id.1].plan.as_mut() {
            p.baseline = Some(today.floor());
        }
        self.execute_as(Op::Batch(vec![Op::SetCells { sheet: id.0, cells }, Op::Tables { sheet: id.0, list }]), "undo.plan_baseline");
        let n = self.undo.len() - before;
        self.merge_undo(n, "undo.plan_baseline");
    }

    /// Levels the resources of plan `id`: tasks of lower priority wait for
    /// their resources (delays in the leveling delay column). Returns
    /// whether anything moved.
    pub fn plan_level(&mut self, id: TableId) -> bool {
        let Some(view) = self.plan_view(id) else { return false };
        let caps: Vec<f64> = view.resources.iter().map(|r| r.capacity).collect();
        let delays = level(&view.input, &view.plan.calendars, view.plan.start, &view.uses, &caps, &view.priority);
        let now: Vec<f64> = view.input.iter().map(|t| t.delay).collect();
        if delays.iter().zip(&now).all(|(a, b)| (a - b).abs() < 1e-9) {
            return false;
        }
        let before = self.undo.len();
        let Some(t) = self.plan_ensure(id, &[Field::LevelDelay], "undo.plan_level") else { return false };
        let Some(lc) = Self::field_col(&t, Field::LevelDelay) else { return false };
        let mut cells = Vec::new();
        for (i, &r) in view.rows.iter().enumerate() {
            let v = if delays[i] > 0.0 { Value::Number((delays[i] * 1000.0).round() / 1000.0) } else { Value::Empty };
            cells.extend(self.plan_put(id.0, CellRef::new(r, lc), v, None, None));
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_level");
        }
        let n = self.undo.len() - before;
        self.merge_undo(n, "undo.plan_level");
        true
    }

    /// Removes the leveling delays.
    pub fn plan_clear_leveling(&mut self, id: TableId) {
        let Some(view) = self.plan_view(id) else { return };
        let Some(lc) = view.col(Field::LevelDelay) else { return };
        let mut cells = Vec::new();
        for &r in &view.rows {
            cells.extend(self.plan_put(id.0, CellRef::new(r, lc), Value::Empty, None, None));
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet: id.0, cells }, "undo.plan_level");
        }
    }

    /// Creates the resource sheet of plan `id` (a new sheet named
    /// `sheet_name` with a resource table of the names its tasks use), or
    /// returns the one it has.
    pub fn plan_resource_sheet(&mut self, id: TableId, sheet_name: &str) -> Option<TableId> {
        let view = self.plan_view(id)?;
        if let Some(r) = (!view.plan.resources.is_empty()).then(|| self.find_table(&view.plan.resources)).flatten() {
            return Some(r);
        }
        let before = self.undo.len();
        let name = if self.sheet_index(sheet_name).is_none() { sheet_name.to_string() } else { self.unique_sheet_name(sheet_name) };
        let index = self.sheets.len();
        let mut sh = crate::Sheet::new(name);
        let fields = Field::RESOURCES;
        let names: Vec<String> = Self::unique_names(fields.iter().map(|f| field_name(*f)).collect());
        let head = self.styles.intern(crate::CellStyle { bold: true, ..Default::default() });
        let money = self.styles.intern(crate::CellStyle { num_fmt: money_format(&self.locale).into(), ..Default::default() });
        let pct = self.styles.intern(crate::CellStyle { num_fmt: "0%".into(), ..Default::default() });
        for (i, n) in names.iter().enumerate() {
            sh.put(CellRef::new(0, i as u32), Some(Cell::new(Content::Value(Value::text(n.as_str())), head)));
        }
        let list: Vec<&super::Resource> = view.resources.iter().collect();
        let rows = list.len().max(1) as u32;
        for (k, r) in list.iter().enumerate() {
            let row = 1 + k as u32;
            sh.put(CellRef::new(row, 0), Some(Cell::new(Content::Value(Value::text(r.name.as_str())), 0)));
            sh.put(CellRef::new(row, 1), Some(Cell::new(Content::Value(Value::text(r.kind.key())), 0)));
            sh.put(CellRef::new(row, 2), Some(Cell::new(Content::Value(Value::Number(r.capacity)), pct)));
            sh.put(CellRef::new(row, 4), Some(Cell::new(Content::Value(Value::Number(r.rate)), money)));
            sh.put(CellRef::new(row, 5), Some(Cell::new(Content::Value(Value::Number(r.per_use)), money)));
        }
        let tname = self.unique_table_name("Resources");
        let mut t = Table::new(self.next_table_id(), tname.clone(), Range::new(CellRef::new(0, 0), CellRef::new(rows, fields.len() as u32 - 1)), names);
        for (c, f) in t.columns.iter_mut().zip(fields) {
            c.field = Some(f);
        }
        t.filter_button = false;
        t.plan = Some(Box::new(Plan::resources()));
        sh.tables.push(t);
        for (i, w) in [18.0f32, 10.0, 10.0, 12.0, 10.0, 10.0].iter().enumerate() {
            sh.col_widths.insert(i as u32, w * 7.0 + 12.0);
        }
        self.execute_as(Op::AddSheet { index, sheet: Box::new(sh) }, "undo.plan_resources");
        let mut plan = view.plan.clone();
        plan.resources = tname;
        self.set_plan(id, plan);
        let n = self.undo.len() - before;
        self.merge_undo(n, "undo.plan_resources");
        Some((index, 0))
    }

    /// Adds a column of `f` to plan `id` (Planning > Columns).
    pub fn plan_add_field(&mut self, id: TableId, f: Field) {
        self.plan_add_fields(id, &[f]);
    }

    /// Adds the columns of `fields` that plan `id` lacks, in one step.
    pub fn plan_add_fields(&mut self, id: TableId, fields: &[Field]) {
        self.plan_ensure(id, fields, "undo.plan_columns");
    }

    /// Team Planner: moves task `task` (by index) from resource `from`
    /// (None: unassigned) to `to` (None: unassigned), keeping its units, in
    /// one step. False when nothing changed.
    pub fn plan_reassign(&mut self, id: TableId, task: usize, from: Option<&str>, to: Option<&str>) -> bool {
        use crate::plan::parse::{Assignment, assignments, assignments_text};
        let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
        if from.zip(to).is_some_and(|(a, b)| same(a, b)) || (from.is_none() && to.is_none()) {
            return false;
        }
        let before = self.undo.len();
        self.plan_ensure(id, &[Field::Resources], "undo.plan_reassign");
        let Some(v) = self.plan_view(id) else { return false };
        let (Some(&row), Some(col)) = (v.rows.get(task), v.col(Field::Resources)) else { return false };
        let at = CellRef::new(row, col);
        let text = self.sheets[id.0].value(at).to_text().map(|t| t.to_string()).unwrap_or_default();
        let mut list = assignments(&text);
        let units = from.and_then(|f| list.iter().find(|a| same(&a.name, f))).and_then(|a| a.units);
        match from {
            Some(f) => list.retain(|a| !same(&a.name, f)),
            None => list.clear(),
        }
        if let Some(t) = to
            && !list.iter().any(|a| same(&a.name, t))
        {
            list.push(Assignment { name: t.trim().to_string(), units });
        }
        self.set_input(id.0, at, &assignments_text(&list));
        let n = self.undo.len() - before;
        self.merge_undo(n, "undo.plan_reassign");
        n > 0
    }

    /// The last day of a task of `days` working days that starts on `day`.
    pub fn plan_finish_of(view: &super::PlanView, day: f64, days: f64) -> i64 {
        let w = view.work_time();
        if days <= 0.0 { day as i64 } else { w.finish_day(w.add(w.norm_start(day.floor()), days)) }
    }

    /// Working days from the start of `from` to the end of `to`.
    pub fn plan_days_between(view: &super::PlanView, from: f64, to: f64) -> f64 {
        let w = view.work_time();
        w.work(w.norm_start(from.floor()), w.norm_end(to.floor() + END)).max(0.0)
    }

    /// The fields of a table's columns (for the Columns menu).
    pub fn plan_fields(&self, id: TableId) -> Vec<Field> {
        self.table(id).map(|t| t.columns.iter().filter_map(|c| c.field).collect()).unwrap_or_default()
    }

    /// Marks table column `col` of `id` as `field` (None: no role).
    pub fn plan_set_field(&mut self, id: TableId, col: usize, field: Option<Field>) {
        let Some(t) = self.table(id) else { return };
        if col >= t.columns.len() || t.columns[col].field == field {
            return;
        }
        let mut list = self.sheets[id.0].tables.clone();
        for c in &mut list[id.1].columns {
            if field.is_some() && c.field == field {
                c.field = None;
            }
        }
        list[id.1].columns[col].field = field;
        self.execute_as(Op::Tables { sheet: id.0, list }, "undo.plan_columns");
    }
}

/// A column made for a plan field (tests and file readers).
pub fn plan_column(f: Field) -> TableColumn {
    let mut c = TableColumn::new(field_name(f));
    c.field = Some(f);
    c
}
