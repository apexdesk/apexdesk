//! Grouped sheets: entries, formats and row and column changes made on
//! one sheet of the group are made on the others too.

use super::*;

/// The undo labels of the edits that grouped sheets share. Others (sort,
/// fill, insert cells...) depend on each sheet's data and stay on one.
const SHARED: [&str; 18] = [
    "undo.edit",
    "undo.typing",
    "undo.paste",
    "undo.clear",
    "undo.clear_formats",
    "undo.format",
    "undo.borders",
    "undo.cell_style",
    "undo.insert_rows",
    "undo.delete_rows",
    "undo.insert_cols",
    "undo.delete_cols",
    "undo.col_width",
    "undo.row_height",
    "undo.hide_rows",
    "undo.show_rows",
    "undo.hide_cols",
    "undo.show_cols",
];

impl Workbook {
    /// Whether an edit named `label` goes to every grouped sheet.
    pub(super) fn grouped(&self, label: &str) -> bool {
        self.group.len() > 1 && (SHARED.contains(&label) || label == "undo.merge" || label == "undo.unmerge" || label == "undo.page_setup")
    }

    /// Applies `op` and its copies on the other grouped sheets; returns the
    /// inverse of all.
    pub(super) fn apply_grouped(&mut self, op: Op) -> Op {
        if let Op::Batch(ops) = op {
            let mut inv: Vec<Op> = ops.into_iter().map(|o| self.apply_grouped(o)).collect();
            inv.reverse();
            return Op::Batch(inv);
        }
        let copies = self.group_copies(&op);
        if copies.is_empty() {
            return self.apply(op);
        }
        let mut inv = vec![self.apply(op)];
        for c in copies {
            if !self.enforce_protection || self.allowed(&c) {
                inv.push(self.apply(c));
            }
        }
        inv.reverse();
        Op::Batch(inv)
    }

    /// The same change as `op` on each other sheet of the group.
    fn group_copies(&mut self, op: &Op) -> Vec<Op> {
        let Some(from) = op_sheet(op) else { return Vec::new() };
        if !self.group.contains(&from) {
            return Vec::new();
        }
        let others: Vec<usize> = self.group.iter().copied().filter(|&s| s != from && s < self.sheets.len()).collect();
        others.into_iter().filter_map(|to| self.copy_to(op, from, to)).collect()
    }

    fn copy_to(&mut self, op: &Op, from: usize, to: usize) -> Option<Op> {
        Some(match op {
            Op::SetCells { cells, .. } => {
                let cells: Vec<(CellRef, Option<Cell>)> = cells.iter().filter_map(|(c, new)| self.cell_change(from, to, *c, new.as_ref()).map(|n| (*c, n))).collect();
                if cells.is_empty() {
                    return None;
                }
                Op::SetCells { sheet: to, cells }
            }
            Op::InsertRows { at, count, .. } => Op::InsertRows { sheet: to, at: *at, count: *count },
            Op::DeleteRows { at, count, .. } => Op::DeleteRows { sheet: to, at: *at, count: *count },
            Op::InsertCols { at, count, .. } => Op::InsertCols { sheet: to, at: *at, count: *count },
            Op::DeleteCols { at, count, .. } => Op::DeleteCols { sheet: to, at: *at, count: *count },
            Op::ColWidths { sizes, .. } => Op::ColWidths { sheet: to, sizes: sizes.clone() },
            Op::RowHeights { sizes, .. } => Op::RowHeights { sheet: to, sizes: sizes.clone() },
            Op::Merges { merges, .. } => {
                // Areas added and removed on the source, applied to the target.
                let old = &self.sheets[from].merges;
                let mut list: Vec<Range> = self.sheets[to].merges.iter().filter(|m| !old.contains(m) || merges.contains(m)).copied().collect();
                for m in merges.iter().filter(|m| !old.contains(m)) {
                    list.retain(|x| x.intersect(m).is_none());
                    list.push(*m);
                }
                Op::Merges { sheet: to, merges: list }
            }
            Op::PageSetup { page, .. } => Op::PageSetup { sheet: to, page: page.clone() },
            _ => return None,
        })
    }

    /// What cell `c` of sheet `to` becomes when the same cell of `from`
    /// changes to `new`: the content, the format properties and the note
    /// or link that changed. None when nothing does.
    fn cell_change(&mut self, from: usize, to: usize, c: CellRef, new: Option<&Cell>) -> Option<Option<Cell>> {
        let old = self.sheets[from].get(c);
        let (old_content, new_content) = (old.map(|x| &x.content), new.map(|x| &x.content));
        let empty = Content::Empty;
        let content_changed = old_content.unwrap_or(&empty) != new_content.unwrap_or(&empty);
        let (os, ns) = (old.map_or(0, |x| x.style), new.map_or(0, |x| x.style));
        let extra_changed = old.and_then(|x| x.extra.as_ref()) != new.and_then(|x| x.extra.as_ref());
        if !content_changed && os == ns && !extra_changed {
            return None;
        }
        let target = self.sheets[to].get(c).cloned();
        let mut cell = target.clone().unwrap_or_else(|| Cell::new(Content::Empty, 0));
        if content_changed {
            cell.content = new_content.cloned().unwrap_or(Content::Empty);
        }
        if os != ns {
            let st = merge_style(self.styles.get(cell.style), self.styles.get(os), self.styles.get(ns));
            cell.style = self.styles.intern(st);
        }
        if extra_changed {
            cell.extra = new.and_then(|x| x.extra.clone());
        }
        let cell = (!cell.is_blank()).then_some(cell);
        (cell != target).then_some(cell)
    }
}

/// The sheet a sheet-level op changes.
fn op_sheet(op: &Op) -> Option<usize> {
    match op {
        Op::SetCells { sheet, .. }
        | Op::InsertRows { sheet, .. }
        | Op::DeleteRows { sheet, .. }
        | Op::InsertCols { sheet, .. }
        | Op::DeleteCols { sheet, .. }
        | Op::ColWidths { sheet, .. }
        | Op::RowHeights { sheet, .. }
        | Op::Merges { sheet, .. }
        | Op::PageSetup { sheet, .. } => Some(*sheet),
        _ => None,
    }
}

/// `target` with the properties that changed from `old` to `new`.
fn merge_style(target: &CellStyle, old: &CellStyle, new: &CellStyle) -> CellStyle {
    let mut s = target.clone();
    macro_rules! take {
        ($($f:ident),*) => {$(
            if old.$f != new.$f {
                s.$f = new.$f.clone();
            }
        )*};
    }
    take!(bold, italic, underline, strike, font, font_size, color, fill, halign, valign, wrap, indent, rotation, borders, num_fmt, unlocked, formula_hidden, named, theme);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_sheets_share_edits() {
        let mut w = Workbook::new();
        w.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("B")) });
        w.execute(Op::AddSheet { index: 2, sheet: Box::new(Sheet::new("C")) });
        w.set_inputs(1, vec![(CellRef::new(0, 1), "keep".into())]);
        w.modify_style(1, &Range::parse("A1").unwrap(), |s| s.italic = true);
        w.group = vec![0, 1];
        w.set_inputs(0, vec![(CellRef::new(0, 0), "x".into()), (CellRef::new(1, 0), "=A1&\"!\"".into())]);
        w.modify_style(0, &Range::parse("A1").unwrap(), |s| s.bold = true);
        w.execute(Op::InsertRows { sheet: 0, at: 0, count: 1 });
        w.recalc_all();
        let a2 = CellRef::new(1, 0);
        assert_eq!(w.sheets[1].value(a2), &Value::text("x"));
        assert_eq!(w.sheets[1].value(CellRef::new(2, 0)), &Value::text("x!"));
        assert_eq!(w.sheets[1].value(CellRef::new(1, 1)), &Value::text("keep"));
        // The bold is added to the italic already there.
        let st = w.styles.get(w.sheets[1].get(a2).unwrap().style);
        assert!(st.bold && st.italic);
        assert!(w.sheets[2].get(a2).is_none());
        // One undo step takes back the change on every sheet.
        w.undo();
        assert_eq!(w.sheets[1].value(CellRef::new(0, 0)), &Value::text("x"));
        w.undo();
        w.undo();
        assert!(w.sheets[1].value(CellRef::new(0, 0)).is_empty());
        // Sorting stays on one sheet.
        w.group = vec![0, 1];
        w.set_inputs(0, vec![(CellRef::new(0, 0), "b".into()), (CellRef::new(1, 0), "a".into())]);
        w.sort_by(0, &Range::parse("A1:A2").unwrap(), &[SortKey::new(0, true)], false);
        assert_eq!(w.sheets[0].value(CellRef::new(0, 0)), &Value::text("a"));
        assert_eq!(w.sheets[1].value(CellRef::new(0, 0)), &Value::text("b"));
    }
}
