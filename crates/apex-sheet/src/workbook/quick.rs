//! Quick Analysis > Totals: sums, averages, counts, % of total and running
//! totals in the row below or the column right of a block of data.

use super::*;
use crate::addr::col_name;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickTotal {
    Sum,
    Average,
    Count,
    Percent,
    Running,
}

/// `$B$2` when `abs`, else `B2`.
fn a1(c: CellRef, abs: bool) -> String {
    let d = if abs { "$" } else { "" };
    format!("{d}{}{d}{}", col_name(c.col), c.row + 1)
}

impl Workbook {
    /// The numbers of `r` without its header row and label column:
    /// (first data row, first data column), None when there are none.
    fn quick_body(&self, sheet: usize, r: &Range) -> Option<(u32, u32)> {
        let sh = &self.sheets[sheet];
        let texty = |c: CellRef| matches!(sh.value(c), Value::Text(_) | Value::Empty);
        let header = r.rows() > 1 && (r.start.col..=r.end.col).all(|col| texty(CellRef::new(r.start.row, col))) && (r.start.col..=r.end.col).any(|col| !sh.value(CellRef::new(r.start.row, col)).is_empty());
        let r0 = r.start.row + header as u32;
        let labels = r.cols() > 1 && (r0..=r.end.row).all(|row| texty(CellRef::new(row, r.start.col)));
        let c0 = r.start.col + labels as u32;
        let any = (r0..=r.end.row).any(|row| (c0..=r.end.col).any(|col| matches!(sh.value(CellRef::new(row, col)), Value::Number(_))));
        any.then_some((r0, c0))
    }

    /// Adds totals of `kind` to the row below `r` (or the column right of
    /// it), in bold, with `label` beside them. False when there are no
    /// numbers or those cells aren't empty.
    pub fn quick_totals(&mut self, sheet: usize, r: &Range, kind: QuickTotal, below: bool, label: &str) -> bool {
        let Some((r0, c0)) = self.quick_body(sheet, r) else { return false };
        let line = if below {
            if r.end.row + 1 >= MAX_ROWS {
                return false;
            }
            Range::new(CellRef::new(r.end.row + 1, r.start.col), CellRef::new(r.end.row + 1, r.end.col))
        } else {
            if r.end.col + 1 >= MAX_COLS {
                return false;
            }
            Range::new(CellRef::new(r.start.row, r.end.col + 1), CellRef::new(r.end.row, r.end.col + 1))
        };
        let sh = &self.sheets[sheet];
        let mut blocked = false;
        sh.for_each_in(&line, |_, cell| blocked |= !cell.value().is_empty());
        if blocked {
            return false;
        }
        let num = |c: CellRef| matches!(sh.value(c), Value::Number(_));
        let (start, end) = (CellRef::new(r0, c0), r.end);
        let all = format!("{}:{}", a1(start, true), a1(end, true));
        let mut inputs = Vec::new();
        // One total per column (below) or row (right) holding numbers.
        let lines: Vec<(u32, CellRef, CellRef, CellRef)> = if below {
            (c0..=r.end.col).filter(|&col| (r0..=r.end.row).any(|row| num(CellRef::new(row, col)))).map(|col| (col, CellRef::new(r0, col), CellRef::new(r.end.row, col), CellRef::new(line.start.row, col))).collect()
        } else {
            (r0..=r.end.row).filter(|&row| (c0..=r.end.col).any(|col| num(CellRef::new(row, col)))).map(|row| (row, CellRef::new(row, c0), CellRef::new(row, r.end.col), CellRef::new(row, line.start.col))).collect()
        };
        for (_, a, b, at) in lines {
            let span = format!("{}:{}", a1(a, false), a1(b, false));
            let f = match kind {
                QuickTotal::Sum => format!("=SUM({span})"),
                QuickTotal::Average => format!("=AVERAGE({span})"),
                QuickTotal::Count => format!("=COUNTA({span})"),
                QuickTotal::Percent => format!("=SUM({span})/SUM({all})"),
                QuickTotal::Running => format!("=SUM({}:{})", a1(start, true), a1(b, false)),
            };
            inputs.push((at, f));
        }
        // The label: in the label column below, in the header row right.
        let label_at = if below { (c0 > r.start.col).then_some(CellRef::new(line.start.row, r.start.col)) } else { (r0 > r.start.row).then_some(CellRef::new(r.start.row, line.start.col)) };
        if let Some(at) = label_at {
            inputs.push((at, label.to_string()));
        }
        let before = self.undo_len();
        // The formulas are A1 whatever the reference style.
        let r1c1 = std::mem::take(&mut self.calc.r1c1);
        self.set_inputs(sheet, inputs);
        self.calc.r1c1 = r1c1;
        let pct = kind == QuickTotal::Percent;
        self.modify_style(sheet, &line, |s| {
            s.bold = true;
            if pct {
                s.num_fmt = "0.00%".into();
            }
        });
        if pct {
            if let Some(at) = label_at {
                self.modify_style(sheet, &Range::single(at), |s| s.num_fmt = "General".into());
            }
        }
        self.merge_undo(self.undo_len() - before, "undo.quick_totals");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Workbook {
        let mut wb = Workbook::new();
        let rows = [["", "Norte", "Sur"], ["Ene", "10", "20"], ["Feb", "30", "40"]];
        for (i, row) in rows.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                if !v.is_empty() {
                    wb.set_input(0, CellRef::new(i as u32, j as u32), v);
                }
            }
        }
        wb
    }

    fn val(wb: &Workbook, a: &str) -> Value {
        wb.sheets[0].value(CellRef::parse(a).unwrap()).clone()
    }

    #[test]
    fn totals_below_and_right() {
        let mut wb = book();
        let r = Range::parse("A1:C3").unwrap();
        let n = wb.undo_len();
        assert!(wb.quick_totals(0, &r, QuickTotal::Sum, true, "Suma"));
        assert_eq!(wb.undo_len(), n + 1);
        assert_eq!(val(&wb, "B4"), Value::Number(40.0));
        assert_eq!(val(&wb, "C4"), Value::Number(60.0));
        assert_eq!(val(&wb, "A4"), Value::text("Suma"));
        assert!(wb.styles.get(wb.sheets[0].get(CellRef::parse("B4").unwrap()).unwrap().style).bold);
        // The row below is taken now.
        assert!(!wb.quick_totals(0, &r, QuickTotal::Average, true, "x"));
        assert!(wb.quick_totals(0, &r, QuickTotal::Percent, false, "% Total"));
        assert_eq!(val(&wb, "D2"), Value::Number(0.3));
        assert_eq!(val(&wb, "D1"), Value::text("% Total"));
        wb.undo();
        assert!(wb.quick_totals(0, &r, QuickTotal::Running, false, "Total"));
        assert_eq!(val(&wb, "D2"), Value::Number(30.0));
        assert_eq!(val(&wb, "D3"), Value::Number(100.0));
        wb.undo();
        wb.undo();
        assert_eq!(val(&wb, "B4"), Value::Empty);
    }
}
