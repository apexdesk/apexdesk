//! Tables (Excel's ListObjects): a named range with a header row, an
//! optional totals row, per-column settings and a table style.
//!
//! A table lives in its sheet (`Sheet::tables`) and changes through
//! `Op::Tables`, so every edit is undoable. Formulas refer to it with
//! structured references (`Table1[Sales]`, see [`StructRef`]), which the
//! workbook resolves against the current table ranges.

use std::sync::Arc;

use crate::addr::{CellRef, Range};
use crate::sheet::{Line, LineStyle};

/// The function of a column's cell in the totals row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TotalFn {
    #[default]
    None,
    Sum,
    Average,
    /// Non-empty cells (COUNTA).
    Count,
    /// Numeric cells (COUNT).
    CountNums,
    Max,
    Min,
    StdDev,
    Var,
    /// Any formula (`TableColumn::total_formula`).
    Custom,
}

impl TotalFn {
    pub const ALL: [TotalFn; 10] = [TotalFn::None, TotalFn::Average, TotalFn::Count, TotalFn::CountNums, TotalFn::Max, TotalFn::Min, TotalFn::Sum, TotalFn::StdDev, TotalFn::Var, TotalFn::Custom];

    /// The name used by xlsx (`totalsRowFunction`).
    pub fn xlsx(self) -> &'static str {
        match self {
            TotalFn::None => "none",
            TotalFn::Sum => "sum",
            TotalFn::Average => "average",
            TotalFn::Count => "count",
            TotalFn::CountNums => "countNums",
            TotalFn::Max => "max",
            TotalFn::Min => "min",
            TotalFn::StdDev => "stdDev",
            TotalFn::Var => "var",
            TotalFn::Custom => "custom",
        }
    }

    pub fn from_xlsx(s: &str) -> TotalFn {
        TotalFn::ALL.into_iter().find(|f| f.xlsx().eq_ignore_ascii_case(s)).unwrap_or(TotalFn::None)
    }

    /// SUBTOTAL function number (the 1xx forms skip hidden rows).
    pub fn subtotal(self) -> Option<u32> {
        Some(match self {
            TotalFn::Average => 101,
            TotalFn::CountNums => 102,
            TotalFn::Count => 103,
            TotalFn::Max => 104,
            TotalFn::Min => 105,
            TotalFn::StdDev => 107,
            TotalFn::Sum => 109,
            TotalFn::Var => 110,
            _ => return None,
        })
    }

    pub fn from_subtotal(n: u32) -> Option<TotalFn> {
        TotalFn::ALL.into_iter().find(|f| f.subtotal().is_some_and(|k| k == n || k == n + 100))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableColumn {
    pub name: String,
    pub total: TotalFn,
    /// Text in the totals row instead of a function ("Total").
    pub total_label: Option<String>,
    /// Canonical formula of a custom total.
    pub total_formula: Option<String>,
    /// Calculated column: canonical formula (without `=`) as written in the
    /// first data row; new rows get it too.
    pub formula: Option<String>,
    /// Its role in a plan table (see [`crate::plan`]).
    pub field: Option<crate::plan::Field>,
}

impl TableColumn {
    pub fn new(name: impl Into<String>) -> TableColumn {
        TableColumn { name: name.into(), ..Default::default() }
    }
}

/// Look options of a table (xlsx `tableStyleInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct TableStyle {
    /// Built-in style name (`TableStyleMedium2`); empty for none.
    pub name: String,
    pub row_stripes: bool,
    pub col_stripes: bool,
    pub first_col: bool,
    pub last_col: bool,
}

impl Default for TableStyle {
    fn default() -> Self {
        TableStyle { name: DEFAULT_STYLE.to_string(), row_stripes: true, col_stripes: false, first_col: false, last_col: false }
    }
}

/// The style Excel gives new tables.
pub const DEFAULT_STYLE: &str = "TableStyleMedium2";

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Unique in the workbook (xlsx `id`).
    pub id: u32,
    /// Unique in the workbook, as used in formulas.
    pub name: String,
    /// Header, data and totals rows.
    pub range: Range,
    pub header: bool,
    pub totals: bool,
    pub columns: Vec<TableColumn>,
    pub style: TableStyle,
    /// Filter buttons in the header row.
    pub filter_button: bool,
    /// The query that fills the table in the file it came from (kept, not
    /// run).
    pub query: Option<std::sync::Arc<apex_doc::kept::TableQuery>>,
    /// A plan table's settings (Insert > Planning).
    pub plan: Option<Box<crate::plan::Plan>>,
}

impl Table {
    /// A table over `range` whose first row holds the column names.
    pub fn new(id: u32, name: impl Into<String>, range: Range, names: Vec<String>) -> Table {
        Table { id, name: name.into(), range, header: true, totals: false, columns: names.into_iter().map(TableColumn::new).collect(), style: TableStyle::default(), filter_button: true, query: None, plan: None }
    }

    pub fn header_row(&self) -> Option<u32> {
        self.header.then_some(self.range.start.row)
    }

    pub fn totals_row(&self) -> Option<u32> {
        self.totals.then_some(self.range.end.row)
    }

    /// Rows of the body (never empty for a valid table).
    pub fn data_rows(&self) -> (u32, u32) {
        let a = self.range.start.row + self.header as u32;
        let b = self.range.end.row.saturating_sub(self.totals as u32);
        (a, b.max(a))
    }

    pub fn data_range(&self) -> Range {
        let (a, b) = self.data_rows();
        Range::new(CellRef::new(a, self.range.start.col), CellRef::new(b, self.range.end.col))
    }

    /// The part a filter covers: header and data, not the totals row.
    pub fn filter_range(&self) -> Range {
        let (_, b) = self.data_rows();
        Range::new(self.range.start, CellRef::new(b, self.range.end.col))
    }

    /// Index of the column named `name` (case-insensitive).
    pub fn column_index(&self, name: &str) -> Option<usize> {
        let low = name.to_lowercase();
        self.columns.iter().position(|c| c.name.to_lowercase() == low).or_else(|| {
            // Names often carry stray spaces.
            let t = low.trim();
            self.columns.iter().position(|c| c.name.trim().to_lowercase() == t)
        })
    }

    /// Sheet column of the table column `i`.
    pub fn col_at(&self, i: usize) -> u32 {
        self.range.start.col + i as u32
    }

    /// A column name not used yet: `Column1`, `Column2`...
    pub fn fresh_name(&self, base: &str) -> String {
        (1..).map(|i| format!("{base}{i}")).find(|n| self.column_index(n).is_none()).unwrap_or_default()
    }

    /// Resolves a structured reference against this table; `at` is the
    /// cell holding the formula (for `[#This Row]`).
    pub fn resolve(&self, s: &StructRef, at: CellRef) -> Result<Range, ()> {
        let (c0, c1) = match &s.cols {
            None => (0, self.columns.len().saturating_sub(1)),
            Some((a, b)) => {
                let i = self.column_index(a).ok_or(())?;
                let j = match b {
                    Some(b) => self.column_index(b).ok_or(())?,
                    None => i,
                };
                (i.min(j), i.max(j))
            }
        };
        let (d0, d1) = self.data_rows();
        let items = if s.items == 0 { items::DATA } else { s.items };
        let (r0, r1) = if items & items::THIS_ROW != 0 {
            if at.row < d0 || at.row > d1 {
                return Err(());
            }
            (at.row, at.row)
        } else if items & items::ALL != 0 {
            (self.range.start.row, self.range.end.row)
        } else {
            let mut rows: Vec<u32> = Vec::new();
            if items & items::HEADERS != 0 {
                rows.push(self.header_row().ok_or(())?);
            }
            if items & items::DATA != 0 {
                rows.extend([d0, d1]);
            }
            if items & items::TOTALS != 0 {
                rows.push(self.totals_row().ok_or(())?);
            }
            (*rows.iter().min().ok_or(())?, *rows.iter().max().ok_or(())?)
        };
        Ok(Range::new(CellRef::new(r0, self.col_at(c0)), CellRef::new(r1, self.col_at(c1))))
    }

    /// Moves the table after rows or columns are inserted (`count > 0`)
    /// or deleted at `at`. False when the table is gone.
    pub fn shift(&mut self, rows: bool, at: u32, count: i64) -> bool {
        let old = self.range;
        let Some(new) = crate::data::shift_range(old, rows, at, count) else { return false };
        if rows {
            if count < 0 {
                let gone = |r: u32| r >= at && (r as i64) < at as i64 - count;
                if self.header && gone(old.start.row) {
                    self.header = false;
                }
                if self.totals && gone(old.end.row) {
                    self.totals = false;
                }
            }
        } else {
            let first = old.start.col;
            if count < 0 {
                let n = (-count) as u32;
                let (a, b) = (at.max(first), (at + n).min(old.end.col + 1));
                if a < b {
                    self.columns.drain((a - first) as usize..(b - first) as usize);
                }
            } else if at > first && at <= old.end.col {
                let i = (at - first) as usize;
                for k in 0..count as usize {
                    let name = self.fresh_name("Column");
                    self.columns.insert(i + k, TableColumn::new(name));
                }
            }
        }
        self.range = new;
        self.columns.len() as u32 == self.range.cols() && self.range.rows() > self.header as u32 + self.totals as u32
    }

    /// The table style's look for cell `c`, if it is in the table.
    pub fn look(&self, c: CellRef) -> Option<CellLook> {
        if !self.range.contains(c) {
            return None;
        }
        style_look(self, c)
    }
}

/// Parts of a table a structured reference selects (bit flags).
pub mod items {
    pub const ALL: u8 = 1;
    pub const DATA: u8 = 2;
    pub const HEADERS: u8 = 4;
    pub const TOTALS: u8 = 8;
    pub const THIS_ROW: u8 = 16;
}

/// A structured reference: `Table1[Sales]`, `[@Sales]`,
/// `Table1[[#Totals],[Sales]]`, `Table1[#All]`...
#[derive(Clone, Debug, PartialEq)]
pub struct StructRef {
    /// None: the table that holds the formula.
    pub table: Option<Arc<str>>,
    /// [`items`] flags; 0 means the data rows.
    pub items: u8,
    /// A column, or a span of columns; None for all of them.
    pub cols: Option<(Arc<str>, Option<Arc<str>>)>,
}

const SPECIALS: [(&str, u8); 5] = [("#All", items::ALL), ("#Data", items::DATA), ("#Headers", items::HEADERS), ("#Totals", items::TOTALS), ("#This Row", items::THIS_ROW)];
/// Spanish Excel's names for the special items.
const SPECIALS_ES: [(&str, u8); 5] = [("#Todo", items::ALL), ("#Datos", items::DATA), ("#Encabezados", items::HEADERS), ("#Totales", items::TOTALS), ("#Esta fila", items::THIS_ROW)];

fn special(s: &str) -> Option<u8> {
    let s = s.trim();
    SPECIALS.iter().chain(SPECIALS_ES.iter()).find(|(n, _)| n.eq_ignore_ascii_case(s)).map(|(_, f)| *f)
}

/// Reads the text between the outer brackets of a structured reference.
pub fn parse_struct(table: Option<&str>, body: &str) -> Option<StructRef> {
    let table = table.map(Arc::from);
    let mk = |items: u8, cols: Option<(Arc<str>, Option<Arc<str>>)>| Some(StructRef { table: table.clone(), items, cols });
    let t = body.trim();
    if t.is_empty() {
        return mk(0, None);
    }
    if !t.starts_with('[') && !t.starts_with('@') {
        // A lone special item or column name.
        if let Some(f) = special(t) {
            return mk(f, None);
        }
        return mk(0, Some((Arc::from(unescape(t).as_str()), None)));
    }
    // A list of bracketed items, `,` between parts, `:` between columns.
    let mut items = 0u8;
    let mut cols: Vec<String> = Vec::new();
    let mut chars = t.char_indices().peekable();
    let mut range_next = false;
    while let Some((_, ch)) = chars.next() {
        match ch {
            ' ' | ',' => {}
            ':' => range_next = true,
            '@' => {
                items |= items::THIS_ROW;
                // `@Col` without brackets.
                if chars.peek().is_some_and(|(_, c)| *c != '[' && *c != ',' && !c.is_whitespace()) {
                    let mut name = String::new();
                    while let Some(&(_, c)) = chars.peek() {
                        if c == ',' || c == ']' {
                            break;
                        }
                        name.push(c);
                        chars.next();
                    }
                    cols.push(unescape(name.trim()));
                }
            }
            '[' => {
                let mut name = String::new();
                let mut closed = false;
                while let Some((_, c)) = chars.next() {
                    match c {
                        '\'' => {
                            if let Some((_, n)) = chars.next() {
                                name.push(n);
                            }
                        }
                        ']' => {
                            closed = true;
                            break;
                        }
                        c => name.push(c),
                    }
                }
                if !closed {
                    return None;
                }
                match special(&name) {
                    Some(f) if !name.starts_with(' ') => items |= f,
                    _ => {
                        if range_next && cols.len() == 1 {
                            cols.push(name);
                        } else if cols.is_empty() {
                            cols.push(name);
                        } else {
                            return None;
                        }
                    }
                }
                range_next = false;
            }
            _ => return None,
        }
    }
    let cols = match cols.len() {
        0 => None,
        1 => Some((Arc::from(cols[0].as_str()), None)),
        _ => Some((Arc::from(cols[0].as_str()), Some(Arc::from(cols[1].as_str())))),
    };
    mk(items, cols)
}

/// Column names escape `[`, `]`, `#` and `'` with a `'`.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\'' {
            if let Some(n) = it.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '[' | ']' | '#' | '\'') {
            out.push('\'');
        }
        out.push(c);
    }
    out
}

/// Whether a column name can go in single brackets (`Table1[Sales]`).
fn simple(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ' || c == '.') && !name.starts_with(' ') && !name.ends_with(' ')
}

impl StructRef {
    /// Formula text. `xlsx` spells `@` as `[#This Row]` (the file form) and
    /// `own` is a table name to leave out (the formula's own table).
    pub fn write(&self, out: &mut String, xlsx: bool, own: Option<&str>) {
        self.write_sep(out, xlsx, own, ",");
    }

    /// OpenFormula as LibreOffice writes it: `Table1[[#This Row];[Qty]]`.
    pub fn write_odf(&self, out: &mut String) {
        self.write_sep(out, true, None, ";");
    }

    fn write_sep(&self, out: &mut String, xlsx: bool, own: Option<&str>, sep: &str) {
        let table = self.table.as_deref().filter(|t| own.is_none_or(|o| !o.eq_ignore_ascii_case(t) || self.items & items::THIS_ROW == 0));
        if let Some(t) = table {
            out.push_str(t);
        }
        let col = |n: &str| format!("[{}]", escape(n));
        let cols = self.cols.as_ref().map(|(a, b)| match b {
            Some(b) => format!("{}:{}", col(a), col(b)),
            None => col(a),
        });
        let names: Vec<&str> = SPECIALS.iter().filter(|(_, f)| self.items & f != 0).map(|(n, _)| *n).collect();
        let this_row = self.items == items::THIS_ROW;
        out.push('[');
        match (&cols, names.as_slice()) {
            (None, []) => {}
            (None, [one]) if !(this_row && !xlsx) => out.push_str(one),
            (None, _) if this_row => out.push('@'),
            (Some(_), _) if this_row && !xlsx => {
                out.push('@');
                let (a, b) = self.cols.as_ref().map(|(a, b)| (a.clone(), b.clone())).unwrap_or_default();
                match b {
                    None if simple(&a) => out.push_str(&a),
                    _ => out.push_str(cols.as_deref().unwrap_or_default()),
                }
            }
            (Some(c), []) => {
                let (a, b) = self.cols.as_ref().map(|(a, b)| (a.clone(), b.clone())).unwrap_or_default();
                if b.is_none() && simple(&a) {
                    out.push_str(&a);
                } else {
                    out.push_str(c);
                }
            }
            (c, names) => {
                let mut parts: Vec<String> = names.iter().map(|n| format!("[{n}]")).collect();
                parts.extend(c.clone());
                out.push_str(&parts.join(sep));
            }
        }
        out.push(']');
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, false, None);
        s
    }
}

// ---------------------------------------------------------------------------
// Names

/// Whether `name` can name a table: a letter, `_` or `\` first, then
/// letters, digits, `_` and `.`; not a cell reference, not R or C.
/// Column names made unique ignoring case (`Name`, `Name2`), in time
/// linear in their number even when thousands are the same.
#[derive(Default)]
pub struct UniqueNames {
    seen: std::collections::HashSet<String>,
    next: std::collections::HashMap<String, usize>,
}

impl UniqueNames {
    pub fn add(&mut self, base: String) -> String {
        let mut n = base.clone();
        if self.seen.contains(&n.to_lowercase()) {
            let k = self.next.entry(base.to_lowercase()).or_insert(2);
            loop {
                n = format!("{base}{k}");
                *k += 1;
                if !self.seen.contains(&n.to_lowercase()) {
                    break;
                }
            }
        }
        self.seen.insert(n.to_lowercase());
        n
    }
}

pub fn valid_name(name: &str) -> bool {
    let mut it = name.chars();
    let Some(first) = it.next() else { return false };
    if !(first.is_alphabetic() || first == '_' || first == '\\') || name.len() > 255 {
        return false;
    }
    if !it.all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '\\') {
        return false;
    }
    let up = name.to_ascii_uppercase();
    if up == "R" || up == "C" || CellRef::parse(name).is_some() || up == "TRUE" || up == "FALSE" {
        return false;
    }
    // R1C1-looking names are references too.
    let rc = up.strip_prefix('R').map(|r| r.trim_start_matches(|c: char| c.is_ascii_digit())).and_then(|r| r.strip_prefix('C')).is_some_and(|c| c.chars().all(|x| x.is_ascii_digit()));
    !rc
}

// ---------------------------------------------------------------------------
// Styles

/// What a table style paints on a cell (direct cell formatting wins).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellLook {
    pub fill: Option<u32>,
    pub color: Option<u32>,
    pub bold: bool,
    pub top: Option<Line>,
    pub bottom: Option<Line>,
    pub left: Option<Line>,
    pub right: Option<Line>,
}

/// Office theme colors: dark 1 (text) and the six accents.
pub const ACCENTS: [u32; 7] = [0x000000, 0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47];

/// Mixes `c` toward white (`t` > 0) or black (`t` < 0).
pub fn tint(c: u32, t: f32) -> u32 {
    let ch = |v: u32| {
        let v = v as f32;
        let x = if t >= 0.0 { v + (255.0 - v) * t } else { v * (1.0 + t) };
        x.round().clamp(0.0, 255.0) as u32
    };
    (ch((c >> 16) & 0xFF) << 16) | (ch((c >> 8) & 0xFF) << 8) | ch(c & 0xFF)
}

/// Built-in style families: (prefix, count).
pub const FAMILIES: [(&str, u32); 3] = [("TableStyleLight", 21), ("TableStyleMedium", 28), ("TableStyleDark", 11)];

/// Family (0 light, 1 medium, 2 dark) and number of a built-in style name.
pub fn parse_style_name(name: &str) -> Option<(usize, u32)> {
    for (i, (p, n)) in FAMILIES.iter().enumerate() {
        if let Some(k) = name.strip_prefix(p).and_then(|k| k.parse::<u32>().ok()) {
            if (1..=*n).contains(&k) {
                return Some((i, k));
            }
        }
    }
    None
}

/// Where a cell sits in its table, for styling.
#[derive(Clone, Copy)]
struct Pos {
    header: bool,
    totals: bool,
    /// Band index of the row (0 = first body row) and of the column.
    band_row: u32,
    band_col: u32,
    first_col: bool,
    last_col: bool,
    top: bool,
    bottom: bool,
}

fn style_look(t: &Table, c: CellRef) -> Option<CellLook> {
    let (fam, k) = parse_style_name(&t.style.name)?;
    let (d0, _) = t.data_rows();
    let p = Pos {
        header: t.header_row() == Some(c.row),
        totals: t.totals_row() == Some(c.row),
        band_row: c.row.saturating_sub(d0),
        band_col: c.col - t.range.start.col,
        first_col: c.col == t.range.start.col,
        last_col: c.col == t.range.end.col,
        top: c.row == t.range.start.row,
        bottom: c.row == t.range.end.row,
    };
    let s = &t.style;
    let odd_row = s.row_stripes && !p.header && !p.totals && p.band_row % 2 == 0;
    let odd_col = s.col_stripes && !p.header && !p.totals && p.band_col % 2 == 0;
    let emph_col = (s.first_col && p.first_col) || (s.last_col && p.last_col);
    // Seven variants per group: dark 1 then the six accents.
    let group = (k - 1) / 7;
    let a = ACCENTS[((k - 1) % 7) as usize];
    let is_black = (k - 1) % 7 == 0;
    let line = |c: u32| Some(Line::new(LineStyle::Thin, Some(c)));
    let mut l = CellLook::default();
    match (fam, group) {
        // Light 1-7: banded tint, lines under the header and above totals.
        (0, 0) => {
            let band = if is_black { 0xD9D9D9 } else { tint(a, 0.8) };
            if odd_row || odd_col {
                l.fill = Some(band);
            }
            if !is_black {
                l.color = Some(tint(a, -0.25));
            }
            if p.top {
                l.top = line(a);
            }
            if p.header {
                l.bold = true;
                l.bottom = line(a);
            }
            if p.bottom {
                l.bottom = line(a);
            }
            if p.totals {
                l.bold = true;
                l.top = Some(Line::new(LineStyle::Double, Some(a)));
                l.color = None;
            }
            l.bold |= emph_col;
        }
        // Light 8-14: accent header with white text, outline and row lines.
        (0, 1) => {
            if p.header {
                l.fill = Some(a);
                l.color = Some(0xFFFFFF);
                l.bold = true;
            }
            l.top = line(a);
            l.bottom = line(a);
            if p.first_col {
                l.left = line(a);
            }
            if p.last_col {
                l.right = line(a);
            }
            if odd_col {
                l.left = line(a);
                l.right = line(a);
            }
            if p.totals {
                l.bold = true;
                l.top = Some(Line::new(LineStyle::Double, Some(a)));
            }
            l.bold |= emph_col;
        }
        // Light 15-21: a full grid in the accent color.
        (0, _) => {
            l.top = line(a);
            l.bottom = line(a);
            l.left = line(a);
            l.right = line(a);
            if odd_row || odd_col {
                l.fill = Some(if is_black { 0xD9D9D9 } else { tint(a, 0.8) });
            }
            if p.header {
                l.bold = true;
                l.bottom = Some(Line::new(LineStyle::Medium, Some(a)));
            }
            if p.totals {
                l.bold = true;
                l.top = Some(Line::new(LineStyle::Double, Some(a)));
            }
            l.bold |= emph_col;
        }
        // Medium 1-7: accent header, light bands, inner lines.
        (1, 0) => {
            let band = if is_black { 0xD9D9D9 } else { tint(a, 0.8) };
            let rule = if is_black { 0x808080 } else { tint(a, 0.4) };
            if p.header {
                l.fill = Some(a);
                l.color = Some(0xFFFFFF);
                l.bold = true;
            } else if odd_row || odd_col {
                l.fill = Some(band);
            }
            if !p.header {
                l.top = line(rule);
                l.bottom = line(rule);
            }
            if p.first_col {
                l.left = line(rule);
            }
            if p.last_col {
                l.right = line(rule);
            }
            if p.totals {
                l.bold = true;
                l.top = Some(Line::new(LineStyle::Double, Some(a)));
            }
            l.bold |= emph_col;
        }
        // Medium 8-14: tinted body with white lines.
        (1, 1) => {
            let white = line(0xFFFFFF);
            l.top = white;
            l.bottom = white;
            l.left = white;
            l.right = white;
            if p.header || p.totals {
                l.fill = Some(a);
                l.color = Some(0xFFFFFF);
                l.bold = true;
            } else if emph_col {
                l.fill = Some(a);
                l.color = Some(0xFFFFFF);
                l.bold = true;
            } else {
                l.fill = Some(if odd_row || odd_col { tint(if is_black { 0x7F7F7F } else { a }, 0.6) } else { tint(if is_black { 0x7F7F7F } else { a }, 0.8) });
            }
        }
        // Medium 15-21: black header, bands, thin rules.
        (1, 2) => {
            if p.header {
                l.fill = Some(0x000000);
                l.color = Some(0xFFFFFF);
                l.bold = true;
            } else if odd_row || odd_col {
                l.fill = Some(if is_black { 0xD9D9D9 } else { tint(a, 0.8) });
            }
            l.top = line(if is_black { 0x000000 } else { a });
            l.bottom = line(if is_black { 0x000000 } else { a });
            if p.totals {
                l.bold = true;
                l.top = Some(Line::new(LineStyle::Double, Some(0x000000)));
            }
            l.bold |= emph_col;
        }
        // Medium 22-28: tinted body, grid lines, bold header.
        (1, _) => {
            let rule = if is_black { 0x808080 } else { tint(a, 0.4) };
            l.top = line(rule);
            l.bottom = line(rule);
            l.left = line(rule);
            l.right = line(rule);
            l.fill = Some(if is_black { if odd_row || odd_col { 0xBFBFBF } else { 0xD9D9D9 } } else if odd_row || odd_col { tint(a, 0.6) } else { tint(a, 0.8) });
            if p.header || p.totals {
                l.bold = true;
                l.fill = Some(if is_black { 0xD9D9D9 } else { tint(a, 0.8) });
            }
            if p.totals {
                l.top = Some(Line::new(LineStyle::Double, Some(if is_black { 0x000000 } else { a })));
            }
            l.bold |= emph_col;
        }
        // Dark 1-7: dark body with white text.
        (2, 0) => {
            let base = if is_black { 0x404040 } else { tint(a, -0.25) };
            l.color = Some(0xFFFFFF);
            if p.header {
                l.fill = Some(0x000000);
                l.bold = true;
                l.bottom = Some(Line::new(LineStyle::Medium, Some(0xFFFFFF)));
            } else if p.totals {
                l.fill = Some(tint(base, -0.4));
                l.bold = true;
                l.top = Some(Line::new(LineStyle::Double, Some(0xFFFFFF)));
            } else if emph_col {
                l.fill = Some(tint(base, -0.4));
                l.bold = true;
            } else {
                l.fill = Some(if odd_row || odd_col { tint(base, -0.2) } else { base });
            }
        }
        // Dark 8-11: black header, light body.
        _ => {
            let pair = [0x000000, 0x4472C4, 0xA5A5A5, 0x5B9BD5][((k - 8) % 4) as usize];
            if p.header {
                l.fill = Some(if k == 8 { 0x000000 } else { pair });
                l.color = Some(0xFFFFFF);
                l.bold = true;
            } else if odd_row || odd_col {
                l.fill = Some(if k == 8 { 0xA6A6A6 } else { tint(pair, 0.6) });
            } else {
                l.fill = Some(if k == 8 { 0xD9D9D9 } else { tint(pair, 0.8) });
            }
            if p.totals {
                l.bold = true;
                l.top = Some(Line::new(LineStyle::Double, Some(0x000000)));
            }
            l.bold |= emph_col;
        }
    }
    Some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Table {
        let r = Range::parse("B2:D6").unwrap();
        let mut t = Table::new(1, "Sales", r, vec!["Region".into(), "Q1".into(), "Q 2".into()]);
        t.totals = true;
        t
    }

    #[test]
    fn parts() {
        let t = t();
        assert_eq!(t.data_range().to_a1(), "B3:D5");
        assert_eq!(t.filter_range().to_a1(), "B2:D5");
        let at = CellRef::parse("E4").unwrap();
        let r = |s: &str| t.resolve(&parse_struct(Some("Sales"), s).unwrap(), at).map(|r| r.to_a1());
        assert_eq!(r("Q1"), Ok("C3:C5".into()));
        assert_eq!(r("[#All]"), Ok("B2:D6".into()));
        assert_eq!(r("#Headers"), Ok("B2:D2".into()));
        assert_eq!(r("[#Totals],[Q 2]"), Ok("D6".into()));
        assert_eq!(r("[#Headers],[#Data],[Q1]"), Ok("C2:C5".into()));
        assert_eq!(r("@Q1"), Ok("C4".into()));
        assert_eq!(r("@[Q 2]"), Ok("D4".into()));
        assert_eq!(r("[#This Row],[Region]:[Q1]"), Ok("B4:C4".into()));
        assert_eq!(r("[Region]:[Q 2]"), Ok("B3:D5".into()));
        assert_eq!(r(""), Ok("B3:D5".into()));
        assert_eq!(r("Nope"), Err(()));
        assert_eq!(t.resolve(&parse_struct(None, "@Q1").unwrap(), CellRef::parse("E9").unwrap()), Err(()));
    }

    #[test]
    fn text_forms() {
        let w = |s: &str, xlsx: bool| {
            let mut o = String::new();
            parse_struct(Some("T"), s).unwrap().write(&mut o, xlsx, None);
            o
        };
        assert_eq!(w("Q1", false), "T[Q1]");
        assert_eq!(w("[Q 2]", false), "T[Q 2]");
        assert_eq!(w("@Q1", false), "T[@Q1]");
        assert_eq!(w("@Q1", true), "T[[#This Row],[Q1]]");
        assert_eq!(w("[#Totals],[Q1]", false), "T[[#Totals],[Q1]]");
        assert_eq!(w("#All", false), "T[#All]");
        assert_eq!(w("[A]:[B]", false), "T[[A]:[B]]");
        assert_eq!(w("[Price '[€']]", false), "T[[Price '[€']]]");
        assert_eq!(w("", false), "T[]");
        assert_eq!(w("[#Esta fila],[Q1]", false), "T[@Q1]");
        let mut o = String::new();
        parse_struct(Some("T"), "@Q1").unwrap().write(&mut o, false, Some("t"));
        assert_eq!(o, "[@Q1]");
    }

    #[test]
    fn shifting() {
        let mut t = t();
        assert!(t.shift(false, 3, 1));
        assert_eq!(t.range.to_a1(), "B2:E6");
        assert_eq!(t.columns[2].name, "Column1");
        assert!(t.shift(false, 2, -2));
        assert_eq!(t.range.to_a1(), "B2:C6");
        assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Region", "Q 2"]);
        assert!(t.shift(true, 5, -1));
        assert!(!t.totals);
        assert!(!t.shift(false, 0, -10));
    }

    #[test]
    fn names_and_looks() {
        assert!(valid_name("Table1") && valid_name("_x.y") && !valid_name("A1") && !valid_name("R1C1") && !valid_name("1a") && !valid_name("a b") && !valid_name("C"));
        let t = t();
        let h = t.look(CellRef::parse("B2").unwrap()).unwrap();
        assert!(h.bold && h.fill == Some(0x4472C4) && h.color == Some(0xFFFFFF));
        let band = t.look(CellRef::parse("C3").unwrap()).unwrap();
        assert!(band.fill.is_some());
        assert!(t.look(CellRef::parse("C4").unwrap()).unwrap().fill.is_none());
        assert!(t.look(CellRef::parse("A1").unwrap()).is_none());
        for (p, n) in FAMILIES {
            for k in 1..=n {
                let mut t = t.clone();
                t.style.name = format!("{p}{k}");
                assert!(t.look(CellRef::parse("C6").unwrap()).is_some());
            }
        }
    }
}
