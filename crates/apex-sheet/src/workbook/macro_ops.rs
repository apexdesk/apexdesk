//! Edits made by a macro: applied one by one (the macro reads what it
//! wrote) and undone together as one step, and the cells the last step
//! changed (the Target of the Worksheet_Change event).

use super::*;

impl Workbook {
    /// Applies `op` and recalculates like [`Self::execute`], without an undo
    /// step of its own: returns its inverse, or `None` when the workbook is
    /// read-only or protection refuses it.
    pub fn apply_op(&mut self, op: Op) -> Option<Op> {
        if self.read_only || self.enforce_protection && !self.allowed(&op) {
            self.blocked = true;
            return None;
        }
        let inv = self.apply(op);
        self.commit();
        Some(inv)
    }

    /// Runs `f` (an edit that makes its own undo steps) and gives the
    /// inverses of what it did, in the order done; the undo history stays
    /// as it was.
    pub fn steps_of<T>(&mut self, f: impl FnOnce(&mut Workbook) -> T) -> (T, Vec<Op>) {
        let undo = std::mem::take(&mut self.undo);
        let redo = std::mem::take(&mut self.redo);
        let t = f(self);
        let steps = std::mem::replace(&mut self.undo, undo).into_iter().map(|(op, _)| op).collect();
        self.redo = redo;
        (t, steps)
    }

    /// Records the inverses [`Self::apply_op`] returned, in the order the
    /// ops were applied, as one undo step named `label`.
    pub fn push_undo(&mut self, mut inverses: Vec<Op>, label: &'static str) {
        if inverses.is_empty() {
            return;
        }
        inverses.reverse();
        let op = if inverses.len() == 1 { inverses.pop().unwrap_or(Op::Batch(Vec::new())) } else { Op::Batch(inverses) };
        self.undo.push((op, label));
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Number of steps that can be redone.
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// The sheet and the area of the cells the last undo step restores
    /// (what the last edit changed), when it changed cells.
    pub fn last_edit(&self) -> Option<(usize, Range)> {
        fn walk(op: &Op, out: &mut Option<(usize, Range)>) {
            match op {
                Op::SetCells { sheet, cells } => {
                    for (c, _) in cells {
                        match out {
                            Some((s, r)) if *s == *sheet => *r = Range::new(CellRef::new(r.start.row.min(c.row), r.start.col.min(c.col)), CellRef::new(r.end.row.max(c.row), r.end.col.max(c.col))),
                            Some(_) => {}
                            None => *out = Some((*sheet, Range::single(*c))),
                        }
                    }
                }
                Op::Batch(ops) => ops.iter().for_each(|o| walk(o, out)),
                _ => {}
            }
        }
        let mut out = None;
        walk(&self.undo.last()?.0, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_steps_undo_together() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "1");
        let a = |wb: &Workbook, r, c| wb.sheets[0].value(CellRef::new(r, c)).clone();
        let set = |r, v: f64| Op::SetCells { sheet: 0, cells: vec![(CellRef::new(r, 1), Some(Cell::new(Content::Value(Value::Number(v)), 0)))] };
        let mut inv = Vec::new();
        inv.extend(wb.apply_op(set(0, 5.0)));
        inv.extend(wb.apply_op(set(3, 6.0)));
        assert_eq!(a(&wb, 3, 1), Value::Number(6.0));
        let n = wb.undo_len();
        wb.push_undo(inv, "undo.edit");
        assert_eq!(wb.undo_len(), n + 1);
        assert_eq!(wb.last_edit(), Some((0, Range::new(CellRef::new(0, 1), CellRef::new(3, 1)))));
        assert!(wb.undo());
        assert_eq!(a(&wb, 0, 1), Value::Empty);
        assert_eq!(a(&wb, 3, 1), Value::Empty);
        assert_eq!(a(&wb, 0, 0), Value::Number(1.0));
        wb.read_only = true;
        assert!(wb.apply_op(set(0, 1.0)).is_none());
    }
}
