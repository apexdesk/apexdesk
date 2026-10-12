//! Annotations on a picture: pen strokes, arrows and boxes drawn in a
//! window and then burnt into the picture's pixels (one undo step).

use std::sync::Arc;

use apex_doc::{Image, Pos};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};

use super::DocView;
use crate::i18n::tr;
use crate::images::Decoded;

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Pen,
    Arrow,
    Box,
}

const COLORS: [Color32; 5] = [Color32::from_rgb(0xE0, 0x30, 0x30), Color32::from_rgb(0xF0, 0xB0, 0x00), Color32::from_rgb(0x20, 0xA0, 0x40), Color32::from_rgb(0x20, 0x70, 0xE0), Color32::BLACK];

/// One mark, in picture pixels.
#[derive(Clone)]
pub(super) struct Mark {
    tool: Tool,
    color: Color32,
    width: f32,
    points: Vec<Pos2>,
}

impl Mark {
    /// The straight pieces the mark is made of.
    fn segments(&self) -> Vec<(Pos2, Pos2)> {
        let (Some(&a), Some(&b)) = (self.points.first(), self.points.last()) else { return Vec::new() };
        match self.tool {
            Tool::Pen if self.points.len() == 1 => vec![(a, a)],
            Tool::Pen => self.points.windows(2).map(|w| (w[0], w[1])).collect(),
            Tool::Box => {
                let (c, d) = (pos2(b.x, a.y), pos2(a.x, b.y));
                vec![(a, c), (c, b), (b, d), (d, a)]
            }
            Tool::Arrow => {
                let dir = (b - a).normalized();
                if !dir.x.is_finite() {
                    return vec![(a, a)];
                }
                let head = (self.width * 4.0).max((b - a).length() * 0.2).min((b - a).length());
                let side = vec2(-dir.y, dir.x);
                let back = b - dir * head;
                vec![(a, b), (b, back + side * head * 0.5), (b, back - side * head * 0.5)]
            }
        }
    }
}

/// Paints `marks` into `rgba` (not premultiplied, `w` × `h`).
pub(super) fn burn(rgba: &mut [u8], w: usize, h: usize, marks: &[Mark]) {
    for m in marks {
        let r = m.width / 2.0;
        let [cr, cg, cb, _] = m.color.to_srgba_unmultiplied();
        for (a, b) in m.segments() {
            let x0 = (a.x.min(b.x) - r - 1.0).floor().max(0.0) as usize;
            let y0 = (a.y.min(b.y) - r - 1.0).floor().max(0.0) as usize;
            let x1 = ((a.x.max(b.x) + r + 1.0).ceil().max(0.0) as usize).min(w);
            let y1 = ((a.y.max(b.y) + r + 1.0).ceil().max(0.0) as usize).min(h);
            let ab = b - a;
            let len2 = ab.length_sq();
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = pos2(x as f32 + 0.5, y as f32 + 0.5);
                    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let cover = (r + 0.5 - (p - (a + ab * t)).length()).clamp(0.0, 1.0);
                    if cover <= 0.0 {
                        continue;
                    }
                    let px = &mut rgba[(y * w + x) * 4..][..4];
                    let mix = |s: u8, d: u8| (s as f32 * cover + d as f32 * (1.0 - cover)).round() as u8;
                    // Over itself a stroke keeps the strongest coverage.
                    px[0] = mix(cr, px[0]);
                    px[1] = mix(cg, px[1]);
                    px[2] = mix(cb, px[2]);
                    px[3] = px[3].max((cover * 255.0) as u8);
                }
            }
        }
    }
}

/// The annotation window.
pub(super) struct Annotator {
    at: Pos,
    img: Image,
    base: Decoded,
    marks: Vec<Mark>,
    drawing: Option<Mark>,
    tool: Tool,
    color: Color32,
    /// Index into the thickness choices.
    thick: usize,
}

const THICK: [(f32, &str); 3] = [(1.0, "annot.thin"), (2.0, "annot.medium"), (4.0, "annot.thick")];

impl Annotator {
    /// Stroke width in picture pixels.
    fn width(&self) -> f32 {
        THICK[self.thick].0 * (self.base.width.max(self.base.height) as f32 / 300.0).max(1.0)
    }

    /// The picture with the marks burnt in, as PNG.
    fn result(&self) -> Option<Vec<u8>> {
        let (w, h) = (self.base.width as usize, self.base.height as usize);
        let mut rgba = self.base.rgba.to_vec();
        burn(&mut rgba, w, h, &self.marks);
        crate::charts::encode_png(&egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
    }
}

/// Whether a picture can be annotated (a plain raster picture).
pub(super) fn can_annotate(img: &Image) -> bool {
    img.object.is_none() && matches!(apex_sheet::ImageFormat::sniff(&img.data), Some(apex_sheet::ImageFormat::Png | apex_sheet::ImageFormat::Jpeg | apex_sheet::ImageFormat::Gif | apex_sheet::ImageFormat::Bmp))
}

impl DocView {
    pub(super) fn open_annotate(&mut self, at: Pos, img: &Image) {
        let Some(base) = crate::images::decode(&img.data) else { return };
        self.pics.annotate = Some(Annotator { at, img: img.clone(), base, marks: Vec::new(), drawing: None, tool: Tool::Pen, color: COLORS[0], thick: 1 });
    }

    /// Applies the marks of the window to the picture.
    pub(super) fn apply_annotations(&mut self) {
        let Some(a) = self.pics.annotate.take() else { return };
        if a.marks.is_empty() {
            return;
        }
        if let Some(png) = a.result() {
            self.doc.set_image(a.at, Image { data: Arc::from(png), ..a.img });
        }
    }

    pub(super) fn annotate_window(&mut self, ctx: &egui::Context) {
        let Some(a) = &mut self.pics.annotate else { return };
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(tr("annot.title")).id(egui::Id::new(("annotate", self.uid))).open(&mut open).collapsible(false).resizable(false).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (t, key) in [(Tool::Pen, "annot.pen"), (Tool::Arrow, "annot.arrow"), (Tool::Box, "annot.box")] {
                    ui.selectable_value(&mut a.tool, t, tr(key));
                }
                ui.separator();
                for c in COLORS {
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 7.0, c);
                    if a.color == c {
                        ui.painter().circle_stroke(r.center(), 8.5, Stroke::new(1.5, ui.visuals().text_color()));
                    }
                    if resp.clicked() {
                        a.color = c;
                    }
                }
                ui.separator();
                egui::ComboBox::from_id_salt(("annotate_thick", self.uid)).selected_text(tr(THICK[a.thick].1)).show_ui(ui, |ui| {
                    for (i, (_, key)) in THICK.iter().enumerate() {
                        ui.selectable_value(&mut a.thick, i, tr(key));
                    }
                });
                ui.separator();
                if ui.add_enabled(!a.marks.is_empty(), egui::Button::new(tr("annot.undo"))).clicked() {
                    a.marks.pop();
                }
                if ui.add_enabled(!a.marks.is_empty(), egui::Button::new(tr("annot.clear"))).clicked() {
                    a.marks.clear();
                }
            });
            ui.add_space(6.0);
            let (w, h) = (a.base.width as f32, a.base.height as f32);
            let s = (640.0 / w).min(440.0 / h).min(1.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w * s, h * s), Sense::drag());
            let key = crate::images::key(&a.img.data);
            if let Some(tex) = crate::images::texture(ui.ctx(), key, &a.img.data, false) {
                ui.painter().image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            let to_pic = |p: Pos2| pos2(((p.x - rect.min.x) / s).clamp(0.0, w), ((p.y - rect.min.y) / s).clamp(0.0, h));
            if resp.drag_started()
                && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
            {
                let p = to_pic(p);
                a.drawing = Some(Mark { tool: a.tool, color: a.color, width: a.width(), points: vec![p] });
            }
            if let (Some(m), Some(p)) = (&mut a.drawing, resp.interact_pointer_pos()) {
                let p = to_pic(p);
                if m.tool == Tool::Pen {
                    if m.points.last().is_none_or(|l| (*l - p).length() * s >= 1.5) {
                        m.points.push(p);
                    }
                } else {
                    m.points.truncate(1);
                    m.points.push(p);
                }
            }
            if resp.drag_stopped()
                && let Some(m) = a.drawing.take()
            {
                a.marks.push(m);
            }
            let painter = ui.painter_at(rect);
            for m in a.marks.iter().chain(&a.drawing) {
                let stroke = Stroke::new((m.width * s).max(1.0), m.color);
                for (p, q) in m.segments() {
                    let (p, q) = (rect.min + p.to_vec2() * s, rect.min + q.to_vec2() * s);
                    painter.line_segment([p, q], stroke);
                    painter.circle_filled(p, stroke.width / 2.0, m.color);
                    painter.circle_filled(q, stroke.width / 2.0, m.color);
                }
            }
            ui.ctx().set_cursor_icon(if resp.hovered() { egui::CursorIcon::Crosshair } else { egui::CursorIcon::Default });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tr("annot.tip")).small().weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("dialog.cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.add_enabled(!a.marks.is_empty(), egui::Button::new(tr("annot.apply"))).clicked() {
                        apply = true;
                    }
                });
            });
        });
        if apply {
            self.apply_annotations();
        } else if cancel || !open {
            self.pics.annotate = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_is_burnt_on_its_edges_only() {
        let (w, h) = (40, 30);
        let mut rgba = vec![255u8; w * h * 4];
        let mark = Mark { tool: Tool::Box, color: Color32::from_rgb(255, 0, 0), width: 3.0, points: vec![pos2(5.0, 5.0), pos2(35.0, 25.0)] };
        burn(&mut rgba, w, h, &[mark]);
        let at = |x: usize, y: usize| &rgba[(y * w + x) * 4..][..4];
        assert_eq!(at(5, 15), [255, 0, 0, 255]);
        assert_eq!(at(20, 15), [255, 255, 255, 255]);
        let arrow = Mark { tool: Tool::Arrow, color: Color32::BLACK, width: 2.0, points: vec![pos2(1.0, 1.0), pos2(30.0, 1.0)] };
        assert_eq!(arrow.segments().len(), 3);
    }
}
