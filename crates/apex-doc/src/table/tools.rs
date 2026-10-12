//! Table tools: AutoFit, Distribute Rows and Columns, Split Table.

use super::*;

impl Document {
    /// The selected block, or the whole table for a caret in one cell.
    fn block_or_table(&self, sel: Selection) -> Option<Block> {
        let b = self.cell_block(sel)?;
        if (b.r0, b.c0) == (b.r1, b.c1) {
            let m = TableMap::build_at(&self.paragraphs, b.start, b.depth)?;
            return Some(Block { r0: 0, c0: 0, r1: m.rows - 1, c1: m.cols - 1, ..b });
        }
        Some(b)
    }

    /// Gives the selected columns (all of them for a caret) the same width,
    /// keeping their total.
    pub fn distribute_columns(&mut self, sel: Selection) -> bool {
        let Some(b) = self.block_or_table(sel) else { return false };
        let Some((tk, mut t)) = self.take_block(&b) else { return false };
        let n = b.c1 - b.c0 + 1;
        let total: f32 = t.props.widths[b.c0..=b.c1.min(t.props.widths.len() - 1)].iter().sum();
        let before = t.props.clone();
        for w in &mut t.props.widths[b.c0..=b.c1] {
            *w = (total / n as f32).max(MIN_COL_WIDTH);
        }
        t.fix();
        if t.props == before {
            return false;
        }
        self.put_raw(&tk, t, sel, sel, EditKind::Table);
        true
    }

    /// Whether Distribute Columns would change anything: the columns it
    /// acts on are not all of the same width.
    pub fn columns_uneven(&self, sel: Selection) -> bool {
        let Some(b) = self.block_or_table(sel) else { return false };
        let Some(p) = TableMap::build_at(&self.paragraphs, b.start, b.depth).and_then(|m| m.props(&self.paragraphs).cloned()) else { return false };
        let w = &p.widths[b.c0.min(p.widths.len())..=b.c1.min(p.widths.len().saturating_sub(1))];
        w.windows(2).any(|x| (x[0] - x[1]).abs() > 0.01)
    }

    /// Gives the selected rows (all of them for a caret) the same height:
    /// the average of `heights`, their heights now in points.
    pub fn distribute_rows(&mut self, sel: Selection, heights: &[f32]) -> bool {
        let Some(b) = self.block_or_table(sel) else { return false };
        let Some((tk, mut t)) = self.take_block(&b) else { return false };
        let rows: Vec<f32> = (b.r0..=b.r1).filter_map(|r| heights.get(r).copied()).filter(|h| h.is_finite() && *h > 0.0).collect();
        if rows.is_empty() {
            return false;
        }
        let avg = rows.iter().sum::<f32>() / rows.len() as f32;
        let before = t.props.clone();
        let hs = &mut t.props.row_heights;
        hs.resize(t.rows.len().max(hs.len()), 0.0);
        for h in &mut hs[b.r0..=b.r1] {
            *h = (avg * 10.0).round() / 10.0;
        }
        t.fix();
        if t.props == before {
            return false;
        }
        self.put_raw(&tk, t, sel, sel, EditKind::Table);
        true
    }

    /// Sets the column widths of the table at the caret (AutoFit to its
    /// contents, worked out by the view).
    pub fn set_column_widths(&mut self, sel: Selection, widths: &[f32]) -> bool {
        let widths = widths.to_vec();
        self.set_table_props(sel, &move |p| {
            for (w, v) in p.widths.iter_mut().zip(&widths) {
                *w = v.max(MIN_COL_WIDTH);
            }
        })
    }

    /// AutoFit to the window: the table spans `width` points.
    pub fn fit_table_width(&mut self, sel: Selection, width: f32) -> bool {
        self.set_table_props(sel, &move |p| {
            let total = p.width();
            if total > 0.0 && width > 0.0 {
                for w in &mut p.widths {
                    *w = (*w * width / total).max(MIN_COL_WIDTH);
                }
                p.indent = 0.0;
                p.align = Align::Left;
            }
        })
    }

    /// Splits the table at the caret above its row: the rows from there on
    /// become a new table after an empty paragraph. In the first row, as in
    /// Word, the empty paragraph goes before the table. Returns the caret,
    /// in that paragraph.
    pub fn split_table(&mut self, sel: Selection) -> Option<Selection> {
        let b = self.cell_block(Selection::caret(sel.cursor))?;
        let r = b.r0;
        let (tk, mut t) = self.take_block(&b)?;
        if r == 0 {
            let mut p = Paragraph::new("");
            p.cell = tk.outer.as_ref().map(InCell::bare);
            let caret = Selection::caret(Pos::new(tk.a, 0));
            self.replace_paras(tk.a, tk.a, vec![p], sel, caret, EditKind::Table);
            return Some(caret);
        }
        // Merged cells across the split: the part below keeps its merge.
        for c in 0..t.ncols() {
            let (ar, ac) = t.anchor_of(r, c);
            if ar < r && ac == c {
                let (rs, cs) = t.span(ar, ac);
                let p = &mut t.rows[r][c].props;
                p.covered = false;
                p.row_span = (ar + rs - r) as u32;
                p.col_span = cs as u16;
                t.rows[ar][ac].props.row_span = (r - ar) as u32;
            }
        }
        let rows2 = t.rows.split_off(r);
        let mut props2 = t.props.clone();
        props2.header_rows = 0;
        props2.row_heights = t.props.row_heights.get(r..).map(<[f32]>::to_vec).unwrap_or_default();
        t.props.row_heights.truncate(r);
        props2.row_rules = t.props.row_rules.get(r..).map(<[_]>::to_vec).unwrap_or_default();
        t.props.row_rules.truncate(r);
        let mut t2 = Table { props: props2, rows: rows2 };
        t.fix();
        t2.fix();
        let id2 = self.next_table_id();
        let mut new = t.into_paras(tk.id);
        let n1 = new.len();
        let caret = Selection::caret(Pos::new(tk.a + n1, 0));
        new.push(Paragraph::new(""));
        new.extend(t2.into_paras(id2));
        let mut new = Self::wrap_taken(&tk, new);
        // The paragraph between the tables belongs to the cell around them.
        new[n1].cell = tk.outer.as_ref().map(InCell::bare);
        self.replace_paras(tk.a, tk.e, new, sel, caret, EditKind::Table);
        Some(caret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> (Document, Selection) {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("")]);
        let s = d.insert_table(Selection::caret(Pos::new(0, 0)), 4, 3, 300.0);
        (d, s)
    }

    #[test]
    fn distribute_and_fit() {
        let (mut d, s) = doc();
        assert!(d.set_column_widths(s, &[50.0, 150.0, 100.0]));
        assert!(d.distribute_columns(s));
        let m = d.table_map(s.cursor.para).unwrap();
        assert_eq!(m.props(&d.paragraphs).unwrap().widths, [100.0, 100.0, 100.0]);
        assert!(d.fit_table_width(s, 450.0));
        assert_eq!(d.table_map(s.cursor.para).unwrap().props(&d.paragraphs).unwrap().widths, [150.0, 150.0, 150.0]);
        assert!(d.distribute_rows(s, &[20.0, 30.0, 40.0, 50.0]));
        assert_eq!(d.table_map(s.cursor.para).unwrap().props(&d.paragraphs).unwrap().row_heights, [35.0; 4]);
    }

    #[test]
    fn split() {
        let (mut d, _) = doc();
        let m = d.table_map(0).unwrap();
        // Merge a column across rows 1..3, then split at row 2.
        let (a, _) = m.cell(1, 0);
        let (b, _) = m.cell(2, 0);
        d.merge_cells(Selection { anchor: Pos::new(a, 0), cursor: Pos::new(b, 0) }).unwrap();
        let m = d.table_map(0).unwrap();
        let (p, _) = m.cell(2, 1);
        let s = d.split_table(Selection::caret(Pos::new(p, 0))).unwrap();
        let tables = d.tables();
        assert_eq!(tables.len(), 2);
        assert_eq!((tables[0].rows, tables[1].rows), (2, 2));
        assert!(d.paragraphs[s.cursor.para].cell.is_none());
        assert_eq!(s.cursor.para, tables[0].end);
        d.undo();
        assert_eq!(d.tables().len(), 1);
        // In the first row: an empty paragraph above the table.
        let n = d.paragraphs.len();
        let s = d.split_table(Selection::caret(Pos::new(0, 0))).unwrap();
        assert_eq!(s.cursor, Pos::new(0, 0));
        assert_eq!(d.paragraphs.len(), n + 1);
        assert!(d.paragraphs[0].cell.is_none() && d.in_table(1));
        d.undo();
        assert_eq!(d.paragraphs.len(), n);
    }

    #[test]
    fn uneven_columns() {
        let (mut d, s) = doc();
        assert!(!d.columns_uneven(s));
        assert!(d.set_column_widths(s, &[50.0, 150.0, 100.0]));
        assert!(d.columns_uneven(s));
        assert!(d.distribute_columns(s));
        assert!(!d.columns_uneven(s));
    }
}
