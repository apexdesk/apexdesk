//! The side pane: page thumbnails, bookmarks and comments.

use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};

use super::pages::THUMB_PX;
use super::{Pane, PdfView, engine};
use crate::i18n::tr;

impl PdfView {
    pub(super) fn side_ui(&mut self, ui: &mut egui::Ui) {
        // Long names (German) go on two lines in a narrow pane.
        ui.horizontal_wrapped(|ui| {
            for (pane, key) in [(Pane::Thumbnails, "pdf.thumbnails"), (Pane::Bookmarks, "pdf.bookmarks"), (Pane::Comments, "pdf.comments")] {
                if ui.selectable_label(self.pane == pane, tr(key)).clicked() {
                    self.pane = pane;
                }
            }
        });
        ui.separator();
        match self.pane {
            Pane::Bookmarks => self.bookmarks_ui(ui),
            Pane::Comments => self.comments_ui(ui),
            _ => self.thumbnails_ui(ui),
        }
    }

    fn thumbnails_ui(&mut self, ui: &mut egui::Ui) {
        let n = self.page_count();
        let w = (ui.available_width() - 24.0).clamp(60.0, THUMB_PX as f32);
        let ctx = ui.ctx().clone();
        let row_h = w * 1.35 + 22.0;
        // Where a dragged page would go: before this page.
        let mut drop_at: Option<usize> = None;
        let pointer = ui.ctx().pointer_interact_pos();
        egui::ScrollArea::vertical().id_salt(("pdf_thumbs", self.id)).auto_shrink([false, false]).show_rows(ui, row_h, n, |ui, range| {
            for i in range {
                let s = self.shown_size(i);
                let z = w / s[0].max(1.0);
                let h = (s[1] * z).min(w * 1.35);
                let z = z.min(h / s[1].max(1.0));
                let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
                let rect = egui::Rect::from_center_size(egui::pos2(outer.center().x, outer.min.y + 4.0 + h / 2.0), vec2(s[0] * z, s[1] * z));
                let painter = ui.painter_at(outer);
                painter.rect_filled(rect, 0.0, Color32::WHITE);
                let map = self.map_of(i, rect, z);
                if engine::available() {
                    self.want(&ctx, i, THUMB_PX);
                    if let Some(t) = self.thumbs.get(&i) {
                        map.paint_texture(&painter, t.handle.id());
                    }
                } else if let Some(p) = self.text.page(i) {
                    super::simple::paint(ui, &painter, p, &map);
                }
                let on = i == self.cur || self.page_sel.contains(&i);
                if self.page_sel.contains(&i) {
                    painter.rect_filled(outer.shrink(2.0), 4.0, crate::theme::ACCENT.gamma_multiply(0.18));
                    painter.rect_filled(rect, 0.0, Color32::WHITE);
                    if engine::available()
                        && let Some(t) = self.thumbs.get(&i)
                    {
                        map.paint_texture(&painter, t.handle.id());
                    }
                }
                painter.rect_stroke(rect, 0.0, Stroke::new(if on { 2.5 } else { 1.0 }, if on { crate::theme::ACCENT } else { Color32::from_gray(150) }), egui::StrokeKind::Outside);
                painter.text(egui::pos2(outer.center().x, rect.max.y + 3.0), egui::Align2::CENTER_TOP, format!("{}", i + 1), egui::FontId::proportional(12.0), ui.visuals().text_color());
                if resp.drag_started() {
                    self.thumb_drag = Some(i);
                }
                if self.thumb_drag.is_some()
                    && let Some(p) = pointer
                    && p.y >= outer.min.y
                    && p.y < outer.max.y
                {
                    let at = if p.y < outer.center().y { i } else { i + 1 };
                    let y = if at == i { outer.min.y } else { outer.max.y };
                    ui.painter().hline(outer.x_range(), y, Stroke::new(3.0, crate::theme::ACCENT));
                    drop_at = Some(at);
                }
                if resp.clicked() {
                    let m = ui.input(|inp| inp.modifiers);
                    self.click_thumb(i, m);
                }
                if resp.secondary_clicked() && !self.page_sel.contains(&i) {
                    self.page_sel.clear();
                    self.go_to(i, 0.0);
                }
                resp.context_menu(|ui| self.thumb_menu(ui));
            }
        });
        if let Some(from) = self.thumb_drag
            && !ui.input(|inp| inp.pointer.any_down())
        {
            self.thumb_drag = None;
            if let Some(to) = drop_at
                && to != from
                && to != from + 1
            {
                self.move_to(from, to);
            }
        }
    }

    fn bookmarks_ui(&mut self, ui: &mut egui::Ui) {
        if self.bookmarks.0 != self.doc.revision {
            self.bookmarks = (self.doc.revision, self.doc.bookmarks());
        }
        let list = self.bookmarks.1.clone();
        if list.is_empty() {
            ui.label(RichText::new(tr("pdf.no_bookmarks")).weak());
            return;
        }
        let mut go = None;
        egui::ScrollArea::vertical().id_salt(("pdf_bm", self.id)).auto_shrink([false, false]).show(ui, |ui| {
            // Collapsed items hide the ones under them.
            let mut skip_until = 0;
            for (k, b) in list.iter().enumerate() {
                if k < skip_until {
                    continue;
                }
                let open_id = ui.id().with(("bm_open", b.id));
                let open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(b.level == 0);
                ui.horizontal(|ui| {
                    ui.add_space(b.level as f32 * 14.0);
                    if b.children > 0 {
                        // A small triangle, pointing down when open.
                        let (r, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
                        let c = r.center();
                        let pts = if open {
                            vec![egui::pos2(c.x - 4.0, c.y - 2.0), egui::pos2(c.x + 4.0, c.y - 2.0), egui::pos2(c.x, c.y + 3.0)]
                        } else {
                            vec![egui::pos2(c.x - 2.0, c.y - 4.0), egui::pos2(c.x + 3.0, c.y), egui::pos2(c.x - 2.0, c.y + 4.0)]
                        };
                        let col = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
                        ui.painter().add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
                        if resp.clicked() {
                            ui.data_mut(|d| d.insert_temp(open_id, !open));
                        }
                    } else {
                        ui.add_space(14.0);
                    }
                    let title = if b.title.is_empty() { "—".to_string() } else { b.title.clone() };
                    if ui.add(egui::Button::new(title).frame(false).truncate()).clicked()
                        && let Some(p) = b.page
                    {
                        go = Some((p, b.top.unwrap_or(0.0)));
                    }
                });
                if !open {
                    skip_until = k + 1 + b.children;
                }
            }
        });
        if let Some((p, top)) = go {
            self.go_to(p, top);
        }
    }
}
