//! Tables. A table is a run of consecutive paragraphs tagged with `InCell`,
//! row by row and cell by cell. The grid lives in the tags, so positions,
//! editing and undo (which snapshots paragraph ranges) keep working on plain
//! paragraphs; table commands rebuild a `Table` from the tags, change it and
//! write the paragraphs back as one undo step.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::{Align, Document, EditKind, LINE_BREAK, Paragraph, Pos, Selection};

mod tools;

/// Default cell padding left and right, in points (like other editors).
pub const CELL_PAD_X: f32 = 5.4;
pub const MIN_COL_WIDTH: f32 = 12.0;
/// Word's limit; more columns are folded into the last one.
pub const MAX_COLS: usize = 63;
/// Rows a table can have (more are split into another table).
pub const MAX_ROWS: usize = 32767;
/// Tables nested deeper than this are read as the paragraphs of their
/// cells (files with thousands of levels would take gigabytes).
pub const MAX_NESTING: usize = 32;

/// A border line; a zero width means no line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    /// Width in points.
    pub width: f32,
    pub color: u32,
}

impl Border {
    pub const NONE: Border = Border { width: 0.0, color: 0 };
    pub const THIN: Border = Border { width: 0.5, color: 0 };

    pub const fn new(width: f32, color: u32) -> Border {
        Border { width, color }
    }

    pub fn is_none(&self) -> bool {
        self.width <= 0.0
    }
}

/// Border settings; `None` inherits (cell from table, table from style).
/// Cells ignore the inside lines.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Borders {
    pub top: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    pub right: Option<Border>,
    pub inside_h: Option<Border>,
    pub inside_v: Option<Border>,
}

impl Borders {
    pub const fn all(b: Border) -> Borders {
        Borders { top: Some(b), bottom: Some(b), left: Some(b), right: Some(b), inside_h: Some(b), inside_v: Some(b) }
    }

    pub fn is_empty(&self) -> bool {
        *self == Borders::default()
    }

    /// Sets the sides that `o` sets.
    pub fn overlay(&mut self, o: &Borders) {
        for (a, b) in [(&mut self.top, o.top), (&mut self.bottom, o.bottom), (&mut self.left, o.left), (&mut self.right, o.right), (&mut self.inside_h, o.inside_h), (&mut self.inside_v, o.inside_v)] {
            if b.is_some() {
                *a = b;
            }
        }
    }
}

/// Which way the text of a cell runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextDir {
    #[default]
    Horizontal,
    /// Turned left: read from the bottom up (Word's btLr).
    Up,
    /// Turned right: read from the top down (Word's tbRl).
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

/// Properties of one cell, kept on its first paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct CellProps {
    /// Columns and rows a merged cell spans (1 = not merged).
    pub col_span: u16,
    pub row_span: u32,
    /// Covered by a merged cell: its paragraphs are kept but not shown.
    pub covered: bool,
    pub shading: Option<u32>,
    pub borders: Borders,
    pub valign: VAlign,
    pub text_dir: TextDir,
    /// The file's own direction when it says more than `text_dir` (Asian
    /// vertical text); written back while it agrees with `text_dir`.
    pub flow: Option<crate::TextFlow>,
}

impl Default for CellProps {
    fn default() -> Self {
        CellProps { col_span: 1, row_span: 1, covered: false, shading: None, borders: Borders::default(), valign: VAlign::Top, text_dir: TextDir::Horizontal, flow: None }
    }
}

/// Which parts of a table style apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableLook {
    pub first_row: bool,
    pub last_row: bool,
    pub first_col: bool,
    pub last_col: bool,
    pub banded_rows: bool,
    pub banded_cols: bool,
}

impl Default for TableLook {
    fn default() -> Self {
        TableLook { first_row: true, last_row: false, first_col: true, last_col: false, banded_rows: true, banded_cols: false }
    }
}

/// Properties of a table, kept on its first paragraph.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TableProps {
    /// Column widths in points.
    pub widths: Vec<f32>,
    /// Built-in style id (see `TABLE_STYLES`); `None` is a plain table.
    pub style: Option<Arc<str>>,
    pub look: TableLook,
    /// Rows repeated at the top of each page.
    pub header_rows: u32,
    /// Direct borders, over the style's.
    pub borders: Borders,
    pub align: Align,
    /// Left indent in points.
    pub indent: f32,
    /// Minimum row heights in points (0 or missing = automatic).
    pub row_heights: Vec<f32>,
    /// How rows break across pages and take their height (missing: the
    /// default rule).
    pub row_rules: Vec<RowRule>,
    /// Default cell margins in points: top, left, bottom, right (`None`:
    /// 0, `CELL_PAD_X`, 0, `CELL_PAD_X`).
    pub cell_margins: Option<[f32; 4]>,
}

/// A row that stays on one page, and one whose height is exactly its
/// `row_heights` entry (the text beyond it is cut).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RowRule {
    pub cant_split: bool,
    pub exact: bool,
}

impl TableProps {
    /// The rule of row `r`.
    pub fn row_rule(&self, r: usize) -> RowRule {
        self.row_rules.get(r).copied().unwrap_or_default()
    }

    pub fn width(&self) -> f32 {
        self.widths.iter().sum()
    }

    /// Cell margins in points: top, left, bottom, right.
    pub fn margins(&self) -> [f32; 4] {
        self.cell_margins.map_or([0.0, CELL_PAD_X, 0.0, CELL_PAD_X], |m| m.map(|v| if v.is_finite() { v.clamp(0.0, 200.0) } else { 0.0 }))
    }

    /// The table's style: one of the document's (`styles`) or a built-in one.
    pub fn style_def<'a>(&self, styles: &'a crate::Styles) -> Option<&'a TableStyle> {
        let id = self.style.as_deref()?;
        styles.tables.iter().find(|s| s.id.eq_ignore_ascii_case(id)).or_else(|| table_style(id))
    }

    /// Resolved look of the cell at (r, c) of a `rows` x `cols` table.
    pub fn cell_look(&self, styles: &crate::Styles, cell: &CellProps, r: usize, c: usize, rows: usize, cols: usize) -> CellLook {
        self.cell_look_in(self.style_def(styles), cell, r, c, rows, cols)
    }

    /// `cell_look` with the table in style `st`.
    pub fn cell_look_in(&self, st: Option<&TableStyle>, cell: &CellProps, r: usize, c: usize, rows: usize, cols: usize) -> CellLook {
        let (rs, cs) = (cell.row_span.max(1) as usize, cell.col_span.max(1) as usize);
        let mut b = st.map_or(Borders::default(), |s| s.borders);
        b.overlay(&self.borders);
        let pick = |outer: bool, o: Option<Border>, i: Option<Border>| if outer { o } else { i };
        let mut top = pick(r == 0, b.top, b.inside_h);
        let mut bottom = pick(r + rs >= rows, b.bottom, b.inside_h);
        let left = pick(c == 0, b.left, b.inside_v);
        let right = pick(c + cs >= cols, b.right, b.inside_v);
        let mut look = CellLook::default();
        if let Some(s) = st {
            let header = self.look.first_row && r == 0;
            if header {
                look.fill = s.header_fill;
                look.color = s.header_color;
                look.bold = s.header_bold;
                if let Some(l) = s.header_line.filter(|_| r + rs < rows) {
                    bottom = Some(l);
                }
            } else if self.look.banded_rows {
                let first = usize::from(self.look.first_row);
                if r >= first && (r - first) % 2 == 0 {
                    look.fill = s.band_fill;
                }
            }
            if r > 0 && self.look.first_row && r == 1 {
                if let Some(l) = s.header_line {
                    top = Some(l);
                }
            }
            if (self.look.first_col && c == 0 || self.look.last_row && r + rs >= rows && rows > 1) && s.first_col_bold {
                look.bold = true;
            }
        }
        let bd = &cell.borders;
        look.top = bd.top.or(top).unwrap_or(Border::NONE);
        look.bottom = bd.bottom.or(bottom).unwrap_or(Border::NONE);
        look.left = bd.left.or(left).unwrap_or(Border::NONE);
        look.right = bd.right.or(right).unwrap_or(Border::NONE);
        if cell.shading.is_some() {
            look.fill = cell.shading;
        }
        look
    }
}

/// How a cell is drawn: fill, text emphasis and its four borders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellLook {
    pub fill: Option<u32>,
    pub bold: bool,
    pub color: Option<u32>,
    pub top: Border,
    pub bottom: Border,
    pub left: Border,
    pub right: Border,
}

impl Default for CellLook {
    fn default() -> Self {
        CellLook { fill: None, bold: false, color: None, top: Border::NONE, bottom: Border::NONE, left: Border::NONE, right: Border::NONE }
    }
}

/// A table style: built-in ones are named like Word's so files keep them;
/// the document's own are in `Styles::tables`.
#[derive(Clone, Debug, PartialEq)]
pub struct TableStyle {
    pub id: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub borders: Borders,
    pub header_fill: Option<u32>,
    pub header_color: Option<u32>,
    pub header_bold: bool,
    /// Line under the header row.
    pub header_line: Option<Border>,
    pub band_fill: Option<u32>,
    /// First column (and last row) in bold.
    pub first_col_bold: bool,
}

const fn grid(width: f32, color: u32) -> Borders {
    Borders::all(Border::new(width, color))
}

const fn lines(outer: Option<Border>, inside_h: Option<Border>, inside_v: Option<Border>) -> Borders {
    Borders { top: outer, bottom: outer, left: outer, right: outer, inside_h, inside_v }
}

pub const PLAIN: TableStyle = TableStyle { id: Cow::Borrowed(""), name: Cow::Borrowed(""), borders: lines(None, None, None), header_fill: None, header_color: None, header_bold: false, header_line: None, band_fill: None, first_col_bold: false };

pub const TABLE_STYLES: [TableStyle; 22] = [
    TableStyle { id: Cow::Borrowed("TableGrid"), name: Cow::Borrowed("Table Grid"), borders: grid(0.5, 0x000000), ..PLAIN },
    TableStyle { id: Cow::Borrowed("PlainTable1"), name: Cow::Borrowed("Plain Table 1"), borders: grid(0.5, 0xBFBFBF), header_bold: true, band_fill: Some(0xF2F2F2), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable1Light"), name: Cow::Borrowed("Grid Table 1 Light"), borders: grid(0.5, 0x999999), header_bold: true, header_line: Some(Border::new(1.5, 0x666666)), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable4-Accent1"), name: Cow::Borrowed("Grid Table 4 - Accent 1"), borders: grid(0.5, 0x8EAADB), header_fill: Some(0x4472C4), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xD9E2F3), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable4-Accent2"), name: Cow::Borrowed("Grid Table 4 - Accent 2"), borders: grid(0.5, 0xF4B083), header_fill: Some(0xED7D31), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xFBE4D5), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable4-Accent6"), name: Cow::Borrowed("Grid Table 4 - Accent 6"), borders: grid(0.5, 0xA8D08D), header_fill: Some(0x70AD47), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xE2EFD9), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("ListTable3-Accent1"), name: Cow::Borrowed("List Table 3 - Accent 1"), borders: lines(Some(Border::new(0.5, 0x4472C4)), None, None), header_fill: Some(0x4472C4), header_color: Some(0xFFFFFF), header_bold: true, first_col_bold: true, ..PLAIN },
    TableStyle {
        id: Cow::Borrowed("ListTable4-Accent1"),
        name: Cow::Borrowed("List Table 4 - Accent 1"),
        borders: Borders { top: Some(Border::new(0.5, 0x8EAADB)), bottom: Some(Border::new(0.5, 0x8EAADB)), left: Some(Border::new(0.5, 0x8EAADB)), right: Some(Border::new(0.5, 0x8EAADB)), inside_h: Some(Border::new(0.5, 0x8EAADB)), inside_v: None },
        header_fill: Some(0x4472C4),
        header_color: Some(0xFFFFFF),
        header_bold: true,
        band_fill: Some(0xD9E2F3),
        first_col_bold: true,
        ..PLAIN
    },
    TableStyle { id: Cow::Borrowed("TableGridLight"), name: Cow::Borrowed("Grid Table Light"), borders: grid(0.5, 0xBFBFBF), ..PLAIN },
    TableStyle { id: Cow::Borrowed("PlainTable2"), name: Cow::Borrowed("Plain Table 2"), borders: lines(Some(Border::new(0.5, 0x7F7F7F)), Some(Border::new(0.5, 0x7F7F7F)), None), header_bold: true, header_line: Some(Border::new(0.5, 0x7F7F7F)), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("PlainTable3"), name: Cow::Borrowed("Plain Table 3"), header_bold: true, header_line: Some(Border::new(0.5, 0x7F7F7F)), band_fill: Some(0xF2F2F2), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("PlainTable4"), name: Cow::Borrowed("Plain Table 4"), header_bold: true, band_fill: Some(0xF2F2F2), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("PlainTable5"), name: Cow::Borrowed("Plain Table 5"), header_line: Some(Border::new(0.5, 0x7F7F7F)), band_fill: Some(0xF2F2F2), ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable2-Accent1"), name: Cow::Borrowed("Grid Table 2 - Accent 1"), borders: Borders { top: Some(Border::new(0.5, 0x8EAADB)), bottom: Some(Border::new(0.5, 0x8EAADB)), left: None, right: None, inside_h: Some(Border::new(0.5, 0x8EAADB)), inside_v: Some(Border::new(0.5, 0x8EAADB)) }, header_bold: true, header_line: Some(Border::new(1.5, 0x8EAADB)), band_fill: Some(0xD9E2F3), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable4-Accent3"), name: Cow::Borrowed("Grid Table 4 - Accent 3"), borders: grid(0.5, 0xC9C9C9), header_fill: Some(0xA5A5A5), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xEDEDED), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable4-Accent4"), name: Cow::Borrowed("Grid Table 4 - Accent 4"), borders: grid(0.5, 0xFFD966), header_fill: Some(0xFFC000), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xFFF2CC), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable4-Accent5"), name: Cow::Borrowed("Grid Table 4 - Accent 5"), borders: grid(0.5, 0x9CC2E5), header_fill: Some(0x5B9BD5), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xDEEAF6), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable5Dark-Accent1"), name: Cow::Borrowed("Grid Table 5 Dark - Accent 1"), borders: grid(0.5, 0xFFFFFF), header_fill: Some(0x4472C4), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xB4C6E7), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("GridTable6Colorful-Accent1"), name: Cow::Borrowed("Grid Table 6 Colorful - Accent 1"), borders: grid(0.5, 0x8EAADB), header_bold: true, header_line: Some(Border::new(1.5, 0x8EAADB)), band_fill: Some(0xD9E2F3), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("ListTable1Light-Accent1"), name: Cow::Borrowed("List Table 1 Light - Accent 1"), header_bold: true, header_line: Some(Border::new(0.5, 0x8EAADB)), band_fill: Some(0xD9E2F3), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("ListTable2-Accent1"), name: Cow::Borrowed("List Table 2 - Accent 1"), borders: lines(Some(Border::new(0.5, 0x8EAADB)), Some(Border::new(0.5, 0x8EAADB)), None), header_bold: true, band_fill: Some(0xD9E2F3), first_col_bold: true, ..PLAIN },
    TableStyle { id: Cow::Borrowed("ListTable4-Accent2"), name: Cow::Borrowed("List Table 4 - Accent 2"), borders: Borders { top: Some(Border::new(0.5, 0xF4B083)), bottom: Some(Border::new(0.5, 0xF4B083)), left: Some(Border::new(0.5, 0xF4B083)), right: Some(Border::new(0.5, 0xF4B083)), inside_h: Some(Border::new(0.5, 0xF4B083)), inside_v: None }, header_fill: Some(0xED7D31), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xFBE4D5), first_col_bold: true, ..PLAIN },
];

/// A built-in style by id (case-insensitive).
pub fn table_style(id: &str) -> Option<&'static TableStyle> {
    TABLE_STYLES.iter().find(|s| s.id.eq_ignore_ascii_case(id))
}

/// The tag of a paragraph that belongs to a table cell. A paragraph of a
/// table nested in a cell has the outer cell's tag with the inner one in
/// `nested` (and so on for deeper tables). Table ids are unique in the
/// document at every level.
#[derive(Clone, Debug, PartialEq)]
pub struct InCell {
    /// Unique per table in the document.
    pub table: u32,
    pub row: u32,
    pub col: u16,
    /// Only on the first paragraph of each cell.
    pub cell: Option<Box<CellProps>>,
    /// Only on the first paragraph of the table.
    pub table_props: Option<Box<TableProps>>,
    /// The cell of a table inside this cell holding the paragraph.
    pub nested: Option<Box<InCell>>,
}

impl InCell {
    pub fn new(table: u32, row: u32, col: u16) -> InCell {
        InCell { table, row, col, cell: None, table_props: None, nested: None }
    }

    /// Table, row and column: equal for paragraphs of the same cell.
    pub fn key(&self) -> (u32, u32, u16) {
        (self.table, self.row, self.col)
    }

    /// The same cells without the properties (for more paragraphs in them).
    pub fn bare(&self) -> InCell {
        InCell { nested: self.nested.as_deref().map(|n| Box::new(n.bare())), ..InCell::new(self.table, self.row, self.col) }
    }

    /// The tag `d` levels in (0 is this one).
    pub fn at(&self, d: usize) -> Option<&InCell> {
        let mut t = self;
        for _ in 0..d {
            t = t.nested.as_deref()?;
        }
        Some(t)
    }

    pub fn at_mut(&mut self, d: usize) -> Option<&mut InCell> {
        let mut t = self;
        for _ in 0..d {
            t = t.nested.as_deref_mut()?;
        }
        Some(t)
    }

    /// Levels of tables (1 for a cell of a table that isn't nested).
    pub fn depth(&self) -> usize {
        1 + self.nested.as_deref().map_or(0, InCell::depth)
    }

    /// The tag of the cell directly holding the paragraph.
    pub fn innermost(&self) -> &InCell {
        self.nested.as_deref().map_or(self, InCell::innermost)
    }

    /// Whether the paragraph is in table `id` at some level.
    pub fn in_table(&self, id: u32) -> bool {
        self.table == id || self.nested.as_deref().is_some_and(|n| n.in_table(id))
    }

    /// The first `levels` levels (the cells around a nested table).
    fn outer(&self, levels: usize) -> Option<InCell> {
        let mut t = self.clone();
        match levels {
            0 => None,
            _ => {
                t.at_mut(levels - 1)?.nested = None;
                Some(t)
            }
        }
    }

    /// Puts `inner` (a paragraph's tag in a table inside these cells) at
    /// the innermost level.
    fn wrap(mut self, inner: Option<InCell>) -> InCell {
        let d = self.depth() - 1;
        if let Some(t) = self.at_mut(d) {
            t.nested = inner.map(Box::new);
        }
        self
    }

    /// Every table id at every level.
    fn ids(&self, out: &mut Vec<u32>) {
        out.push(self.table);
        if let Some(n) = &self.nested {
            n.ids(out);
        }
    }

    fn map_ids(&mut self, f: &mut dyn FnMut(u32) -> u32) {
        self.table = f(self.table);
        if let Some(n) = &mut self.nested {
            n.map_ids(f);
        }
    }
}

/// The tag of a paragraph at nesting level `d`.
fn tag_at(p: &Paragraph, d: usize) -> Option<&InCell> {
    p.cell.as_ref()?.at(d)
}

/// Strips the outer `levels` levels of tags, for editing a nested table
/// as a table of its own.
fn promote(mut p: Paragraph, levels: usize) -> Paragraph {
    for _ in 0..levels {
        p.cell = p.cell.and_then(|c| c.nested).map(|b| *b);
    }
    p
}

/// One cell with its paragraphs (untagged).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub props: CellProps,
    pub paras: Vec<Paragraph>,
}

impl Cell {
    pub fn empty() -> Cell {
        Cell { props: CellProps::default(), paras: vec![Paragraph::new("")] }
    }

    /// A new empty cell formatted like `c` (shading, borders, first
    /// paragraph style and character formatting).
    fn like(c: &Cell) -> Cell {
        let mut p = Paragraph::new("");
        if let Some(f) = c.paras.first() {
            p.style = f.style;
            p.style.page_break_before = false;
            p.runs[0].attrs = f.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
            p.runs[0].attrs.link = None;
            p.runs[0].attrs.link_tip = None;
        }
        Cell { props: CellProps { shading: c.props.shading, borders: c.props.borders, valign: c.props.valign, text_dir: c.props.text_dir, ..CellProps::default() }, paras: vec![p] }
    }

    pub fn is_blank(&self) -> bool {
        self.paras.iter().all(|p| p.text.is_empty())
    }

    /// The cell's text, paragraphs joined by `sep`.
    pub fn text(&self, sep: &str) -> String {
        let mut s = String::new();
        for (i, p) in self.paras.iter().enumerate() {
            if i > 0 {
                s.push_str(sep);
            }
            s.push_str(&p.text.replace(LINE_BREAK, " "));
        }
        s
    }
}

/// Where a table taken out for editing was: its paragraphs `a..e`, id and
/// the cells around it when nested (with the properties of paragraph `a`).
struct Taken {
    a: usize,
    e: usize,
    id: u32,
    outer: Option<InCell>,
}

/// A table taken out of the paragraphs for editing.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub props: TableProps,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    /// An empty `rows` x `cols` table `width` points wide.
    pub fn new(rows: usize, cols: usize, width: f32) -> Table {
        let (rows, cols) = (rows.clamp(1, MAX_ROWS), cols.clamp(1, MAX_COLS));
        let w = (width / cols as f32).max(MIN_COL_WIDTH);
        let props = TableProps { widths: vec![w; cols], style: Some("TableGrid".into()), ..Default::default() };
        Table { props, rows: (0..rows).map(|_| (0..cols).map(|_| Cell::empty()).collect()).collect() }
    }

    pub fn nrows(&self) -> usize {
        self.rows.len()
    }

    pub fn ncols(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }

    /// Rebuilds a table from tagged paragraphs, repairing whatever is
    /// missing (cells, widths, spans).
    pub fn from_paras(ps: Vec<Paragraph>) -> Table {
        let mut props: Option<TableProps> = None;
        let mut cells: BTreeMap<(u32, u16), Cell> = BTreeMap::new();
        for mut p in ps {
            let Some(mut ic) = p.cell.take() else { continue };
            // Tags of tables in the cell stay on its paragraphs.
            p.cell = ic.nested.take().map(|b| *b);
            if props.is_none() {
                props = ic.table_props.map(|b| *b);
            }
            let col = ic.col.min(MAX_COLS as u16 - 1);
            let e = cells.entry((ic.row, col)).or_insert_with(|| Cell { props: CellProps::default(), paras: Vec::new() });
            if let Some(cp) = ic.cell {
                if e.paras.is_empty() {
                    e.props = *cp;
                }
            }
            e.paras.push(p);
        }
        let mut props = props.unwrap_or_default();
        // Rows are renumbered in order, so gaps can't make the grid huge.
        let mut row_ids: Vec<u32> = cells.keys().map(|k| k.0).collect();
        row_ids.dedup();
        let cols = cells.keys().map(|k| k.1 as usize + 1).max().unwrap_or(1).max(props.widths.len().min(MAX_COLS)).max(1);
        let mut rows: Vec<Vec<Cell>> = (0..row_ids.len().max(1)).map(|_| (0..cols).map(|_| Cell { props: CellProps::default(), paras: Vec::new() }).collect()).collect();
        let mut ri = 0;
        for ((r, c), cell) in cells {
            while row_ids[ri] != r {
                ri += 1;
            }
            rows[ri][c as usize] = cell;
        }
        props.widths.truncate(cols);
        let mut t = Table { props, rows };
        t.fix();
        t
    }

    /// Tagged paragraphs of the table.
    pub fn into_paras(self, id: u32) -> Vec<Paragraph> {
        let mut out = Vec::new();
        let mut props = Some(Box::new(self.props));
        for (r, row) in self.rows.into_iter().enumerate() {
            for (c, cell) in row.into_iter().enumerate() {
                let n = cell.paras.len();
                let mut cp = Some(Box::new(cell.props));
                for (k, mut p) in cell.paras.into_iter().enumerate() {
                    let mut tag = InCell::new(id, r as u32, c as u16);
                    if k == 0 {
                        tag.cell = cp.take();
                        if r == 0 && c == 0 {
                            tag.table_props = props.take();
                        }
                    }
                    tag.nested = p.cell.take().map(Box::new);
                    p.cell = Some(tag);
                    out.push(p);
                }
                debug_assert!(n > 0);
            }
        }
        out
    }

    /// Offset (from the table's first paragraph) of the first paragraph of
    /// cell (r, c).
    pub fn para_offset(&self, r: usize, c: usize) -> usize {
        let mut n = 0;
        for (ri, row) in self.rows.iter().enumerate() {
            for (ci, cell) in row.iter().enumerate() {
                if (ri, ci) == (r, c) {
                    return n;
                }
                n += cell.paras.len();
            }
        }
        n
    }

    fn span(&self, r: usize, c: usize) -> (usize, usize) {
        let p = &self.rows[r][c].props;
        (p.row_span.max(1) as usize, p.col_span.max(1) as usize)
    }

    /// The merged cell covering (r, c), or (r, c) itself.
    pub fn anchor_of(&self, r: usize, c: usize) -> (usize, usize) {
        if !self.rows[r][c].props.covered {
            return (r, c);
        }
        for ar in (0..=r).rev() {
            for ac in (0..=c).rev() {
                let cell = &self.rows[ar][ac];
                let (rs, cs) = self.span(ar, ac);
                if !cell.props.covered && ar + rs > r && ac + cs > c {
                    return (ar, ac);
                }
            }
        }
        (r, c)
    }

    /// Makes the grid consistent: every row as wide as the widths, every
    /// cell with a paragraph, spans inside the grid and covered flags
    /// matching them.
    pub fn fix(&mut self) {
        if self.rows.is_empty() {
            self.rows.push(vec![Cell::empty()]);
        }
        let nc = self.rows.iter().map(Vec::len).max().unwrap_or(1).clamp(1, MAX_COLS);
        for row in &mut self.rows {
            // Extra cells fold into the last one.
            while row.len() > nc {
                let extra = row.pop().map(|c| c.paras).unwrap_or_default();
                if let Some(last) = row.last_mut() {
                    last.paras.extend(extra.into_iter().filter(|p| !p.text.is_empty()));
                }
            }
            row.resize_with(nc, Cell::empty);
            for cell in row.iter_mut() {
                if cell.paras.is_empty() {
                    cell.paras.push(Paragraph::new(""));
                }
                // A cell can't end with a nested table.
                if cell.paras.last().is_some_and(|p| p.cell.is_some()) {
                    cell.paras.push(Paragraph::new(""));
                }
            }
        }
        let w = &mut self.props.widths;
        let known: Vec<f32> = w.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
        let avg = if known.is_empty() { 72.0 } else { known.iter().sum::<f32>() / known.len() as f32 };
        w.resize(nc, avg);
        for v in w.iter_mut() {
            if !v.is_finite() || *v <= 0.0 {
                *v = avg;
            }
            *v = v.clamp(MIN_COL_WIDTH, 2000.0);
        }
        let nr = self.rows.len();
        self.props.row_heights.truncate(nr);
        self.props.row_rules.truncate(nr);
        while self.props.row_rules.last() == Some(&RowRule::default()) {
            self.props.row_rules.pop();
        }
        for h in &mut self.props.row_heights {
            if !h.is_finite() || *h < 0.0 {
                *h = 0.0;
            }
            *h = h.min(1000.0);
        }
        self.props.header_rows = self.props.header_rows.min(nr as u32);
        if !self.props.indent.is_finite() {
            self.props.indent = 0.0;
        }
        self.fix_spans();
    }

    fn fix_spans(&mut self) {
        let (nr, nc) = (self.nrows(), self.ncols());
        let mut covered = vec![false; nr * nc];
        for r in 0..nr {
            for c in 0..nc {
                let p = &mut self.rows[r][c].props;
                if covered[r * nc + c] {
                    p.covered = true;
                    p.row_span = 1;
                    p.col_span = 1;
                    continue;
                }
                p.covered = false;
                let mut cs = (p.col_span.max(1) as usize).min(nc - c);
                while cs > 1 && (1..cs).any(|k| covered[r * nc + c + k]) {
                    cs -= 1;
                }
                let mut rs = (p.row_span.max(1) as usize).min(nr - r);
                while rs > 1 && (1..rs).any(|k| (c..c + cs).any(|cc| covered[(r + k) * nc + cc])) {
                    rs -= 1;
                }
                p.col_span = cs as u16;
                p.row_span = rs as u32;
                for rr in r..r + rs {
                    for cc in c..c + cs {
                        if (rr, cc) != (r, c) {
                            covered[rr * nc + cc] = true;
                        }
                    }
                }
            }
        }
    }

    /// Grows a cell rectangle until no merged cell crosses its edges.
    pub fn expand(&self, mut r0: usize, mut c0: usize, mut r1: usize, mut c1: usize) -> (usize, usize, usize, usize) {
        loop {
            let before = (r0, c0, r1, c1);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let (ar, ac) = self.anchor_of(r, c);
                    let (rs, cs) = self.span(ar, ac);
                    r0 = r0.min(ar);
                    c0 = c0.min(ac);
                    r1 = r1.max(ar + rs - 1);
                    c1 = c1.max(ac + cs - 1);
                }
            }
            if before == (r0, c0, r1, c1) {
                return before;
            }
        }
    }

    fn anchors(&self) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if !cell.props.covered {
                    v.push((r, c));
                }
            }
        }
        v
    }

    /// Inserts `n` rows at `at`, formatted like row `tmpl`.
    pub fn insert_rows(&mut self, at: usize, n: usize, tmpl: usize) {
        let at = at.min(self.nrows());
        let tmpl = tmpl.min(self.nrows() - 1);
        for (r, c) in self.anchors() {
            let (rs, _) = self.span(r, c);
            if r < at && r + rs > at {
                self.rows[r][c].props.row_span += n as u32;
            }
        }
        let nc = self.ncols();
        let mut new_row = Vec::with_capacity(nc);
        for c in 0..nc {
            let (ar, ac) = self.anchor_of(tmpl, c);
            let mut cell = Cell::like(&self.rows[ar][ac]);
            if ac == c {
                cell.props.col_span = self.span(ar, ac).1 as u16;
            }
            new_row.push(cell);
        }
        for _ in 0..n {
            self.rows.insert(at, new_row.clone());
        }
        if self.props.row_heights.len() > at {
            self.props.row_heights.splice(at..at, std::iter::repeat_n(0.0, n));
        }
        if self.props.row_rules.len() > at {
            self.props.row_rules.splice(at..at, std::iter::repeat_n(RowRule::default(), n));
        }
        if (at as u32) < self.props.header_rows {
            self.props.header_rows += n as u32;
        }
        self.fix();
    }

    /// Inserts `n` columns at `at`, formatted like column `tmpl`.
    pub fn insert_cols(&mut self, at: usize, n: usize, tmpl: usize) {
        let nc = self.ncols();
        let n = n.min(MAX_COLS.saturating_sub(nc));
        if n == 0 {
            return;
        }
        let at = at.min(nc);
        let tmpl = tmpl.min(nc - 1);
        for (r, c) in self.anchors() {
            let (_, cs) = self.span(r, c);
            if c < at && c + cs > at {
                self.rows[r][c].props.col_span += n as u16;
            }
        }
        for r in 0..self.nrows() {
            let (ar, ac) = self.anchor_of(r, tmpl);
            let mut cell = Cell::like(&self.rows[ar][ac]);
            if ar == r {
                cell.props.row_span = self.span(ar, ac).0 as u32;
            }
            for _ in 0..n {
                self.rows[r].insert(at, cell.clone());
            }
        }
        let w = self.props.widths[tmpl];
        self.props.widths.splice(at..at, std::iter::repeat_n(w, n));
        self.fix();
    }

    /// Deletes rows `r0..=r1` (false if that's all of them).
    pub fn delete_rows(&mut self, r0: usize, r1: usize) -> bool {
        let r1 = r1.min(self.nrows() - 1);
        if r0 > r1 || r1 - r0 + 1 >= self.nrows() {
            return false;
        }
        for (r, c) in self.anchors() {
            let (rs, _) = self.span(r, c);
            let last = r + rs - 1;
            if r < r0 && last >= r0 {
                self.rows[r][c].props.row_span -= (last.min(r1) + 1 - r0) as u32;
            } else if (r0..=r1).contains(&r) && last > r1 {
                // The merged cell continues below: its lower part keeps it.
                let mut props = self.rows[r][c].props.clone();
                props.row_span = (last - r1) as u32;
                self.rows[r1 + 1][c].props = props;
            }
        }
        self.rows.drain(r0..=r1);
        let h = &mut self.props.row_heights;
        if h.len() > r0 {
            h.drain(r0..=r1.min(h.len() - 1));
        }
        let rr = &mut self.props.row_rules;
        if rr.len() > r0 {
            rr.drain(r0..=r1.min(rr.len() - 1));
        }
        let hr = self.props.header_rows as usize;
        if hr > r0 {
            self.props.header_rows = (r0 + hr.saturating_sub(r1 + 1)) as u32;
        }
        self.fix();
        true
    }

    /// Deletes columns `c0..=c1` (false if that's all of them).
    pub fn delete_cols(&mut self, c0: usize, c1: usize) -> bool {
        let c1 = c1.min(self.ncols() - 1);
        if c0 > c1 || c1 - c0 + 1 >= self.ncols() {
            return false;
        }
        for (r, c) in self.anchors() {
            let (_, cs) = self.span(r, c);
            let last = c + cs - 1;
            if c < c0 && last >= c0 {
                self.rows[r][c].props.col_span -= (last.min(c1) + 1 - c0) as u16;
            } else if (c0..=c1).contains(&c) && last > c1 {
                let mut props = self.rows[r][c].props.clone();
                props.col_span = (last - c1) as u16;
                self.rows[r][c1 + 1].props = props;
            }
        }
        for row in &mut self.rows {
            row.drain(c0..=c1);
        }
        self.props.widths.drain(c0..=c1);
        self.fix();
        true
    }

    /// Merges a rectangle of cells (grown over merged cells) into its
    /// top-left cell, keeping all their text.
    pub fn merge(&mut self, r0: usize, c0: usize, r1: usize, c1: usize) {
        let (r0, c0, r1, c1) = self.expand(r0, c0, r1.min(self.nrows() - 1), c1.min(self.ncols() - 1));
        let mut paras = Vec::new();
        for r in r0..=r1 {
            for c in c0..=c1 {
                let cell = &mut self.rows[r][c];
                if !cell.is_blank() {
                    paras.append(&mut cell.paras);
                }
                if (r, c) != (r0, c0) {
                    cell.paras = vec![Paragraph::new("")];
                }
            }
        }
        let anchor = &mut self.rows[r0][c0];
        if !paras.is_empty() {
            anchor.paras = paras;
        } else if anchor.paras.is_empty() {
            anchor.paras.push(Paragraph::new(""));
        }
        anchor.props.row_span = (r1 - r0 + 1) as u32;
        anchor.props.col_span = (c1 - c0 + 1) as u16;
        self.fix();
    }

    /// Splits cell (r, c): a merged cell goes back to its single cells;
    /// another one becomes `rows` x `cols` cells.
    pub fn split(&mut self, r: usize, c: usize, rows: usize, cols: usize) {
        let (r, c) = self.anchor_of(r.min(self.nrows() - 1), c.min(self.ncols() - 1));
        let (rs, cs) = self.span(r, c);
        if rs > 1 || cs > 1 {
            let p = &mut self.rows[r][c].props;
            p.row_span = 1;
            p.col_span = 1;
            self.fix();
            return;
        }
        let k = cols.clamp(1, MAX_COLS.saturating_sub(self.ncols()) + 1);
        let m = rows.clamp(1, 100);
        if k > 1 {
            for (ar, ac) in self.anchors() {
                let (ars, acs) = self.span(ar, ac);
                if (ar, ac) != (r, c) && ac <= c && c < ac + acs && !(ar <= r && r < ar + ars) {
                    self.rows[ar][ac].props.col_span += (k - 1) as u16;
                }
            }
            let tmpl = Cell::like(&self.rows[r][c]);
            for rr in 0..self.nrows() {
                for _ in 1..k {
                    let cell = if rr == r { tmpl.clone() } else { Cell::empty() };
                    self.rows[rr].insert(c + 1, cell);
                }
            }
            let w = self.props.widths[c] / k as f32;
            self.props.widths[c] = w;
            self.props.widths.splice(c + 1..c + 1, std::iter::repeat_n(w, k - 1));
        }
        if m > 1 {
            for (ar, ac) in self.anchors() {
                let (ars, _) = self.span(ar, ac);
                let piece = ar == r && (c..c + k).contains(&ac);
                if !piece && ar <= r && r < ar + ars {
                    self.rows[ar][ac].props.row_span += (m - 1) as u32;
                }
            }
            let tmpl = Cell::like(&self.rows[r][c]);
            let row: Vec<Cell> = (0..self.ncols()).map(|cc| if (c..c + k).contains(&cc) { tmpl.clone() } else { Cell::empty() }).collect();
            for _ in 1..m {
                self.rows.insert(r + 1, row.clone());
            }
            if self.props.row_heights.len() > r + 1 {
                self.props.row_heights.splice(r + 1..r + 1, std::iter::repeat_n(0.0, m - 1));
            }
            if self.props.row_rules.len() > r + 1 {
                let rule = self.props.row_rules[r];
                self.props.row_rules.splice(r + 1..r + 1, std::iter::repeat_n(rule, m - 1));
            }
        }
        self.fix();
    }

    /// Sorts the rows below the first `skip` by the text of column `col`:
    /// numbers before text, empty cells last. False when merged cells span
    /// rows there.
    pub fn sort(&mut self, col: usize, ascending: bool, skip: usize) -> bool {
        let nr = self.nrows();
        if skip >= nr || col >= self.ncols() {
            return false;
        }
        for (r, c) in self.anchors() {
            let (rs, _) = self.span(r, c);
            if rs > 1 && r + rs > skip {
                return false;
            }
        }
        let keys: Vec<SortKey> = (skip..nr)
            .map(|r| {
                let (ar, ac) = self.anchor_of(r, col);
                SortKey::of(&self.rows[ar][ac].text(" "))
            })
            .collect();
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&keys[a], &keys[b]);
            match (x, y) {
                (SortKey::Empty, SortKey::Empty) => std::cmp::Ordering::Equal,
                (SortKey::Empty, _) => std::cmp::Ordering::Greater,
                (_, SortKey::Empty) => std::cmp::Ordering::Less,
                _ if ascending => x.cmp_to(y),
                _ => y.cmp_to(x),
            }
        });
        let mut body: Vec<Option<Vec<Cell>>> = self.rows.drain(skip..).map(Some).collect();
        let heights = self.props.row_heights.clone();
        let rules = self.props.row_rules.clone();
        for &i in &order {
            if let Some(row) = body[i].take() {
                self.rows.push(row);
            }
        }
        if heights.len() > skip {
            let h = |i: usize| heights.get(skip + i).copied().unwrap_or(0.0);
            let sorted: Vec<f32> = order.iter().map(|&i| h(i)).collect();
            self.props.row_heights.truncate(skip);
            self.props.row_heights.extend(sorted);
        }
        if rules.len() > skip {
            let sorted: Vec<RowRule> = order.iter().map(|&i| rules.get(skip + i).copied().unwrap_or_default()).collect();
            self.props.row_rules.truncate(skip);
            self.props.row_rules.extend(sorted);
        }
        self.fix();
        true
    }
}

enum SortKey {
    Empty,
    Num(f64),
    Text(String),
}

impl SortKey {
    fn of(s: &str) -> SortKey {
        let t = s.trim();
        if t.is_empty() {
            return SortKey::Empty;
        }
        let n: String = t.chars().filter(|c| !c.is_whitespace() && !matches!(c, '%' | '€' | '$' | '£')).collect();
        let n = if n.contains(',') && !n.contains('.') { n.replacen(',', ".", 1) } else if n.contains(',') && n.contains('.') { n.replace(',', "") } else { n };
        match n.parse::<f64>() {
            Ok(v) if v.is_finite() => SortKey::Num(v),
            _ => SortKey::Text(t.to_lowercase()),
        }
    }

    fn cmp_to(&self, o: &SortKey) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match (self, o) {
            (SortKey::Num(a), SortKey::Num(b)) => a.total_cmp(b),
            (SortKey::Num(_), _) => Less,
            (_, SortKey::Num(_)) => Greater,
            (SortKey::Text(a), SortKey::Text(b)) => a.cmp(b),
            _ => Equal,
        }
    }
}

/// Where a table is and how its cells map onto paragraphs, built from the
/// tags without copying anything.
#[derive(Clone, Debug, PartialEq)]
pub struct TableMap {
    /// Paragraph range of the table.
    pub start: usize,
    pub end: usize,
    pub rows: usize,
    pub cols: usize,
    /// First paragraph and end (exclusive) of each grid cell, row-major.
    pub cells: Vec<(usize, usize)>,
    /// The merged cell each grid cell belongs to (itself if not covered).
    pub anchor: Vec<(usize, usize)>,
    /// Rows and columns spanned by each anchor cell ((1, 1) for covered ones).
    pub span: Vec<(usize, usize)>,
    /// Nesting level (0 for a table that isn't in another one's cell).
    pub depth: usize,
}

impl TableMap {
    /// Maps the table that starts at paragraph `start`.
    pub fn build(paras: &[Paragraph], start: usize) -> Option<TableMap> {
        TableMap::build_at(paras, start, 0)
    }

    /// Maps the table at nesting level `depth` that starts at `start`.
    pub fn build_at(paras: &[Paragraph], start: usize, depth: usize) -> Option<TableMap> {
        let tag = |i: usize| paras.get(i).and_then(|p| tag_at(p, depth));
        let id = tag(start)?.table;
        let mut end = start;
        let (mut rows, mut cols) = (0usize, 0usize);
        while let Some(c) = tag(end).filter(|c| c.table == id) {
            rows = rows.max(c.row as usize + 1);
            cols = cols.max(c.col as usize + 1);
            end += 1;
        }
        let widths = tag(start).and_then(|c| c.table_props.as_ref()).map_or(0, |p| p.widths.len());
        // A file may give any number of grid columns: no more than a table has.
        let cols = cols.max(widths.min(MAX_COLS)).max(1);
        let rows = rows.min(end - start).max(1);
        let mut m = TableMap { start, end, rows, cols, cells: vec![(end, end); rows * cols], anchor: Vec::with_capacity(rows * cols), span: vec![(1, 1); rows * cols], depth };
        let mut i = start;
        while i < end {
            let c = tag(i)?;
            let (r, cc) = (c.row as usize, c.col as usize);
            let first = i;
            let spans = c.cell.as_ref().map(|p| (p.row_span.max(1) as usize, p.col_span.max(1) as usize, p.covered));
            i += 1;
            while i < end && tag(i).is_some_and(|x| x.row as usize == r && x.col as usize == cc) {
                i += 1;
            }
            if r < rows && cc < cols {
                m.cells[r * cols + cc] = (first, i);
                if let Some((rs, cs, covered)) = spans {
                    m.span[r * cols + cc] = if covered { (0, 0) } else { (rs.min(rows - r), cs.min(cols - cc)) };
                }
            }
        }
        m.anchor = (0..rows * cols).map(|k| (k / cols, k % cols)).collect();
        for r in 0..rows {
            for c in 0..cols {
                let (rs, cs) = m.span[r * cols + c];
                if rs == 0 || m.anchor[r * cols + c] != (r, c) {
                    continue;
                }
                for rr in r..r + rs {
                    for cc in c..c + cs {
                        if (rr, cc) != (r, c) {
                            m.anchor[rr * cols + cc] = (r, c);
                        }
                    }
                }
            }
        }
        for k in 0..rows * cols {
            if m.span[k] == (0, 0) || m.anchor[k] != (k / cols, k % cols) {
                m.span[k] = (1, 1);
            }
        }
        Some(m)
    }

    pub fn cell(&self, r: usize, c: usize) -> (usize, usize) {
        self.cells[r * self.cols + c]
    }

    pub fn anchor_of(&self, r: usize, c: usize) -> (usize, usize) {
        self.anchor[r * self.cols + c]
    }

    pub fn span_of(&self, r: usize, c: usize) -> (usize, usize) {
        self.span[r * self.cols + c]
    }

    pub fn is_anchor(&self, r: usize, c: usize) -> bool {
        self.anchor_of(r, c) == (r, c)
    }

    /// Grid cell of a paragraph of the table.
    pub fn pos_of(&self, paras: &[Paragraph], para: usize) -> (usize, usize) {
        let c = paras.get(para).and_then(|p| tag_at(p, self.depth));
        c.map_or((0, 0), |c| ((c.row as usize).min(self.rows - 1), (c.col as usize).min(self.cols - 1)))
    }

    pub fn props<'a>(&self, paras: &'a [Paragraph]) -> Option<&'a TableProps> {
        tag_at(paras.get(self.start)?, self.depth)?.table_props.as_deref()
    }

    pub fn cell_props<'a>(&self, paras: &'a [Paragraph], r: usize, c: usize) -> Option<&'a CellProps> {
        let (a, b) = self.cell(r, c);
        if a >= b {
            return None;
        }
        tag_at(&paras[a], self.depth)?.cell.as_deref()
    }

    /// The tables nested in this table's cells (one level in), in order.
    pub fn nested(&self, paras: &[Paragraph]) -> Vec<TableMap> {
        let mut out = Vec::new();
        let mut i = self.start;
        while i < self.end {
            if tag_at(&paras[i], self.depth + 1).is_some() {
                match TableMap::build_at(paras, i, self.depth + 1) {
                    Some(m) => {
                        i = m.end.max(i + 1);
                        out.push(m);
                    }
                    None => i += 1,
                }
            } else {
                i += 1;
            }
        }
        out
    }

    /// Grows a cell rectangle until no merged cell crosses its edges.
    pub fn expand(&self, mut r0: usize, mut c0: usize, mut r1: usize, mut c1: usize) -> (usize, usize, usize, usize) {
        loop {
            let before = (r0, c0, r1, c1);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let (ar, ac) = self.anchor_of(r, c);
                    let (rs, cs) = self.span_of(ar, ac);
                    r0 = r0.min(ar);
                    c0 = c0.min(ac);
                    r1 = r1.max(ar + rs - 1);
                    c1 = c1.max(ac + cs - 1);
                }
            }
            if before == (r0, c0, r1, c1) {
                return before;
            }
        }
    }
}

/// A rectangle of cells of the table whose paragraphs are `start..end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    /// Nesting level of the table.
    pub depth: usize,
    pub r0: usize,
    pub c0: usize,
    pub r1: usize,
    pub c1: usize,
}

impl Block {
    pub fn contains(&self, r: usize, c: usize) -> bool {
        (self.r0..=self.r1).contains(&r) && (self.c0..=self.c1).contains(&c)
    }
}

/// Where a cell of a block lies, for border commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

/// Separator for converting between text and tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSep {
    Tab,
    Comma,
    Semicolon,
    Char(char),
    /// Table to text only: every cell becomes its own paragraph(s).
    Paragraph,
}

impl TextSep {
    fn char(self) -> Option<char> {
        match self {
            TextSep::Tab => Some('\t'),
            TextSep::Comma => Some(','),
            TextSep::Semicolon => Some(';'),
            TextSep::Char(c) => Some(c),
            TextSep::Paragraph => None,
        }
    }
}

/// What a selection command selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TablePart {
    Cell,
    Row,
    Column,
    Table,
}

/// Repairs the tables of a paragraph list: complete grids, unique ids, and
/// a paragraph after a table that ends the document.
pub(crate) fn normalize_paras(ps: Vec<Paragraph>) -> Vec<Paragraph> {
    if ps.iter().all(|p| p.cell.is_none()) {
        return ps;
    }
    let mut all = Vec::new();
    for c in ps.iter().filter_map(|p| p.cell.as_ref()) {
        c.ids(&mut all);
    }
    let mut next = all.iter().copied().max().unwrap_or(0).wrapping_add(1);
    let mut used = std::collections::HashSet::new();
    let mut out = normalize_level(ps, &mut used, &mut next);
    if out.last().is_some_and(|p| p.cell.is_some()) {
        out.push(Paragraph::new(""));
    }
    out
}

/// Rebuilds the tables of a run of paragraphs (and those nested in their
/// cells), giving tables ids not used before.
fn normalize_level(ps: Vec<Paragraph>, used: &mut std::collections::HashSet<u32>, next: &mut u32) -> Vec<Paragraph> {
    normalize_at(ps, used, next, 0)
}

/// `normalize_level` at nesting `depth`: past `MAX_NESTING` the tables
/// are dropped and their paragraphs stay in the cell.
fn normalize_at(ps: Vec<Paragraph>, used: &mut std::collections::HashSet<u32>, next: &mut u32, depth: usize) -> Vec<Paragraph> {
    if ps.iter().all(|p| p.cell.is_none()) {
        return ps;
    }
    if depth >= MAX_NESTING {
        let mut ps = ps;
        for p in &mut ps {
            p.cell = None;
        }
        return ps;
    }
    // Runs of paragraphs outside tables or in the same table (taking `ps`
    // apart first: deep nesting mustn't keep a copy per level).
    let mut groups: Vec<(Option<u32>, Vec<Paragraph>)> = Vec::new();
    for p in ps {
        let id = p.cell.as_ref().map(|c| c.table);
        match groups.last_mut() {
            Some((g, v)) if *g == id && id.is_some() => v.push(p),
            _ => groups.push((id, vec![p])),
        }
    }
    let mut out = Vec::new();
    for (id, group) in groups {
        let Some(id) = id else {
            out.extend(group);
            continue;
        };
        let id = if used.insert(id) {
            id
        } else {
            while used.contains(next) {
                *next = next.wrapping_add(1);
            }
            used.insert(*next);
            *next
        };
        let mut t = Table::from_paras(group);
        for row in &mut t.rows {
            for cell in row {
                if cell.paras.iter().any(|p| p.cell.is_some()) {
                    cell.paras = normalize_at(std::mem::take(&mut cell.paras), used, next, depth + 1);
                }
            }
        }
        out.extend(t.into_paras(id));
    }
    out
}

impl Document {
    /// The table, row and column of the cell directly holding a paragraph
    /// (the innermost one for nested tables).
    pub fn container(&self, para: usize) -> Option<(u32, u32, u16)> {
        self.paragraphs.get(para)?.cell.as_ref().map(|c| c.innermost().key())
    }

    /// Nesting level of the innermost table holding a paragraph.
    pub fn table_depth(&self, para: usize) -> Option<usize> {
        self.paragraphs.get(para)?.cell.as_ref().map(|c| c.depth() - 1)
    }

    pub fn in_table(&self, para: usize) -> bool {
        self.paragraphs.get(para).is_some_and(|p| p.cell.is_some())
    }

    /// Paragraph range of the table containing `para`.
    pub fn table_bounds(&self, para: usize) -> Option<(usize, usize)> {
        let id = self.paragraphs.get(para)?.cell.as_ref()?.innermost().table;
        let same = |i: usize| self.paragraphs[i].cell.as_ref().is_some_and(|c| c.in_table(id));
        let mut a = para;
        while a > 0 && same(a - 1) {
            a -= 1;
        }
        let mut b = para + 1;
        while b < self.paragraphs.len() && same(b) {
            b += 1;
        }
        Some((a, b))
    }

    pub fn table_map(&self, para: usize) -> Option<TableMap> {
        let (a, _) = self.table_bounds(para)?;
        TableMap::build_at(&self.paragraphs, a, self.table_depth(para)?)
    }

    /// Maps of every table, nested ones too: each table comes before the
    /// tables in its cells.
    pub fn all_tables(&self) -> Vec<TableMap> {
        fn walk(paras: &[Paragraph], m: TableMap, out: &mut Vec<TableMap>) {
            let inner = m.nested(paras);
            out.push(m);
            for n in inner {
                walk(paras, n, out);
            }
        }
        let mut out = Vec::new();
        for m in self.tables() {
            walk(&self.paragraphs, m, &mut out);
        }
        out
    }

    /// Maps of the tables that aren't nested, in order.
    pub fn tables(&self) -> Vec<TableMap> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.paragraphs.len() {
            if self.paragraphs[i].cell.is_some() {
                match TableMap::build(&self.paragraphs, i) {
                    Some(m) => {
                        i = m.end.max(i + 1);
                        out.push(m);
                    }
                    None => i += 1,
                }
            } else {
                i += 1;
            }
        }
        out
    }

    /// True for paragraphs of cells covered by a merged cell: they are
    /// kept but not shown, and the caret skips them.
    pub fn is_hidden(&self, para: usize) -> bool {
        let Some(depth) = self.table_depth(para) else { return false };
        // Covered at any level.
        (0..=depth).any(|d| {
            let key = |i: usize| tag_at(&self.paragraphs[i], d).map(InCell::key);
            let k = key(para);
            let mut i = para;
            loop {
                if let Some(cp) = tag_at(&self.paragraphs[i], d).and_then(|c| c.cell.as_ref()) {
                    return cp.covered;
                }
                if i == 0 || key(i - 1) != k {
                    return false;
                }
                i -= 1;
            }
        })
    }

    /// Re-establishes the table invariants over the whole document.
    pub fn normalize_tables(&mut self) {
        let ps = std::mem::take(&mut self.paragraphs);
        self.paragraphs = normalize_paras(ps);
    }

    pub(crate) fn next_table_id(&self) -> u32 {
        let mut ids = Vec::new();
        for c in self.paragraphs.iter().filter_map(|p| p.cell.as_ref()) {
            c.ids(&mut ids);
        }
        ids.into_iter().max().map_or(1, |m| m.wrapping_add(1))
    }

    /// Replaces paragraphs `start..end` with `new` as one undo step.
    pub(crate) fn replace_paras(&mut self, start: usize, end: usize, new: Vec<Paragraph>, sel: Selection, after: Selection, kind: EditKind) {
        let n = new.len();
        let before: Vec<Paragraph> = self.paragraphs.splice(start..end, new).collect();
        self.record(start, before, n, sel, after, kind);
    }

    /// The cells between the selection's ends (grown over merged cells), when
    /// both ends are in the same table; a single cell for a caret in one.
    pub fn cell_block(&self, sel: Selection) -> Option<Block> {
        let m = self.table_map(sel.cursor.para)?;
        let (r1, c1) = m.pos_of(&self.paragraphs, sel.cursor.para);
        let same = (m.start..m.end).contains(&sel.anchor.para);
        let (r0, c0) = if same { m.pos_of(&self.paragraphs, sel.anchor.para) } else { (r1, c1) };
        let (r0, c0, r1, c1) = m.expand(r0.min(r1), c0.min(c1), r0.max(r1), c0.max(c1));
        Some(Block { start: m.start, end: m.end, depth: m.depth, r0, c0, r1, c1 })
    }

    /// Like `cell_block`, but only when the selection spans several cells of
    /// one table (it is then shown and edited as whole cells).
    pub fn block_selection(&self, sel: Selection) -> Option<Block> {
        let (a, c) = (self.container(sel.anchor.para)?, self.container(sel.cursor.para)?);
        if a.0 != c.0 || (a.1, a.2) == (c.1, c.2) {
            return None;
        }
        self.cell_block(sel)
    }

    /// The innermost table holding `para`, taken out for editing.
    fn take_table(&self, para: usize) -> Option<(Taken, Table)> {
        let (a, _) = self.table_bounds(para)?;
        self.take_at(a, self.table_depth(para)?)
    }

    /// The table of a block, taken out for editing.
    fn take_block(&self, b: &Block) -> Option<(Taken, Table)> {
        self.take_at(b.start, b.depth)
    }

    /// The table at nesting level `depth` starting at paragraph `a`.
    fn take_at(&self, a: usize, depth: usize) -> Option<(Taken, Table)> {
        let m = TableMap::build_at(&self.paragraphs, a, depth)?;
        let tag = self.paragraphs[a].cell.as_ref()?;
        let id = tag.at(depth)?.table;
        let outer = tag.outer(depth);
        let paras = self.paragraphs[a..m.end].iter().map(|p| promote(p.clone(), depth)).collect();
        Some((Taken { a, e: m.end, id, outer }, Table::from_paras(paras)))
    }

    /// Paragraphs of a changed table (or of what replaces it) back in the
    /// cells around it.
    fn wrap_taken(tk: &Taken, mut paras: Vec<Paragraph>) -> Vec<Paragraph> {
        if let Some(outer) = &tk.outer {
            for (k, p) in paras.iter_mut().enumerate() {
                let chain = if k == 0 { outer.clone() } else { outer.bare() };
                p.cell = Some(chain.wrap(p.cell.take()));
            }
        }
        paras
    }

    /// Replaces a taken table with `t` as one undo step.
    fn put_raw(&mut self, tk: &Taken, t: Table, sel: Selection, after: Selection, kind: EditKind) {
        let new = Self::wrap_taken(tk, t.into_paras(tk.id));
        self.replace_paras(tk.a, tk.e, new, sel, after, kind);
    }

    /// Writes a changed table back; the caret goes to the first paragraph
    /// of cell `at`.
    fn put_table(&mut self, tk: &Taken, t: Table, sel: Selection, at: (usize, usize)) -> Selection {
        let (r, c) = (at.0.min(t.nrows() - 1), at.1.min(t.ncols() - 1));
        let (r, c) = t.anchor_of(r, c);
        let caret = Selection::caret(Pos::new(tk.a + t.para_offset(r, c), 0));
        self.put_raw(tk, t, sel, caret, EditKind::Table);
        caret
    }

    /// Inserts a `rows` x `cols` table at the caret, `width` points wide
    /// (in a cell, nested in it). Returns the caret, in its first cell.
    pub fn insert_table(&mut self, sel: Selection, rows: usize, cols: usize, width: f32) -> Selection {
        let mut t = Table::new(rows, cols, width);
        let id = self.next_table_id();
        let p = self.clamp(sel.start());
        let mut head = self.paragraphs[p.para].clone();
        let mut tail = head.split_off(p.offset);
        // Formatting of the paragraph carries into the cells.
        for row in &mut t.rows {
            for cell in row {
                cell.paras[0].style.align = head.style.align;
                cell.paras[0].runs[0].attrs = head.attrs_at(p.offset);
                cell.paras[0].runs[0].attrs.link = None;
            }
        }
        let around = head.cell.clone();
        let mut new = Vec::new();
        let caret;
        if p.offset == 0 {
            // The table goes before the paragraph, which keeps its marks.
            tail.marks.append(&mut head.marks);
            tail.style = head.style;
            caret = Selection::caret(Pos::new(p.para, 0));
        } else {
            new.push(head);
            caret = Selection::caret(Pos::new(p.para + 1, 0));
        }
        let mut tp = t.into_paras(id);
        if let Some(c) = &around {
            wrap_in(c, p.offset == 0, &mut tp);
        }
        new.extend(tp);
        new.push(tail);
        self.replace_paras(p.para, p.para + 1, new, sel, caret, EditKind::Table);
        caret
    }

    /// Inserts as many rows as the selection covers, above or below it.
    pub fn table_insert_rows(&mut self, sel: Selection, below: bool) -> Option<Selection> {
        let b = self.cell_block(sel)?;
        let (tk, mut t) = self.take_block(&b)?;
        let n = b.r1 - b.r0 + 1;
        let (at, tmpl) = if below { (b.r1 + 1, b.r1) } else { (b.r0, b.r0) };
        t.insert_rows(at, n, tmpl);
        Some(self.put_table(&tk, t, sel, (at, b.c0)))
    }

    /// Inserts as many columns as the selection covers, left or right of
    /// it. The table is narrowed to `max_width` if it gets wider.
    pub fn table_insert_cols(&mut self, sel: Selection, right: bool, max_width: f32) -> Option<Selection> {
        let b = self.cell_block(sel)?;
        let (tk, mut t) = self.take_block(&b)?;
        let n = b.c1 - b.c0 + 1;
        let (at, tmpl) = if right { (b.c1 + 1, b.c1) } else { (b.c0, b.c0) };
        let before = t.props.width();
        t.insert_cols(at, n, tmpl);
        let w = t.props.width();
        let limit = before.max(max_width);
        if w > limit && w > 0.0 {
            for v in &mut t.props.widths {
                *v = (*v * limit / w).max(MIN_COL_WIDTH);
            }
        }
        Some(self.put_table(&tk, t, sel, (b.r0, at)))
    }

    /// Deletes the rows of the selection (the whole table if that's all).
    pub fn table_delete_rows(&mut self, sel: Selection) -> Option<Selection> {
        let b = self.cell_block(sel)?;
        let (tk, mut t) = self.take_block(&b)?;
        if !t.delete_rows(b.r0, b.r1) {
            return self.delete_table(sel);
        }
        Some(self.put_table(&tk, t, sel, (b.r0, b.c0)))
    }

    /// Deletes the columns of the selection (the whole table if that's all).
    pub fn table_delete_cols(&mut self, sel: Selection) -> Option<Selection> {
        let b = self.cell_block(sel)?;
        let (tk, mut t) = self.take_block(&b)?;
        if !t.delete_cols(b.c0, b.c1) {
            return self.delete_table(sel);
        }
        Some(self.put_table(&tk, t, sel, (b.r0, b.c0.saturating_sub(1))))
    }

    pub fn delete_table(&mut self, sel: Selection) -> Option<Selection> {
        let (a, b) = self.table_bounds(sel.cursor.para)?;
        let caret = Selection::caret(Pos::new(a, 0));
        let depth = self.table_depth(sel.cursor.para)?;
        if depth > 0 {
            // The paragraph after a nested table (always in the same cell)
            // takes the properties of the cells around it that the table
            // had.
            let (tk, _) = self.take_at(a, depth)?;
            let mut next = self.paragraphs.get(b)?.clone();
            if let (Some(outer), Some(to)) = (&tk.outer, next.cell.as_mut()) {
                inherit_props(outer, to);
            }
            self.replace_paras(a, b + 1, vec![next], sel, caret, EditKind::Table);
            return Some(caret);
        }
        self.replace_paras(a, b, Vec::new(), sel, caret, EditKind::Table);
        // A table can't end the document (there is always a paragraph
        // after one), so `a` is a paragraph now.
        Some(caret)
    }

    /// Merges the selected cells into one.
    pub fn merge_cells(&mut self, sel: Selection) -> Option<Selection> {
        let b = self.cell_block(sel)?;
        if (b.r0, b.c0) == (b.r1, b.c1) {
            return None;
        }
        let (tk, mut t) = self.take_block(&b)?;
        t.merge(b.r0, b.c0, b.r1, b.c1);
        Some(self.put_table(&tk, t, sel, (b.r0, b.c0)))
    }

    /// Splits the cell at the caret (see `Table::split`).
    pub fn split_cell(&mut self, sel: Selection, rows: usize, cols: usize) -> Option<Selection> {
        let b = self.cell_block(Selection::caret(sel.cursor))?;
        let (tk, mut t) = self.take_block(&b)?;
        let before = t.clone();
        t.split(b.r0, b.c0, rows, cols);
        if t == before {
            return None;
        }
        Some(self.put_table(&tk, t, sel, (b.r0, b.c0)))
    }

    /// Changes the properties of the table at the caret.
    pub fn set_table_props(&mut self, sel: Selection, f: &dyn Fn(&mut TableProps)) -> bool {
        let Some((tk, mut t)) = self.take_table(sel.cursor.para) else { return false };
        let old = t.props.clone();
        f(&mut t.props);
        t.fix();
        if t.props == old {
            return false;
        }
        self.put_raw(&tk, t, sel, sel, EditKind::Table);
        true
    }

    /// Changes the properties of the selected cells (the cell at the caret
    /// if the selection is inside one); `f` also gets where each cell lies
    /// in the selection.
    pub fn set_cell_props(&mut self, sel: Selection, f: &dyn Fn(&mut CellProps, Edges)) -> bool {
        let Some(b) = self.cell_block(sel) else { return false };
        let Some((tk, mut t)) = self.take_block(&b) else { return false };
        let old = t.clone();
        for r in b.r0..=b.r1 {
            for c in b.c0..=b.c1 {
                if t.rows[r][c].props.covered {
                    continue;
                }
                let (rs, cs) = t.span(r, c);
                let edges = Edges { top: r == b.r0, bottom: r + rs > b.r1, left: c == b.c0, right: c + cs > b.c1 };
                f(&mut t.rows[r][c].props, edges);
            }
        }
        share_edges(&mut t, &old, &b);
        t.fix();
        if t == old {
            return false;
        }
        self.put_raw(&tk, t, sel, sel, EditKind::Table);
        true
    }

    /// Moves the vertical border `edge` (0 = left edge of the table) of the
    /// table containing `para` by `delta` points.
    pub fn resize_column(&mut self, para: usize, edge: usize, delta: f32, sel: Selection) -> bool {
        if !delta.is_finite() || delta == 0.0 {
            return false;
        }
        self.set_table_props(sel_in(para, sel), &move |p: &mut TableProps| {
            let n = p.widths.len();
            if n == 0 || edge > n {
                return;
            }
            if edge == 0 {
                let d = delta.min(p.widths[0] - MIN_COL_WIDTH);
                p.indent += d;
                p.widths[0] -= d;
            } else if edge == n {
                p.widths[n - 1] = (p.widths[n - 1] + delta).max(MIN_COL_WIDTH);
            } else {
                // Columns already narrower than the minimum (imported) keep their widths.
                let (lo, hi) = (MIN_COL_WIDTH - p.widths[edge - 1], p.widths[edge] - MIN_COL_WIDTH);
                let d = if lo <= hi { delta.clamp(lo, hi) } else { 0.0 };
                p.widths[edge - 1] += d;
                p.widths[edge] -= d;
            }
        })
    }

    /// Clears the text of the selected cells, keeping the grid.
    pub fn clear_cells(&mut self, sel: Selection) -> Option<Pos> {
        let b = self.cell_block(sel)?;
        let (tk, mut t) = self.take_block(&b)?;
        for r in b.r0..=b.r1 {
            for c in b.c0..=b.c1 {
                let cell = &mut t.rows[r][c];
                let mut p = cell.paras[0].clone();
                let len = p.len();
                p.remove(0, len);
                p.marks.clear();
                let marks: Vec<_> = cell.paras.iter_mut().flat_map(|p| std::mem::take(&mut p.marks)).map(|m| crate::Mark { offset: 0, ..m }).collect();
                p.marks = marks;
                cell.paras = vec![p];
            }
        }
        let s = self.put_table(&tk, t, sel, (b.r0, b.c0));
        if let Some(last) = self.undo.last_mut() {
            last.kind = EditKind::Delete;
        }
        Some(s.cursor)
    }

    /// Pastes a grid of cells (the paragraphs of each) into the table from
    /// the caret's cell, adding rows at the bottom when needed. Cells that
    /// fall outside the columns or inside merged cells are dropped.
    pub fn fill_cells(&mut self, sel: Selection, grid: &[Vec<Vec<Paragraph>>]) -> Option<Selection> {
        let b = self.cell_block(Selection::caret(sel.start()))?;
        if grid.is_empty() {
            return None;
        }
        let (tk, mut t) = self.take_block(&b)?;
        let need = b.r0 + grid.len();
        if need > t.nrows() {
            let last = t.nrows() - 1;
            t.insert_rows(t.nrows(), need - t.nrows(), last);
        }
        for (i, row) in grid.iter().enumerate() {
            for (j, paras) in row.iter().enumerate() {
                let (r, c) = (b.r0 + i, b.c0 + j);
                if paras.is_empty() || c >= t.ncols() || t.anchor_of(r, c) != (r, c) {
                    continue;
                }
                t.rows[r][c].paras = paras
                    .iter()
                    .map(|p| {
                        let mut p = p.clone();
                        p.cell = None;
                        p
                    })
                    .collect();
            }
        }
        Some(self.put_table(&tk, t, sel, (b.r0, b.c0)))
    }

    /// Selection covering a cell, row, column or the whole table.
    pub fn select_table_part(&self, sel: Selection, part: TablePart) -> Option<Selection> {
        let m = self.table_map(sel.cursor.para)?;
        let b = self.cell_block(sel)?;
        let (r0, c0, r1, c1) = match part {
            TablePart::Cell => (b.r0, b.c0, b.r1, b.c1),
            TablePart::Row => (b.r0, 0, b.r1, m.cols - 1),
            TablePart::Column => (0, b.c0, m.rows - 1, b.c1),
            TablePart::Table => (0, 0, m.rows - 1, m.cols - 1),
        };
        let (r0, c0, r1, c1) = m.expand(r0, c0, r1, c1);
        let (a, _) = m.cell(r0, c0);
        let (_, e) = m.cell(r1, c1);
        let a = a.min(self.paragraphs.len() - 1);
        let e = e.clamp(a + 1, m.end);
        let last = e - 1;
        Some(Selection { anchor: Pos::new(a, 0), cursor: Pos::new(last, self.paragraphs[last].len()) })
    }

    /// The next (or previous) cell's content, for Tab and Shift+Tab. `None`
    /// past the last or first cell.
    pub fn next_cell(&self, para: usize, back: bool) -> Option<Selection> {
        let m = self.table_map(para)?;
        let (r, c) = m.pos_of(&self.paragraphs, para);
        let (r, c) = m.anchor_of(r, c);
        let mut k = r * m.cols + c;
        loop {
            if back {
                k = k.checked_sub(1)?;
            } else {
                k += 1;
                if k >= m.rows * m.cols {
                    return None;
                }
            }
            let (rr, cc) = (k / m.cols, k % m.cols);
            if m.is_anchor(rr, cc) {
                let (a, e) = m.cell(rr, cc);
                if a < e {
                    let last = e - 1;
                    return Some(Selection { anchor: Pos::new(a, 0), cursor: Pos::new(last, self.paragraphs[last].len()) });
                }
            }
        }
    }

    /// The text of the selected cells as tab-separated rows.
    pub fn block_text(&self, b: &Block) -> String {
        let Some(m) = TableMap::build_at(&self.paragraphs, b.start, b.depth) else { return String::new() };
        let mut out = String::new();
        for r in b.r0..=b.r1 {
            if r > b.r0 {
                out.push('\n');
            }
            for c in b.c0..=b.c1 {
                if c > b.c0 {
                    out.push('\t');
                }
                if !m.is_anchor(r, c) {
                    continue;
                }
                let (a, e) = m.cell(r, c);
                let text: Vec<String> = (a..e).map(|i| self.paragraphs[i].text.replace(LINE_BREAK, " ")).collect();
                out.push_str(&text.join(" "));
            }
        }
        out
    }

    /// The selected cells as a standalone table (for the clipboard).
    pub fn block_slice(&self, b: &Block) -> Vec<Paragraph> {
        let Some(t) = self.take_block(b).map(|x| x.1) else { return Vec::new() };
        let mut rows: Vec<Vec<Cell>> = t.rows[b.r0..=b.r1].iter().map(|r| r[b.c0..=b.c1].to_vec()).collect();
        for row in &mut rows {
            for cell in row {
                for p in &mut cell.paras {
                    p.marks.clear();
                }
            }
        }
        let mut props = t.props.clone();
        props.widths = props.widths[b.c0..=b.c1].to_vec();
        props.row_heights = props.row_heights.iter().skip(b.r0).take(b.r1 - b.r0 + 1).copied().collect();
        props.row_rules = props.row_rules.iter().skip(b.r0).take(b.r1 - b.r0 + 1).copied().collect();
        props.header_rows = 0;
        let mut t = Table { props, rows };
        t.fix();
        t.into_paras(0)
    }

    /// Converts the selected paragraphs into a table, one row each, split at
    /// `sep`. Returns the table's selection.
    pub fn text_to_table(&mut self, sel: Selection, sep: TextSep, width: f32) -> Option<Selection> {
        let (a, b) = (sel.start().para, sel.end().para);
        let sepc = sep.char()?;
        if (a..=b).any(|i| self.in_table(i)) {
            return None;
        }
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        for i in a..=b {
            let mut p = self.paragraphs[i].clone();
            p.style.page_break_before = false;
            if p.style.kind.is_list() {
                p.style.kind = crate::ParaKind::Normal;
                p.style.level = 0;
            }
            let mut cells = Vec::new();
            while let Some(k) = p.text.find(sepc) {
                let mut rest = p.split_off(k);
                rest.remove(0, sepc.len_utf8());
                rest.style = p.style;
                cells.push(Cell { props: CellProps::default(), paras: vec![p] });
                p = rest;
            }
            cells.push(Cell { props: CellProps::default(), paras: vec![p] });
            rows.push(cells);
        }
        let cols = rows.iter().map(Vec::len).max().unwrap_or(1).min(MAX_COLS);
        let mut t = Table::new(rows.len(), cols, width);
        for (r, row) in rows.into_iter().enumerate() {
            for (c, cell) in row.into_iter().enumerate() {
                if c < cols {
                    t.rows[r][c] = cell;
                } else {
                    // Past the column limit: keep the text in the last cell.
                    let extra = cell.paras;
                    t.rows[r][cols - 1].paras.extend(extra);
                }
            }
        }
        t.fix();
        let id = self.next_table_id();
        let mut new = t.into_paras(id);
        let n = new.len();
        if b + 1 >= self.paragraphs.len() || self.in_table(b + 1) {
            new.push(Paragraph::new(""));
        }
        let last = a + n - 1;
        let after = Selection { anchor: Pos::new(a, 0), cursor: Pos::new(last, new[n - 1].len()) };
        self.replace_paras(a, b + 1, new, sel, after, EditKind::Table);
        Some(after)
    }

    /// Converts the table at the caret into paragraphs: one per row with
    /// the cells separated by `sep`, or one per cell paragraph.
    pub fn table_to_text(&mut self, sel: Selection, sep: TextSep) -> Option<Selection> {
        let (tk, t) = self.take_table(sel.cursor.para)?;
        let a = tk.a;
        let mut out: Vec<Paragraph> = Vec::new();
        for row in t.rows {
            let mut line: Option<Paragraph> = None;
            for cell in row {
                if cell.props.covered {
                    continue;
                }
                match sep.char() {
                    None => out.extend(cell.paras),
                    Some(ch) => {
                        let mut joined: Option<Paragraph> = None;
                        for p in cell.paras {
                            match &mut joined {
                                None => joined = Some(p),
                                Some(j) => {
                                    let at = j.len();
                                    let attrs = j.attrs_at(at);
                                    j.insert_str(at, " ", attrs);
                                    j.append(p);
                                }
                            }
                        }
                        let Some(mut cell_p) = joined else { continue };
                        cell_p.cell = None;
                        match &mut line {
                            None => line = Some(cell_p),
                            Some(l) => {
                                let at = l.len();
                                let attrs = l.attrs_at(at);
                                l.insert_str(at, &ch.to_string(), attrs);
                                l.append(cell_p);
                            }
                        }
                    }
                }
            }
            out.extend(line);
        }
        if out.is_empty() {
            out.push(Paragraph::new(""));
        }
        // Tables that were in the cells stay (as tables of this level).
        let mut out = normalize_level(out, &mut Default::default(), &mut self.next_table_id());
        if out.last().is_some_and(|p| p.cell.is_some()) {
            out.push(Paragraph::new(""));
        }
        let n = out.len();
        let last_len = out[n - 1].len();
        let after = Selection { anchor: Pos::new(a, 0), cursor: Pos::new(a + n - 1, last_len) };
        let out = Self::wrap_taken(&tk, out);
        self.replace_paras(a, tk.e, out, sel, after, EditKind::Table);
        Some(after)
    }

    /// Sorts the table rows by column `col`; `header` keeps the header rows
    /// (at least the first) in place. False if merged cells prevent it.
    pub fn sort_table(&mut self, sel: Selection, col: usize, ascending: bool, header: bool) -> bool {
        let Some((tk, mut t)) = self.take_table(sel.cursor.para) else { return false };
        let skip = if header { (t.props.header_rows as usize).max(1) } else { 0 };
        let before = t.clone();
        if !t.sort(col, ascending, skip) {
            return false;
        }
        if t != before {
            self.put_raw(&tk, t, sel, Selection::caret(Pos::new(tk.a, 0)), EditKind::Table);
        }
        true
    }

    /// Pasted paragraphs with fresh table ids (at every level) and no
    /// bookmarks.
    pub(crate) fn retag_pasted(&self, paras: &[Paragraph]) -> Vec<Paragraph> {
        let mut next = self.next_table_id();
        let mut ids: HashMap<u32, u32> = HashMap::new();
        paras
            .iter()
            .map(|p| {
                let mut p = Paragraph { marks: Vec::new(), ..p.clone() };
                if let Some(c) = &mut p.cell {
                    c.map_ids(&mut |id| {
                        *ids.entry(id).or_insert_with(|| {
                            next = next.wrapping_add(1);
                            next.wrapping_sub(1)
                        })
                    });
                }
                p
            })
            .collect()
    }
}

/// Nests the tables of `paras` in the cell whose tags are `cell` (a
/// paragraph's); the first gets the cell's properties when `first`.
pub(crate) fn wrap_in(cell: &InCell, first: bool, paras: &mut [Paragraph]) {
    for (k, p) in paras.iter_mut().enumerate() {
        let base = if k == 0 && first { cell.clone() } else { cell.bare() };
        p.cell = Some(base.wrap(p.cell.take()));
    }
}

/// Gives `to` the cell and table properties of `from` (the tags of a
/// paragraph that goes away) at the levels where they are the same cell.
pub(crate) fn inherit_props(from: &InCell, to: &mut InCell) {
    let (mut f, mut t) = (Some(from), Some(to));
    while let (Some(a), Some(b)) = (f, t) {
        if a.key() != b.key() {
            break;
        }
        if b.cell.is_none() {
            b.cell.clone_from(&a.cell);
        }
        if b.table_props.is_none() {
            b.table_props.clone_from(&a.table_props);
        }
        f = a.nested.as_deref();
        t = b.nested.as_deref_mut();
    }
}

/// A caret in paragraph `para` (keeping `sel` when it is already there).
fn sel_in(para: usize, sel: Selection) -> Selection {
    if sel.cursor.para == para { sel } else { Selection::caret(Pos::new(para, 0)) }
}

/// A border changed on the outside of block `b` is shared with the cell
/// beyond it, which draws the same line: that cell's facing side follows,
/// or "no border" on a cell would leave its right and bottom lines.
fn share_edges(t: &mut Table, old: &Table, b: &Block) {
    let rows = t.rows.len();
    let mut set: Vec<(usize, usize, u8, Option<Border>)> = Vec::new();
    for r in b.r0..=b.r1 {
        for c in b.c0..=b.c1 {
            if r >= rows || c >= t.rows[r].len() || t.rows[r][c].props.covered {
                continue;
            }
            let (rs, cs) = t.span(r, c);
            let (new, was) = (&t.rows[r][c].props.borders, &old.rows[r][c].props.borders);
            // Top, bottom, left, right: the neighbors' cells and their facing side.
            if new.top != was.top && r == b.r0 && r > 0 {
                (c..c + cs).for_each(|cc| set.push((r - 1, cc, 1, new.top)));
            }
            if new.bottom != was.bottom && r + rs > b.r1 && r + rs < rows {
                (c..c + cs).for_each(|cc| set.push((r + rs, cc, 0, new.bottom)));
            }
            if new.left != was.left && c == b.c0 && c > 0 {
                (r..r + rs).for_each(|rr| set.push((rr, c - 1, 3, new.left)));
            }
            if new.right != was.right && c + cs > b.c1 {
                (r..r + rs).for_each(|rr| set.push((rr, c + cs, 2, new.right)));
            }
        }
    }
    for (r, c, side, v) in set {
        if r >= rows || c >= t.rows[r].len() {
            continue;
        }
        let (ar, ac) = t.anchor_of(r, c);
        let bd = &mut t.rows[ar][ac].props.borders;
        *match side {
            0 => &mut bd.top,
            1 => &mut bd.bottom,
            2 => &mut bd.left,
            _ => &mut bd.right,
        } = v;
    }
}
