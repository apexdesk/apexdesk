//! Presentation mode: one page at a time on the whole screen, turned
//! with the keys, the wheel or a click, until Esc.

use eframe::egui::{self, Color32, Key, Modifiers, Rect, Sense, vec2};

use super::{PdfView, engine};

pub struct Show {
    pub page: usize,
    /// The window was in full screen already: leave it so at the end.
    pub was_full: bool,
}

impl PdfView {
    pub fn start_show(&mut self, ctx: &egui::Context, page: usize) {
        if self.page_count() == 0 {
            return;
        }
        let was_full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        self.show = Some(Show { page: page.min(self.page_count() - 1), was_full });
        self.sel = None;
        if !was_full {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
    }

    pub fn end_show(&mut self, ctx: &egui::Context) {
        if let Some(s) = self.show.take() {
            self.cur = s.page;
            self.goto = Some((s.page, 0.0));
            if !s.was_full {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        }
    }

    pub(super) fn show_ui(&mut self, ui: &mut egui::Ui) {
        let n = self.page_count();
        let Some(page) = self.show.as_ref().map(|s| s.page) else { return };
        let (fwd, back, first, last, esc, wheel) = ui.input_mut(|i| {
            let f = [Key::ArrowRight, Key::ArrowDown, Key::Space, Key::PageDown, Key::Enter, Key::N].iter().any(|k| i.consume_key(Modifiers::NONE, *k));
            let b = [Key::ArrowLeft, Key::ArrowUp, Key::PageUp, Key::Backspace, Key::P].iter().any(|k| i.consume_key(Modifiers::NONE, *k));
            (f, b, i.consume_key(Modifiers::NONE, Key::Home), i.consume_key(Modifiers::NONE, Key::End), i.consume_key(Modifiers::NONE, Key::Escape), i.smooth_scroll_delta.y)
        });
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("pdf_show"), Sense::click());
        let mut next = page;
        if fwd || resp.clicked() || wheel < -1.0 {
            next = (page + 1).min(n - 1);
        }
        if back || resp.secondary_clicked() || wheel > 1.0 {
            next = page.saturating_sub(1);
        }
        if first {
            next = 0;
        }
        if last {
            next = n - 1;
        }
        if let Some(s) = &mut self.show {
            s.page = next;
        }
        if esc {
            self.end_show(ui.ctx());
            return;
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        let s = self.shown_size(next);
        let z = (rect.width() / s[0].max(1.0)).min(rect.height() / s[1].max(1.0));
        let r = Rect::from_center_size(rect.center(), vec2(s[0] * z, s[1] * z));
        painter.rect_filled(r, 0.0, self.paper());
        let map = self.map_of(next, r, z);
        let ctx = ui.ctx().clone();
        let mut drawn = false;
        if engine::available() {
            let px = ((map.w * z * ctx.pixels_per_point()) / 64.0).ceil() * 64.0;
            self.want(&ctx, next, px.clamp(64.0, 6000.0) as u32);
            if let Some(t) = self.tex.get(&next) {
                map.paint_texture(&painter, t.handle.id());
                drawn = true;
            }
        }
        if !drawn && let Some(p) = self.text.page(next) {
            super::simple::paint(ui, &painter, p, &map);
            let annots = self.annots_of(next);
            super::simple::paint_annots(&painter, &annots, &map);
        }
    }
}
