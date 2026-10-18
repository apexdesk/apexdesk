//! Paste Special extras: column widths, validation and paste link.

use super::*;
use crate::data::{Validation, ranges_minus};

impl Workbook {
    /// Gives the columns from `at_col` the widths of the columns of `src`.
    pub fn paste_col_widths(&mut self, src_sheet: usize, src: &Range, sheet: usize, at_col: u32) {
        let clip = self.copy(src_sheet, src);
        self.paste_clip_col_widths(sheet, at_col, &clip);
    }

    /// Gives the columns from `at_col` the widths the columns of `clip`
    /// had where it was copied (in this workbook or another).
    pub fn paste_clip_col_widths(&mut self, sheet: usize, at_col: u32, clip: &Clip) {
        let width = |i: u32| clip.col_widths.iter().find(|(c, _)| *c == i).map(|(_, w)| *w);
        let sizes: Vec<(u32, Option<f32>)> = (0..clip.cols).filter(|i| at_col + i < MAX_COLS).map(|i| (at_col + i, width(i))).collect();
        self.execute_as(Op::ColWidths { sheet, sizes }, "undo.paste");
    }

    /// Copies the validation rules of `src` to the same shape at `at`,
    /// replacing the rules there. Relative references in the rules move.
    pub fn paste_validation(&mut self, src_sheet: usize, src: &Range, sheet: usize, at: CellRef) -> Range {
        let clip = self.copy(src_sheet, src);
        self.paste_clip_validation(sheet, at, &clip)
    }

    /// The validation rules of the cells of `clip` (copied in this
    /// workbook or another) over the same shape at `at`, replacing the
    /// rules there. Relative references in the rules move.
    pub fn paste_clip_validation(&mut self, sheet: usize, at: CellRef, clip: &Clip) -> Range {
        let src = &Range::new(clip.origin, CellRef::new(clip.origin.row + clip.rows.max(1) - 1, clip.origin.col + clip.cols.max(1) - 1));
        let end = CellRef::new((at.row + src.rows() - 1).min(MAX_ROWS - 1), (at.col + src.cols() - 1).min(MAX_COLS - 1));
        let dest = Range::new(at, end);
        let (dr, dc) = (at.row as i64 - src.start.row as i64, at.col as i64 - src.start.col as i64);
        let moved = |f: &str| -> String {
            match crate::formula::parse_canonical(f) {
                Ok(mut e) if !f.is_empty() => {
                    e.offset(dr, dc);
                    e.to_canonical()
                }
                _ => f.to_string(),
            }
        };
        let mut added: Vec<Validation> = Vec::new();
        for v in &clip.validations {
            let ranges: Vec<Range> = v
                .ranges
                .iter()
                .filter_map(|r| r.intersect(src))
                .map(|r| {
                    let s = CellRef::new((r.start.row as i64 + dr) as u32, (r.start.col as i64 + dc) as u32);
                    let e = CellRef::new(((r.end.row as i64 + dr) as u32).min(MAX_ROWS - 1), ((r.end.col as i64 + dc) as u32).min(MAX_COLS - 1));
                    Range::new(s, e)
                })
                .collect();
            if !ranges.is_empty() {
                added.push(Validation { ranges, f1: moved(&v.f1), f2: moved(&v.f2), ..v.clone() });
            }
        }
        let mut list: Vec<Validation> = self.sheets[sheet]
            .validations
            .iter()
            .cloned()
            .filter_map(|mut x| {
                x.ranges = ranges_minus(&x.ranges, &dest);
                (!x.ranges.is_empty()).then_some(x)
            })
            .collect();
        list.extend(added);
        if list != self.sheets[sheet].validations {
            self.execute(Op::Validations { sheet, list });
        }
        dest
    }

    /// Paste Link: formulas that point at the cells of `src` (absolute for
    /// one cell, relative for a range, like Excel).
    pub fn paste_link(&mut self, src_sheet: usize, src: &Range, sheet: usize, at: CellRef) -> Range {
        let end = CellRef::new((at.row + src.rows() - 1).min(MAX_ROWS - 1), (at.col + src.cols() - 1).min(MAX_COLS - 1));
        let dest = Range::new(at, end);
        if dest.cell_count() > 100_000 {
            return Range::single(at);
        }
        let prefix = if src_sheet == sheet { String::new() } else { format!("{}!", crate::chart::quote_sheet(&self.sheets[src_sheet].name)) };
        let mut cells = Vec::new();
        for r in 0..dest.rows() {
            for c in 0..dest.cols() {
                let from = CellRef::new(src.start.row + r, src.start.col + c);
                let a1 = if src.is_single() { format!("${}${}", crate::addr::col_name(from.col), from.row + 1) } else { from.to_a1() };
                let Ok(expr) = crate::formula::parse_canonical(&format!("{prefix}{a1}")) else { continue };
                let to = CellRef::new(at.row + r, at.col + c);
                let content = Content::Formula { expr: expr.into(), value: Value::Empty };
                cells.push((to, Some(Cell::with_content(self.sheets[sheet].get(to), content).unwrap_or_else(|| Cell::new(Content::Empty, 0)))));
            }
        }
        self.execute_as(Op::SetCells { sheet, cells }, "undo.paste");
        dest
    }

    /// Paste Link for cells copied in another workbook: formulas that
    /// point at them in their file (`target`: how this workbook names it,
    /// see `ExternalBook::target`), which becomes a linked book holding
    /// the values the cells had when they were copied. `None` when the
    /// clip doesn't say where it comes from.
    pub fn paste_link_external(&mut self, sheet: usize, at: CellRef, clip: &Clip, target: &str) -> Option<Range> {
        let src_sheet = &clip.source.as_ref()?.sheet;
        let end = CellRef::new((at.row + clip.rows.max(1) - 1).min(MAX_ROWS - 1), (at.col + clip.cols.max(1) - 1).min(MAX_COLS - 1));
        let dest = Range::new(at, end);
        if dest.cell_count() > 100_000 {
            return Some(Range::single(at));
        }
        let book = match self.externals.iter().position(|b| b.target == target) {
            Some(i) => i,
            None => {
                self.externals.push(crate::external::ExternalBook { target: target.to_string(), ..Default::default() });
                self.externals.len() - 1
            }
        };
        let b = &mut self.externals[book];
        let si = b.sheets.iter().position(|s| s.eq_ignore_ascii_case(src_sheet)).unwrap_or_else(|| {
            b.sheets.push(src_sheet.clone());
            b.cells.push(Default::default());
            b.sheets.len() - 1
        });
        for (dr, dc, cell) in &clip.cells {
            b.cells[si].insert(CellRef::new(clip.origin.row + dr, clip.origin.col + dc), cell.value().clone());
        }
        let name: Arc<str> = Arc::from(format!("[{}]{src_sheet}", book + 1));
        let single = clip.rows <= 1 && clip.cols <= 1;
        let mut cells = Vec::new();
        for r in 0..dest.rows() {
            for c in 0..dest.cols() {
                let addr = crate::formula::CellAddr { row: clip.origin.row + r, col: clip.origin.col + c, row_abs: single, col_abs: single };
                let to = CellRef::new(at.row + r, at.col + c);
                let content = Content::Formula { expr: Expr::Ref { sheet: Some(name.clone()), addr }.into(), value: Value::Empty };
                cells.push((to, Some(Cell::with_content(self.sheets[sheet].get(to), content).unwrap_or_else(|| Cell::new(Content::Empty, 0)))));
            }
        }
        self.execute_as(Op::SetCells { sheet, cells }, "undo.paste");
        Some(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ValKind;

    fn c(a: &str) -> CellRef {
        CellRef::parse(a).unwrap()
    }

    #[test]
    fn paste_extras() {
        let mut w = Workbook::new();
        w.sheets.push(crate::Sheet::new("Two words"));
        w.set_input(1, c("B2"), "7");
        w.set_input(1, c("C2"), "8");
        w.sheets[1].col_widths.insert(1, 150.0);
        let mut v = Validation::new(vec![Range::parse("B2:C2").unwrap()]);
        v.kind = ValKind::Custom;
        v.f1 = "B2>0".into();
        w.execute(Op::Validations { sheet: 1, list: vec![v] });
        let src = Range::parse("B2:C2").unwrap();
        w.paste_col_widths(1, &src, 0, 3);
        assert_eq!(w.sheets[0].col_widths.get(&3), Some(&150.0));
        assert_eq!(w.sheets[0].col_widths.get(&4), None);
        let d = w.paste_validation(1, &src, 0, c("D5"));
        assert_eq!(d.to_a1(), "D5:E5");
        let v = &w.sheets[0].validations[0];
        assert_eq!((v.ranges[0].to_a1(), v.f1.as_str()), ("D5:E5".to_string(), "D5>0"));
        w.paste_link(1, &src, 0, c("A1"));
        assert_eq!(w.display(0, c("B1")).0, "8");
        assert_eq!(w.sheets[0].get(c("A1")).unwrap().formula().unwrap().expr().to_canonical(), "'Two words'!B2");
        w.paste_link(1, &Range::single(c("B2")), 0, c("A3"));
        assert_eq!(w.sheets[0].get(c("A3")).unwrap().formula().unwrap().expr().to_canonical(), "'Two words'!$B$2");
        w.set_input(1, c("B2"), "9");
        assert_eq!(w.display(0, c("A3")).0, "9");
    }

    #[test]
    fn extras_go_to_another_workbook() {
        let mut a = Workbook::new();
        a.sheets[0].name = "Two words".into();
        a.set_input(0, c("B2"), "7");
        a.set_input(0, c("C2"), "=B2+1");
        a.sheets[0].col_widths.insert(2, 150.0);
        a.sheets[0].row_heights.insert(1, 40.0);
        let mut v = Validation::new(vec![Range::parse("A1:B9").unwrap()]);
        v.kind = ValKind::Custom;
        v.f1 = "B2>0".into();
        a.execute(Op::Validations { sheet: 0, list: vec![v] });
        let mut clip = a.copy(0, &Range::parse("B2:C2").unwrap());
        assert_eq!((clip.col_widths.clone(), clip.row_heights.clone()), (vec![(1, 150.0)], vec![(0, 40.0)]));

        let mut b = Workbook::new();
        let clip_b = b.adopt_clip(&clip);
        b.paste_clip_col_widths(0, 4, &clip_b);
        assert_eq!((b.sheets[0].col_widths.get(&4), b.sheets[0].col_widths.get(&5)), (None, Some(&150.0)));
        assert_eq!(b.paste_clip_validation(0, c("D5"), &clip_b).to_a1(), "D5:E5");
        let v = &b.sheets[0].validations[0];
        assert_eq!((v.ranges[0].to_a1(), v.f1.as_str()), ("D5".to_string(), "D5>0"));
        // A link needs to know the file.
        assert_eq!(b.paste_link_external(0, c("A1"), &clip_b, "Budget.xlsx"), None);
        clip.source = Some(ClipSource { file: "/home/u/Budget.xlsx".into(), sheet: "Two words".into() });
        let d = b.paste_link_external(0, c("A1"), &clip, "Budget.xlsx").unwrap();
        assert_eq!(d.to_a1(), "A1:B1");
        assert_eq!((b.display(0, c("A1")).0, b.display(0, c("B1")).0), ("7".to_string(), "8".to_string()));
        assert_eq!(b.edit_text(0, c("B1")), "='[Budget.xlsx]Two words'!C2");
        assert_eq!((b.externals.len(), b.externals[0].sheets.clone()), (1, vec!["Two words".to_string()]));
        // Once more: the same linked book.
        b.paste_link_external(0, c("A3"), &clip, "Budget.xlsx");
        assert_eq!(b.externals.len(), 1);
    }
}
