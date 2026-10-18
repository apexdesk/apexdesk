//! Data tools stored with a sheet: autofilter, data validation and
//! conditional formatting. Formulas inside them are kept as canonical text
//! (English names, `,` separators, no leading `=`), like xlsx stores them,
//! and are relative to the top-left cell of their first range.

use crate::addr::{CellRef, MAX_COLS, MAX_ROWS, Range};
use crate::functions::CritOp;

/// Moves `r` after inserting (`count > 0`) or deleting (`count < 0`) rows
/// or columns at `at`. `None` when the whole range was deleted.
pub fn shift_range(r: Range, rows: bool, at: u32, count: i64) -> Option<Range> {
    let (s, e) = if rows { (r.start.row as i64, r.end.row as i64) } else { (r.start.col as i64, r.end.col as i64) };
    let max = if rows { MAX_ROWS } else { MAX_COLS } as i64 - 1;
    let at = at as i64;
    let (ns, ne) = if count >= 0 {
        (if s >= at { s + count } else { s }, if e >= at { e + count } else { e })
    } else {
        let n = -count;
        let map_s = if s < at { s } else if s >= at + n { s - n } else { at };
        let map_e = if e < at { e } else if e >= at + n { e - n } else { at - 1 };
        (map_s, map_e)
    };
    if ne < ns || ns > max {
        return None;
    }
    let ne = ne.min(max);
    Some(if rows {
        Range::new(CellRef::new(ns as u32, r.start.col), CellRef::new(ne as u32, r.end.col))
    } else {
        Range::new(CellRef::new(r.start.row, ns as u32), CellRef::new(r.end.row, ne as u32))
    })
}

/// Shifts every range of a list, dropping deleted ones.
pub fn shift_ranges(v: &[Range], rows: bool, at: u32, count: i64) -> Vec<Range> {
    v.iter().filter_map(|r| shift_range(*r, rows, at, count)).collect()
}

/// The parts of `r` outside `cut` (up to four rectangles).
pub fn range_minus(r: &Range, cut: &Range) -> Vec<Range> {
    let Some(i) = r.intersect(cut) else { return vec![*r] };
    let mut out = Vec::new();
    if i.start.row > r.start.row {
        out.push(Range::new(r.start, CellRef::new(i.start.row - 1, r.end.col)));
    }
    if i.end.row < r.end.row {
        out.push(Range::new(CellRef::new(i.end.row + 1, r.start.col), r.end));
    }
    if i.start.col > r.start.col {
        out.push(Range::new(CellRef::new(i.start.row, r.start.col), CellRef::new(i.end.row, i.start.col - 1)));
    }
    if i.end.col < r.end.col {
        out.push(Range::new(CellRef::new(i.start.row, i.end.col + 1), CellRef::new(i.end.row, r.end.col)));
    }
    out
}

/// `ranges` without the cells of `cut`.
pub fn ranges_minus(ranges: &[Range], cut: &Range) -> Vec<Range> {
    ranges.iter().flat_map(|r| range_minus(r, cut)).collect()
}

// ---------------------------------------------------------------------------
// Autofilter

#[derive(Clone, Debug, PartialEq)]
pub struct AutoFilter {
    /// Header row plus data.
    pub range: Range,
    pub cols: Vec<ColumnFilter>,
    /// Column sorted from its filter button, and whether ascending.
    pub sort: Option<(u32, bool)>,
}

impl AutoFilter {
    pub fn new(range: Range) -> Self {
        AutoFilter { range, cols: Vec::new(), sort: None }
    }

    pub fn column(&self, col: u32) -> Option<&ColumnFilter> {
        self.cols.iter().find(|f| f.col == col)
    }

    /// Rows below the header.
    pub fn data_rows(&self) -> Option<Range> {
        (self.range.end.row > self.range.start.row).then(|| Range::new(CellRef::new(self.range.start.row + 1, self.range.start.col), self.range.end))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnFilter {
    /// Absolute column.
    pub col: u32,
    pub kind: FilterKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FilterKind {
    /// Shows rows whose displayed text is one of `items` (case-insensitive).
    Values { items: Vec<String>, blanks: bool },
    /// One or two conditions; operands may use `*` and `?`.
    Custom { and: bool, a: (CritOp, String), b: Option<(CritOp, String)> },
    Top { top: bool, percent: bool, n: f64 },
    Average { above: bool },
    /// Cell fill (or font color when `font`); `None` = no color.
    Color { font: bool, color: Option<u32> },
}

// ---------------------------------------------------------------------------
// Data validation

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ValKind {
    #[default]
    Any,
    Whole,
    Decimal,
    List,
    Date,
    Time,
    TextLength,
    Custom,
}

impl ValKind {
    pub const ALL: [ValKind; 8] = [ValKind::Any, ValKind::Whole, ValKind::Decimal, ValKind::List, ValKind::Date, ValKind::Time, ValKind::TextLength, ValKind::Custom];

    pub fn xlsx(self) -> &'static str {
        match self {
            ValKind::Any => "none",
            ValKind::Whole => "whole",
            ValKind::Decimal => "decimal",
            ValKind::List => "list",
            ValKind::Date => "date",
            ValKind::Time => "time",
            ValKind::TextLength => "textLength",
            ValKind::Custom => "custom",
        }
    }

    pub fn from_xlsx(s: &str) -> ValKind {
        ValKind::ALL.into_iter().find(|k| k.xlsx() == s).unwrap_or_default()
    }

    /// Uses an operator and bounds.
    pub fn has_op(self) -> bool {
        matches!(self, ValKind::Whole | ValKind::Decimal | ValKind::Date | ValKind::Time | ValKind::TextLength)
    }
}

/// Comparison of validation and "cell value is" rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CmpOp {
    #[default]
    Between,
    NotBetween,
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterEq,
    LessEq,
}

impl CmpOp {
    pub const ALL: [CmpOp; 8] = [CmpOp::Between, CmpOp::NotBetween, CmpOp::Equal, CmpOp::NotEqual, CmpOp::Greater, CmpOp::Less, CmpOp::GreaterEq, CmpOp::LessEq];

    pub fn xlsx(self) -> &'static str {
        match self {
            CmpOp::Between => "between",
            CmpOp::NotBetween => "notBetween",
            CmpOp::Equal => "equal",
            CmpOp::NotEqual => "notEqual",
            CmpOp::Greater => "greaterThan",
            CmpOp::Less => "lessThan",
            CmpOp::GreaterEq => "greaterThanOrEqual",
            CmpOp::LessEq => "lessThanOrEqual",
        }
    }

    pub fn from_xlsx(s: &str) -> CmpOp {
        CmpOp::ALL.into_iter().find(|k| k.xlsx() == s).unwrap_or_default()
    }

    pub fn two_args(self) -> bool {
        matches!(self, CmpOp::Between | CmpOp::NotBetween)
    }

    pub fn test(self, v: f64, a: f64, b: f64) -> bool {
        let (lo, hi) = (a.min(b), a.max(b));
        match self {
            CmpOp::Between => v >= lo && v <= hi,
            CmpOp::NotBetween => v < lo || v > hi,
            CmpOp::Equal => v == a,
            CmpOp::NotEqual => v != a,
            CmpOp::Greater => v > a,
            CmpOp::Less => v < a,
            CmpOp::GreaterEq => v >= a,
            CmpOp::LessEq => v <= a,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ErrorStyle {
    #[default]
    Stop,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Validation {
    pub ranges: Vec<Range>,
    pub kind: ValKind,
    pub op: CmpOp,
    /// Canonical formulas; a literal list is `"a,b,c"` (with the quotes).
    pub f1: String,
    pub f2: String,
    pub allow_blank: bool,
    /// Shows the in-cell dropdown for lists.
    pub dropdown: bool,
    pub show_input: bool,
    pub input_title: String,
    pub input_msg: String,
    pub show_error: bool,
    pub error_style: ErrorStyle,
    pub error_title: String,
    pub error_msg: String,
}

impl Validation {
    pub fn new(ranges: Vec<Range>) -> Self {
        Validation { ranges, allow_blank: true, dropdown: true, show_input: true, show_error: true, ..Default::default() }
    }

    pub fn contains(&self, c: CellRef) -> bool {
        self.ranges.iter().any(|r| r.contains(c))
    }

    /// Items of a literal list (`"a,b,c"`), or `None` for a formula source.
    pub fn literal_items(&self) -> Option<Vec<String>> {
        let s = self.f1.trim();
        let inner = s.strip_prefix('"')?.strip_suffix('"')?;
        Some(inner.replace("\"\"", "\"").split(',').map(|x| x.trim().to_string()).collect())
    }
}

// ---------------------------------------------------------------------------
// Conditional formatting

/// Format applied by a matching rule; `None` keeps the cell's own value.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CfStyle {
    pub color: Option<u32>,
    pub fill: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
}

impl CfStyle {
    pub fn is_empty(&self) -> bool {
        *self == CfStyle::default()
    }

    /// Fills unset properties from a lower-priority rule.
    pub fn merge_under(&mut self, o: &CfStyle) {
        self.color = self.color.or(o.color);
        self.fill = self.fill.or(o.fill);
        self.bold = self.bold.or(o.bold);
        self.italic = self.italic.or(o.italic);
        self.underline = self.underline.or(o.underline);
        self.strike = self.strike.or(o.strike);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextOp {
    #[default]
    Contains,
    NotContains,
    BeginsWith,
    EndsWith,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Period {
    Yesterday,
    #[default]
    Today,
    Tomorrow,
    Last7Days,
    LastWeek,
    ThisWeek,
    NextWeek,
    LastMonth,
    ThisMonth,
    NextMonth,
}

impl Period {
    pub const ALL: [Period; 10] =
        [Period::Yesterday, Period::Today, Period::Tomorrow, Period::Last7Days, Period::LastWeek, Period::ThisWeek, Period::NextWeek, Period::LastMonth, Period::ThisMonth, Period::NextMonth];

    pub fn xlsx(self) -> &'static str {
        match self {
            Period::Yesterday => "yesterday",
            Period::Today => "today",
            Period::Tomorrow => "tomorrow",
            Period::Last7Days => "last7Days",
            Period::LastWeek => "lastWeek",
            Period::ThisWeek => "thisWeek",
            Period::NextWeek => "nextWeek",
            Period::LastMonth => "lastMonth",
            Period::ThisMonth => "thisMonth",
            Period::NextMonth => "nextMonth",
        }
    }

    pub fn from_xlsx(s: &str) -> Period {
        Period::ALL.into_iter().find(|k| k.xlsx() == s).unwrap_or_default()
    }

    /// Whether the date serial `d` falls in the period, `today` being a serial.
    pub fn contains(self, d: f64, today: f64) -> bool {
        let d = d.floor();
        let t = today.floor();
        // Weeks start on Sunday, like Excel. Serial 1 (1900-01-01) is a Sunday.
        let week_start = t - ((t as i64 - 1).rem_euclid(7)) as f64;
        let month = |off: i64| {
            let (y, m, _) = crate::format::serial_to_date(t);
            let mi = y * 12 + m as i64 - 1 + off;
            let (yy, mm) = (mi.div_euclid(12), mi.rem_euclid(12) + 1);
            let a = crate::format::date_to_serial(yy, mm, 1);
            let b = crate::format::date_to_serial(yy, mm + 1, 1);
            d >= a && d < b
        };
        match self {
            Period::Yesterday => d == t - 1.0,
            Period::Today => d == t,
            Period::Tomorrow => d == t + 1.0,
            Period::Last7Days => d > t - 7.0 && d <= t,
            Period::LastWeek => d >= week_start - 7.0 && d < week_start,
            Period::ThisWeek => d >= week_start && d < week_start + 7.0,
            Period::NextWeek => d >= week_start + 7.0 && d < week_start + 14.0,
            Period::LastMonth => month(-1),
            Period::ThisMonth => month(0),
            Period::NextMonth => month(1),
        }
    }
}

/// A threshold of color scales, data bars and icon sets.
#[derive(Clone, Debug, PartialEq)]
pub enum Cfvo {
    Min,
    Max,
    Num(f64),
    Percent(f64),
    Percentile(f64),
    Formula(String),
}

/// Icon sets: xlsx name and number of icons.
pub const ICON_SETS: [(&str, u8); 17] = [
    ("3Arrows", 3),
    ("3ArrowsGray", 3),
    ("3Flags", 3),
    ("3TrafficLights1", 3),
    ("3TrafficLights2", 3),
    ("3Signs", 3),
    ("3Symbols", 3),
    ("3Symbols2", 3),
    ("3Stars", 3),
    ("3Triangles", 3),
    ("4Arrows", 4),
    ("4ArrowsGray", 4),
    ("4RedToBlack", 4),
    ("4Rating", 4),
    ("4TrafficLights", 4),
    ("5Arrows", 5),
    ("5Rating", 5),
];

pub fn icon_count(set: &str) -> usize {
    ICON_SETS.iter().find(|(n, _)| *n == set).map_or(3, |x| x.1 as usize)
}

#[derive(Clone, Debug, PartialEq)]
pub enum CfKind {
    CellIs { op: CmpOp, f1: String, f2: String },
    Expression(String),
    Text { op: TextOp, text: String },
    /// `true`: blank cells; `false`: non-blank.
    Blanks(bool),
    Errors(bool),
    /// `true`: duplicate values; `false`: unique values.
    Duplicates(bool),
    Top { bottom: bool, percent: bool, rank: u32 },
    Average { below: bool, equal: bool },
    Period(Period),
    /// Two or three stops.
    ColorScale(Vec<(Cfvo, u32)>),
    DataBar { color: u32, min: Cfvo, max: Cfvo },
    /// `points` are the lower bounds of icons 2..n (n-1 values).
    IconSet { set: String, reverse: bool, show_value: bool, points: Vec<Cfvo> },
}

impl CfKind {
    /// Rules drawn with a scale, bar or icon instead of a format.
    pub fn is_graphic(&self) -> bool {
        matches!(self, CfKind::ColorScale(_) | CfKind::DataBar { .. } | CfKind::IconSet { .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CfRule {
    pub ranges: Vec<Range>,
    pub kind: CfKind,
    pub style: CfStyle,
    /// Stop If True: lower rules are skipped for matching cells.
    pub stop: bool,
}

impl CfRule {
    pub fn contains(&self, c: CellRef) -> bool {
        self.ranges.iter().any(|r| r.contains(c))
    }

    /// Top-left cell that relative formulas refer to.
    pub fn origin(&self) -> CellRef {
        self.ranges.iter().map(|r| r.start).min_by_key(|c| (c.row, c.col)).unwrap_or_default()
    }
}

/// What conditional formatting does to one cell.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CfResult {
    pub style: CfStyle,
    /// Data bar: fraction of the cell width and color.
    pub bar: Option<(f32, u32)>,
    /// Icon set name and icon index (0 = lowest).
    pub icon: Option<(String, u8)>,
    pub hide_value: bool,
}

impl CfResult {
    pub fn is_empty(&self) -> bool {
        self.style.is_empty() && self.bar.is_none() && self.icon.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: &str) -> Range {
        Range::parse(a).unwrap()
    }

    #[test]
    fn shifting() {
        assert_eq!(shift_range(r("A2:B5"), true, 0, 2), Some(r("A4:B7")));
        assert_eq!(shift_range(r("A2:B5"), true, 3, 2), Some(r("A2:B7")));
        assert_eq!(shift_range(r("A2:B5"), true, 9, 2), Some(r("A2:B5")));
        assert_eq!(shift_range(r("A2:B5"), true, 1, -2), Some(r("A2:B3")));
        assert_eq!(shift_range(r("A2:B5"), true, 0, -1), Some(r("A1:B4")));
        assert_eq!(shift_range(r("A2:B5"), true, 1, -4), None);
        assert_eq!(shift_range(r("B2:D2"), false, 2, -1), Some(r("B2:C2")));
        assert_eq!(shift_range(r("B2:D2"), false, 0, 1), Some(r("C2:E2")));
    }

    #[test]
    fn periods() {
        let t = crate::format::date_to_serial(2026, 9, 24); // Thursday
        assert!(Period::Today.contains(t + 0.5, t));
        assert!(Period::Yesterday.contains(t - 1.0, t));
        assert!(Period::ThisWeek.contains(t - 4.0, t)); // Sunday
        assert!(!Period::ThisWeek.contains(t - 5.0, t));
        assert!(Period::LastWeek.contains(t - 5.0, t));
        assert!(Period::ThisMonth.contains(crate::format::date_to_serial(2026, 9, 1), t));
        assert!(Period::LastMonth.contains(crate::format::date_to_serial(2026, 8, 31), t));
        assert!(Period::NextMonth.contains(crate::format::date_to_serial(2026, 10, 31), t));
    }

    #[test]
    fn literal_list() {
        let mut v = Validation::new(vec![]);
        v.f1 = "\"Sí,No, Tal vez\"".into();
        assert_eq!(v.literal_items().unwrap(), vec!["Sí", "No", "Tal vez"]);
        v.f1 = "$A$1:$A$3".into();
        assert!(v.literal_items().is_none());
    }

    #[test]
    fn minus() {
        let r = Range::parse("A1:C3").unwrap();
        let parts = range_minus(&r, &Range::parse("B2").unwrap());
        assert_eq!(parts.iter().map(|p| p.cell_count()).sum::<u64>(), 8);
        assert_eq!(range_minus(&r, &Range::parse("A1:D9").unwrap()), vec![]);
        assert_eq!(range_minus(&r, &Range::parse("E5").unwrap()), vec![r]);
    }
}
