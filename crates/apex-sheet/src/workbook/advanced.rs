//! Advanced Filter (a criteria range) and Consolidate.

use super::*;
use crate::functions::Criteria;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvancedFilterError {
    /// The list needs a header row and at least one data row.
    List,
    /// The criteria range needs a header row.
    Criteria,
}

/// How Consolidate combines the values of each position or label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConsolidateFn {
    #[default]
    Sum,
    /// Non-empty values.
    Count,
    Average,
    Max,
    Min,
    Product,
    /// Numbers only.
    CountNums,
    StdDev,
    StdDevP,
    Var,
    VarP,
}

impl ConsolidateFn {
    pub const ALL: [ConsolidateFn; 11] = [
        ConsolidateFn::Sum,
        ConsolidateFn::Count,
        ConsolidateFn::Average,
        ConsolidateFn::Max,
        ConsolidateFn::Min,
        ConsolidateFn::Product,
        ConsolidateFn::CountNums,
        ConsolidateFn::StdDev,
        ConsolidateFn::StdDevP,
        ConsolidateFn::Var,
        ConsolidateFn::VarP,
    ];

    fn apply(self, nums: &[f64], count: usize) -> Value {
        let n = nums.len() as f64;
        let mean = nums.iter().sum::<f64>() / n;
        let ss = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>();
        let div0 = Value::Error(ErrorKind::Div0);
        match self {
            ConsolidateFn::Sum => Value::Number(nums.iter().sum()),
            ConsolidateFn::Count => Value::Number(count as f64),
            ConsolidateFn::CountNums => Value::Number(n),
            ConsolidateFn::Average if nums.is_empty() => div0,
            ConsolidateFn::Average => Value::Number(mean),
            ConsolidateFn::Max => Value::Number(nums.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(if nums.is_empty() { 0.0 } else { f64::NEG_INFINITY })),
            ConsolidateFn::Min => Value::Number(nums.iter().copied().fold(f64::INFINITY, f64::min).min(if nums.is_empty() { 0.0 } else { f64::INFINITY })),
            ConsolidateFn::Product => Value::Number(nums.iter().product()),
            ConsolidateFn::StdDev | ConsolidateFn::Var if nums.len() < 2 => div0,
            ConsolidateFn::StdDevP | ConsolidateFn::VarP if nums.is_empty() => div0,
            ConsolidateFn::StdDev => Value::Number((ss / (n - 1.0)).sqrt()),
            ConsolidateFn::Var => Value::Number(ss / (n - 1.0)),
            ConsolidateFn::StdDevP => Value::Number((ss / n).sqrt()),
            ConsolidateFn::VarP => Value::Number(ss / n),
        }
    }
}

/// One test of a criteria row.
enum Test {
    Col(u32, Criteria),
    /// A formula written for the first data row; it moves down row by row.
    Formula(CellRef, Arc<Expr>),
}

impl Workbook {
    /// Data rows of `list` (below its header row) that pass `criteria`: a
    /// header row naming columns of the list and rows of conditions (the
    /// conditions of a row must all hold; any row may). Text conditions
    /// without an operator match the start of the text, like Excel.
    pub fn advanced_filter_rows(&self, sheet: usize, list: &Range, crit_sheet: usize, criteria: &Range, unique: bool) -> Result<Vec<u32>, AdvancedFilterError> {
        if list.rows() < 2 {
            return Err(AdvancedFilterError::List);
        }
        if criteria.rows() < 1 {
            return Err(AdvancedFilterError::Criteria);
        }
        let sh = &self.sheets[sheet];
        let cs = &self.sheets[crit_sheet];
        let headers: Vec<String> = (list.start.col..=list.end.col).map(|c| self.display(sheet, CellRef::new(list.start.row, c)).0.trim().to_lowercase()).collect();
        let mut rules: Vec<Vec<Test>> = Vec::new();
        for row in criteria.start.row + 1..=criteria.end.row {
            let mut tests = Vec::new();
            for col in criteria.start.col..=criteria.end.col {
                let at = CellRef::new(row, col);
                let Some(cell) = cs.get(at) else { continue };
                let head = self.display(crit_sheet, CellRef::new(criteria.start.row, col)).0.trim().to_lowercase();
                let target = headers.iter().position(|h| !h.is_empty() && *h == head);
                match (cell.formula(), target) {
                    // A formula under a label that is not a column: computed criteria.
                    (Some(e), None) => tests.push(Test::Formula(at, e.to_arc())),
                    (_, Some(i)) => {
                        let v = cell.value();
                        if v.is_empty() {
                            continue;
                        }
                        let crit = match v {
                            Value::Text(t) if !t.starts_with(['<', '>', '=']) && !t.ends_with('*') => Criteria::new(Value::text(format!("{t}*")), &self.locale),
                            v => Criteria::new(v.clone(), &self.locale),
                        };
                        tests.push(Test::Col(list.start.col + i as u32, crit));
                    }
                    _ => {}
                }
            }
            rules.push(tests);
        }
        // A criteria range with only headers (or blank rows) lets everything through.
        let all = rules.is_empty() || rules.iter().any(|t| t.is_empty());
        let first = list.start.row + 1;
        let passes = |row: u32| -> bool {
            all || rules.iter().any(|tests| {
                tests.iter().all(|t| match t {
                    Test::Col(col, c) => c.matches(sh.value(CellRef::new(row, *col))),
                    Test::Formula(at, e) => {
                        let mut e = (**e).clone();
                        e.offset(row as i64 - first as i64, 0);
                        let env = Env::new(self, crit_sheet, *at, self.locale);
                        matches!(env.eval(&e), Value::Bool(true)) || matches!(env.eval(&e), Value::Number(n) if n != 0.0)
                    }
                })
            })
        };
        let mut seen = std::collections::HashSet::new();
        Ok((first..=list.end.row)
            .filter(|&row| passes(row))
            .filter(|&row| !unique || seen.insert((list.start.col..=list.end.col).map(|c| self.display(sheet, CellRef::new(row, c)).0.to_lowercase()).collect::<Vec<_>>()))
            .collect())
    }

    /// Advanced Filter: hides the rows of `list` that fail `criteria`, or
    /// copies the header and the rows that pass to `copy_to` (values and
    /// formats). Returns how many rows pass.
    pub fn advanced_filter(&mut self, sheet: usize, list: &Range, crit_sheet: usize, criteria: &Range, unique: bool, copy_to: Option<(usize, CellRef)>) -> Result<usize, AdvancedFilterError> {
        let rows = self.advanced_filter_rows(sheet, list, crit_sheet, criteria, unique)?;
        let n = rows.len();
        match copy_to {
            None => {
                let keep: std::collections::HashSet<u32> = rows.into_iter().collect();
                let sizes: Vec<(u32, Option<f32>)> = (list.start.row + 1..=list.end.row)
                    .map(|r| (r, if keep.contains(&r) { self.sheets[sheet].row_heights.get(&r).copied().filter(|h| *h > 0.0) } else { Some(0.0) }))
                    .collect();
                self.execute_as(Op::RowHeights { sheet, sizes }, "undo.filter");
            }
            Some((to_sheet, at)) => {
                let src = &self.sheets[sheet];
                let mut cells = Vec::new();
                for (i, row) in std::iter::once(list.start.row).chain(rows).enumerate() {
                    for (j, col) in (list.start.col..=list.end.col).enumerate() {
                        let dst = CellRef::new(at.row + i as u32, at.col + j as u32);
                        if dst.row >= MAX_ROWS || dst.col >= MAX_COLS {
                            continue;
                        }
                        let cell = src.get(CellRef::new(row, col)).map(|c| {
                            let content = match c.value() {
                                Value::Empty => Content::Empty,
                                v => Content::Value(v.clone()),
                            };
                            Cell { content, style: c.style, extra: None }
                        });
                        cells.push((dst, cell));
                    }
                }
                self.execute_as(Op::SetCells { sheet: to_sheet, cells }, "undo.filter");
            }
        }
        Ok(n)
    }

    /// Consolidate: combines `refs` into a table at `at` of `sheet`. With
    /// labels, rows (left column) and columns (top row) are matched by
    /// their label; otherwise by position. Writes values; returns the area.
    pub fn consolidate(&mut self, sheet: usize, at: CellRef, refs: &[(usize, Range)], f: ConsolidateFn, top: bool, left: bool) -> Option<Range> {
        if refs.is_empty() {
            return None;
        }
        let label = |si: usize, c: CellRef| self.display(si, c).0.trim().to_string();
        let mut row_keys: Vec<String> = Vec::new();
        let mut col_keys: Vec<String> = Vec::new();
        let add = |keys: &mut Vec<String>, k: String| {
            if !keys.iter().any(|x| x.eq_ignore_ascii_case(&k) || x.to_lowercase() == k.to_lowercase()) {
                keys.push(k);
            }
        };
        let (mut rows, mut cols) = (0u32, 0u32);
        for (si, r) in refs {
            let (dr, dc) = (u32::from(top), u32::from(left));
            rows = rows.max(r.rows().saturating_sub(dr));
            cols = cols.max(r.cols().saturating_sub(dc));
            if left {
                for row in r.start.row + dr..=r.end.row {
                    add(&mut row_keys, label(*si, CellRef::new(row, r.start.col)));
                }
            }
            if top {
                for col in r.start.col + dc..=r.end.col {
                    add(&mut col_keys, label(*si, CellRef::new(r.start.row, col)));
                }
            }
        }
        if left {
            rows = row_keys.len() as u32;
        }
        if top {
            cols = col_keys.len() as u32;
        }
        if rows == 0 || cols == 0 || rows as u64 * cols as u64 > 1_000_000 {
            return None;
        }
        let find = |keys: &[String], k: &str| keys.iter().position(|x| x.to_lowercase() == k.to_lowercase());
        let mut acc: HashMap<(u32, u32), (Vec<f64>, usize)> = HashMap::default();
        for (si, r) in refs {
            let sh = &self.sheets[*si];
            let (dr, dc) = (u32::from(top), u32::from(left));
            for row in r.start.row + dr..=r.end.row {
                let i = if left { find(&row_keys, &label(*si, CellRef::new(row, r.start.col))).map(|x| x as u32) } else { Some(row - r.start.row - dr) };
                for col in r.start.col + dc..=r.end.col {
                    let j = if top { find(&col_keys, &label(*si, CellRef::new(r.start.row, col))).map(|x| x as u32) } else { Some(col - r.start.col - dc) };
                    let (Some(i), Some(j)) = (i, j) else { continue };
                    let v = sh.value(CellRef::new(row, col));
                    let e = acc.entry((i, j)).or_default();
                    if let Value::Number(n) = v {
                        e.0.push(*n);
                    }
                    if !v.is_empty() {
                        e.1 += 1;
                    }
                }
            }
        }
        let (or, oc) = (u32::from(top), u32::from(left));
        let mut cells = Vec::new();
        let put = |cells: &mut Vec<(CellRef, Option<Cell>)>, c: CellRef, v: Value, wb: &Workbook| {
            if c.row < MAX_ROWS && c.col < MAX_COLS {
                let content = if v.is_empty() { Content::Empty } else { Content::Value(v) };
                cells.push((c, Cell::with_content(wb.sheets[sheet].get(c), content)));
            }
        };
        for (j, k) in col_keys.iter().enumerate() {
            put(&mut cells, CellRef::new(at.row, at.col + oc + j as u32), Value::text(k.clone()), self);
        }
        for (i, k) in row_keys.iter().enumerate() {
            put(&mut cells, CellRef::new(at.row + or + i as u32, at.col), Value::text(k.clone()), self);
        }
        for i in 0..rows {
            for j in 0..cols {
                let v = acc.get(&(i, j)).map_or(Value::Empty, |(nums, count)| if *count == 0 { Value::Empty } else { f.apply(nums, *count) });
                put(&mut cells, CellRef::new(at.row + or + i, at.col + oc + j), v, self);
            }
        }
        self.execute_as(Op::SetCells { sheet, cells }, "undo.consolidate");
        Some(Range::new(at, CellRef::new((at.row + or + rows - 1).min(MAX_ROWS - 1), (at.col + oc + cols - 1).min(MAX_COLS - 1))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(a: &str) -> CellRef {
        CellRef::parse(a).unwrap()
    }

    fn fill(w: &mut Workbook, sheet: usize, at: &str, rows: &[&[&str]]) {
        let at = c(at);
        for (i, r) in rows.iter().enumerate() {
            for (j, t) in r.iter().enumerate() {
                w.set_input(sheet, CellRef::new(at.row + i as u32, at.col + j as u32), t);
            }
        }
    }

    #[test]
    fn advanced_filter() {
        let mut w = Workbook::new();
        fill(&mut w, 0, "A1", &[&["Name", "Region", "Sales"], &["Ann", "North", "10"], &["Bob", "South", "30"], &["Ana", "North", "50"], &["Bob", "South", "30"]]);
        let list = Range::parse("A1:C5").unwrap();
        // Region North and sales > 20, or name starting with "B".
        fill(&mut w, 0, "E1", &[&["Region", "Sales", "Name"], &["North", ">20", ""], &["", "", "B"]]);
        let crit = Range::parse("E1:G3").unwrap();
        assert_eq!(w.advanced_filter_rows(0, &list, 0, &crit, false).unwrap(), [2, 3, 4]);
        assert_eq!(w.advanced_filter_rows(0, &list, 0, &crit, true).unwrap(), [2, 3]);
        // Computed criteria: a formula under a label that is not a column.
        fill(&mut w, 0, "I1", &[&["Big"], &["=C2>=30"]]);
        assert_eq!(w.advanced_filter_rows(0, &list, 0, &Range::parse("I1:I2").unwrap(), false).unwrap(), [2, 3, 4]);
        assert_eq!(w.advanced_filter(0, &list, 0, &crit, true, Some((0, c("A10")))), Ok(2));
        assert_eq!(w.display(0, c("A10")).0, "Name");
        assert_eq!(w.display(0, c("A12")).0, "Ana");
        w.advanced_filter(0, &list, 0, &crit, false, None).unwrap();
        assert_eq!(w.sheets[0].row_height(1), 0.0);
        assert!(w.sheets[0].row_height(2) > 0.0);
        assert_eq!(w.advanced_filter_rows(0, &Range::parse("A1:C1").unwrap(), 0, &crit, false), Err(AdvancedFilterError::List));
    }

    #[test]
    fn consolidate_by_labels_and_position() {
        let mut w = Workbook::new();
        fill(&mut w, 0, "A1", &[&["", "Q1", "Q2"], &["Pens", "1", "2"], &["Ink", "3", "4"]]);
        fill(&mut w, 0, "E1", &[&["", "Q2", "Q3"], &["Ink", "10", "20"], &["Paper", "5", "6"]]);
        let refs = [(0, Range::parse("A1:C3").unwrap()), (0, Range::parse("E1:G3").unwrap())];
        let r = w.consolidate(0, c("A10"), &refs, ConsolidateFn::Sum, true, true).unwrap();
        assert_eq!(r.to_a1(), "A10:D13");
        let row = |w: &Workbook, n: u32| (0..4).map(|i| w.display(0, CellRef::new(n, i)).0).collect::<Vec<_>>();
        assert_eq!(row(&w, 9), ["", "Q1", "Q2", "Q3"]);
        assert_eq!(row(&w, 10), ["Pens", "1", "2", ""]);
        assert_eq!(row(&w, 11), ["Ink", "3", "14", "20"]);
        assert_eq!(row(&w, 12), ["Paper", "", "5", "6"]);
        // By position, the average of B2:C3 and F2:G3.
        let refs = [(0, Range::parse("B2:C3").unwrap()), (0, Range::parse("F2:G3").unwrap())];
        w.consolidate(0, c("A20"), &refs, ConsolidateFn::Average, false, false).unwrap();
        assert_eq!(w.display(0, c("B21")).0, "5");
    }
}
