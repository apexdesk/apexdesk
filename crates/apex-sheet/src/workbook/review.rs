//! Sheet review: warnings on cells that look like mistakes (a formula that
//! breaks its column's pattern, a number typed among formulas, a range
//! that stops short of its data, a total that doesn't add up), and the
//! workbook's tests: formulas saved with the file that must stay true,
//! checked on every recalculation, with proposals made from the patterns
//! found in a sheet.

use super::*;

/// A test of the workbook: `formula` (canonical text, without `=`) read
/// from sheet `sheet` must be true (or a number other than zero). An
/// empty name shows the formula.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Check {
    pub name: String,
    pub sheet: String,
    pub formula: String,
}

/// How a test came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    /// The formula can't be read or gives an error.
    Error,
}

/// A warning of the review on a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Review {
    /// The formula differs from the ones around it.
    Inconsistent,
    /// A number typed where the cells around it have the same formula.
    Constant,
    /// A range of the formula stops next to more numbers (the range that
    /// would take them in).
    ShortRange(Range),
    /// A total that isn't the sum of the numbers above it (that sum).
    BadTotal(f64),
}

/// Words that name a total in a row's label (lowercase).
const TOTAL_WORDS: [&str; 12] = ["total", "suma", "sum", "summe", "gesamt", "somme", "totale", "soma", "合計", "합계", "总计", "總計"];

fn is_total_label(s: &str) -> bool {
    let s = s.to_lowercase();
    // "Subtotal" rows are totals too, but of a part: they are left alone.
    !s.contains("sub") && TOTAL_WORDS.iter().any(|w| s.split(|c: char| !c.is_alphanumeric()).any(|t| t == *w) || (!w.is_ascii() && s.contains(w)))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

impl Workbook {
    fn review_number(&self, sheet: usize, c: CellRef) -> Option<f64> {
        match self.sheets[sheet].get(c)?.value() {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    fn typed_number(&self, sheet: usize, c: CellRef) -> bool {
        matches!(self.sheets[sheet].get(c).map(|x| &x.content), Some(Content::Value(Value::Number(_))))
    }

    /// The formula at `c` moved by (dr, dc), in canonical text.
    fn moved_formula(&self, sheet: usize, c: CellRef, dr: i64, dc: i64) -> Option<String> {
        let mut e = self.sheets[sheet].get(c)?.formula()?.expr().into_owned();
        e.offset(dr, dc);
        Some(e.to_canonical())
    }

    /// What the review warns about at `c`, if anything.
    pub fn review(&self, sheet: usize, c: CellRef) -> Option<Review> {
        let cell = self.sheets.get(sheet)?.get(c)?;
        if cell.formula().is_some() {
            if self.inconsistent_formula(sheet, c) {
                return Some(Review::Inconsistent);
            }
            if let Some(r) = self.short_range(sheet, c) {
                return Some(Review::ShortRange(r));
            }
        } else if self.typed_number(sheet, c) && self.constant_among_formulas(sheet, c) {
            return Some(Review::Constant);
        }
        self.bad_total(sheet, c).map(Review::BadTotal)
    }

    /// A number typed between two cells whose formulas agree (above and
    /// below, or left and right).
    fn constant_among_formulas(&self, sheet: usize, c: CellRef) -> bool {
        let (r, col) = (c.row as i64, c.col as i64);
        [((-1, 0), (1, 0)), ((0, -1), (0, 1))].into_iter().any(|((ar, ac), (br, bc))| {
            let (Some(a), Some(b)) = (at(r + ar, col + ac), at(r + br, col + bc)) else { return false };
            // Both moved onto `c`.
            match (self.moved_formula(sheet, a, -ar, -ac), self.moved_formula(sheet, b, -br, -bc)) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            }
        })
    }

    /// A one-column (or one-row) range of the formula at `c` with a number
    /// right past its end, or right before its start, that it leaves out.
    fn short_range(&self, sheet: usize, c: CellRef) -> Option<Range> {
        for (s, r) in self.precedents(sheet, c) {
            if s != sheet {
                continue;
            }
            let vertical = r.cols() == 1 && r.rows() >= 2;
            let horizontal = r.rows() == 1 && r.cols() >= 2;
            if !vertical && !horizontal {
                continue;
            }
            let (after, before) = if vertical {
                (at(r.end.row as i64 + 1, r.start.col as i64), at(r.start.row as i64 - 1, r.start.col as i64))
            } else {
                (at(r.start.row as i64, r.end.col as i64 + 1), at(r.start.row as i64, r.start.col as i64 - 1))
            };
            let left_out = |x: Option<CellRef>| x.filter(|x| *x != c && self.review_number(sheet, *x).is_some());
            if let Some(x) = left_out(after) {
                // Up to the last number of the run (and not past `c`).
                let mut end = x;
                loop {
                    let next = if vertical { at(end.row as i64 + 1, end.col as i64) } else { at(end.row as i64, end.col as i64 + 1) };
                    match left_out(next) {
                        Some(n) if !(vertical && n.col == c.col && n.row >= c.row) && !(horizontal && n.row == c.row && n.col >= c.col) => end = n,
                        _ => break,
                    }
                }
                if vertical && x.col == c.col && x.row > c.row || horizontal && x.row == c.row && x.col > c.col {
                    continue;
                }
                return Some(Range::new(r.start, end));
            }
            if let Some(x) = left_out(before) {
                let mut start = x;
                while let Some(n) = left_out(if vertical { at(start.row as i64 - 1, start.col as i64) } else { at(start.row as i64, start.col as i64 - 1) }) {
                    start = n;
                }
                return Some(Range::new(start, r.end));
            }
        }
        None
    }

    /// The numbers right above `c` (up to a blank or a text), when there
    /// are two or more.
    fn block_above(&self, sheet: usize, c: CellRef) -> Option<Range> {
        let mut top = c.row;
        while top > 0 && self.review_number(sheet, CellRef::new(top - 1, c.col)).is_some() {
            top -= 1;
        }
        (c.row - top >= 2).then(|| Range::new(CellRef::new(top, c.col), CellRef::new(c.row - 1, c.col)))
    }

    /// Whether row `row` is labelled as a total left of column `col`.
    fn total_row(&self, sheet: usize, row: u32, col: u32) -> bool {
        (0..col).rev().take(30).any(|x| match self.sheets[sheet].get(CellRef::new(row, x)).map(|v| v.value()) {
            Some(Value::Text(t)) => is_total_label(t),
            _ => false,
        })
    }

    /// The sum of the numbers above a total at `c` that doesn't match it.
    fn bad_total(&self, sheet: usize, c: CellRef) -> Option<f64> {
        let v = self.review_number(sheet, c)?;
        if !self.total_row(sheet, c.row, c.col) {
            return None;
        }
        let block = self.block_above(sheet, c)?;
        // Other totals inside the block: subtotals, not to add up.
        if (block.start.row..=block.end.row).any(|r| self.total_row(sheet, r, c.col)) {
            return None;
        }
        let sum: f64 = (block.start.row..=block.end.row).filter_map(|r| self.review_number(sheet, CellRef::new(r, c.col))).sum();
        (!close(sum, v)).then_some(sum)
    }

    // --- tests -------------------------------------------------------------------

    /// Runs one test.
    pub fn run_check(&self, check: &Check) -> Outcome {
        let sheet = self.sheets.iter().position(|s| s.name == check.sheet).unwrap_or(0);
        let Ok(e) = formula::parse(&check.formula, &Locale::default()) else { return Outcome::Error };
        if self.sheets.is_empty() {
            return Outcome::Error;
        }
        match Env::new(self, sheet, CellRef::default(), self.locale).eval(&e) {
            Value::Bool(true) => Outcome::Pass,
            Value::Bool(false) => Outcome::Fail,
            Value::Number(n) => if n != 0.0 { Outcome::Pass } else { Outcome::Fail },
            _ => Outcome::Error,
        }
    }

    /// The cells and ranges a test reads, as (sheet, range).
    pub fn precedent_ranges(&self, check: &Check) -> Vec<(usize, Range)> {
        let sheet = self.sheets.iter().position(|s| s.name == check.sheet).unwrap_or(0);
        let Ok(e) = formula::parse(&check.formula, &Locale::default()) else { return Vec::new() };
        if self.sheets.is_empty() {
            return Vec::new();
        }
        self.deps_of(sheet, CellRef::default(), &e).into_iter().map(|d| match d {
            Dep::Cell(k) => (k.sheet as usize, Range::new(k.cell(), k.cell())),
            Dep::Range(s, r) => (s as usize, r),
        }).collect()
    }

    /// Runs every test of the workbook.
    pub fn run_checks(&self) -> Vec<Outcome> {
        self.checks.iter().map(|c| self.run_check(c)).collect()
    }

    /// Tests worth having on `sheet`, from what it holds now: totals equal
    /// to the sum of what they total, and blocks of formulas without
    /// errors. Only those that pass now and aren't there yet.
    pub fn propose_checks(&self, sheet: usize) -> Vec<Check> {
        let Some(sh) = self.sheets.get(sheet) else { return Vec::new() };
        let name = sh.name.clone();
        let mut out: Vec<Check> = Vec::new();
        let mut cells: Vec<CellRef> = sh.iter().map(|(c, _)| c).collect();
        cells.sort_unstable_by_key(|c| (c.col, c.row));
        // Totals.
        for &c in &cells {
            if out.len() >= 20 {
                break;
            }
            if self.review_number(sheet, c).is_none() || !self.total_row(sheet, c.row, c.col) {
                continue;
            }
            let Some(block) = self.block_above(sheet, c) else { continue };
            let formula = format!("SUM({})={}", block.to_a1(), c.to_a1());
            out.push(Check { name: String::new(), sheet: name.clone(), formula });
        }
        // Columns of formulas: no errors in them.
        let mut col = None::<(u32, u32, u32)>;
        let mut runs = Vec::new();
        for &c in &cells {
            let f = sh.get(c).and_then(|x| x.formula()).is_some();
            match (&mut col, f) {
                (Some((cc, _, end)), true) if *cc == c.col && *end + 1 == c.row => *end = c.row,
                (_, true) => {
                    if let Some(x) = col.take() {
                        runs.push(x);
                    }
                    col = Some((c.col, c.row, c.row));
                }
                (_, false) => {
                    if let Some(x) = col.take() {
                        runs.push(x);
                    }
                }
            }
        }
        runs.extend(col);
        for (c, a, b) in runs {
            if b - a + 1 < 3 || out.len() >= 30 {
                continue;
            }
            let r = Range::new(CellRef::new(a, c), CellRef::new(b, c));
            out.push(Check { name: String::new(), sheet: name.clone(), formula: format!("NOT(ISERROR(SUM({})))", r.to_a1()) });
        }
        out.retain(|c| self.run_check(c) == Outcome::Pass && !self.checks.iter().any(|k| k.sheet == c.sheet && k.formula == c.formula));
        out
    }

    /// Replaces the tests, as one undo step.
    pub fn set_checks(&mut self, list: Vec<Check>) {
        self.execute_as(Op::Checks(list), "undo.checks");
    }
}

fn at(r: i64, c: i64) -> Option<CellRef> {
    (r >= 0 && c >= 0 && r < MAX_ROWS as i64 && c < MAX_COLS as i64).then(|| CellRef::new(r as u32, c as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(cells: &[(&str, &str)]) -> Workbook {
        let mut wb = Workbook::empty();
        wb.sheets.push(Sheet::new("S"));
        for (a, v) in cells {
            wb.set_input(0, CellRef::parse(a).unwrap(), v);
        }
        wb
    }

    fn c(a: &str) -> CellRef {
        CellRef::parse(a).unwrap()
    }

    #[test]
    fn warns_about_patterns() {
        let wb = book(&[
            ("A1", "Item"), ("B1", "Qty"), ("C1", "Price"), ("D1", "Amount"),
            ("A2", "a"), ("B2", "2"), ("C2", "10"), ("D2", "=B2*C2"),
            ("A3", "b"), ("B3", "3"), ("C3", "10"), ("D3", "30"),
            ("A4", "c"), ("B4", "4"), ("C4", "10"), ("D4", "=B4*C4"),
            ("A5", "d"), ("B5", "1"), ("C5", "5"), ("D5", "=B5*C5"),
            ("A6", "Total"), ("B6", "=SUM(B2:B4)"), ("C6", "99"), ("D6", "=SUM(D2:D5)"),
        ]);
        assert_eq!(wb.review(0, c("D3")), Some(Review::Constant));
        assert_eq!(wb.review(0, c("D2")), None);
        assert_eq!(wb.review(0, c("B6")), Some(Review::ShortRange(Range::new(c("B2"), c("B5")))));
        assert_eq!(wb.review(0, c("C6")), Some(Review::BadTotal(35.0)));
        assert_eq!(wb.review(0, c("D6")), None);
        assert!(is_total_label("Grand Total") && is_total_label("合計") && !is_total_label("Subtotal") && !is_total_label("Totally"));
    }

    #[test]
    fn tests_run_and_are_proposed() {
        let mut wb = book(&[("A1", "x"), ("B1", "=1+1"), ("B2", "=2+2"), ("B3", "=3+3"), ("A4", "Total"), ("B4", "=SUM(B1:B3)")]);
        let props = wb.propose_checks(0);
        assert!(props.iter().any(|p| p.formula == "SUM(B1:B3)=B4"), "{props:?}");
        assert!(props.iter().any(|p| p.formula == "NOT(ISERROR(SUM(B1:B4)))"), "{props:?}");
        wb.set_checks(props.clone());
        assert!(wb.run_checks().iter().all(|o| *o == Outcome::Pass));
        assert!(wb.propose_checks(0).is_empty(), "no proposal twice");
        wb.set_input(0, c("B4"), "13");
        assert_eq!(wb.run_check(&props[0]), Outcome::Fail);
        wb.undo();
        assert_eq!(wb.run_check(&props[0]), Outcome::Pass);
        let n = wb.checks.len();
        wb.set_checks(Vec::new());
        wb.undo();
        assert_eq!(wb.checks.len(), n);
        assert_eq!(wb.run_check(&Check { name: "bad".into(), sheet: "S".into(), formula: "SUM(".into() }), Outcome::Error);
    }
}
