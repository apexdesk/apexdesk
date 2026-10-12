//! Slicers: floating panels of buttons over the sheet that filter a pivot
//! table by the items of a field. Click selects one item, Ctrl+click adds
//! or removes items; the header moves the panel and the corner resizes it.

use apex_sheet::analysis::Slicer;
use eframe::egui::Align2;

use super::images_ui::to_screen;
use super::*;

#[derive(Default)]
pub(super) struct SlicerState {
    /// Slicer being moved or resized: index, resizing, box at the start.
    drag: Option<(usize, bool, [f32; 4])>,
    /// Box shown while dragging.
    preview: Option<(usize, [f32; 4])>,
}

enum Action {
    Select(usize, Vec<String>),
    Place(usize, [f32; 4]),
    Columns(usize, u8),
    Remove(usize),
}

/// Height of the caption bar and of a button, and the gaps (sheet pixels).
const HEAD: f32 = 30.0;
const BUTTON: f32 = 24.0;
const GAP: f32 = 4.0;
const PAD: f32 = 8.0;

/// The selection after clicking `label` (empty: everything).
fn click_selection(items: &[(String, bool)], label: &str, add: bool) -> Vec<String> {
    let filtered = items.iter().any(|x| !x.1);
    let mut sel: Vec<String> = if add {
        items.iter().filter(|x| x.1 && (filtered || x.0 != label)).map(|x| x.0.clone()).collect()
    } else {
        vec![label.to_string()]
    };
    if add && filtered {
        match sel.iter().position(|x| x == label) {
            Some(k) => {
                sel.remove(k);
            }
            None => sel.push(label.to_string()),
        }
    }
    if sel.len() >= items.len() {
        sel.clear();
    }
    sel
}

impl SheetView {
    /// Draws the slicers of the current sheet and handles their input.
    pub(super) fn slicers_ui(&mut self, ui: &mut egui::Ui, g: &Geo) {
        let name = &self.sh().name;
        let list: Vec<(usize, Slicer)> = self.wb.analysis.slicers.iter().enumerate().filter(|(_, s)| &s.sheet == name).map(|(i, s)| (i, s.clone())).collect();
        if list.is_empty() {
            return;
        }
        let z = g.z;
        let accent = theme::SHEET_ACCENT;
        let mut action: Option<Action> = None;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let add_key = ui.input(|i| i.modifiers.command);
        for (i, s) in list {
            let [x, y, w, h] = match self.f10.pivot.slicer.preview {
                Some((k, b)) if k == i => b,
                _ => [s.x, s.y, s.width, s.height],
            };
            let rect = Rect::from_min_max(to_screen(g, self.sh(), false, false, x, y), to_screen(g, self.sh(), false, false, x + w, y + h));
            if !rect.intersects(g.view) {
                continue;
            }
            let id = Id::new(("slicer", self.uid, i));
            let mut child = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(rect));
            child.set_clip_rect(g.view.intersect(ui.clip_rect()));
            // The whole panel takes the pointer from the grid below (the
            // grid senses drags, so this must too).
            let bg = child.interact(rect, id.with("bg"), Sense::click_and_drag());
            let vis = child.visuals().clone();
            let p = child.painter().clone();
            p.rect(rect, 4.0 * z, vis.window_fill, Stroke::new(1.0, vis.widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
            let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD * z));
            let items = self.wb.slicer_items(i);
            let filtered = items.iter().any(|x| !x.1);
            // Clear filter button.
            let clear = Rect::from_center_size(pos2(head.max.x - 16.0 * z, head.center().y), Vec2::splat(20.0 * z));
            let cresp = child.interact(clear, id.with("clear"), Sense::click()).on_hover_text(tr("slicer.clear"));
            if cresp.hovered() && filtered {
                p.rect_filled(clear, 3.0, vis.widgets.hovered.bg_fill);
            }
            // A funnel with a cross, bright while the slicer filters.
            let icon_color = if filtered { accent } else { vis.weak_text_color() };
            let st = Stroke::new(1.3, icon_color);
            let (c, u) = (clear.center(), 7.0 * z);
            p.line_segment([c + vec2(-u, -u * 0.7), c + vec2(u * 0.4, -u * 0.7)], st);
            p.line_segment([c + vec2(-u, -u * 0.7), c + vec2(-u * 0.35, 0.0)], st);
            p.line_segment([c + vec2(u * 0.4, -u * 0.7), c + vec2(-u * 0.05, 0.0)], st);
            p.line_segment([c + vec2(-u * 0.35, 0.0), c + vec2(-u * 0.35, u * 0.8)], st);
            p.line_segment([c + vec2(-u * 0.05, 0.0), c + vec2(-u * 0.05, u * 0.5)], st);
            p.line_segment([c + vec2(u * 0.3, u * 0.1), c + vec2(u, u * 0.8)], st);
            p.line_segment([c + vec2(u, u * 0.1), c + vec2(u * 0.3, u * 0.8)], st);
            if cresp.clicked() && filtered {
                action = Some(Action::Select(i, Vec::new()));
            }
            let caption_clip = Rect::from_min_max(head.min, pos2(clear.min.x - 2.0, head.max.y));
            p.with_clip_rect(caption_clip.intersect(child.clip_rect())).text(head.left_center() + vec2(PAD * z, 0.0), Align2::LEFT_CENTER, &s.caption, FontId::proportional(13.0 * z), vis.strong_text_color());
            // Buttons.
            let cols = s.columns.max(1) as usize;
            let inner = rect.width() - 2.0 * PAD * z;
            let bw = ((inner - GAP * z * (cols as f32 - 1.0)) / cols as f32).max(4.0);
            let body = Rect::from_min_max(pos2(rect.min.x, head.max.y), rect.max - vec2(0.0, PAD * z * 0.5));
            let bp = p.with_clip_rect(body.intersect(child.clip_rect()));
            for (k, (label, on)) in items.iter().enumerate() {
                let (row, col) = (k / cols, k % cols);
                let r = Rect::from_min_size(pos2(rect.min.x + PAD * z + col as f32 * (bw + GAP * z), head.max.y + row as f32 * (BUTTON + GAP) * z), vec2(bw, BUTTON * z));
                if r.min.y >= body.max.y {
                    break;
                }
                let resp = child.interact(r.intersect(body), id.with(("item", k)), Sense::click());
                crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, label, Some(*on));
                let fill = if *on && filtered {
                    accent.gamma_multiply(0.35)
                } else if *on {
                    accent.gamma_multiply(0.18)
                } else {
                    vis.faint_bg_color
                };
                let stroke = if resp.hovered() { Stroke::new(1.0, accent) } else { Stroke::new(1.0, vis.widgets.noninteractive.bg_stroke.color) };
                bp.rect(r, 3.0, fill, stroke, StrokeKind::Inside);
                let color = if *on { vis.strong_text_color() } else { vis.weak_text_color() };
                bp.with_clip_rect(r.shrink(2.0).intersect(body)).text(r.left_center() + vec2(6.0 * z, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(12.0 * z), color);
                if resp.clicked() {
                    action = Some(Action::Select(i, click_selection(&items, label, add_key)));
                }
            }
            // Move by the caption, resize by the corner.
            let hresp = child.interact(caption_clip, id.with("head"), Sense::click_and_drag());
            let grip = Rect::from_min_max(rect.max - Vec2::splat(12.0), rect.max);
            let rresp = child.interact(grip, id.with("grip"), Sense::drag());
            if hresp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
            if rresp.hovered() || rresp.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeSouthEast);
            }
            for k in 0..3 {
                let o = 3.0 + k as f32 * 3.0;
                p.line_segment([pos2(rect.max.x - o, rect.max.y - 2.0), pos2(rect.max.x - 2.0, rect.max.y - o)], Stroke::new(1.0, vis.weak_text_color()));
            }
            let start = [s.x, s.y, s.width, s.height];
            if hresp.drag_started() {
                self.f10.pivot.slicer.drag = Some((i, false, start));
            }
            if rresp.drag_started() {
                self.f10.pivot.slicer.drag = Some((i, true, start));
            }
            if let (Some((k, resize, b)), true) = (self.f10.pivot.slicer.drag, hresp.dragged() || rresp.dragged()) {
                if k == i {
                    let origin = if resize { rresp.interact_pointer_pos() } else { hresp.interact_pointer_pos() };
                    let press = ui.input(|inp| inp.pointer.press_origin());
                    if let (Some(now), Some(from)) = (origin.or(pointer), press) {
                        let d = (now - from) / z;
                        let nb = if resize { [b[0], b[1], (b[2] + d.x).max(80.0), (b[3] + d.y).max(HEAD + BUTTON + PAD)] } else { [(b[0] + d.x).max(0.0), (b[1] + d.y).max(0.0), b[2], b[3]] };
                        self.f10.pivot.slicer.preview = Some((i, nb));
                    }
                }
            }
            if hresp.drag_stopped() || rresp.drag_stopped() {
                if let Some((k, b)) = self.f10.pivot.slicer.preview.take() {
                    if k == i {
                        action = Some(Action::Place(i, b));
                    }
                }
                self.f10.pivot.slicer.drag = None;
            }
            hresp.union(bg).context_menu(|ui| {
                crate::widgets::menu_button(ui, tr("slicer.columns"), |ui| {
                    for n in 1..=4u8 {
                        if ui.radio(s.columns == n, n.to_string()).clicked() {
                            action = Some(Action::Columns(i, n));
                            ui.close();
                        }
                    }
                });
                if ui.add_enabled(filtered, egui::Button::new(tr("slicer.clear"))).clicked() {
                    action = Some(Action::Select(i, Vec::new()));
                    ui.close();
                }
                ui.separator();
                if ui.button(tr("slicer.remove")).clicked() {
                    action = Some(Action::Remove(i));
                    ui.close();
                }
            });
        }
        let Some(a) = action else { return };
        self.settle_edit();
        // Keys (Ctrl+Z) go back to the grid.
        self.focus_grid = true;
        match a {
            Action::Select(i, sel) => {
                if let Err(e) = self.wb.set_slicer_selection(i, sel) {
                    self.notices.push(Notice::refused(super::analysis_ui::error_text(&e)));
                }
            }
            Action::Place(i, [x, y, w, h]) => {
                if let Some(s) = self.wb.analysis.slicers.get(i).cloned() {
                    self.wb.set_slicer(i, Slicer { x, y, width: w, height: h, ..s });
                }
            }
            Action::Columns(i, n) => {
                if let Some(s) = self.wb.analysis.slicers.get(i).cloned() {
                    self.wb.set_slicer(i, Slicer { columns: n, ..s });
                }
            }
            Action::Remove(i) => self.wb.remove_slicer(i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicer_clicks() {
        let all = |v: &[&str]| v.iter().map(|s| (s.to_string(), true)).collect::<Vec<_>>();
        let items = all(&["A", "B", "C"]);
        assert_eq!(click_selection(&items, "B", false), vec!["B"]);
        assert_eq!(click_selection(&items, "B", true), vec!["A", "C"]);
        let some = vec![("A".to_string(), true), ("B".to_string(), false), ("C".to_string(), false)];
        assert_eq!(click_selection(&some, "C", true), vec!["A", "C"]);
        assert!(click_selection(&some, "A", true).is_empty());
        let two = vec![("A".to_string(), true), ("B".to_string(), true), ("C".to_string(), false)];
        assert!(click_selection(&two, "C", true).is_empty());
    }
}
