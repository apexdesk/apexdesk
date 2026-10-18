//! Pivot table engine: groups the records of a source range by row and
//! column fields, aggregates value fields and lays the result out as a
//! grid of cells tagged for styling (headers, labels, subtotals, totals).
//!
//! The spec is stored in the workbook (`analysis::State::pivots`) with the
//! output location, so the table can be refreshed or edited later.

use crate::workbook::HashMap;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

pub mod calc;

use super::labels::{Lang, month_short};
use crate::addr::Range;
use crate::format;
use crate::value::{ErrorKind, Value};

/// How the items of a field are grouped.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Grouping {
    #[default]
    None,
    /// Dates (serial numbers) by one calendar unit. Add the same source
    /// column twice (Years, then Months) for Excel's multi-level grouping.
    Date(DatePart),
    /// Numbers in buckets of `by` from `start`; values outside
    /// `start..=end` go to `<start` and `>end`.
    Number { start: f64, end: f64, by: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePart {
    Years,
    Quarters,
    Months,
    /// Day and month (`1-Jan`).
    Days,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    LabelAsc,
    LabelDesc,
    /// By the grand total of the value field with this index.
    ValueAsc(usize),
    ValueDesc(usize),
}

/// A row, column or filter field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    /// Column of the source range (0 = its first column).
    pub col: u32,
    pub group: Grouping,
    pub sort: SortOrder,
    /// Items (labels as shown) left out of the table.
    pub hidden: Vec<String>,
    /// Caption instead of the header of the source column.
    pub name: Option<String>,
    /// Only items whose label passes show.
    pub label_filter: Option<LabelFilter>,
    /// Only items whose total passes show (within their parent item).
    pub value_filter: Option<ValueFilter>,
}

/// Comparisons of a label filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelOp {
    #[default]
    Equals,
    NotEquals,
    BeginsWith,
    NotBeginsWith,
    EndsWith,
    NotEndsWith,
    Contains,
    NotContains,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Between,
    NotBetween,
}

impl LabelOp {
    pub const ALL: [LabelOp; 14] = [
        LabelOp::Equals,
        LabelOp::NotEquals,
        LabelOp::BeginsWith,
        LabelOp::NotBeginsWith,
        LabelOp::EndsWith,
        LabelOp::NotEndsWith,
        LabelOp::Contains,
        LabelOp::NotContains,
        LabelOp::Greater,
        LabelOp::GreaterEq,
        LabelOp::Less,
        LabelOp::LessEq,
        LabelOp::Between,
        LabelOp::NotBetween,
    ];

    /// Takes two operands.
    pub fn two(self) -> bool {
        matches!(self, LabelOp::Between | LabelOp::NotBetween)
    }
}

/// Keeps the items whose label compares as asked (`*` and `?` are
/// wildcards in equals; numbers compare as numbers).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LabelFilter {
    pub op: LabelOp,
    pub a: String,
    pub b: String,
}

fn parse_label_num(s: &str) -> Option<f64> {
    let t = s.trim();
    t.parse::<f64>().ok().or_else(|| t.replace(',', ".").parse().ok()).filter(|n: &f64| n.is_finite())
}

impl LabelFilter {
    pub fn keeps(&self, label: &str) -> bool {
        let l = label.to_lowercase();
        let (a, b) = (self.a.to_lowercase(), self.b.to_lowercase());
        let order = |x: &str, y: &str| -> std::cmp::Ordering {
            match (parse_label_num(x), parse_label_num(y)) {
                (Some(p), Some(q)) => p.total_cmp(&q),
                _ => crate::value::cmp_text_ci(x, y),
            }
        };
        let cmp = |x: &str| order(&l, x);
        let glob = |p: &str| {
            let pc: Vec<char> = p.chars().collect();
            let lc: Vec<char> = l.chars().collect();
            crate::functions::wildcard(&pc, &lc)
        };
        use std::cmp::Ordering::*;
        match self.op {
            LabelOp::Equals => glob(&a),
            LabelOp::NotEquals => !glob(&a),
            LabelOp::BeginsWith => l.starts_with(&a),
            LabelOp::NotBeginsWith => !l.starts_with(&a),
            LabelOp::EndsWith => l.ends_with(&a),
            LabelOp::NotEndsWith => !l.ends_with(&a),
            LabelOp::Contains => l.contains(&a),
            LabelOp::NotContains => !l.contains(&a),
            LabelOp::Greater => cmp(&a) == Greater,
            LabelOp::GreaterEq => cmp(&a) != Less,
            LabelOp::Less => cmp(&a) == Less,
            LabelOp::LessEq => cmp(&a) != Greater,
            LabelOp::Between | LabelOp::NotBetween => {
                let (lo, hi) = if order(&a, &b) == Greater { (&b, &a) } else { (&a, &b) };
                let inside = cmp(lo) != Less && cmp(hi) != Greater;
                inside == (self.op == LabelOp::Between)
            }
        }
    }
}

/// Comparisons and top/bottom rules of a value filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValueOp {
    #[default]
    Equals,
    NotEquals,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Between,
    NotBetween,
    /// The `a` largest items.
    TopItems,
    BottomItems,
    /// The largest items that make up `a` percent of the total.
    TopPercent,
    BottomPercent,
    /// The largest items whose sum reaches `a`.
    TopSum,
    BottomSum,
}

impl ValueOp {
    pub const ALL: [ValueOp; 14] = [
        ValueOp::Equals,
        ValueOp::NotEquals,
        ValueOp::Greater,
        ValueOp::GreaterEq,
        ValueOp::Less,
        ValueOp::LessEq,
        ValueOp::Between,
        ValueOp::NotBetween,
        ValueOp::TopItems,
        ValueOp::BottomItems,
        ValueOp::TopPercent,
        ValueOp::BottomPercent,
        ValueOp::TopSum,
        ValueOp::BottomSum,
    ];

    pub fn two(self) -> bool {
        matches!(self, ValueOp::Between | ValueOp::NotBetween)
    }
}

/// Keeps the items whose total of value field `value` passes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValueFilter {
    pub value: usize,
    pub op: ValueOp,
    pub a: f64,
    pub b: f64,
}

impl ValueFilter {
    /// Which of the sibling items (their totals) stay.
    pub fn keeps(&self, vals: &[Option<f64>]) -> Vec<bool> {
        let (lo, hi) = (self.a.min(self.b), self.a.max(self.b));
        let cmp = |x: f64| match self.op {
            ValueOp::Equals => x == self.a,
            ValueOp::NotEquals => x != self.a,
            ValueOp::Greater => x > self.a,
            ValueOp::GreaterEq => x >= self.a,
            ValueOp::Less => x < self.a,
            ValueOp::LessEq => x <= self.a,
            ValueOp::Between => (lo..=hi).contains(&x),
            ValueOp::NotBetween => !(lo..=hi).contains(&x),
            _ => true,
        };
        let top = matches!(self.op, ValueOp::TopItems | ValueOp::TopPercent | ValueOp::TopSum);
        let mut order: Vec<usize> = (0..vals.len()).filter(|i| vals[*i].is_some()).collect();
        order.sort_by(|a, b| {
            let (x, y) = (vals[*a].unwrap_or(0.0), vals[*b].unwrap_or(0.0));
            if top { y.total_cmp(&x) } else { x.total_cmp(&y) }
        });
        let mut keep = vec![false; vals.len()];
        match self.op {
            ValueOp::TopItems | ValueOp::BottomItems => {
                for i in order.into_iter().take(self.a.max(0.0) as usize) {
                    keep[i] = true;
                }
            }
            ValueOp::TopPercent | ValueOp::BottomPercent | ValueOp::TopSum | ValueOp::BottomSum => {
                let total: f64 = vals.iter().flatten().sum();
                let target = if matches!(self.op, ValueOp::TopPercent | ValueOp::BottomPercent) { total * self.a / 100.0 } else { self.a };
                let mut acc = 0.0;
                for i in order {
                    if acc >= target {
                        break;
                    }
                    keep[i] = true;
                    acc += vals[i].unwrap_or(0.0);
                }
            }
            _ => {
                for (k, v) in keep.iter_mut().zip(vals) {
                    *k = v.is_some_and(cmp);
                }
            }
        }
        keep
    }
}

impl Field {
    pub fn new(col: u32) -> Field {
        Field { col, ..Default::default() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AggFunc {
    #[default]
    Sum,
    /// Non-empty cells (COUNTA).
    Count,
    Average,
    Max,
    Min,
    Product,
    /// Numeric cells (COUNT).
    CountNums,
    StdDev,
    StdDevP,
    Var,
    VarP,
    /// Different values (the Data Model's distinct count).
    DistinctCount,
}

/// Axis and level of a base field for relative "show values as".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseField {
    Row(usize),
    Col(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BaseItem {
    Previous,
    Next,
    /// The item with this label.
    Named(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ShowAs {
    #[default]
    Normal,
    PercentOfGrand,
    /// Of the row's grand total.
    PercentOfRow,
    /// Of the column's grand total.
    PercentOfCol,
    /// Of the parent row item's total.
    PercentOfParent,
    DifferenceFrom(BaseField, BaseItem),
    RunningTotal(BaseField),
    RankAsc(BaseField),
    RankDesc(BaseField),
    /// Percent of the value of a base item.
    PercentOf(BaseField, BaseItem),
    PercentDifferenceFrom(BaseField, BaseItem),
    PercentRunningTotal(BaseField),
    /// Of the parent column item's total.
    PercentOfParentCol,
    /// (value × grand total) / (row total × column total).
    Index,
}

impl ShowAs {
    fn is_percent(&self) -> bool {
        matches!(
            self,
            ShowAs::PercentOfGrand | ShowAs::PercentOfRow | ShowAs::PercentOfCol | ShowAs::PercentOfParent | ShowAs::PercentOf(..) | ShowAs::PercentDifferenceFrom(..) | ShowAs::PercentRunningTotal(_) | ShowAs::PercentOfParentCol
        )
    }
}

/// Value fields with a column from here on show calculated field
/// `col - CALC_BASE` of [`PivotSpec::calculated`].
pub const CALC_BASE: u32 = 1 << 20;

/// Value fields with a column from here on show measure
/// `col - MEASURE_BASE` of [`ModelSource::measures`].
pub const MEASURE_BASE: u32 = 1 << 24;

/// Where a pivot table on the data model takes its fields from: field
/// column `k` is `cols[k]`, a (table, column) of the model. The records are
/// the rows of the table all the used tables relate to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelSource {
    pub cols: Vec<(String, String)>,
    /// Measures (by name) shown as values.
    pub measures: Vec<String>,
}

impl ModelSource {
    /// The field column of a model column, added when new.
    pub fn col_of(&mut self, table: &str, col: &str) -> u32 {
        let pos = self.cols.iter().position(|(t, c)| t.eq_ignore_ascii_case(table) && c.eq_ignore_ascii_case(col));
        pos.unwrap_or_else(|| {
            self.cols.push((table.to_string(), col.to_string()));
            self.cols.len() - 1
        }) as u32
    }

    /// The value column of a measure, added when new.
    pub fn measure_of(&mut self, name: &str) -> u32 {
        let pos = self.measures.iter().position(|m| m.eq_ignore_ascii_case(name));
        MEASURE_BASE
            + pos.unwrap_or_else(|| {
                self.measures.push(name.to_string());
                self.measures.len() - 1
            }) as u32
    }
}

/// A filter on the items of a source column, as the measures of a cell
/// see it: only records whose item (label under `group`, lowercase) is in
/// `items` count.
#[derive(Clone, Debug)]
pub struct ItemFilter {
    pub col: u32,
    pub group: Grouping,
    pub items: Arc<HashSet<String>>,
}

/// Evaluates measure `k` of [`ModelSource::measures`] under some filters
/// (None: blank; NaN: an error).
pub trait MeasureEval: std::fmt::Debug {
    fn eval(&self, k: usize, filters: &[ItemFilter]) -> Option<f64>;
}

/// The label of the item a value falls in under a grouping.
pub fn item_text(v: &Value, g: Grouping, lang: Lang) -> String {
    label_text(&group_value(v, g, lang).1, lang)
}

/// A calculated field: a formula over the source fields, evaluated on
/// their sums.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalcField {
    pub name: String,
    pub formula: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValueField {
    pub col: u32,
    pub func: AggFunc,
    pub show_as: ShowAs,
    /// Caption; default `Sum of <field>` and so on.
    pub name: Option<String>,
    pub number_format: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Row fields share one indented column; subtotals on the group row.
    #[default]
    Compact,
    /// One column per row field; subtotals on the group row.
    Outline,
    /// One column per row field; subtotals below each group.
    Tabular,
}

/// A report filter: only records whose item is selected count.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldFilter {
    pub field: Field,
    /// Selected labels; empty selects everything.
    pub items: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PivotSpec {
    /// Sheet (by name) and range of the source, header row included.
    pub source_sheet: String,
    pub source: Range,
    pub rows: Vec<Field>,
    pub cols: Vec<Field>,
    pub values: Vec<ValueField>,
    pub filters: Vec<FieldFilter>,
    pub layout: Layout,
    pub subtotals: bool,
    /// Grand total row at the bottom.
    pub grand_rows: bool,
    /// Grand total column at the right.
    pub grand_cols: bool,
    pub repeat_labels: bool,
    /// Shown in value cells without data.
    pub empty_text: String,
    /// Language of the captions (Row Labels, Grand Total...).
    pub lang: Lang,
    /// Style of the output (`PivotStyleLight16`, see [`style_look`]).
    pub style: String,
    pub calculated: Vec<CalcField>,
    /// Selections of the slicers: they filter records like report filters
    /// but show nothing in the table.
    pub slicers: Vec<FieldFilter>,
    /// Built on the data model instead of `source_sheet`/`source`.
    pub model: Option<ModelSource>,
}

impl Default for PivotSpec {
    fn default() -> Self {
        PivotSpec {
            source_sheet: String::new(),
            source: Range::default(),
            rows: Vec::new(),
            cols: Vec::new(),
            values: Vec::new(),
            filters: Vec::new(),
            layout: Layout::Compact,
            subtotals: true,
            grand_rows: true,
            grand_cols: true,
            repeat_labels: false,
            empty_text: String::new(),
            lang: Lang::En,
            style: DEFAULT_STYLE.to_string(),
            calculated: Vec::new(),
            slicers: Vec::new(),
            model: None,
        }
    }
}

impl PivotSpec {
    /// The formula of calculated field column `col`, parsed against the
    /// source field names.
    pub fn calc_expr(&self, col: u32, names: &[String]) -> Option<Result<calc::Expr, String>> {
        let f = self.calculated.get(col.checked_sub(CALC_BASE)? as usize)?;
        Some(calc::parse(&f.formula, names, self.lang == Lang::Es))
    }

    /// Source columns read by the table: its fields and the ones the
    /// calculated fields in use name.
    pub fn used_columns(&self, names: &[String]) -> Vec<u32> {
        let mut out: Vec<u32> = self.rows.iter().chain(&self.cols).map(|f| f.col).chain(self.filters.iter().chain(&self.slicers).map(|f| f.field.col)).collect();
        for v in &self.values {
            match self.calc_expr(v.col, names) {
                Some(Ok(e)) => e.fields(&mut out),
                Some(Err(_)) => {}
                None => out.push(v.col),
            }
        }
        out.retain(|c| *c < CALC_BASE);
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The style Excel gives new pivot tables.
pub const DEFAULT_STYLE: &str = "PivotStyleLight16";

/// Built-in style families: (prefix, count).
pub const STYLE_FAMILIES: [(&str, u32); 3] = [("PivotStyleLight", 28), ("PivotStyleMedium", 28), ("PivotStyleDark", 28)];

/// What a pivot style paints on a cell of the given kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PivotLook {
    pub fill: Option<u32>,
    pub color: Option<u32>,
    pub bold: bool,
    /// A line above the cell (grand totals), in this color.
    pub top: Option<u32>,
    /// A line below the cell (captions), in this color.
    pub bottom: Option<u32>,
}

/// The look of a cell kind under a built-in pivot style: seven variants
/// per group (dark 1 and the six accents), four groups per family, after
/// Excel's gallery. Unknown names fall back to the default style.
pub fn style_look(style: &str, kind: PivotCellKind) -> PivotLook {
    use crate::table::{ACCENTS, tint};
    let parsed = STYLE_FAMILIES.iter().enumerate().find_map(|(fi, (p, n))| style.strip_prefix(p).and_then(|k| k.parse::<u32>().ok()).filter(|k| (1..=*n).contains(k)).map(|k| (fi, k)));
    let (fam, k) = parsed.unwrap_or((0, 16));
    let a = ACCENTS[((k - 1) % 7) as usize];
    let group = (k - 1) / 7;
    let white = Some(0xFFFFFF);
    let mut l = PivotLook::default();
    use PivotCellKind as K;
    let caption = matches!(kind, K::Header | K::ColLabel);
    let grand = matches!(kind, K::GrandLabel | K::GrandValue);
    let sub = matches!(kind, K::SubtotalLabel | K::SubtotalValue);
    match (fam, group) {
        (0, 0) => {
            if caption {
                l.bold = kind == K::Header;
                l.bottom = Some(a);
            }
            if grand {
                l.bold = true;
                l.top = Some(a);
            }
            l.bold |= sub || kind == K::FilterLabel;
        }
        (0, 1) | (0, 2) => {
            let band = if group == 1 { tint(a, 0.6) } else { tint(a, 0.8) };
            if caption {
                l.fill = Some(band);
                l.bold = kind == K::Header;
            }
            if grand {
                l.fill = Some(band);
                l.bold = true;
                l.top = Some(a);
            }
            l.bold |= sub || kind == K::FilterLabel;
        }
        (0, _) => {
            if caption {
                l.fill = Some(tint(a, 0.6));
                l.bold = true;
            }
            if sub {
                l.fill = Some(tint(a, 0.8));
                l.bold = true;
            }
            if grand {
                l.fill = Some(tint(a, 0.6));
                l.bold = true;
            }
            l.bold |= kind == K::FilterLabel;
        }
        (1, g) => {
            let head = if g == 1 { tint(a, -0.25) } else { a };
            if kind == K::Header || (kind == K::ColLabel && g != 0) {
                l.fill = Some(head);
                l.color = white;
                l.bold = true;
            } else if kind == K::ColLabel {
                l.fill = Some(tint(a, 0.6));
            }
            if sub {
                l.fill = Some(tint(a, if g == 1 { 0.6 } else { 0.8 }));
                l.bold = true;
            }
            if grand {
                if g == 3 {
                    l.fill = Some(head);
                    l.color = white;
                } else {
                    l.fill = Some(tint(a, 0.6));
                    l.top = Some(head);
                }
                l.bold = true;
            }
            if g >= 2 && matches!(kind, K::RowLabel | K::Value) {
                l.fill = Some(tint(a, 0.8));
            }
            l.bold |= kind == K::FilterLabel;
        }
        _ => {
            let dark = tint(a, -0.5);
            if kind == K::Header {
                l.fill = Some(dark);
                l.color = white;
                l.bold = true;
            }
            if kind == K::ColLabel {
                l.fill = Some(tint(a, -0.25));
                l.color = white;
            }
            if matches!(kind, K::RowLabel | K::Value) {
                l.fill = Some(tint(a, if group % 2 == 0 { 0.6 } else { 0.8 }));
            }
            if sub {
                l.fill = Some(tint(a, 0.4));
                l.bold = true;
            }
            if grand {
                l.fill = Some(dark);
                l.color = white;
                l.bold = true;
            }
            l.bold |= kind == K::FilterLabel;
        }
    }
    l
}

/// What a cell of the output is, for styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PivotCellKind {
    Blank,
    FilterLabel,
    FilterValue,
    /// Captions: value names, "Row Labels", field names.
    Header,
    ColLabel,
    RowLabel,
    Value,
    SubtotalLabel,
    SubtotalValue,
    GrandLabel,
    GrandValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PivotCell {
    pub value: Value,
    pub kind: PivotCellKind,
    /// Indentation level of compact row labels.
    pub indent: u8,
    pub format: Option<Arc<str>>,
}

impl PivotCell {
    fn blank() -> PivotCell {
        PivotCell { value: Value::Empty, kind: PivotCellKind::Blank, indent: 0, format: None }
    }

    fn new(value: Value, kind: PivotCellKind) -> PivotCell {
        PivotCell { value, kind, indent: 0, format: None }
    }
}

/// The laid out table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PivotTable {
    pub cells: Vec<Vec<PivotCell>>,
    /// Rows above the body (filters, captions and column labels).
    pub header_rows: usize,
    /// Columns holding row labels.
    pub label_cols: usize,
    /// The row items (labels, outer field first) behind each output row:
    /// empty for the grand total, None above the body.
    pub row_paths: Vec<Option<Vec<String>>>,
    /// The column items behind each output column (None for labels).
    pub col_paths: Vec<Option<Vec<String>>>,
    /// Leaf rows by leaf value columns, for pivot charts.
    pub chart: PivotChartData,
}

/// What a pivot chart plots: a category per leaf row, a series per leaf
/// column and value field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PivotChartData {
    pub categories: Vec<String>,
    pub series: Vec<(String, Vec<Option<f64>>)>,
}

impl PivotTable {
    pub fn rows(&self) -> usize {
        self.cells.len()
    }

    pub fn cols(&self) -> usize {
        self.cells.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub fn get(&self, r: usize, c: usize) -> Option<&PivotCell> {
        self.cells.get(r)?.get(c)
    }
}

/// Source data: field names (the header row) and columns of values. Only
/// columns the spec uses need to be filled; shorter columns read as empty.
#[derive(Clone, Debug, Default)]
pub struct Source {
    pub names: Vec<String>,
    pub cols: Vec<Vec<Value>>,
    /// Number format of each column's data (dates keep their format in
    /// the labels).
    pub formats: Vec<Option<Arc<str>>>,
    pub records: usize,
    /// The measures of a table on the data model.
    pub measures: Option<Arc<dyn MeasureEval>>,
}

impl Source {
    fn get(&self, row: usize, col: u32) -> &Value {
        static EMPTY: Value = Value::Empty;
        self.cols.get(col as usize).and_then(|c| c.get(row)).unwrap_or(&EMPTY)
    }

    fn name(&self, col: u32) -> String {
        self.names.get(col as usize).cloned().unwrap_or_else(|| format!("Column{}", col + 1))
    }
}

// ---------------------------------------------------------------------------
// Items

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Num(u64),
    Text(String),
    Bool(bool),
    Err(ErrorKind),
    Blank,
}

/// Sort position: numbers < text < booleans < errors < blank.
#[derive(Clone, Debug, PartialEq)]
enum SortKey {
    Num(f64),
    Text(String),
    Bool(bool),
    Err(u8),
    Blank,
}

impl SortKey {
    fn rank(&self) -> u8 {
        match self {
            SortKey::Num(_) => 0,
            SortKey::Text(_) => 1,
            SortKey::Bool(_) => 2,
            SortKey::Err(_) => 3,
            SortKey::Blank => 4,
        }
    }

    fn cmp(&self, o: &SortKey) -> std::cmp::Ordering {
        match (self, o) {
            (SortKey::Num(a), SortKey::Num(b)) => a.total_cmp(b),
            (SortKey::Text(a), SortKey::Text(b)) => crate::value::cmp_text_ci(a, b),
            (SortKey::Bool(a), SortKey::Bool(b)) => a.cmp(b),
            (SortKey::Err(a), SortKey::Err(b)) => a.cmp(b),
            _ => self.rank().cmp(&o.rank()),
        }
    }
}

struct Item {
    label: Value,
    /// Label as text (for filters, subtotal captions and base items).
    text: String,
    sort: SortKey,
}

/// A field's distinct items and the item of every record.
struct FieldItems {
    items: Vec<Item>,
    ids: Vec<u32>,
    /// Items hidden by `Field::hidden` or not selected by a filter.
    excluded: Vec<bool>,
}

fn num_key(n: f64) -> Key {
    // -0.0 and 0.0 are one item.
    Key::Num(if n == 0.0 { 0 } else { n.to_bits() })
}

fn group_value(v: &Value, g: Grouping, lang: Lang) -> (Key, Value, SortKey) {
    match (v, g) {
        (Value::Number(n), Grouping::Date(part)) => {
            let (y, m, d) = format::serial_to_date(*n);
            match part {
                DatePart::Years => (Key::Num(y as u64), Value::Number(y as f64), SortKey::Num(y as f64)),
                DatePart::Quarters => {
                    let q = (m - 1) / 3 + 1;
                    (Key::Num(q as u64), Value::text(lang.fmt("quarter_n", &[&q.to_string()])), SortKey::Num(q as f64))
                }
                DatePart::Months => (Key::Num(m as u64), Value::text(month_short(lang, m)), SortKey::Num(m as f64)),
                DatePart::Days => {
                    let k = m * 100 + d;
                    (Key::Num(k as u64), Value::text(format!("{}-{}", d, month_short(lang, m))), SortKey::Num(k as f64))
                }
            }
        }
        (Value::Number(n), Grouping::Number { start, end, by }) if by > 0.0 && start <= end => {
            let g = |x: f64| crate::format::general_number(x, if lang == Lang::Es { ',' } else { '.' });
            if *n < start {
                (Key::Num(f64::NEG_INFINITY.to_bits()), Value::text(format!("<{}", g(start))), SortKey::Num(f64::NEG_INFINITY))
            } else if *n > end {
                (Key::Num(f64::INFINITY.to_bits()), Value::text(format!(">{}", g(end))), SortKey::Num(f64::INFINITY))
            } else {
                let b = ((n - start) / by).floor();
                let lo = start + b * by;
                let whole = start.fract() == 0.0 && by.fract() == 0.0;
                let hi = if whole { lo + by - 1.0 } else { lo + by };
                (num_key(lo), Value::text(format!("{}-{}", g(lo), g(hi.min(end.max(lo))))), SortKey::Num(lo))
            }
        }
        (Value::Number(n), _) => (num_key(*n), Value::Number(*n), SortKey::Num(*n)),
        (Value::Text(t), _) if t.is_empty() => (Key::Blank, Value::text(lang.text("blank")), SortKey::Blank),
        (Value::Text(t), _) => (Key::Text(t.to_lowercase()), Value::Text(t.clone()), SortKey::Text(t.to_string())),
        (Value::Bool(b), _) => (Key::Bool(*b), Value::Bool(*b), SortKey::Bool(*b)),
        (Value::Error(e), _) => (Key::Err(*e), Value::Error(*e), SortKey::Err(*e as u8)),
        _ => (Key::Blank, Value::text(lang.text("blank")), SortKey::Blank),
    }
}

fn label_text(v: &Value, lang: Lang) -> String {
    match v {
        Value::Number(n) => format::general_number(*n, if lang == Lang::Es { ',' } else { '.' }),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Error(e) => e.as_str().to_string(),
        Value::Text(t) => t.to_string(),
        _ => String::new(),
    }
}

/// A source value as typed, for the per-field cache of `collect_items`.
#[derive(PartialEq, Eq, Hash)]
enum Raw<'a> {
    Num(u64),
    Text(&'a str),
}

fn collect_items(src: &Source, f: &Field, lang: Lang, selected: Option<&[String]>) -> FieldItems {
    let mut map: HashMap<Key, u32> = HashMap::default();
    // Exact texts and numbers seen so far: repeats skip the lowercasing and
    // date grouping of `group_value`.
    let mut seen: HashMap<Raw, u32> = HashMap::default();
    let mut items: Vec<Item> = Vec::new();
    let mut ids = Vec::with_capacity(src.records);
    // Consecutive records often repeat an item: skip the hashing then.
    let mut last: Option<(&Value, u32)> = None;
    for r in 0..src.records {
        let v = src.get(r, f.col);
        if let Some((lv, id)) = last {
            if std::ptr::eq(lv, v) || lv == v {
                ids.push(id);
                continue;
            }
        }
        let raw = match v {
            Value::Number(n) => Some(Raw::Num(n.to_bits())),
            Value::Text(t) => Some(Raw::Text(t)),
            _ => None,
        };
        if let Some(&id) = raw.as_ref().and_then(|k| seen.get(k)) {
            ids.push(id);
            last = Some((v, id));
            continue;
        }
        let (key, label, sort) = group_value(v, f.group, lang);
        let id = *map.entry(key).or_insert_with(|| {
            let text = label_text(&label, lang);
            items.push(Item { label, text, sort });
            (items.len() - 1) as u32
        });
        ids.push(id);
        if let Some(k) = raw {
            seen.insert(k, id);
        }
        last = Some((v, id));
    }
    let lower = |v: &[String]| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
    let hidden = lower(&f.hidden);
    let sel = selected.filter(|s| !s.is_empty()).map(lower);
    let excluded = items
        .iter()
        .map(|it| {
            let t = it.text.to_lowercase();
            hidden.contains(&t) || sel.as_ref().is_some_and(|s| !s.contains(&t)) || f.label_filter.as_ref().is_some_and(|lf| !lf.keeps(&it.text))
        })
        .collect();
    FieldItems { items, ids, excluded }
}

// ---------------------------------------------------------------------------
// Aggregation

#[derive(Clone, Copy, Debug)]
struct Acc {
    count: u32,
    nums: u32,
    sum: f64,
    min: f64,
    max: f64,
    product: f64,
    mean: f64,
    m2: f64,
    distinct: u32,
}

impl Default for Acc {
    fn default() -> Self {
        Acc { count: 0, nums: 0, sum: 0.0, min: f64::INFINITY, max: f64::NEG_INFINITY, product: 1.0, mean: 0.0, m2: 0.0, distinct: 0 }
    }
}

/// The identity of a value for distinct counts (text ignores case).
fn distinct_key(v: &Value) -> Option<Key> {
    match v {
        Value::Number(n) => Some(num_key(*n)),
        Value::Text(t) if t.is_empty() => None,
        Value::Text(t) => Some(Key::Text(t.to_lowercase())),
        Value::Bool(b) => Some(Key::Bool(*b)),
        Value::Error(e) => Some(Key::Err(*e)),
        _ => None,
    }
}

impl Acc {
    fn add(&mut self, v: &Value) {
        match v {
            Value::Empty => {}
            Value::Number(n) => {
                self.count += 1;
                self.nums += 1;
                self.sum += n;
                self.min = self.min.min(*n);
                self.max = self.max.max(*n);
                self.product *= n;
                let d = n - self.mean;
                self.mean += d / self.nums as f64;
                self.m2 += d * (n - self.mean);
            }
            Value::Text(t) if t.is_empty() => {}
            _ => self.count += 1,
        }
    }

    fn result(&self, f: AggFunc) -> Option<f64> {
        let n = self.nums as f64;
        Some(match f {
            AggFunc::Count => self.count as f64,
            AggFunc::CountNums => n,
            AggFunc::DistinctCount => self.distinct as f64,
            _ if self.nums == 0 => return if self.count > 0 && f == AggFunc::Sum { Some(0.0) } else { None },
            AggFunc::Sum => self.sum,
            AggFunc::Average => self.sum / n,
            AggFunc::Max => self.max,
            AggFunc::Min => self.min,
            AggFunc::Product => self.product,
            AggFunc::StdDev => (self.m2 / (n - 1.0)).sqrt(),
            AggFunc::StdDevP => (self.m2 / n).sqrt(),
            AggFunc::Var => self.m2 / (n - 1.0),
            AggFunc::VarP => self.m2 / n,
        })
    }
}

/// A node of the row or column tree; node 0 is the grand total.
struct Node {
    parent: u32,
    item: u32,
    depth: u32,
    children: Vec<u32>,
}

struct Tree {
    nodes: Vec<Node>,
    index: HashMap<(u32, u32), u32>,
}

impl Tree {
    fn new() -> Tree {
        Tree { nodes: vec![Node { parent: 0, item: 0, depth: 0, children: Vec::new() }], index: HashMap::default() }
    }

    fn child(&mut self, parent: u32, item: u32) -> u32 {
        if let Some(&n) = self.index.get(&(parent, item)) {
            return n;
        }
        let id = self.nodes.len() as u32;
        let depth = self.nodes[parent as usize].depth + 1;
        self.nodes.push(Node { parent, item, depth, children: Vec::new() });
        self.nodes[parent as usize].children.push(id);
        self.index.insert((parent, item), id);
        id
    }

    fn find(&self, parent: u32, item: u32) -> Option<u32> {
        self.index.get(&(parent, item)).copied()
    }

    /// Items from the root down to `n`.
    fn path(&self, mut n: u32) -> Vec<u32> {
        let mut p = Vec::new();
        while n != 0 {
            p.push(self.nodes[n as usize].item);
            n = self.nodes[n as usize].parent;
        }
        p.reverse();
        p
    }

    fn ancestor(&self, mut n: u32, depth: u32) -> u32 {
        while self.nodes[n as usize].depth > depth {
            n = self.nodes[n as usize].parent;
        }
        n
    }

    fn walk(&self, from: u32, items: &[u32]) -> Option<u32> {
        items.iter().try_fold(from, |n, it| self.find(n, *it))
    }
}

struct Model<'a> {
    spec: &'a PivotSpec,
    row_items: &'a [FieldItems],
    col_items: &'a [FieldItems],
    rows: Tree,
    cols: Tree,
    cells: HashMap<(u32, u32), u32>,
    accs: Vec<Acc>,
    nv: usize,
    /// Accumulators per cell: one per value field, then the sums of the
    /// columns calculated fields read (`aux`).
    w: usize,
    aux: &'a [u32],
    /// Parsed formula of each value field that is a calculated field.
    calc: &'a [Option<calc::Expr>],
    /// Measures of a table on the data model, the filters every cell
    /// shares and the values worked out so far.
    measures: Option<&'a dyn MeasureEval>,
    base: &'a [ItemFilter],
    measured: RefCell<HashMap<(u32, u32, usize), Option<f64>>>,
}

/// Where the base item of "difference from" and "% of" is.
enum Base {
    /// The cell is the base item.
    Own,
    Other(Option<f64>),
}

impl Model<'_> {
    /// A measure in the filter context of cell (r, c): the shared filters
    /// and the items of its row and column.
    fn measure(&self, r: u32, c: u32, v: usize, k: usize) -> Option<f64> {
        if let Some(x) = self.measured.borrow().get(&(r, c, v)) {
            return *x;
        }
        let eval = self.measures?;
        let mut filters = self.base.to_vec();
        for (tree, items, fields, n) in [(&self.rows, self.row_items, &self.spec.rows, r), (&self.cols, self.col_items, &self.spec.cols, c)] {
            for (l, it) in tree.path(n).iter().enumerate() {
                let text = items[l].items[*it as usize].text.to_lowercase();
                filters.push(ItemFilter { col: fields[l].col, group: fields[l].group, items: Arc::new(HashSet::from([text])) });
            }
        }
        let x = eval.eval(k, &filters);
        self.measured.borrow_mut().insert((r, c, v), x);
        x
    }

    fn raw(&self, r: u32, c: u32, v: usize) -> Option<f64> {
        let i = *self.cells.get(&(r, c))? as usize * self.w;
        if let Some(k) = self.spec.values[v].col.checked_sub(MEASURE_BASE) {
            return self.measure(r, c, v, k as usize);
        }
        if let Some(Some(e)) = self.calc.get(v) {
            let x = e.eval(&|col| self.aux.iter().position(|a| *a == col).map_or(0.0, |k| self.accs[i + self.nv + k].sum));
            return Some(x);
        }
        self.accs[i + v].result(self.spec.values[v].func)
    }

    fn tree(&self, b: BaseField) -> (&Tree, usize) {
        match b {
            BaseField::Row(l) => (&self.rows, l),
            BaseField::Col(l) => (&self.cols, l),
        }
    }

    /// The node standing for `n` with its item at base level replaced by
    /// sibling `k` (index into the ordered siblings).
    fn sibling(&self, tree: &Tree, n: u32, level: usize, sib: u32) -> Option<u32> {
        let path = tree.path(n);
        let anc = tree.ancestor(n, level as u32);
        let s = tree.find(anc, sib)?;
        tree.walk(s, &path[level + 1..])
    }

    /// The value of the base item for cell (r, c); None where the cell has
    /// no item of the base field (subtotals above it) or no such item.
    fn base(&self, r: u32, c: u32, v: usize, b: BaseField, item: &BaseItem) -> Option<Base> {
        let (tree, level) = self.tree(b);
        let n = if matches!(b, BaseField::Row(_)) { r } else { c };
        if (tree.nodes[n as usize].depth as usize) < level + 1 {
            return None;
        }
        let at = |node: u32| if matches!(b, BaseField::Row(_)) { self.raw(node, c, v) } else { self.raw(r, node, v) };
        let anc = tree.ancestor(n, level as u32 + 1);
        let own_item = tree.nodes[anc as usize].item;
        let sibs = &tree.nodes[tree.nodes[anc as usize].parent as usize].children;
        let pos = sibs.iter().position(|s| *s == anc)?;
        let base_item = match item {
            BaseItem::Previous => tree.nodes[*sibs.get(pos.checked_sub(1)?)? as usize].item,
            BaseItem::Next => tree.nodes[*sibs.get(pos + 1)? as usize].item,
            BaseItem::Named(name) => {
                let name = name.to_lowercase();
                let found = self.items_of(b).iter().position(|it| it.text.to_lowercase() == name)? as u32;
                if found == own_item {
                    return Some(Base::Own);
                }
                found
            }
        };
        Some(Base::Other(self.sibling(tree, n, level, base_item).and_then(at)))
    }

    /// The values of the siblings of the cell's base-field item and the
    /// cell's position among them (running totals and ranks).
    fn siblings(&self, r: u32, c: u32, v: usize, b: BaseField) -> Option<(Vec<Option<f64>>, usize)> {
        let (tree, level) = self.tree(b);
        let n = if matches!(b, BaseField::Row(_)) { r } else { c };
        if (tree.nodes[n as usize].depth as usize) < level + 1 {
            return None;
        }
        let at = |node: u32| if matches!(b, BaseField::Row(_)) { self.raw(node, c, v) } else { self.raw(r, node, v) };
        let anc = tree.ancestor(n, level as u32 + 1);
        let sibs = &tree.nodes[tree.nodes[anc as usize].parent as usize].children;
        let vals: Vec<Option<f64>> = sibs.iter().map(|s| self.sibling(tree, n, level, tree.nodes[*s as usize].item).and_then(at)).collect();
        let pos = sibs.iter().position(|s| *s == anc)?;
        Some((vals, pos))
    }

    /// Value as shown (after "show values as").
    fn shown(&self, r: u32, c: u32, v: usize) -> Option<f64> {
        let raw = self.raw(r, c, v);
        let ratio = |den: Option<f64>| match (raw, den) {
            (Some(a), Some(b)) if b != 0.0 => Some(a / b),
            (Some(_), _) => Some(f64::NAN),
            _ => None,
        };
        match &self.spec.values[v].show_as {
            ShowAs::Normal => raw,
            ShowAs::PercentOfGrand => ratio(self.raw(0, 0, v)),
            ShowAs::PercentOfRow => ratio(self.raw(r, 0, v)),
            ShowAs::PercentOfCol => ratio(self.raw(0, c, v)),
            ShowAs::PercentOfParent => ratio(self.raw(if r == 0 { 0 } else { self.rows.nodes[r as usize].parent }, c, v)),
            ShowAs::PercentOfParentCol => ratio(self.raw(r, if c == 0 { 0 } else { self.cols.nodes[c as usize].parent }, v)),
            ShowAs::Index => {
                let (a, rt, ct, g) = (raw?, self.raw(r, 0, v)?, self.raw(0, c, v)?, self.raw(0, 0, v)?);
                Some(if rt * ct == 0.0 { f64::NAN } else { a * g / (rt * ct) })
            }
            ShowAs::DifferenceFrom(b, item) => match self.base(r, c, v, *b, item)? {
                Base::Own => None,
                Base::Other(o) => Some(raw.unwrap_or(0.0) - o.unwrap_or(0.0)),
            },
            ShowAs::PercentOf(b, item) => match self.base(r, c, v, *b, item)? {
                Base::Own => raw.map(|_| 1.0),
                Base::Other(o) => {
                    o?;
                    ratio(o)
                }
            },
            ShowAs::PercentDifferenceFrom(b, item) => match self.base(r, c, v, *b, item)? {
                Base::Own => None,
                Base::Other(o) => {
                    let o = o?;
                    Some(if o == 0.0 { f64::NAN } else { (raw.unwrap_or(0.0) - o) / o })
                }
            },
            ShowAs::RunningTotal(b) | ShowAs::PercentRunningTotal(b) => {
                let pct = matches!(self.spec.values[v].show_as, ShowAs::PercentRunningTotal(_));
                let Some((vals, pos)) = self.siblings(r, c, v, *b) else { return if pct { raw.map(|_| 1.0) } else { raw } };
                let run: f64 = vals[..=pos].iter().flatten().sum();
                if pct {
                    let total: f64 = vals.iter().flatten().sum();
                    Some(if total == 0.0 { f64::NAN } else { run / total })
                } else {
                    Some(run)
                }
            }
            ShowAs::RankAsc(b) | ShowAs::RankDesc(b) => {
                let (vals, _) = self.siblings(r, c, v, *b)?;
                let own = raw?;
                let asc = matches!(self.spec.values[v].show_as, ShowAs::RankAsc(_));
                let better = vals.iter().flatten().filter(|x| if asc { **x < own } else { **x > own }).count();
                Some((better + 1) as f64)
            }
        }
    }

    fn items_of(&self, b: BaseField) -> &[Item] {
        match b {
            BaseField::Row(l) => self.row_items.get(l).map_or(&[], |f| &f.items[..]),
            BaseField::Col(l) => self.col_items.get(l).map_or(&[], |f| &f.items[..]),
        }
    }
}

// ---------------------------------------------------------------------------
// Layout

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Leaf,
    /// A group's own line above its items (compact/outline), carrying the
    /// subtotal when subtotals are on.
    Group,
    /// Subtotal line after a group's items (tabular rows, all columns).
    Bottom,
    Grand,
}

#[derive(Clone, Copy)]
struct Line {
    node: u32,
    kind: LineKind,
}

fn lines(tree: &Tree, n: u32, levels: usize, top: bool, subtotals: bool, out: &mut Vec<Line>) {
    for &ch in &tree.nodes[n as usize].children {
        let internal = (tree.nodes[ch as usize].depth as usize) < levels;
        if !internal {
            out.push(Line { node: ch, kind: LineKind::Leaf });
        } else if top {
            out.push(Line { node: ch, kind: LineKind::Group });
            lines(tree, ch, levels, top, subtotals, out);
        } else {
            lines(tree, ch, levels, top, subtotals, out);
            if subtotals {
                out.push(Line { node: ch, kind: LineKind::Bottom });
            }
        }
    }
}

fn sort_tree(tree: &mut Tree, fields: &[Field], items: &[FieldItems], value_of: &dyn Fn(u32, usize) -> Option<f64>) {
    for n in 0..tree.nodes.len() {
        let depth = tree.nodes[n].depth as usize;
        let Some(f) = fields.get(depth) else { continue };
        let mut ch = std::mem::take(&mut tree.nodes[n].children);
        let its = &items[depth].items;
        let item = |x: &u32| &its[tree.nodes[*x as usize].item as usize].sort;
        match f.sort {
            SortOrder::LabelAsc => ch.sort_by(|a, b| item(a).cmp(item(b))),
            SortOrder::LabelDesc => ch.sort_by(|a, b| item(b).cmp(item(a))),
            SortOrder::ValueAsc(v) | SortOrder::ValueDesc(v) => {
                let key = |x: &u32| value_of(*x, v).unwrap_or(f64::NEG_INFINITY);
                ch.sort_by(|a, b| key(a).total_cmp(&key(b)));
                if matches!(f.sort, SortOrder::ValueDesc(_)) {
                    ch.reverse();
                }
            }
        }
        tree.nodes[n].children = ch;
    }
}

/// A value field's caption (`Sum of Sales`).
pub(crate) fn caption(spec: &PivotSpec, src: &Source, v: &ValueField) -> String {
    if let Some(n) = &v.name {
        return n.clone();
    }
    let key = match v.func {
        AggFunc::Sum => "sum_of",
        AggFunc::Count => "count_of",
        AggFunc::Average => "average_of",
        AggFunc::Max => "max_of",
        AggFunc::Min => "min_of",
        AggFunc::Product => "product_of",
        AggFunc::CountNums => "countnums_of",
        AggFunc::StdDev => "stddev_of",
        AggFunc::StdDevP => "stddevp_of",
        AggFunc::Var => "var_of",
        AggFunc::VarP => "varp_of",
        AggFunc::DistinctCount => "distinct_of",
    };
    // Measures show under their own names.
    if let Some(k) = v.col.checked_sub(MEASURE_BASE) {
        return spec.model.as_ref().and_then(|m| m.measures.get(k as usize)).cloned().unwrap_or_default();
    }
    let field = match v.col.checked_sub(CALC_BASE) {
        Some(k) => spec.calculated.get(k as usize).map_or_else(String::new, |c| c.name.clone()),
        None => src.name(v.col),
    };
    // Calculated fields always add up their parts.
    let key = if v.col >= CALC_BASE { "sum_of" } else { key };
    spec.lang.fmt(key, &[&field])
}

fn field_name(src: &Source, f: &Field) -> String {
    f.name.clone().unwrap_or_else(|| src.name(f.col))
}

/// The distinct items of a field as the table shows them, sorted: the
/// choices of a filter or of hidden items.
pub fn field_labels(src: &Source, f: &Field, lang: Lang) -> Vec<String> {
    let bare = Field { hidden: Vec::new(), sort: SortOrder::LabelAsc, ..f.clone() };
    let fi = collect_items(src, &bare, lang, None);
    let mut items: Vec<&Item> = fi.items.iter().collect();
    items.sort_by(|a, b| a.sort.cmp(&b.sort));
    items.into_iter().map(|i| i.text.clone()).collect()
}

/// Items of every field, the parsed calculated fields and the item paths
/// value filters leave out.
struct Prep {
    row_items: Vec<FieldItems>,
    col_items: Vec<FieldItems>,
    filter_items: Vec<FieldItems>,
    /// Slicer selections (record filters that show nothing).
    slicer_items: Vec<FieldItems>,
    aux: Vec<u32>,
    calc: Vec<Option<calc::Expr>>,
    /// Row and column item paths (item ids from the outer field in) left
    /// out by value filters.
    prune_rows: HashSet<Vec<u32>>,
    prune_cols: HashSet<Vec<u32>>,
    /// What filters, slicers and hidden items leave, for measures.
    base: Vec<ItemFilter>,
}

impl Prep {
    fn new(spec: &PivotSpec, src: &Source) -> Prep {
        let lang = spec.lang;
        let calc: Vec<Option<calc::Expr>> = spec.values.iter().map(|v| spec.calc_expr(v.col, &src.names).and_then(Result::ok)).collect();
        let mut aux = Vec::new();
        for e in calc.iter().flatten() {
            e.fields(&mut aux);
        }
        let mut p = Prep {
            row_items: spec.rows.iter().map(|f| collect_items(src, f, lang, None)).collect(),
            col_items: spec.cols.iter().map(|f| collect_items(src, f, lang, None)).collect(),
            filter_items: spec.filters.iter().map(|f| collect_items(src, &f.field, lang, Some(&f.items))).collect(),
            slicer_items: spec.slicers.iter().map(|f| collect_items(src, &f.field, lang, Some(&f.items))).collect(),
            aux,
            calc,
            prune_rows: HashSet::new(),
            prune_cols: HashSet::new(),
            base: Vec::new(),
        };
        if src.measures.is_some() {
            let fields = spec.filters.iter().map(|f| &f.field).chain(spec.slicers.iter().map(|f| &f.field)).chain(&spec.rows).chain(&spec.cols);
            let items = p.filter_items.iter().chain(&p.slicer_items).chain(&p.row_items).chain(&p.col_items);
            for (f, fi) in fields.zip(items) {
                if fi.excluded.iter().any(|e| *e) {
                    let keep: HashSet<String> = fi.items.iter().zip(&fi.excluded).filter(|(_, e)| !**e).map(|(i, _)| i.text.to_lowercase()).collect();
                    p.base.push(ItemFilter { col: f.col, group: f.group, items: Arc::new(keep) });
                }
            }
        }
        let filtered = |fs: &[Field]| fs.iter().any(|f| f.value_filter.as_ref().is_some_and(|vf| vf.value < spec.values.len()));
        if filtered(&spec.rows) || filtered(&spec.cols) {
            // Value filters look at the totals of a first pass.
            let (rows, cols) = {
                let m = aggregate(spec, src, &p);
                (prune(&m.rows, &spec.rows, &|n, v| m.raw(n, 0, v)), prune(&m.cols, &spec.cols, &|n, v| m.raw(0, n, v)))
            };
            p.prune_rows = rows;
            p.prune_cols = cols;
        }
        p
    }

    /// Whether record `r` counts (filters, hidden items, value filters).
    fn keeps(&self, r: usize) -> bool {
        for fi in self.filter_items.iter().chain(&self.slicer_items).chain(&self.row_items).chain(&self.col_items) {
            if fi.excluded[fi.ids[r] as usize] {
                return false;
            }
        }
        let pruned = |items: &[FieldItems], set: &HashSet<Vec<u32>>| {
            if set.is_empty() {
                return false;
            }
            let mut path = Vec::with_capacity(items.len());
            items.iter().any(|fi| {
                path.push(fi.ids[r]);
                set.contains(&path)
            })
        };
        !pruned(&self.row_items, &self.prune_rows) && !pruned(&self.col_items, &self.prune_cols)
    }
}

/// The item paths value filters leave out of one axis.
fn prune(tree: &Tree, fields: &[Field], value_of: &dyn Fn(u32, usize) -> Option<f64>) -> HashSet<Vec<u32>> {
    let mut out = HashSet::new();
    for n in 0..tree.nodes.len() {
        let depth = tree.nodes[n].depth as usize;
        let Some(vf) = fields.get(depth).and_then(|f| f.value_filter.as_ref()) else { continue };
        let ch = &tree.nodes[n].children;
        let vals: Vec<Option<f64>> = ch.iter().map(|c| value_of(*c, vf.value)).collect();
        for (c, keep) in ch.iter().zip(vf.keeps(&vals)) {
            if !keep {
                out.insert(tree.path(*c));
            }
        }
    }
    out
}

fn aggregate<'a>(spec: &'a PivotSpec, src: &'a Source, p: &'a Prep) -> Model<'a> {
    let nv = spec.values.len();
    let w = nv + p.aux.len();
    let mut m = Model {
        spec,
        row_items: &p.row_items,
        col_items: &p.col_items,
        rows: Tree::new(),
        cols: Tree::new(),
        cells: HashMap::default(),
        accs: Vec::new(),
        nv,
        w,
        aux: &p.aux,
        calc: &p.calc,
        measures: src.measures.as_deref(),
        base: &p.base,
        measured: RefCell::new(HashMap::default()),
    };
    let distinct: Vec<usize> = spec.values.iter().enumerate().filter(|(_, v)| v.func == AggFunc::DistinctCount).map(|(i, _)| i).collect();
    // Each distinct value once, by number: a long text would otherwise
    // be copied for every cell it is counted in.
    let mut keys: HashMap<Key, u32> = HashMap::default();
    let mut seen: HashSet<(u32, u32, u32)> = HashSet::new();
    let mut rpath: Vec<u32> = Vec::new();
    let mut cpath: Vec<u32> = Vec::new();
    // Each record adds to a cell per row and column level it is under.
    let per = (p.row_items.len() + 1).saturating_mul(p.col_items.len() + 1).saturating_mul(w.max(1));
    for r in 0..src.records.min(MAX_WORK / per) {
        if !p.keeps(r) {
            continue;
        }
        rpath.clear();
        rpath.push(0);
        for f in &p.row_items {
            let n = m.rows.child(*rpath.last().unwrap_or(&0), f.ids[r]);
            rpath.push(n);
        }
        cpath.clear();
        cpath.push(0);
        for f in &p.col_items {
            let n = m.cols.child(*cpath.last().unwrap_or(&0), f.ids[r]);
            cpath.push(n);
        }
        // The record's distinct values, each looked up once.
        let dkeys: Vec<Option<u32>> = distinct
            .iter()
            .map(|&vi| {
                distinct_key(src.get(r, spec.values[vi].col)).map(|key| {
                    let next = keys.len() as u32;
                    *keys.entry(key).or_insert(next)
                })
            })
            .collect();
        for &rn in &rpath {
            for &cn in &cpath {
                let next = m.cells.len() as u32;
                // Past the budget, new cells are left out (the layout
                // shows at most MAX_PIVOT_CELLS anyway).
                if (next as usize + 1).saturating_mul(w) > MAX_ACCS && !m.cells.contains_key(&(rn, cn)) {
                    continue;
                }
                let idx = *m.cells.entry((rn, cn)).or_insert(next) as usize;
                if (idx + 1) * w > m.accs.len() {
                    m.accs.resize((idx + 1) * w, Acc::default());
                }
                for (vi, vf) in spec.values.iter().enumerate() {
                    m.accs[idx * w + vi].add(src.get(r, vf.col));
                }
                for (k, col) in p.aux.iter().enumerate() {
                    m.accs[idx * w + nv + k].add(src.get(r, *col));
                }
                for (&vi, key) in distinct.iter().zip(&dkeys) {
                    if let Some(key) = *key {
                        if seen.len() < MAX_ACCS && seen.insert((idx as u32, vi as u32, key)) {
                            m.accs[idx * w + vi].distinct += 1;
                        }
                    }
                }
            }
        }
    }
    m
}

/// Accumulators a table may take (cells by values), and the most
/// value fields and row or column fields it uses: a file could ask for
/// any number.
const MAX_ACCS: usize = 1 << 22;
/// Additions a table may make in all (records past them are left out).
const MAX_WORK: usize = 1 << 30;
pub const MAX_VALUE_FIELDS: usize = 64;
pub const MAX_AXIS_FIELDS: usize = 16;

/// `spec` within [`MAX_VALUE_FIELDS`] and [`MAX_AXIS_FIELDS`].
pub fn bounded(spec: &PivotSpec) -> std::borrow::Cow<'_, PivotSpec> {
    if spec.values.len() <= MAX_VALUE_FIELDS && spec.rows.len() <= MAX_AXIS_FIELDS && spec.cols.len() <= MAX_AXIS_FIELDS {
        return std::borrow::Cow::Borrowed(spec);
    }
    let mut s = spec.clone();
    s.values.truncate(MAX_VALUE_FIELDS);
    s.rows.truncate(MAX_AXIS_FIELDS);
    s.cols.truncate(MAX_AXIS_FIELDS);
    std::borrow::Cow::Owned(s)
}

/// Groups the source and lays out the table.
pub fn compute(spec: &PivotSpec, src: &Source) -> PivotTable {
    let spec = &*bounded(spec);
    let p = Prep::new(spec, src);
    let mut m = aggregate(spec, src, &p);
    let nv = spec.values.len();
    // Order the items.
    let mut rows = std::mem::replace(&mut m.rows, Tree::new());
    sort_tree(&mut rows, &spec.rows, &p.row_items, &|n, v| if v < nv { m.raw(n, 0, v) } else { None });
    m.rows = rows;
    let mut cols = std::mem::replace(&mut m.cols, Tree::new());
    sort_tree(&mut cols, &spec.cols, &p.col_items, &|n, v| if v < nv { m.raw(0, n, v) } else { None });
    m.cols = cols;
    layout(&m, src, &p.filter_items)
}

/// The records behind a value of the table (Show Details): those under the
/// row items `rows` and column items `cols` (labels from the outer field
/// in, as in [`PivotTable::row_paths`]) that the filters let through.
pub fn detail_records(spec: &PivotSpec, src: &Source, rows: &[String], cols: &[String]) -> Vec<usize> {
    let p = Prep::new(spec, src);
    let on = |items: &[FieldItems], path: &[String], r: usize| items.iter().zip(path).all(|(fi, l)| fi.items[fi.ids[r] as usize].text.to_lowercase() == l.to_lowercase());
    (0..src.records).filter(|r| p.keeps(*r) && on(&p.row_items, rows, *r) && on(&p.col_items, cols, *r)).collect()
}

/// Cells a laid out table may have.
const MAX_PIVOT_CELLS: usize = 1 << 22;

fn layout(m: &Model, src: &Source, filter_items: &[FieldItems]) -> PivotTable {
    let spec = m.spec;
    let lang = spec.lang;
    let (nrow, ncol, nv) = (spec.rows.len(), spec.cols.len(), spec.values.len());
    let mut rlines = Vec::new();
    if nrow == 0 {
        rlines.push(Line { node: 0, kind: LineKind::Grand });
    } else {
        lines(&m.rows, 0, nrow, spec.layout != Layout::Tabular, spec.subtotals, &mut rlines);
        if spec.grand_rows {
            rlines.push(Line { node: 0, kind: LineKind::Grand });
        }
    }
    let mut clines = Vec::new();
    if ncol == 0 {
        clines.push(Line { node: 0, kind: LineKind::Leaf });
    } else {
        lines(&m.cols, 0, ncol, false, spec.subtotals, &mut clines);
        if spec.grand_cols {
            clines.push(Line { node: 0, kind: LineKind::Grand });
        }
    }
    // No more than a sheet holds: many items on both axes would make a
    // table of billions of cells, mostly empty.
    clines.truncate(crate::addr::MAX_COLS as usize / nv.max(1) + 1);
    let data: Vec<(usize, usize)> = (0..clines.len()).flat_map(|c| (0..nv).map(move |v| (c, v))).collect();
    let label_cols = if spec.layout == Layout::Compact { 1 } else { nrow.max(1) };
    let width = label_cols + data.len();
    rlines.truncate((MAX_PIVOT_CELLS / width.max(1)).max(1));
    let row = || vec![PivotCell::blank(); width];
    let mut cells: Vec<Vec<PivotCell>> = Vec::new();
    let hdr = |s: String| PivotCell::new(Value::text(s), PivotCellKind::Header);

    // Report filters.
    for (f, fi) in spec.filters.iter().zip(filter_items) {
        let shown: Vec<&Item> = fi.items.iter().zip(&fi.excluded).filter(|(_, e)| !**e).map(|(i, _)| i).collect();
        let sel = if shown.len() == fi.items.len() {
            Value::text(lang.text("all"))
        } else if shown.len() == 1 {
            shown[0].label.clone()
        } else {
            Value::text(lang.text("multiple_items"))
        };
        let mut r = vec![PivotCell::new(Value::text(field_name(src, &f.field)), PivotCellKind::FilterLabel), PivotCell::new(sel, PivotCellKind::FilterValue)];
        r.resize(width.max(2), PivotCell::blank());
        cells.push(r);
    }
    if !spec.filters.is_empty() {
        cells.push(row());
    }
    let top = cells.len();

    let captions: Vec<String> = spec.values.iter().map(|v| caption(spec, src, v)).collect();
    let item_of = |items: &'_ [FieldItems], tree: &Tree, n: u32| -> (String, Value) {
        let node = &tree.nodes[n as usize];
        let it = &items[node.depth as usize - 1].items[node.item as usize];
        (it.text.clone(), it.label.clone())
    };
    let label_format = |f: &Field| -> Option<Arc<str>> { if f.group == Grouping::None { src.formats.get(f.col as usize).cloned().flatten() } else { None } };
    // Column headers.
    if ncol > 0 {
        let mut r = row();
        if nv == 1 {
            r[0] = hdr(captions[0].clone());
        }
        if label_cols < width {
            r[label_cols] = hdr(if spec.layout == Layout::Compact { lang.text("column_labels") } else { field_name(src, &spec.cols[0]) });
        }
        cells.push(r);
        for l in 0..ncol {
            let mut r = row();
            // Labels show once, over the first column of their group.
            let mut last: Option<(u32, bool)> = None;
            for (k, &(ci, _)) in data.iter().enumerate() {
                let line = clines[ci];
                let x = label_cols + k;
                let depth = m.cols.nodes[line.node as usize].depth as usize;
                let (key, cell) = match line.kind {
                    LineKind::Grand => ((0, true), (l == 0).then(|| PivotCell::new(Value::text(lang.text("grand_total")), PivotCellKind::GrandLabel))),
                    LineKind::Bottom if depth == l + 1 => {
                        let (t, _) = item_of(&m.col_items, &m.cols, line.node);
                        ((line.node, true), Some(PivotCell::new(Value::text(lang.fmt("item_total", &[&t])), PivotCellKind::SubtotalLabel)))
                    }
                    LineKind::Leaf | LineKind::Group if depth > l => {
                        let a = m.cols.ancestor(line.node, l as u32 + 1);
                        let (_, v) = item_of(&m.col_items, &m.cols, a);
                        let mut c = PivotCell::new(v, PivotCellKind::ColLabel);
                        c.format = label_format(&spec.cols[l]);
                        ((a, false), Some(c))
                    }
                    _ => ((u32::MAX, false), None),
                };
                if last != Some(key) {
                    if let Some(c) = cell {
                        r[x] = c;
                    }
                }
                last = Some(key);
            }
            cells.push(r);
        }
    }
    if nv > 1 || ncol == 0 {
        let mut r = row();
        for (k, &(ci, vi)) in data.iter().enumerate() {
            let text = if clines[ci].kind == LineKind::Grand && ncol > 0 { lang.fmt("total_of", &[&captions[vi]]) } else { captions[vi].clone() };
            r[label_cols + k] = hdr(text);
        }
        cells.push(r);
    }
    // Row field captions go on the last header row.
    if cells.len() == top {
        cells.push(row());
    }
    if let Some(last) = cells.last_mut() {
        if spec.layout == Layout::Compact {
            if nrow > 0 {
                last[0] = hdr(lang.text("row_labels"));
            }
        } else {
            for (i, f) in spec.rows.iter().enumerate() {
                last[i] = hdr(field_name(src, f));
            }
        }
    }
    let header_rows = cells.len();
    let path_texts = |items: &[FieldItems], tree: &Tree, n: u32| -> Vec<String> { tree.path(n).iter().enumerate().map(|(l, it)| items[l].items[*it as usize].text.clone()).collect() };
    let mut row_paths: Vec<Option<Vec<String>>> = vec![None; header_rows];
    let mut col_paths: Vec<Option<Vec<String>>> = vec![None; label_cols];
    col_paths.extend(data.iter().map(|&(ci, _)| Some(path_texts(m.col_items, &m.cols, clines[ci].node))));

    // Body.
    let fmt_of = |vi: usize| -> Option<Arc<str>> {
        let v = &spec.values[vi];
        v.number_format.as_deref().map(Arc::from).or_else(|| v.show_as.is_percent().then(|| Arc::from("0.00%")))
    };
    let empty = if spec.empty_text.is_empty() { Value::Empty } else { Value::text(spec.empty_text.as_str()) };
    let mut printed: Vec<Option<u32>> = vec![None; nrow];
    for line in &rlines {
        let mut r = row();
        let depth = m.rows.nodes[line.node as usize].depth as usize;
        let sub = matches!(line.kind, LineKind::Group | LineKind::Bottom);
        match line.kind {
            LineKind::Grand => {
                r[0] = PivotCell::new(Value::text(lang.text("grand_total")), PivotCellKind::GrandLabel);
            }
            _ if spec.layout == Layout::Compact => {
                let (_, v) = item_of(&m.row_items, &m.rows, line.node);
                r[0] = PivotCell::new(v, if sub { PivotCellKind::SubtotalLabel } else { PivotCellKind::RowLabel });
                r[0].indent = (depth - 1).min(255) as u8;
                r[0].format = label_format(&spec.rows[depth - 1]);
            }
            LineKind::Bottom => {
                let (t, _) = item_of(&m.row_items, &m.rows, line.node);
                r[depth - 1] = PivotCell::new(Value::text(lang.fmt("item_total", &[&t])), PivotCellKind::SubtotalLabel);
                printed.iter_mut().skip(depth - 1).for_each(|p| *p = None);
            }
            _ => {
                for l in 0..depth {
                    let a = m.rows.ancestor(line.node, l as u32 + 1);
                    let own = l + 1 == depth;
                    // Outer labels show on the first line of their group
                    // (tabular), or on every line with "repeat labels".
                    let first = printed[l] != Some(a) && spec.layout == Layout::Tabular;
                    if own || first || spec.repeat_labels {
                        let (_, v) = item_of(&m.row_items, &m.rows, a);
                        let kind = if own && sub { PivotCellKind::SubtotalLabel } else { PivotCellKind::RowLabel };
                        r[l] = PivotCell::new(v, kind);
                        r[l].format = label_format(&spec.rows[l]);
                    }
                    printed[l] = Some(a);
                }
            }
        }
        if line.kind != LineKind::Group || spec.subtotals {
            for (k, &(ci, vi)) in data.iter().enumerate() {
                let cl = clines[ci];
                let kind = if line.kind == LineKind::Grand || cl.kind == LineKind::Grand {
                    PivotCellKind::GrandValue
                } else if sub || cl.kind == LineKind::Bottom {
                    PivotCellKind::SubtotalValue
                } else {
                    PivotCellKind::Value
                };
                let value = match m.shown(line.node, cl.node, vi) {
                    Some(x) if x.is_finite() => Value::Number(x),
                    Some(_) => Value::Error(ErrorKind::Div0),
                    None => empty.clone(),
                };
                let mut c = PivotCell::new(value, kind);
                c.format = fmt_of(vi);
                r[label_cols + k] = c;
            }
        }
        cells.push(r);
        row_paths.push(Some(path_texts(m.row_items, &m.rows, line.node)));
    }
    // Chart data: leaf lines only (the grand total without row fields).
    let mut chart = PivotChartData::default();
    let leaf_rows: Vec<&Line> = rlines.iter().filter(|l| l.kind == LineKind::Leaf || nrow == 0).collect();
    chart.categories = leaf_rows.iter().map(|l| if nrow == 0 { lang.text("grand_total") } else { path_texts(m.row_items, &m.rows, l.node).join(" - ") }).collect();
    for &(ci, vi) in &data {
        let cl = clines[ci];
        if cl.kind != LineKind::Leaf {
            continue;
        }
        let mut name = path_texts(m.col_items, &m.cols, cl.node).join(" - ");
        if name.is_empty() || nv > 1 {
            name = if name.is_empty() { captions[vi].clone() } else { format!("{} - {}", name, captions[vi]) };
        }
        let vals = leaf_rows.iter().map(|l| m.shown(l.node, cl.node, vi).filter(|x| x.is_finite())).collect();
        chart.series.push((name, vals));
    }
    PivotTable { cells, header_rows, label_cols, row_paths, col_paths, chart }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Region, Product, Date, Sales, Units
    fn source() -> Source {
        let recs: &[(&str, &str, f64, f64, f64)] = &[
            ("East", "Pen", 45292.0, 10.0, 1.0),
            ("East", "Ink", 45323.0, 20.0, 2.0),
            ("West", "Pen", 45352.0, 5.0, 3.0),
            ("West", "Ink", 45658.0, 7.0, 4.0),
            ("East", "Pen", 45700.0, 30.0, 5.0),
            ("North", "Pad", 45710.0, 1.0, 6.0),
        ];
        let mut cols = vec![Vec::new(); 5];
        for r in recs {
            cols[0].push(Value::text(r.0));
            cols[1].push(Value::text(r.1));
            cols[2].push(Value::Number(r.2));
            cols[3].push(Value::Number(r.3));
            cols[4].push(Value::Number(r.4));
        }
        Source { names: ["Region", "Product", "Date", "Sales", "Units"].iter().map(|s| s.to_string()).collect(), cols, formats: vec![None; 5], records: recs.len(), measures: None }
    }

    fn text(t: &PivotTable) -> Vec<Vec<String>> {
        t.cells
            .iter()
            .map(|r| {
                r.iter()
                    .map(|c| match &c.value {
                        Value::Number(n) => format!("{}", (n * 1000.0).round() / 1000.0),
                        v => label_text(v, Lang::En),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn styles() {
        let d = style_look(DEFAULT_STYLE, PivotCellKind::Header);
        assert!(d.bold && d.fill.is_some());
        assert_eq!(style_look("nonsense", PivotCellKind::GrandValue), style_look(DEFAULT_STYLE, PivotCellKind::GrandValue));
        let m = style_look("PivotStyleMedium2", PivotCellKind::Header);
        assert_eq!((m.fill, m.color), (Some(0x4472C4), Some(0xFFFFFF)));
        let dk = style_look("PivotStyleDark3", PivotCellKind::Value);
        assert!(dk.fill.is_some());
        assert_eq!(style_look("PivotStyleLight1", PivotCellKind::Value), PivotLook::default());
    }

    fn spec(rows: Vec<Field>, cols: Vec<Field>, values: Vec<ValueField>) -> PivotSpec {
        PivotSpec { rows, cols, values, ..Default::default() }
    }

    fn sum(col: u32) -> ValueField {
        ValueField { col, ..Default::default() }
    }

    fn r3(x: f64) -> String {
        format!("{}", (x * 1000.0).round() / 1000.0)
    }

    #[test]
    fn compact_rows_and_columns() {
        let t = compute(&spec(vec![Field::new(0)], vec![Field::new(1)], vec![sum(3)]), &source());
        let g = text(&t);
        assert_eq!(g[0][..2], ["Sum of Sales", "Column Labels"]);
        assert_eq!(g[1], ["Row Labels", "Ink", "Pad", "Pen", "Grand Total"]);
        assert_eq!(g[2], ["East", "20", "", "40", "60"]);
        assert_eq!(g[3], ["North", "", "1", "", "1"]);
        assert_eq!(g[4], ["West", "7", "", "5", "12"]);
        assert_eq!(g[5], ["Grand Total", "27", "1", "45", "73"]);
        assert_eq!(t.header_rows, 2);
        assert_eq!(t.cells[5][4].kind, PivotCellKind::GrandValue);
        assert_eq!(t.cells[2][0].kind, PivotCellKind::RowLabel);
    }

    #[test]
    fn nested_rows_layouts() {
        let mut s = spec(vec![Field::new(0), Field::new(1)], vec![], vec![sum(3)]);
        let t = compute(&s, &source());
        let g = text(&t);
        assert_eq!(g[0], ["Row Labels", "Sum of Sales"]);
        assert_eq!(g[1], ["East", "60"]);
        assert_eq!(g[2], ["Ink", "20"]);
        assert_eq!(g[3], ["Pen", "40"]);
        assert_eq!(g.last().unwrap(), &["Grand Total", "73"]);
        assert_eq!(t.cells[2][0].indent, 1);
        assert_eq!(t.cells[1][0].kind, PivotCellKind::SubtotalLabel);

        s.layout = Layout::Tabular;
        let g = text(&compute(&s, &source()));
        assert_eq!(g[0], ["Region", "Product", "Sum of Sales"]);
        assert_eq!(g[1], ["East", "Ink", "20"]);
        assert_eq!(g[2], ["", "Pen", "40"]);
        assert_eq!(g[3], ["East Total", "", "60"]);
        assert_eq!(g[4], ["North", "Pad", "1"]);
        s.repeat_labels = true;
        let g = text(&compute(&s, &source()));
        assert_eq!(g[2], ["East", "Pen", "40"]);

        s.layout = Layout::Outline;
        s.repeat_labels = false;
        s.subtotals = false;
        let g = text(&compute(&s, &source()));
        assert_eq!(g[1], ["East", "", ""]);
        assert_eq!(g[2], ["", "Ink", "20"]);
    }

    #[test]
    fn functions_and_multiple_values() {
        let mut v2 = sum(4);
        v2.func = AggFunc::Average;
        let mut v3 = sum(0);
        v3.func = AggFunc::Count;
        let g = text(&compute(&spec(vec![Field::new(0)], vec![], vec![sum(3), v2, v3]), &source()));
        assert_eq!(g[0], ["Row Labels", "Sum of Sales", "Average of Units", "Count of Region"]);
        assert_eq!(g[1], ["East", "60", "2.667", "3"]);
        // Two values with a column field: a values row under the items.
        let g = text(&compute(&spec(vec![Field::new(0)], vec![Field::new(1)], vec![sum(3), sum(4)]), &source()));
        assert_eq!(g[1][1..3], ["Ink", ""]);
        assert_eq!(g[2][1..3], ["Sum of Sales", "Sum of Units"]);
        assert_eq!(g[2][7], "Total Sum of Sales");
        assert_eq!(g[3][..3], ["East", "20", "2"]);
    }

    #[test]
    fn show_values_as() {
        let mut v = sum(3);
        v.show_as = ShowAs::PercentOfGrand;
        let t = compute(&spec(vec![Field::new(0)], vec![], vec![v.clone()]), &source());
        let g = text(&t);
        assert_eq!(g[1][1], r3(60.0 / 73.0));
        assert_eq!(t.cells[1][1].format.as_deref(), Some("0.00%"));
        assert_eq!(g[4][1], "1");
        v.show_as = ShowAs::PercentOfRow;
        let g = text(&compute(&spec(vec![Field::new(0)], vec![Field::new(1)], vec![v.clone()]), &source()));
        assert_eq!(g[2][1..], ["0.333", "", "0.667", "1"]);
        v.show_as = ShowAs::PercentOfCol;
        let g = text(&compute(&spec(vec![Field::new(0)], vec![Field::new(1)], vec![v.clone()]), &source()));
        assert_eq!(g[2][1], r3(20.0 / 27.0));
        v.show_as = ShowAs::PercentOfParent;
        let g = text(&compute(&spec(vec![Field::new(0), Field::new(1)], vec![], vec![v.clone()]), &source()));
        assert_eq!(g[2], ["Ink", "0.333"]);
        v.show_as = ShowAs::RunningTotal(BaseField::Row(0));
        let g = text(&compute(&spec(vec![Field::new(0)], vec![], vec![v.clone()]), &source()));
        assert_eq!(g[1..4], [["East", "60"], ["North", "61"], ["West", "73"]]);
        v.show_as = ShowAs::RankDesc(BaseField::Row(0));
        let g = text(&compute(&spec(vec![Field::new(0)], vec![], vec![v.clone()]), &source()));
        assert_eq!(g[1..4], [["East", "1"], ["North", "3"], ["West", "2"]]);
        v.show_as = ShowAs::DifferenceFrom(BaseField::Row(0), BaseItem::Previous);
        let g = text(&compute(&spec(vec![Field::new(0)], vec![], vec![v.clone()]), &source()));
        assert_eq!(g[1..4], [["East", ""], ["North", "-59"], ["West", "11"]]);
        v.show_as = ShowAs::DifferenceFrom(BaseField::Row(0), BaseItem::Named("east".into()));
        let g = text(&compute(&spec(vec![Field::new(0)], vec![], vec![v]), &source()));
        assert_eq!(g[1..4], [["East", ""], ["North", "-59"], ["West", "-48"]]);
    }

    #[test]
    fn grouping_sorting_filters() {
        let mut years = Field::new(2);
        years.group = Grouping::Date(DatePart::Years);
        let mut quarters = Field::new(2);
        quarters.group = Grouping::Date(DatePart::Quarters);
        let g = text(&compute(&spec(vec![years, quarters], vec![], vec![sum(3)]), &source()));
        assert_eq!(g[1], ["2024", "35"]);
        assert_eq!(g[2], ["Qtr1", "35"]);
        assert_eq!(g[3], ["2025", "38"]);
        let mut months = Field::new(2);
        months.group = Grouping::Date(DatePart::Months);
        let g = text(&compute(&spec(vec![months], vec![], vec![sum(3)]), &source()));
        assert_eq!(g[1..4], [["Jan", "17"], ["Feb", "51"], ["Mar", "5"]]);
        let mut units = Field::new(4);
        units.group = Grouping::Number { start: 1.0, end: 6.0, by: 2.0 };
        let g = text(&compute(&spec(vec![units], vec![], vec![sum(3)]), &source()));
        assert_eq!(g[1..4], [["1-2", "30"], ["3-4", "12"], ["5-6", "31"]]);
        // Sort by value, descending; hide an item; filter.
        let mut region = Field::new(0);
        region.sort = SortOrder::ValueDesc(0);
        let mut s = spec(vec![region.clone()], vec![], vec![sum(3)]);
        let g = text(&compute(&s, &source()));
        assert_eq!(g[1][0], "East");
        assert_eq!(g[2][0], "West");
        region.hidden = vec!["west".into()];
        s.rows = vec![region];
        s.filters = vec![FieldFilter { field: Field::new(1), items: vec!["Pen".into()] }];
        let t = compute(&s, &source());
        let g = text(&t);
        assert_eq!(g[0][..2], ["Product", "Pen"]);
        assert_eq!(g[1][0], "");
        assert_eq!(g[3], ["East", "40"]);
        assert_eq!(g[4], ["Grand Total", "40"]);
        assert_eq!(t.header_rows, 3);
        s.filters[0].items = vec!["Pen".into(), "Ink".into()];
        assert_eq!(text(&compute(&s, &source()))[0][1], "(Multiple Items)");
    }

    #[test]
    fn empty_text_and_no_values() {
        let mut s = spec(vec![Field::new(0)], vec![Field::new(1)], vec![sum(3)]);
        s.empty_text = "-".into();
        s.grand_cols = false;
        s.grand_rows = false;
        let g = text(&compute(&s, &source()));
        assert_eq!(g[3], ["North", "-", "1", "-"]);
        assert_eq!(g.len(), 5);
        let g = text(&compute(&spec(vec![Field::new(1)], vec![], vec![]), &source()));
        assert_eq!(g[1..4], [["Ink"], ["Pad"], ["Pen"]]);
    }

    #[test]
    fn large_source() {
        let n = 100_000;
        let regions = ["North", "South", "East", "West", "Center"].map(Value::text);
        let products: Vec<Value> = (0..40).map(|i| Value::text(format!("P{i}"))).collect();
        let mut cols = vec![Vec::with_capacity(n); 4];
        for i in 0..n {
            cols[0].push(regions[i % 5].clone());
            cols[1].push(products[(i * 7) % 40].clone());
            cols[2].push(Value::Number(44927.0 + (i % 730) as f64));
            cols[3].push(Value::Number((i % 97) as f64));
        }
        let src = Source { names: ["R", "P", "D", "S"].iter().map(|s| s.to_string()).collect(), cols, formats: vec![None; 4], records: n, measures: None };
        let mut y = Field::new(2);
        y.group = Grouping::Date(DatePart::Years);
        let t0 = web_time::Instant::now();
        let t = compute(&spec(vec![Field::new(0), Field::new(1)], vec![y], vec![sum(3)]), &src);
        let el = t0.elapsed();
        let last = t.cells.last().unwrap();
        assert_eq!(last[0].value, Value::text("Grand Total"));
        let total: f64 = (0..n).map(|i| (i % 97) as f64).sum();
        assert_eq!(last.last().unwrap().value, Value::Number(total));
        // Generous bound: tests may run unoptimized.
        assert!(el.as_secs_f64() < 5.0, "{el:?}");
    }

    #[test]
    fn distinct_calculated_and_percent_of() {
        let src = source();
        let mut s = spec(vec![Field::new(0)], vec![], vec![ValueField { col: 1, func: AggFunc::DistinctCount, ..Default::default() }]);
        let g = text(&compute(&s, &src));
        assert_eq!(g[0], ["Row Labels", "Distinct Count of Product"]);
        assert_eq!(g[1..], [["East", "2"], ["North", "1"], ["West", "2"], ["Grand Total", "3"]]);

        s.calculated.push(CalcField { name: "Price".into(), formula: "=Sales/Units".into() });
        s.values = vec![ValueField { col: CALC_BASE, ..Default::default() }];
        let g = text(&compute(&s, &src));
        assert_eq!(g[0][1], "Sum of Price");
        assert_eq!(g[1][1], "7.5");
        assert_eq!(g[4][1], r3(73.0 / 21.0));
        assert_eq!(s.used_columns(&src.names), vec![0, 3, 4]);

        s.values = vec![ValueField { col: 3, show_as: ShowAs::PercentOf(BaseField::Row(0), BaseItem::Named("east".into())), ..Default::default() }];
        let t = compute(&s, &src);
        let g = text(&t);
        assert_eq!((g[1][1].as_str(), g[3][1].as_str(), g[4][1].as_str()), ("1", "0.2", ""));
        assert_eq!(t.cells[1][1].format.as_deref(), Some("0.00%"));
        s.values[0].show_as = ShowAs::PercentRunningTotal(BaseField::Row(0));
        let g = text(&compute(&s, &src));
        assert_eq!((g[1][1].as_str(), g[3][1].as_str()), (r3(60.0 / 73.0).as_str(), "1"));
        s.values[0].show_as = ShowAs::PercentDifferenceFrom(BaseField::Row(0), BaseItem::Previous);
        let g = text(&compute(&s, &src));
        assert_eq!((g[1][1].as_str(), g[3][1].as_str()), ("", "11"));
    }

    #[test]
    fn label_value_filters_details_slicers() {
        let src = source();
        let mut s = spec(vec![Field::new(0)], vec![], vec![sum(3)]);
        s.rows[0].value_filter = Some(ValueFilter { value: 0, op: ValueOp::TopItems, a: 1.0, b: 0.0 });
        let g = text(&compute(&s, &src));
        assert_eq!(g[1..], [["East", "60"], ["Grand Total", "60"]]);
        s.rows[0].value_filter = Some(ValueFilter { value: 0, op: ValueOp::Less, a: 20.0, b: 0.0 });
        let g = text(&compute(&s, &src));
        assert_eq!(g[1..], [["North", "1"], ["West", "12"], ["Grand Total", "13"]]);
        // Details follow the filters.
        assert_eq!(detail_records(&s, &src, &["West".into()], &[]), vec![2, 3]);
        assert!(detail_records(&s, &src, &["East".into()], &[]).is_empty());
        assert_eq!(detail_records(&s, &src, &[], &[]), vec![2, 3, 5]);

        s.rows[0].value_filter = None;
        s.rows[0].label_filter = Some(LabelFilter { op: LabelOp::BeginsWith, a: "w".into(), b: String::new() });
        let g = text(&compute(&s, &src));
        assert_eq!(g[1..], [["West", "12"], ["Grand Total", "12"]]);
        s.rows[0].label_filter = Some(LabelFilter { op: LabelOp::Equals, a: "*TH".into(), b: String::new() });
        assert_eq!(text(&compute(&s, &src))[1], ["North", "1"]);
        s.rows[0].label_filter = Some(LabelFilter { op: LabelOp::Between, a: "West".into(), b: "F".into() });
        assert_eq!(text(&compute(&s, &src))[1], ["North", "1"]);
        s.rows[0].label_filter = None;

        // A slicer on Product (not in the table).
        s.slicers.push(FieldFilter { field: Field::new(1), items: vec!["pen".into()] });
        let t = compute(&s, &src);
        assert_eq!(text(&t)[1..], [["East", "40"], ["West", "5"], ["Grand Total", "45"]]);
        assert_eq!(t.row_paths[1], Some(vec!["East".to_string()]));
        assert_eq!(t.row_paths[3], Some(vec![]));
        assert_eq!(t.row_paths[0], None);
        s.slicers.clear();

        // Chart data: leaf rows by leaf columns.
        let t = compute(&spec(vec![Field::new(0)], vec![Field::new(1)], vec![sum(3)]), &src);
        assert_eq!(t.chart.categories, ["East", "North", "West"]);
        assert_eq!(t.chart.series.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["Ink", "Pad", "Pen"]);
        assert_eq!(t.chart.series[2].1, vec![Some(40.0), None, Some(5.0)]);
        assert_eq!(t.col_paths[3], Some(vec!["Pen".to_string()]));
        assert_eq!(t.col_paths[4], Some(vec![]));
    }

    #[test]
    fn value_filter_rules() {
        let v = [Some(10.0), Some(40.0), None, Some(30.0), Some(20.0)];
        let f = |op, a| ValueFilter { value: 0, op, a, b: 0.0 }.keeps(&v);
        assert_eq!(f(ValueOp::TopItems, 2.0), [false, true, false, true, false]);
        assert_eq!(f(ValueOp::BottomItems, 1.0), [true, false, false, false, false]);
        assert_eq!(f(ValueOp::TopPercent, 40.0), [false, true, false, false, false]);
        assert_eq!(f(ValueOp::TopPercent, 60.0), [false, true, false, true, false]);
        assert_eq!(f(ValueOp::BottomSum, 25.0), [true, false, false, false, true]);
        assert_eq!(ValueFilter { value: 0, op: ValueOp::Between, a: 35.0, b: 15.0 }.keeps(&v), [false, false, false, true, true]);
    }
}
