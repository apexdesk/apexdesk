//! View > Split: up to four panes that scroll on their own, with draggable
//! split bars. As in Excel, the panes of a row share the vertical position
//! and the panes of a column share the horizontal one.

use super::*;

const BAR: f32 = 6.0;

/// Which headers and scroll bars the pane being drawn shows: as in Excel,
/// row headers only left, column headers only on top and scroll bars
/// only right and at the bottom.
#[derive(Clone, Copy)]
pub(super) struct Chrome {
    pub row_hdr: bool,
    pub col_hdr: bool,
    pub vbar: bool,
    pub hbar: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Chrome { row_hdr: true, col_hdr: true, vbar: true, hbar: true }
    }
}

#[derive(Default)]
pub(super) struct Split {
    /// Bar positions from the top left of the grid area, in points.
    pub x: Option<f32>,
    pub y: Option<f32>,
    /// Scroll of the left/right columns and the top/bottom rows of panes.
    pub xs: [f32; 2],
    pub ys: [f32; 2],
    /// Bar being dragged: 1 vertical, 2 horizontal, 3 both.
    drag: u8,
    /// Split requested from the menu, placed on the next frame.
    pending: bool,
    /// What the pane being drawn shows.
    pub chrome: Chrome,
    /// Screen area of the panes in the last frame.
    pub area: Option<Rect>,
}

impl Split {
    fn on(&self) -> bool {
        self.x.is_some() || self.y.is_some() || self.pending
    }
}

/// The rectangles of the four panes (empty when that pane is absent).
fn pane_rects(area: Rect, x: Option<f32>, y: Option<f32>) -> [Rect; 4] {
    let mx = x.map(|x| area.min.x + x);
    let my = y.map(|y| area.min.y + y);
    let (l, r) = match mx {
        Some(m) => ((area.min.x, m - BAR / 2.0), Some((m + BAR / 2.0, area.max.x))),
        None => ((area.min.x, area.max.x), None),
    };
    let (t, b) = match my {
        Some(m) => ((area.min.y, m - BAR / 2.0), Some((m + BAR / 2.0, area.max.y))),
        None => ((area.min.y, area.max.y), None),
    };
    let mk = |h: Option<(f32, f32)>, v: Option<(f32, f32)>| match (h, v) {
        (Some(h), Some(v)) => Rect::from_min_max(pos2(h.0, v.0), pos2(h.1, v.1)),
        _ => Rect::NOTHING,
    };
    [mk(Some(l), Some(t)), mk(r, Some(t)), mk(Some(l), b), mk(r, b)]
}

impl SheetView {
    /// A second window on the same workbook (View > New Window): it starts
    /// where this one is and gets the workbook when it becomes active.
    pub fn linked_window(&self) -> SheetView {
        let mut v = SheetView::new(Workbook::new());
        v.zoom = self.zoom;
        v.sheet = self.sheet;
        v.anchor = self.anchor;
        v.cursor = self.cursor;
        v.scroll = self.scroll;
        v.show_formulas = self.show_formulas;
        v
    }

    /// Hands the workbook to another window of it (each window keeps its
    /// own sheet, selection, scroll and pending edit).
    pub fn swap_workbook(&mut self, other: &mut SheetView) {
        std::mem::swap(&mut self.wb, &mut other.wb);
        for v in [self, other] {
            if !v.wb.sheets.is_empty() && v.sheet >= v.wb.sheets.len() {
                v.clamp_sheet();
            }
        }
    }

    /// Row and column at the top left of the scrolled cells (View Side by
    /// Side scrolls two windows by as many rows and columns).
    pub fn top_left_cell(&self) -> (u32, u32) {
        let sh = self.sh();
        (sh.row_at(self.scroll.y + 0.5), sh.col_at(self.scroll.x + 0.5))
    }

    pub fn set_top_left_cell(&mut self, row: u32, col: u32) {
        let sh = self.sh();
        self.scroll = vec2(sh.col_x(col), sh.row_y(row));
    }

    pub(super) fn split_on(&self) -> bool {
        self.f10.split.on()
    }

    pub(super) fn toggle_split(&mut self) {
        if self.split_on() {
            self.remove_split();
        } else {
            // Split and freeze panes exclude each other.
            if self.frozen() {
                self.freeze(0, 0);
            }
            self.f10.split = Split { pending: true, xs: [self.scroll.x; 2], ys: [self.scroll.y; 2], ..Default::default() };
        }
    }

    pub(super) fn remove_split(&mut self) {
        if !self.split_on() {
            return;
        }
        let s = &self.f10.split;
        let (i, j) = ((self.f10.pane / 2) as usize, (self.f10.pane % 2) as usize);
        self.scroll = vec2(s.xs[j], s.ys[i]);
        self.f10.split = Split::default();
        let focused = self.ctx.as_ref().is_some_and(|c| c.memory(|m| m.focused() == Some(self.grid_id())));
        self.f10.pane = 0;
        if focused {
            self.focus_grid = true;
        }
    }

    /// Side panels of these features (the PivotTable field list).
    pub(super) fn f10_panels(&mut self, ui: &mut egui::Ui) {
        self.pivot_panel(ui);
    }

    /// The grid split in panes; false when there is no split.
    pub(super) fn split_grid(&mut self, ui: &mut egui::Ui, full: Rect) -> bool {
        if !self.split_on() {
            return false;
        }
        let area = full;
        self.f10.split.area = Some(area);
        // The headers of the grid without a split (the left and top panes').
        let head = {
            self.f10.split.chrome = Chrome::default();
            let g = self.geo(area);
            g.origin - area.min
        };
        if std::mem::take(&mut self.f10.split.pending) {
            self.place_split(area);
        }
        {
            let s = &mut self.f10.split;
            s.x = s.x.map(|x| x.clamp(BAR, (area.width() - BAR).max(BAR)));
            s.y = s.y.map(|y| y.clamp(BAR, (area.height() - BAR).max(BAR)));
        }
        let rects = pane_rects(area, self.f10.split.x, self.f10.split.y);
        if !rects[self.f10.pane as usize % 4].is_positive() {
            self.f10.pane = 0;
        }
        let (ai, aj) = ((self.f10.pane / 2) as usize, (self.f10.pane % 2) as usize);
        // Scrolling done elsewhere (go to, find) moves the active pane.
        self.f10.split.xs[aj] = self.scroll.x;
        self.f10.split.ys[ai] = self.scroll.y;
        // A press in another pane makes it the active one.
        let (pos, pressed) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_pressed()));
        if pressed && self.drag == Drag::None && self.f10.split.drag == 0 {
            if let Some(p) = pos {
                let over_popup = ui.ctx().layer_id_at(p).is_some_and(|l| l.order != egui::Order::Background);
                if let Some(k) = (0..4u8).find(|k| rects[*k as usize].contains(p)).filter(|k| *k != self.f10.pane && !over_popup) {
                    let had = ui.memory(|m| m.focused() == Some(self.grid_id()));
                    self.f10.pane = k;
                    if had {
                        let id = self.grid_id();
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                }
            }
        }
        let active = self.f10.pane;
        let order = (0..4u8).filter(|k| *k != active).chain(std::iter::once(active));
        for k in order {
            let r = rects[k as usize];
            if !r.is_positive() {
                continue;
            }
            let (i, j) = ((k / 2) as usize, (k % 2) as usize);
            self.scroll = vec2(self.f10.split.xs[j], self.f10.split.ys[i]);
            let s = &self.f10.split;
            self.f10.split.chrome = Chrome { row_hdr: j == 0, col_hdr: i == 0, vbar: j == 1 || s.x.is_none(), hbar: i == 1 || s.y.is_none() };
            self.grid_pane(ui, r, r, k, k == active);
            self.f10.split.xs[j] = self.scroll.x;
            self.f10.split.ys[i] = self.scroll.y;
        }
        self.f10.split.chrome = Chrome::default();
        let (ai, aj) = ((active / 2) as usize, (active % 2) as usize);
        self.scroll = vec2(self.f10.split.xs[aj], self.f10.split.ys[ai]);
        self.split_bars(ui, area, head);
        true
    }

    /// Places a new split as Excel does: above and left of the active
    /// cell, only between columns when it is in the top row in view, only
    /// between rows when in the first column; at the top left cell in view
    /// (or out of view), in the middle of the view.
    fn place_split(&mut self, area: Rect) {
        let g = self.geo(area);
        let sh = self.sh();
        let top = CellRef::new(g.row_at(sh, g.origin.y + 1.0), g.col_at(sh, g.origin.x + 1.0));
        let c = self.cursor;
        let cr = g.cell_rect(sh, c);
        let shown = g.view.contains(cr.min + vec2(1.0, 1.0));
        let (row, col) = if !shown || c == top {
            let mid = g.view.center();
            (Some(g.row_at(sh, mid.y).max(top.row + 1)), Some(g.col_at(sh, mid.x).max(top.col + 1)))
        } else {
            ((c.row > top.row).then_some(c.row), (c.col > top.col).then_some(c.col))
        };
        let x = col.map(|col| g.x(sh, col) - area.min.x);
        let y = row.map(|row| g.y(sh, row) - area.min.y);
        // The right and bottom panes go on where the others end.
        let xs = [g.scroll.x, col.map_or(g.scroll.x, |col| sh.col_x(col))];
        let ys = [g.scroll.y, row.map_or(g.scroll.y, |row| sh.row_y(row))];
        let s = &mut self.f10.split;
        (s.x, s.y, s.xs, s.ys) = (x, y, xs, ys);
        // The active cell's pane is the active one.
        self.f10.pane = (row.is_some() as u8) * 2 + col.is_some() as u8;
        if !shown || c == top {
            self.f10.pane = 0;
        }
        self.scroll = vec2(s.xs[(self.f10.pane % 2) as usize], s.ys[(self.f10.pane / 2) as usize]);
    }

    /// The split bars; one dragged onto the headers or the scroll bar
    /// (the edges of the grid) goes away.
    fn split_bars(&mut self, ui: &mut egui::Ui, area: Rect, head: Vec2) {
        let s = &self.f10.split;
        let fill = if ui.visuals().dark_mode { Color32::from_gray(70) } else { Color32::from_gray(200) };
        let edge = if ui.visuals().dark_mode { Color32::from_gray(95) } else { Color32::from_gray(165) };
        let vbar = s.x.map(|x| Rect::from_center_size(pos2(area.min.x + x, area.center().y), vec2(BAR, area.height())));
        let hbar = s.y.map(|y| Rect::from_center_size(pos2(area.center().x, area.min.y + y), vec2(area.width(), BAR)));
        let p = ui.painter();
        for b in [vbar, hbar].into_iter().flatten() {
            p.rect_filled(b, 0.0, fill);
            p.rect_stroke(b, 0.0, Stroke::new(1.0, edge), StrokeKind::Inside);
        }
        let mut drag = self.f10.split.drag;
        let base = Id::new(("split_bar", self.uid));
        // The crossing drags both bars.
        let cross = vbar.zip(hbar).map(|(v, h)| v.intersect(h).expand(2.0));
        let mut delta = Vec2::ZERO;
        let mut released = false;
        // The crossing goes last, on top of the bars that cover it.
        for (n, r) in [(1u8, vbar), (2, hbar), (3, cross)] {
            let Some(r) = r else { continue };
            let resp = ui.interact(r, base.with(n), Sense::click_and_drag());
            let icon = match n {
                1 => CursorIcon::ResizeColumn,
                2 => CursorIcon::ResizeRow,
                _ => CursorIcon::Move,
            };
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(icon);
            }
            if resp.drag_started() {
                drag = n;
            }
            if resp.dragged() && drag == n {
                delta = resp.drag_delta();
            }
            if resp.drag_stopped() && drag == n {
                released = true;
            }
            if resp.double_clicked() {
                // Double clicking a bar removes it.
                match n {
                    1 => self.f10.split.x = None,
                    2 => self.f10.split.y = None,
                    _ => {
                        self.remove_split();
                        return;
                    }
                }
                self.after_bar_change();
                return;
            }
        }
        let s = &mut self.f10.split;
        s.drag = drag;
        if drag & 1 != 0 {
            s.x = s.x.map(|x| x + delta.x);
        }
        if drag & 2 != 0 {
            s.y = s.y.map(|y| y + delta.y);
        }
        if released {
            s.drag = 0;
            let (w, h) = (area.width(), area.height());
            if s.x.is_some_and(|x| x <= head.x + BAR || x >= w - SCROLLBAR - BAR) {
                s.x = None;
            }
            if s.y.is_some_and(|y| y <= head.y + BAR || y >= h - SCROLLBAR - BAR) {
                s.y = None;
            }
            self.after_bar_change();
        }
    }

    /// Keeps the active pane valid after a bar went away.
    fn after_bar_change(&mut self) {
        let s = &self.f10.split;
        if s.x.is_none() && s.y.is_none() {
            self.remove_split();
            return;
        }
        let (mut i, mut j) = (self.f10.pane / 2, self.f10.pane % 2);
        if s.x.is_none() {
            j = 0;
        }
        if s.y.is_none() {
            i = 0;
        }
        let k = i * 2 + j;
        if k != self.f10.pane {
            let focused = self.ctx.as_ref().is_some_and(|c| c.memory(|m| m.focused() == Some(self.grid_id())));
            self.f10.pane = k;
            if focused {
                self.focus_grid = true;
            }
        }
        let s = &mut self.f10.split;
        if s.x.is_none() {
            s.xs = [s.xs[j as usize]; 2];
        }
        if s.y.is_none() {
            s.ys = [s.ys[i as usize]; 2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panes() {
        let a = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let r = pane_rects(a, Some(100.0), None);
        assert!(r[0].is_positive() && r[1].is_positive() && !r[2].is_positive() && !r[3].is_positive());
        assert_eq!(r[0].max.x, 97.0);
        assert_eq!(r[1].min.x, 103.0);
        let r = pane_rects(a, Some(100.0), Some(150.0));
        assert!(r.iter().all(|r| r.is_positive()));
        assert_eq!(r[3].min, pos2(103.0, 153.0));
        let r = pane_rects(a, None, None);
        assert_eq!(r[0], a);
    }
}
