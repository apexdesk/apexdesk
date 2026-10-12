//! Tables on slides (`a:tbl`): column widths, rows of cells with their
//! text, fill, borders and merges, and a table style from the built-in
//! ones (header row and banded rows in an accent color).

use std::sync::Arc;

use apex_doc::VAnchor;

use crate::color::{Mod, SColor, Scheme};
use crate::fill::{Fill, Line};
use crate::text::TextBody;

/// Most columns a table keeps (PowerPoint makes up to 75).
pub const MAX_COLS: usize = 1000;
/// Most cells a table keeps, rows times columns.
pub const MAX_CELLS: usize = 250_000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub text: TextBody,
    pub fill: Option<Fill>,
    /// Left, right, top, bottom.
    pub borders: [Option<Line>; 4],
    /// Columns and rows it spans (1 = itself).
    pub span: u32,
    pub rspan: u32,
    /// Covered by the cell to its left or above.
    pub h_merge: bool,
    pub v_merge: bool,
    /// Margins left, right, top, bottom, points.
    pub margins: [Option<f32>; 4],
    pub anchor: Option<VAnchor>,
}

impl Cell {
    pub fn new(text: &str) -> Cell {
        Cell { text: TextBody::plain(text), span: 1, rspan: 1, ..Default::default() }
    }

    pub fn covered(&self) -> bool {
        self.h_merge || self.v_merge
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// Points (the least: rows grow with their text).
    pub height: f32,
    pub cells: Vec<Cell>,
}

/// Which parts of the table style apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableLook {
    pub first_row: bool,
    pub last_row: bool,
    pub first_col: bool,
    pub last_col: bool,
    pub band_rows: bool,
    pub band_cols: bool,
}

impl Default for TableLook {
    fn default() -> Self {
        TableLook { first_row: true, last_row: false, first_col: false, last_col: false, band_rows: true, band_cols: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// Column widths, points.
    pub cols: Vec<f32>,
    pub rows: Vec<Row>,
    /// Table style id (a GUID of the built-in styles).
    pub style: Option<Arc<str>>,
    pub look: TableLook,
}

/// PowerPoint's default table style: Medium Style 2 - Accent 1.
pub const DEFAULT_STYLE: &str = "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}";

/// The built-in styles we draw: id, accent (None: dark), family.
const STYLES: &[(&str, Option<u8>, u8)] = &[
    ("{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}", Some(0), 2),
    ("{21E4AEA4-8DFA-4A89-87EB-49C32662AFE8}", Some(1), 2),
    ("{F5AB1C69-6EDB-4FF4-983F-18BD219EF322}", Some(2), 2),
    ("{00A15C55-8517-42AA-B614-E9B94910E393}", Some(3), 2),
    ("{7DF18680-E054-41AD-8BC1-D1AEF772440D}", Some(4), 2),
    ("{93296810-A885-4BE3-A3E7-6D5BEEA58F35}", Some(5), 2),
    ("{073A0DAA-6AF3-43AB-8588-CEC1D06C72B9}", None, 2),
    ("{9D7B26C5-4107-4FEC-AEDC-1716B250EE53}", None, 1),
    ("{3B4B98B0-60AC-42C2-AFA5-B58CD77FA1E5}", Some(0), 1),
    ("{BC89EF96-8CEA-46FF-86C4-4CE0E7609802}", Some(0), 3),
    ("{2D5ABB26-0587-4C30-8999-92F81FD0307C}", None, 0),
];

/// Built-in style ids offered in the gallery, in order.
pub fn style_ids() -> impl Iterator<Item = &'static str> {
    STYLES.iter().map(|s| s.0)
}

/// How a cell looks under the table style: fill, text color and bold.
#[derive(Clone, Debug, PartialEq)]
pub struct CellLook {
    pub fill: Option<Fill>,
    pub text: Option<SColor>,
    pub bold: bool,
    /// Outline of the cell (thin white lines in the medium styles).
    pub border: Option<Line>,
}

impl Table {
    /// A table of `rows` by `cols` empty cells in a `w` wide frame.
    pub fn new(rows: usize, cols: usize, w: f32) -> Table {
        let cols = cols.max(1);
        let cw = w / cols as f32;
        Table {
            cols: vec![cw; cols],
            rows: (0..rows.max(1)).map(|_| Row { height: 26.0, cells: (0..cols).map(|_| Cell::new("")).collect() }).collect(),
            style: Some(Arc::from(DEFAULT_STYLE)),
            look: TableLook::default(),
        }
    }

    pub fn width(&self) -> f32 {
        self.cols.iter().sum()
    }

    /// Keeps every row as wide as the grid. A grid past [`MAX_COLS`]
    /// columns or [`MAX_CELLS`] cells (from a damaged file) is cut.
    pub fn normalize(&mut self) {
        if self.cols.is_empty() {
            self.cols.push(72.0);
        }
        self.cols.truncate(MAX_COLS);
        let n = self.cols.len();
        self.rows.truncate(MAX_CELLS / n);
        for r in &mut self.rows {
            r.cells.resize_with(n, || Cell::new(""));
            for c in &mut r.cells {
                c.span = c.span.max(1);
                c.rspan = c.rspan.max(1);
            }
        }
    }

    /// The style's look for cell (r, c).
    pub fn cell_look(&self, r: usize, c: usize) -> CellLook {
        let s = self.style.as_deref().unwrap_or("");
        let (accent, family) = STYLES.iter().find(|x| x.0.eq_ignore_ascii_case(s)).map_or((Some(0), 2), |x| (x.1, x.2));
        if s.is_empty() || family == 0 {
            // No style, no grid.
            return CellLook { fill: None, text: None, bold: false, border: None };
        }
        let base = match accent {
            Some(a) => SColor::scheme(Scheme::Accent(a)),
            None => SColor::scheme(Scheme::Tx1),
        };
        let nrows = self.rows.len();
        let ncols = self.cols.len();
        let header = self.look.first_row && r == 0;
        let footer = self.look.last_row && r + 1 == nrows && nrows > 1;
        let first_col = self.look.first_col && c == 0;
        let last_col = self.look.last_col && c + 1 == ncols && ncols > 1;
        let body_r = if self.look.first_row { r.saturating_sub(1) } else { r };
        let band = self.look.band_rows && body_r % 2 == 0 || self.look.band_cols && c % 2 == 0;
        let white = Line::solid(SColor::scheme(Scheme::Lt1), 1.0);
        match family {
            // Light styles: lines in the accent, header text bold.
            1 => CellLook {
                fill: if band && !header { Some(Fill::Solid(base.clone().with(Mod::Alpha(20000)))) } else { None },
                text: None,
                bold: header || footer || first_col || last_col,
                border: Some(Line::solid(base, 1.0)),
            },
            // Dark styles.
            3 => CellLook {
                fill: Some(Fill::Solid(if header || footer { SColor::scheme(Scheme::Dk1) } else if band { base.clone().with(Mod::Shade(60000)) } else { base.clone().with(Mod::Shade(40000)) })),
                text: Some(SColor::scheme(Scheme::Lt1)),
                bold: header || footer || first_col || last_col,
                border: Some(white),
            },
            // Medium styles (the default).
            _ => {
                if header || footer || first_col || last_col {
                    CellLook { fill: Some(Fill::Solid(base)), text: Some(SColor::scheme(Scheme::Lt1)), bold: true, border: Some(white) }
                } else {
                    let tint = if band { 40000 } else { 20000 };
                    CellLook { fill: Some(Fill::Solid(base.with(Mod::Tint(tint)))), text: Some(SColor::scheme(Scheme::Dk1)), bold: false, border: Some(white) }
                }
            }
        }
    }

    /// Inserts a row at `at` like row `like`.
    pub fn insert_row(&mut self, at: usize, like: usize) {
        let n = self.cols.len();
        let h = self.rows.get(like).map_or(26.0, |r| r.height);
        let cells = (0..n).map(|c| {
            let mut cell = Cell::new("");
            if let Some(src) = self.rows.get(like).and_then(|r| r.cells.get(c)) {
                cell.fill = src.fill.clone();
                cell.borders = src.borders.clone();
                if let (Some(p), Some(d)) = (src.text.paras.first(), cell.text.paras.first_mut()) {
                    d.style = p.style;
                    d.runs[0].attrs = p.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
                }
                cell.text.props = src.text.props.clone();
            }
            cell
        });
        let row = Row { height: h, cells: cells.collect() };
        self.rows.insert(at.min(self.rows.len()), row);
    }

    pub fn insert_col(&mut self, at: usize, like: usize) {
        let w = self.cols.get(like).copied().unwrap_or(72.0);
        let at = at.min(self.cols.len());
        self.cols.insert(at, w);
        for r in &mut self.rows {
            r.cells.insert(at.min(r.cells.len()), Cell::new(""));
        }
    }

    pub fn delete_row(&mut self, r: usize) {
        if self.rows.len() > 1 && r < self.rows.len() {
            self.rows.remove(r);
        }
    }

    pub fn delete_col(&mut self, c: usize) {
        if self.cols.len() > 1 && c < self.cols.len() {
            self.cols.remove(c);
            for r in &mut self.rows {
                if c < r.cells.len() {
                    r.cells.remove(c);
                }
            }
        }
    }

    /// Merges the cells of rows r0..=r1 and columns c0..=c1 into the
    /// top-left one (their text is appended to it).
    pub fn merge(&mut self, r0: usize, c0: usize, r1: usize, c1: usize) {
        let (r1, c1) = (r1.min(self.rows.len() - 1), c1.min(self.cols.len() - 1));
        if r0 > r1 || c0 > c1 {
            return;
        }
        let mut extra = Vec::new();
        for r in r0..=r1 {
            for c in c0..=c1 {
                if (r, c) == (r0, c0) {
                    continue;
                }
                let cell = &mut self.rows[r].cells[c];
                if !cell.text.is_empty() {
                    extra.extend(cell.text.paras.drain(..));
                }
                cell.text = TextBody::plain("");
                cell.h_merge = c > c0;
                cell.v_merge = r > r0 && c == c0 || r > r0;
                cell.span = 1;
                cell.rspan = 1;
            }
        }
        let head = &mut self.rows[r0].cells[c0];
        head.span = (c1 - c0 + 1) as u32;
        head.rspan = (r1 - r0 + 1) as u32;
        head.h_merge = false;
        head.v_merge = false;
        head.text.paras.extend(extra);
    }

    /// Splits a merged cell back into its cells.
    pub fn unmerge(&mut self, r0: usize, c0: usize) {
        let Some(head) = self.rows.get_mut(r0).and_then(|r| r.cells.get_mut(c0)) else { return };
        let (span, rspan) = (head.span as usize, head.rspan as usize);
        head.span = 1;
        head.rspan = 1;
        for r in r0..(r0 + rspan).min(self.rows.len()) {
            for c in c0..(c0 + span).min(self.cols.len()) {
                let cell = &mut self.rows[r].cells[c];
                cell.h_merge = false;
                cell.v_merge = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_grid_is_cut() {
        let mut t = Table { cols: vec![10.0; 100_000], rows: vec![Row::default(); 100_000], ..Default::default() };
        t.normalize();
        assert_eq!(t.cols.len(), MAX_COLS);
        assert_eq!(t.rows.len() * MAX_COLS, MAX_CELLS);
    }

    #[test]
    fn merging_and_rows() {
        let mut t = Table::new(3, 3, 300.0);
        t.rows[0].cells[1].text = TextBody::plain("b");
        t.merge(0, 0, 1, 1);
        assert_eq!((t.rows[0].cells[0].span, t.rows[0].cells[0].rspan), (2, 2));
        assert!(t.rows[1].cells[1].covered() && t.rows[0].cells[1].h_merge);
        assert_eq!(t.rows[0].cells[0].text.text(), "\nb");
        t.unmerge(0, 0);
        assert!(!t.rows[1].cells[1].covered());
        t.insert_row(1, 0);
        t.insert_col(3, 2);
        assert_eq!((t.rows.len(), t.cols.len(), t.rows[1].cells.len()), (4, 4, 4));
        t.delete_col(0);
        t.delete_row(0);
        assert_eq!((t.rows.len(), t.cols.len()), (3, 3));
        let head = t.cell_look(0, 0);
        assert!(head.bold);
    }
}
