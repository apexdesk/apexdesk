//! Grouping of rows and columns (outline) and automatic subtotals.

use super::*;

pub const MAX_LEVEL: u8 = 7;

/// A run of consecutive rows (or columns) at `level` or deeper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub level: u8,
    pub start: u32,
    pub end: u32,
}

/// Settings of the Subtotals dialog.
#[derive(Clone, Debug)]
pub struct SubtotalOpts {
    /// Column whose changes start a new group.
    pub by: u32,
    /// SUBTOTAL function number (9 = sum).
    pub func: u8,
    pub cols: Vec<u32>,
    pub replace: bool,
    /// Label of a group's total; `{}` is the group value.
    pub total_label: String,
    pub grand_label: String,
}

impl Workbook {
    pub fn outline_levels(&self, sheet: usize, rows: bool) -> &BTreeMap<u32, u8> {
        let s = &self.sheets[sheet];
        if rows { &s.row_levels } else { &s.col_levels }
    }

    fn is_hidden(&self, sheet: usize, rows: bool, i: u32) -> bool {
        let s = &self.sheets[sheet];
        (if rows { &s.row_heights } else { &s.col_widths }).get(&i) == Some(&0.0)
    }

    /// Adds one outline level to `from..=to`.
    pub fn group(&mut self, sheet: usize, rows: bool, from: u32, to: u32) {
        let mut l = self.outline_levels(sheet, rows).clone();
        for i in from..=to {
            let e = l.entry(i).or_insert(0);
            *e = (*e + 1).min(MAX_LEVEL);
        }
        self.execute_as(Op::Outline { sheet, rows, levels: l }, "undo.group");
    }

    /// Removes one outline level from `from..=to`; false if none had one.
    pub fn ungroup(&mut self, sheet: usize, rows: bool, from: u32, to: u32) -> bool {
        let mut l = self.outline_levels(sheet, rows).clone();
        let mut changed = false;
        for i in from..=to {
            if let Some(v) = l.get_mut(&i) {
                *v -= 1;
                changed = true;
                if *v == 0 {
                    l.remove(&i);
                }
            }
        }
        if changed {
            self.execute_as(Op::Outline { sheet, rows, levels: l }, "undo.ungroup");
        }
        changed
    }

    /// Removes every group of the sheet and shows what they had collapsed.
    pub fn clear_outline(&mut self, sheet: usize) {
        let mut ops = Vec::new();
        for rows in [true, false] {
            let l = self.outline_levels(sheet, rows);
            if l.is_empty() {
                continue;
            }
            let sizes: Vec<(u32, Option<f32>)> = l.keys().filter(|&&i| self.is_hidden(sheet, rows, i)).map(|&i| (i, None)).collect();
            if !sizes.is_empty() {
                ops.push(if rows { Op::RowHeights { sheet, sizes } } else { Op::ColWidths { sheet, sizes } });
            }
            ops.push(Op::Outline { sheet, rows, levels: BTreeMap::new() });
        }
        if !ops.is_empty() {
            self.execute_as(Op::Batch(ops), "undo.ungroup");
        }
    }

    pub fn max_level(&self, sheet: usize, rows: bool) -> u8 {
        self.outline_levels(sheet, rows).values().copied().max().unwrap_or(0)
    }

    /// All groups, outer levels first within each position.
    pub fn outline_groups(&self, sheet: usize, rows: bool) -> Vec<Group> {
        let mut out = Vec::new();
        let mut open: [Option<(u32, u32)>; MAX_LEVEL as usize + 1] = [None; MAX_LEVEL as usize + 1];
        let close = |out: &mut Vec<Group>, level: usize, run: (u32, u32)| out.push(Group { level: level as u8, start: run.0, end: run.1 });
        for (&i, &lv) in self.outline_levels(sheet, rows) {
            for level in 1..=MAX_LEVEL as usize {
                let run = open[level];
                if level <= lv as usize {
                    open[level] = match run {
                        Some((s, last)) if last + 1 == i => Some((s, i)),
                        Some(r) => {
                            close(&mut out, level, r);
                            Some((i, i))
                        }
                        None => Some((i, i)),
                    };
                } else if let Some(r) = run {
                    close(&mut out, level, r);
                    open[level] = None;
                }
            }
        }
        for (level, run) in open.iter().enumerate() {
            if let Some(r) = run {
                close(&mut out, level, *r);
            }
        }
        out.sort_by_key(|g| (g.start, g.level));
        out
    }

    /// Whether every row (or column) of the group is hidden.
    pub fn is_collapsed(&self, sheet: usize, rows: bool, g: &Group) -> bool {
        let s = &self.sheets[sheet];
        let m = if rows { &s.row_heights } else { &s.col_widths };
        m.range(g.start..=g.end).filter(|(_, v)| **v == 0.0).count() as u32 == g.end - g.start + 1
    }

    /// Hides (collapse) or shows (expand) the rows of a group.
    pub fn set_collapsed(&mut self, sheet: usize, rows: bool, g: &Group, collapse: bool) {
        let sizes: Vec<(u32, Option<f32>)> = if collapse {
            (g.start..=g.end).map(|i| (i, Some(0.0))).collect()
        } else {
            (g.start..=g.end).filter(|&i| self.is_hidden(sheet, rows, i)).map(|i| (i, None)).collect()
        };
        if sizes.is_empty() {
            return;
        }
        let label = if collapse { "undo.collapse" } else { "undo.expand" };
        self.execute_as(if rows { Op::RowHeights { sheet, sizes } } else { Op::ColWidths { sheet, sizes } }, label);
    }

    /// Shows the outline down to `level`: deeper rows are hidden.
    pub fn show_level(&mut self, sheet: usize, rows: bool, level: u8) {
        let sizes: Vec<(u32, Option<f32>)> = self
            .outline_levels(sheet, rows)
            .iter()
            .filter_map(|(&i, &lv)| {
                let hide = lv >= level;
                (hide != self.is_hidden(sheet, rows, i)).then_some((i, hide.then_some(0.0)))
            })
            .collect();
        if !sizes.is_empty() {
            let label = if rows { "undo.hide_rows" } else { "undo.hide_cols" };
            self.execute_as(if rows { Op::RowHeights { sheet, sizes } } else { Op::ColWidths { sheet, sizes } }, label);
        }
    }

    /// Rows of `r` that hold a SUBTOTAL formula.
    fn subtotal_rows(&self, sheet: usize, r: &Range) -> Vec<u32> {
        let mut rows = Vec::new();
        self.sheets[sheet].for_each_in(r, |c, cell| {
            if cell.formula().is_some_and(|e| e.expr().calls(Func::Subtotal)) {
                rows.push(c.row);
            }
        });
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// Deletes the subtotal rows of `r` and its row outline; returns the
    /// remaining range and the number of undo steps taken.
    fn strip_subtotals(&mut self, sheet: usize, r: &Range) -> (Range, usize) {
        let rows = self.subtotal_rows(sheet, r);
        let mut steps = 0;
        let mut out = *r;
        let hidden: Vec<(u32, Option<f32>)> = (r.start.row..=r.end.row).filter(|&i| self.is_hidden(sheet, true, i)).map(|i| (i, None)).collect();
        if !hidden.is_empty() {
            self.execute_as(Op::RowHeights { sheet, sizes: hidden }, "undo.subtotals");
            steps += 1;
        }
        if !rows.is_empty() {
            self.execute_as(Op::DeleteRowsAt { sheet, rows: rows.clone() }, "undo.subtotals");
            out.end.row -= rows.len() as u32;
            steps += 1;
        }
        let mut l = self.sheets[sheet].row_levels.clone();
        let before = l.len();
        l.retain(|&i, _| i < out.start.row || i > out.end.row);
        if l.len() != before {
            self.execute_as(Op::Outline { sheet, rows: true, levels: l }, "undo.subtotals");
            steps += 1;
        }
        (out, steps)
    }

    /// Removes subtotals and the row outline from `r` (one undo step).
    pub fn remove_subtotals(&mut self, sheet: usize, r: &Range) -> Range {
        let (out, steps) = self.strip_subtotals(sheet, r);
        self.merge_undo(steps, "undo.subtotals");
        out
    }

    /// Inserts a SUBTOTAL row after each group of equal values in column
    /// `o.by` of `r` (first row = headers) plus a grand total, and outlines
    /// them. Returns the resulting range.
    pub fn subtotals(&mut self, sheet: usize, r: &Range, o: &SubtotalOpts) -> Range {
        let (r, mut steps) = if o.replace { self.strip_subtotals(sheet, r) } else { (*r, 0) };
        if r.rows() < 2 {
            self.merge_undo(steps, "undo.subtotals");
            return r;
        }
        let first = r.start.row + 1;
        // Groups as (first row, last row, label value).
        let mut groups: Vec<(u32, u32, String)> = Vec::new();
        for row in first..=r.end.row {
            let text = self.display(sheet, CellRef::new(row, o.by)).0;
            match groups.last_mut() {
                Some(g) if g.2.to_lowercase() == text.to_lowercase() => g.1 = row,
                _ => groups.push((row, row, text)),
            }
        }
        let n = groups.len() as u32;
        // A row after each group; the last group's and the grand total's
        // both go after the data.
        let mut at: Vec<u32> = groups.iter().map(|g| g.1 + 1).collect();
        at.push(r.end.row + 1);
        self.execute_as(Op::InsertRowsAt { sheet, at }, "undo.subtotals");
        steps += 1;
        let formula = |col: u32, a: u32, b: u32| {
            let rng = Range::new(CellRef::new(a, col), CellRef::new(b, col));
            let src = format!("SUBTOTAL({},{})", o.func, rng.to_a1());
            formula::parse_canonical(&src).ok().map(|e| Cell::new(Content::Formula { expr: e.into(), value: Value::Empty }, 0))
        };
        let text = |t: String| Some(Cell::new(Content::Value(Value::text(t)), 0));
        let mut cells = Vec::new();
        let mut levels = self.sheets[sheet].row_levels.clone();
        for (k, g) in groups.iter().enumerate() {
            let (a, b) = (g.0 + k as u32, g.1 + k as u32);
            let at = b + 1;
            cells.push((CellRef::new(at, o.by), text(o.total_label.replace("{}", &g.2))));
            for &c in o.cols.iter().filter(|&&c| c != o.by) {
                cells.push((CellRef::new(at, c), formula(c, a, b)));
            }
            for i in a..=b {
                levels.insert(i, 2);
            }
            levels.insert(at, 1);
        }
        let last = r.end.row + n;
        let grand = last + 1;
        cells.push((CellRef::new(grand, o.by), text(o.grand_label.clone())));
        for &c in o.cols.iter().filter(|&&c| c != o.by) {
            cells.push((CellRef::new(grand, c), formula(c, first, last)));
        }
        levels.remove(&grand);
        self.execute_as(Op::SetCells { sheet, cells }, "undo.subtotals");
        self.execute_as(Op::Outline { sheet, rows: true, levels }, "undo.subtotals");
        self.merge_undo(steps + 2, "undo.subtotals");
        Range::new(r.start, CellRef::new(grand, r.end.col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let data = [("Zone", "N"), ("a", "1"), ("a", "2"), ("b", "10"), ("c", "5"), ("c", "5")];
        let inputs = data.iter().enumerate().flat_map(|(i, (z, n))| [(CellRef::new(i as u32, 0), z.to_string()), (CellRef::new(i as u32, 1), n.to_string())]).collect();
        w.set_inputs(0, inputs);
        w
    }

    fn num(w: &Workbook, r: u32, c: u32) -> f64 {
        w.sheets[0].value(CellRef::new(r, c)).to_number().unwrap_or(f64::NAN)
    }

    #[test]
    fn groups_and_levels() {
        let mut w = Workbook::new();
        w.group(0, true, 1, 6);
        w.group(0, true, 2, 3);
        w.group(0, true, 5, 5);
        let g = w.outline_groups(0, true);
        assert_eq!(g, vec![Group { level: 1, start: 1, end: 6 }, Group { level: 2, start: 2, end: 3 }, Group { level: 2, start: 5, end: 5 }]);
        assert_eq!(w.max_level(0, true), 2);
        w.set_collapsed(0, true, &g[1], true);
        assert!(w.is_collapsed(0, true, &g[1]));
        assert!(!w.is_collapsed(0, true, &g[0]));
        w.show_level(0, true, 1);
        assert!(w.is_collapsed(0, true, &g[0]));
        w.show_level(0, true, 3);
        assert!(!w.is_collapsed(0, true, &g[1]));
        // Inserting rows moves the outline with them.
        w.execute(Op::InsertRows { sheet: 0, at: 0, count: 2 });
        assert_eq!(w.outline_groups(0, true)[0], Group { level: 1, start: 3, end: 8 });
        w.undo();
        assert!(w.ungroup(0, true, 1, 6));
        assert_eq!(w.max_level(0, true), 1);
        w.clear_outline(0);
        assert!(w.outline_groups(0, true).is_empty());
    }

    #[test]
    fn subtotal_function() {
        let mut w = sample();
        w.set_input(0, CellRef::new(7, 1), "=SUBTOTAL(9,B2:B6)");
        w.set_input(0, CellRef::new(8, 1), "=SUBTOTAL(109,B2:B8)");
        w.set_input(0, CellRef::new(9, 1), "=SUBTOTAL(1,B2:B6)");
        assert_eq!(num(&w, 7, 1), 23.0);
        // Nested subtotals are skipped.
        assert_eq!(num(&w, 8, 1), 23.0);
        assert_eq!(num(&w, 9, 1), 4.6);
        w.set_hidden(0, true, 3, 3, true);
        assert_eq!(num(&w, 7, 1), 23.0);
        assert_eq!(num(&w, 8, 1), 13.0);
        w.undo();
        assert_eq!(num(&w, 8, 1), 23.0);
    }

    fn snapshot(w: &Workbook) -> Vec<(CellRef, String, String)> {
        let mut v: Vec<(CellRef, String, String)> = w.sheets[0].iter().map(|(c, _)| (c, w.edit_text(0, c), w.display(0, c).0)).collect();
        v.sort_by_key(|x| (x.0.row, x.0.col));
        v
    }

    #[test]
    fn rows_at_many_points() {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let mut inputs: Vec<(CellRef, String)> = (0..10).map(|i| (CellRef::new(i, 0), (i + 1).to_string())).collect();
        inputs.push((CellRef::new(11, 1), "=SUM(A1:A10)".into()));
        inputs.push((CellRef::new(0, 2), "=A5+A7".into()));
        inputs.push((CellRef::new(1, 3), "=SUM(A2:A3)".into()));
        w.set_inputs(0, inputs);
        w.sheets[0].row_heights.insert(6, 30.0);
        w.sheets[0].row_levels.insert(3, 1);
        w.sheets[0].merges.push(Range::parse("E6:F9").unwrap());
        let orig = snapshot(&w);
        for (points, insert) in [(vec![2, 5, 5, 11], true), (vec![1, 2, 6], false)] {
            let mut one = w.clone();
            for &p in points.iter().rev() {
                one.execute(if insert { Op::InsertRows { sheet: 0, at: p, count: 1 } } else { Op::DeleteRows { sheet: 0, at: p, count: 1 } });
            }
            let op = if insert { Op::InsertRowsAt { sheet: 0, at: points.clone() } } else { Op::DeleteRowsAt { sheet: 0, rows: points.clone() } };
            w.execute(op);
            assert_eq!(snapshot(&w), snapshot(&one), "{points:?}");
            assert_eq!(w.sheets[0].row_heights.clone(), one.sheets[0].row_heights.clone());
            assert_eq!(w.sheets[0].row_levels, one.sheets[0].row_levels);
            assert_eq!(w.sheets[0].merges, one.sheets[0].merges);
            w.undo();
            assert_eq!(snapshot(&w), orig);
            assert_eq!(w.sheets[0].row_levels.get(&3), Some(&1));
            assert_eq!(w.sheets[0].row_height(6), 30.0);
            w.redo();
            assert_eq!(snapshot(&w), snapshot(&one));
            w.undo();
        }
    }

    #[test]
    fn automatic_subtotals() {
        let mut w = sample();
        let r = Range::new(CellRef::new(0, 0), CellRef::new(5, 1));
        let o = SubtotalOpts { by: 0, func: 9, cols: vec![1], replace: true, total_label: "{} Total".into(), grand_label: "Grand Total".into() };
        let out = w.subtotals(0, &r, &o);
        assert_eq!(out.end.row, 9);
        let col: Vec<String> = (0..=9).map(|i| w.display(0, CellRef::new(i, 0)).0).collect();
        assert_eq!(col, ["Zone", "a", "a", "a Total", "b", "b Total", "c", "c", "c Total", "Grand Total"]);
        assert_eq!(num(&w, 3, 1), 3.0);
        assert_eq!(num(&w, 5, 1), 10.0);
        assert_eq!(num(&w, 8, 1), 10.0);
        assert_eq!(num(&w, 9, 1), 23.0);
        assert_eq!(w.max_level(0, true), 2);
        w.show_level(0, true, 2);
        assert_eq!(w.sheets[0].row_height(1), 0.0);
        assert_eq!(num(&w, 9, 1), 23.0);
        // Running again replaces instead of stacking.
        let again = w.subtotals(0, &out, &o);
        assert_eq!(again.end.row, 9);
        assert_eq!(w.sheets[0].row_height(1), w.sheets[0].default_row_height);
        let back = w.remove_subtotals(0, &again);
        assert_eq!(back.end.row, 5);
        assert!(w.outline_groups(0, true).is_empty());
        w.undo();
        assert_eq!(w.display(0, CellRef::new(9, 0)).0, "Grand Total");
        w.undo();
        assert_eq!(w.display(0, CellRef::new(3, 0)).0, "a Total");
        w.undo(); // show_level
        w.undo();
        assert_eq!(w.display(0, CellRef::new(3, 0)).0, "b");
        assert!(w.outline_groups(0, true).is_empty());
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore]
    fn subtotals_speed() {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let mut inputs = vec![(CellRef::new(0, 0), "Zone".to_string()), (CellRef::new(0, 1), "N".to_string())];
        for i in 1..=30000u32 {
            inputs.push((CellRef::new(i, 0), format!("R{:04}", i / 30)));
            inputs.push((CellRef::new(i, 1), (i % 17).to_string()));
            inputs.push((CellRef::new(i, 2), "x".to_string()));
            inputs.push((CellRef::new(i, 3), "=B2*2".to_string()));
        }
        w.set_inputs(0, inputs);
        let r = Range::new(CellRef::new(0, 0), CellRef::new(30000, 3));
        let o = SubtotalOpts { by: 0, func: 9, cols: vec![1], replace: true, total_label: "{} Total".into(), grand_label: "Grand Total".into() };
        let t = std::time::Instant::now();
        let out = w.subtotals(0, &r, &o);
        eprintln!("subtotals: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        w.undo();
        eprintln!("undo: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        w.redo();
        eprintln!("redo: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        w.remove_subtotals(0, &out);
        eprintln!("remove: {:?}", t.elapsed());
    }
}
