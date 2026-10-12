//! Go To Special: finds cells of a kind (blanks, formulas, notes...).

use super::*;

/// Value types for constants and formulas in Go To Special.
pub const SPECIAL_NUMBERS: u8 = 1;
pub const SPECIAL_TEXT: u8 = 2;
pub const SPECIAL_LOGICAL: u8 = 4;
pub const SPECIAL_ERRORS: u8 = 8;
pub const SPECIAL_ALL_TYPES: u8 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Special {
    Blanks,
    /// Typed values of the given types (`SPECIAL_*` bits).
    Constants(u8),
    Formulas(u8),
    Notes,
    CurrentRegion,
    LastCell,
    Visible,
    /// Cells with conditional formats; `same` keeps only the active cell's rules.
    CondFormats { same: bool },
    Validation { same: bool },
    /// Cells that differ from the active cell's column in their row.
    RowDiffs,
    /// Cells that differ from the active cell's row in their column.
    ColDiffs,
}

fn type_bit(v: &Value) -> u8 {
    match v {
        Value::Number(_) => SPECIAL_NUMBERS,
        Value::Text(_) => SPECIAL_TEXT,
        Value::Bool(_) => SPECIAL_LOGICAL,
        Value::Error(_) => SPECIAL_ERRORS,
        _ => 0,
    }
}

/// Joins cells into as few rectangles as it can, in reading order.
pub fn cells_to_ranges(mut cells: Vec<CellRef>) -> Vec<Range> {
    cells.sort_by_key(|c| (c.col, c.row));
    cells.dedup();
    // Vertical runs per column, then runs with the same rows in adjacent columns.
    let mut runs: Vec<Range> = Vec::new();
    for c in cells {
        match runs.last_mut() {
            Some(r) if r.end.col == c.col && r.end.row + 1 == c.row => r.end.row = c.row,
            _ => runs.push(Range::single(c)),
        }
    }
    let mut open: std::collections::HashMap<(u32, u32), usize> = std::collections::HashMap::new();
    let mut out: Vec<Range> = Vec::new();
    for r in runs {
        let key = (r.start.row, r.end.row);
        match open.get(&key) {
            Some(&i) if out[i].end.col + 1 == r.start.col => out[i].end.col = r.start.col,
            _ => {
                open.insert(key, out.len());
                out.push(r);
            }
        }
    }
    out.sort_by_key(|r| (r.start.row, r.start.col));
    out
}

/// Consecutive visible indexes in `a..=b`, as `(from, to)` blocks.
fn visible_blocks(sizes: &crate::sheet::Sizes, a: u32, b: u32) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    let mut cur = a;
    // Only the hidden entries matter; walk them instead of every index.
    for (&i, _) in sizes.range(a..=b).filter(|(_, s)| **s == 0.0) {
        if i > cur {
            out.push((cur, i - 1));
        }
        cur = i + 1;
    }
    if cur <= b {
        out.push((cur, b));
    }
    out
}

impl Workbook {
    /// The cells of `kind` in `sel` (the used range when `sel` is one cell).
    /// `active` is the active cell. Empty when nothing matches.
    pub fn special_cells(&self, sheet: usize, sel: &Range, kind: Special, active: CellRef) -> Vec<Range> {
        let sh = &self.sheets[sheet];
        let Some(ext) = sh.extent() else {
            return match kind {
                Special::Visible if !sel.is_single() => self.special_cells_visible(sheet, sel),
                _ => Vec::new(),
            };
        };
        let used = Range::new(CellRef::new(0, 0), ext);
        let scope = if sel.is_single() { used } else { *sel };
        let within = scope.intersect(&used);
        let cells = |f: &dyn Fn(CellRef, &crate::sheet::Cell) -> bool| -> Vec<Range> {
            let Some(w) = within else { return Vec::new() };
            cells_to_ranges(sh.iter().filter(|(c, x)| w.contains(*c) && f(*c, x)).map(|(c, _)| c).collect())
        };
        match kind {
            Special::Blanks => {
                let Some(w) = within else { return Vec::new() };
                let mut out = Vec::new();
                for col in w.start.col..=w.end.col {
                    for row in w.start.row..=w.end.row {
                        let c = CellRef::new(row, col);
                        if sh.value(c).is_empty() && !self.merge_hides(sheet, c) {
                            out.push(c);
                        }
                    }
                }
                cells_to_ranges(out)
            }
            Special::Constants(t) => cells(&|_, x| matches!(x.content, Content::Value(_)) && type_bit(x.value()) & t != 0),
            Special::Formulas(t) => cells(&|_, x| x.formula().is_some() && type_bit(x.value()) & t != 0),
            Special::Notes => cells(&|_, x| x.note().is_some()),
            Special::CurrentRegion => vec![self.current_region(sheet, active)],
            Special::LastCell => vec![Range::single(ext)],
            Special::Visible => self.special_cells_visible(sheet, &scope),
            Special::CondFormats { same } => {
                let rules = sh.cond_formats.iter().filter(|r| !same || r.ranges.iter().any(|x| x.contains(active)));
                Self::clip_ranges(rules.flat_map(|r| r.ranges.iter()), &scope)
            }
            Special::Validation { same } => {
                let rules = sh.validations.iter().filter(|r| !same || r.ranges.iter().any(|x| x.contains(active)));
                Self::clip_ranges(rules.flat_map(|r| r.ranges.iter()), &scope)
            }
            Special::RowDiffs | Special::ColDiffs => {
                let Some(w) = within else { return Vec::new() };
                let rows = kind == Special::RowDiffs;
                let mut out = Vec::new();
                for row in w.start.row..=w.end.row {
                    for col in w.start.col..=w.end.col {
                        let c = CellRef::new(row, col);
                        let base = if rows { CellRef::new(row, active.col) } else { CellRef::new(active.row, col) };
                        if c != base && sh.value(c) != sh.value(base) {
                            out.push(c);
                        }
                    }
                }
                cells_to_ranges(out)
            }
        }
    }

    fn special_cells_visible(&self, sheet: usize, scope: &Range) -> Vec<Range> {
        let sh = &self.sheets[sheet];
        let rows = visible_blocks(&sh.row_heights, scope.start.row, scope.end.row);
        let cols = visible_blocks(&sh.col_widths, scope.start.col, scope.end.col);
        rows.iter()
            .flat_map(|&(r0, r1)| cols.iter().map(move |&(c0, c1)| Range::new(CellRef::new(r0, c0), CellRef::new(r1, c1))))
            .collect()
    }

    /// Cells of a merge other than its top-left one.
    fn merge_hides(&self, sheet: usize, c: CellRef) -> bool {
        self.sheets[sheet].merges.iter().any(|m| m.contains(c) && m.start != c)
    }

    fn clip_ranges<'a>(ranges: impl Iterator<Item = &'a Range>, scope: &Range) -> Vec<Range> {
        let mut cells = Vec::new();
        let mut out: Vec<Range> = Vec::new();
        for r in ranges.filter_map(|r| r.intersect(scope)) {
            // Small areas go through the cell merge so overlaps join up.
            if r.cell_count() <= 100_000 {
                for row in r.start.row..=r.end.row {
                    for col in r.start.col..=r.end.col {
                        cells.push(CellRef::new(row, col));
                    }
                }
            } else if !out.contains(&r) {
                out.push(r);
            }
        }
        out.extend(cells_to_ranges(cells));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(a: &str) -> CellRef {
        CellRef::parse(a).unwrap()
    }

    fn a1(v: &[Range]) -> Vec<String> {
        v.iter().map(Range::to_a1).collect()
    }

    #[test]
    fn finds_special_cells() {
        let mut w = Workbook::new();
        w.set_input(0, cell("A1"), "1");
        w.set_input(0, cell("B1"), "x");
        w.set_input(0, cell("A2"), "=A1*2");
        w.set_input(0, cell("B2"), "=1/0");
        w.set_input(0, cell("A3"), "1");
        w.set_input(0, cell("B3"), "2");
        let all = Range::single(cell("A1"));
        let at = cell("A1");
        assert_eq!(a1(&w.special_cells(0, &all, Special::Constants(SPECIAL_ALL_TYPES), at)), ["A1:B1", "A3:B3"]);
        assert_eq!(a1(&w.special_cells(0, &all, Special::Constants(SPECIAL_TEXT), at)), ["B1"]);
        assert_eq!(a1(&w.special_cells(0, &all, Special::Formulas(SPECIAL_ALL_TYPES), at)), ["A2:B2"]);
        assert_eq!(a1(&w.special_cells(0, &all, Special::Formulas(SPECIAL_ERRORS), at)), ["B2"]);
        assert_eq!(a1(&w.special_cells(0, &all, Special::LastCell, at)), ["B3"]);
        assert!(w.special_cells(0, &all, Special::Blanks, at).is_empty());
        // Blanks only count inside the used range.
        let wide = Range::new(cell("A1"), cell("C3"));
        assert!(w.special_cells(0, &wide, Special::Blanks, at).is_empty());
        w.set_input(0, cell("D4"), "z");
        assert_eq!(a1(&w.special_cells(0, &wide, Special::Blanks, at)), ["C1:C3"]);
        // B3 = 2 differs from A3 = 1 in its row; B1 "x" differs from 1.
        assert_eq!(a1(&w.special_cells(0, &Range::new(cell("A1"), cell("B3")), Special::RowDiffs, at)), ["B1:B3"]);
        assert_eq!(a1(&w.special_cells(0, &Range::new(cell("A1"), cell("B3")), Special::ColDiffs, at)), ["A2", "B2:B3"]);
    }

    #[test]
    fn visible_cells_skip_hidden_rows() {
        let mut w = Workbook::new();
        w.set_input(0, cell("A1"), "1");
        w.sheets[0].row_heights.insert(2, 0.0);
        w.sheets[0].col_widths.insert(1, 0.0);
        let r = Range::new(cell("A1"), cell("C5"));
        assert_eq!(a1(&w.special_cells(0, &r, Special::Visible, cell("A1"))), ["A1:A2", "C1:C2", "A4:A5", "C4:C5"]);
    }

    #[test]
    fn joins_cells_into_rectangles() {
        let cells = ["A1", "B1", "A2", "B2", "D2"].map(cell).to_vec();
        assert_eq!(a1(&cells_to_ranges(cells)), ["A1:B2", "D2"]);
    }
}
