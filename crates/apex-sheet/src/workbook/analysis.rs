//! What-if analysis, Solver, statistics output, pivot tables, Flash Fill
//! and custom lists on a workbook. Each command is one undo step; the
//! trial evaluations of goal seek, data tables and Solver set cells
//! directly and recalculate only their dependents, then put everything
//! back before the final edit.

use super::*;
use crate::analysis::pivot::{self, PivotCellKind, PivotSpec, PivotTable, Source};
use crate::analysis::solver::{self, ConstraintOp, Core, SolverModel, SolverResult, SolverStatus};
use crate::analysis::stats::Series;
use crate::analysis::{Lang, Scenario, Slicer, State, StoredPivot, Table, flash, lp::Cmp};

/// Variable cells and constrained cells a Solver model may have.
const MAX_SOLVER_VARS: usize = 1_000;
const MAX_SOLVER_ROWS: u64 = 2_000;

/// Calculations begun (a table GETPIVOTDATA worked out is good for one).
pub(crate) static PIVOT_PASS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    /// The last table GETPIVOTDATA worked out, with what it was for.
    #[allow(clippy::type_complexity)]
    static PIVOT_MEMO: std::cell::RefCell<Option<((usize, u64, u64, usize), std::sync::Arc<(Source, PivotTable)>)>> = const { std::cell::RefCell::new(None) };
}

/// Why an analysis command could not run.
#[derive(Clone, Debug, PartialEq)]
pub enum AnalysisError {
    /// The cell must hold a formula (goal seek's "set cell").
    NotAFormula(CellRef),
    /// The cell must hold a constant (changing cells, table inputs).
    NotAValue(CellRef),
    /// A range too small or of the wrong shape.
    BadRange,
    /// No such sheet, scenario or pivot table.
    NotFound,
    /// Flash Fill found no pattern in the examples.
    NoPattern,
    /// Nothing to work on (an empty source range...).
    NoData,
    Stats(crate::analysis::stats::StatsError),
    /// The data can't be forecast (Forecast Sheet).
    Forecast(crate::analysis::ets::EtsError),
    /// The fields of a pivot table on the data model come from tables no
    /// relationship joins.
    NoRelationship,
}

/// Result of a goal seek.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoalSeekResult {
    /// Value found for the changing cell.
    pub value: f64,
    /// Value of the set cell with it.
    pub achieved: f64,
    /// Whether `achieved` is within the tolerance of the target.
    pub found: bool,
    pub iterations: usize,
}

/// Where a tool writes its output.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// A new sheet (appended) with this name, numbered if it is taken.
    NewSheet(String),
    /// From this cell of an existing sheet.
    At { sheet: usize, cell: CellRef },
}

impl Workbook {
    // --- trial evaluation ----------------------------------------------------

    /// Puts values into cells without undo and recalculates what depends
    /// on them. Returns the cells as they were.
    fn probe(&mut self, sheet: usize, cells: &[(CellRef, Value)]) -> Vec<(CellRef, Option<Cell>)> {
        let mut saved = Vec::with_capacity(cells.len());
        let mut keys = Vec::with_capacity(cells.len());
        for (c, v) in cells {
            let old = self.sheets[sheet].get(*c).cloned();
            let content = if v.is_empty() { Content::Empty } else { Content::Value(v.clone()) };
            let cell = Cell::with_content(old.as_ref(), content);
            self.sheets[sheet].put(*c, cell);
            saved.push((*c, old));
            keys.push(CellKey::new(sheet, *c));
        }
        self.recalc_from(&keys);
        saved
    }

    /// Undoes [`Workbook::probe`] (only the first save of each cell counts).
    fn unprobe(&mut self, sheet: usize, saved: Vec<(CellRef, Option<Cell>)>) {
        let mut keys = Vec::with_capacity(saved.len());
        for (c, cell) in saved.into_iter().rev() {
            self.sheets[sheet].put(c, cell);
            keys.push(CellKey::new(sheet, c));
        }
        self.recalc_from(&keys);
    }

    /// A cell as a number for the numeric tools (blank is 0).
    fn number_at(&self, sheet: usize, c: CellRef) -> Option<f64> {
        match self.sheets[sheet].value(c) {
            Value::Number(n) => Some(*n),
            Value::Empty => Some(0.0),
            Value::Bool(b) => Some(*b as u8 as f64),
            _ => None,
        }
    }

    fn check_constant(&self, sheet: usize, c: CellRef) -> Result<(), AnalysisError> {
        match self.sheets[sheet].get(c).map(|x| &x.content) {
            Some(Content::Formula { .. }) | Some(Content::Spill(_)) => Err(AnalysisError::NotAValue(c)),
            _ => Ok(()),
        }
    }

    fn check_sheet(&self, sheet: usize) -> Result<(), AnalysisError> {
        if sheet < self.sheets.len() { Ok(()) } else { Err(AnalysisError::NotFound) }
    }

    /// Writes numbers into cells as one undo step (keeping their format).
    fn set_numbers(&mut self, sheet: usize, cells: &[(CellRef, f64)], label: &'static str) {
        let cells: Vec<(CellRef, Option<Cell>)> = cells.iter().map(|(c, v)| (*c, Cell::with_content(self.sheets[sheet].get(*c), Content::Value(Value::Number(*v))))).collect();
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, label);
        }
    }

    /// Replaces the analysis state as its own undo step.
    fn set_state(&mut self, state: State, label: &'static str) {
        self.execute_as(Op::Analysis(Box::new(state)), label);
    }

    // --- goal seek -------------------------------------------------------------

    /// Finds the value of `changing` that makes the formula in `set_cell`
    /// equal `target`, leaving the workbook as it was.
    pub fn goal_seek_preview(&mut self, sheet: usize, set_cell: CellRef, target: f64, changing: CellRef, max_iter: usize, tol: f64) -> Result<GoalSeekResult, AnalysisError> {
        self.check_sheet(sheet)?;
        if self.sheets[sheet].get(set_cell).and_then(Cell::formula).is_none() {
            return Err(AnalysisError::NotAFormula(set_cell));
        }
        self.check_constant(sheet, changing)?;
        let x0 = match self.sheets[sheet].value(changing) {
            Value::Number(n) => *n,
            _ => 0.0,
        };
        let original = self.sheets[sheet].get(changing).cloned();
        let tol = if tol > 0.0 { tol } else { 0.001 };
        let mut iterations = 0usize;
        let mut f = |wb: &mut Workbook, x: f64| -> f64 {
            iterations += 1;
            wb.probe(sheet, &[(changing, Value::Number(x))]);
            wb.number_at(sheet, set_cell).map_or(f64::NAN, |v| v - target)
        };
        let (x, fx) = seek(|x| f(self, x), x0, max_iter.max(1), tol);
        // Put the original cell back.
        self.sheets[sheet].put(changing, original);
        self.recalc_from(&[CellKey::new(sheet, changing)]);
        Ok(GoalSeekResult { value: x, achieved: fx + target, found: fx.abs() <= tol, iterations })
    }

    /// Goal seek that keeps the value found (one undo step), even when the
    /// target was not reached, like Excel; `undo` restores the cell.
    pub fn goal_seek(&mut self, sheet: usize, set_cell: CellRef, target: f64, changing: CellRef, max_iter: usize, tol: f64) -> Result<GoalSeekResult, AnalysisError> {
        let r = self.goal_seek_preview(sheet, set_cell, target, changing, max_iter, tol)?;
        if r.value.is_finite() {
            self.set_numbers(sheet, &[(changing, r.value)], "undo.goal_seek");
        }
        Ok(r)
    }

    // --- data tables -----------------------------------------------------------

    /// Fills a what-if data table (Data > What-If Analysis > Data Table).
    ///
    /// With only `col_input`, the input values run down the first column
    /// of `r` and the formulas sit in its first row; with only
    /// `row_input`, inputs run along the first row and formulas down the
    /// first column; with both, the formula is the top-left cell. Results
    /// are written as static values (Excel keeps a live `{=TABLE()}`
    /// array instead): run it again after changing the model.
    pub fn data_table(&mut self, sheet: usize, r: &Range, row_input: Option<CellRef>, col_input: Option<CellRef>) -> Result<usize, AnalysisError> {
        self.check_sheet(sheet)?;
        if r.rows() < 2 || r.cols() < 2 || r.cell_count() > 1_000_000 {
            return Err(AnalysisError::BadRange);
        }
        for c in row_input.iter().chain(col_input.iter()) {
            self.check_constant(sheet, *c)?;
            if r.contains(*c) {
                return Err(AnalysisError::BadRange);
            }
        }
        let (r0, c0) = (r.start.row, r.start.col);
        let val = |wb: &Workbook, row: u32, col: u32| wb.sheets[sheet].value(CellRef::new(row, col)).clone();
        // (cell to fill, inputs to set, cell to read)
        let mut jobs: Vec<(CellRef, Vec<(CellRef, Value)>, CellRef)> = Vec::new();
        match (row_input, col_input) {
            (None, Some(ci)) => {
                for row in r0 + 1..=r.end.row {
                    for col in c0 + 1..=r.end.col {
                        jobs.push((CellRef::new(row, col), vec![(ci, val(self, row, c0))], CellRef::new(r0, col)));
                    }
                }
            }
            (Some(ri), None) => {
                for row in r0 + 1..=r.end.row {
                    for col in c0 + 1..=r.end.col {
                        jobs.push((CellRef::new(row, col), vec![(ri, val(self, r0, col))], CellRef::new(row, c0)));
                    }
                }
            }
            (Some(ri), Some(ci)) => {
                for row in r0 + 1..=r.end.row {
                    for col in c0 + 1..=r.end.col {
                        jobs.push((CellRef::new(row, col), vec![(ri, val(self, r0, col)), (ci, val(self, row, c0))], CellRef::new(r0, c0)));
                    }
                }
            }
            (None, None) => return Err(AnalysisError::BadRange),
        }
        let inputs: Vec<CellRef> = row_input.into_iter().chain(col_input).collect();
        let saved: Vec<(CellRef, Option<Cell>)> = inputs.iter().map(|c| (*c, self.sheets[sheet].get(*c).cloned())).collect();
        let mut results = Vec::with_capacity(jobs.len());
        for (dst, set, read) in &jobs {
            self.probe(sheet, set);
            let v = match self.sheets[sheet].value(*read) {
                Value::Empty => Value::Number(0.0),
                v => v.clone(),
            };
            results.push((*dst, v));
        }
        self.unprobe(sheet, saved);
        let cells: Vec<(CellRef, Option<Cell>)> = results.into_iter().map(|(c, v)| (c, Cell::with_content(self.sheets[sheet].get(c), Content::Value(v)))).collect();
        let n = cells.len();
        self.execute_as(Op::SetCells { sheet, cells }, "undo.data_table");
        Ok(n)
    }

    // --- scenarios -------------------------------------------------------------

    /// The scenarios of a sheet.
    pub fn scenarios(&self, sheet: usize) -> &[Scenario] {
        self.sheets.get(sheet).and_then(|s| self.analysis.scenarios.get(&s.name)).map_or(&[], |v| &v[..])
    }

    /// Adds a scenario, or replaces the one with the same name.
    pub fn add_scenario(&mut self, sheet: usize, sc: Scenario) -> Result<(), AnalysisError> {
        self.check_sheet(sheet)?;
        if sc.name.trim().is_empty() || sc.changing.is_empty() || sc.changing.len() != sc.values.len() {
            return Err(AnalysisError::BadRange);
        }
        let mut st = self.analysis.clone();
        let list = st.scenarios.entry(self.sheets[sheet].name.clone()).or_default();
        match list.iter_mut().find(|s| s.name.eq_ignore_ascii_case(&sc.name)) {
            Some(s) => *s = sc,
            None => list.push(sc),
        }
        self.set_state(st, "undo.scenario");
        Ok(())
    }

    pub fn delete_scenario(&mut self, sheet: usize, name: &str) -> Result<(), AnalysisError> {
        self.check_sheet(sheet)?;
        let mut st = self.analysis.clone();
        let list = st.scenarios.get_mut(&self.sheets[sheet].name).ok_or(AnalysisError::NotFound)?;
        let i = list.iter().position(|s| s.name.eq_ignore_ascii_case(name)).ok_or(AnalysisError::NotFound)?;
        list.remove(i);
        if list.is_empty() {
            st.scenarios.remove(&self.sheets[sheet].name);
        }
        // Indexes after the deleted scenario move up.
        if let Some(info) = st.scenario_info.get_mut(&self.sheets[sheet].name) {
            let fix = |x: Option<usize>| x.and_then(|k| if k == i { None } else if k > i { Some(k - 1) } else { Some(k) });
            info.current = fix(info.current);
            info.selected = fix(info.selected);
        }
        self.set_state(st, "undo.scenario");
        Ok(())
    }

    /// Puts a scenario's values into its cells and makes it the current
    /// one (one undo step).
    pub fn show_scenario(&mut self, sheet: usize, name: &str) -> Result<(), AnalysisError> {
        let i = self.scenarios(sheet).iter().position(|s| s.name.eq_ignore_ascii_case(name)).ok_or(AnalysisError::NotFound)?;
        let sc = self.scenarios(sheet)[i].clone();
        let inputs: Vec<(CellRef, String)> = sc.changing.iter().copied().zip(sc.values.iter().cloned()).collect();
        let len = self.undo.len();
        self.set_inputs(sheet, inputs);
        let key = self.sheets[sheet].name.clone();
        if self.analysis.scenario_info.get(&key).and_then(|x| x.current) != Some(i) {
            let mut st = self.analysis.clone();
            st.scenario_info.entry(key).or_default().current = Some(i);
            self.set_state(st, "undo.scenario");
        }
        let n = self.undo.len() - len;
        self.merge_undo(n, "undo.scenario");
        if n == 1 {
            if let Some(last) = self.undo.last_mut() {
                last.1 = "undo.scenario";
            }
        }
        Ok(())
    }

    /// A scenario input as a value for trial evaluation (formulas are
    /// evaluated once, where the cell is).
    fn input_value(&self, sheet: usize, at: CellRef, text: &str) -> Value {
        match self.content_from_input(text).0 {
            Content::Value(v) => v,
            Content::Formula { expr, .. } => Env::new(self, sheet, at, self.locale).eval(&expr.expr()),
            _ => Value::Empty,
        }
    }

    /// Scenario Summary report on a new sheet: the changing cells and the
    /// `result_cells` under the current values and under each scenario.
    /// Returns the new sheet's index.
    pub fn scenario_summary(&mut self, sheet: usize, result_cells: &[CellRef], lang: Lang) -> Result<usize, AnalysisError> {
        self.check_sheet(sheet)?;
        let scs = self.scenarios(sheet).to_vec();
        if scs.is_empty() {
            return Err(AnalysisError::NotFound);
        }
        let mut changing: Vec<CellRef> = Vec::new();
        for s in &scs {
            for c in &s.changing {
                if !changing.contains(c) {
                    changing.push(*c);
                }
            }
        }
        let current = |wb: &Workbook, cells: &[CellRef]| -> Vec<Value> { cells.iter().map(|c| wb.sheets[sheet].value(*c).clone()).collect() };
        let mut cols: Vec<(String, Vec<Value>, Vec<Value>)> = vec![(lang.text("current_values"), current(self, &changing), current(self, result_cells))];
        for s in &scs {
            let set: Vec<(CellRef, Value)> = s.changing.iter().zip(&s.values).map(|(c, t)| (*c, self.input_value(sheet, *c, t))).collect();
            let saved = self.probe(sheet, &set);
            cols.push((s.name.clone(), current(self, &changing), current(self, result_cells)));
            self.unprobe(sheet, saved);
        }
        let abs = |c: &CellRef| format!("${}${}", crate::addr::col_name(c.col), c.row + 1);
        let mut t = Table::new();
        t.push(vec![Value::Empty, Value::text(lang.text("scenario_summary"))]);
        let mut head = vec![Value::Empty, Value::Empty];
        head.extend(cols.iter().map(|c| Value::text(c.0.as_str())));
        t.push(head);
        t.push(vec![Value::Empty, Value::text(lang.text("changing_cells"))]);
        for (i, c) in changing.iter().enumerate() {
            let mut row = vec![Value::Empty, Value::text(abs(c))];
            row.extend(cols.iter().map(|x| x.1[i].clone()));
            t.push(row);
        }
        if !result_cells.is_empty() {
            t.push(vec![Value::Empty, Value::text(lang.text("result_cells"))]);
            for (i, c) in result_cells.iter().enumerate() {
                let mut row = vec![Value::Empty, Value::text(abs(c))];
                row.extend(cols.iter().map(|x| x.2[i].clone()));
                t.push(row);
            }
        }
        t.push(vec![Value::Empty, Value::text(lang.text("scenario_note"))]);
        let len = self.undo.len();
        let (s, _) = self.write_table(&t, &Output::NewSheet(lang.text("sheet_scenario")), "undo.scenario")?;
        // Remember the result cells for the next report (one undo step).
        let results: Vec<Range> = result_cells.iter().map(|c| Range::single(*c)).collect();
        let key = self.sheets[sheet].name.clone();
        if self.analysis.scenario_info.get(&key).map_or(&[][..], |x| &x.results[..]) != &results[..] {
            let mut st = self.analysis.clone();
            st.scenario_info.entry(key).or_default().results = results;
            self.set_state(st, "undo.scenario");
            self.merge_undo(self.undo.len() - len, "undo.scenario");
        }
        Ok(s)
    }

    // --- Solver ----------------------------------------------------------------

    /// Runs Solver and keeps the solution when one is found (one undo
    /// step); otherwise the variable cells are left as they were.
    pub fn solve(&mut self, sheet: usize, model: &SolverModel) -> SolverResult {
        let r = self.solve_preview(sheet, model);
        if r.is_solution() {
            if let Ok(vars) = self.solver_vars(sheet, model) {
                let cells: Vec<(CellRef, f64)> = vars.into_iter().zip(r.values.iter().copied()).collect();
                self.set_numbers(sheet, &cells, "undo.solver");
            }
        }
        r
    }

    /// Runs Solver and restores the variable cells.
    pub fn solve_preview(&mut self, sheet: usize, model: &SolverModel) -> SolverResult {
        let bad = SolverResult { status: SolverStatus::BadModel, values: Vec::new(), objective: f64::NAN };
        if self.check_sheet(sheet).is_err() {
            return bad;
        }
        let Ok(vars) = self.solver_vars(sheet, model) else { return bad };
        let Some(setup) = self.solver_setup(model, &vars) else { return bad };
        let (core, general) = setup;
        let x0: Vec<f64> = vars.iter().map(|c| self.number_at(sheet, *c).unwrap_or(0.0)).collect();
        let saved: Vec<(CellRef, Option<Cell>)> = vars.iter().map(|c| (*c, self.sheets[sheet].get(*c).cloned())).collect();
        let objective = model.objective;
        let locale = self.locale;
        let mut eval = |x: &[f64]| -> Option<(f64, Vec<f64>)> {
            let set: Vec<(CellRef, Value)> = vars.iter().zip(x).map(|(c, v)| (*c, Value::Number(*v))).collect();
            self.probe(sheet, &set);
            let f = match self.sheets[sheet].value(objective) {
                Value::Number(n) => *n,
                Value::Empty => 0.0,
                Value::Bool(b) => *b as u8 as f64,
                _ => return None,
            };
            let mut g = Vec::new();
            for (cells, rhs) in &general {
                let rv: Vec<f64> = match rhs {
                    Rhs::Num(n) => vec![*n],
                    Rhs::Expr(e) => match Env::new(&*self, sheet, objective, locale).eval(e) {
                        Value::Number(n) => vec![n],
                        Value::Empty => vec![0.0],
                        Value::Bool(b) => vec![b as u8 as f64],
                        Value::Array(m) => m.data.iter().map(|v| match v {
                            Value::Number(n) => Some(*n),
                            Value::Empty => Some(0.0),
                            _ => None,
                        }).collect::<Option<Vec<f64>>>()?,
                        _ => return None,
                    },
                };
                for (i, c) in cells.iter().enumerate() {
                    let l = self.number_at(sheet, *c)?;
                    let r = if rv.len() == 1 { rv[0] } else { *rv.get(i)? };
                    g.push(l - r);
                }
            }
            Some((f, g))
        };
        let r = solver::run(&core, &mut eval, &x0);
        self.unprobe(sheet, saved);
        r
    }

    /// Variable cells of a model, row by row, without repeats.
    fn solver_vars(&self, sheet: usize, model: &SolverModel) -> Result<Vec<CellRef>, AnalysisError> {
        let mut vars = Vec::new();
        let mut seen = HashSet::default();
        for r in &model.variables {
            if r.cell_count() + vars.len() as u64 > MAX_SOLVER_VARS as u64 {
                return Err(AnalysisError::BadRange);
            }
            for row in r.start.row..=r.end.row {
                for col in r.start.col..=r.end.col {
                    let c = CellRef::new(row, col);
                    self.check_constant(sheet, c)?;
                    if seen.insert(c) {
                        vars.push(c);
                    }
                }
            }
        }
        if vars.is_empty() { Err(AnalysisError::BadRange) } else { Ok(vars) }
    }

    /// Bounds, integer marks and the general constraints (cells compared
    /// with a right side) of a model.
    fn solver_setup(&self, model: &SolverModel, vars: &[CellRef]) -> Option<(Core, Vec<(Vec<CellRef>, Rhs)>)> {
        let n = vars.len();
        let mut core = Core::new(n, model);
        let mut has_lb = vec![false; n];
        core.lb = vec![f64::NEG_INFINITY; n];
        let var_index: HashMap<CellRef, usize> = vars.iter().enumerate().map(|(i, c)| (*c, i)).collect();
        let mut general = Vec::new();
        // The cells constrained, in all: the simplex works on a dense
        // table of them by the variables.
        let mut rows = 0u64;
        for con in &model.constraints {
            rows += con.lhs.cell_count();
            if rows > MAX_SOLVER_ROWS {
                return None;
            }
            let cells: Vec<CellRef> = (con.lhs.start.row..=con.lhs.end.row).flat_map(|r| (con.lhs.start.col..=con.lhs.end.col).map(move |c| CellRef::new(r, c))).collect();
            match con.op {
                ConstraintOp::Int | ConstraintOp::Bin => {
                    for c in &cells {
                        let j = *var_index.get(c)?;
                        core.int[j] = true;
                        if con.op == ConstraintOp::Bin {
                            core.lb[j] = core.lb[j].max(0.0);
                            core.ub[j] = core.ub[j].min(1.0);
                            has_lb[j] = true;
                        }
                    }
                    continue;
                }
                _ => {}
            }
            let rhs = self.parse_rhs(&con.rhs)?;
            let kind = match con.op {
                ConstraintOp::Le => Cmp::Le,
                ConstraintOp::Ge => Cmp::Ge,
                _ => Cmp::Eq,
            };
            // A constant bound on variable cells is a bound, not a row.
            if let Rhs::Num(b) = rhs {
                if cells.iter().all(|c| var_index.contains_key(c)) {
                    for c in &cells {
                        let j = var_index[c];
                        if kind != Cmp::Ge {
                            core.ub[j] = core.ub[j].min(b);
                        }
                        if kind != Cmp::Le {
                            core.lb[j] = if has_lb[j] { core.lb[j].max(b) } else { b };
                            has_lb[j] = true;
                        }
                    }
                    continue;
                }
            }
            for _ in 0..cells.len() {
                core.kinds.push(kind);
            }
            general.push((cells, rhs));
        }
        for j in 0..n {
            if !has_lb[j] && model.non_negative {
                core.lb[j] = 0.0;
            }
        }
        Some((core, general))
    }

    fn parse_rhs(&self, text: &str) -> Option<Rhs> {
        let t = text.trim();
        let t = t.strip_prefix('=').unwrap_or(t).trim();
        if t.is_empty() {
            return None;
        }
        if let Value::Number(n) = format::parse_input(t, &self.locale).value {
            return Some(Rhs::Num(n));
        }
        formula::parse(&format!("={}", t), &self.locale).ok().map(|e| Rhs::Expr(Box::new(e)))
    }

    /// Remembers the Solver model of a sheet (one undo step).
    pub fn save_solver_model(&mut self, sheet: usize, model: SolverModel) {
        let Some(name) = self.sheets.get(sheet).map(|s| s.name.clone()) else { return };
        let mut st = self.analysis.clone();
        st.solver_models.insert(name, model);
        self.set_state(st, "undo.solver");
    }

    pub fn solver_model(&self, sheet: usize) -> Option<&SolverModel> {
        self.analysis.solver_models.get(&self.sheets.get(sheet)?.name)
    }

    // --- statistics tools ----------------------------------------------------

    /// Reads the variables of an input range: one series per column (or
    /// per row), captions from the first cell when `labels`. Whole rows or
    /// columns are clamped to the used area.
    pub fn read_series(&self, sheet: usize, r: &Range, by_rows: bool, labels: bool) -> Vec<Series> {
        let Some(sh) = self.sheets.get(sheet) else { return Vec::new() };
        let r = if r.cell_count() > 1_000_000 { match self.clamp_to_used(sheet, r) { Some(x) => x, None => return Vec::new() } } else { *r };
        let (lines, len) = if by_rows { (r.rows(), r.cols()) } else { (r.cols(), r.rows()) };
        (0..lines)
            .map(|l| {
                let at = |i: u32| if by_rows { CellRef::new(r.start.row + l, r.start.col + i) } else { CellRef::new(r.start.row + i, r.start.col + l) };
                let label = labels.then(|| self.display(sheet, at(0)).0).filter(|s| !s.is_empty());
                let first = labels as u32;
                let values = (first..len)
                    .map(|i| match sh.value(at(i)) {
                        Value::Number(n) if n.is_finite() => Some(*n),
                        _ => None,
                    })
                    .collect();
                Series { label, values }
            })
            .collect()
    }

    /// The numbers of a range (histogram bins, sampling input).
    pub fn read_numbers(&self, sheet: usize, r: &Range) -> Vec<f64> {
        self.read_series(sheet, r, false, false).iter().flat_map(Series::nums).collect()
    }

    /// Writes a tool's output table as one undo step. Returns the sheet
    /// and the area written.
    pub fn write_table(&mut self, t: &Table, out: &Output, label: &'static str) -> Result<(usize, Range), AnalysisError> {
        let (h, w) = (t.height() as u32, t.width().max(1) as u32);
        if h == 0 {
            return Err(AnalysisError::NoData);
        }
        let formats: HashMap<(usize, usize), &str> = t.formats.iter().map(|(k, f)| (*k, f.as_str())).collect();
        match out {
            Output::NewSheet(name) => {
                let name = if self.sheet_index(name).is_none() && !name.is_empty() { name.clone() } else { self.unique_sheet_name(if name.is_empty() { "Sheet" } else { name }) };
                let mut sh = Sheet::new(name);
                for (r, row) in t.rows.iter().enumerate() {
                    for (c, v) in row.iter().enumerate() {
                        let style = formats.get(&(r, c)).map_or(0, |f| self.style_with_format(0, f));
                        if !v.is_empty() || style != 0 {
                            let content = if v.is_empty() { Content::Empty } else { Content::Value(v.clone()) };
                            sh.put(CellRef::new(r as u32, c as u32), Some(Cell::new(content, style)));
                        }
                    }
                }
                // Room for the captions of the first column.
                sh.col_widths.insert(0, crate::sheet::DEFAULT_COL_WIDTH * 2.0);
                let index = self.sheets.len();
                self.execute_as(Op::AddSheet { index, sheet: Box::new(sh) }, label);
                Ok((index, Range::new(CellRef::new(0, 0), CellRef::new(h - 1, w - 1))))
            }
            Output::At { sheet, cell } => {
                self.check_sheet(*sheet)?;
                if cell.row + h > MAX_ROWS || cell.col + w > MAX_COLS {
                    return Err(AnalysisError::BadRange);
                }
                let mut cells = Vec::new();
                for r in 0..h {
                    for c in 0..w {
                        let at = CellRef::new(cell.row + r, cell.col + c);
                        let v = t.get(r as usize, c as usize);
                        let old = self.sheets[*sheet].get(at).cloned();
                        let content = if v.is_empty() { Content::Empty } else { Content::Value(v.clone()) };
                        let mut new = Cell::with_content(old.as_ref(), content);
                        if let Some(f) = formats.get(&(r as usize, c as usize)) {
                            let style = self.style_with_format(old.as_ref().map_or(0, |x| x.style), f);
                            new = Cell::with_style(new.as_ref().or(Some(&Cell::new(Content::Empty, 0))), style);
                        }
                        if new != old {
                            cells.push((at, new));
                        }
                    }
                }
                self.execute_as(Op::SetCells { sheet: *sheet, cells }, label);
                Ok((*sheet, Range::new(*cell, CellRef::new(cell.row + h - 1, cell.col + w - 1))))
            }
        }
    }

    /// Whether writing a `rows` x `cols` block at `at` would overwrite data.
    pub fn output_overwrites(&self, sheet: usize, at: CellRef, rows: u32, cols: u32) -> bool {
        let Some(sh) = self.sheets.get(sheet) else { return false };
        let r = Range::new(at, CellRef::new((at.row + rows.max(1) - 1).min(MAX_ROWS - 1), (at.col + cols.max(1) - 1).min(MAX_COLS - 1)));
        let mut any = false;
        sh.for_each_in(&r, |_, c| any |= !c.value().is_empty());
        any
    }

    pub(super) fn style_with_format(&mut self, style: u32, code: &str) -> u32 {
        let mut s = self.styles.get(style).clone();
        s.num_fmt = Arc::from(code);
        self.styles.intern(s)
    }

    // --- pivot tables ----------------------------------------------------------

    /// Reads the source range of a pivot table (header row plus records).
    pub fn pivot_source(&self, spec: &PivotSpec) -> Result<Source, AnalysisError> {
        if spec.model.is_some() {
            return self.model_pivot_source(spec);
        }
        let sheet = self.sheet_index(&spec.source_sheet).ok_or(AnalysisError::NotFound)?;
        let r = self.clamp_to_used(sheet, &spec.source).ok_or(AnalysisError::NoData)?;
        if r.rows() < 2 {
            return Err(AnalysisError::NoData);
        }
        let sh = &self.sheets[sheet];
        let ncols = r.cols() as usize;
        let names: Vec<String> = (0..ncols as u32)
            .map(|c| {
                let t = self.display(sheet, CellRef::new(r.start.row, r.start.col + c)).0;
                if t.is_empty() { format!("Column{}", c + 1) } else { t }
            })
            .collect();
        let used = spec.used_columns(&names);
        if used.iter().any(|c| *c as usize >= ncols) || spec.values.iter().any(|v| (pivot::CALC_BASE..pivot::MEASURE_BASE).contains(&v.col) && spec.calc_expr(v.col, &names).is_none()) {
            return Err(AnalysisError::BadRange);
        }
        let records = (r.rows() - 1) as usize;
        let first = r.start.row + 1;
        let mut cols: Vec<Vec<Value>> = vec![Vec::new(); ncols];
        let mut formats: Vec<Option<Arc<str>>> = vec![None; ncols];
        for &c in &used {
            let ci = c as usize;
            if !cols[ci].is_empty() {
                continue;
            }
            let mut v = vec![Value::Empty; records];
            let col = r.start.col + c;
            let mut fmt_style = None;
            sh.for_each_in(&Range::new(CellRef::new(first, col), CellRef::new(r.end.row, col)), |at, cell| {
                v[(at.row - first) as usize] = cell.value().clone();
                if fmt_style.is_none() && matches!(cell.value(), Value::Number(_)) {
                    fmt_style = Some(cell.style);
                }
            });
            if let Some(st) = fmt_style {
                if self.styles.format(st).is_date() {
                    formats[ci] = Some(self.styles.get(st).num_fmt.clone());
                }
            }
            cols[ci] = v;
        }
        Ok(Source { names, cols, formats, records, measures: None })
    }

    /// Computes a pivot table without writing it.
    pub fn pivot_compute(&self, spec: &PivotSpec) -> Result<PivotTable, AnalysisError> {
        Ok(pivot::compute(spec, &self.pivot_source(spec)?))
    }

    /// Cells (values plus style hints) that put `t` at `at`, clearing what
    /// is left of `old` (a previous output).
    fn pivot_cells(&mut self, sheet: usize, at: CellRef, t: &PivotTable, old: Option<Range>, style: &str) -> (Vec<(CellRef, Option<Cell>)>, Range) {
        let rows = (t.rows() as u32).min(MAX_ROWS - at.row).max(1);
        let cols = (t.cols() as u32).min(MAX_COLS - at.col).max(1);
        let area = Range::new(at, CellRef::new(at.row + rows - 1, at.col + cols - 1));
        let mut styles: HashMap<(PivotCellKind, u8, Option<Arc<str>>), u32> = HashMap::default();
        let mut out = Vec::new();
        if let (Some(o), Some(sh)) = (old, self.sheets.get(sheet)) {
            sh.for_each_in(&o, |c, _| {
                if !area.contains(c) {
                    out.push((c, None));
                }
            });
        }
        for r in 0..rows {
            for c in 0..cols {
                let Some(pc) = t.get(r as usize, c as usize) else { continue };
                let key = (pc.kind, pc.indent, pc.format.clone());
                let style = match styles.get(&key) {
                    Some(s) => *s,
                    None => {
                        let s = self.styles.intern(pivot_style(style, pc.kind, pc.indent, pc.format.as_deref()));
                        styles.insert(key, s);
                        s
                    }
                };
                let content = if pc.value.is_empty() { Content::Empty } else { Content::Value(pc.value.clone()) };
                let at = CellRef::new(at.row + r, at.col + c);
                let cell = Cell::new(content, style);
                out.push((at, (!cell.is_blank()).then_some(cell)));
            }
        }
        (out, area)
    }

    /// Writes a computed pivot table at `at` (values and styles), as one
    /// undo step. Returns the area written.
    pub fn write_pivot(&mut self, sheet: usize, at: CellRef, t: &PivotTable) -> Range {
        let (cells, area) = self.pivot_cells(sheet, at, t, None, pivot::DEFAULT_STYLE);
        self.execute_as(Op::SetCells { sheet, cells }, "undo.pivot");
        area
    }

    /// Creates a pivot table: computes it, writes it at `dest` (a new
    /// sheet when None) and keeps the spec for refreshing. Returns the
    /// pivot's index in `analysis.pivots`.
    pub fn create_pivot(&mut self, spec: PivotSpec, dest: Option<(usize, CellRef)>) -> Result<usize, AnalysisError> {
        let t = self.pivot_compute(&spec)?;
        let mut st = self.analysis.clone();
        let op = match dest {
            Some((sheet, at)) => {
                self.check_sheet(sheet)?;
                let (cells, area) = self.pivot_cells(sheet, at, &t, None, &spec.style);
                st.pivots.push(StoredPivot { spec, sheet: self.sheets[sheet].name.clone(), at, area: Some(area), charts: Vec::new() });
                Op::SetCells { sheet, cells }
            }
            None => {
                // Like Excel, a new sheet with the table at A3.
                let name = self.unique_sheet_name(&spec.lang.text("sheet_pivot").replace(' ', ""));
                let index = self.sheets.len();
                let at = CellRef::new(2, 0);
                let (cells, area) = self.pivot_cells(usize::MAX, at, &t, None, &spec.style);
                let mut sh = Sheet::new(name.clone());
                for (c, cell) in cells {
                    sh.put(c, cell);
                }
                st.pivots.push(StoredPivot { spec, sheet: name, at, area: Some(area), charts: Vec::new() });
                Op::AddSheet { index, sheet: Box::new(sh) }
            }
        };
        let index = st.pivots.len() - 1;
        self.execute_as(Op::Batch(vec![op, Op::Analysis(Box::new(st))]), "undo.pivot");
        Ok(index)
    }

    /// Recomputes a stored pivot table from its source (one undo step).
    pub fn refresh_pivot(&mut self, index: usize) -> Result<(), AnalysisError> {
        let p = self.analysis.pivots.get(index).cloned().ok_or(AnalysisError::NotFound)?;
        self.replace_pivot(index, p.spec)
    }

    /// Changes the spec of a stored pivot table and rewrites it.
    pub fn replace_pivot(&mut self, index: usize, spec: PivotSpec) -> Result<(), AnalysisError> {
        let p = self.analysis.pivots.get(index).cloned().ok_or(AnalysisError::NotFound)?;
        let sheet = self.sheet_index(&p.sheet).ok_or(AnalysisError::NotFound)?;
        let t = self.pivot_compute(&spec)?;
        let (cells, area) = self.pivot_cells(sheet, p.at, &t, p.area, &spec.style);
        let mut ops = vec![Op::SetCells { sheet, cells }];
        // Pivot charts follow the table.
        let mut list = self.sheets[sheet].charts.clone();
        let mut charts = p.charts.clone();
        charts.retain(|id| list.iter().any(|c| c.id == *id));
        let mut touched = false;
        for c in list.iter_mut().filter(|c| charts.contains(&c.id)) {
            fill_pivot_chart(c, &t.chart);
            touched = true;
        }
        if touched && list != self.sheets[sheet].charts {
            ops.push(Op::Charts { sheet, list });
        }
        let mut st = self.analysis.clone();
        st.pivots[index] = StoredPivot { spec, sheet: p.sheet, at: p.at, area: Some(area), charts };
        ops.push(Op::Analysis(Box::new(st)));
        self.execute_as(Op::Batch(ops), "undo.pivot");
        Ok(())
    }

    /// Adds a pivot chart beside a pivot table: it plots the leaf rows and
    /// columns and follows the table when it changes. Returns the chart id.
    pub fn create_pivot_chart(&mut self, index: usize, kind: crate::chart::ChartType) -> Result<u32, AnalysisError> {
        let p = self.analysis.pivots.get(index).cloned().ok_or(AnalysisError::NotFound)?;
        let sheet = self.sheet_index(&p.sheet).ok_or(AnalysisError::NotFound)?;
        let t = self.pivot_compute(&p.spec)?;
        if t.chart.series.is_empty() {
            return Err(AnalysisError::NoData);
        }
        let right = p.area.map_or(p.at.col, |a| a.end.col) + 2;
        let mut chart = crate::chart::Chart::new(kind, CellRef::new(p.at.row, right.min(MAX_COLS - 1)), 480.0, 288.0);
        fill_pivot_chart(&mut chart, &t.chart);
        chart.legend = if chart.series.len() > 1 || kind.is_round() { crate::chart::LegendPos::Right } else { crate::chart::LegendPos::None };
        let before = self.undo.len();
        let id = self.add_chart(sheet, chart);
        let mut st = self.analysis.clone();
        st.pivots[index].charts.push(id);
        self.execute_as(Op::Analysis(Box::new(st)), "undo.pivot");
        let steps = self.undo.len().saturating_sub(before);
        self.merge_undo(steps, "undo.insert_chart");
        Ok(id)
    }

    /// Show Details: copies the records behind the value at `c` (a cell of
    /// a pivot table) to a new sheet, formatted as a table. Returns the
    /// new sheet's index.
    pub fn pivot_details(&mut self, sheet: usize, c: CellRef) -> Result<usize, AnalysisError> {
        let index = self.pivot_at(sheet, c).ok_or(AnalysisError::NotFound)?;
        let p = self.analysis.pivots[index].clone();
        let src = self.pivot_source(&p.spec)?;
        let t = pivot::compute(&p.spec, &src);
        let (r, col) = ((c.row - p.at.row) as usize, (c.col - p.at.col) as usize);
        let rows = t.row_paths.get(r).cloned().flatten().ok_or(AnalysisError::NotFound)?;
        let cols = t.col_paths.get(col).cloned().flatten().ok_or(AnalysisError::NotFound)?;
        let recs = pivot::detail_records(&p.spec, &src, &rows, &cols);
        if p.spec.model.is_some() {
            let name = self.unique_sheet_name(&p.spec.lang.text("sheet_details"));
            let sh = self.model_details_sheet(&src, &recs, name);
            let width = sh.extent().map_or(1, |e| e.col + 1);
            let index = self.sheets.len();
            let before = self.undo.len();
            self.execute_as(Op::AddSheet { index, sheet: Box::new(sh) }, "undo.pivot");
            if !recs.is_empty() {
                let _ = self.create_table(index, Range::new(CellRef::new(0, 0), CellRef::new(recs.len() as u32, width - 1)), true, crate::table::DEFAULT_STYLE);
            }
            let steps = self.undo.len().saturating_sub(before);
            self.merge_undo(steps, "undo.pivot");
            return Ok(index);
        }
        let si = self.sheet_index(&p.spec.source_sheet).ok_or(AnalysisError::NotFound)?;
        let range = self.clamp_to_used(si, &p.spec.source).ok_or(AnalysisError::NoData)?;
        let from = &self.sheets[si];
        let name = self.unique_sheet_name(&p.spec.lang.text("sheet_details"));
        let mut sh = Sheet::new(name);
        let copy = |sh: &mut Sheet, row: u32, to: u32| {
            for k in 0..range.cols() {
                let at = CellRef::new(row, range.start.col + k);
                let v = from.value(at).clone();
                let style = from.get(at).map_or(0, |x| x.style);
                let content = if v.is_empty() { Content::Empty } else { Content::Value(v) };
                let cell = Cell::new(content, style);
                sh.put(CellRef::new(to, k), (!cell.is_blank()).then_some(cell));
            }
        };
        copy(&mut sh, range.start.row, 0);
        for (i, rec) in recs.iter().enumerate() {
            copy(&mut sh, range.start.row + 1 + *rec as u32, i as u32 + 1);
        }
        for k in 0..range.cols() {
            let w = from.col_width(range.start.col + k);
            if w != from.default_col_width {
                sh.col_widths.insert(k, w);
            }
        }
        let index = self.sheets.len();
        let before = self.undo.len();
        self.execute_as(Op::AddSheet { index, sheet: Box::new(sh) }, "undo.pivot");
        if !recs.is_empty() {
            let area = Range::new(CellRef::new(0, 0), CellRef::new(recs.len() as u32, range.cols() - 1));
            let _ = self.create_table(index, area, true, crate::table::DEFAULT_STYLE);
        }
        let steps = self.undo.len().saturating_sub(before);
        self.merge_undo(steps, "undo.pivot");
        Ok(index)
    }

    // --- slicers ---------------------------------------------------------------

    /// Adds a slicer for source column `col` of pivot `index` at (x, y)
    /// sheet pixels of the pivot's sheet. Returns its index.
    pub fn add_slicer(&mut self, index: usize, col: u32, x: f32, y: f32) -> Result<usize, AnalysisError> {
        let p = self.analysis.pivots.get(index).cloned().ok_or(AnalysisError::NotFound)?;
        let caption = match &p.spec.model {
            Some(m) => m.cols.get(col as usize).ok_or(AnalysisError::BadRange)?.1.clone(),
            None => {
                let si = self.sheet_index(&p.spec.source_sheet).ok_or(AnalysisError::NotFound)?;
                let range = self.clamp_to_used(si, &p.spec.source).ok_or(AnalysisError::NoData)?;
                if col >= range.cols() {
                    return Err(AnalysisError::BadRange);
                }
                self.display(si, CellRef::new(range.start.row, range.start.col + col)).0
            }
        };
        let mut st = self.analysis.clone();
        if !st.pivots[index].spec.slicers.iter().any(|f| f.field.col == col) {
            st.pivots[index].spec.slicers.push(pivot::FieldFilter { field: pivot::Field::new(col), items: Vec::new() });
        }
        st.slicers.push(Slicer { sheet: p.sheet.clone(), x, y, width: 180.0, height: 220.0, pivot: index, col, caption, columns: 1 });
        let i = st.slicers.len() - 1;
        self.execute_as(Op::Analysis(Box::new(st)), "undo.slicer");
        Ok(i)
    }

    /// The items of a slicer's field and whether each is selected.
    pub fn slicer_items(&self, i: usize) -> Vec<(String, bool)> {
        let Some(s) = self.analysis.slicers.get(i) else { return Vec::new() };
        let Some(p) = self.analysis.pivots.get(s.pivot) else { return Vec::new() };
        let f = pivot::Field::new(s.col);
        let probe = PivotSpec { rows: vec![f.clone()], cols: Vec::new(), values: Vec::new(), filters: Vec::new(), slicers: Vec::new(), ..p.spec.clone() };
        let Ok(src) = self.pivot_source(&probe) else { return Vec::new() };
        let sel = p.spec.slicers.iter().find(|x| x.field.col == s.col).map(|x| x.items.clone()).unwrap_or_default();
        pivot::field_labels(&src, &f, p.spec.lang).into_iter().map(|l| {
            let on = sel.is_empty() || sel.iter().any(|x| x.to_lowercase() == l.to_lowercase());
            (l, on)
        }).collect()
    }

    /// Selects the items of a slicer (empty: all) and refreshes its pivot.
    pub fn set_slicer_selection(&mut self, i: usize, items: Vec<String>) -> Result<(), AnalysisError> {
        let s = self.analysis.slicers.get(i).cloned().ok_or(AnalysisError::NotFound)?;
        let mut spec = self.analysis.pivots.get(s.pivot).ok_or(AnalysisError::NotFound)?.spec.clone();
        match spec.slicers.iter_mut().find(|f| f.field.col == s.col) {
            Some(f) => f.items = items,
            None => spec.slicers.push(pivot::FieldFilter { field: pivot::Field::new(s.col), items }),
        }
        self.replace_pivot(s.pivot, spec)?;
        self.merge_undo(1, "undo.slicer");
        Ok(())
    }

    /// Moves, resizes or renames a slicer.
    pub fn set_slicer(&mut self, i: usize, s: Slicer) {
        if self.analysis.slicers.get(i).is_none_or(|x| *x == s) {
            return;
        }
        let mut st = self.analysis.clone();
        st.slicers[i] = s;
        self.execute_as(Op::Analysis(Box::new(st)), "undo.slicer");
    }

    /// Removes a slicer; its filter goes when no other slicer shares it.
    pub fn remove_slicer(&mut self, i: usize) {
        let Some(s) = self.analysis.slicers.get(i).cloned() else { return };
        let before = self.undo.len();
        let shared = self.analysis.slicers.iter().enumerate().any(|(k, o)| k != i && o.pivot == s.pivot && o.col == s.col);
        if !shared {
            if let Some(p) = self.analysis.pivots.get(s.pivot) {
                let mut spec = p.spec.clone();
                let n = spec.slicers.len();
                spec.slicers.retain(|f| f.field.col != s.col);
                if spec.slicers.len() != n {
                    let _ = self.replace_pivot(s.pivot, spec);
                }
            }
        }
        let mut st = self.analysis.clone();
        st.slicers.remove(i);
        self.execute_as(Op::Analysis(Box::new(st)), "undo.slicer");
        let steps = self.undo.len().saturating_sub(before);
        self.merge_undo(steps, "undo.slicer");
    }

    /// Refreshes every stored pivot table (one undo step).
    pub fn refresh_all_pivots(&mut self) {
        let before = self.undo.len();
        for i in 0..self.analysis.pivots.len() {
            let _ = self.refresh_pivot(i);
        }
        let steps = self.undo.len().saturating_sub(before);
        self.merge_undo(steps, "undo.pivot");
    }

    /// Clears a pivot table's cells and forgets its spec.
    pub fn delete_pivot(&mut self, index: usize) -> Result<(), AnalysisError> {
        let p = self.analysis.pivots.get(index).cloned().ok_or(AnalysisError::NotFound)?;
        let mut ops = Vec::new();
        if let (Some(sheet), Some(area)) = (self.sheet_index(&p.sheet), p.area) {
            let mut cells = Vec::new();
            self.sheets[sheet].for_each_in(&area, |c, _| cells.push((c, None)));
            ops.push(Op::SetCells { sheet, cells });
        }
        let mut st = self.analysis.clone();
        st.pivots.remove(index);
        st.slicers.retain(|s| s.pivot != index);
        for s in &mut st.slicers {
            if s.pivot > index {
                s.pivot -= 1;
            }
        }
        ops.push(Op::Analysis(Box::new(st)));
        self.execute_as(Op::Batch(ops), "undo.pivot");
        Ok(())
    }

    /// The stored pivot table whose output covers `c`.
    pub fn pivot_at(&self, sheet: usize, c: CellRef) -> Option<usize> {
        let name = &self.sheets.get(sheet)?.name;
        self.analysis.pivots.iter().position(|p| &p.sheet == name && p.area.is_some_and(|a| a.contains(c)))
    }

    /// GETPIVOTDATA: the value of data field `field` (caption or source
    /// column name) for the row and column items `pairs` names (outer
    /// fields first; none for the grand total), in the pivot table
    /// covering `c`. `None` when the table doesn't show such a value.
    pub fn pivot_value(&self, sheet: usize, c: CellRef, field: &str, pairs: &[(String, Value)]) -> Option<Value> {
        let at = self.pivot_at(sheet, c)?;
        let p = &self.analysis.pivots[at];
        let spec = &*pivot::bounded(&p.spec);
        // A table is worked out once per calculation, however many
        // formulas ask it.
        let key = (self as *const Self as usize, PIVOT_PASS.load(std::sync::atomic::Ordering::Relaxed), self.revision, at);
        let memo = PIVOT_MEMO.with(|m| m.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, v)| v.clone()));
        let memo = match memo {
            Some(v) => v,
            None => {
                let src = self.pivot_source(spec).ok()?;
                let t = pivot::compute(spec, &src);
                let v = std::sync::Arc::new((src, t));
                PIVOT_MEMO.with(|m| *m.borrow_mut() = Some((key, v.clone())));
                v
            }
        };
        let (src, t) = (&memo.0, &memo.1);
        let lc = |s: &str| s.trim().to_lowercase();
        let col_name = |c: u32| src.names.get(c as usize).map_or(String::new(), |n| lc(n));
        let vi = spec.values.iter().position(|v| lc(&pivot::caption(spec, &src, v)) == lc(field) || (v.col < pivot::CALC_BASE && col_name(v.col) == lc(field)))?;
        let fname = |f: &pivot::Field| f.name.as_deref().map_or_else(|| col_name(f.col), lc);
        let (mut rows, mut cols) = (vec![None; spec.rows.len()], vec![None; spec.cols.len()]);
        for (name, item) in pairs {
            let item = lc(&item.to_text().ok()?);
            if let Some(i) = spec.rows.iter().position(|f| fname(f) == lc(name)) {
                rows[i] = Some(item);
            } else if let Some(i) = spec.cols.iter().position(|f| fname(f) == lc(name)) {
                cols[i] = Some(item);
            } else {
                return None;
            }
        }
        // The items given must be those of the outer fields.
        let path = |v: Vec<Option<String>>| -> Option<Vec<String>> {
            let k = v.iter().take_while(|x| x.is_some()).count();
            if v[k..].iter().any(Option::is_some) { None } else { Some(v.into_iter().flatten().collect()) }
        };
        let (rp, cp) = (path(rows)?, path(cols)?);
        let same = |p: &Option<Vec<String>>, want: &[String]| p.as_ref().is_some_and(|p| p.len() == want.len() && p.iter().zip(want).all(|(a, b)| lc(a) == *b));
        let r = t.row_paths.iter().position(|p| same(p, &rp))?;
        let nv = spec.values.len();
        let x = (t.label_cols..t.col_paths.len()).find(|&x| same(&t.col_paths[x], &cp) && (x - t.label_cols) % nv == vi)?;
        let cell = t.get(r, x)?;
        if cell.kind == PivotCellKind::Blank {
            return None;
        }
        // What the sheet shows (as of the last refresh) when it fits.
        let area = p.area?;
        if area.rows() as usize == t.rows() && area.cols() as usize == t.cols() {
            return Some(self.sheets[sheet].value(CellRef::new(area.start.row + r as u32, area.start.col + x as u32)).clone());
        }
        Some(cell.value.clone())
    }

    // --- Flash Fill ------------------------------------------------------------

    /// Flash Fill (Ctrl+E) for the column of `c`: learns from the filled
    /// cells of that column in the current region, with the region's other
    /// columns as inputs, and fills its empty cells. Returns how many cells
    /// were filled (one undo step).
    pub fn flash_fill(&mut self, sheet: usize, c: CellRef) -> Result<usize, AnalysisError> {
        self.check_sheet(sheet)?;
        let mut region = self.current_region(sheet, c);
        // The output column is often still empty below the examples.
        if region.cols() == 1 && c.col > 0 {
            region = self.current_region(sheet, CellRef::new(c.row, c.col - 1));
            region.end.col = region.end.col.max(c.col);
        }
        if !region.contains(c) || region.cols() < 2 {
            return Err(AnalysisError::NoData);
        }
        let inputs: Vec<u32> = (region.start.col..=region.end.col).filter(|x| *x != c.col).collect();
        let text = |wb: &Workbook, row: u32, col: u32| wb.display(sheet, CellRef::new(row, col)).0;
        let mut examples = Vec::new();
        let mut todo = Vec::new();
        for row in region.start.row..=region.end.row {
            let ins: Vec<String> = inputs.iter().map(|&col| text(self, row, col)).collect();
            if ins.iter().all(String::is_empty) {
                continue;
            }
            let out = text(self, row, c.col);
            if out.is_empty() {
                todo.push((row, ins));
            } else {
                examples.push((row, ins, out));
            }
        }
        if examples.is_empty() || todo.is_empty() {
            return Err(AnalysisError::NoData);
        }
        // A handful of examples is plenty (the search grows with them).
        let ex = |skip_first: bool| -> Vec<(Vec<String>, String)> { examples.iter().skip(skip_first as usize).take(12).map(|(_, i, o)| (i.clone(), o.clone())).collect() };
        // A header row is not an example: retry without the first row.
        let program = flash::synthesize(&ex(false)).or_else(|| if examples[0].0 == region.start.row && examples.len() > 1 { flash::synthesize(&ex(true)) } else { None });
        let program = program.ok_or(AnalysisError::NoPattern)?;
        let mut cells = Vec::new();
        for (row, ins) in todo {
            if let Some(v) = program.apply(&ins).filter(|s| !s.is_empty()) {
                let at = CellRef::new(row, c.col);
                cells.push((at, Cell::with_content(self.sheets[sheet].get(at), Content::Value(Value::text(v)))));
            }
        }
        let n = cells.len();
        if n == 0 {
            return Err(AnalysisError::NoPattern);
        }
        self.execute_as(Op::SetCells { sheet, cells }, "undo.flash_fill");
        Ok(n)
    }

    // --- custom lists ----------------------------------------------------------

    /// Replaces the user's custom lists (one undo step).
    pub fn set_custom_lists(&mut self, lists: Vec<Vec<String>>) {
        let mut st = self.analysis.clone();
        st.custom_lists = lists.into_iter().filter(|l| !l.is_empty()).collect();
        self.set_state(st, "undo.custom_lists");
    }
}

/// Right side of a Solver constraint.
enum Rhs {
    Num(f64),
    Expr(Box<Expr>),
}

/// Puts a pivot table's chart data into a chart as literal series, keeping
/// the look of the series that stay.
fn fill_pivot_chart(chart: &mut crate::chart::Chart, d: &pivot::PivotChartData) {
    use crate::chart::DataRef;
    let old = std::mem::take(&mut chart.series);
    let cats = DataRef::literal(d.categories.iter().map(|c| Value::text(c.as_str())).collect());
    for (k, (name, vals)) in d.series.iter().enumerate().take(crate::chart::MAX_SERIES) {
        let mut s = old.get(k).cloned().unwrap_or_default();
        s.name = DataRef::text(name);
        s.categories = cats.clone();
        s.values = DataRef::literal(vals.iter().map(|v| v.map_or(Value::Empty, Value::Number)).collect());
        chart.series.push(s);
    }
    chart.source = None;
}

/// The cell style of a pivot cell under the table's style.
fn pivot_style(style: &str, kind: PivotCellKind, indent: u8, fmt: Option<&str>) -> CellStyle {
    let l = pivot::style_look(style, kind);
    let mut s = CellStyle { fill: l.fill, color: l.color, bold: l.bold, ..CellStyle::default() };
    if let Some(c) = l.top {
        s.borders.top = crate::sheet::Line::new(crate::sheet::LineStyle::Thin, Some(c));
    }
    if let Some(c) = l.bottom {
        s.borders.bottom = crate::sheet::Line::new(crate::sheet::LineStyle::Thin, Some(c));
    }
    s.indent = indent;
    if let Some(f) = fmt {
        s.num_fmt = Arc::from(f);
    }
    s
}

/// Secant search with a bracketing (Illinois) fallback for `f(x) = 0`.
/// Returns the best point and its value.
fn seek(mut f: impl FnMut(f64) -> f64, x0: f64, max_iter: usize, tol: f64) -> (f64, f64) {
    let mut best = (x0, f(x0));
    let better = |b: &mut (f64, f64), x: f64, fx: f64| {
        if fx.is_finite() && (!b.1.is_finite() || fx.abs() < b.1.abs()) {
            *b = (x, fx);
        }
    };
    if best.1.is_finite() && best.1.abs() <= tol {
        return best;
    }
    let (mut a, mut fa) = best;
    let mut b = if x0 == 0.0 { 0.01 } else { x0 * 1.01 };
    let mut fb = f(b);
    better(&mut best, b, fb);
    let mut bracket: Option<(f64, f64, f64, f64)> = None;
    let mut side = 0i8;
    for _ in 2..max_iter {
        if best.1.abs() <= tol {
            break;
        }
        if bracket.is_none() && fa.is_finite() && fb.is_finite() && fa.signum() != fb.signum() {
            bracket = Some((a, fa, b, fb));
        }
        let next = match bracket {
            Some((lo, flo, hi, fhi)) => {
                let x = hi - fhi * (hi - lo) / (fhi - flo);
                if x.is_finite() && x > lo.min(hi) && x < lo.max(hi) { x } else { (lo + hi) / 2.0 }
            }
            None if fa.is_finite() && fb.is_finite() && fa != fb => {
                let x = b - fb * (b - a) / (fb - fa);
                // Wild secant steps are cut to a few times the last step.
                let lim = 100.0 * (b - a).abs().max(1e-9) + 1.0;
                if x.is_finite() { b + (x - b).clamp(-lim, lim) } else { b + 2.0 * (b - a) }
            }
            None if !fb.is_finite() => (a + b) / 2.0,
            None => b + 2.0 * (b - a),
        };
        let fnext = f(next);
        better(&mut best, next, fnext);
        match &mut bracket {
            Some((lo, flo, hi, fhi)) => {
                if !fnext.is_finite() {
                    // Shrink towards the finite side.
                    *hi = next;
                } else if fnext.signum() == fhi.signum() {
                    *hi = next;
                    *fhi = fnext;
                    if side == 1 {
                        *flo /= 2.0;
                    }
                    side = 1;
                } else {
                    *lo = next;
                    *flo = fnext;
                    if side == -1 {
                        *fhi /= 2.0;
                    }
                    side = -1;
                }
                if (*hi - *lo).abs() <= 1e-15 * (1.0 + hi.abs()) {
                    break;
                }
            }
            None => {
                a = b;
                fa = fb;
                b = next;
                fb = fnext;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::pivot::{Field, ValueField};
    use crate::analysis::solver::{Constraint, SolverGoal, SolverMethod};
    use crate::analysis::stats;

    fn wb(rows: &[&[&str]]) -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let mut inputs = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, t) in row.iter().enumerate() {
                if !t.is_empty() {
                    inputs.push((CellRef::new(r as u32, c as u32), t.to_string()));
                }
            }
        }
        w.set_inputs(0, inputs);
        w.undo.clear();
        w
    }

    fn a1(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    fn rg(s: &str) -> Range {
        Range::parse(s).unwrap()
    }

    fn num(w: &Workbook, s: usize, c: &str) -> f64 {
        w.sheets[s].value(a1(c)).as_number().unwrap_or(f64::NAN)
    }

    #[test]
    fn goal_seek_loan() {
        // Payment of a loan; find the rate that gives a payment of -1000.
        let mut w = wb(&[&["0.05", "=PMT(A1/12,360,200000)"]]);
        let r = w.goal_seek_preview(0, a1("B1"), -1000.0, a1("A1"), 100, 0.001).unwrap();
        assert!(r.found, "{r:?}");
        assert!((r.achieved + 1000.0).abs() <= 0.001);
        assert_eq!(num(&w, 0, "A1"), 0.05);
        assert!(!w.can_undo());
        let r = w.goal_seek(0, a1("B1"), -1000.0, a1("A1"), 100, 0.001).unwrap();
        assert!((num(&w, 0, "A1") - r.value).abs() < 1e-12);
        assert!((num(&w, 0, "B1") + 1000.0).abs() <= 0.001);
        assert_eq!(w.undo_history(), vec!["undo.goal_seek"]);
        w.undo();
        assert_eq!(num(&w, 0, "A1"), 0.05);
        // Quadratic from zero, and errors.
        let mut w = wb(&[&["", "=A1*A1-2"]]);
        let r = w.goal_seek(0, a1("B1"), 0.0, a1("A1"), 100, 1e-9).unwrap();
        assert!((r.value.abs() - 2f64.sqrt()).abs() < 1e-6, "{r:?}");
        assert_eq!(w.goal_seek(0, a1("A1"), 0.0, a1("B1"), 100, 0.001), Err(AnalysisError::NotAFormula(a1("A1"))));
        assert_eq!(w.goal_seek(0, a1("B1"), 0.0, a1("B1"), 100, 0.001), Err(AnalysisError::NotAValue(a1("B1"))));
        // Unreachable target: best effort, not found.
        let mut w = wb(&[&["1", "=A1*A1"]]);
        let r = w.goal_seek_preview(0, a1("B1"), -5.0, a1("A1"), 50, 0.001).unwrap();
        assert!(!r.found);
    }

    #[test]
    fn data_tables() {
        // One variable, inputs down column D, formula in E1.
        let mut w = wb(&[&["2", "=A1*10", "", "", "=B1+1"], &["", "", "", "1"], &["", "", "", "5"]]);
        let n = w.data_table(0, &rg("D1:E3"), None, Some(a1("A1"))).unwrap();
        assert_eq!(n, 2);
        assert_eq!(num(&w, 0, "E2"), 11.0);
        assert_eq!(num(&w, 0, "E3"), 51.0);
        assert_eq!(num(&w, 0, "A1"), 2.0);
        assert_eq!(num(&w, 0, "E1"), 21.0);
        assert_eq!(w.undo_history(), vec!["undo.data_table"]);
        // Two variables: formula at the corner.
        let mut w = wb(&[&["1", "1", "=A1*B1"], &[], &["=C1", "2", "3"], &["4"], &["5"]]);
        w.data_table(0, &rg("A3:C5"), Some(a1("A1")), Some(a1("B1"))).unwrap();
        assert_eq!(num(&w, 0, "B4"), 8.0);
        assert_eq!(num(&w, 0, "C5"), 15.0);
        assert_eq!(num(&w, 0, "C1"), 1.0);
        // Row-oriented.
        let mut w = wb(&[&["3", "=A1+100"], &[], &["", "1", "2"], &["=B1"]]);
        w.data_table(0, &rg("A3:C4"), Some(a1("A1")), None).unwrap();
        assert_eq!(num(&w, 0, "B4"), 101.0);
        assert_eq!(num(&w, 0, "C4"), 102.0);
        assert_eq!(w.data_table(0, &rg("A3:A4"), Some(a1("A1")), None), Err(AnalysisError::BadRange));
    }

    #[test]
    fn scenarios_and_summary() {
        let mut w = wb(&[&["10", "20", "=A1+B1"]]);
        w.add_scenario(0, Scenario { name: "Best".into(), changing: vec![a1("A1"), a1("B1")], values: vec!["100".into(), "200".into()], comment: String::new(), ..Default::default() }).unwrap();
        w.add_scenario(0, Scenario { name: "Worst".into(), changing: vec![a1("A1")], values: vec!["1".into()], comment: "low".into(), ..Default::default() }).unwrap();
        assert_eq!(w.scenarios(0).len(), 2);
        w.show_scenario(0, "best").unwrap();
        assert_eq!(num(&w, 0, "C1"), 300.0);
        assert_eq!(w.undo_history()[0], "undo.scenario");
        assert_eq!(w.analysis.scenario_info["Sheet1"].current, Some(0));
        let steps = w.undo_history().len();
        w.undo();
        assert_eq!(num(&w, 0, "C1"), 30.0);
        // Values and the current scenario go back in one step.
        assert_eq!(w.undo_history().len(), steps - 1);
        assert!(w.analysis.scenario_info.get("Sheet1").is_none_or(|i| i.current.is_none()));
        let s = w.scenario_summary(0, &[a1("C1")], Lang::En).unwrap();
        assert_eq!(w.sheets[s].name, "Scenario Summary");
        assert_eq!(w.display(s, a1("C2")).0, "Current Values:");
        assert_eq!(w.display(s, a1("D2")).0, "Best");
        assert_eq!(w.display(s, a1("B4")).0, "$A$1");
        assert_eq!(num(&w, s, "C4"), 10.0);
        assert_eq!(num(&w, s, "D4"), 100.0);
        assert_eq!(num(&w, s, "E5"), 20.0);
        // Result row: C1 under each scenario.
        assert_eq!(num(&w, s, "C7"), 30.0);
        assert_eq!(num(&w, s, "D7"), 300.0);
        assert_eq!(num(&w, s, "E7"), 21.0);
        assert_eq!(num(&w, 0, "C1"), 30.0);
        w.undo();
        assert_eq!(w.sheets.len(), 1);
        w.delete_scenario(0, "Worst").unwrap();
        assert_eq!(w.scenarios(0).len(), 1);
        w.undo();
        assert_eq!(w.scenarios(0).len(), 2);
    }

    #[test]
    fn solver_product_mix() {
        // Products in B2:C2; profit in D1; resources in D3:D4 <= E3:E4.
        let mut w = wb(&[
            &["", "Tables", "Chairs", "=SUMPRODUCT(B2:C2,B5:C5)"],
            &["Qty", "0", "0"],
            &["Wood", "4", "3", "=SUMPRODUCT($B$2:$C$2,B3:C3)", "240"],
            &["Labor", "2", "1", "=SUMPRODUCT($B$2:$C$2,B4:C4)", "100"],
            &["Profit", "70", "50"],
        ]);
        let model = SolverModel {
            objective: a1("D1"),
            goal: SolverGoal::Max,
            variables: vec![rg("B2:C2")],
            constraints: vec![Constraint { lhs: rg("D3:D4"), op: ConstraintOp::Le, rhs: "E3:E4".into() }],
            method: SolverMethod::SimplexLp,
            ..Default::default()
        };
        let r = w.solve(0, &model);
        assert_eq!(r.status, SolverStatus::Optimal, "{r:?}");
        assert!((num(&w, 0, "B2") - 30.0).abs() < 1e-9 && (num(&w, 0, "C2") - 40.0).abs() < 1e-9);
        assert!((num(&w, 0, "D1") - 4100.0).abs() < 1e-9);
        assert_eq!(w.undo_history(), vec!["undo.solver"]);
        w.undo();
        assert_eq!(num(&w, 0, "B2"), 0.0);
        // The same model through the nonlinear method.
        let r = w.solve_preview(0, &SolverModel { method: SolverMethod::Nonlinear, seed: Some(3), ..model.clone() });
        assert!(r.is_solution(), "{r:?}");
        assert!((r.objective - 4100.0).abs() < 0.1, "{r:?}");
        assert_eq!(num(&w, 0, "B2"), 0.0);
        // Refused: a variable holding a formula.
        let bad = SolverModel { variables: vec![rg("D1")], ..model };
        assert_eq!(w.solve(0, &bad).status, SolverStatus::BadModel);
    }

    #[test]
    fn solver_knapsack_and_transport() {
        let mut w = wb(&[
            &["0", "0", "0", "0", "=SUMPRODUCT(A1:D1,A2:D2)", "=SUMPRODUCT(A1:D1,A3:D3)"],
            &["10", "13", "7", "8"],
            &["5", "7", "4", "3"],
        ]);
        let model = SolverModel {
            objective: a1("E1"),
            goal: SolverGoal::Max,
            variables: vec![rg("A1:D1")],
            constraints: vec![Constraint { lhs: rg("F1"), op: ConstraintOp::Le, rhs: "10".into() }, Constraint { lhs: rg("A1:D1"), op: ConstraintOp::Bin, rhs: String::new() }],
            method: SolverMethod::SimplexLp,
            ..Default::default()
        };
        let r = w.solve(0, &model);
        assert_eq!(r.status, SolverStatus::Optimal);
        assert_eq!(r.values, vec![0.0, 1.0, 0.0, 1.0]);
        assert_eq!(num(&w, 0, "E1"), 21.0);

        // Transport: 2 plants (supply 20, 30) x 3 markets (demand 10, 25, 15).
        let mut w = wb(&[
            &["0", "0", "0", "=SUM(A1:C1)", "20"],
            &["0", "0", "0", "=SUM(A2:C2)", "30"],
            &["=SUM(A1:A2)", "=SUM(B1:B2)", "=SUM(C1:C2)"],
            &["10", "25", "15"],
            &["8", "6", "10", "=SUMPRODUCT(A1:C2,A5:C6)"],
            &["9", "12", "13"],
        ]);
        let model = SolverModel {
            objective: a1("D5"),
            goal: SolverGoal::Min,
            variables: vec![rg("A1:C2")],
            constraints: vec![Constraint { lhs: rg("D1:D2"), op: ConstraintOp::Le, rhs: "E1:E2".into() }, Constraint { lhs: rg("A3:C3"), op: ConstraintOp::Ge, rhs: "A4:C4".into() }],
            method: SolverMethod::SimplexLp,
            ..Default::default()
        };
        let r = w.solve(0, &model);
        assert_eq!(r.status, SolverStatus::Optimal, "{r:?}");
        // Optimal cost: plant 1 sends 20 to market 2; plant 2 sends 10, 5, 15.
        assert!((r.objective - (20.0 * 6.0 + 10.0 * 9.0 + 5.0 * 12.0 + 15.0 * 13.0)).abs() < 1e-6, "{r:?}");
        // Nonlinear model refused by Simplex LP.
        let mut w = wb(&[&["1", "=A1*A1"]]);
        let m = SolverModel { objective: a1("B1"), goal: SolverGoal::Min, variables: vec![rg("A1")], method: SolverMethod::SimplexLp, ..Default::default() };
        assert_eq!(w.solve(0, &m).status, SolverStatus::NotLinear);
    }

    #[test]
    fn solver_nonlinear() {
        // Rosenbrock.
        let mut w = wb(&[&["-1.2", "1", "=(1-A1)^2+100*(B1-A1^2)^2"]]);
        let m = SolverModel { objective: a1("C1"), goal: SolverGoal::Min, variables: vec![rg("A1:B1")], non_negative: false, ..Default::default() };
        let r = w.solve(0, &m);
        assert!(r.is_solution(), "{r:?}");
        assert!((num(&w, 0, "A1") - 1.0).abs() < 1e-3 && (num(&w, 0, "B1") - 1.0).abs() < 1e-3, "{r:?}");
        // Portfolio: minimize variance with a return target, weights sum 1.
        let mut w = wb(&[
            &["0.3", "0.3", "0.4", "=SUM(A1:C1)", "=SUMPRODUCT(A1:C1,A2:C2)", "=MMULT(MMULT(A1:C1,A3:C5),TRANSPOSE(A1:C1))"],
            &["0.1", "0.15", "0.07"],
            &["0.04", "0.006", "0.002"],
            &["0.006", "0.09", "0.001"],
            &["0.002", "0.001", "0.01"],
        ]);
        let m = SolverModel {
            objective: a1("F1"),
            goal: SolverGoal::Min,
            variables: vec![rg("A1:C1")],
            constraints: vec![Constraint { lhs: rg("D1"), op: ConstraintOp::Eq, rhs: "1".into() }, Constraint { lhs: rg("E1"), op: ConstraintOp::Ge, rhs: "0.1".into() }],
            ..Default::default()
        };
        let r = w.solve(0, &m);
        assert!(r.is_solution(), "{r:?}");
        let x = [num(&w, 0, "A1"), num(&w, 0, "B1"), num(&w, 0, "C1")];
        assert!((x.iter().sum::<f64>() - 1.0).abs() < 1e-5);
        assert!(num(&w, 0, "E1") >= 0.1 - 1e-5);
        assert!(x.iter().all(|v| *v >= -1e-9));
        // No grid point does better (coarse check of optimality).
        let cov = [[0.04, 0.006, 0.002], [0.006, 0.09, 0.001], [0.002, 0.001, 0.01]];
        let var = |w: &[f64; 3]| (0..3).map(|i| (0..3).map(|j| w[i] * cov[i][j] * w[j]).sum::<f64>()).sum::<f64>();
        let mut best = f64::INFINITY;
        for i in 0..=200 {
            for j in 0..=(200 - i) {
                let p = [i as f64 / 200.0, j as f64 / 200.0, (200 - i - j) as f64 / 200.0];
                if 0.1 * p[0] + 0.15 * p[1] + 0.07 * p[2] >= 0.1 {
                    best = best.min(var(&p));
                }
            }
        }
        assert!(r.objective <= best + 1e-6, "{} vs {}", r.objective, best);
    }

    #[test]
    fn statistics_output() {
        let mut w = wb(&[&["Height", "Weight"], &["170", "65"], &["180", "80"], &["165", "60"], &["x", "70"]]);
        let s = w.read_series(0, &rg("A1:B5"), false, true);
        assert_eq!(s[0].label.as_deref(), Some("Height"));
        assert_eq!(s[0].values, vec![Some(170.0), Some(180.0), Some(165.0), None]);
        let t = stats::descriptive(&s, &Default::default(), Lang::En).unwrap();
        let (sheet, area) = w.write_table(&t, &Output::NewSheet("Stats".into()), "undo.analysis_tool").unwrap();
        assert_eq!(w.sheets[sheet].name, "Stats");
        assert_eq!(w.display(sheet, a1("A3")).0, "Mean");
        assert_eq!(area.cols(), 4);
        assert_eq!(w.undo_history(), vec!["undo.analysis_tool"]);
        // Into a range, with a percent format.
        let t = stats::histogram(&w.read_numbers(0, &rg("B2:B5")), Some(&[65.0, 75.0]), None, &stats::HistogramOpts { pareto: false, cumulative: true }, Lang::En).unwrap();
        w.write_table(&t, &Output::At { sheet: 0, cell: a1("D1") }, "undo.analysis_tool").unwrap();
        assert_eq!(w.display(0, a1("E2")).0, "2");
        assert_eq!(w.display(0, a1("F2")).0, "50.00%");
        assert!(w.output_overwrites(0, a1("D1"), 1, 1));
        w.undo();
        assert!(!w.output_overwrites(0, a1("D1"), 3, 3));
    }

    #[test]
    fn pivot_create_refresh_undo() {
        let mut w = wb(&[&["Region", "Sales"], &["East", "10"], &["West", "5"], &["East", "7"]]);
        let spec = PivotSpec { source_sheet: "Sheet1".into(), source: rg("A1:B1048576"), rows: vec![Field::new(0)], values: vec![ValueField { col: 1, ..Default::default() }], ..Default::default() };
        let t = w.pivot_compute(&spec).unwrap();
        assert_eq!(t.rows(), 4);
        let i = w.create_pivot(spec.clone(), Some((0, a1("D1")))).unwrap();
        assert_eq!(w.display(0, a1("D1")).0, "Row Labels");
        assert_eq!(w.display(0, a1("E2")).0, "17");
        assert_eq!(w.display(0, a1("E4")).0, "22");
        assert!(w.styles.get(w.cell(0, a1("D4")).unwrap().style).bold);
        assert_eq!(w.pivot_at(0, a1("E3")), Some(i));
        // Refresh after editing the source.
        w.set_input(0, a1("B3"), "50");
        w.refresh_pivot(i).unwrap();
        assert_eq!(w.display(0, a1("E3")).0, "50");
        w.undo();
        assert_eq!(w.display(0, a1("E3")).0, "5");
        // A new source row appears (whole-column source).
        w.set_input(0, a1("A5"), "North");
        w.set_input(0, a1("B5"), "1");
        w.refresh_pivot(i).unwrap();
        assert_eq!(w.display(0, a1("D4")).0, "West");
        assert_eq!(w.display(0, a1("E5")).0, "68");
        assert_eq!(w.analysis.pivots[i].area.map(|a| a.rows()), Some(5));
        // New-sheet pivot and undo of the creation.
        let j = w.create_pivot(spec, None).unwrap();
        assert_eq!(w.sheets.len(), 2);
        assert_eq!(w.display(1, a1("A3")).0, "Row Labels");
        w.undo();
        assert_eq!(w.sheets.len(), 1);
        assert_eq!(w.analysis.pivots.len(), j);
        w.delete_pivot(i).unwrap();
        assert_eq!(w.display(0, a1("D1")).0, "");
        assert!(w.analysis.pivots.is_empty());
        w.undo();
        assert_eq!(w.analysis.pivots.len(), 1);
    }

    #[test]
    fn pivot_details_chart_slicers() {
        let mut w = wb(&[&["Region", "Product", "Sales"], &["East", "Pen", "10"], &["West", "Ink", "5"], &["East", "Ink", "7"], &["West", "Pen", "2"]]);
        let spec = PivotSpec { source_sheet: "Sheet1".into(), source: rg("A1:C5"), rows: vec![Field::new(0)], values: vec![ValueField { col: 2, ..Default::default() }], ..Default::default() };
        let i = w.create_pivot(spec, Some((0, a1("F1")))).unwrap();
        assert_eq!(w.display(0, a1("G2")).0, "17");
        // Show details of East.
        let d = w.pivot_details(0, a1("G2")).unwrap();
        assert_eq!(w.sheets[d].name, "Details1");
        assert_eq!(w.display(d, a1("A1")).0, "Region");
        assert_eq!(w.display(d, a1("B3")).0, "Ink");
        assert_eq!(w.display(d, a1("C2")).0, "10");
        assert_eq!(w.display(d, a1("A4")).0, "");
        assert!(w.pivot_details(0, a1("F2")).is_err());
        w.undo();
        assert_eq!(w.sheets.len(), 1);
        // A pivot chart follows the table.
        let id = w.create_pivot_chart(i, crate::chart::ChartType::Column).unwrap();
        let c = w.sheets[0].charts.iter().find(|c| c.id == id).unwrap().clone();
        assert_eq!(w.chart_data(0, &c).categories, ["East", "West"]);
        assert_eq!(w.analysis.pivots[i].charts, vec![id]);
        // A slicer on Product filters table and chart.
        let k = w.add_slicer(i, 1, 400.0, 10.0).unwrap();
        assert_eq!(w.analysis.slicers[k].caption, "Product");
        assert_eq!(w.slicer_items(k), vec![("Ink".to_string(), true), ("Pen".to_string(), true)]);
        w.set_slicer_selection(k, vec!["Pen".into()]).unwrap();
        assert_eq!(w.display(0, a1("G2")).0, "10");
        assert_eq!(w.display(0, a1("G4")).0, "12");
        assert_eq!(w.slicer_items(k), vec![("Ink".to_string(), false), ("Pen".to_string(), true)]);
        let c = w.sheets[0].charts.iter().find(|c| c.id == id).unwrap().clone();
        assert_eq!(w.chart_data(0, &c).series[0].values, [Some(10.0), Some(2.0)]);
        w.undo();
        assert_eq!(w.display(0, a1("G2")).0, "17");
        w.redo();
        w.remove_slicer(k);
        assert!(w.analysis.slicers.is_empty());
        assert_eq!(w.display(0, a1("G2")).0, "17");
        w.undo();
        assert_eq!(w.analysis.slicers.len(), 1);
        assert_eq!(w.display(0, a1("G2")).0, "10");
        w.delete_pivot(i).unwrap();
        assert!(w.analysis.slicers.is_empty());
    }

    #[test]
    fn get_pivot_data() {
        let mut w = wb(&[&["Region", "Product", "Sales"], &["East", "Pen", "10"], &["West", "Ink", "5"], &["East", "Ink", "7"], &["West", "Pen", "2"]]);
        let spec = PivotSpec { source_sheet: "Sheet1".into(), source: rg("A1:C5"), rows: vec![Field::new(0)], cols: vec![Field::new(1)], values: vec![ValueField { col: 2, ..Default::default() }], ..Default::default() };
        let i = w.create_pivot(spec, Some((0, a1("F1")))).unwrap();
        let f = |w: &Workbook, args: &str| w.eval_formula(0, &format!("=GETPIVOTDATA({args})"));
        assert_eq!(f(&w, "\"Sales\",F1"), Value::Number(24.0));
        assert_eq!(f(&w, "\"Sum of Sales\",$G$4,\"Region\",\"east\""), Value::Number(17.0));
        assert_eq!(f(&w, "\"Sales\",F1,\"Product\",\"Ink\",\"Region\",\"West\""), Value::Number(5.0));
        assert_eq!(f(&w, "\"Sales\",F1,\"Product\",\"Pen\""), Value::Number(12.0));
        assert_eq!(f(&w, "\"Sales\",F1,\"Region\",\"North\""), Value::Error(ErrorKind::Ref));
        assert_eq!(f(&w, "\"Sales\",F1,\"Nope\",\"East\""), Value::Error(ErrorKind::Ref));
        assert_eq!(f(&w, "\"Units\",F1"), Value::Error(ErrorKind::Ref));
        assert_eq!(f(&w, "\"Sales\",A1"), Value::Error(ErrorKind::Ref));
        // A formula follows the table when it is refreshed.
        w.set_input(0, a1("K1"), "=GETPIVOTDATA(\"Sales\",F1,\"Region\",\"West\")");
        assert_eq!(num(&w, 0, "K1"), 7.0);
        w.set_input(0, a1("C3"), "50");
        w.refresh_pivot(i).unwrap();
        assert_eq!(num(&w, 0, "K1"), 52.0);
    }

    /// `cargo test --release -- --ignored pivot_100k` prints the time.
    #[test]
    #[ignore]
    fn pivot_100k_timing() {
        let mut w = Workbook::new();
        let n = 100_000u32;
        for (c, h) in ["Region", "Product", "Date", "Sales"].iter().enumerate() {
            w.load_cell(0, CellRef::new(0, c as u32), Content::Value(Value::text(*h)), 0);
        }
        let regions = ["North", "South", "East", "West", "Center"].map(Value::text);
        for i in 0..n {
            let r = i + 1;
            w.load_cell(0, CellRef::new(r, 0), Content::Value(regions[(i % 5) as usize].clone()), 0);
            w.load_cell(0, CellRef::new(r, 1), Content::Value(Value::text(format!("P{}", (i * 7) % 40))), 0);
            w.load_cell(0, CellRef::new(r, 2), Content::Value(Value::Number(44927.0 + (i % 730) as f64)), 0);
            w.load_cell(0, CellRef::new(r, 3), Content::Value(Value::Number((i % 97) as f64)), 0);
        }
        w.finish_load();
        let mut y = Field::new(2);
        y.group = crate::analysis::pivot::Grouping::Date(crate::analysis::pivot::DatePart::Years);
        let spec = PivotSpec { source_sheet: "Sheet1".into(), source: rg("A1:D1048576"), rows: vec![Field::new(0), Field::new(1)], cols: vec![y], values: vec![ValueField { col: 3, ..Default::default() }], ..Default::default() };
        let t0 = std::time::Instant::now();
        let src = w.pivot_source(&spec).unwrap();
        let read = t0.elapsed();
        let t = crate::analysis::pivot::compute(&spec, &src);
        let computed = t0.elapsed();
        w.create_pivot(spec, None).unwrap();
        eprintln!("pivot 100k: read {:?}, read+compute {:?}, all+write {:?}, {} rows", read, computed, t0.elapsed(), t.rows());
    }

    #[test]
    fn flash_fill_column() {
        let mut w = wb(&[&["Full name", "First"], &["John Smith", "John"], &["Ana Lopez", ""], &["Bob Stone", ""]]);
        let n = w.flash_fill(0, a1("B3")).unwrap();
        assert_eq!(n, 2);
        assert_eq!(w.display(0, a1("B3")).0, "Ana");
        assert_eq!(w.display(0, a1("B4")).0, "Bob");
        assert_eq!(w.undo_history(), vec!["undo.flash_fill"]);
        let mut w = wb(&[&["a"], &["b"]]);
        assert!(w.flash_fill(0, a1("B1")).is_err());
    }

    #[test]
    fn custom_lists_state() {
        let mut w = wb(&[&["x"]]);
        w.set_custom_lists(vec![vec!["Low".into(), "Mid".into(), "High".into()]]);
        assert_eq!(w.analysis.custom_lists.len(), 1);
        // The fill handle follows built-in and user lists.
        w.set_input(0, a1("B1"), "Mon");
        w.set_input(0, a1("C1"), "mid");
        w.set_input(0, a1("D1"), "Jan");
        w.set_input(0, a1("D2"), "Apr");
        w.fill_series(0, &rg("B1:C1"), &rg("B1:C3"));
        assert_eq!(w.display(0, a1("B3")).0, "Wed");
        assert_eq!(w.display(0, a1("C2")).0, "high");
        assert_eq!(w.display(0, a1("C3")).0, "low");
        w.fill_series(0, &rg("D1:D2"), &rg("D1:D4"));
        assert_eq!(w.display(0, a1("D4")).0, "Oct");
        // Other text still repeats.
        w.fill_series(0, &rg("A1"), &rg("A1:A2"));
        assert_eq!(w.display(0, a1("A2")).0, "x");
        while w.undo() {}
        assert!(w.analysis.custom_lists.is_empty());
    }
}
