//! Shapes and text boxes in .ods: anchored to a cell (inside the
//! `table:table-cell`) or to the sheet (inside `table:shapes`), placed like
//! pictures (see [`super::images`]).

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_sheet::{Anchor, CellRef, FormControl, MAX_COLS, MAX_ROWS, Sheet, SheetShape};

use super::images::{self, RawFrame, SX, SY, cell_address, len};
use crate::drawingml::{Node, read_node};
use crate::odf_shape::{self, Place, Styles};
use crate::xml::{Reader, Tag, esc};

const MAX_SHAPES: usize = 10_000;

/// A shape as read, placed once the sheet's geometry is known.
pub(super) struct RawShape {
    sheet: usize,
    cell: Option<CellRef>,
    resize: bool,
    node: Node,
}

/// Whether an element of a cell or of `table:shapes` is a drawing object
/// (a frame or a shape).
pub(super) fn is_object(name: &str) -> bool {
    name == "draw:frame" || name == "draw:control" || odf_shape::is_shape(name)
}

/// Reads a drawing object whose start (or empty tag) was just returned:
/// pictures and charts go to `frames`, shapes and text boxes to `shapes`.
pub(super) fn read(r: &mut Reader, t: &Tag, empty: bool, sheet: usize, cell: Option<CellRef>, frames: &mut Vec<RawFrame>, shapes: &mut Vec<RawShape>) {
    let resize = t.attr("table:end-cell-address").is_some();
    if t.name != "draw:frame" {
        let node = read_node(r, t, empty);
        shapes.push(RawShape { sheet, cell, resize, node });
        return;
    }
    if empty {
        return;
    }
    let raw = r.raw_element();
    let mut sub = Reader::new(raw);
    if odf_shape::frame_is_text_box(raw) {
        let node = read_node(&mut sub, t, false);
        shapes.push(RawShape { sheet, cell, resize, node });
    } else if let Some(f) = images::read_frame(&mut sub, t, sheet, cell) {
        frames.push(f);
    }
}

/// Turns the shapes read into shapes of their sheets.
pub(super) fn resolve(list: Vec<RawShape>, st: &Styles, sheets: &mut [Sheet], forms: &mut HashMap<(usize, String), FormControl>) {
    let mut read: Vec<(usize, Option<CellRef>, bool, odf_shape::Raw, Option<FormControl>)> = list
        .into_iter()
        .filter_map(|s| {
            if s.node.local() != "control" {
                return Some((s.sheet, s.cell, s.resize, odf_shape::read(&s.node, st)?, None));
            }
            // A form control: its box here, its settings in the forms.
            let c = forms.remove(&(s.sheet, s.node.attr("draw:control")?.to_string()))?;
            let n = &s.node;
            let len = |k: &str| n.attr(k).and_then(crate::parse_length_pt).filter(|v| v.is_finite()).unwrap_or(0.0);
            let z = n.attr("draw:z-index").and_then(|v| v.parse().ok()).unwrap_or(0);
            let raw = odf_shape::Raw { shape: apex_doc::Shape::new(apex_doc::ShapeKind::Rect), x: len("svg:x"), y: len("svg:y"), w: len("svg:width"), h: len("svg:height"), z, style: None };
            Some((s.sheet, s.cell, s.resize, raw, Some(c)))
        })
        .collect();
    read.sort_by_key(|r| (r.0, r.3.z));
    for (si, cell, resize, raw, control) in read {
        let Some(sh) = sheets.get_mut(si) else { continue };
        if sh.shapes.len() >= MAX_SHAPES {
            continue;
        }
        // Points to LibreOffice pixels.
        let (x, y, w, h) = (raw.x / 0.75, raw.y / 0.75, raw.w / 0.75, raw.h / 0.75);
        let (ox, oy) = cell.map_or((0.0, 0.0), |c| (sh.col_x(c.col) / SX, sh.row_y(c.row) / SY));
        let mut s = SheetShape::new(raw.shape, CellRef::new(0, 0), w, h);
        s.anchor = match (cell, resize) {
            (None, _) => Anchor::Absolute,
            (Some(_), true) => Anchor::TwoCell,
            (Some(_), false) => Anchor::OneCell,
        };
        s.set_pos(sh, (ox + x) * SX, (oy + y) * SY);
        s.control = control.map(Box::new);
        s.sanitize();
        s.id = sh.shapes.len() as u32 + 1;
        sh.shapes.push(s);
    }
}

/// The last row a shape reaches down to (see [`images::bottom_row`]).
pub(super) fn bottom_row(sh: &Sheet) -> u32 {
    sh.shapes.iter().map(|s| s.pos(sh).1 + s.height.max(s.width) * SY).filter(|y| y.is_finite()).map(|y| sh.row_at(y.max(0.0))).max().unwrap_or(0)
}

/// Writes the shapes of a sheet: into `shapes` (anchored to the sheet) or,
/// by cell, into `cells`. `z` is the first z-index.
pub(super) fn frames(sh: &Sheet, si: usize, w: &mut odf_shape::Writer, z: usize, shapes: &mut String, cells: &mut HashMap<(u32, u32), String>) {
    for (k, s) in sh.shapes.iter().enumerate() {
        let (px, py) = s.pos(sh);
        // Top-left in LibreOffice pixels, before turning.
        let (lx, ly) = (px / SX, py / SY);
        let (sw, shh) = (s.width, s.height);
        let cell = match s.anchor {
            Anchor::Absolute => None,
            _ => Some(s.from),
        };
        let (ox, oy) = cell.map_or((0.0, 0.0), |c| (sh.col_x(c.col) / SX, sh.row_y(c.row) / SY));
        let mut attrs = String::new();
        if let (Some(_), Anchor::TwoCell) = (cell, s.anchor) {
            let (ex, ey) = ((lx + sw) * SX, (ly + shh) * SY);
            let end = CellRef::new(sh.row_at(ey.max(0.0)).min(MAX_ROWS - 1), sh.col_at(ex.max(0.0)).min(MAX_COLS - 1));
            let _ = write!(
                attrs,
                r#"table:end-cell-address="{}" table:end-x="{}" table:end-y="{}""#,
                esc(&cell_address(&sh.name, end)),
                len((ex - sh.col_x(end.col)).max(0.0) / SX),
                len((ey - sh.row_y(end.row)).max(0.0) / SY)
            );
        }
        let mut out = String::new();
        let place = Place { x: (lx - ox) * 0.75, y: (ly - oy) * 0.75, w: sw * 0.75, h: shh * 0.75, attrs: &attrs, graphic: "", z: (z + k) as i64 };
        match &s.control {
            Some(_) => {
                let _ = write!(
                    out,
                    r#"<draw:control draw:z-index="{}" draw:control="{}" svg:x="{}" svg:y="{}" svg:width="{}" svg:height="{}" {attrs}/>"#,
                    place.z,
                    super::controls::control_id(si, s.id),
                    super::fmt_len(place.x),
                    super::fmt_len(place.y),
                    super::fmt_len(place.w),
                    super::fmt_len(place.h)
                );
            }
            None => w.write(&mut out, &s.shape, &place),
        }
        match cell {
            Some(c) => cells.entry((c.row, c.col)).or_default().push_str(&out),
            None => shapes.push_str(&out),
        }
    }
}
