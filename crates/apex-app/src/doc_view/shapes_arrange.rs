//! Arranging floating shapes: picking several (Ctrl+click), lining them
//! up and spacing them out, and grouping them.

use std::collections::HashSet;

use apex_doc::{DocShape, Pos};
use eframe::egui::{self, Rect, Stroke, Vec2};

use super::DocView;
use super::shapes_ui::{ACCENT, Boxed};
use crate::i18n::tr;
use crate::shapes;

/// How shapes line up.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum LineUp {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
    SpreadH,
    SpreadV,
}

impl LineUp {
    const ALL: [LineUp; 8] = [LineUp::Left, LineUp::Center, LineUp::Right, LineUp::Top, LineUp::Middle, LineUp::Bottom, LineUp::SpreadH, LineUp::SpreadV];

    fn key(self) -> &'static str {
        match self {
            LineUp::Left => "arrange.left",
            LineUp::Center => "arrange.center",
            LineUp::Right => "arrange.right",
            LineUp::Top => "arrange.top",
            LineUp::Middle => "arrange.middle",
            LineUp::Bottom => "arrange.bottom",
            LineUp::SpreadH => "arrange.spread_h",
            LineUp::SpreadV => "arrange.spread_v",
        }
    }
}

/// The box around shapes.
fn union(bs: &[Boxed]) -> Rect {
    bs.iter().fold(Rect::NOTHING, |r, b| r.union(b.rect))
}

impl DocView {
    /// Shapes picked besides the selected one (Ctrl+click); forgotten when
    /// the selection changes.
    pub(super) fn picked_extra(&self) -> Vec<Pos> {
        let (sel, v) = &self.shapes.multi;
        if *sel != self.sel {
            return Vec::new();
        }
        v.iter().copied().filter(|p| self.doc.shape_at(*p).is_some()).collect()
    }

    /// Whether the shape at `at` is picked (selected, or Ctrl+clicked).
    pub(super) fn is_picked(&self, at: Pos) -> bool {
        self.selected_shape().is_some_and(|b| b.at == at) || self.picked_extra().contains(&at)
    }

    /// Ctrl+click on a shape: adds it to the picked shapes, or takes it
    /// out.
    pub(super) fn toggle_picked(&mut self, at: Pos) {
        let Some(cur) = self.selected_shape() else {
            self.select_shape(at);
            return;
        };
        let mut v = self.picked_extra();
        if at == cur.at {
            // The next picked shape becomes the selected one.
            if v.is_empty() {
                return;
            }
            let next = v.remove(0);
            self.select_shape(next);
        } else if let Some(i) = v.iter().position(|p| *p == at) {
            v.remove(i);
        } else {
            v.push(at);
        }
        self.shapes.multi = (self.sel, v);
    }

    /// The picked floating shapes, in units that move together (a group
    /// is one).
    pub(super) fn picked_units(&self) -> Vec<Vec<Boxed>> {
        let Some(sel) = self.selected_shape() else { return Vec::new() };
        let boxes = self.all_boxes();
        let mut seen = HashSet::new();
        let mut units = Vec::new();
        for at in std::iter::once(sel.at).chain(self.picked_extra()) {
            if seen.contains(&at) {
                continue;
            }
            let members: Vec<Boxed> = self.doc.shape_group(at).iter().filter_map(|(p, _)| boxes.iter().find(|b| b.at == *p).cloned()).collect();
            seen.extend(members.iter().map(|m| m.at));
            if !members.is_empty() && members.iter().all(|m| m.shape.wrap.floats()) {
                units.push(members);
            }
        }
        units
    }

    /// Moves each unit by its offset (canvas), as one undo step.
    fn move_units(&mut self, moves: Vec<(Vec<Boxed>, Vec2)>) {
        let mut changes = Vec::new();
        for (unit, d) in moves {
            if d.length() < 0.01 {
                continue;
            }
            for b in unit {
                let mut s = (*b.shape).clone();
                s.h_align = None;
                (s.x, s.y) = self.float_offsets(b.at.para, &s, b.rect.min + d);
                s.sanitize();
                changes.push((b.at, s));
            }
        }
        self.doc.change_shapes(changes, self.sel);
    }

    /// The text area of the page of the shape at `at` (canvas).
    fn margin_rect(&self, at: Pos) -> Option<Rect> {
        let page = self.layout.segs_of(at.para).first()?.page;
        let g = self.layout.geo_of(page);
        Some(Rect::from_min_size(self.page_min(page) + g.margin, g.content))
    }

    /// Lines up the picked shapes: with each other when there are several,
    /// else on the margins.
    pub(super) fn line_up(&mut self, how: LineUp) {
        let units = self.picked_units();
        let Some(first) = units.first().and_then(|u| u.first()) else { return };
        let rects: Vec<Rect> = units.iter().map(|u| union(u)).collect();
        let reference = if units.len() > 1 {
            rects.iter().fold(Rect::NOTHING, |a, r| a.union(*r))
        } else {
            match self.margin_rect(first.at) {
                Some(r) => r,
                None => return,
            }
        };
        let n = units.len();
        let mut offs = vec![Vec2::ZERO; n];
        let spread = matches!(how, LineUp::SpreadH | LineUp::SpreadV);
        if spread && n > 1 {
            // The first and the last stay; the gaps between all become equal.
            let horiz = how == LineUp::SpreadH;
            let size = |r: &Rect| if horiz { r.width() } else { r.height() };
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by(|&a, &b| {
                let k = |r: &Rect| if horiz { r.center().x } else { r.center().y };
                k(&rects[a]).total_cmp(&k(&rects[b]))
            });
            let total: f32 = rects.iter().map(size).sum();
            let span = size(&reference);
            let gap = (span - total) / (n - 1) as f32;
            let mut pos = if horiz { reference.min.x } else { reference.min.y };
            for i in order {
                let r = rects[i];
                if horiz {
                    offs[i].x = pos - r.min.x;
                } else {
                    offs[i].y = pos - r.min.y;
                }
                pos += size(&r) + gap;
            }
        } else {
            for (o, r) in offs.iter_mut().zip(&rects) {
                match how {
                    LineUp::Left => o.x = reference.min.x - r.min.x,
                    LineUp::Center | LineUp::SpreadH => o.x = reference.center().x - r.center().x,
                    LineUp::Right => o.x = reference.max.x - r.max.x,
                    LineUp::Top => o.y = reference.min.y - r.min.y,
                    LineUp::Middle | LineUp::SpreadV => o.y = reference.center().y - r.center().y,
                    LineUp::Bottom => o.y = reference.max.y - r.max.y,
                }
            }
        }
        self.move_units(units.into_iter().zip(offs).collect());
    }

    pub(super) fn can_group(&self) -> bool {
        self.picked_units().len() > 1
    }

    pub(super) fn can_ungroup(&self) -> bool {
        self.selected_shape().is_some_and(|b| b.shape.group.is_some())
    }

    /// Groups the picked shapes: they go to the paragraph of the first,
    /// placed from its frame.
    pub(super) fn group_picked(&mut self) {
        let mut members: Vec<Boxed> = self.picked_units().into_iter().flatten().collect();
        if members.len() < 2 {
            return;
        }
        members.sort_by_key(|b| b.at);
        let first = (*members[0].shape).clone();
        let to = members[0].at.para;
        let list = members
            .iter()
            .map(|b| {
                let mut s = DocShape { wrap: first.wrap, h_rel: first.h_rel, v_rel: first.v_rel, h_align: None, ..(*b.shape).clone() };
                (s.x, s.y) = self.float_offsets(to, &s, b.rect.min);
                s.sanitize();
                (b.at, s)
            })
            .collect();
        let at = self.doc.group_shapes(to, list);
        if let Some(a) = at.first() {
            self.select_shape(*a);
        }
        self.shapes.multi.1.clear();
    }

    pub(super) fn ungroup_selected(&mut self) {
        if let Some(b) = self.selected_shape() {
            self.doc.ungroup_shapes(b.at);
        }
    }

    /// Align, Group and Ungroup, for menus.
    pub(super) fn arrange_items(&mut self, ui: &mut egui::Ui) {
        crate::widgets::menu_button(ui, tr("arrange.align"), |ui| self.align_menu(ui));
        if ui.add_enabled(self.can_group(), egui::Button::new(tr("arrange.group"))).clicked() {
            self.group_picked();
            ui.close();
        }
        if ui.add_enabled(self.can_ungroup(), egui::Button::new(tr("arrange.ungroup"))).clicked() {
            self.ungroup_selected();
            ui.close();
        }
    }

    pub(super) fn align_menu(&mut self, ui: &mut egui::Ui) {
        let many = self.picked_units().len() > 1;
        ui.weak(tr(if many { "arrange.to_selection" } else { "arrange.to_margin" }));
        for (i, how) in LineUp::ALL.into_iter().enumerate() {
            if i == 3 || i == 6 {
                ui.separator();
            }
            if ui.button(tr(how.key())).clicked() {
                self.line_up(how);
                ui.close();
            }
        }
        ui.separator();
        ui.weak(tr("arrange.pick_hint"));
    }

    /// Handles of the other picked shapes, and a box around groups.
    pub(super) fn paint_picked(&self, p: &egui::Painter, off: Vec2, moved: Vec2) {
        let extra = self.picked_extra();
        for u in self.picked_units() {
            if u.len() > 1 {
                p.rect_stroke(union(&u).translate(off + moved).expand(4.0), 0.0, Stroke::new(1.0, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
            }
            for b in u.iter().filter(|b| extra.contains(&b.at)) {
                shapes::paint_handles(p, &b.shape.shape, b.rect.translate(off + moved), ACCENT);
            }
        }
    }

    /// The shapes that move with the one at `at` when it's dragged: the
    /// rest of its group and the other picked shapes.
    pub(super) fn moving_with(&self, at: Pos) -> Vec<Boxed> {
        let units = self.picked_units();
        if !units.iter().flatten().any(|b| b.at == at) {
            return Vec::new();
        }
        units.into_iter().flatten().filter(|b| b.at != at).collect()
    }

    /// Moves the shapes dragged along with another (see
    /// [`DocView::moving_with`]) and it, by `d` (canvas), as one step.
    pub(super) fn drag_along(&mut self, dragged: Boxed, with: Vec<Boxed>, d: Vec2) {
        let mut all = with;
        all.push(dragged);
        self.move_units(vec![(all, d)]);
    }
}
