//! Data > Form: the records of a list with a header row, one at a time.
//! A list is a range whose first row holds the field names; record `i`
//! (from 0) is the row `i + 1` of it.

use super::*;
use crate::functions::Criteria;

/// One field of a record as the form shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FormField {
    /// Text to edit, or the result of a formula.
    pub text: String,
    /// Computed by a formula: shown, not edited.
    pub computed: bool,
}

impl Workbook {
    /// The list around `c` (its current region), None when there is no
    /// data there.
    pub fn form_list(&self, sheet: usize, c: CellRef) -> Option<Range> {
        let r = self.current_region(sheet, c);
        let sh = &self.sheets[sheet];
        (0..r.cols()).any(|k| !sh.value(CellRef::new(r.start.row, r.start.col + k)).is_empty()).then_some(r)
    }

    /// Field names: the header texts (the column letter where empty).
    pub fn form_fields(&self, sheet: usize, list: &Range) -> Vec<String> {
        (list.start.col..=list.end.col)
            .map(|col| {
                let t = self.display(sheet, CellRef::new(list.start.row, col)).0;
                if t.trim().is_empty() { crate::addr::col_name(col) } else { t }
            })
            .collect()
    }

    /// Number of records.
    pub fn form_count(list: &Range) -> usize {
        list.rows() as usize - 1
    }

    fn form_row(list: &Range, i: usize) -> u32 {
        list.start.row + 1 + i as u32
    }

    /// The fields of record `i`.
    pub fn form_record(&self, sheet: usize, list: &Range, i: usize) -> Vec<FormField> {
        let row = Self::form_row(list, i);
        (list.start.col..=list.end.col)
            .map(|col| {
                let c = CellRef::new(row, col);
                match self.cell(sheet, c).and_then(|x| x.formula()) {
                    Some(_) => FormField { text: self.display(sheet, c).0, computed: true },
                    None => FormField { text: self.edit_text(sheet, c), computed: false },
                }
            })
            .collect()
    }

    /// Writes the edited fields of record `i` (computed ones are left
    /// alone): one undo step. False when nothing changed.
    pub fn form_update(&mut self, sheet: usize, list: &Range, i: usize, values: &[String]) -> bool {
        let row = Self::form_row(list, i);
        let old = self.form_record(sheet, list, i);
        let inputs: Vec<(CellRef, String)> =
            old.iter().zip(values).enumerate().filter(|(_, (f, v))| !f.computed && f.text != **v).map(|(k, (_, v))| (CellRef::new(row, list.start.col + k as u32), v.clone())).collect();
        if inputs.is_empty() {
            return false;
        }
        let before = self.undo.len();
        self.set_inputs(sheet, inputs);
        self.label_steps(before, "undo.data_form")
    }

    /// Adds a record under the list; formulas of the last record are
    /// copied down. Returns the grown list (the same one if nothing could
    /// be added).
    pub fn form_add(&mut self, sheet: usize, list: &Range, values: &[String]) -> Range {
        let row = list.end.row + 1;
        if row >= MAX_ROWS || values.iter().all(|v| v.trim().is_empty()) {
            return *list;
        }
        let before = self.undo.len();
        let prev = self.form_record(sheet, list, Self::form_count(list).saturating_sub(1));
        let has_prev = Self::form_count(list) > 0;
        let mut inputs = Vec::new();
        for (k, col) in (list.start.col..=list.end.col).enumerate() {
            let c = CellRef::new(row, col);
            if has_prev && prev.get(k).is_some_and(|f| f.computed) {
                self.fill_from(sheet, &Range::new(CellRef::new(row - 1, col), c), CellRef::new(row - 1, col));
            } else if let Some(v) = values.get(k).filter(|v| !v.is_empty()) {
                inputs.push((c, v.clone()));
            }
        }
        if !inputs.is_empty() {
            self.set_inputs(sheet, inputs);
        }
        if !self.label_steps(before, "undo.data_form") {
            return *list;
        }
        Range::new(list.start, CellRef::new(row, list.end.col))
    }

    /// Deletes record `i`, moving the records below it up. A row with
    /// nothing else in it is deleted whole (references follow); otherwise
    /// only the list's cells move, their formulas as if filled.
    pub fn form_delete(&mut self, sheet: usize, list: &Range, i: usize) -> Range {
        if i >= Self::form_count(list) {
            return *list;
        }
        let row = Self::form_row(list, i);
        let before = self.undo.len();
        let sh = &self.sheets[sheet];
        let mut alone = true;
        sh.for_each_in(&Range::new(CellRef::new(row, 0), CellRef::new(row, MAX_COLS - 1)), |c, _| alone &= list.contains(c));
        if alone {
            self.execute(Op::DeleteRows { sheet, at: row, count: 1 });
        } else {
            let mut cells = Vec::new();
            for r in row..=list.end.row {
                for col in list.start.col..=list.end.col {
                    let below = (r < list.end.row).then(|| sh.get(CellRef::new(r + 1, col)).cloned()).flatten();
                    let new = below.map(|c| edit::offset_formula(c, -1, 0));
                    if new.as_ref() != sh.get(CellRef::new(r, col)) {
                        cells.push((CellRef::new(r, col), new));
                    }
                }
            }
            if !cells.is_empty() {
                self.execute(Op::SetCells { sheet, cells });
            }
        }
        if !self.label_steps(before, "undo.data_form") {
            return *list;
        }
        Range::new(list.start, CellRef::new(list.end.row - 1, list.end.col))
    }

    /// The next record after `from` (or before it, going back) that meets
    /// the criteria: one per field, empty for any value. Plain text finds
    /// the values that start with it; `>10`, `<>x`, `=abc` compare.
    pub fn form_find(&self, sheet: usize, list: &Range, from: Option<usize>, forward: bool, criteria: &[String]) -> Option<usize> {
        let n = Self::form_count(list);
        let conds: Vec<(u32, Criteria)> = criteria
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.trim().is_empty())
            .map(|(k, t)| {
                let plain = !t.starts_with(['<', '>', '=']) && matches!(format::parse_input(t, &self.locale).value, Value::Text(_));
                let t = if plain { format!("{t}*") } else { t.clone() };
                (list.start.col + k as u32, Criteria::new(Value::text(t), &self.locale))
            })
            .collect();
        let hit = |i: usize| {
            let row = Self::form_row(list, i);
            conds.iter().all(|(col, c)| c.matches(self.sheets[sheet].value(CellRef::new(row, *col))))
        };
        if forward {
            let start = from.map_or(0, |f| f + 1);
            (start..n).find(|&i| hit(i))
        } else {
            let end = from.unwrap_or(n).min(n);
            (0..end).rev().find(|&i| hit(i))
        }
    }

    /// Joins the undo steps made since `before` into one named `label`;
    /// false when there were none.
    fn label_steps(&mut self, before: usize, label: &'static str) -> bool {
        let n = self.undo.len().saturating_sub(before);
        match n {
            0 => false,
            1 => {
                if let Some(last) = self.undo.last_mut() {
                    last.1 = label;
                }
                true
            }
            _ => {
                self.merge_undo(n, label);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> (Workbook, Range) {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let c = |a: &str| CellRef::parse(a).unwrap();
        for (a, t) in [("B2", "Name"), ("C2", "Qty"), ("D2", "Double"), ("B3", "apple"), ("C3", "3"), ("D3", "=C3*2"), ("B4", "banana"), ("C4", "12"), ("D4", "=C4*2"), ("B5", "avocado"), ("C5", "7"), ("D5", "=C5*2")] {
            w.set_input(0, c(a), t);
        }
        let list = w.form_list(0, c("C4")).unwrap();
        (w, list)
    }

    #[test]
    fn records_and_edits() {
        let (mut w, list) = book();
        let c = |a: &str| CellRef::parse(a).unwrap();
        assert_eq!(list, Range::parse("B2:D5").unwrap());
        assert_eq!(w.form_fields(0, &list), vec!["Name", "Qty", "Double"]);
        assert_eq!(Workbook::form_count(&list), 3);
        let rec = w.form_record(0, &list, 1);
        assert_eq!(rec[0], FormField { text: "banana".into(), computed: false });
        assert_eq!(rec[2], FormField { text: "24".into(), computed: true });
        assert!(w.form_list(0, c("H20")).is_none());

        // Editing: computed fields are left alone; one undo step.
        let n = w.undo_len();
        assert!(w.form_update(0, &list, 1, &["cherry".into(), "5".into(), "999".into()]));
        assert_eq!(w.undo_len(), n + 1);
        assert_eq!(w.undo_history()[0], "undo.data_form");
        assert_eq!(w.display(0, c("D4")).0, "10");
        assert!(!w.form_update(0, &list, 1, &["cherry".into(), "5".into(), "10".into()]));
        w.undo();
        assert_eq!(w.display(0, c("B4")).0, "banana");

        // New record: formulas come down from the last one.
        let grown = w.form_add(0, &list, &["kiwi".into(), "4".into(), String::new()]);
        assert_eq!(grown, Range::parse("B2:D6").unwrap());
        assert_eq!(w.display(0, c("D6")).0, "8");
        assert_eq!(w.undo_history()[0], "undo.data_form");
        assert_eq!(w.form_add(0, &grown, &[String::new(), " ".into()]), grown);
        w.undo();
        assert!(w.cell(0, c("B6")).is_none() && w.cell(0, c("D6")).is_none());

        // Deleting moves the records below up, inside the list only.
        w.set_input(0, c("F3"), "outside");
        let shrunk = w.form_delete(0, &list, 0);
        assert_eq!(shrunk, Range::parse("B2:D4").unwrap());
        assert_eq!(w.display(0, c("B3")).0, "banana");
        assert_eq!(w.display(0, c("D3")).0, "24");
        assert_eq!(w.display(0, c("F3")).0, "outside");
        w.undo();
        assert_eq!(w.display(0, c("B3")).0, "apple");
        // A row with nothing else is deleted whole.
        w.set_input(0, c("H1"), "=SUM(C3:C5)");
        w.form_delete(0, &list, 1);
        assert_eq!(w.display(0, c("B4")).0, "avocado");
        assert_eq!(w.display(0, c("D4")).0, "14");
        assert_eq!(w.display(0, c("H1")).0, "10");
        assert_eq!(w.edit_text(0, c("H1")), "=SUM(C3:C4)");
    }

    #[test]
    fn criteria() {
        let (w, list) = book();
        let crit = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Text finds values starting with it.
        assert_eq!(w.form_find(0, &list, None, true, &crit(&["a"])), Some(0));
        assert_eq!(w.form_find(0, &list, Some(0), true, &crit(&["a"])), Some(2));
        assert_eq!(w.form_find(0, &list, Some(2), true, &crit(&["a"])), None);
        assert_eq!(w.form_find(0, &list, Some(2), false, &crit(&["a"])), Some(0));
        // Comparisons, on several fields.
        assert_eq!(w.form_find(0, &list, None, true, &crit(&["", ">5"])), Some(1));
        assert_eq!(w.form_find(0, &list, None, true, &crit(&["a", ">5"])), Some(2));
        assert_eq!(w.form_find(0, &list, None, true, &crit(&["", "", "=6"])), Some(0));
        assert_eq!(w.form_find(0, &list, None, false, &crit(&[])), Some(2));
    }
}
