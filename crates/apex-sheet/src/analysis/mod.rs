//! What-if analysis, Solver, statistics tools, pivot tables, Flash Fill
//! and custom lists: the engines behind the Data > Analysis dialogs.
//!
//! The modules here are pure logic over numbers, strings and grids of
//! values; the `Workbook` methods that read and write cells (one undo step
//! per command) live in `workbook/analysis.rs`.

pub mod ets;
pub mod flash;
pub mod labels;
pub mod lists;
pub mod pivot;
pub mod solver;
pub mod stats;

pub(crate) mod lp;
pub(crate) mod nlp;
pub(crate) mod numeric;

use std::collections::BTreeMap;

use crate::addr::{CellRef, Range};
use crate::value::Value;

pub use flash::flash_fill;
pub use labels::Lang;
pub use pivot::{PivotCell, PivotCellKind, PivotSpec, PivotTable};
pub use solver::{Constraint, ConstraintOp, SolverGoal, SolverMethod, SolverModel, SolverResult, SolverStatus};

/// Analysis data a workbook carries besides its cells. It changes through
/// `Op::Analysis`, so every edit is undoable like any other.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Scenarios of each sheet, keyed by sheet name.
    pub scenarios: BTreeMap<String, Vec<Scenario>>,
    /// Scenario Manager state of each sheet, keyed by sheet name.
    pub scenario_info: BTreeMap<String, ScenarioInfo>,
    /// Pivot tables written into the workbook, to refresh or edit them.
    pub pivots: Vec<StoredPivot>,
    /// User-defined fill lists (the built-in ones are in [`lists`]).
    pub custom_lists: Vec<Vec<String>>,
    /// The last Solver model of each sheet, keyed by sheet name.
    pub solver_models: BTreeMap<String, SolverModel>,
    /// Slicers of the pivot tables.
    pub slicers: Vec<Slicer>,
    /// Relationships between tables and measures (see [`crate::model`]).
    pub model: crate::model::DataModel,
}

impl State {
    /// Follows a sheet rename (scenario keys and pivot locations).
    pub fn rename_sheet(&mut self, old: &str, new: &str) {
        if let Some(v) = self.scenarios.remove(old) {
            self.scenarios.insert(new.to_string(), v);
        }
        if let Some(v) = self.scenario_info.remove(old) {
            self.scenario_info.insert(new.to_string(), v);
        }
        if let Some(v) = self.solver_models.remove(old) {
            self.solver_models.insert(new.to_string(), v);
        }
        for p in &mut self.pivots {
            if p.sheet == old {
                p.sheet = new.to_string();
            }
            if p.spec.source_sheet == old {
                p.spec.source_sheet = new.to_string();
            }
        }
        for s in &mut self.slicers {
            if s.sheet == old {
                s.sheet = new.to_string();
            }
        }
    }
}

/// A named set of values for some input cells (Scenario Manager).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scenario {
    pub name: String,
    pub changing: Vec<CellRef>,
    /// Inputs as typed (numbers, text or formulas), one per changing cell.
    pub values: Vec<String>,
    pub comment: String,
    /// Protected when the sheet is (Excel's "Prevent changes").
    pub locked: bool,
    /// Left out of the list when the sheet is protected.
    pub hidden: bool,
    /// Who made or last changed it.
    pub user: String,
    /// Settings of other programs kept as read, to be saved back
    /// (LibreOffice's display-border, copy-back...: attribute, value).
    pub kept: Vec<(String, String)>,
}

/// The scenario state of a sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScenarioInfo {
    /// The scenario shown last (index in the sheet's list).
    pub current: Option<usize>,
    /// The scenario selected in the manager.
    pub selected: Option<usize>,
    /// Result cells of the last summary report.
    pub results: Vec<Range>,
}

/// A pivot table placed in the workbook.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPivot {
    pub spec: PivotSpec,
    /// Sheet (by name) and top-left cell of the output.
    pub sheet: String,
    pub at: CellRef,
    /// Area written last time (cleared on refresh).
    pub area: Option<Range>,
    /// Pivot charts (chart ids on the output sheet), redrawn on refresh.
    pub charts: Vec<u32>,
}

/// A slicer: buttons that filter a pivot table by the items of a field.
/// Its selection is the matching entry of the pivot's `spec.slicers`.
#[derive(Clone, Debug, PartialEq)]
pub struct Slicer {
    /// Sheet (by name) it floats over, and its box in sheet pixels.
    pub sheet: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Index of the pivot table in `State::pivots`.
    pub pivot: usize,
    /// Source column of the field.
    pub col: u32,
    pub caption: String,
    /// Buttons per row.
    pub columns: u8,
}

/// A rectangular block of output values (statistics tools, reports).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub rows: Vec<Vec<Value>>,
    /// Number formats of some cells, by (row, column).
    pub formats: Vec<((usize, usize), String)>,
}

impl Table {
    pub fn new() -> Self {
        Table::default()
    }

    /// Appends a row.
    pub fn push(&mut self, row: Vec<Value>) {
        self.rows.push(row);
    }

    /// A row made of a label followed by numbers.
    pub fn push_labeled(&mut self, label: &str, nums: &[f64]) {
        let mut row = vec![Value::text(label)];
        row.extend(nums.iter().map(|n| num(*n)));
        self.rows.push(row);
    }

    /// Gives cell (r, c) a number format code.
    pub fn set_format(&mut self, r: usize, c: usize, code: &str) {
        self.formats.push(((r, c), code.to_string()));
    }

    pub fn blank(&mut self) {
        self.rows.push(Vec::new());
    }

    pub fn width(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.rows.len()
    }

    pub fn get(&self, r: usize, c: usize) -> &Value {
        static EMPTY: Value = Value::Empty;
        self.rows.get(r).and_then(|x| x.get(c)).unwrap_or(&EMPTY)
    }

    /// Places `other` to the right of this table, starting at row 0 and
    /// leaving `gap` empty columns.
    pub fn beside(&mut self, other: Table, gap: usize) {
        let w = self.width() + gap;
        self.formats.extend(other.formats.into_iter().map(|((r, c), f)| ((r, c + w), f)));
        for (i, row) in other.rows.into_iter().enumerate() {
            if self.rows.len() <= i {
                self.rows.push(Vec::new());
            }
            let r = &mut self.rows[i];
            r.resize(w, Value::Empty);
            r.extend(row);
        }
    }
}

/// A number as a cell value: NaN and infinities become #NUM! (#DIV/0!
/// where it comes from a division by zero is indistinguishable here).
pub fn num(n: f64) -> Value {
    if n.is_finite() { Value::Number(n) } else { Value::Error(crate::value::ErrorKind::Num) }
}

/// A result cell of #N/A (Excel's marker for "not available").
pub fn na() -> Value {
    Value::Error(crate::value::ErrorKind::Na)
}
