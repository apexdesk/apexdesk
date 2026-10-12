//! Workbook: sheets, dependency graph, incremental recalculation and undo.

use std::collections::{BTreeMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use crate::addr::{CellRef, MAX_COLS, MAX_ROWS, Range};
use crate::format::{self, Locale, NumFormat};
use crate::cell_formula::CellFormula;
use crate::formula::{self, Expr};
use crate::functions::{Env, EvalContext, Func, Hidden};
use crate::data::{AutoFilter, CfRule, Validation};
use crate::sheet::{Cell, CellStyle, Content, Sheet, StyleTable};
use crate::value::{ErrorKind, Value};
use crate::protect::Protection;

/// Fast non-cryptographic hasher (FxHash) for integer keys.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellKey {
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
}

impl CellKey {
    pub fn new(sheet: usize, c: CellRef) -> Self {
        CellKey { sheet: sheet as u32, row: c.row, col: c.col }
    }

    pub fn cell(&self) -> CellRef {
        CellRef::new(self.row, self.col)
    }
}

/// Auto Fill Options: how a dragged fill fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoFill {
    /// Series where the source looks like one, copies elsewhere.
    Smart,
    Copy,
    Series,
    /// Only the format.
    Formats,
    /// Without the format.
    Values,
    Days,
    Weekdays,
    Months,
    Years,
}

/// A reversible edit. Applying an op yields its inverse.
#[derive(Clone, Debug)]
pub enum Op {
    SetCells { sheet: usize, cells: Vec<(CellRef, Option<Cell>)> },
    InsertRows { sheet: usize, at: u32, count: u32 },
    DeleteRows { sheet: usize, at: u32, count: u32 },
    InsertCols { sheet: usize, at: u32, count: u32 },
    DeleteCols { sheet: usize, at: u32, count: u32 },
    /// One blank row before each of `at` (ascending, may repeat), in one
    /// pass: subtotals insert many.
    InsertRowsAt { sheet: usize, at: Vec<u32> },
    /// Deletes each of `rows` (ascending).
    DeleteRowsAt { sheet: usize, rows: Vec<u32> },
    ColWidths { sheet: usize, sizes: Vec<(u32, Option<f32>)> },
    RowHeights { sheet: usize, sizes: Vec<(u32, Option<f32>)> },
    AddSheet { index: usize, sheet: Box<Sheet> },
    RemoveSheet { index: usize },
    RenameSheet { index: usize, name: String },
    /// Freezes the rows above `rows` and the columns left of `cols`, the
    /// frozen panes showing from `top` on.
    Freeze { sheet: usize, rows: u32, cols: u32, top: CellRef },
    /// Replaces the merged areas of a sheet.
    Merges { sheet: usize, merges: Vec<Range> },
    /// Visibility and tab color of a sheet.
    SheetProps { index: usize, hidden: bool, tab_color: Option<u32> },
    MoveSheet { from: usize, to: usize },
    Filter { sheet: usize, filter: Option<AutoFilter> },
    Validations { sheet: usize, list: Vec<Validation> },
    CondFormats { sheet: usize, list: Vec<CfRule> },
    /// Replaces the outline levels of the rows (or columns) of a sheet.
    Outline { sheet: usize, rows: bool, levels: BTreeMap<u32, u8> },
    /// Protection of a sheet, or of the workbook's structure (`sheet` None).
    Protect { sheet: Option<usize>, protection: Option<Protection> },
    /// Replaces the ranges editable on a protected sheet.
    EditRanges { sheet: usize, list: Vec<crate::protect::EditRange> },
    PageSetup { sheet: usize, page: Box<crate::SheetPage> },
    /// Replaces the pictures of a sheet.
    Images { sheet: usize, list: Vec<crate::SheetImage> },
    /// Replaces the charts (or the sparklines) of a sheet.
    Charts { sheet: usize, list: Vec<crate::Chart> },
    Sparklines { sheet: usize, list: Vec<crate::SparkGroup> },
    /// Replaces the tables of a sheet.
    Tables { sheet: usize, list: Vec<crate::table::Table> },
    /// Replaces the shapes of a sheet.
    Shapes { sheet: usize, list: Vec<crate::SheetShape> },
    /// Replaces the what-if/pivot/list state (scenarios, pivot specs...).
    Analysis(Box<crate::analysis::State>),
    /// Replaces the named cell styles and the theme.
    CellStyles(Box<crate::cellstyles::CellStyles>),
    Names(Vec<crate::names::DefinedName>),
    /// The linked books ([`Workbook::externals`]).
    Externals(Vec<crate::external::ExternalBook>),
    /// The Get Data queries ([`Workbook::queries`]).
    Queries(Vec<crate::query::Query>),
    /// The properties, macros and kept parts of the file (the file
    /// inspector removes them).
    FileParts(Box<FileParts>),
    /// The workbook's tests ([`Workbook::checks`]).
    Checks(Vec<review::Check>),
    /// The calculation options ([`Workbook::calc`]).
    Calc(CalcOptions),
    Batch(Vec<Op>),
}

/// What a workbook carries besides its sheets' content, swapped as a
/// whole by [`Op::FileParts`].
#[derive(Clone, Debug, Default)]
pub struct FileParts {
    pub meta: apex_doc::Meta,
    pub macros: apex_doc::Macros,
    pub kept: apex_doc::KeptParts,
    /// The parts each sheet points at ([`Sheet::kept`]), in sheet order.
    pub sheet_kept: Vec<Vec<apex_doc::KeptLink>>,
}

impl Op {
    /// Default undo history label (i18n key).
    pub fn label(&self) -> &'static str {
        match self {
            Op::SetCells { .. } => "undo.edit",
            Op::InsertRows { .. } => "undo.insert_rows",
            Op::DeleteRows { .. } | Op::DeleteRowsAt { .. } => "undo.delete_rows",
            Op::InsertRowsAt { .. } => "undo.insert_rows",
            Op::InsertCols { .. } => "undo.insert_cols",
            Op::DeleteCols { .. } => "undo.delete_cols",
            Op::ColWidths { .. } => "undo.col_width",
            Op::RowHeights { .. } => "undo.row_height",
            Op::AddSheet { .. } => "undo.add_sheet",
            Op::RemoveSheet { .. } => "undo.delete_sheet",
            Op::RenameSheet { .. } => "undo.rename_sheet",
            Op::Freeze { .. } => "undo.freeze",
            Op::Merges { .. } => "undo.merge",
            Op::SheetProps { .. } => "undo.sheet_props",
            Op::MoveSheet { .. } => "undo.move_sheet",
            Op::Filter { .. } => "undo.filter",
            Op::Validations { .. } => "undo.validation",
            Op::CondFormats { .. } => "undo.cond_format",
            Op::Outline { .. } => "undo.group",
            Op::Protect { .. } => "undo.protect",
            Op::EditRanges { .. } => "undo.edit_ranges",
            Op::PageSetup { .. } => "undo.page_setup",
            Op::Images { .. } => "undo.image",
            Op::Charts { .. } => "undo.chart",
            Op::Sparklines { .. } => "undo.sparklines",
            Op::Tables { .. } => "undo.table",
            Op::Shapes { .. } => "undo.shape",
            Op::Analysis(_) => "undo.analysis",
            Op::CellStyles(_) => "undo.cell_style",
            Op::Names(_) => "undo.names",
            Op::Externals(_) => "undo.break_link",
            Op::Queries(_) => "undo.queries",
            Op::Checks(_) => "undo.checks",
            Op::Calc(_) => "undo.calc",
            Op::FileParts(_) => "undo.inspect",
            Op::Batch(ops) => ops.first().map_or("undo.edit", Op::label),
        }
    }
}

// ---------------------------------------------------------------------------
// Dependency graph

#[derive(Default, Clone)]
struct DepGraph {
    /// precedent cell -> formula cells that reference it directly
    cells: HashMap<CellKey, Readers>,
    ranges: Vec<Option<(u32, Range, CellKey)>>,
    free: Vec<usize>,
    /// (sheet, col) -> range ids covering that column
    buckets: HashMap<(u32, u32), Vec<usize>>,
    /// ranges spanning many columns are scanned linearly
    wide: Vec<usize>,
    owned: HashMap<CellKey, Vec<usize>>,
}

const WIDE_COLS: u32 = 32;

/// The formulas that read a cell: most cells are read by one, kept
/// without a list of its own (a list costs four times as much).
#[derive(Clone)]
enum Readers {
    One(CellKey),
    Many(Vec<CellKey>),
}

impl Readers {
    fn push(&mut self, k: CellKey) {
        match self {
            Readers::One(a) => *self = Readers::Many(vec![*a, k]),
            Readers::Many(v) => v.push(k),
        }
    }

    fn as_slice(&self) -> &[CellKey] {
        match self {
            Readers::One(a) => std::slice::from_ref(a),
            Readers::Many(v) => v,
        }
    }

    /// Takes out `k`; false when none are left.
    fn remove(&mut self, k: CellKey) -> bool {
        let v = match self {
            Readers::One(a) => return *a != k,
            Readers::Many(v) => v,
        };
        if let Some(i) = v.iter().position(|x| *x == k) {
            v.swap_remove(i);
        }
        if let [one] = v[..] {
            *self = Readers::One(one);
            return true;
        }
        !v.is_empty()
    }
}

impl DepGraph {
    fn build(keys: &[CellKey], deps: &KeyDeps) -> DepGraph {
        let mut g = DepGraph::default();
        for (i, &k) in keys.iter().enumerate() {
            g.add(k, deps.of(i));
        }
        g
    }

    fn add(&mut self, key: CellKey, deps: &[Dep]) {
        for d in deps {
            match *d {
                Dep::Cell(p) => match self.cells.entry(p) {
                    std::collections::hash_map::Entry::Occupied(mut e) => e.get_mut().push(key),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(Readers::One(key));
                    }
                },
                Dep::Range(s, r) => {
                    let id = match self.free.pop() {
                        Some(i) => {
                            self.ranges[i] = Some((s, r, key));
                            i
                        }
                        None => {
                            self.ranges.push(Some((s, r, key)));
                            self.ranges.len() - 1
                        }
                    };
                    if r.cols() > WIDE_COLS {
                        self.wide.push(id);
                    } else {
                        for c in r.start.col..=r.end.col {
                            self.buckets.entry((s, c)).or_default().push(id);
                        }
                    }
                    self.owned.entry(key).or_default().push(id);
                }
            }
        }
    }

    fn remove(&mut self, key: CellKey, deps: &[Dep]) {
        for d in deps {
            if let Dep::Cell(p) = d {
                if self.cells.get_mut(p).is_some_and(|v| !v.remove(key)) {
                    self.cells.remove(p);
                }
            }
        }
        if let Some(ids) = self.owned.remove(&key) {
            for id in ids {
                if let Some((s, r, _)) = self.ranges[id].take() {
                    if r.cols() > WIDE_COLS {
                        self.wide.retain(|&x| x != id);
                    } else {
                        for c in r.start.col..=r.end.col {
                            if let Some(b) = self.buckets.get_mut(&(s, c)) {
                                b.retain(|&x| x != id);
                            }
                        }
                    }
                    self.free.push(id);
                }
            }
        }
    }

    fn dependents(&self, k: CellKey, out: &mut Vec<CellKey>) {
        if let Some(v) = self.cells.get(&k) {
            out.extend_from_slice(v.as_slice());
        }
        let c = k.cell();
        let check = |id: &usize, out: &mut Vec<CellKey>| {
            if let Some((s, r, d)) = &self.ranges[*id] {
                if *s == k.sheet && r.contains(c) {
                    out.push(*d);
                }
            }
        };
        if let Some(b) = self.buckets.get(&(k.sheet, k.col)) {
            b.iter().for_each(|id| check(id, out));
        }
        self.wide.iter().for_each(|id| check(id, out));
    }
}

#[derive(Clone, Copy, Debug)]
enum Dep {
    Cell(CellKey),
    Range(u32, Range),
}

/// What each of a list of formulas reads, in one list.
/// In parts of `per` formulas each, as the threads found them (joining
/// them would hold everything twice for a moment).
struct KeyDeps {
    per: usize,
    /// Where each formula's start in `items`, and the end.
    parts: Vec<(Vec<u32>, Vec<Dep>)>,
}

impl KeyDeps {
    fn of(&self, i: usize) -> &[Dep] {
        let (start, items) = &self.parts[i / self.per];
        let j = i % self.per;
        &items[start[j] as usize..start[j + 1] as usize]
    }
}

/// A range in the evaluation order: how many of its formulas are still
/// to be calculated, and the formulas that read it.
struct RangeNode {
    waiting: u32,
    readers: Vec<u32>,
}

/// Lists of numbers by number (edges of the evaluation order), all in
/// one array.
struct Csr {
    start: Vec<u32>,
    items: Vec<u32>,
}

impl Csr {
    /// From (from, to) pairs, `from` below `n`.
    fn new(n: usize, pairs: Vec<(u32, u32)>) -> Csr {
        let mut start = vec![0u32; n + 1];
        for &(a, _) in &pairs {
            start[a as usize + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut next = start.clone();
        let mut items = vec![0u32; pairs.len()];
        for (a, b) in pairs {
            items[next[a as usize] as usize] = b;
            next[a as usize] += 1;
        }
        Csr { start, items }
    }

    fn of(&self, i: u32) -> &[u32] {
        &self.items[self.start[i as usize] as usize..self.start[i as usize + 1] as usize]
    }
}

mod edit;
mod dynorder;
mod tools;
mod outline;
mod annotate;
mod analysis;
mod tables;
mod plan;
mod plan_cmds;
mod plan_custom;
mod plan_report;
#[cfg(test)]
mod plan_tests;
mod names;
mod special;
mod paste_extra;
mod advanced;
mod evalsteps;
mod forecast;
mod group;
mod dataform;
mod model;
pub use dataform::FormField;
mod incell;
mod quick;
mod macro_ops;
pub use quick::QuickTotal;
mod queryload;
pub use queryload::{LoadError, QueryResult};
mod review;
pub use review::{Check, Outcome, Review};
mod origin;
pub use origin::Origin;
pub use evalsteps::{EvalSteps, EvalView};
pub use advanced::{AdvancedFilterError, ConsolidateFn};
pub use special::{SPECIAL_ALL_TYPES, SPECIAL_ERRORS, SPECIAL_LOGICAL, SPECIAL_NUMBERS, SPECIAL_TEXT, Special, cells_to_ranges};
#[cfg(test)]
mod refops_tests;
pub use outline::{Group, MAX_LEVEL, SubtotalOpts};
pub use tools::{BookStats, CfCache, DAYS_1904, SplitMode, split_text};
pub use tables::{TableError, TableId, apply_look, unqualified};
pub use plan::{PlanView, Resource, field_name, field_of_header, set_field_names};
pub use plan_cmds::{NewTask, plan_column};
pub use plan_report::PlanReport;
pub use plan_custom::Indicator;
pub use analysis::{AnalysisError, GoalSeekResult, Output};
pub use forecast::{ForecastOptions, ForecastSource};
pub use edit::{BorderPreset, FillDir, PasteOp, PasteSpecial, PasteWhat, SeriesKind, SeriesOpts, SortKey, SortOn};

// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    /// Workbook properties (File > Properties).
    pub meta: apex_doc::Meta,
    pub styles: StyleTable,
    pub locale: Locale,
    graph: DepGraph,
    volatile: HashSet<CellKey>,
    /// Cells whose result depends on which rows are hidden (SUBTOTAL).
    visibility: HashSet<CellKey>,
    /// Inverse ops with the i18n key naming what they undo.
    undo: Vec<(Op, &'static str)>,
    redo: Vec<(Op, &'static str)>,
    /// Incremented on every change (UI caches key on this).
    pub revision: u64,
    /// Incremented by every edit with an undo step of its own (not by
    /// undo, redo or a macro's changes): what fires Worksheet_Change.
    pub edits: u64,
    pub modified: bool,
    /// Some formula (not a volatile one) worked out to other than the value
    /// its file keeps, which other programs show until they recalculate.
    pub loaded_stale: bool,
    pending: Vec<CellKey>,
    full_rebuild: bool,
    /// Dynamic array anchors and the area each one spills (or wants to).
    spills: HashMap<CellKey, SpillArea>,
    spill_depth: u32,
    /// Structure protection (sheets can't be added, removed, renamed...).
    pub protection: Option<Protection>,
    /// Refuse the changes protection forbids (the app turns this on).
    pub enforce_protection: bool,
    /// The file was opened read-only: every change is refused.
    pub read_only: bool,
    /// A change was refused since the last `take_blocked`.
    blocked: bool,
    /// Scenarios, pivot table specs and custom lists (see [`crate::analysis`]).
    pub analysis: crate::analysis::State,
    /// Named cell styles and the theme.
    pub cell_styles: crate::cellstyles::CellStyles,
    /// Defined names (Formulas > Name Manager).
    pub names: Vec<crate::names::DefinedName>,
    /// Other workbooks the formulas refer to, with their saved values.
    pub externals: Vec<crate::external::ExternalBook>,
    /// Arrays of external ranges, kept while a recalculation runs.
    ext_ranges: crate::external::RangeCache,
    /// Formulas still to calculate while a recalculation runs.
    pending_cells: dynorder::Pending,
    /// Automatic or manual calculation, and iteration of circular references.
    pub calc: CalcOptions,
    /// Formulas left out of date in manual mode (`stale_all`: every one).
    stale: HashSet<CellKey>,
    stale_all: bool,
    /// A cell of the last circular reference found.
    circular: Option<CellKey>,
    /// Grouped sheets (two or more: their edits are shared, see `group`).
    pub group: Vec<usize>,
    /// View > Custom Views (each sheet keeps its part).
    pub custom_views: Vec<crate::views::CustomView>,
    /// Parts of the file kept as they were (connections, queries, data
    /// model, pivot caches, custom XML…).
    pub kept: apex_doc::KeptParts,
    /// Macros kept from the file (not run).
    pub macros: apex_doc::Macros,
    /// Get Data queries (Data > Get Data).
    pub queries: Vec<crate::query::Query>,
    /// Tests that must stay true (Formulas > Tests), saved in a part of
    /// our own.
    pub checks: Vec<review::Check>,
}

/// Calculation options (File > Options > Formulas in Excel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalcOptions {
    /// Only F9 recalculates; edits compute just the edited formulas.
    pub manual: bool,
    /// Circular references are iterated instead of giving #CIRC!.
    pub iterate: bool,
    pub max_iter: u32,
    /// Iteration stops when no value changes more than this.
    pub max_change: f64,
    /// Saved with the 1904 date system (Mac Excel): dates are kept as
    /// 1900 serials here and shifted when read and written.
    pub date1904: bool,
    /// Formulas shown and typed in R1C1 style.
    pub r1c1: bool,
    /// Precision as displayed: formula results are rounded to the
    /// decimals their number format shows.
    pub as_shown: bool,
}

impl Default for CalcOptions {
    fn default() -> Self {
        CalcOptions { manual: false, iterate: false, max_iter: 100, max_change: 0.001, date1904: false, r1c1: false, as_shown: false }
    }
}

#[derive(Clone, Copy, Debug)]
struct SpillArea {
    range: Range,
    /// Something is in the way; the anchor shows #SPILL!.
    blocked: bool,
}

/// Largest area a formula may spill into.
const MAX_SPILL: usize = 2_000_000;
/// Seconds one recalculation may take before the rest is left for F9.
const MAX_RECALC_SECS: u64 = 60;

impl Default for Workbook {
    fn default() -> Self {
        Self::new()
    }
}

/// Most merged ranges a sheet keeps from a file.
pub const MAX_MERGES: usize = 100_000;

impl Workbook {
    pub fn new() -> Self {
        let mut wb = Self::empty();
        wb.sheets.push(Sheet::new("Sheet1"));
        wb
    }

    /// A workbook without sheets (importers add their own).
    pub fn empty() -> Self {
        let locale = Locale::from_env();
        Workbook {
            sheets: Vec::new(),
            meta: apex_doc::Meta::default(),
            styles: StyleTable::default(),
            locale,
            graph: DepGraph::default(),
            volatile: HashSet::default(),
            visibility: HashSet::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            edits: 0,
            modified: false,
            loaded_stale: false,
            pending: Vec::new(),
            full_rebuild: false,
            spills: HashMap::default(),
            spill_depth: 0,
            protection: None,
            enforce_protection: false,
            read_only: false,
            blocked: false,
            analysis: Default::default(),
            cell_styles: crate::cellstyles::CellStyles::new(apex_doc::Theme::default(), &locale),
            names: Vec::new(),
            externals: Vec::new(),
            ext_ranges: Default::default(),
            pending_cells: Default::default(),
            calc: CalcOptions::default(),
            stale: HashSet::default(),
            stale_all: false,
            circular: None,
            group: Vec::new(),
            custom_views: Vec::new(),
            kept: apex_doc::KeptParts::default(),
            macros: apex_doc::Macros::default(),
            queries: Vec::new(),
            checks: Vec::new(),
        }
    }

    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.name.eq_ignore_ascii_case(name) || s.name.to_lowercase() == name.to_lowercase())
    }

    pub fn unique_sheet_name(&self, base: &str) -> String {
        let taken: std::collections::HashSet<String> = self.sheets.iter().map(|s| s.name.to_lowercase()).collect();
        (1..).map(|i| format!("{}{}", base, i)).find(|n| !taken.contains(&n.to_lowercase())).unwrap()
    }

    // --- loading ----------------------------------------------------------

    /// Raw cell write used by importers; call [`Workbook::finish_load`] afterwards.
    /// Cells past the sheet's last row or column (damaged files) are dropped.
    pub fn load_cell(&mut self, sheet: usize, c: CellRef, content: Content, style: u32) {
        self.sheets[sheet].load_cell(c, content, style);
    }

    /// `e` for the cell `c`, sharing a neighbour's tree when it can.
    fn placed_formula(&self, sheet: usize, c: CellRef, e: Expr) -> CellFormula {
        self.sheets[sheet].copy_of_neighbour(c, &e).unwrap_or_else(|| e.into())
    }

    /// Rebuilds the dependency graph and recalculates everything.
    pub fn finish_load(&mut self) {
        // Each merge once, and no more than a sheet could show: every
        // frame looks through them.
        for sh in &mut self.sheets {
            let mut seen = HashSet::default();
            sh.merges.retain(|m| seen.insert(*m));
            sh.merges.truncate(MAX_MERGES);
            // Frozen panes within the sheet (a file may say any number).
            sh.frozen_rows = sh.frozen_rows.min(crate::addr::MAX_ROWS);
            sh.frozen_cols = sh.frozen_cols.min(crate::addr::MAX_COLS);
            sh.frozen_top = CellRef::new(sh.frozen_top.row.min(sh.frozen_rows.saturating_sub(1)), sh.frozen_top.col.min(sh.frozen_cols.saturating_sub(1)));
        }
        self.unqualify_own_tables();
        self.settle_name_intersections();
        let keys = self.formula_keys();
        let kept: Vec<Value> = keys.iter().map(|k| self.sheets[k.sheet as usize].value(k.cell()).clone()).collect();
        self.rebuild_and_recalc();
        self.loaded_stale = keys.iter().zip(&kept).any(|(k, v)| !self.volatile.contains(k) && self.sheets[k.sheet as usize].value(k.cell()) != v);
        self.undo.clear();
        self.redo.clear();
        self.modified = false;
        self.revision += 1;
    }

    // --- dependency helpers --------------------------------------------------

    fn deps_of(&self, sheet: usize, cell: CellRef, expr: &Expr) -> Vec<Dep> {
        let mut out = Vec::new();
        self.deps_into(sheet, cell, expr, &mut out, 0, &mut HashSet::default());
        out
    }

    /// `seen`: the names already followed for this formula (each once:
    /// names using each other several times over would take exponential
    /// time).
    fn deps_into(&self, sheet: usize, cell: CellRef, expr: &Expr, out: &mut Vec<Dep>, depth: u32, seen: &mut HashSet<String>) {
        // Defined names depend on what they stand for.
        if !self.names.is_empty() && depth < 16 {
            expr.visit_names(&mut |n| {
                if !seen.insert(n.to_lowercase()) {
                    return;
                }
                if let Some(e) = self.name_expr(sheet, n) {
                    let mut e = (*e).clone();
                    if e.has_relative_refs() {
                        e.offset_wrap(cell.row as i64, cell.col as i64);
                    }
                    self.deps_into(sheet, cell, &e, out, depth + 1, seen);
                }
            });
        }
        expr.visit_refs(&mut |e| match e {
            Expr::Structured(r) => {
                if let Some((si, range)) = self.table_range(sheet, cell, r) {
                    out.push(Dep::Range(si as u32, range));
                }
            }
            Expr::Ref { sheet: s, addr } => {
                for si in self.resolve_span(sheet, s) {
                    out.push(Dep::Cell(CellKey { sheet: si as u32, row: addr.row, col: addr.col }));
                }
            }
            Expr::Range { sheet: s, a, b, .. } => {
                for si in self.resolve_span(sheet, s) {
                    out.push(Dep::Range(si as u32, Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col))));
                }
            }
            // The anchor rewrites its spill cells before its dependents run.
            Expr::SpillRef { sheet: s, addr } => {
                if let Some(si) = self.resolve(sheet, s) {
                    out.push(Dep::Cell(CellKey { sheet: si as u32, row: addr.row, col: addr.col }));
                }
            }
            _ => {}
        });
        self.range_op_deps(sheet, expr, out, depth);
    }

    /// The cells between the ends of the range operator (`A1:Last`), as
    /// far as they are known before calculating.
    fn range_op_deps(&self, sheet: usize, e: &Expr, out: &mut Vec<Dep>, depth: u32) {
        match e {
            Expr::Binary(crate::formula::BinOp::Range, l, r) => {
                if let (Some((s1, a)), Some((s2, b))) = (self.static_ref(sheet, l, depth), self.static_ref(sheet, r, depth))
                    && s1 == s2
                {
                    let start = CellRef::new(a.start.row.min(b.start.row), a.start.col.min(b.start.col));
                    out.push(Dep::Range(s1 as u32, Range::new(start, CellRef::new(a.end.row.max(b.end.row), a.end.col.max(b.end.col)))));
                }
                self.range_op_deps(sheet, l, out, depth);
                self.range_op_deps(sheet, r, out, depth);
            }
            Expr::Unary(_, x) | Expr::Percent(x) | Expr::Paren(x) => self.range_op_deps(sheet, x, out, depth),
            Expr::Binary(_, l, r) => {
                self.range_op_deps(sheet, l, out, depth);
                self.range_op_deps(sheet, r, out, depth);
            }
            Expr::Call { args, .. } => args.iter().for_each(|a| self.range_op_deps(sheet, a, out, depth)),
            _ => {}
        }
    }

    /// The cells a reference stands for when that doesn't depend on
    /// values (INDEX: somewhere in its range).
    fn static_ref(&self, sheet: usize, e: &Expr, depth: u32) -> Option<(usize, Range)> {
        match e {
            Expr::Ref { sheet: s, addr } => Some((self.resolve(sheet, s)?, Range::single(CellRef::new(addr.row, addr.col)))),
            Expr::Range { sheet: s, a, b, .. } => Some((self.resolve(sheet, s)?, Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col)))),
            Expr::Paren(x) => self.static_ref(sheet, x, depth),
            Expr::Call { func: Some(Func::Index), args, .. } => self.static_ref(sheet, args.first()?, depth),
            Expr::Name(n) if depth < 8 => self.static_ref(sheet, &*self.name_expr(sheet, n)?, depth + 1),
            _ => None,
        }
    }

    /// The sheets a reference reaches: one, or all of a 3D span.
    fn resolve_span(&self, own: usize, s: &Option<Arc<str>>) -> std::ops::RangeInclusive<usize> {
        if let Some((a, b)) = s.as_deref().and_then(crate::formula::split_3d) {
            return match (self.sheet_index(a), self.sheet_index(b)) {
                (Some(a), Some(b)) => a.min(b)..=a.max(b),
                _ => 1..=0,
            };
        }
        self.resolve(own, s).map_or(1..=0, |i| i..=i)
    }

    fn resolve(&self, own: usize, s: &Option<Arc<str>>) -> Option<usize> {
        match s {
            None => Some(own),
            Some(n) => self.sheet_index(n),
        }
    }

    /// The formula cells of the book, in sheet order (column by column,
    /// as the cells are stored).
    fn formula_keys(&self) -> Vec<CellKey> {
        self.sheets.iter().enumerate().flat_map(|(si, sh)| sh.iter().filter(|(_, c)| c.formula().is_some()).map(move |(c, _)| CellKey::new(si, c))).collect()
    }

    /// What each formula of `keys` reads: `deps.of(i)`. Many formulas are
    /// worked out on several threads.
    fn deps_of_keys(&self, keys: &[CellKey]) -> KeyDeps {
        let part = |keys: &[CellKey]| {
            let mut start = Vec::with_capacity(keys.len() + 1);
            let mut items = Vec::new();
            for k in keys {
                start.push(items.len() as u32);
                if let Some(f) = self.sheets[k.sheet as usize].get(k.cell()).and_then(|c| c.formula()) {
                    self.deps_into(k.sheet as usize, k.cell(), &f.expr(), &mut items, 0, &mut HashSet::default());
                }
            }
            start.push(items.len() as u32);
            items.shrink_to_fit();
            (start, items)
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
        let per = if keys.len() < 20_000 || threads < 2 { keys.len().max(1) } else { keys.len().div_ceil(threads) };
        let parts = if per >= keys.len() {
            vec![part(keys)]
        } else {
            std::thread::scope(|s| {
                let handles: Vec<_> = keys.chunks(per).map(|c| s.spawn(move || part(c))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect::<Vec<_>>()
            })
        };
        KeyDeps { per, parts }
    }

    fn rebuild_graph(&mut self) {
        let keys = self.formula_keys();
        let deps = self.deps_of_keys(&keys);
        self.rebuild_graph_from(&keys, &deps);
    }

    /// Rebuilds the graph and recalculates everything, working out what
    /// each formula reads once for both.
    fn rebuild_and_recalc(&mut self) {
        let keys = self.formula_keys();
        let deps = self.deps_of_keys(&keys);
        self.mark_volatile(&keys);
        self.clear_for_recalc();
        // Clearing spilled values leaves the formulas as they were. The
        // graph is wanted after calculating, for spills: built meanwhile.
        let spilled = std::thread::scope(|s| {
            let graph = s.spawn(|| DepGraph::build(&keys, &deps));
            let spilled = self.evaluate_with(&keys, Some(&deps));
            self.graph = graph.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
            spilled
        });
        self.recalc_spilled(spilled);
    }

    fn rebuild_graph_from(&mut self, keys: &[CellKey], deps: &KeyDeps) {
        self.graph = DepGraph::build(keys, deps);
        self.mark_volatile(keys);
    }

    /// Notes which formulas of `keys` are volatile and which depend on
    /// hidden rows.
    fn mark_volatile(&mut self, keys: &[CellKey]) {
        let mut vol = HashSet::default();
        let mut vis = HashSet::default();
        // Copies of one formula follow each other: its tree is looked at once.
        let mut last: Option<(*const Expr, u32, bool, bool)> = None;
        for &k in keys {
            let Some(f) = self.sheets[k.sheet as usize].get(k.cell()).and_then(|c| c.formula()) else { continue };
            // Moving a copy changes only its references.
            let e = f.shape();
            let (is_vol, is_vis) = match last {
                Some((p, s, v, h)) if std::ptr::eq(p, e) && s == k.sheet => (v, h),
                _ => {
                    let v = e.is_volatile() || self.uses_volatile_name(k.sheet as usize, e);
                    let h = e.calls(Func::Subtotal) || e.calls(Func::Aggregate);
                    last = Some((e, k.sheet, v, h));
                    (v, h)
                }
            };
            if is_vol {
                vol.insert(k);
            }
            if is_vis {
                vis.insert(k);
            }
        }
        self.volatile = vol;
        self.visibility = vis;
    }

    // --- recalculation ----------------------------------------------------

    pub fn recalc_all(&mut self) {
        self.clear_for_recalc();
        let all = self.formula_keys();
        self.evaluate(all);
    }

    fn clear_for_recalc(&mut self) {
        self.stale.clear();
        self.stale_all = false;
        self.circular = None;
        // Spilled values are rebuilt from their anchors.
        self.spills.clear();
        for sh in &mut self.sheets {
            let spilled: Vec<CellRef> = sh.iter().filter(|(_, c)| matches!(c.content, Content::Spill(_))).map(|(c, _)| c).collect();
            for c in spilled {
                let cell = Cell::with_content(sh.get(c), Content::Empty);
                sh.put(c, cell);
            }
        }
    }

    /// Recalculates volatile cells (NOW, RAND...) and their dependents.
    pub fn recalc_volatile(&mut self) {
        if self.volatile.is_empty() || self.calc.manual {
            return;
        }
        let seeds: Vec<CellKey> = self.volatile.iter().copied().collect();
        self.recalc_from(&seeds);
        self.revision += 1;
    }

    fn recalc_from(&mut self, changed: &[CellKey]) {
        let mut dirty: HashSet<CellKey> = HashSet::default();
        let mut stack: Vec<CellKey> = Vec::new();
        for &k in changed.iter().chain(self.volatile.iter()) {
            if self.is_formula(k) && dirty.insert(k) {
                stack.push(k);
            } else {
                stack.push(k);
            }
        }
        let mut buf = Vec::new();
        while let Some(k) = stack.pop() {
            buf.clear();
            self.graph.dependents(k, &mut buf);
            for &d in &buf {
                if dirty.insert(d) {
                    stack.push(d);
                }
            }
        }
        self.evaluate(dirty.into_iter().collect());
    }

    fn is_formula(&self, k: CellKey) -> bool {
        self.sheets.get(k.sheet as usize).and_then(|s| s.get(k.cell())).is_some_and(|c| c.formula().is_some())
    }

    /// Topologically evaluates `dirty` formula cells; cycles get #CIRC!.
    ///
    /// The formulas are numbered in sheet order (column by column, as the
    /// cells are stored) and the order is worked out on those numbers:
    /// arrays instead of maps keyed by cell, and edges in one list.
    fn evaluate(&mut self, mut keys: Vec<CellKey>) {
        keys.retain(|k| self.is_formula(*k));
        keys.sort_unstable_by_key(|k| (k.sheet, k.col, k.row));
        keys.dedup();
        let spilled = self.evaluate_with(&keys, None);
        self.recalc_spilled(spilled);
    }

    fn recalc_spilled(&mut self, spilled: Vec<CellKey>) {
        // Formulas that read spilled cells had no edge to the anchor.
        if !spilled.is_empty() && self.spill_depth < 8 {
            self.spill_depth += 1;
            self.recalc_from(&spilled);
            self.spill_depth -= 1;
        }
    }

    /// [`Self::evaluate`] of formula cells in sheet order, with what they
    /// read when that is known.
    /// Returns the anchors whose spilled cells changed.
    fn evaluate_with(&mut self, keys: &[CellKey], deps: Option<&KeyDeps>) -> Vec<CellKey> {
        analysis::PIVOT_PASS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if keys.is_empty() {
            return Vec::new();
        }
        let n = keys.len();
        let index: HashMap<CellKey, u32> = keys.iter().enumerate().map(|(i, k)| (*k, i as u32)).collect();
        let mut indeg = vec![0u32; n];
        // (formula read, formula that reads it)
        let mut edges: Vec<(u32, u32)> = Vec::new();
        // A range read by many formulas is one node: its formulas point to
        // it and it to its readers, instead of an edge per pair (thousands
        // of VLOOKUPs over the same table would need billions).
        let mut range_ids: HashMap<(u32, Range), usize> = HashMap::default();
        let mut ranges: Vec<RangeNode> = Vec::new();
        // (formula, range node holding it)
        let mut members: Vec<(u32, u32)> = Vec::new();
        let found;
        let deps = match deps {
            Some(d) => d,
            None => {
                found = self.deps_of_keys(keys);
                &found
            }
        };
        for i in 0..n as u32 {
            for dep in deps.of(i as usize) {
                match *dep {
                    Dep::Cell(p) => {
                        if let Some(&j) = index.get(&p) {
                            edges.push((j, i));
                            indeg[i as usize] += 1;
                        }
                    }
                    Dep::Range(s, r) => {
                        let id = *range_ids.entry((s, r)).or_insert_with(|| {
                            let inside = self.dirty_in(s, r, &keys, &index);
                            members.extend(inside.iter().map(|&m| (m, ranges.len() as u32)));
                            ranges.push(RangeNode { waiting: inside.len() as u32, readers: Vec::new() });
                            ranges.len() - 1
                        });
                        if ranges[id].waiting > 0 {
                            ranges[id].readers.push(i);
                            indeg[i as usize] += 1;
                        }
                    }
                }
            }
        }
        drop(range_ids);
        let out_edges = Csr::new(n, edges);
        let cell_ranges = Csr::new(n, members);
        // Edges found while calculating (see `eval_watching`).
        let mut late: HashMap<u32, Vec<u32>> = HashMap::default();
        self.ext_ranges.set_active(true);
        let mut queue: VecDeque<u32> = (0..n as u32).filter(|&i| indeg[i as usize] == 0).collect();
        let mut done = 0usize;
        let mut spilled = Vec::new();
        // OFFSET, INDIRECT...: what they read is known only when they run.
        let watch = keys.iter().any(|k| self.volatile.contains(k));
        if watch {
            self.pending_cells.set(keys.iter().copied().collect());
        }
        // Without formulas that find what they read as they run, all the
        // formulas ready at once are calculated on several threads.
        let threads = if watch || n < 20_000 { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()).min(12) };
        // A file's formulas may cost far more than its size: past the
        // limit, the rest keep the values they have, out of date (F9).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(MAX_RECALC_SECS);
        let mut cut = false;
        let mut batch: Vec<(u32, Option<Value>)> = Vec::new();
        while !queue.is_empty() {
            batch.clear();
            cut = cut || std::time::Instant::now() > deadline;
            if cut {
                batch.extend(queue.drain(..).map(|i| (i, None)));
            } else if threads > 1 && queue.len() >= 1024 {
                let ready: Vec<u32> = queue.drain(..).collect();
                batch.extend(ready.iter().copied().zip(self.eval_many(&keys, &ready, threads, deadline)));
            } else if let Some(i) = queue.pop_front() {
                let k = keys[i as usize];
                let v = if watch && self.volatile.contains(&k) {
                    match self.eval_watching(k) {
                        Ok(v) => v,
                        // It reads a formula still to calculate: after it.
                        Err(p) if p != k => {
                            if let Some(&j) = index.get(&p) {
                                late.entry(j).or_default().push(i);
                            }
                            indeg[i as usize] += 1;
                            continue;
                        }
                        Err(_) => Value::Error(ErrorKind::Circular),
                    }
                } else {
                    self.eval_cell(k)
                };
                batch.push((i, Some(v)));
            }
            for (i, v) in batch.drain(..) {
            let k = keys[i as usize];
            done += 1;
            if watch {
                self.pending_cells.done(k);
            }
            if let Some(v) = v {
                let v = self.place(k, v, &mut spilled);
                if let Some(Content::Formula { value, .. }) = self.sheets[k.sheet as usize].get_mut(k.cell()).map(|c| &mut c.content) {
                    *value = v;
                }
            } else {
                self.stale.insert(k);
            }
            let mut release = |d: u32, queue: &mut VecDeque<u32>| {
                let e = &mut indeg[d as usize];
                *e -= 1;
                if *e == 0 {
                    queue.push_back(d);
                }
            };
            for &d in out_edges.of(i).iter().chain(late.remove(&i).iter().flatten()) {
                release(d, &mut queue);
            }
            for &id in cell_ranges.of(i) {
                let node = &mut ranges[id as usize];
                node.waiting -= 1;
                if node.waiting == 0 {
                    for d in std::mem::take(&mut node.readers) {
                        release(d, &mut queue);
                    }
                }
            }
            }
        }
        self.pending_cells.clear();
        if done < n {
            // A cycle and what depends on it.
            let mut rest: Vec<CellKey> = (0..n).filter(|&i| indeg[i] > 0).map(|i| keys[i]).collect();
            rest.sort_by_key(|k| (k.sheet, k.row, k.col));
            if self.calc.iterate {
                self.circular = rest.first().copied();
                self.iterate(&rest, &mut spilled);
            } else {
                self.resolve_cycles(&rest, &mut spilled);
                self.circular = rest.iter().copied().find(|k| matches!(self.sheets[k.sheet as usize].value(k.cell()), Value::Error(ErrorKind::Circular)));
            }
        }
        self.ext_ranges.set_active(false);
        spilled
    }

    /// The values of formulas `ready` (of `keys`), none reading another,
    /// calculated on `threads` threads.
    /// Past `deadline`, none (left as they are).
    fn eval_many(&self, keys: &[CellKey], ready: &[u32], threads: usize, deadline: std::time::Instant) -> Vec<Option<Value>> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // Small pieces taken in turn: a few slow formulas (SUMIF over
        // whole columns) don't hold up one thread while the rest wait.
        const PIECE: usize = 4;
        let pieces: Vec<&[u32]> = ready.chunks(PIECE).collect();
        let next = AtomicUsize::new(0);
        let mut done: Vec<(usize, Vec<Option<Value>>)> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    s.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let p = next.fetch_add(1, Ordering::Relaxed);
                            let Some(piece) = pieces.get(p) else { break out };
                            let late = std::time::Instant::now() > deadline;
                            out.push((p, piece.iter().map(|&i| (!late).then(|| self.eval_cell(keys[i as usize]))).collect::<Vec<_>>()));
                        }
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
        });
        done.sort_unstable_by_key(|d| d.0);
        done.into_iter().flat_map(|d| d.1).collect()
    }

    /// The formulas of `keys` (numbered by `index`) inside range `r` of
    /// sheet `s`.
    fn dirty_in(&self, s: u32, r: Range, keys: &[CellKey], index: &HashMap<CellKey, u32>) -> Vec<u32> {
        let sheet = &self.sheets[s as usize];
        let Some(r) = sheet.extent().and_then(|ext| r.intersect(&Range::new(CellRef::new(0, 0), ext))) else { return Vec::new() };
        let mut out = Vec::new();
        if r.cell_count() as usize <= keys.len() * 4 {
            sheet.for_each_in(&r, |c, cell| {
                if cell.formula().is_some() {
                    out.extend(index.get(&CellKey { sheet: s, row: c.row, col: c.col }));
                }
            });
        } else {
            out.extend((0..keys.len() as u32).filter(|&i| keys[i as usize].sheet == s && r.contains(keys[i as usize].cell())));
        }
        out
    }

    /// Evaluates the cells of circular references again and again, in
    /// sheet order from their previous values, until they settle.
    fn iterate(&mut self, cells: &[CellKey], spilled: &mut Vec<CellKey>) {
        for &k in cells {
            if let Some(Content::Formula { value, .. }) = self.sheets[k.sheet as usize].get_mut(k.cell()).map(|c| &mut c.content) {
                if matches!(value, Value::Error(ErrorKind::Circular) | Value::Empty) {
                    *value = Value::Number(0.0);
                }
            }
        }
        // The file gives the count and the change: Excel's most at most,
        // and no more evaluations in all than a big model takes.
        let rounds = (self.calc.max_iter.clamp(1, 32767) as usize).min((100_000_000 / cells.len().max(1)).max(1));
        let settled = if self.calc.max_change >= 0.0 { self.calc.max_change } else { 0.001 };
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            // Costly cells going round: what they have after a while stays.
            if start.elapsed() > std::time::Duration::from_secs(10) {
                break;
            }
            let mut change = 0.0f64;
            for &k in cells {
                let v = self.eval_cell(k);
                let v = self.place(k, v, spilled);
                if let Some(Content::Formula { value, .. }) = self.sheets[k.sheet as usize].get_mut(k.cell()).map(|c| &mut c.content) {
                    change = change.max(match (&*value, &v) {
                        (Value::Number(a), Value::Number(b)) => (a - b).abs(),
                        (a, b) if a == b => 0.0,
                        _ => f64::INFINITY,
                    });
                    *value = v;
                }
            }
            if change < settled {
                break;
            }
        }
    }

    /// Writes an array result into the cells below and to the right of its
    /// anchor, or reports #SPILL! when something is in the way. Returns
    /// the value the anchor shows; cells that change go to `changed`.
    fn place(&mut self, k: CellKey, v: Value, changed: &mut Vec<CellKey>) -> Value {
        let si = k.sheet as usize;
        let old = self.spills.remove(&k);
        // An array formula from an older file fills its area exactly: cut,
        // repeated or padded with #N/A.
        let fixed = self.sheets[si].get(k.cell()).and_then(|c| c.extra.as_ref()?.array_size);
        let v = match (fixed, v) {
            (Some((rows, cols)), v) if !matches!(v, Value::Lambda(_)) && (rows > 1 || cols > 1 || matches!(v, Value::Array(_))) => {
                let m = match v {
                    Value::Array(m) => m,
                    v => Arc::new(crate::value::Matrix::filled(1, 1, v)),
                };
                let (rows, cols) = (rows as usize, cols as usize);
                // An area no spill may cover is not built.
                if rows.saturating_mul(cols) > MAX_SPILL {
                    if let Some(o) = old {
                        self.clear_spill(si, &o.range, k.cell(), None, changed);
                    }
                    return Value::Error(ErrorKind::Spill);
                }
                crate::value::Matrix::new(rows, cols, (0..rows * cols).map(|i| m.at(i / cols, i % cols)).collect()).into_value()
            }
            (_, v) => v,
        };
        let m = match v {
            Value::Array(m) => m,
            v => {
                if let Some(o) = old {
                    self.clear_spill(si, &o.range, k.cell(), None, changed);
                }
                return match v {
                    Value::Lambda(_) => Value::Error(ErrorKind::Calc),
                    v => v,
                };
            }
        };
        let anchor = k.cell();
        let (end_r, end_c) = (anchor.row as usize + m.rows - 1, anchor.col as usize + m.cols - 1);
        if m.data.is_empty() || end_r >= MAX_ROWS as usize || end_c >= MAX_COLS as usize || m.data.len() > MAX_SPILL {
            if let Some(o) = old {
                self.clear_spill(si, &o.range, anchor, None, changed);
            }
            return Value::Error(ErrorKind::Spill);
        }
        let target = Range::new(anchor, CellRef::new(end_r as u32, end_c as u32));
        let others: Vec<Range> = self.spills.iter().filter(|(a, s)| a.sheet == k.sheet && !s.blocked && s.range.intersect(&target).is_some()).map(|(_, s)| s.range).collect();
        let own = old.filter(|o| !o.blocked).map(|o| o.range);
        let mut blocked = false;
        self.sheets[si].for_each_in(&target, |c, cell| {
            if c == anchor {
                return;
            }
            match &cell.content {
                Content::Empty => {}
                // A stale spilled value is free unless another anchor owns it.
                Content::Spill(_) => blocked |= own.is_none_or(|o| !o.contains(c)) && others.iter().any(|o| o.contains(c)),
                _ => blocked = true,
            }
        });
        if let Some(o) = old {
            self.clear_spill(si, &o.range, anchor, (!blocked).then_some(&target), changed);
        }
        if blocked {
            self.spills.insert(k, SpillArea { range: target, blocked: true });
            return Value::Error(ErrorKind::Spill);
        }
        let sheet = &mut self.sheets[si];
        let fix = |v: &Value| match v {
            Value::Empty => Value::Number(0.0),
            Value::Array(_) | Value::Lambda(_) => Value::Error(ErrorKind::Value),
            v => v.clone(),
        };
        for r in 0..m.rows {
            for c in 0..m.cols {
                if r == 0 && c == 0 {
                    continue;
                }
                let at = CellRef::new(anchor.row + r as u32, anchor.col + c as u32);
                let content = Content::Spill(fix(m.get(r, c)));
                match sheet.get_mut(at) {
                    Some(cell) if cell.content == content => {}
                    Some(cell) => {
                        cell.content = content;
                        changed.push(CellKey::new(si, at));
                    }
                    None => {
                        sheet.put(at, Some(Cell::new(content, 0)));
                        changed.push(CellKey::new(si, at));
                    }
                }
            }
        }
        self.spills.insert(k, SpillArea { range: target, blocked: false });
        fix(m.get(0, 0))
    }

    /// Empties the spilled cells of `area` (outside `keep`).
    fn clear_spill(&mut self, si: usize, area: &Range, anchor: CellRef, keep: Option<&Range>, changed: &mut Vec<CellKey>) {
        let mut cells = Vec::new();
        self.sheets[si].for_each_in(area, |c, cell| {
            if c != anchor && matches!(cell.content, Content::Spill(_)) && !keep.is_some_and(|k| k.contains(c)) {
                cells.push(c);
            }
        });
        for c in cells {
            let cell = Cell::with_content(self.sheets[si].get(c), Content::Empty);
            self.sheets[si].put(c, cell);
            changed.push(CellKey::new(si, c));
        }
    }

    /// The area the dynamic array formula at `c` spills into.
    pub fn spill_range(&self, sheet: usize, c: CellRef) -> Option<Range> {
        self.spills.get(&CellKey::new(sheet, c)).filter(|s| !s.blocked).map(|s| s.range)
    }

    /// Whether the formula at `c` returns an array (spilled or blocked).
    pub fn is_array_formula(&self, sheet: usize, c: CellRef) -> bool {
        self.spills.contains_key(&CellKey::new(sheet, c))
    }

    pub fn has_array_formulas(&self) -> bool {
        !self.spills.is_empty()
    }

    // --- auditing ------------------------------------------------------------

    /// Cells and ranges the formula at `c` reads, as (sheet, range).
    pub fn precedents(&self, sheet: usize, c: CellRef) -> Vec<(usize, Range)> {
        let Some(e) = self.sheets.get(sheet).and_then(|s| s.get(c)).and_then(|x| x.formula()).map(|f| f.expr()) else { return Vec::new() };
        let mut out: Vec<(usize, Range)> = Vec::new();
        for d in self.deps_of(sheet, c, &e) {
            let item = match d {
                Dep::Cell(k) => (k.sheet as usize, Range::new(k.cell(), k.cell())),
                Dep::Range(s, r) => {
                    // Whole columns/rows shrink to the used area.
                    let mut r = r;
                    if let Some(ext) = self.sheets[s as usize].extent() {
                        r.end.row = r.end.row.min(ext.row.max(r.start.row));
                        r.end.col = r.end.col.min(ext.col.max(r.start.col));
                    }
                    (s as usize, r)
                }
            };
            if !out.contains(&item) {
                out.push(item);
            }
        }
        out
    }

    /// Formula cells that read `c` directly.
    pub fn dependents(&self, sheet: usize, c: CellRef) -> Vec<(usize, CellRef)> {
        let mut keys = Vec::new();
        self.graph.dependents(CellKey::new(sheet, c), &mut keys);
        let mut out: Vec<(usize, CellRef)> = keys.into_iter().map(|k| (k.sheet as usize, k.cell())).collect();
        out.sort_unstable_by_key(|(s, c)| (*s, c.row, c.col));
        out.dedup();
        out
    }

    /// Whether the formula at `c` differs from the one its neighbours share
    /// (both above and below, or both left and right, agree in R1C1 terms).
    pub fn inconsistent_formula(&self, sheet: usize, c: CellRef) -> bool {
        let sh = &self.sheets[sheet];
        let Some(own) = sh.get(c).and_then(|x| x.formula()).map(|f| f.expr()) else { return false };
        // A neighbour's formula moved onto `c`, in canonical text.
        let moved = |r: i64, col: i64| -> Option<String> {
            if r < 0 || col < 0 {
                return None;
            }
            let n = CellRef::new(r as u32, col as u32);
            let mut e = sh.get(n)?.formula()?.expr().into_owned();
            e.offset(c.row as i64 - r, c.col as i64 - col);
            Some(e.to_canonical())
        };
        let own = own.to_canonical();
        let (r, col) = (c.row as i64, c.col as i64);
        [((r - 1, col), (r + 1, col)), ((r, col - 1), (r, col + 1))].into_iter().any(|(a, b)| match (moved(a.0, a.1), moved(b.0, b.1)) {
            (Some(x), Some(y)) => x == y && x != own,
            _ => false,
        })
    }

    /// The anchor whose array result fills `c`.
    pub fn spill_anchor(&self, sheet: usize, c: CellRef) -> Option<CellRef> {
        if !matches!(self.sheets.get(sheet)?.get(c)?.content, Content::Spill(_)) {
            return None;
        }
        self.spills.iter().find(|(k, s)| k.sheet as usize == sheet && !s.blocked && s.range.contains(c)).map(|(k, _)| k.cell())
    }

    fn eval_cell(&self, k: CellKey) -> Value {
        let Some(expr) = self.sheets[k.sheet as usize].get(k.cell()).and_then(|c| c.formula()).map(|f| f.expr()) else {
            return Value::Empty;
        };
        let env = Env::new(self, k.sheet as usize, k.cell(), self.locale);
        let v = match env.eval(&expr) {
            Value::Empty => Value::Number(0.0),
            Value::Number(n) if n != 0.0 && n.abs() < 1e-3 => env.settle_sum(&expr, n),
            v => v,
        };
        let v = match v {
            Value::Number(n) if self.calc.as_shown => {
                let style = self.sheets[k.sheet as usize].get(k.cell()).map_or(0, |c| c.style);
                Value::Number(self.styles.format(style).as_shown(n))
            }
            v => v,
        };
        // Functions this program doesn't have (add-ins, macros) and links
        // to other books without saved values keep the value the file
        // saved, as Excel does until it can calculate them.
        if let Value::Error(kind) = v {
            let saved = self.sheets[k.sheet as usize].value(k.cell());
            if !matches!(saved, Value::Empty) && *saved != v {
                let mut keep = false;
                if kind == ErrorKind::Name {
                    expr.visit_unknown_calls(&mut |n| keep |= self.find_name(k.sheet as usize, n).is_none());
                }
                expr.visit_refs(&mut |e| {
                    if let Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. } = e {
                        keep |= crate::external::is_external(s);
                    }
                });
                expr.visit_names(&mut |n| keep |= n.starts_with('['));
                if keep {
                    return saved.clone();
                }
            }
        }
        v
    }

    /// Evaluates an ad-hoc formula (status bar, tests).
    pub fn eval_formula(&self, sheet: usize, src: &str) -> Value {
        match formula::parse(src, &self.locale) {
            Ok(e) => Env::new(self, sheet, CellRef::default(), self.locale).eval(&e),
            Err(_) => Value::Error(ErrorKind::Value),
        }
    }

    // --- ops ---------------------------------------------------------------

    /// Applies an op, records it for undo, and recalculates.
    pub fn execute(&mut self, op: Op) {
        let label = op.label();
        self.execute_as(op, label);
    }

    /// Like `execute`, naming the step for the undo history (an i18n key).
    pub fn execute_as(&mut self, op: Op, label: &'static str) {
        if self.read_only || self.enforce_protection && !self.allowed(&op) {
            self.blocked = true;
            return;
        }
        // Plans follow the edit in the same undo step.
        let touch = Self::plan_touch(&op);
        let copy = if touch && (Self::op_makes_plan(&op) || !self.task_plans().is_empty()) { Some(op.clone()) } else { None };
        let mut inv = if self.grouped(label) { self.apply_grouped(op) } else { self.apply(op) };
        if let Some(op) = copy {
            self.commit();
            let mut more = self.plan_follow(&op);
            if !more.is_empty() {
                more.push(inv);
                inv = Op::Batch(more);
            }
        }
        self.undo.push((inv, label));
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.edits += 1;
        self.commit();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some((op, label)) = self.undo.pop() else { return false };
        let inv = self.apply(op);
        self.redo.push((inv, label));
        self.commit();
        true
    }

    /// Number of steps that can be undone.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Joins the last `n` undo steps into one named `label`.
    pub fn merge_undo(&mut self, n: usize, label: &'static str) {
        let n = n.min(self.undo.len());
        if n < 2 {
            return;
        }
        let ops: Vec<Op> = self.undo.drain(self.undo.len() - n..).rev().map(|x| x.0).collect();
        self.undo.push((Op::Batch(ops), label));
    }

    /// Forgets the last undone step (an entry that was rejected).
    pub fn discard_redo(&mut self) {
        self.redo.pop();
    }

    pub fn redo(&mut self) -> bool {
        let Some((op, label)) = self.redo.pop() else { return false };
        let inv = self.apply(op);
        self.undo.push((inv, label));
        self.commit();
        true
    }

    /// Labels (i18n keys) of the undo steps, most recent first.
    pub fn undo_history(&self) -> Vec<&'static str> {
        self.undo.iter().rev().map(|x| x.1).collect()
    }

    pub fn redo_history(&self) -> Vec<&'static str> {
        self.redo.iter().rev().map(|x| x.1).collect()
    }

    /// Formulas wait for F9 (manual calculation).
    pub fn needs_calc(&self) -> bool {
        self.stale_all || !self.stale.is_empty()
    }

    /// A cell in a circular reference that shows #CIRC! (for the status bar).
    pub fn circular_cell(&self) -> Option<(usize, CellRef)> {
        let k = self.circular?;
        let sh = self.sheets.get(k.sheet as usize)?;
        matches!(sh.value(k.cell()), Value::Error(ErrorKind::Circular)).then(|| (k.sheet as usize, k.cell()))
    }

    fn commit(&mut self) {
        if self.full_rebuild && self.calc.manual {
            self.full_rebuild = false;
            self.pending.clear();
            self.rebuild_graph();
            self.stale_all = true;
        } else if self.full_rebuild {
            self.full_rebuild = false;
            self.pending.clear();
            self.rebuild_and_recalc();
        } else if self.calc.manual {
            // Only the edited formulas; what depends on them waits for F9.
            let pending = std::mem::take(&mut self.pending);
            let mut buf = Vec::new();
            let mut stack = pending.clone();
            while let Some(k) = stack.pop() {
                buf.clear();
                self.graph.dependents(k, &mut buf);
                for &d in &buf {
                    if self.stale.insert(d) {
                        stack.push(d);
                    }
                }
            }
            let own: HashSet<CellKey> = pending.into_iter().filter(|k| self.is_formula(*k)).collect();
            self.stale.retain(|k| !own.contains(k));
            self.evaluate(own.into_iter().collect());
        } else {
            let mut pending = std::mem::take(&mut self.pending);
            if !self.spills.is_empty() {
                self.spill_edits(&mut pending);
            }
            self.recalc_from(&pending);
        }
        self.revision += 1;
        self.modified = true;
    }

    /// Edits that touch dynamic arrays: a removed anchor takes its spilled
    /// cells with it, and typing in a spill area re-evaluates the anchor.
    fn spill_edits(&mut self, pending: &mut Vec<CellKey>) {
        let mut extra = Vec::new();
        for &p in pending.iter() {
            if let Some(area) = self.spills.get(&p).copied() {
                if !self.is_formula(p) {
                    self.spills.remove(&p);
                    self.clear_spill(p.sheet as usize, &area.range, p.cell(), None, &mut extra);
                }
            }
            for (a, s) in &self.spills {
                if *a != p && a.sheet == p.sheet && s.range.contains(p.cell()) {
                    extra.push(*a);
                }
            }
        }
        pending.extend(extra);
    }

    fn apply(&mut self, op: Op) -> Op {
        match op {
            Op::SetCells { sheet, cells } => {
                let mut inv = Vec::with_capacity(cells.len());
                for (c, new) in cells {
                    let k = CellKey::new(sheet, c);
                    let old = self.sheets[sheet].put(c, None);
                    if let Some(e) = old.as_ref().and_then(Cell::formula).map(|f| f.expr()) {
                        let deps = self.deps_of(sheet, c, &e);
                        self.graph.remove(k, &deps);
                        self.volatile.remove(&k);
                        self.visibility.remove(&k);
                    }
                    if let Some(e) = new.as_ref().and_then(Cell::formula).map(|f| f.expr()) {
                        let deps = self.deps_of(sheet, c, &e);
                        self.graph.add(k, &deps);
                        if e.is_volatile() {
                            self.volatile.insert(k);
                        }
                        if e.calls(Func::Subtotal) || e.calls(Func::Aggregate) {
                            self.visibility.insert(k);
                        }
                    }
                    self.sheets[sheet].put(c, new);
                    self.pending.push(k);
                    inv.push((c, old));
                }
                inv.reverse();
                Op::SetCells { sheet, cells: inv }
            }
            Op::InsertRows { sheet, at, count } => {
                self.structural(sheet, true, at, count as i64);
                Op::DeleteRows { sheet, at, count }
            }
            Op::InsertCols { sheet, at, count } => {
                self.structural(sheet, false, at, count as i64);
                Op::DeleteCols { sheet, at, count }
            }
            Op::DeleteRows { sheet, at, count } | Op::DeleteCols { sheet, at, count } => {
                let rows = matches!(op, Op::DeleteRows { .. });
                let sizes_before: Vec<(u32, Option<f32>)> = {
                    let m = if rows { &self.sheets[sheet].row_heights } else { &self.sheets[sheet].col_widths };
                    m.range(at..at + count).map(|(k, v)| (*k, Some(*v))).collect()
                };
                let sh = &self.sheets[sheet];
                let areas = [
                    Op::Merges { sheet, merges: sh.merges.clone() },
                    Op::Filter { sheet, filter: sh.filter.clone() },
                    Op::Validations { sheet, list: sh.validations.clone() },
                    Op::CondFormats { sheet, list: sh.cond_formats.clone() },
                    Op::EditRanges { sheet, list: sh.edit_ranges.clone() },
                    Op::Outline { sheet, rows, levels: if rows { sh.row_levels.clone() } else { sh.col_levels.clone() } },
                    Op::Images { sheet, list: sh.images.clone() },
                    Op::Tables { sheet, list: sh.tables.clone() },
                ];
                let objects = self.object_snapshots();
                let (removed, rewritten) = self.structural(sheet, rows, at, -(count as i64));
                let mut ops = vec![if rows { Op::InsertRows { sheet, at, count } } else { Op::InsertCols { sheet, at, count } }];
                // Deleted areas don't come back by shifting: restore them.
                ops.extend(areas);
                ops.extend(objects);
                ops.push(Op::SetCells { sheet, cells: removed.into_iter().map(|(c, cell)| (c, Some(cell))).collect() });
                let mut by_sheet: HashMap<usize, Vec<(CellRef, Option<Cell>)>> = HashMap::default();
                for (k, cell) in rewritten {
                    by_sheet.entry(k.sheet as usize).or_default().push((k.cell(), Some(cell)));
                }
                for (s, cells) in by_sheet {
                    ops.push(Op::SetCells { sheet: s, cells });
                }
                if !sizes_before.is_empty() {
                    ops.push(if rows { Op::RowHeights { sheet, sizes: sizes_before } } else { Op::ColWidths { sheet, sizes: sizes_before } });
                }
                Op::Batch(ops)
            }
            Op::InsertRowsAt { sheet, at } => {
                self.structural_at(sheet, &at, true);
                let rows = at.iter().enumerate().map(|(i, &p)| p + i as u32).collect();
                Op::DeleteRowsAt { sheet, rows }
            }
            Op::DeleteRowsAt { sheet, rows } => {
                let sh = &self.sheets[sheet];
                let sizes_before: Vec<(u32, Option<f32>)> = rows.iter().filter_map(|r| sh.row_heights.get(r).map(|h| (*r, Some(*h)))).collect();
                let areas = [
                    Op::Merges { sheet, merges: sh.merges.clone() },
                    Op::Filter { sheet, filter: sh.filter.clone() },
                    Op::Validations { sheet, list: sh.validations.clone() },
                    Op::CondFormats { sheet, list: sh.cond_formats.clone() },
                    Op::EditRanges { sheet, list: sh.edit_ranges.clone() },
                    Op::Outline { sheet, rows: true, levels: sh.row_levels.clone() },
                    Op::Images { sheet, list: sh.images.clone() },
                    Op::Tables { sheet, list: sh.tables.clone() },
                ];
                let objects = self.object_snapshots();
                let (removed, rewritten) = self.structural_at(sheet, &rows, false);
                let at = rows.iter().enumerate().map(|(i, &r)| r - i as u32).collect();
                let mut ops = vec![Op::InsertRowsAt { sheet, at }];
                ops.extend(areas);
                ops.extend(objects);
                ops.push(Op::SetCells { sheet, cells: removed.into_iter().map(|(c, cell)| (c, Some(cell))).collect() });
                let mut by_sheet: HashMap<usize, Vec<(CellRef, Option<Cell>)>> = HashMap::default();
                for (k, cell) in rewritten {
                    by_sheet.entry(k.sheet as usize).or_default().push((k.cell(), Some(cell)));
                }
                for (s, cells) in by_sheet {
                    ops.push(Op::SetCells { sheet: s, cells });
                }
                if !sizes_before.is_empty() {
                    ops.push(Op::RowHeights { sheet, sizes: sizes_before });
                }
                Op::Batch(ops)
            }
            Op::ColWidths { .. } | Op::RowHeights { .. } => {
                let rows = matches!(op, Op::RowHeights { .. });
                let (Op::ColWidths { sheet, sizes } | Op::RowHeights { sheet, sizes }) = op else { unreachable!() };
                let map = if rows { &mut self.sheets[sheet].row_heights } else { &mut self.sheets[sheet].col_widths };
                let mut inv = Vec::with_capacity(sizes.len());
                let mut shown = false;
                for (i, s) in sizes {
                    let old = match s {
                        Some(v) => map.insert(i, v),
                        None => map.remove(&i),
                    };
                    shown |= rows && (old == Some(0.0)) != (s == Some(0.0));
                    inv.push((i, old));
                }
                inv.reverse();
                if shown {
                    self.pending.extend(self.visibility.iter().copied());
                }
                if rows { Op::RowHeights { sheet, sizes: inv } } else { Op::ColWidths { sheet, sizes: inv } }
            }
            Op::AddSheet { index, sheet } => {
                self.group.clear();
                self.sheets.insert(index.min(self.sheets.len()), *sheet);
                self.full_rebuild = true;
                Op::RemoveSheet { index }
            }
            Op::RemoveSheet { index } => {
                self.group.clear();
                let sheet = self.sheets.remove(index);
                self.full_rebuild = true;
                Op::AddSheet { index, sheet: Box::new(sheet) }
            }
            Op::RenameSheet { index, name } => {
                let old = std::mem::replace(&mut self.sheets[index].name, name.clone());
                self.objects_rename(&old, &name);
                // Rewrite references in all formulas.
                for sh in &mut self.sheets {
                    let keys: Vec<CellRef> = sh.iter().filter(|(_, c)| c.formula().is_some()).map(|(c, _)| c).collect();
                    for c in keys {
                        if let Some(cell) = sh.get_mut(c) {
                            if let Content::Formula { expr, .. } = &mut cell.content {
                                let mut e = expr.expr().into_owned();
                                if e.rename_sheet(&old, &name) {
                                    *expr = e.into();
                                }
                            }
                        }
                    }
                }
                self.analysis.rename_sheet(&old, &name);
                self.full_rebuild = true;
                Op::RenameSheet { index, name: old }
            }
            Op::Freeze { sheet, rows, cols, top } => {
                let s = &mut self.sheets[sheet];
                let inv = Op::Freeze { sheet, rows: s.frozen_rows, cols: s.frozen_cols, top: s.frozen_top };
                s.frozen_rows = rows;
                s.frozen_cols = cols;
                s.frozen_top = CellRef::new(top.row.min(rows.saturating_sub(1)), top.col.min(cols.saturating_sub(1)));
                inv
            }
            Op::Merges { sheet, merges } => {
                let old = std::mem::replace(&mut self.sheets[sheet].merges, merges);
                Op::Merges { sheet, merges: old }
            }
            Op::SheetProps { index, hidden, tab_color } => {
                let s = &mut self.sheets[index];
                let inv = Op::SheetProps { index, hidden: s.hidden, tab_color: s.tab_color };
                s.hidden = hidden;
                s.tab_color = tab_color;
                inv
            }
            Op::MoveSheet { from, to } => {
                self.group.clear();
                let sh = self.sheets.remove(from);
                self.sheets.insert(to, sh);
                self.full_rebuild = true;
                Op::MoveSheet { from: to, to: from }
            }
            Op::Filter { sheet, filter } => Op::Filter { sheet, filter: std::mem::replace(&mut self.sheets[sheet].filter, filter) },
            Op::Validations { sheet, list } => Op::Validations { sheet, list: std::mem::replace(&mut self.sheets[sheet].validations, list) },
            Op::CondFormats { sheet, list } => Op::CondFormats { sheet, list: std::mem::replace(&mut self.sheets[sheet].cond_formats, list) },
            Op::Outline { sheet, rows, levels } => {
                let s = &mut self.sheets[sheet];
                let old = std::mem::replace(if rows { &mut s.row_levels } else { &mut s.col_levels }, levels);
                Op::Outline { sheet, rows, levels: old }
            }
            Op::Protect { sheet, protection } => {
                let slot = match sheet {
                    Some(i) => &mut self.sheets[i].protection,
                    None => &mut self.protection,
                };
                Op::Protect { sheet, protection: std::mem::replace(slot, protection) }
            }
            Op::EditRanges { sheet, list } => Op::EditRanges { sheet, list: std::mem::replace(&mut self.sheets[sheet].edit_ranges, list) },
            Op::PageSetup { sheet, page } => Op::PageSetup { sheet, page: Box::new(std::mem::replace(&mut self.sheets[sheet].page, *page)) },
            Op::Images { sheet, list } => Op::Images { sheet, list: std::mem::replace(&mut self.sheets[sheet].images, list) },
            Op::Charts { sheet, list } => Op::Charts { sheet, list: std::mem::replace(&mut self.sheets[sheet].charts, list) },
            Op::Sparklines { sheet, list } => Op::Sparklines { sheet, list: std::mem::replace(&mut self.sheets[sheet].sparklines, list) },
            Op::Tables { sheet, list } => {
                // Structured references resolve against the tables.
                self.full_rebuild = true;
                Op::Tables { sheet, list: std::mem::replace(&mut self.sheets[sheet].tables, list) }
            }
            Op::Shapes { sheet, list } => Op::Shapes { sheet, list: std::mem::replace(&mut self.sheets[sheet].shapes, list) },
            Op::Analysis(state) => Op::Analysis(Box::new(std::mem::replace(&mut self.analysis, *state))),
            Op::CellStyles(state) => {
                let old = std::mem::replace(&mut self.cell_styles, *state);
                self.sync_cell_styles(&old.theme);
                Op::CellStyles(Box::new(old))
            }
            Op::Names(list) => {
                self.full_rebuild = true;
                Op::Names(std::mem::replace(&mut self.names, list))
            }
            Op::Externals(list) => {
                self.full_rebuild = true;
                Op::Externals(std::mem::replace(&mut self.externals, list))
            }
            Op::Queries(list) => Op::Queries(std::mem::replace(&mut self.queries, list)),
            Op::Checks(list) => Op::Checks(std::mem::replace(&mut self.checks, list)),
            Op::Calc(o) => {
                // Back to automatic (or iterating otherwise): all again.
                self.full_rebuild |= !o.manual;
                Op::Calc(std::mem::replace(&mut self.calc, o))
            }
            Op::FileParts(p) => {
                let p = *p;
                let mut sheet_kept = Vec::new();
                for (s, k) in self.sheets.iter_mut().zip(p.sheet_kept) {
                    sheet_kept.push(std::mem::replace(&mut s.kept, k));
                }
                let old = FileParts { meta: std::mem::replace(&mut self.meta, p.meta), macros: std::mem::replace(&mut self.macros, p.macros), kept: std::mem::replace(&mut self.kept, p.kept), sheet_kept };
                Op::FileParts(Box::new(old))
            }
            Op::Batch(ops) => {
                let mut inv: Vec<Op> = ops.into_iter().map(|o| self.apply(o)).collect();
                inv.reverse();
                Op::Batch(inv)
            }
        }
    }

    /// Shifts cells and rewrites formulas. Returns removed cells and the
    /// original version of every formula cell that was rewritten (keyed by
    /// its position *before* the edit, for undo).
    fn structural(&mut self, sheet: usize, rows: bool, at: u32, count: i64) -> (Vec<(CellRef, Cell)>, Vec<(CellKey, Cell)>) {
        let target = self.sheets[sheet].name.clone();
        let mut rewritten = Vec::new();
        for si in 0..self.sheets.len() {
            let own = self.sheets[si].name.clone();
            let keys: Vec<CellRef> = self.sheets[si].iter().filter(|(_, c)| c.formula().is_some()).map(|(c, _)| c).collect();
            for c in keys {
                // Deleted cells keep their formula for undo.
                let in_deleted = si == sheet && count < 0 && {
                    let v = if rows { c.row } else { c.col };
                    v >= at && (v as i64) < at as i64 - count
                };
                if in_deleted {
                    continue;
                }
                let Some(mut e) = self.sheets[si].get(c).and_then(|x| x.formula()).map(|f| f.expr().into_owned()) else { continue };
                if e.adjust_structure(&own, &target, rows, at, count) {
                    // Rewritten copies of one formula stay shared.
                    let f = self.placed_formula(si, c, e);
                    let cell = self.sheets[si].get_mut(c).unwrap();
                    // Position before the edit: undo restores cells after
                    // the inverse insert/delete has shifted them back.
                    rewritten.push((CellKey::new(si, c), cell.clone()));
                    if let Content::Formula { expr, .. } = &mut cell.content {
                        *expr = f;
                    }
                }
            }
        }
        self.objects_structure(&target, rows, at, count);
        let removed = if rows { self.sheets[sheet].shift_rows(at, count) } else { self.sheets[sheet].shift_cols(at, count) };
        // Map rewritten keys in the edited sheet to their post-edit location,
        // then back: undo applies the inverse structural op first, which
        // restores original positions, so we keep pre-edit coordinates.
        self.full_rebuild = true;
        (removed, rewritten)
    }

    /// [`Workbook::structural`] for single rows at many points.
    fn structural_at(&mut self, sheet: usize, points: &[u32], insert: bool) -> (Vec<(CellRef, Cell)>, Vec<(CellKey, Cell)>) {
        let target = self.sheets[sheet].name.clone();
        let mut rewritten = Vec::new();
        for si in 0..self.sheets.len() {
            let own = self.sheets[si].name.clone();
            let keys: Vec<CellRef> = self.sheets[si].iter().filter(|(_, c)| c.formula().is_some()).map(|(c, _)| c).collect();
            for c in keys {
                if si == sheet && !insert && points.binary_search(&c.row).is_ok() {
                    continue;
                }
                let Some(mut e) = self.sheets[si].get(c).and_then(|x| x.formula()).map(|f| f.expr().into_owned()) else { continue };
                if e.adjust_rows_at(&own, &target, points, insert) {
                    let f = self.placed_formula(si, c, e);
                    let cell = self.sheets[si].get_mut(c).unwrap();
                    rewritten.push((CellKey::new(si, c), cell.clone()));
                    if let Content::Formula { expr, .. } = &mut cell.content {
                        *expr = f;
                    }
                }
            }
        }
        self.objects_rows_at(&target, points, insert);
        let removed = self.sheets[sheet].shift_rows_at(points, insert);
        self.full_rebuild = true;
        (removed, rewritten)
    }

    // --- high-level editing API ------------------------------------------

    pub fn cell(&self, sheet: usize, c: CellRef) -> Option<&Cell> {
        self.sheets.get(sheet)?.get(c)
    }

    /// Builds cell content from text typed by the user.
    pub fn content_from_input(&self, text: &str) -> (Content, Option<String>) {
        if text.len() > 1 && text.starts_with('=') {
            match formula::parse(text, &self.locale) {
                Ok(e) => return (Content::Formula { expr: e.into(), value: Value::Empty }, None),
                Err(_) => return (Content::Value(Value::text(text)), None),
            }
        }
        let p = format::parse_input(text, &self.locale);
        match p.value {
            Value::Empty => (Content::Empty, None),
            v => (Content::Value(v), p.format),
        }
    }

    /// Validates a formula typed by the user; returns an error message.
    pub fn check_formula(&self, text: &str) -> Result<(), String> {
        if text.len() > 1 && text.starts_with('=') {
            formula::parse(text, &self.locale).map(|_| ()).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }

    fn with_format(&mut self, style: u32, fmt: Option<String>) -> u32 {
        match fmt {
            Some(f) if self.styles.get(style).num_fmt.as_ref() == "General" => {
                let mut s = self.styles.get(style).clone();
                s.num_fmt = Arc::from(f);
                self.styles.intern(s)
            }
            _ => style,
        }
    }

    pub fn set_input(&mut self, sheet: usize, c: CellRef, text: &str) {
        self.set_inputs(sheet, vec![(c, text.to_string())]);
    }

    pub fn set_inputs(&mut self, sheet: usize, inputs: Vec<(CellRef, String)>) {
        let mut cells = Vec::with_capacity(inputs.len());
        for (c, text) in inputs {
            let text = match self.calc.r1c1 && text.starts_with('=') {
                true => formula::r1c1_to_a1(&text, c).unwrap_or(text),
                false => text,
            };
            let (mut content, fmt) = self.content_from_input(&text);
            if let Content::Formula { expr, .. } = &mut content {
                let mut e = expr.expr().into_owned();
                if self.link_formula(&mut e) {
                    *expr = e.into();
                }
            }
            let old = self.cell(sheet, c).cloned();
            let style = self.with_format(old.as_ref().map_or(0, |x| x.style), fmt);
            let mut cell = Cell::with_content(old.as_ref(), content);
            if let Some(x) = &mut cell {
                x.style = style;
            }
            cells.push((c, cell));
        }
        self.execute(Op::SetCells { sheet, cells });
    }

    /// Like [`Self::set_input`], keeping `runs` of mixed formatting when the
    /// input stays text (`None` removes them).
    pub fn set_rich_input(&mut self, sheet: usize, c: CellRef, text: &str, runs: Option<crate::rich::RichText>) {
        let (content, fmt) = self.content_from_input(text);
        let old = self.cell(sheet, c).cloned();
        let style = self.with_format(old.as_ref().map_or(0, |x| x.style), fmt);
        let mut cell = Cell::with_content(old.as_ref(), content);
        if let Some(x) = &mut cell {
            x.style = style;
            let fits = matches!(&x.content, Content::Value(Value::Text(t)) if runs.as_ref().is_some_and(|r| crate::rich::fits(r, t)));
            x.set_extra(|e| e.rich = if fits { runs.map(Arc::new) } else { None });
        }
        self.execute(Op::SetCells { sheet, cells: vec![(c, cell)] });
    }

    /// Clears contents (keeps formatting) of existing cells in `r`.
    pub fn clear_contents(&mut self, sheet: usize, r: &Range) {
        let mut cells = Vec::new();
        self.sheets[sheet].for_each_in(r, |c, cell| {
            if !matches!(cell.content, Content::Empty) {
                cells.push((c, Cell::with_content(Some(cell), Content::Empty)));
            }
        });
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.clear");
        }
    }

    /// Clamps a selection (possibly whole rows/columns) to the used area.
    pub fn clamp_to_used(&self, sheet: usize, r: &Range) -> Option<Range> {
        let ext = self.sheets[sheet].extent()?;
        r.intersect(&Range::new(CellRef::new(0, 0), ext))
    }

    /// Applies a style modification to every cell in `r`.
    pub fn modify_style(&mut self, sheet: usize, r: &Range, f: impl Fn(&mut CellStyle)) {
        self.modify_style_as(sheet, r, "undo.format", f);
    }

    /// `modify_style` naming the undo step.
    pub fn modify_style_as(&mut self, sheet: usize, r: &Range, label: &'static str, f: impl Fn(&mut CellStyle)) {
        let r = if r.cell_count() > 1_000_000 {
            match self.clamp_to_used(sheet, r) {
                Some(x) => x,
                None => return,
            }
        } else {
            *r
        };
        // Still huge (cells far apart): only the cells there are.
        let sparse = (r.cell_count() > 4_000_000).then(|| {
            let mut v = Vec::new();
            self.sheets[sheet].for_each_in(&r, |c, _| v.push(c));
            v
        });
        let all = (r.start.row..=r.end.row).flat_map(|row| (r.start.col..=r.end.col).map(move |col| CellRef::new(row, col)));
        let targets: Box<dyn Iterator<Item = CellRef>> = match sparse {
            Some(v) => Box::new(v.into_iter()),
            None => Box::new(all),
        };
        let mut cache: HashMap<u32, u32> = HashMap::default();
        let mut cells = Vec::new();
        for c in targets {
            let old = self.sheets[sheet].get(c).cloned();
            let old_style = old.as_ref().map_or(0, |x| x.style);
            let new_style = match cache.get(&old_style) {
                Some(&s) => s,
                None => {
                    let mut s = self.styles.get(old_style).clone();
                    f(&mut s);
                    let id = self.styles.intern(s);
                    cache.insert(old_style, id);
                    id
                }
            };
            if new_style != old_style {
                cells.push((c, Cell::with_style(old.as_ref(), new_style)));
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, label);
        }
    }

    /// Copies a block into a clipboard structure.
    pub fn copy(&self, sheet: usize, r: &Range) -> Clip {
        let mut cells = Vec::new();
        self.sheets[sheet].for_each_in(r, |c, cell| {
            let mut cell = cell.clone();
            // Spilled values paste as plain values.
            if let Content::Spill(v) = cell.content {
                cell.content = Content::Value(v);
            }
            cells.push((c.row - r.start.row, c.col - r.start.col, cell));
        });
        let mut ids: Vec<u32> = cells.iter().map(|(_, _, c)| c.style).filter(|&id| id != 0).collect();
        ids.sort_unstable();
        ids.dedup();
        let styles = ids.into_iter().map(|id| (id, self.styles.get(id).clone())).collect();
        let sh = &self.sheets[sheet];
        let sizes = |m: &BTreeMap<u32, f32>, a: u32, b: u32| m.range(a..=b).map(|(i, s)| (i - a, *s)).collect();
        let col_widths = sizes(&sh.col_widths, r.start.col, r.end.col);
        let row_heights = sizes(&sh.row_heights, r.start.row, r.end.row);
        let validations = sh
            .validations
            .iter()
            .filter_map(|v| {
                let ranges: Vec<Range> = v.ranges.iter().filter_map(|x| x.intersect(r)).collect();
                (!ranges.is_empty()).then(|| Validation { ranges, ..v.clone() })
            })
            .collect();
        Clip { rows: r.rows(), cols: r.cols(), origin: r.start, cells, styles, col_widths, row_heights, validations, source: None }
    }

    /// A clip copied in another workbook as one of this workbook: cell
    /// formats are numbers in a workbook's own list, so the cells take
    /// the numbers the same formats have here. Formulas stay as they are
    /// (references to other sheets go by sheet name).
    pub fn adopt_clip(&mut self, clip: &Clip) -> Clip {
        let map: HashMap<u32, u32> = clip.styles.iter().map(|(id, st)| (*id, self.styles.intern(st.clone()))).collect();
        let mut out = clip.clone();
        for (_, _, cell) in &mut out.cells {
            // A format the clip doesn't have (a clip built by hand): none.
            cell.style = if cell.style == 0 { 0 } else { map.get(&cell.style).copied().unwrap_or(0) };
        }
        for (id, _) in &mut out.styles {
            *id = map[id];
        }
        out
    }

    /// A clip of cells copied in another program (an HTML table), as cells
    /// of this workbook. Each text is read as if typed here; the number a
    /// program sends with it wins when the text reads as another one (a
    /// decimal point in a locale with decimal commas). Texts starting with
    /// `=` stay text: they are what a formula showed.
    pub fn clip_from_inputs(&mut self, rows: u32, cols: u32, inputs: Vec<(u32, u32, Option<f64>, String, CellStyle)>, col_widths: Vec<(u32, f32)>) -> Clip {
        let mut cells = Vec::with_capacity(inputs.len());
        let mut ids = Vec::new();
        for (r, c, num, text, style) in inputs {
            let (mut content, mut fmt) = if text.starts_with('=') { (Content::Value(Value::text(text.as_str())), None) } else { self.content_from_input(&text) };
            if let Some(n) = num.filter(|n| n.is_finite()) {
                let same = matches!(content, Content::Value(Value::Number(v)) if (v - n).abs() <= 1e-9 * n.abs().max(1.0));
                if !same {
                    (content, fmt) = (Content::Value(Value::Number(n)), None);
                }
            }
            let base = if style == CellStyle::default() { 0 } else { self.styles.intern(style) };
            let id = self.with_format(base, fmt);
            let cell = Cell { content, style: id, extra: None };
            if cell.is_blank() {
                continue;
            }
            if id != 0 {
                ids.push(id);
            }
            cells.push((r, c, cell));
        }
        ids.sort_unstable();
        ids.dedup();
        let styles = ids.into_iter().map(|id| (id, self.styles.get(id).clone())).collect();
        Clip { rows: rows.max(1), cols: cols.max(1), origin: CellRef::new(0, 0), cells, styles, col_widths, row_heights: Vec::new(), validations: Vec::new(), source: None }
    }

    /// Pastes a clip at `at`, tiling it across `target` when it is a multiple.
    pub fn paste(&mut self, sheet: usize, target: &Range, clip: &Clip) {
        let reps_r = if target.rows() > clip.rows && target.rows() % clip.rows == 0 { target.rows() / clip.rows } else { 1 };
        let reps_c = if target.cols() > clip.cols && target.cols() % clip.cols == 0 { target.cols() / clip.cols } else { 1 };
        let mut out: HashMap<CellRef, Option<Cell>> = HashMap::default();
        for rr in 0..reps_r {
            for rc in 0..reps_c {
                let base = CellRef::new(target.start.row + rr * clip.rows, target.start.col + rc * clip.cols);
                for dr in 0..clip.rows {
                    for dc in 0..clip.cols {
                        let (r, c) = (base.row + dr, base.col + dc);
                        if r < MAX_ROWS && c < MAX_COLS {
                            out.insert(CellRef::new(r, c), None);
                        }
                    }
                }
                for (dr, dc, cell) in &clip.cells {
                    let dst = CellRef::new(base.row + dr, base.col + dc);
                    if dst.row >= MAX_ROWS || dst.col >= MAX_COLS {
                        continue;
                    }
                    let mut cell = cell.clone();
                    if let Content::Formula { expr, .. } = &mut cell.content {
                        let mut e = expr.expr().into_owned();
                        e.offset(dst.row as i64 - (clip.origin.row + dr) as i64, dst.col as i64 - (clip.origin.col + dc) as i64);
                        *expr = e.into();
                    }
                    out.insert(dst, Some(cell));
                }
            }
        }
        let mut cells: Vec<_> = out.into_iter().filter(|(c, v)| v.is_some() || self.sheets[sheet].get(*c).is_some()).collect();
        cells.sort_by_key(|(c, _)| *c);
        self.execute_as(Op::SetCells { sheet, cells }, "undo.paste");
    }

    /// Fills the first row (down) or first column (right) of `r` across it.
    pub fn fill(&mut self, sheet: usize, r: &Range, down: bool) {
        self.fill_dir(sheet, r, if down { FillDir::Down } else { FillDir::Right });
    }

    /// Smart fill (drag handle): extends numeric series, repeats the rest.
    pub fn fill_series(&mut self, sheet: usize, src: &Range, dst: &Range) {
        self.auto_fill(sheet, src, dst, AutoFill::Smart);
    }

    /// Fills `dst` from `src` (inside it) the way Auto Fill Options says.
    pub fn auto_fill(&mut self, sheet: usize, src: &Range, dst: &Range, mode: AutoFill) {
        let down = dst.cols() == src.cols() && dst.start.col == src.start.col;
        let dates = match mode {
            AutoFill::Days => Some(SeriesKind::Days),
            AutoFill::Weekdays => Some(SeriesKind::Weekdays),
            AutoFill::Months => Some(SeriesKind::Months),
            AutoFill::Years => Some(SeriesKind::Years),
            _ => None,
        };
        if let Some(kind) = dates {
            self.fill_series_opts(sheet, dst, SeriesOpts { down, kind, step: 1.0, stop: None });
            return;
        }
        let mut cells = Vec::new();
        let lines = if down { src.cols() } else { src.rows() };
        for line in 0..lines {
            let seq: Vec<Option<Cell>> = (0..if down { src.rows() } else { src.cols() })
                .map(|i| {
                    let c = if down { CellRef::new(src.start.row + i, src.start.col + line) } else { CellRef::new(src.start.row + line, src.start.col + i) };
                    self.sheets[sheet].get(c).cloned()
                })
                .collect();
            let nums: Option<Vec<f64>> = seq
                .iter()
                .map(|c| match c.as_ref().map(|x| &x.content) {
                    Some(Content::Value(Value::Number(n))) => Some(*n),
                    _ => None,
                })
                .collect();
            let step = match &nums {
                _ if mode == AutoFill::Copy => None,
                Some(v) if v.len() >= 2 => Some(v[1] - v[0]),
                Some(v) if v.len() == 1 && mode == AutoFill::Series => Some(1.0),
                // A lone date goes on day by day, as in Excel.
                Some(v) if v.len() == 1 && seq[0].as_ref().is_some_and(|c| self.styles.format(c.style).has_date_part()) => Some(1.0),
                _ => None,
            };
            // Text from a fill list (weekdays, months, the user's lists).
            let texts: Option<Vec<String>> = seq
                .iter()
                .map(|c| match c.as_ref().map(|x| &x.content) {
                    Some(Content::Value(Value::Text(t))) => Some(t.to_string()),
                    _ => None,
                })
                .collect();
            let listed = texts.filter(|t| mode != AutoFill::Copy && crate::analysis::lists::series_item(t, 0, &self.analysis.custom_lists).is_some());
            let span = if down { dst.rows() } else { dst.cols() };
            let origin = if down { src.start.row } else { src.start.col };
            let dst_start = if down { dst.start.row } else { dst.start.col };
            for i in 0..span {
                let pos = dst_start + i;
                if pos >= origin && pos < origin + seq.len() as u32 {
                    continue; // inside the source
                }
                let c = if down { CellRef::new(pos, src.start.col + line) } else { CellRef::new(src.start.row + line, pos) };
                let rel = pos as i64 - origin as i64;
                let src_idx = rel.rem_euclid(seq.len() as i64) as usize;
                let cell = match (step, &nums) {
                    (Some(st), Some(v)) => {
                        let last = *v.last().unwrap();
                        let k = rel - (v.len() as i64 - 1);
                        let style = seq[0].as_ref().map_or(0, |x| x.style);
                        Some(Cell::new(Content::Value(Value::Number(last + st * k as f64)), style))
                    }
                    _ if listed.is_some() => {
                        let text = listed.as_deref().and_then(|t| crate::analysis::lists::series_item(t, rel, &self.analysis.custom_lists));
                        let old = seq[src_idx].as_ref();
                        text.and_then(|t| Cell::with_content(old, Content::Value(Value::text(t))))
                    }
                    _ => seq[src_idx].clone().map(|mut cell| {
                        if let Content::Formula { expr, .. } = &mut cell.content {
                            let mut e = expr.expr().into_owned();
                            let d = pos as i64 - (origin as i64 + src_idx as i64);
                            if down { e.offset(d, 0) } else { e.offset(0, d) }
                            *expr = e.into();
                        }
                        cell
                    }),
                };
                let old = self.sheets[sheet].get(c);
                let cell = match mode {
                    // The source's format over what is there.
                    AutoFill::Formats => Cell::with_style(old, seq[src_idx].as_ref().map_or(0, |x| x.style)),
                    // The values, keeping the cells' own format.
                    AutoFill::Values => cell.and_then(|x| Cell::with_style(Some(&x), old.map_or(0, |o| o.style))).or_else(|| old.and_then(|o| Cell::with_content(Some(o), Content::Empty))),
                    _ => cell,
                };
                cells.push((c, cell));
            }
        }
        if !cells.is_empty() {
            let label = if mode == AutoFill::Smart { "undo.edit" } else { "undo.fill" };
            self.execute_as(Op::SetCells { sheet, cells }, label);
        }
    }

    /// Sorts the rows of `r` by column `key_col` (absolute index).
    pub fn sort(&mut self, sheet: usize, r: &Range, key_col: u32, ascending: bool) {
        self.sort_by(sheet, r, &[SortKey::new(key_col, ascending)], false);
    }

    // --- display ---------------------------------------------------------

    /// Formatted text and optional color for a cell.
    pub fn display(&self, sheet: usize, c: CellRef) -> (String, Option<u32>) {
        match self.cell(sheet, c) {
            None => (String::new(), None),
            Some(cell) => self.styles.format(cell.style).format(cell.value(), &self.locale),
        }
    }

    /// Like `display`, with `FILL` where the cell is padded (accounting).
    pub fn display_marked(&self, sheet: usize, c: CellRef) -> (String, Option<u32>) {
        match self.cell(sheet, c) {
            None => (String::new(), None),
            Some(cell) => self.styles.format(cell.style).format_marked(cell.value(), &self.locale),
        }
    }

    pub fn format_value(&self, style: u32, v: &Value) -> (String, Option<u32>) {
        self.styles.format(style).format(v, &self.locale)
    }

    /// Text shown when editing a cell.
    pub fn edit_text(&self, sheet: usize, c: CellRef) -> String {
        let Some(cell) = self.cell(sheet, c) else { return String::new() };
        match &cell.content {
            Content::Empty => String::new(),
            Content::Formula { expr, .. } if expr.expr().has_external() => self.externals_by_name(&expr.expr()).to_display_in(&self.locale, self.sheets[sheet].table_at(c).map(|t| t.name.as_str())),
            Content::Formula { expr, .. } if self.calc.r1c1 => expr.expr().to_display_r1c1(&self.locale, c),
            Content::Formula { expr, .. } => expr.expr().to_display_in(&self.locale, self.sheets[sheet].table_at(c).map(|t| t.name.as_str())),
            Content::Value(v) | Content::Spill(v) => match v {
                Value::Number(n) => {
                    let f: &NumFormat = self.styles.format(cell.style);
                    if f.is_date() {
                        let code = match (f.has_date_part(), f.has_time()) {
                            (true, true) => format!("{} hh:mm:ss", self.locale.short_date_code()),
                            (false, true) => "hh:mm:ss".to_string(),
                            _ => self.locale.short_date_code().to_string(),
                        };
                        NumFormat::parse(&code).format_number(*n, &self.locale).0
                    } else if f.is_percent() {
                        format!("{}%", format::edit_number(*n * 100.0, self.locale.decimal))
                    } else {
                        format::edit_number(*n, self.locale.decimal)
                    }
                }
                Value::Text(t) => {
                    // Keep text that would otherwise parse as a number as text.
                    if matches!(format::parse_input(t, &self.locale).value, Value::Text(_)) && !t.starts_with('=') {
                        t.to_string()
                    } else {
                        format!("'{}", t)
                    }
                }
                Value::Bool(_) | Value::Error(_) => self.format_value(0, v).0,
                Value::Empty | Value::Array(_) | Value::Lambda(_) => String::new(),
            },
        }
    }

    /// Sum/count/average of numeric cells in a selection (status bar).
    pub fn stats(&self, sheet: usize, r: &Range) -> SelectionStats {
        let mut st = SelectionStats::default();
        let Some(r) = self.clamp_to_used(sheet, r) else { return st };
        self.sheets[sheet].for_each_in(&r, |_, cell| match cell.value() {
            Value::Number(n) => {
                st.count += 1;
                st.numbers += 1;
                st.sum += n;
                st.min = Some(st.min.map_or(*n, |m: f64| m.min(*n)));
                st.max = Some(st.max.map_or(*n, |m: f64| m.max(*n)));
            }
            Value::Empty => {}
            _ => st.count += 1,
        });
        st
    }
}

#[derive(Clone, Debug, Default)]
pub struct SelectionStats {
    pub count: usize,
    pub numbers: usize,
    pub sum: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Clip {
    pub rows: u32,
    pub cols: u32,
    pub origin: CellRef,
    pub cells: Vec<(u32, u32, Cell)>,
    /// The formats of the cells, by their number in the workbook they were
    /// copied from (see `Workbook::adopt_clip`).
    pub styles: Vec<(u32, CellStyle)>,
    /// Its columns and rows that have a size of their own: their number
    /// in the clip (from 0) and the size.
    pub col_widths: Vec<(u32, f32)>,
    pub row_heights: Vec<(u32, f32)>,
    /// The validation rules over its cells, their ranges cut to it (where
    /// they were in the sheet, like `origin`).
    pub validations: Vec<Validation>,
    /// The saved file and the sheet it was copied from, for Paste Link in
    /// another workbook (the program sets it; `copy` doesn't know files).
    pub source: Option<ClipSource>,
}

/// Where the cells of a clip are.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipSource {
    /// The file, as a path.
    pub file: String,
    pub sheet: String,
}

impl EvalContext for Workbook {
    fn value(&self, sheet: usize, cell: CellRef) -> Value {
        self.pending_cells.read(CellKey::new(sheet, cell));
        self.sheets.get(sheet).map(|s| s.value(cell).clone()).unwrap_or(Value::Error(ErrorKind::Ref))
    }

    fn sheet_index(&self, name: &str) -> Option<usize> {
        Workbook::sheet_index(self, name)
    }

    fn text_as_zero(&self, sheet: usize) -> bool {
        self.sheets.get(sheet).is_some_and(|s| s.lotus_eval)
    }

    fn for_each(&self, sheet: usize, range: &Range, f: &mut dyn FnMut(CellRef, &Value)) {
        if let Some(s) = self.sheets.get(sheet) {
            let watch = self.pending_cells.watching();
            s.for_each_in(range, |c, cell| {
                if watch {
                    self.pending_cells.read(CellKey::new(sheet, c));
                }
                let v = cell.value();
                if !v.is_empty() {
                    f(c, v)
                }
            });
        }
    }

    fn used_extent(&self, sheet: usize) -> CellRef {
        self.sheets.get(sheet).and_then(Sheet::extent).unwrap_or_default()
    }

    fn row_hidden(&self, sheet: usize, row: u32) -> Hidden {
        let Some(sh) = self.sheets.get(sheet) else { return Hidden::No };
        if sh.row_heights.get(&row) != Some(&0.0) {
            Hidden::No
        } else if sh.filter.as_ref().is_some_and(|f| f.range.start.row < row && row <= f.range.end.row) {
            Hidden::Filtered
        } else {
            Hidden::Manual
        }
    }

    fn is_subtotal(&self, sheet: usize, cell: CellRef) -> bool {
        self.visibility.contains(&CellKey::new(sheet, cell))
    }

    fn spill_range(&self, sheet: usize, cell: CellRef) -> Option<Range> {
        Workbook::spill_range(self, sheet, cell)
    }

    fn formula_text(&self, sheet: usize, cell: CellRef) -> Option<String> {
        Some(self.sheets.get(sheet)?.get(cell)?.formula()?.expr().to_display(&self.locale))
    }

    fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    fn sheet_name(&self, sheet: usize) -> Option<String> {
        self.sheets.get(sheet).map(|s| s.name.clone())
    }

    fn table_ref(&self, sheet: usize, cell: CellRef, r: &crate::table::StructRef) -> Result<(usize, Range), ErrorKind> {
        self.table_range(sheet, cell, r).ok_or(ErrorKind::Ref)
    }

    fn external(&self, sheet: &str, range: &Range) -> Value {
        self.ext_ranges.get(sheet, range, || crate::external::value(&self.externals, sheet, range))
    }

    fn defined_name(&self, sheet: usize, name: &str) -> Option<Arc<Expr>> {
        if name.starts_with('[') || name.starts_with("'[") {
            return crate::external::name_expr(&self.externals, name);
        }
        self.name_expr(sheet, name)
    }

    fn manual_calc(&self) -> bool {
        self.calc.manual
    }

    fn pivot_value(&self, sheet: usize, cell: CellRef, field: &str, pairs: &[(String, Value)]) -> Option<Value> {
        Workbook::pivot_value(self, sheet, cell, field, pairs)
    }

    fn cell_format(&self, sheet: usize, cell: CellRef) -> (String, f32) {
        let Some(sh) = self.sheets.get(sheet) else { return (String::from("General"), 8.43) };
        let style = sh.get(cell).map_or(0, |c| c.style);
        // Widths are stored in pixels; CELL reports characters.
        (self.styles.get(style).num_fmt.to_string(), sh.col_width(cell.col) / 7.0)
    }
}
