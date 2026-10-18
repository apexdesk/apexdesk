//! Controls and pictures in cells: check boxes (painting, clicking and
//! Space) and pictures that are a cell's value (painting, Place in Cell,
//! Place over Cells and Insert > Picture in Cell).

use std::sync::Arc;

use apex_sheet::{CellPicture, CellRef, CellStyle, HAlign, ImageFormat, Range, VAlign};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

use super::{Geo, SheetView};
use crate::app::Notice;
use crate::i18n::{tr, trf};
use crate::images;
use crate::theme::{self, Paper};

/// Check box side at 100 % zoom, in sheet pixels.
const BOX: f32 = 13.0;

/// Where the check box of a cell shown in `rect` goes: by the cell's
/// alignment, centered unless it says otherwise.
pub(super) fn checkbox_rect(rect: Rect, st: &CellStyle, z: f32) -> Rect {
    let side = (BOX * z).min(rect.height() - 2.0).min(rect.width() - 2.0).max(1.0);
    let pad = 3.0 * z;
    let x = match st.halign {
        HAlign::Left => rect.min.x + pad,
        HAlign::Right => rect.max.x - pad - side,
        _ => rect.center().x - side / 2.0,
    };
    let y = match st.valign {
        VAlign::Top => rect.min.y + 2.0 * z,
        VAlign::Center => rect.center().y - side / 2.0,
        VAlign::Bottom => rect.max.y - 2.0 * z - side,
    };
    Rect::from_min_size(pos2(x, y.max(rect.min.y)), vec2(side, side))
}

/// Paints a check box in the cell's text color.
pub(super) fn paint_checkbox(p: &egui::Painter, rect: Rect, st: &CellStyle, checked: bool, z: f32) {
    let r = checkbox_rect(rect, st, z);
    let ink = st.color.map_or(Paper::TEXT, theme::rgb);
    let round = 2.0 * z;
    if checked {
        p.rect_filled(r, round, ink);
        let at = |x: f32, y: f32| pos2(r.min.x + x * r.width(), r.min.y + y * r.height());
        let mark = Stroke::new((r.width() / 7.0).max(1.2), Color32::WHITE);
        p.line_segment([at(0.22, 0.52), at(0.42, 0.72)], mark);
        p.line_segment([at(0.42, 0.72), at(0.78, 0.3)], mark);
    } else {
        p.rect_filled(r, round, Paper::BG);
        p.rect_stroke(r, round, Stroke::new((z * 1.2).max(1.0), ink.gamma_multiply(0.8)), StrokeKind::Inside);
    }
}

/// Paints a picture fitted to `rect` with its proportions. `wait`
/// decodes it now (printing) instead of showing a placeholder.
pub(super) fn paint_cell_picture(p: &egui::Painter, rect: Rect, pic: &CellPicture, wait: bool) {
    let r = rect.shrink(1.0);
    if r.width() < 1.0 || r.height() < 1.0 {
        return;
    }
    let [dx, dy, w, h] = pic.fit(r.width(), r.height());
    let at = Rect::from_min_size(r.min + vec2(dx, dy), vec2(w, h));
    let tex = images::texture(p.ctx(), images::key(&pic.data), &pic.data, wait);
    images::paint(p, tex.as_ref(), at, 0.0, [0.0; 4]);
}

impl SheetView {
    /// The screen rectangle of cell `c` (its merged area when merged).
    fn shown_cell_rect(&self, g: &Geo, c: CellRef) -> Rect {
        let sh = self.sh();
        match sh.merges.iter().find(|m| m.contains(c)) {
            Some(m) => g.range_rect(sh, m),
            None => g.cell_rect(sh, c),
        }
    }

    /// Whether `p` is on the check box shown in cell `c`.
    pub(super) fn on_checkbox(&self, g: &Geo, c: CellRef, p: Pos2) -> bool {
        if !self.wb.is_checkbox(self.sheet, c) || self.show_formulas {
            return false;
        }
        let c = self.sh().merges.iter().find(|m| m.contains(c)).map_or(c, |m| m.start);
        let st = self.wb.styles.get(self.sh().get(c).map_or(0, |x| x.style));
        checkbox_rect(self.shown_cell_rect(g, c), st, g.z).expand(2.0).contains(p)
    }

    /// A click on a check box selects its cell and toggles it.
    pub(super) fn checkbox_click(&mut self, g: &Geo, c: CellRef, p: Pos2) -> bool {
        if !self.on_checkbox(g, c, p) {
            return false;
        }
        self.select(c, false);
        if self.wb.cell_locked(self.sheet, c) {
            self.notices.push(Notice::error(tr("protect.blocked")));
        } else {
            self.wb.toggle_checkboxes(self.sheet, &[c]);
        }
        true
    }

    /// Space on a check box: toggles the check boxes of the selection.
    /// False (Space types) when the active cell has none.
    pub(super) fn checkbox_space(&mut self) -> bool {
        if !self.wb.is_checkbox(self.sheet, self.cursor) {
            return false;
        }
        let mut cells = Vec::new();
        for r in self.areas() {
            let Some(r) = self.wb.clamp_to_used(self.sheet, &r) else { continue };
            if r.cell_count() > 1 << 20 {
                continue;
            }
            self.sh().for_each_in(&r, |c, _| cells.push(c));
        }
        if !cells.contains(&self.cursor) {
            cells.push(self.cursor);
        }
        if cells.iter().any(|&c| self.wb.is_checkbox(self.sheet, c) && self.wb.cell_locked(self.sheet, c)) {
            self.notices.push(Notice::error(tr("protect.blocked")));
            return true;
        }
        self.wb.toggle_checkboxes(self.sheet, &cells);
        true
    }

    /// Insert > Checkbox on the selection.
    pub(super) fn insert_checkboxes(&mut self) {
        self.settle_edit();
        let areas = self.areas();
        if areas.iter().any(|r| !self.wb.can_edit(self.sheet, r)) {
            self.notices.push(Notice::error(tr("protect.blocked")));
            return;
        }
        let n = self.wb.undo_len();
        for r in &areas {
            self.wb.insert_checkboxes(self.sheet, r);
        }
        let steps = self.wb.undo_len() - n;
        if steps > 1 {
            self.wb.merge_undo(steps, "undo.checkbox");
        }
    }

    // --- pictures in cells ---------------------------------------------------------

    /// Place in Cell: the selected picture becomes the value of the cell
    /// under its top-left corner.
    pub(super) fn place_in_cell(&mut self) {
        let Some(img) = self.selected_image().cloned() else { return };
        if !self.wb.can_edit(self.sheet, &Range::single(img.from)) {
            self.notices.push(Notice::error(tr("protect.blocked")));
            return;
        }
        self.images.deselect_picture();
        if let Some(c) = self.wb.place_in_cell(self.sheet, img.id) {
            self.anchor = c;
            self.cursor = c;
        }
    }

    /// Place over Cells: the picture in the active cell floats over it.
    pub(super) fn place_over_cells(&mut self) {
        self.settle_edit();
        if self.sh().get(self.cursor).and_then(|c| c.picture()).is_none() {
            self.notices.push(Notice::info(tr("image.no_cell_picture")));
            return;
        }
        if let Some(id) = self.wb.place_over_cells(self.sheet, self.cursor) {
            self.images.selected = Some((self.sheet, id));
        }
    }

    /// Whether the active cell holds a picture.
    pub(super) fn cursor_has_picture(&self) -> bool {
        self.sh().get(self.cursor).is_some_and(|c| c.picture().is_some())
    }

    /// Insert > Picture in Cell: a picture file as the active cell's value.
    pub(super) fn insert_cell_picture_dialog(&mut self) {
        self.settle_edit();
        if !self.guard_edit() {
            return;
        }
        crate::file_dialog::FileDialog::new().set_title(tr("image.in_cell_file")).add_filter(tr("image.filter"), &super::images_ui::EXTENSIONS).pick_file(crate::file_dialog::in_view(|v: &mut SheetView, path: std::path::PathBuf| v.insert_cell_picture_file(&path)));
    }

    /// The picture file `path` as the active cell's value.
    fn insert_cell_picture_file(&mut self, path: &std::path::Path) {
        let data: Arc<[u8]> = match apex_fs::read(path) {
            Ok(b) => b.into(),
            Err(e) => {
                self.notices.push(Notice::error(trf("error.open", &[("error", &e.to_string())])));
                return;
            }
        };
        let Some(format) = ImageFormat::sniff(&data).filter(|_| images::decode(&data).is_some()) else {
            self.notices.push(Notice::error(tr("image.bad")));
            return;
        };
        let mut pic = CellPicture::new(data, format);
        pic.alt = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.wb.set_cell_picture(self.sheet, self.cursor, pic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkbox_follows_alignment() {
        let cell = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 20.0));
        let mut st = CellStyle::default();
        let r = checkbox_rect(cell, &st, 1.0);
        assert!((r.center().x - 50.0).abs() < 0.01 && r.width() == BOX && r.max.y <= 20.0);
        st.halign = HAlign::Left;
        assert!(checkbox_rect(cell, &st, 1.0).min.x < 10.0);
        st.halign = HAlign::Right;
        assert!(checkbox_rect(cell, &st, 1.0).max.x > 90.0);
        // A short row shrinks the box.
        let low = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 8.0));
        assert!(checkbox_rect(low, &st, 1.0).height() <= 6.0);
    }
}
