//! The pages laid out one under another, one at a time or two side by
//! side, drawn at the zoom and turn of the view, and the mouse on them:
//! selecting text and following links.

use apex_io::pdf::edit::outline::Target;
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Vec2, pos2, vec2};

use super::text::{self, char_box};
use super::{Layout, PdfView, Selection, Tool, engine, simple};
use crate::common::Fit;
use crate::i18n::tr;

/// Thumbnails are drawn this wide (pixels) at most.
pub const THUMB_PX: u32 = 160;
/// Space between pages.
const GAP: f32 = 14.0;

/// Where a page is on screen: maps points on the page as shown to the
/// screen, turned by the view.
#[derive(Clone, Copy, Debug)]
pub struct PageMap {
    pub rect: Rect,
    pub z: f32,
    /// The page's size as shown, before the view's turn.
    pub w: f32,
    pub h: f32,
    pub turn: u8,
}

impl PageMap {
    pub fn to_screen(&self, x: f32, y: f32) -> Pos2 {
        let (u, v) = match self.turn % 4 {
            0 => (x, y),
            1 => (self.h - y, x),
            2 => (self.w - x, self.h - y),
            _ => (y, self.w - x),
        };
        self.rect.min + vec2(u * self.z, v * self.z)
    }

    pub fn to_page(&self, p: Pos2) -> [f32; 2] {
        let u = (p.x - self.rect.min.x) / self.z;
        let v = (p.y - self.rect.min.y) / self.z;
        match self.turn % 4 {
            0 => [u, v],
            1 => [v, self.h - u],
            2 => [self.w - u, self.h - v],
            _ => [self.w - v, u],
        }
    }

    pub fn rect_to_screen(&self, r: [f32; 4]) -> Rect {
        Rect::from_two_pos(self.to_screen(r[0], r[1]), self.to_screen(r[2], r[3]))
    }

    /// Paints a texture of the page (drawn upright) turned as the view.
    pub fn paint_texture(&self, painter: &egui::Painter, tex: egui::TextureId) {
        let mut mesh = egui::Mesh::with_texture(tex);
        let corners = [(0.0, 0.0, 0.0, 0.0), (self.w, 0.0, 1.0, 0.0), (self.w, self.h, 1.0, 1.0), (0.0, self.h, 0.0, 1.0)];
        for (x, y, u, v) in corners {
            mesh.vertices.push(egui::epaint::Vertex { pos: self.to_screen(x, y), uv: pos2(u, v), color: Color32::WHITE });
        }
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(egui::Shape::mesh(mesh));
    }
}

fn blue(a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(0x33, 0x88, 0xFF, a)
}

impl PdfView {
    /// Each page's box in the scrolled content (points at zoom).
    fn page_boxes(&self, z: f32, avail_w: f32) -> (Vec<(usize, Rect)>, Vec2) {
        let n = self.page_count();
        let mut out = Vec::with_capacity(n);
        let list: Vec<usize> = match self.layout {
            Layout::Single => {
                let a = self.cur.min(n.saturating_sub(1));
                vec![a]
            }
            _ => (0..n).collect(),
        };
        let per_row = if self.layout == Layout::Two { 2 } else { 1 };
        let rows: Vec<&[usize]> = list.chunks(per_row).collect();
        let row_w = |r: &[usize]| r.iter().map(|&i| self.shown_size(i)[0] * z).sum::<f32>() + GAP * (r.len().saturating_sub(1)) as f32;
        let width = rows.iter().map(|r| row_w(r)).fold(0.0, f32::max) + 2.0 * GAP;
        let width = width.max(avail_w);
        let mut y = GAP;
        for r in rows {
            let mut x = (width - row_w(r)) / 2.0;
            let h = r.iter().map(|&i| self.shown_size(i)[1] * z).fold(0.0, f32::max);
            for &i in r {
                let s = self.shown_size(i);
                out.push((i, Rect::from_min_size(pos2(x, y), vec2(s[0] * z, s[1] * z))));
                x += s[0] * z + GAP;
            }
            y += h + GAP;
        }
        (out, vec2(width, y))
    }

    /// The zoom that fits the pages in `avail`.
    fn fit_zoom(&self, fit: Fit, avail: Vec2) -> f32 {
        let s = self.shown_size(self.cur);
        let per_row = if self.layout == Layout::Two { 2.0 } else { 1.0 };
        let zw = (avail.x - 2.0 * GAP - 16.0 - GAP * (per_row - 1.0)) / (s[0] * per_row).max(1.0);
        let z = match fit {
            Fit::PageWidth => zw,
            Fit::WholePage => zw.min((avail.y - 2.0 * GAP) / s[1].max(1.0)),
        };
        z.clamp(0.1, 8.0)
    }

    pub(super) fn map_of(&self, i: usize, rect: Rect, z: f32) -> PageMap {
        let s = self.sizes.1.get(i).copied().unwrap_or([612.0, 792.0]);
        PageMap { rect, z, w: s[0], h: s[1], turn: self.turn }
    }

    pub(super) fn pages_ui(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        self.view_size = avail;
        if let Some(f) = self.fit {
            self.zoom = self.fit_zoom(f, avail);
        }
        let z = self.zoom;
        let (boxes, total) = self.page_boxes(z, avail.x);
        let mut area = egui::ScrollArea::both().id_salt(("pdf_pages", self.id)).auto_shrink([false, false]);
        if let Some((page, top)) = self.goto.take()
            && let Some((_, r)) = boxes.iter().find(|(i, _)| *i == page)
        {
            area = area.vertical_scroll_offset((r.min.y - GAP + top * z).max(0.0));
        } else if let Some((page, top, was)) = self.view_top
            && (was - z).abs() > 1e-4
            && let Some((_, r)) = boxes.iter().find(|(i, _)| *i == page)
        {
            // A new zoom (or the pane beside it resized with the page
            // fitted): the same place stays at the top.
            area = area.vertical_scroll_offset((r.min.y + top * z).max(0.0));
        }
        let ppp = ui.ctx().pixels_per_point();
        let ctx = ui.ctx().clone();
        area.show_viewport(ui, |ui, viewport| {
            let origin = ui.max_rect().min;
            let hand = self.tool == Tool::Hand;
            let back = ui.allocate_rect(Rect::from_min_size(origin, total), if hand { Sense::drag() } else { Sense::hover() });
            if hand {
                self.hand_drag(ui, &back);
            }
            // What is at the top of the view, to keep it there.
            if let Some((i, r)) = boxes.iter().find(|(_, r)| r.max.y > viewport.min.y) {
                self.view_top = Some((*i, (viewport.min.y - r.min.y) / z, z));
            }
            // The page at the top of the view is the current one.
            if self.layout != Layout::Single
                && let Some((i, _)) = boxes.iter().find(|(_, r)| r.max.y > viewport.min.y + viewport.height() * 0.3)
            {
                self.cur = *i;
            }
            for (i, r) in boxes.clone() {
                if !r.intersects(viewport.expand(200.0)) {
                    continue;
                }
                let rect = r.translate(origin.to_vec2());
                self.page_ui(ui, &ctx, i, rect, z, ppp);
            }
        });
    }

    fn page_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, i: usize, rect: Rect, z: f32, ppp: f32) {
        let painter = ui.painter_at(rect.expand(4.0));
        painter.rect_filled(rect.translate(vec2(2.0, 3.0)), 0.0, Color32::from_black_alpha(50));
        painter.rect_filled(rect, 0.0, self.paper());
        let map = self.map_of(i, rect, z);
        // Drawn by pdfium: at the width shown, in steps (so zooming
        // doesn't ask for every size).
        let mut drawn = false;
        if engine::available() {
            let px = ((map.w * z * ppp) / 64.0).ceil() * 64.0;
            let px = px.clamp(64.0, 6000.0) as u32;
            self.want(ctx, i, px);
            if let Some(t) = self.tex.get(&i) {
                map.paint_texture(&painter, t.handle.id());
                drawn = true;
            } else if let Some(t) = self.thumbs.get(&i) {
                map.paint_texture(&painter, t.handle.id());
                drawn = true;
            }
        }
        if !drawn {
            match self.text.page(i) {
                Some(p) => {
                    simple::paint(ui, &painter, p, &map);
                    let annots = self.annots_of(i);
                    simple::paint_annots(&painter, &annots, &map);
                }
                None => {
                    painter.text(rect.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(24.0), Color32::GRAY);
                }
            }
        }
        // Search matches and the selection.
        for (k, h) in self.find.hits.iter().enumerate() {
            if h.page != i {
                continue;
            }
            let color = if k == self.find.cur { Color32::from_rgba_unmultiplied(255, 140, 0, 110) } else { Color32::from_rgba_unmultiplied(255, 230, 0, 90) };
            self.paint_chars(&painter, &map, *h, color);
        }
        if let Some(s) = self.sel.filter(|s| s.page == i) {
            self.paint_chars(&painter, &map, s, blue(80));
        }
        self.paint_changes(&painter, &map, i);
        self.paint_annot_overlay(&painter, i, &map);
        self.page_input(ui, i, rect, map);
        // The form's fields over the page take the mouse first.
        if self.tool == Tool::Select {
            self.fields_ui(ui, i, &map);
        }
    }

    pub(super) fn paint_chars(&self, painter: &egui::Painter, map: &PageMap, s: Selection, color: Color32) {
        let Some(p) = self.text.page(s.page) else { return };
        // One box a line (a box a letter would overlap where letters do).
        for r in super::annotate::line_boxes(p, s.a.min(s.b), s.a.max(s.b)) {
            painter.rect_filled(map.rect_to_screen(r), 0.0, color);
        }
    }

    /// The annotations of page `i` (for drawing without pdfium).
    pub(super) fn annots_of(&mut self, i: usize) -> std::sync::Arc<Vec<apex_io::pdf::edit::annots::Annot>> {
        let rev = self.doc.revision;
        if let Some((r, a)) = self.annot_cache.get(&i)
            && *r == rev
        {
            return a.clone();
        }
        let a = std::sync::Arc::new(self.doc.annots(i));
        self.annot_cache.insert(i, (rev, a.clone()));
        a
    }

    fn links_of(&mut self, i: usize) -> std::sync::Arc<Vec<([f32; 4], Target)>> {
        let rev = self.doc.revision;
        if let Some((r, a)) = self.link_cache.get(&i)
            && *r == rev
        {
            return a.clone();
        }
        let a = std::sync::Arc::new(self.doc.links(i));
        self.link_cache.insert(i, (rev, a.clone()));
        a
    }

    fn page_input(&mut self, ui: &mut egui::Ui, i: usize, rect: Rect, map: PageMap) {
        let id = ui.id().with(("pdf_page", i));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        if self.tool == Tool::Hand {
            self.hand_drag(ui, &resp);
        }
        let Some(pos) = resp.hover_pos().or(resp.interact_pointer_pos()) else {
            self.page_menu(&resp, i);
            return;
        };
        let pt = map.to_page(pos);
        let links = self.links_of(i);
        let link = links.iter().find(|(r, _)| pt[0] >= r[0] && pt[0] <= r[2] && pt[1] >= r[1] && pt[1] <= r[3]).map(|l| l.1.clone());
        if let Some(Target::Uri(u)) = &link {
            resp.clone().on_hover_text(u.as_str());
        }
        // Drags start where the button went down (not where they are
        // first noticed).
        let start = ui.input(|inp| inp.pointer.press_origin()).map_or(pt, |p| map.to_page(p));
        if self.annot_input(ui, &resp, i, pt, start) {
            self.page_menu(&resp, i);
            return;
        }
        if matches!(self.tool, Tool::Select | Tool::Markup(_)) {
            let on_text = self.text.page(i).is_some_and(|p| p.chars.iter().any(|c| {
                let b = char_box(c);
                pt[0] >= b[0] && pt[0] <= b[2] && pt[1] >= b[1] && pt[1] <= b[3]
            }));
            if link.is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            } else if on_text {
                ui.ctx().set_cursor_icon(CursorIcon::Text);
            }
            if resp.drag_started() {
                self.sel = self.text.page(i).and_then(|p| text::char_at(p, start[0], start[1])).map(|c| Selection { page: i, a: c, b: c });
                self.drag_from = self.sel.map(|s| s.a);
            } else if resp.dragged()
                && let (Some(from), Some(p)) = (self.drag_from, self.text.page(i))
                && let Some(c) = text::char_at(p, pt[0], pt[1])
            {
                self.sel = Some(Selection { page: i, a: from, b: c });
            }
            if resp.double_clicked()
                && let Some(p) = self.text.page(i)
                && let Some(c) = text::char_at(p, pt[0], pt[1])
            {
                let (a, b) = text::word_at(p, c);
                self.sel = Some(Selection { page: i, a, b });
            } else if resp.clicked() {
                match link {
                    Some(Target::Page(p, top)) => self.go_to(p, top.unwrap_or(0.0)),
                    Some(Target::Uri(u)) => {
                        if let Err(e) = crate::links::follow_asked(&u) {
                            self.notices.push(crate::app::Notice::error(e));
                        }
                    }
                    None => self.sel = None,
                }
            }
            if let Tool::Markup(k) = self.tool
                && (resp.drag_stopped() || resp.double_clicked())
                && self.sel.is_some_and(|s| s.page == i)
            {
                self.mark_selection(k);
            }
        } else if resp.clicked()
            && let Some(Target::Page(p, top)) = link
        {
            self.go_to(p, top.unwrap_or(0.0));
        }
        self.page_menu(&resp, i);
    }

    /// The hand: dragging moves the pages.
    fn hand_drag(&self, ui: &egui::Ui, resp: &egui::Response) {
        if resp.dragged() {
            ui.scroll_with_delta(resp.drag_delta());
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }

    /// The right-click menu of a page.
    fn page_menu(&mut self, resp: &egui::Response, i: usize) {
        resp.context_menu(|ui| {
            if let Some(id) = self.picked {
                if ui.button(tr("pdf.edit_comment")).clicked() {
                    self.edit_comment(id);
                    ui.close();
                }
                crate::widgets::menu_button(ui, tr("pdf.color"), |ui| {
                    for c in super::annotate::COLORS {
                        let (r, resp) = ui.allocate_exact_size(vec2(60.0, 16.0), Sense::click());
                        ui.painter().rect_filled(r.shrink(1.0), 2.0, super::annotate::color32(c));
                        if resp.clicked() {
                            self.recolor(c);
                            ui.close();
                        }
                    }
                });
                if ui.button(tr("pdf.delete_annot")).clicked() {
                    self.delete_picked();
                    ui.close();
                }
                ui.separator();
            }
            if ui.add_enabled(self.sel.is_some(), egui::Button::new(tr("edit.copy"))).clicked() {
                if let Some(t) = self.selected_text() {
                    ui.ctx().copy_text(t);
                }
                ui.close();
            }
            if ui.add_enabled(self.sel.is_some(), egui::Button::new(tr("nb.send_fragment"))).on_hover_text(tr("nb.send_fragment_tip")).clicked() {
                self.fragment_wanted = true;
                ui.close();
            }
            if ui.button(tr("pdf.select_page_text")).clicked() {
                self.select_page(i);
                ui.close();
            }
            ui.separator();
            if ui.button(tr("pdf.rotate_view_right")).clicked() {
                self.turn = (self.turn + 1) % 4;
                ui.close();
            }
        });
    }
}
