//! The Selection Pane (Office's Home > Select > Selection Pane): the
//! objects of a slide, a document or a sheet by name, the front one first.
//! A click selects (Ctrl+click adds), a double click renames, the eye
//! shows or hides, a drag or the arrows restack, and Show All / Hide All.
//! Each view lists its objects and does what the pane asks.

use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Stroke, pos2, vec2};

use crate::i18n::tr;
use crate::icons::Icon;
use crate::widgets::{self, Face};

/// An object as the pane lists it.
#[derive(Clone, Debug)]
pub struct Item {
    /// What the view knows it by.
    pub key: u64,
    pub name: String,
    pub hidden: bool,
    pub selected: bool,
    /// Can be restacked (objects in the text of a document can't).
    pub movable: bool,
}

/// What the pane asks the view to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Select the object (true: add it to the selection).
    Select(u64, bool),
    Rename(u64, String),
    /// Hide (true) or show it.
    Hide(u64, bool),
    /// Hide (true) or show every object.
    All(bool),
    /// Restack it to place `usize` of the list (front first).
    Move(u64, usize),
    Close,
}

#[derive(Default)]
pub struct PaneState {
    /// The object being renamed and its new name.
    rename: Option<(u64, String)>,
    /// The object being dragged to another place.
    drag: Option<u64>,
}

const ROW_H: f32 = 22.0;

/// The pane's contents; `items` front first.
pub fn show(ui: &mut egui::Ui, st: &mut PaneState, items: &[Item]) -> Vec<Action> {
    let mut out = Vec::new();
    ui.horizontal(|ui| {
        ui.strong(tr("selpane.title"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("×").on_hover_text(tr("dialog.close")).clicked() {
                out.push(Action::Close);
            }
        });
    });
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(items.iter().any(|i| i.hidden), egui::Button::new(tr("selpane.show_all"))).clicked() {
            out.push(Action::All(false));
        }
        if ui.add_enabled(items.iter().any(|i| !i.hidden), egui::Button::new(tr("selpane.hide_all"))).clicked() {
            out.push(Action::All(true));
        }
        // Forward and backward: the (first) selected object.
        let at = items.iter().position(|i| i.selected && i.movable);
        let front = at.is_some_and(|k| k > 0 && items[k - 1].movable);
        let back = at.is_some_and(|k| items.get(k + 1).is_some_and(|i| i.movable));
        if ui.add_enabled_ui(front, |ui| widgets::tool_button(ui, Face::Icon(Icon::ChevronUp), false, tr("shape.forward"))).inner.clicked()
            && let Some(k) = at
        {
            out.push(Action::Move(items[k].key, k - 1));
        }
        if ui.add_enabled_ui(back, |ui| widgets::tool_button(ui, Face::Icon(Icon::ChevronDown), false, tr("shape.backward"))).inner.clicked()
            && let Some(k) = at
        {
            out.push(Action::Move(items[k].key, k + 1));
        }
    });
    ui.separator();
    if items.is_empty() {
        ui.label(egui::RichText::new(tr("selpane.empty")).weak());
        return out;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut rows: Vec<Rect> = Vec::with_capacity(items.len());
        for it in items {
            rows.push(row(ui, st, it, &mut out));
        }
        // Dragging a row: a line where it goes, and there when let go.
        if let Some(key) = st.drag {
            let from = items.iter().position(|i| i.key == key);
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let to = pointer.map(|p| drop_index(&rows, p));
            if let (Some(to), Some(from)) = (to, from) {
                let y = if to < rows.len() { rows[to].min.y } else { rows[rows.len() - 1].max.y };
                ui.painter().hline(rows[0].x_range(), y, Stroke::new(2.0, ui.visuals().selection.stroke.color));
                if ui.input(|i| i.pointer.any_released()) {
                    st.drag = None;
                    // Its place once taken out of the list.
                    let to = if to > from { to - 1 } else { to };
                    if to != from && items.get(to).is_some_and(|i| i.movable) {
                        out.push(Action::Move(key, to));
                    }
                }
            }
            if !ui.input(|i| i.pointer.any_down()) {
                st.drag = None;
            }
        }
    });
    out
}

/// Where a row dropped at `p` goes: before row `k` (or at the end).
pub fn drop_index(rows: &[Rect], p: Pos2) -> usize {
    rows.iter().position(|r| p.y < r.center().y).unwrap_or(rows.len())
}

fn row(ui: &mut egui::Ui, st: &mut PaneState, it: &Item, out: &mut Vec<Action>) -> Rect {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click_and_drag());
    let v = ui.visuals().clone();
    let eye_rect = Rect::from_center_size(pos2(rect.max.x - 14.0, rect.center().y), vec2(22.0, ROW_H - 2.0));
    let text_color = if it.hidden { v.weak_text_color() } else if it.selected { v.selection.stroke.color } else { v.text_color() };
    if it.selected {
        ui.painter().rect_filled(rect, 3.0, v.selection.bg_fill);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 3.0, v.widgets.hovered.weak_bg_fill);
    }
    let renaming = st.rename.as_ref().is_some_and(|(k, _)| *k == it.key);
    if renaming {
        let r = Rect::from_min_max(pos2(rect.min.x + 4.0, rect.min.y + 1.0), pos2(eye_rect.min.x - 4.0, rect.max.y - 1.0));
        let Some((_, name)) = &mut st.rename else { return rect };
        let te = ui.put(r, egui::TextEdit::singleline(name).id(Id::new(("selpane_rename", it.key))));
        if !te.has_focus() && !te.lost_focus() {
            te.request_focus();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            st.rename = None;
        } else if te.lost_focus() {
            if let Some((k, n)) = st.rename.take() {
                let n = n.trim().to_string();
                if !n.is_empty() && n != it.name {
                    out.push(Action::Rename(k, n));
                }
            }
        }
    } else {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let g = ui.painter().layout(it.name.clone(), font, text_color, (eye_rect.min.x - rect.min.x - 10.0).max(10.0));
        ui.painter().with_clip_rect(rect).galley(pos2(rect.min.x + 6.0, rect.center().y - g.size().y / 2.0), g, text_color);
    }
    // The eye.
    let eye = ui.interact(eye_rect, Id::new(("selpane_eye", it.key)), Sense::click());
    paint_eye(ui.painter(), eye_rect.center(), it.hidden, if eye.hovered() { v.strong_text_color() } else { v.text_color() });
    if eye.clicked() {
        out.push(Action::Hide(it.key, !it.hidden));
    }
    let eye = eye.on_hover_text(tr(if it.hidden { "selpane.show" } else { "selpane.hide" }));
    if eye.hovered() || renaming {
        return rect;
    }
    if resp.double_clicked() {
        st.rename = Some((it.key, it.name.clone()));
    } else if resp.clicked() {
        out.push(Action::Select(it.key, ui.input(|i| i.modifiers.command || i.modifiers.shift)));
    }
    if resp.drag_started() && it.movable {
        st.drag = Some(it.key);
    }
    if st.drag == Some(it.key) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    rect
}

/// An open eye, or a shut one with a stroke through it.
fn paint_eye(p: &egui::Painter, c: Pos2, hidden: bool, color: Color32) {
    let s = Stroke::new(1.3, color);
    let (w, h) = (7.0, 4.0);
    let pts = |sign: f32| (0..=12).map(|i| {
        let t = i as f32 / 12.0;
        let x = -w + 2.0 * w * t;
        pos2(c.x + x, c.y + sign * h * (1.0 - (x / w).powi(2)))
    }).collect::<Vec<_>>();
    if hidden {
        p.add(egui::Shape::line(pts(1.0), s));
        p.line_segment([pos2(c.x - w, c.y - h - 1.0), pos2(c.x + w, c.y + h + 1.0)], s);
    } else {
        p.add(egui::Shape::line(pts(1.0), s));
        p.add(egui::Shape::line(pts(-1.0), s));
        p.circle_filled(c, 2.0, color);
    }
}

/// Restacks `list` (front first) as the pane asked: `key` to place `to`.
pub fn moved<T: Clone>(list: &[T], from: usize, to: usize) -> Vec<T> {
    let mut v = list.to_vec();
    if from < v.len() {
        let x = v.remove(from);
        v.insert(to.min(v.len()), x);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_drop_between() {
        let rows: Vec<Rect> = (0..3).map(|i| Rect::from_min_size(pos2(0.0, i as f32 * 20.0), vec2(100.0, 20.0))).collect();
        assert_eq!(drop_index(&rows, pos2(5.0, 2.0)), 0);
        assert_eq!(drop_index(&rows, pos2(5.0, 15.0)), 1);
        assert_eq!(drop_index(&rows, pos2(5.0, 45.0)), 2);
        assert_eq!(drop_index(&rows, pos2(5.0, 90.0)), 3);
        assert_eq!(moved(&[1, 2, 3, 4], 0, 2), vec![2, 3, 1, 4]);
        assert_eq!(moved(&[1, 2, 3, 4], 3, 0), vec![4, 1, 2, 3]);
    }
}
