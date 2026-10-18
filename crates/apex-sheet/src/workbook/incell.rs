//! Controls and pictures inside cells: check boxes (a cell format that
//! shows TRUE/FALSE as a box) and pictures that are a cell's value.

use super::*;
use crate::image::{Anchor, CellPicture, SheetImage};

impl Workbook {
    /// Insert > Checkbox: gives the cells of `r` the check box format and
    /// turns the empty ones into FALSE (one undo step).
    pub fn insert_checkboxes(&mut self, sheet: usize, r: &Range) {
        let r = if r.cell_count() > 1_000_000 {
            match self.clamp_to_used(sheet, r) {
                Some(x) => x,
                None => return,
            }
        } else {
            *r
        };
        let mut cache: HashMap<u32, u32> = HashMap::default();
        let mut cells = Vec::new();
        for row in r.start.row..=r.end.row {
            for col in r.start.col..=r.end.col {
                let c = CellRef::new(row, col);
                let old = self.sheets[sheet].get(c);
                let old_style = old.map_or(0, |x| x.style);
                let style = *cache.entry(old_style).or_insert_with(|| {
                    let mut s = self.styles.get(old_style).clone();
                    s.checkbox = true;
                    self.styles.intern(s)
                });
                let empty = old.is_none_or(|x| matches!(x.content, Content::Empty));
                if style == old_style && !empty {
                    continue;
                }
                let content = if empty { Content::Value(Value::Bool(false)) } else { old.map_or(Content::Empty, |x| x.content.clone()) };
                let mut cell = Cell::with_content(old, content).unwrap_or_else(|| Cell::new(Content::Empty, 0));
                cell.style = style;
                cells.push((c, Some(cell)));
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.checkbox");
        }
    }

    /// Whether a cell shows a check box: the format asks for it and the
    /// value is TRUE, FALSE or nothing.
    pub fn is_checkbox(&self, sheet: usize, c: CellRef) -> bool {
        let Some(cell) = self.sheets[sheet].get(c) else { return false };
        self.styles.get(cell.style).checkbox && matches!(cell.content, Content::Empty | Content::Value(Value::Bool(_)))
    }

    /// Clicks (or Space) on the check boxes among `cells`: one box flips;
    /// several all become checked, or unchecked when they already all
    /// are. Formulas and locked cells of a protected sheet are left alone.
    /// Returns whether anything changed.
    pub fn toggle_checkboxes(&mut self, sheet: usize, cells: &[CellRef]) -> bool {
        let boxes: Vec<(CellRef, bool)> = cells
            .iter()
            .filter(|&&c| self.is_checkbox(sheet, c) && !self.cell_locked(sheet, c))
            .map(|&c| (c, matches!(self.sheets[sheet].get(c).map(|x| &x.content), Some(Content::Value(Value::Bool(true))))))
            .collect();
        if boxes.is_empty() {
            return false;
        }
        let to = if boxes.len() == 1 { !boxes[0].1 } else { !boxes.iter().all(|b| b.1) };
        let cells: Vec<_> = boxes
            .into_iter()
            .filter(|b| b.1 != to)
            .map(|(c, _)| (c, Cell::with_content(self.sheets[sheet].get(c), Content::Value(Value::Bool(to)))))
            .collect();
        if cells.is_empty() {
            return false;
        }
        self.execute_as(Op::SetCells { sheet, cells }, "undo.checkbox_toggle");
        true
    }

    /// Place in Cell: the floating picture `id` becomes the value of the
    /// cell under its top-left corner.
    pub fn place_in_cell(&mut self, sheet: usize, id: u32) -> Option<CellRef> {
        let img = self.sheets[sheet].image(id)?.clone();
        let c = img.from;
        let mut pic = CellPicture::new(img.data.clone(), img.format);
        pic.alt = img.alt.clone();
        let cell = Cell::with_picture(self.sheets[sheet].get(c), pic);
        let mut list = self.sheets[sheet].images.clone();
        list.retain(|x| x.id != id);
        let ops = vec![Op::Images { sheet, list }, Op::SetCells { sheet, cells: vec![(c, Some(cell))] }];
        self.execute_as(Op::Batch(ops), "undo.place_in_cell");
        Some(c)
    }

    /// Place over Cells: the picture in cell `c` becomes a floating
    /// picture over it, at the size it had in the cell, and the cell is
    /// emptied. Returns the new picture's id.
    pub fn place_over_cells(&mut self, sheet: usize, c: CellRef) -> Option<u32> {
        let sh = &self.sheets[sheet];
        let cell = sh.get(c)?;
        let pic = cell.picture()?.clone();
        let (w, h) = (sh.col_width(c.col), sh.row_height(c.row));
        let [dx, dy, pw, ph] = pic.fit(w, h);
        let mut img = SheetImage::new(pic.data.clone(), pic.format, c, pw.max(1.0), ph.max(1.0));
        img.dx = dx;
        img.dy = dy;
        img.alt = pic.alt.clone();
        img.anchor = Anchor::OneCell;
        img.id = sh.next_image_id();
        img.name = format!("Picture {}", img.id);
        img.sanitize();
        let id = img.id;
        let mut list = sh.images.clone();
        list.push(img);
        let cleared = Cell::with_content(Some(cell), Content::Empty);
        let ops = vec![Op::SetCells { sheet, cells: vec![(c, cleared)] }, Op::Images { sheet, list }];
        self.execute_as(Op::Batch(ops), "undo.place_over_cells");
        Some(id)
    }

    /// Whether any cell holds a picture.
    pub fn has_cell_pictures(&self) -> bool {
        self.sheets.iter().any(|s| s.iter().any(|(_, c)| c.picture().is_some()))
    }

    /// A copy where the pictures in cells float over their cells, moving
    /// and sizing with them, for formats without pictures as values (ODF).
    pub fn with_floating_cell_pictures(&self) -> Workbook {
        let mut wb = self.clone();
        for sh in &mut wb.sheets {
            let cells: Vec<(CellRef, CellPicture)> = sh.iter().filter_map(|(c, cell)| Some((c, cell.picture()?.clone()))).collect();
            for (c, pic) in cells {
                let [dx, dy, w, h] = pic.fit(sh.col_width(c.col), sh.row_height(c.row));
                let mut img = SheetImage::new(pic.data.clone(), pic.format, c, w.max(1.0), h.max(1.0));
                (img.dx, img.dy, img.alt, img.anchor) = (dx, dy, pic.alt.clone(), Anchor::TwoCell);
                img.id = sh.next_image_id();
                img.name = format!("Picture {}", img.id);
                img.sanitize();
                sh.images.push(img);
                let cell = Cell::with_content(sh.get(c), Content::Empty);
                sh.put(c, cell);
            }
        }
        wb
    }

    /// Puts a picture file into cell `c` as its value (undoable).
    pub fn set_cell_picture(&mut self, sheet: usize, c: CellRef, pic: CellPicture) {
        let cell = Cell::with_picture(self.sheets[sheet].get(c), pic);
        self.execute_as(Op::SetCells { sheet, cells: vec![(c, Some(cell))] }, "undo.place_in_cell");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::ImageFormat;

    const PNG_4X2: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 4, 0, 0, 0, 2, 8, 6, 0, 0, 0];

    #[test]
    fn checkboxes_insert_toggle_and_undo() {
        let mut w = Workbook::new();
        w.set_input(0, CellRef::new(1, 0), "TRUE");
        w.set_input(0, CellRef::new(2, 0), "7");
        w.insert_checkboxes(0, &Range::new(CellRef::new(0, 0), CellRef::new(3, 0)));
        let v = |w: &Workbook, r| w.sheets[0].get(CellRef::new(r, 0)).map(|c| c.value().clone());
        assert_eq!(v(&w, 0), Some(Value::Bool(false)));
        assert_eq!(v(&w, 1), Some(Value::Bool(true)));
        assert_eq!(v(&w, 2), Some(Value::Number(7.0)));
        assert!(w.is_checkbox(0, CellRef::new(0, 0)) && !w.is_checkbox(0, CellRef::new(2, 0)));
        // One step to undo.
        assert!(w.undo());
        assert_eq!(v(&w, 0), None);
        assert!(w.redo());
        assert!(w.toggle_checkboxes(0, &[CellRef::new(0, 0)]));
        assert_eq!(v(&w, 0), Some(Value::Bool(true)));
        // Mixed: all become checked; then all unchecked.
        w.set_input(0, CellRef::new(1, 0), "FALSE");
        let all = [CellRef::new(0, 0), CellRef::new(1, 0), CellRef::new(2, 0)];
        assert!(w.toggle_checkboxes(0, &all));
        assert_eq!((v(&w, 0), v(&w, 1)), (Some(Value::Bool(true)), Some(Value::Bool(true))));
        assert!(w.toggle_checkboxes(0, &all));
        assert_eq!((v(&w, 0), v(&w, 1)), (Some(Value::Bool(false)), Some(Value::Bool(false))));
        w.undo();
        assert_eq!(v(&w, 0), Some(Value::Bool(true)));
        // Clear Formats takes the box away and keeps the value.
        w.modify_style(0, &Range::single(CellRef::new(0, 0)), |s| *s = CellStyle::default());
        assert!(!w.is_checkbox(0, CellRef::new(0, 0)));
        assert!(!w.toggle_checkboxes(0, &[CellRef::new(0, 0)]));
    }

    #[test]
    fn place_in_cell_and_back() {
        let mut w = Workbook::new();
        let mut img = SheetImage::new(Arc::from(PNG_4X2), ImageFormat::Png, CellRef::new(1, 1), 40.0, 20.0);
        img.alt = "logo".into();
        let id = w.add_image(0, img);
        let c = w.place_in_cell(0, id).unwrap();
        assert_eq!(c, CellRef::new(1, 1));
        assert!(w.sheets[0].images.is_empty());
        let cell = w.sheets[0].get(c).unwrap();
        assert_eq!(cell.picture().map(|p| p.alt.as_str()), Some("logo"));
        assert_eq!(cell.value(), &Value::Error(ErrorKind::Value));
        // Fitted: 4x2 in a wider cell keeps 2:1.
        let [_, _, fw, fh] = cell.picture().unwrap().fit(100.0, 20.0);
        assert!((fw / fh - 2.0).abs() < 1e-4);
        let id2 = w.place_over_cells(0, c).unwrap();
        assert!(w.sheets[0].get(c).is_none_or(|x| x.picture().is_none()));
        assert_eq!(w.sheets[0].image(id2).map(|i| i.alt.as_str()), Some("logo"));
        w.undo();
        assert!(w.sheets[0].get(c).unwrap().picture().is_some() && w.sheets[0].images.is_empty());
        w.undo();
        assert_eq!(w.sheets[0].images.len(), 1);
        // Typing over the picture drops it.
        w.place_in_cell(0, id);
        w.set_input(0, c, "x");
        assert!(w.sheets[0].get(c).unwrap().extra.is_none());
    }
}
