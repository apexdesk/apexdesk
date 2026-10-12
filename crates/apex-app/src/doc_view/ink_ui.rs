//! Drawing by hand on the pages (the Draw tab, see [`crate::ink`]): a
//! stroke joins the ink object drawn just before when it is close to it,
//! or starts a new one floating in front of the text and anchored to the
//! paragraph under it; the eraser takes out whole strokes and the lasso
//! gathers strokes into an object of their own, selected to be moved or
//! deleted like any shape. Every change is one undo step.

use std::sync::Arc;

use apex_doc::{DocShape, Ink, InkStroke, Pos, Selection, Shape};
use eframe::egui::{self, CursorIcon, Event, Key, Pos2, Rect, pos2, vec2};

use super::shapes_ui::Boxed;
use super::{DocView, PT};
use crate::ink::{self, Mode, TabAction};
use crate::toolstrip::Body;

/// How close (points) a stroke must come to the ink drawn before it to
/// join it.
const JOIN_PT: f32 = 28.0;
/// How far (screen pixels) the eraser reaches besides the stroke width.
const ERASER_PX: f32 = 4.0;

/// Box of absolute strokes, as `[x, y, w, h]`, turned into a rect.
fn rect_of(b: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(b[0], b[1]), vec2(b[2], b[3]))
}

/// Whether the ink of a box may take more strokes in place: shown as
/// stored (no turn or flip), floating.
fn plain(b: &Boxed) -> bool {
    let s = &b.shape.shape;
    s.ink.is_some() && s.rotation == 0.0 && !s.flip_h && !s.flip_v && b.shape.wrap.floats()
}

impl DocView {
    fn ink_unit(&self) -> f32 {
        PT * self.zoom
    }

    fn ink_boxes(&self) -> Vec<Boxed> {
        self.all_boxes().into_iter().filter(|b| b.shape.shape.ink.is_some()).collect()
    }

    pub(super) fn ink_cursor(&self, local: Pos2) -> Option<CursorIcon> {
        match self.ink.mode? {
            Mode::Lasso if self.selected_shape().is_some_and(|b| b.rect.expand(crate::shapes::GRAB).contains(local)) => None,
            Mode::Eraser => Some(CursorIcon::Cell),
            _ => Some(CursorIcon::Crosshair),
        }
    }

    /// Mouse handling while a Draw tool is on; true when it took the
    /// event. `origin`: the canvas on screen.
    pub(super) fn ink_mouse(&mut self, ui: &egui::Ui, resp: &egui::Response, origin: Pos2) -> bool {
        let Some(mode) = self.ink.mode else { return false };
        // The lasso's pick being dragged as a shape.
        if self.shapes.busy() {
            return false;
        }
        let (pressed, down) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down()));
        let (trail, force) = ink::trail(ui.ctx());
        let local: Vec<Pos2> = trail.iter().map(|p| *p - origin.to_vec2()).collect();
        let Some(&cur) = local.last() else { return self.ink.busy() };
        let start = pressed && resp.hovered();
        match mode {
            Mode::Draw(_) => {
                if self.ink.drawing() {
                    self.ink.extend(&local, force);
                    if !down {
                        let unit = self.ink_unit();
                        if let Some(s) = self.ink.finish(unit) {
                            self.add_stroke(s);
                        }
                    }
                    ui.ctx().request_repaint();
                    return true;
                }
                if start {
                    resp.request_focus();
                    let at = resp.interact_pointer_pos().map_or(cur, |p| p - origin.to_vec2());
                    self.ink.begin(at, force);
                    return true;
                }
            }
            Mode::Eraser => {
                if self.ink.erasing() {
                    for q in &local {
                        self.erase_at(*q);
                    }
                    if !down {
                        self.finish_erase();
                    }
                    ui.ctx().request_repaint();
                    return true;
                }
                if start {
                    resp.request_focus();
                    self.ink.erase_begin();
                    self.erase_at(cur);
                    return true;
                }
            }
            Mode::Lasso => {
                if self.ink.lassoing() {
                    for q in &local {
                        self.ink.lasso_push(*q);
                    }
                    if !down {
                        let l = self.ink.take_lasso();
                        self.lasso_pick(&l);
                    }
                    ui.ctx().request_repaint();
                    return true;
                }
                // On the picked strokes: they are dragged as a shape.
                if self.selected_shape().is_some_and(|b| b.rect.expand(crate::shapes::GRAB).contains(cur)) {
                    return false;
                }
                if start {
                    resp.request_focus();
                    self.ink.lasso_push(cur);
                    return true;
                }
            }
        }
        // Clicks with a tool on don't move the caret.
        resp.hovered() && (pressed || resp.clicked() || resp.dragged())
    }

    /// Adds a finished stroke (canvas points).
    fn add_stroke(&mut self, s: InkStroke) {
        let unit = self.ink_unit();
        let Some((_, b0)) = Ink::from_abs(vec![s.clone()], unit) else { return };
        let near = rect_of(b0).expand(JOIN_PT * unit);
        let last = self.ink_last.and_then(|at| self.ink_boxes().into_iter().find(|b| b.at == at)).filter(|b| plain(b) && b.rect.intersects(near));
        if let Some(b) = last {
            let Some(old) = &b.shape.shape.ink else { return };
            let Some((ink, nb)) = old.with_stroke([b.rect.min.x, b.rect.min.y, b.rect.width(), b.rect.height()], s, unit) else { return };
            let mut ds = (*b.shape).clone();
            ds.shape.ink = Some(Arc::new(ink));
            (ds.width, ds.height) = (nb[2] / unit, nb[3] / unit);
            ds.h_align = None;
            (ds.x, ds.y) = self.float_offsets(b.at.para, &ds, pos2(nb[0], nb[1]));
            ds.sanitize();
            self.doc.set_shape(b.at, ds);
            return;
        }
        let Some((ink, nb)) = Ink::from_abs(vec![s.clone()], unit) else { return };
        let first = s.points.first().map_or(pos2(nb[0], nb[1]), |p| pos2(p.0, p.1));
        let para = self.pos_at(first).para;
        let mut ds = DocShape::new(Shape::ink(ink), nb[2] / unit, nb[3] / unit);
        (ds.x, ds.y) = self.float_offsets(para, &ds, pos2(nb[0], nb[1]));
        ds.z = self.doc.shapes().iter().map(|(_, x)| x.z + 1).max().unwrap_or(0);
        ds.sanitize();
        let sel = self.doc.insert_shape(Selection::caret(Pos::new(para, 0)), ds);
        self.ink_last = Some(sel.start());
        // The caret stays out of the way, after the new object.
        self.set_sel(Selection::caret(sel.end()));
        self.scroll_to_caret = false;
    }

    fn erase_at(&mut self, q: Pos2) {
        let unit = self.ink_unit();
        for b in self.ink_boxes() {
            let hits = ink::strokes_near(&b.shape.shape, b.rect, q, ERASER_PX, unit);
            if let (false, Some(i)) = (hits.is_empty(), &b.shape.shape.ink) {
                self.ink.erase(i, &hits);
            }
        }
    }

    fn finish_erase(&mut self) {
        let gone = self.ink.take_erased();
        if gone.is_empty() {
            return;
        }
        let unit = self.ink_unit();
        let mut changes = Vec::new();
        for b in self.ink_boxes() {
            let Some(i) = &b.shape.shape.ink else { continue };
            let Some((_, idx)) = gone.iter().find(|(k, _)| *k == ink::ink_key(i)) else { continue };
            changes.push((b.at, self.without_strokes(&b, idx, unit)));
        }
        let sel = self.doc.change_ink(changes, Vec::new(), self.sel);
        self.set_sel(sel);
        self.scroll_to_caret = false;
    }

    /// The ink object of `b` without strokes `idx`, its box shrunk to what
    /// is left; None when nothing is.
    fn without_strokes(&self, b: &Boxed, idx: &[usize], unit: f32) -> Option<DocShape> {
        let i = b.shape.shape.ink.as_ref()?;
        let rest = ink::without(i, idx.to_vec());
        if rest.strokes.is_empty() {
            return None;
        }
        let mut ds = (*b.shape).clone();
        if plain(b) {
            let keep: Vec<usize> = (0..i.strokes.len()).filter(|k| !idx.contains(k)).collect();
            if let Some((fit, nb)) = Ink::from_abs(ink::absolute(&ds.shape, i, b.rect, &keep), unit) {
                ds.shape.ink = Some(Arc::new(fit));
                (ds.width, ds.height) = (nb[2] / unit, nb[3] / unit);
                ds.h_align = None;
                (ds.x, ds.y) = self.float_offsets(b.at.para, &ds, pos2(nb[0], nb[1]));
                ds.sanitize();
                return Some(ds);
            }
        }
        ds.shape.ink = Some(Arc::new(rest));
        Some(ds)
    }

    /// Picks the strokes inside the lasso: whole objects are selected as
    /// they are, parts of objects go into an object of their own.
    fn lasso_pick(&mut self, lasso: &[Pos2]) {
        let unit = self.ink_unit();
        let picked: Vec<(Boxed, Vec<usize>)> = self
            .ink_boxes()
            .into_iter()
            .filter_map(|b| {
                let idx = ink::strokes_in(&b.shape.shape, b.rect, lasso);
                (!idx.is_empty()).then_some((b, idx))
            })
            .collect();
        if picked.is_empty() {
            return;
        }
        let whole = |(b, idx): &(Boxed, Vec<usize>)| b.shape.shape.ink.as_ref().is_some_and(|i| i.strokes.len() == idx.len());
        if picked.iter().all(whole) && (picked.len() == 1 || picked.iter().all(|(b, _)| b.shape.wrap.floats())) {
            self.select_shape(picked[0].0.at);
            let rest: Vec<Pos> = picked[1..].iter().map(|(b, _)| b.at).collect();
            self.shapes.multi = (self.sel, rest);
            return;
        }
        let mut strokes = Vec::new();
        let mut changes = Vec::new();
        for (b, idx) in &picked {
            let Some(i) = &b.shape.shape.ink else { continue };
            strokes.extend(ink::absolute(&b.shape.shape, i, b.rect, idx));
            changes.push((b.at, self.without_strokes(b, idx, unit)));
        }
        let Some((fit, nb)) = Ink::from_abs(strokes, unit) else { return };
        let para = self.pos_at(pos2(nb[0], nb[1] + 1.0)).para;
        let mut ds = DocShape::new(Shape::ink(fit), nb[2] / unit, nb[3] / unit);
        (ds.x, ds.y) = self.float_offsets(para, &ds, pos2(nb[0], nb[1]));
        ds.z = self.doc.shapes().iter().map(|(_, x)| x.z + 1).max().unwrap_or(0);
        ds.sanitize();
        let sel = self.doc.change_ink(changes, vec![(para, ds)], self.sel);
        self.set_sel(sel);
        self.scroll_to_caret = false;
        self.ink_last = None;
    }

    /// Selects every ink object that floats (to move or delete them).
    fn select_all_ink(&mut self) {
        let all: Vec<Pos> = self.ink_boxes().into_iter().filter(|b| b.shape.wrap.floats()).map(|b| b.at).collect();
        let Some(first) = all.first() else { return };
        self.select_shape(*first);
        self.shapes.multi = (self.sel, all[1..].to_vec());
        self.focus = true;
    }

    /// Keys while a Draw tool is on: Escape drops what is being drawn
    /// (while the pointer is down), else the tool. True when the text
    /// must not see them.
    pub(super) fn ink_keys(&mut self, ui: &egui::Ui) -> bool {
        if !self.ink.active() {
            return false;
        }
        let esc = ui.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { key: Key::Escape, pressed: true, .. })));
        if esc {
            if self.ink.busy() && ui.input(|i| i.pointer.any_down()) {
                self.ink.cancel();
            } else {
                self.ink.set_mode(None);
            }
            return true;
        }
        false
    }

    /// The stroke or lasso being drawn.
    pub(super) fn paint_ink_live(&self, p: &egui::Painter, off: egui::Vec2) {
        self.ink.paint_live(p, |q| q + off, self.ink_unit());
    }

    pub(super) fn draw_tab(&mut self, b: &mut Body) {
        let any = self.ink_boxes().iter().any(|b| b.shape.wrap.floats());
        if ink::tab(b, &mut self.ink, Some(any)) == Some(TabAction::SelectAll) {
            self.select_all_ink();
        }
        if self.ink.active() {
            self.ink_last = self.ink_last.filter(|at| self.doc.shape_at(*at).is_some_and(|(_, s)| s.shape.is_ink()));
        }
    }
}
