//! Shapes and text boxes floating over the cells of a sheet, placed like
//! pictures (see [`crate::image`]).

use apex_doc::Shape;

use crate::image::Anchor;
use crate::workbook::Op;
use crate::{CellRef, MAX_COLS, MAX_ROWS, Sheet, Workbook};

/// A shape over the cells. Sizes and offsets are sheet pixels (96 dpi).
#[derive(Clone, Debug, PartialEq)]
pub struct SheetShape {
    /// Unique within its sheet (0 = not assigned yet).
    pub id: u32,
    pub shape: Shape,
    /// Cell holding the top-left corner and the offset inside it.
    pub from: CellRef,
    pub dx: f32,
    pub dy: f32,
    /// Size before rotation (a line may have no width or no height).
    pub width: f32,
    pub height: f32,
    pub anchor: Anchor,
    /// A form control drawn instead of the shape (see [`crate::form_control`]).
    pub control: Option<Box<crate::FormControl>>,
    /// The macro a click runs (`Macro1`, `Module1.Macro1`); empty for none.
    pub macro_name: String,
}

impl SheetShape {
    pub fn new(shape: Shape, from: CellRef, width: f32, height: f32) -> SheetShape {
        let mut s = SheetShape { id: 0, shape, from, dx: 0.0, dy: 0.0, width, height, anchor: Anchor::TwoCell, control: None, macro_name: String::new() };
        s.sanitize();
        s
    }

    /// Top-left corner (before rotation) in sheet pixels.
    pub fn pos(&self, sh: &Sheet) -> (f32, f32) {
        (sh.col_x(self.from.col) + self.dx, sh.row_y(self.from.row) + self.dy)
    }

    /// Moves the top-left corner to `(x, y)` sheet pixels, re-anchoring it
    /// to the cell there.
    pub fn set_pos(&mut self, sh: &Sheet, x: f32, y: f32) {
        let (x, y) = (sane(x, 0.0, f32::MAX, 0.0), sane(y, 0.0, f32::MAX, 0.0));
        let col = sh.col_at(x).min(MAX_COLS - 1);
        let row = sh.row_at(y).min(MAX_ROWS - 1);
        self.from = CellRef::new(row, col);
        self.dx = (x - sh.col_x(col)).max(0.0);
        self.dy = (y - sh.row_y(row)).max(0.0);
    }

    /// Unrotated rectangle: x, y, width, height.
    pub fn rect(&self, sh: &Sheet) -> [f32; 4] {
        let (x, y) = self.pos(sh);
        [x, y, self.width, self.height]
    }

    /// Bounding box of the rotated shape: min x, min y, max x, max y.
    pub fn bounds(&self, sh: &Sheet) -> [f32; 4] {
        let [x, y, w, h] = self.rect(sh);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (s, c) = self.shape.rotation.to_radians().sin_cos();
        let (hw, hh) = ((w / 2.0 * c).abs() + (h / 2.0 * s).abs(), (w / 2.0 * s).abs() + (h / 2.0 * c).abs());
        [cx - hw, cy - hh, cx + hw, cy + hh]
    }

    /// Last cell the shape covers.
    pub fn last_cell(&self, sh: &Sheet) -> CellRef {
        let [_, _, x1, y1] = self.bounds(sh);
        CellRef::new(sh.row_at((y1 - 0.5).max(0.0)).min(MAX_ROWS - 1), sh.col_at((x1 - 0.5).max(0.0)).min(MAX_COLS - 1))
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        let min = if self.shape.kind.is_line() { 0.0 } else { 1.0 };
        self.width = sane(self.width, min, 1.0e6, 100.0);
        self.height = sane(self.height, min, 1.0e6, 100.0);
        self.dx = sane(self.dx, 0.0, 1.0e7, 0.0);
        self.dy = sane(self.dy, 0.0, 1.0e7, 0.0);
        self.from = CellRef::new(self.from.row.min(MAX_ROWS - 1), self.from.col.min(MAX_COLS - 1));
        self.shape.sanitize();
        if let Some(c) = &mut self.control {
            c.sanitize();
        }
    }

    /// Moves the anchor for rows (or columns) inserted or deleted at `at`,
    /// as pictures and charts do.
    pub(crate) fn shift(&mut self, rows: bool, at: u32, count: i64) {
        if self.anchor == Anchor::Absolute {
            return;
        }
        let (v, max) = if rows { (self.from.row, MAX_ROWS - 1) } else { (self.from.col, MAX_COLS - 1) };
        if v < at {
            return;
        }
        let nv = if count >= 0 {
            (v as i64 + count).min(max as i64) as u32
        } else if (v as i64) < at as i64 - count {
            if rows {
                self.dy = 0.0;
            } else {
                self.dx = 0.0;
            }
            at
        } else {
            (v as i64 + count) as u32
        };
        if rows {
            self.from.row = nv;
        } else {
            self.from.col = nv;
        }
    }
}

fn sane(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

/// Where a shape goes in the stacking order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Forward,
    Backward,
    Front,
    Back,
}

impl Workbook {
    /// Adds a shape on top of the others (undoable); returns its id.
    pub fn add_shape(&mut self, sheet: usize, mut s: SheetShape) -> u32 {
        let sh = &self.sheets[sheet];
        s.id = sh.shapes.iter().map(|x| x.id).max().unwrap_or(0) + 1;
        if s.shape.name.is_empty() {
            let base = if s.shape.is_text_box() { "TextBox" } else { "Shape" };
            s.shape.name = format!("{base} {}", s.id);
        }
        s.sanitize();
        let id = s.id;
        let mut list = sh.shapes.clone();
        list.push(s);
        self.execute_as(Op::Shapes { sheet, list }, "undo.insert_shape");
        id
    }

    /// Replaces the shape with the same id (undoable).
    pub fn set_shape(&mut self, sheet: usize, mut s: SheetShape) {
        s.sanitize();
        let sh = &self.sheets[sheet];
        let Some(i) = sh.shapes.iter().position(|x| x.id == s.id) else { return };
        if sh.shapes[i] == s {
            return;
        }
        let mut list = sh.shapes.clone();
        list[i] = s;
        self.execute_as(Op::Shapes { sheet, list }, "undo.shape");
    }

    pub fn remove_shape(&mut self, sheet: usize, id: u32) {
        let mut list = self.sheets[sheet].shapes.clone();
        let n = list.len();
        list.retain(|x| x.id != id);
        if list.len() != n {
            self.execute_as(Op::Shapes { sheet, list }, "undo.delete_shape");
        }
    }

    /// Replaces the shapes of a sheet as one undo step (ink erased from
    /// several objects, strokes moved into one of their own); shapes with
    /// id 0 get a new one. Returns the ids given.
    pub fn set_shapes(&mut self, sheet: usize, mut list: Vec<SheetShape>, label: &'static str) -> Vec<u32> {
        let mut next = list.iter().map(|x| x.id).max().unwrap_or(0) + 1;
        let mut given = Vec::new();
        for s in &mut list {
            if s.id == 0 {
                s.id = next;
                next += 1;
                given.push(s.id);
            }
            s.sanitize();
        }
        if list != self.sheets[sheet].shapes {
            self.execute_as(Op::Shapes { sheet, list }, label);
        }
        given
    }

    /// Moves a shape in the stacking order.
    pub fn reorder_shape(&mut self, sheet: usize, id: u32, to: Order) {
        let mut list = self.sheets[sheet].shapes.clone();
        let Some(i) = list.iter().position(|x| x.id == id) else { return };
        let s = list.remove(i);
        let at = match to {
            Order::Front => list.len(),
            Order::Back => 0,
            Order::Forward => (i + 1).min(list.len()),
            Order::Backward => i.saturating_sub(1),
        };
        list.insert(at, s);
        if list != self.sheets[sheet].shapes {
            self.execute_as(Op::Shapes { sheet, list }, "undo.shape");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::ShapeKind;

    #[test]
    fn shapes_undo_order_and_structure() {
        let mut wb = Workbook::new();
        let a = wb.add_shape(0, SheetShape::new(Shape::new(ShapeKind::Rect), CellRef::new(5, 2), 120.0, 60.0));
        let b = wb.add_shape(0, SheetShape::new(Shape::new(ShapeKind::TextBox), CellRef::new(1, 1), 50.0, 20.0));
        assert_eq!(wb.sheets[0].shapes[0].shape.name, "Shape 1");
        assert_eq!(wb.sheets[0].shapes[1].shape.name, "TextBox 2");
        // Rows inserted above move it, like a chart.
        wb.execute(Op::InsertRows { sheet: 0, at: 2, count: 2 });
        assert_eq!(wb.sheets[0].shapes[0].from, CellRef::new(7, 2));
        assert_eq!(wb.sheets[0].shapes[1].from, CellRef::new(1, 1));
        wb.undo();
        assert_eq!(wb.sheets[0].shapes[0].from, CellRef::new(5, 2));
        // Deleting the anchor row moves it to the next row; undo restores it.
        let mut s = wb.sheets[0].shapes[0].clone();
        s.dy = 4.0;
        wb.set_shape(0, s);
        wb.execute(Op::DeleteRows { sheet: 0, at: 5, count: 1 });
        assert_eq!((wb.sheets[0].shapes[0].from.row, wb.sheets[0].shapes[0].dy), (5, 0.0));
        wb.undo();
        assert_eq!((wb.sheets[0].shapes[0].from.row, wb.sheets[0].shapes[0].dy), (5, 4.0));
        wb.reorder_shape(0, a, Order::Front);
        assert_eq!(wb.sheets[0].shapes[1].id, a);
        wb.reorder_shape(0, a, Order::Backward);
        assert_eq!(wb.sheets[0].shapes[0].id, a);
        wb.reorder_shape(0, a, Order::Back);
        assert_eq!(wb.sheets[0].shapes[0].id, a);
        wb.remove_shape(0, b);
        assert_eq!(wb.sheets[0].shapes.len(), 1);
        wb.undo();
        assert_eq!(wb.sheets[0].shapes.len(), 2);
        // Several shapes changed, removed and added as one step.
        let mut list = vec![wb.sheets[0].shapes[1].clone()];
        list.push(SheetShape::new(Shape::new(ShapeKind::Rect), CellRef::new(0, 0), 10.0, 10.0));
        let ids = wb.set_shapes(0, list, "undo.shape");
        assert_eq!(ids, vec![3]);
        assert_eq!(wb.sheets[0].shapes.iter().map(|s| s.id).collect::<Vec<_>>(), vec![b, 3]);
        wb.undo();
        assert_eq!(wb.sheets[0].shapes.len(), 2);
    }

    #[test]
    fn lines_and_hostile_values() {
        let sh = Sheet::new("S");
        let mut l = SheetShape::new(Shape::new(ShapeKind::Line), CellRef::new(0, 0), 80.0, 0.0);
        assert_eq!(l.height, 0.0);
        l.width = f32::NAN;
        l.dx = -3.0;
        l.shape.rotation = f32::INFINITY;
        l.sanitize();
        assert_eq!((l.width, l.dx, l.shape.rotation), (100.0, 0.0, 0.0));
        let mut r = SheetShape::new(Shape::new(ShapeKind::Rect), CellRef::new(0, 0), 100.0, 50.0);
        r.shape.rotation = 90.0;
        let b = r.bounds(&sh);
        assert!(((b[2] - b[0]) - 50.0).abs() < 1e-3);
    }
}
