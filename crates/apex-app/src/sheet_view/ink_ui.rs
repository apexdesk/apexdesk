//! Drawing by hand over the cells (the Draw tab, see [`crate::ink`]):
//! strokes in sheet pixels join the ink object drawn just before when
//! close to it, or make a new one anchored to the cell under it; the
//! eraser takes out whole strokes and the lasso gathers strokes into an
//! object of their own, selected to be moved or deleted like any shape.
//! Every change is one undo step.

use std::sync::Arc;

use apex_doc::{Ink, InkStroke, Shape};
use apex_sheet::{CellRef, SheetShape};
use eframe::egui::{self, CursorIcon, Event, Key, Pos2, Rect, pos2, vec2};

use super::images_ui::{pane_at, to_screen};
use super::{Geo, SheetView};
use crate::ink::{self, Mode};
use crate::toolstrip::Body;
use crate::shapes::PX_PT;

/// How close (points) a stroke must come to the ink drawn before it to
/// join it.
const JOIN_PT: f32 = 28.0;
/// How far (screen pixels) the eraser reaches besides the stroke width.
const ERASER_PX: f32 = 4.0;

fn rect_of(s: &SheetShape, sh: &apex_sheet::Sheet) -> Rect {
    let [x, y, w, h] = s.rect(sh);
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

fn plain(s: &SheetShape) -> bool {
    s.shape.ink.is_some() && s.shape.rotation == 0.0 && !s.shape.flip_h && !s.shape.flip_v
}

impl SheetView {
    /// Mouse handling while a Draw tool is on; true when it took the
    /// event.
    pub(super) fn ink_pointer(&mut self, ui: &egui::Ui, resp: &egui::Response, g: &Geo, p: Pos2, pressed: bool, down: bool) -> bool {
        let Some(mode) = self.images.shapes.ink.mode else { return false };
        // The lasso's pick being dragged as a shape.
        if self.images.shapes.dragging() {
            return false;
        }
        let busy = self.images.shapes.ink.busy();
        let over = resp.hovered() && g.view.contains(p);
        if !busy && over {
            let on_picked = mode == Mode::Lasso && self.lasso_target(g, p);
            if on_picked {
                return false;
            }
            ui.ctx().set_cursor_icon(if mode == Mode::Eraser { CursorIcon::Cell } else { CursorIcon::Crosshair });
        }
        let pane = if busy { self.images.shapes.ink_pane } else { pane_at(g, p) };
        let (trail, force) = ink::trail(ui.ctx());
        let pts: Vec<Pos2> = trail.iter().map(|q| g.screen_to_point(pane, *q)).collect();
        let cur = g.screen_to_point(pane, p);
        let start = pressed && over;
        if start {
            if self.edit.is_some() && !self.settle_edit() {
                return true;
            }
            resp.request_focus();
            self.images.shapes.ink_pane = pane;
            self.images.shapes.selected = None;
        }
        let st = &mut self.images.shapes.ink;
        match mode {
            Mode::Draw(_) => {
                if st.drawing() {
                    st.extend(&pts, force);
                    if !down {
                        if let Some(s) = st.finish(PX_PT) {
                            self.add_sheet_stroke(s);
                        }
                    }
                    ui.ctx().request_repaint();
                    return true;
                }
                if start {
                    st.begin(cur, force);
                    return true;
                }
            }
            Mode::Eraser => {
                if st.erasing() {
                    for q in &pts {
                        self.sheet_erase_at(g, *q);
                    }
                    if !down {
                        self.finish_sheet_erase();
                    }
                    ui.ctx().request_repaint();
                    return true;
                }
                if start {
                    st.erase_begin();
                    self.sheet_erase_at(g, cur);
                    return true;
                }
            }
            Mode::Lasso => {
                if st.lassoing() {
                    for q in &pts {
                        st.lasso_push(*q);
                    }
                    if !down {
                        let l = st.take_lasso();
                        self.sheet_lasso(&l);
                    }
                    ui.ctx().request_repaint();
                    return true;
                }
                if start {
                    st.lasso_push(cur);
                    return true;
                }
            }
        }
        over && (pressed || down)
    }

    /// The pointer is on the selected shape (the lasso's pick, to drag).
    fn lasso_target(&self, g: &Geo, p: Pos2) -> bool {
        let Some(s) = self.selected_shape() else { return false };
        let sh = self.sh();
        let q = g.screen_to_point(pane_at(g, p), p);
        rect_of(s, sh).expand(crate::shapes::GRAB / g.z).contains(q)
    }

    fn add_sheet_stroke(&mut self, s: InkStroke) {
        let sheet = self.sheet;
        let sh = self.sh();
        let Some((_, b0)) = Ink::from_abs(vec![s.clone()], PX_PT) else { return };
        let near = Rect::from_min_size(pos2(b0[0], b0[1]), vec2(b0[2], b0[3])).expand(JOIN_PT * PX_PT);
        let last = self.images.shapes.ink_last.and_then(|id| sh.shapes.iter().find(|x| x.id == id)).filter(|x| plain(x) && rect_of(x, sh).intersects(near));
        if let Some(old) = last {
            let Some(i) = &old.shape.ink else { return };
            let Some((ink, nb)) = i.with_stroke(old.rect(sh), s, PX_PT) else { return };
            let mut n = old.clone();
            n.shape.ink = Some(Arc::new(ink));
            (n.width, n.height) = (nb[2], nb[3]);
            n.set_pos(sh, nb[0], nb[1]);
            self.wb.set_shape(sheet, n);
            return;
        }
        let Some((ink, nb)) = Ink::from_abs(vec![s], PX_PT) else { return };
        let mut n = SheetShape::new(Shape::ink(ink), CellRef::new(0, 0), nb[2], nb[3]);
        n.set_pos(sh, nb[0], nb[1]);
        n.anchor = apex_sheet::Anchor::OneCell;
        let id = self.wb.add_shape(sheet, n);
        self.images.shapes.ink_last = Some(id);
    }

    fn sheet_erase_at(&mut self, g: &Geo, q: Pos2) {
        let sh = &self.wb.sheets[self.sheet];
        let st = &mut self.images.shapes.ink;
        for s in &sh.shapes {
            let Some(i) = &s.shape.ink else { continue };
            let hits = ink::strokes_near(&s.shape, rect_of(s, sh), q, ERASER_PX / g.z, PX_PT);
            if !hits.is_empty() {
                st.erase(i, &hits);
            }
        }
    }

    /// The shape without strokes `idx`, its box shrunk to what is left;
    /// None when nothing is.
    fn sheet_without(&self, s: &SheetShape, idx: &[usize]) -> Option<SheetShape> {
        let i = s.shape.ink.as_ref()?;
        let rest = ink::without(i, idx.to_vec());
        if rest.strokes.is_empty() {
            return None;
        }
        let sh = self.sh();
        let mut n = s.clone();
        if plain(s) {
            let keep: Vec<usize> = (0..i.strokes.len()).filter(|k| !idx.contains(k)).collect();
            if let Some((fit, nb)) = Ink::from_abs(ink::absolute(&s.shape, i, rect_of(s, sh), &keep), PX_PT) {
                n.shape.ink = Some(Arc::new(fit));
                (n.width, n.height) = (nb[2], nb[3]);
                n.set_pos(sh, nb[0], nb[1]);
                return Some(n);
            }
        }
        n.shape.ink = Some(Arc::new(rest));
        Some(n)
    }

    fn finish_sheet_erase(&mut self) {
        let gone = self.images.shapes.ink.take_erased();
        if gone.is_empty() {
            return;
        }
        let list: Vec<SheetShape> = self
            .sh()
            .shapes
            .iter()
            .filter_map(|s| {
                let idx = s.shape.ink.as_ref().and_then(|i| gone.iter().find(|(k, _)| *k == ink::ink_key(i)));
                match idx {
                    Some((_, idx)) => self.sheet_without(s, idx),
                    None => Some(s.clone()),
                }
            })
            .collect();
        self.wb.set_shapes(self.sheet, list, "undo.delete_shape");
    }

    /// Picks the strokes inside the lasso (sheet pixels): a whole object is
    /// selected as it is, strokes of several objects or parts of one go
    /// into an object of their own.
    fn sheet_lasso(&mut self, lasso: &[Pos2]) {
        let sh = self.sh();
        let picked: Vec<(SheetShape, Vec<usize>)> = sh
            .shapes
            .iter()
            .filter(|s| s.shape.ink.is_some())
            .filter_map(|s| {
                let idx = ink::strokes_in(&s.shape, rect_of(s, sh), lasso);
                (!idx.is_empty()).then(|| (s.clone(), idx))
            })
            .collect();
        match &picked[..] {
            [] => return,
            [(s, idx)] if s.shape.ink.as_ref().is_some_and(|i| i.strokes.len() == idx.len()) => {
                self.select_shape(s.id);
                return;
            }
            _ => {}
        }
        let mut strokes = Vec::new();
        for (s, idx) in &picked {
            if let Some(i) = &s.shape.ink {
                strokes.extend(ink::absolute(&s.shape, i, rect_of(s, sh), idx));
            }
        }
        let Some((fit, nb)) = Ink::from_abs(strokes, PX_PT) else { return };
        let mut n = SheetShape::new(Shape::ink(fit), CellRef::new(0, 0), nb[2], nb[3]);
        n.set_pos(sh, nb[0], nb[1]);
        n.anchor = apex_sheet::Anchor::OneCell;
        n.shape.name = format!("Ink {}", sh.shapes.iter().map(|x| x.id).max().unwrap_or(0) + 1);
        let mut list: Vec<SheetShape> = Vec::new();
        for s in &sh.shapes {
            match picked.iter().find(|(p, _)| p.id == s.id) {
                Some((_, idx)) => list.extend(self.sheet_without(s, idx)),
                None => list.push(s.clone()),
            }
        }
        list.push(n);
        let ids = self.wb.set_shapes(self.sheet, list, "undo.shape");
        if let Some(id) = ids.first() {
            self.select_shape(*id);
        }
        self.images.shapes.ink_last = None;
    }

    /// Escape drops what is being drawn (while the pointer is down), else
    /// the tool.
    pub(super) fn ink_keys(&mut self, ctx: &egui::Context, events: &[Event]) -> bool {
        let st = &mut self.images.shapes.ink;
        if !st.active() || !events.iter().any(|e| matches!(e, Event::Key { key: Key::Escape, pressed: true, .. })) {
            return false;
        }
        if st.busy() && ctx.input(|i| i.pointer.any_down()) {
            st.cancel();
        } else {
            st.set_mode(None);
        }
        true
    }

    /// The stroke or lasso being drawn.
    pub(super) fn paint_ink_live(&self, painter: &egui::Painter, g: &Geo) {
        let st = &self.images.shapes;
        if !st.ink.busy() {
            return;
        }
        let sh = self.sh();
        let pane = st.ink_pane;
        st.ink.paint_live(&painter.with_clip_rect(g.view), |q| to_screen(g, sh, pane.0, pane.1, q.x, q.y), PX_PT * g.z);
    }

    pub(super) fn draw_tab(&mut self, b: &mut Body) {
        ink::tab(b, &mut self.images.shapes.ink, None);
        let sh = &self.wb.sheets[self.sheet];
        let st = &mut self.images.shapes;
        st.ink_last = st.ink_last.filter(|id| sh.shapes.iter().any(|x| x.id == *id && x.shape.is_ink()));
    }
}
