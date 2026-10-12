//! Pictures in the document: layout, painting, selection and editing.

use std::sync::Arc;

use apex_doc::{Align, Image, OBJECT, Paragraph, Wrap};
use eframe::egui::text::TextFormat;
use eframe::egui::{self, Color32, Galley, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use super::{DocView, PT, ParaLayout, Segment};
use crate::i18n::tr;

/// Space between a picture and the text beside it, in points.
const GAP_PT: f32 = 9.0;

/// A picture placed in a paragraph layout.
#[derive(Clone)]
pub(super) struct Placed {
    /// Byte offset of its character.
    pub byte: usize,
    pub img: Arc<Image>,
    /// Relative to the galley origin.
    pub rect: Rect,
}

/// Room taken by a picture's effects around it (left, top, right, bottom)
/// at zoom `z` for a shown height `h`.
fn fx_room(img: &Image, z: f32, h: f32) -> [f32; 4] {
    img.fx.as_deref().map_or([0.0; 4], |fx| fx.extent(h / (PT * z).max(1e-3)).map(|v| v * PT * z))
}

/// Shown size at zoom `z`, shrunk to fit `width`.
pub(super) fn shown_size(img: &Image, z: f32, width: f32) -> Vec2 {
    let s = vec2(img.width, img.height) * PT * z;
    if s.x > width && s.x > 0.0 { s * (width / s.x) } else { s }
}

/// Size in the flow of the text: on a vertical page a picture lies on its
/// side (and is drawn upright, see `tategaki`).
fn flow_size(img: &Image, z: f32, width: f32, turned: bool) -> Vec2 {
    if !turned {
        return shown_size(img, z, width);
    }
    let s = vec2(img.height, img.width) * PT * z;
    if s.x > width && s.x > 0.0 { s * (width / s.x) } else { s }
}

/// [`fx_room`] of a picture `s` big in the flow of the text.
fn flow_room(img: &Image, z: f32, s: Vec2, turned: bool) -> [f32; 4] {
    if !turned {
        return fx_room(img, z, s.y);
    }
    let [l, t, r, b] = fx_room(img, z, s.x);
    [t, r, b, l]
}

fn images_of(p: &Paragraph) -> impl Iterator<Item = (usize, &Arc<Image>)> {
    p.spans().filter_map(|(s, e, a)| a.image.as_ref().map(|i| (s, e, i))).flat_map(move |(s, e, img)| p.text[s..e].match_indices(OBJECT).map(move |(o, _)| (s + o, img)))
}

/// Pictures with text beside them sit on the left or the right.
fn on_right(img: &Image, size: Vec2, z: f32, width: f32) -> bool {
    match img.align {
        Some(a) => a == Align::Right,
        None => img.offset_x * PT * z + size.x / 2.0 > width / 2.0,
    }
}

/// Widths taken on the left and right by pictures with text beside them.
pub(super) fn side_widths(p: &Paragraph, z: f32, width: f32, turned: bool) -> (f32, f32) {
    let (mut l, mut r) = (0.0f32, 0.0f32);
    if p.runs.iter().all(|r| r.attrs.image.is_none()) {
        return (0.0, 0.0);
    }
    for (_, img) in images_of(p) {
        if img.wrap != Wrap::Square {
            continue;
        }
        let size = flow_size(img, z, width * 0.6, turned);
        let [el, _, er, _] = flow_room(img, z, size, turned);
        let w = size.x + el + er + GAP_PT * PT * z;
        if on_right(img, size, z, width) {
            r = r.max(w);
        } else {
            l = l.max(w);
        }
    }
    (l, r)
}

/// Width of a no-break space per unit of font size.
pub(super) fn nbsp_ratio(ctx: &egui::Context) -> f32 {
    let font = crate::fonts::font(crate::fonts::Face::Sans, false, 100.0);
    (ctx.fonts_mut(|f| f.glyph_width(&font, '\u{A0}')) / 100.0).max(0.05)
}

/// Text format of a picture's character: an invisible no-break space in a
/// font size that makes it as wide as the picture, on a line as tall.
pub(super) fn object_format(base: &TextFormat, img: &Image, z: f32, width: f32, nbsp: f32, turned: bool) -> TextFormat {
    let mut f = base.clone();
    f.font_id = crate::fonts::font(crate::fonts::Face::Sans, false, 1.0);
    f.color = Color32::TRANSPARENT;
    f.underline = Stroke::NONE;
    f.strikethrough = Stroke::NONE;
    f.background = Color32::TRANSPARENT;
    f.italics = false;
    f.valign = egui::Align::BOTTOM;
    let s = flow_size(img, z, width, turned);
    let [el, et, er, eb] = flow_room(img, z, s, turned);
    match img.wrap {
        Wrap::Inline => {
            f.font_id.size = ((s.x + el + er) / nbsp).max(0.5);
            f.line_height = Some((s.y + et + eb).max(base.line_height.unwrap_or(0.0)));
        }
        // A line of its own.
        Wrap::TopBottom => {
            f.font_id.size = ((width - 1.0) / nbsp).max(0.5);
            f.line_height = Some(s.y + et + eb);
        }
        Wrap::Square => f.line_height = base.line_height,
    }
    f
}

/// Where the pictures of a paragraph go, relative to its galley.
pub(super) fn place(p: &Paragraph, galley: &Galley, z: f32, width: f32, align_off: f32, side_l: f32, turned: bool) -> Vec<Placed> {
    let mut out = Vec::new();
    if p.runs.iter().all(|r| r.attrs.image.is_none()) {
        return out;
    }
    let (mut y_l, mut y_r) = (0.0f32, 0.0f32);
    for (byte, img) in images_of(p) {
        let ci = p.text[..byte].chars().count();
        let rect = match img.wrap {
            Wrap::Square => {
                let full = width + side_l;
                let s = flow_size(img, z, full * 0.6, turned);
                let [el, et, _, eb] = flow_room(img, z, s, turned);
                let gap = GAP_PT * PT * z;
                if on_right(img, s, z, full) {
                    let r = Rect::from_min_size(pos2(width + gap + el - align_off, y_r + et), s);
                    y_r += s.y + et + eb + gap;
                    r
                } else {
                    let r = Rect::from_min_size(pos2(-side_l + el - align_off, y_l + et), s);
                    y_l += s.y + et + eb + gap;
                    r
                }
            }
            wrap => {
                let Some((x, row_bottom)) = glyph_at(galley, ci) else { continue };
                let s = flow_size(img, z, width, turned);
                let [el, _, er, eb] = flow_room(img, z, s, turned);
                let row_bottom = row_bottom - eb;
                let x = if wrap == Wrap::Inline {
                    x + el
                } else {
                    let room = (width - s.x - el - er).max(0.0);
                    -align_off
                        + match img.align {
                            Some(Align::Center) => room / 2.0,
                            Some(Align::Right) => room,
                            Some(_) => 0.0,
                            None => (img.offset_x * PT * z).clamp(0.0, room),
                        }
                        + el
                };
                Rect::from_min_size(pos2(x, row_bottom - s.y), s)
            }
        };
        out.push(Placed { byte, img: img.clone(), rect });
    }
    out
}

/// x of the glyph of char `ci` and its row's bottom.
pub(super) fn glyph_at(galley: &Galley, ci: usize) -> Option<(f32, f32)> {
    let mut start = 0usize;
    for row in &galley.rows {
        let n = row.row.glyphs.len() + usize::from(row.ends_with_newline);
        if ci < start + row.row.glyphs.len() {
            let g = &row.row.glyphs[ci - start];
            return Some((row.pos.x + g.pos.x, row.pos.y + row.row.size.y));
        }
        start += n;
    }
    None
}

impl DocView {
    /// Pictures whose top is in the segment.
    pub(super) fn paint_images(&self, p: &egui::Painter, pl: &ParaLayout, seg: &Segment, galley_pos: Pos2) {
        let last = seg.gy1 >= pl.galley.size().y - 0.5;
        for placed in pl.images.iter().filter(|i| !i.img.hidden) {
            let y = placed.rect.min.y;
            if y < seg.gy0 - 0.5 || (y >= seg.gy1 - 0.5 && !last) {
                continue;
            }
            let rect = placed.rect.translate(galley_pos.to_vec2());
            paint_image(p, rect, &placed.img);
        }
    }
}

/// Draws a picture in `rect` (cropped and rotated as set).
pub(crate) fn paint_image(p: &egui::Painter, rect: Rect, img: &Image) {
    if let Some(o) = &img.object {
        return crate::objects::paint(p, rect, &img.data, o, img.rotation, false);
    }
    crate::images::paint_picture(p, &img.data, rect, img.rotation, img.crop, img.fx.as_deref(), img.width, false);
}

// --- selection and editing ------------------------------------------------------

/// Mouse drag on a selected picture.
struct Drag {
    at: apex_doc::Pos,
    /// 0..4 corners (TL, TR, BR, BL), 4..8 edges (T, R, B, L), 8 = move.
    handle: usize,
    start: Pos2,
    rect: Rect,
    cur: Rect,
}

/// Picture properties being edited.
struct Props {
    at: apex_doc::Pos,
    img: Image,
    lock: bool,
    tab: usize,
}

#[derive(Default)]
pub(super) struct PicState {
    drag: Option<Drag>,
    props: Option<Props>,
    /// A picture being read from the system clipboard.
    incoming: Option<std::sync::mpsc::Receiver<Option<Vec<u8>>>>,
    /// Insert > Object, and what the application is asked to do with an
    /// object.
    insert_object: crate::objects::InsertDlg,
    object_request: Option<crate::objects::Request>,
    /// Insert > QR code, and the code it edits.
    barcode: crate::barcode::Dialog,
    barcode_at: Option<apex_doc::Pos>,
    /// Insert > Library.
    library: crate::library::Dialog,
    /// Annotations being drawn on a picture.
    pub(super) annotate: Option<super::annotate_ui::Annotator>,
}

impl PicState {
    pub fn dialog_open(&self) -> bool {
        self.props.is_some() || self.annotate.is_some()
    }
}

impl DocView {
    pub(super) fn pics_dragging(&self) -> bool {
        self.pics.drag.is_some()
    }
}

fn handles(r: Rect) -> [Pos2; 8] {
    let c = r.center();
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), pos2(c.x, r.top()), pos2(r.right(), c.y), pos2(c.x, r.bottom()), pos2(r.left(), c.y)]
}

impl DocView {
    /// Canvas rects of the pictures of the paragraphs `from..to`.
    fn picture_rects(&self, from: usize, to: usize) -> Vec<(apex_doc::Pos, Rect, Arc<Image>)> {
        let l = &self.layout;
        let mut out = Vec::new();
        for (i, pl) in l.paras.iter().enumerate().take(to).skip(from) {
            if pl.images.is_empty() {
                continue;
            }
            let segs = l.segs_of(i);
            for placed in &pl.images {
                let y = placed.rect.min.y;
                let seg = segs.iter().rev().find(|s| y >= s.gy0 - 0.5).unwrap_or(&segs[0]);
                let (galley_pos, _) = super::seg_geometry(l, seg, pl, self.seg_origin(seg));
                let r = placed.rect.translate(galley_pos.to_vec2());
                out.push((apex_doc::Pos::new(i, placed.byte), self.turn_of(seg.page).map_or(r, |t| t.rect(r)), placed.img.clone()));
            }
        }
        out
    }

    /// The picture under a canvas point.
    fn picture_at(&self, p: Pos2) -> Option<(apex_doc::Pos, Rect)> {
        // Only the paragraphs near the point can hold it (pictures beside
        // text may reach into the next paragraphs).
        let near = self.pos_at(p).para;
        let from = near.saturating_sub(8);
        let to = (near + 3).min(self.layout.paras.len());
        self.picture_rects(from, to).into_iter().rev().find(|(_, r, img)| !img.hidden && r.contains(p)).map(|(at, r, _)| (at, r))
    }

    pub(super) fn picture_at_point(&self, p: Pos2) -> bool {
        self.picture_at(p).is_some()
    }

    /// The picture selected: the selection is exactly one picture.
    pub(super) fn selected_picture(&self) -> Option<(apex_doc::Pos, Rect, Arc<Image>)> {
        let (a, b) = (self.sel.start(), self.sel.end());
        if a.para != b.para || b.offset != a.offset + OBJECT.len_utf8() {
            return None;
        }
        self.doc.image_at(a)?;
        self.picture_rects(a.para, a.para + 1).into_iter().find(|(at, _, _)| *at == a)
    }

    /// Mouse handling for pictures; true when the event was used.
    /// A click at `local` would take a picture (or a handle of the
    /// selected one).
    pub(super) fn picture_grab_at(&self, local: Pos2) -> bool {
        let on_selected = self.selected_picture().is_some_and(|(_, rect, _)| rect.contains(local) || handles(rect).iter().any(|h| (local - *h).length() <= 7.0));
        on_selected || self.picture_at(local).is_some()
    }

    pub(super) fn picture_mouse(&mut self, ui: &egui::Ui, resp: &egui::Response, local: Pos2) -> bool {
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        let down = ui.input(|i| i.pointer.primary_down());
        if let Some(d) = &mut self.pics.drag {
            if down {
                let delta = local - d.start;
                let shift = ui.input(|i| i.modifiers.shift);
                d.cur = drag_rect(d.rect, d.handle, delta, !shift);
                ui.ctx().request_repaint();
                return true;
            }
            let d = self.pics.drag.take().unwrap();
            if resp.double_clicked() && (local - d.start).length() < 3.0 {
                // An embedded object opens.
                if let Some(o) = self.doc.image_at(d.at).and_then(|(_, i)| i.object.clone()) {
                    self.pics.object_request = Some(crate::objects::Request::Open(o));
                    return true;
                }
            }
            self.finish_drag(d, local);
            return true;
        }
        if !pressed || !resp.hovered() {
            return false;
        }
        let hs = 5.0;
        if let Some((at, rect, img)) = self.selected_picture() {
            if let Some(h) = handles(rect).iter().position(|h| (local - *h).length() <= hs + 2.0) {
                self.pics.drag = Some(Drag { at, handle: h, start: local, rect, cur: rect });
                return true;
            }
            if rect.contains(local) {
                let _ = img;
                self.pics.drag = Some(Drag { at, handle: 8, start: local, rect, cur: rect });
                return true;
            }
        }
        if let Some((at, _)) = self.picture_at(local) {
            resp.request_focus();
            self.set_sel(apex_doc::Selection { anchor: at, cursor: apex_doc::Pos::new(at.para, at.offset + OBJECT.len_utf8()) });
            self.scroll_to_caret = false;
            return true;
        }
        false
    }

    fn finish_drag(&mut self, d: Drag, local: Pos2) {
        let Some((_, img)) = self.doc.image_at(d.at) else { return };
        let mut new = (*img).clone();
        if d.handle == 8 {
            let delta = local - d.start;
            if delta.length() < 3.0 {
                return;
            }
            if img.wrap == Wrap::Inline {
                // Moves the picture to where it was dropped.
                let to = self.pos_at(local);
                let sel = apex_doc::Selection { anchor: d.at, cursor: apex_doc::Pos::new(d.at.para, d.at.offset + OBJECT.len_utf8()) };
                if to >= sel.start() && to <= sel.end() {
                    return;
                }
                let at = self.doc.delete(sel);
                let _ = at;
                let to = if to.para == d.at.para && to.offset > d.at.offset { apex_doc::Pos::new(to.para, to.offset - OBJECT.len_utf8()) } else { to };
                let s = self.doc.insert_image(apex_doc::Selection::caret(self.doc.clamp(to)), new);
                self.set_sel(s);
                return;
            }
            new.align = None;
            new.offset_x = (img.offset_x + delta.x / (PT * self.zoom)).max(0.0);
        } else {
            if d.cur.width() < 2.0 || d.cur.height() < 2.0 || d.cur == d.rect {
                return;
            }
            new.width = (img.width * d.cur.width() / d.rect.width()).max(1.0);
            new.height = (img.height * d.cur.height() / d.rect.height()).max(1.0);
        }
        self.doc.set_image(d.at, new);
    }

    /// Selection frame and handles of the selected picture.
    pub(super) fn paint_picture_selection(&self, p: &egui::Painter, off: egui::Vec2) {
        let blue = Color32::from_rgb(0x1A, 0x73, 0xE8);
        if let Some(d) = &self.pics.drag {
            let r = d.cur.translate(off);
            if d.handle == 8 {
                let delta = p.ctx().input(|i| i.pointer.interact_pos()).map_or(egui::Vec2::ZERO, |q| q - off - d.start);
                p.rect_stroke(r.translate(delta), 0.0, Stroke::new(1.0, blue), egui::StrokeKind::Middle);
            } else {
                p.rect_stroke(r, 0.0, Stroke::new(1.0, blue), egui::StrokeKind::Middle);
            }
            return;
        }
        let Some((_, rect, img)) = self.selected_picture() else { return };
        let r = rect.translate(off);
        if img.rotation != 0.0 {
            crate::images::outline(p, r, img.rotation, Stroke::new(1.0, blue));
        }
        p.rect_stroke(r, 0.0, Stroke::new(1.0, blue), egui::StrokeKind::Middle);
        for h in handles(r) {
            p.circle_filled(h, 4.5, Color32::WHITE);
            p.circle_stroke(h, 4.5, Stroke::new(1.0, blue));
        }
    }

    /// Cursor over pictures and their handles.
    pub(super) fn picture_cursor(&self, local: Pos2) -> Option<egui::CursorIcon> {
        use egui::CursorIcon as C;
        if let Some(d) = &self.pics.drag {
            return Some(if d.handle == 8 { C::Grabbing } else { handle_cursor(d.handle) });
        }
        if let Some((_, rect, _)) = self.selected_picture() {
            if let Some(h) = handles(rect).iter().position(|h| (local - *h).length() <= 7.0) {
                return Some(handle_cursor(h));
            }
            if rect.contains(local) {
                return Some(C::Grab);
            }
        }
        self.picture_at(local).map(|_| C::Default)
    }

    /// Picture entries of the context menu (when one is selected).
    pub(super) fn picture_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let Some((at, _, img)) = self.selected_picture() else { return false };
        if let Some(o) = &img.object {
            self.object_menu(ui, o);
        }
        if ui.button(format!("{}…", tr("pic.properties"))).clicked() {
            self.open_picture_props(at, 0);
            ui.close();
        }
        if ui.button(format!("{}…", tr("pic.alt_text"))).clicked() {
            self.open_picture_props(at, 2);
            ui.close();
        }
        if super::annotate_ui::can_annotate(&img) && ui.button(format!("{}…", tr("annot.menu"))).clicked() {
            self.open_annotate(at, &img);
            ui.close();
        }
        crate::widgets::menu_button(ui, tr("pic.wrap"), |ui| self.picture_wrap_menu(ui));
        crate::widgets::menu_button(ui, tr("pic.rotate"), |ui| {
            for (d, key) in [(90.0, "pic.rotate_right"), (-90.0, "pic.rotate_left")] {
                if ui.button(tr(key)).clicked() {
                    self.doc.set_image(at, Image { rotation: (img.rotation + d).rem_euclid(360.0), ..(*img).clone() });
                    ui.close();
                }
            }
        });
        if ui.button(tr("pic.reset")).clicked() {
            let mut new = (*img).clone();
            reset_size(&mut new);
            self.doc.set_image(at, new);
            ui.close();
        }
        ui.separator();
        true
    }

    /// Text wrapping choices for the selected picture: in the text flow,
    /// or floating behind or in front of the text.
    pub(super) fn picture_wrap_menu(&mut self, ui: &mut egui::Ui) {
        let Some((at, rect, img)) = self.selected_picture() else { return };
        for (w, key) in [(Wrap::Inline, "pic.wrap_inline"), (Wrap::Square, "pic.wrap_square"), (Wrap::TopBottom, "pic.wrap_top_bottom")] {
            if ui.radio(img.wrap == w, tr(key)).clicked() {
                self.doc.set_image(at, Image { wrap: w, ..(*img).clone() });
                ui.close();
            }
        }
        for (behind, key) in [(true, "shape.wrap_behind"), (false, "shape.wrap_front")] {
            if ui.radio(false, tr(key)).clicked() {
                // It stays where it is on the page.
                let probe = apex_doc::DocShape::new(apex_doc::Shape::default(), img.width, img.height);
                let off = self.float_offsets(at.para, &probe, rect.min);
                self.doc.float_image(at, behind, Some(off));
                self.select_shape(at);
                ui.close();
            }
        }
    }

    /// Open and Save as file of an embedded object's menu.
    pub(super) fn object_menu(&mut self, ui: &mut egui::Ui, o: &Arc<apex_doc::Embedded>) {
        match crate::objects::menu(ui, o) {
            Some(crate::objects::MenuAction::Open) => self.pics.object_request = Some(crate::objects::Request::Open(o.clone())),
            Some(crate::objects::MenuAction::SaveAs) => crate::objects::save_as_file(o),
            None => {}
        }
    }

    /// Opens an embedded object (double click on a floating one).
    pub(super) fn open_object(&mut self, o: Arc<apex_doc::Embedded>) {
        self.pics.object_request = Some(crate::objects::Request::Open(o));
    }

    /// What the application is asked to do with an object.
    pub(crate) fn take_object_request(&mut self) -> Option<crate::objects::Request> {
        self.pics.object_request.take()
    }

    /// Insert > Object.
    pub(super) fn show_insert_object(&mut self) {
        self.pics.insert_object.show();
    }

    /// The Insert > Object dialog; the object goes where the cursor is.
    pub(super) fn object_dialog(&mut self, ctx: &egui::Context) {
        let Some(m) = self.pics.insert_object.ui(ctx) else { return };
        let img = Image { object: Some(m.object.clone()), ..Image::new(m.preview, m.size.0, m.size.1) };
        let s = self.doc.insert_image(self.sel, img);
        self.set_sel(s);
        if m.edit {
            self.pics.object_request = Some(crate::objects::Request::Open(m.object));
        }
    }

    /// Insert > QR code.
    pub(super) fn show_barcode(&mut self) {
        self.pics.barcode.show();
        self.pics.barcode_at = None;
    }

    /// Insert > Library.
    pub(super) fn show_library(&mut self) {
        self.pics.library.show();
    }

    /// Opens the code picture `img` (at `at`) in the QR code dialog; false
    /// if it isn't a code.
    fn edit_barcode(&mut self, at: apex_doc::Pos, img: &Image) -> bool {
        let Some((kind, text)) = crate::barcode::from_alt(&img.alt) else { return false };
        self.pics.barcode.edit(kind, text);
        self.pics.barcode_at = Some(at);
        true
    }

    pub(super) fn barcode_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.pics.library.ui(ctx) {
            self.insert_picture_file(&p);
        }
        if let Some(m) = self.pics.barcode.ui(ctx) {
            self.put_barcode(m);
        }
    }

    /// A new code goes at the caret; an edited one keeps its place, width
    /// and layout.
    fn put_barcode(&mut self, m: crate::barcode::Made) {
        if let Some((at, old)) = self.pics.barcode_at.take().and_then(|at| Some((at, self.doc.image_at(at)?.1))) {
            let height = old.width * m.size.1 / m.size.0;
            let img = Image { data: m.png, height, alt: m.alt, crop: [0.0; 4], fx: None, ..(*old).clone() };
            self.doc.set_image(at, img);
            return;
        }
        let s = (self.doc.page.content_width() / m.size.0).min(1.0);
        let img = Image { alt: m.alt, name: m.name, ..Image::new(m.png, m.size.0 * s, m.size.1 * s) };
        let sel = self.doc.insert_image(self.sel, img);
        self.set_sel(sel);
    }

    pub(super) fn open_picture_props(&mut self, at: apex_doc::Pos, tab: usize) {
        if let Some((_, img)) = self.doc.image_at(at) {
            self.pics.props = Some(Props { at, img: (*img).clone(), lock: true, tab });
        }
    }

    /// Opens the picture properties of the selected picture.
    pub(super) fn picture_props_selected(&mut self) -> bool {
        match self.selected_picture() {
            // An embedded object opens instead, and a code in its dialog.
            Some((_, _, img)) if img.object.is_some() => {
                self.pics.object_request = img.object.clone().map(crate::objects::Request::Open);
                true
            }
            Some((at, _, img)) if self.edit_barcode(at, &img) => true,
            Some((at, _, _)) => {
                self.open_picture_props(at, 0);
                true
            }
            None => false,
        }
    }

    pub(super) fn picture_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.pics.props else { return };
        let pages = [(crate::icons::Icon::AutoFit, "pic.tab_size"), (crate::icons::Icon::Wrap, "pic.tab_layout"), (crate::icons::Icon::Comment, "pic.tab_alt"), (crate::icons::Icon::Brightness, "picfx.tab")];
        let mut tab = d.tab;
        let res = crate::widgets::nav_dialog(ctx, "pic-props", tr("pic.properties"), &pages, &mut tab, egui::vec2(440.0, 380.0), |ui, page| {
            let img = &mut d.img;
            crate::widgets::section(ui, tr(if page == 1 { "pic.wrap" } else { pages[page].1 }));
            match page {
                0 => {
                    let cm = |pt: f32| pt / 72.0 * 2.54;
                    crate::widgets::form(ui, "pic-size", |ui| {
                        let (mut w, mut h) = (cm(img.width), cm(img.height));
                        ui.label(tr("pic.width"));
                        let cw = ui.add(egui::DragValue::new(&mut w).range(0.05..=100.0).speed(0.05).max_decimals(2).suffix(" cm")).changed();
                        ui.end_row();
                        ui.label(tr("pic.height"));
                        let ch = ui.add(egui::DragValue::new(&mut h).range(0.05..=100.0).speed(0.05).max_decimals(2).suffix(" cm")).changed();
                        ui.end_row();
                        if cw {
                            let r = img.height / img.width.max(0.01);
                            img.width = w / 2.54 * 72.0;
                            if d.lock {
                                img.height = img.width * r;
                            }
                        }
                        if ch {
                            let r = img.width / img.height.max(0.01);
                            img.height = h / 2.54 * 72.0;
                            if d.lock {
                                img.width = img.height * r;
                            }
                        }
                        ui.label("");
                        ui.checkbox(&mut d.lock, tr("pic.lock_aspect"));
                        ui.end_row();
                        ui.label(tr("pic.rotation"));
                        ui.add(egui::DragValue::new(&mut img.rotation).range(0.0..=359.9).speed(1.0).max_decimals(1).suffix("°"));
                        ui.end_row();
                    });
                    crate::widgets::section(ui, tr("pic.crop"));
                    egui::Grid::new("pic-crop").num_columns(4).spacing([10.0, 8.0]).show(ui, |ui| {
                        let before = img.crop;
                        for (i, key) in ["pic.left", "pic.top", "pic.right", "pic.bottom"].iter().enumerate() {
                            let mut v = img.crop[i] * 100.0;
                            ui.label(tr(key));
                            if ui.add(egui::DragValue::new(&mut v).range(0.0..=90.0).speed(0.5).max_decimals(1).suffix(" %")).changed() {
                                img.crop[i] = v / 100.0;
                            }
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                        // Keep the scale: cropping changes the shown size.
                        if img.crop != before {
                            let (fx0, fy0) = (1.0 - before[0] - before[2], 1.0 - before[1] - before[3]);
                            let (fx1, fy1) = (1.0 - img.crop[0] - img.crop[2], 1.0 - img.crop[1] - img.crop[3]);
                            if fx0 > 0.01 && fy0 > 0.01 && fx1 > 0.01 && fy1 > 0.01 {
                                img.width *= fx1 / fx0;
                                img.height *= fy1 / fy0;
                            } else {
                                img.crop = before;
                            }
                        }
                    });
                    ui.add_space(10.0);
                    if ui.button(tr("pic.reset")).clicked() {
                        reset_size(img);
                    }
                }
                1 => {
                    for (w, key) in [(Wrap::Inline, "pic.wrap_inline"), (Wrap::Square, "pic.wrap_square"), (Wrap::TopBottom, "pic.wrap_top_bottom")] {
                        ui.radio_value(&mut img.wrap, w, tr(key));
                    }
                    ui.add_enabled_ui(img.wrap != Wrap::Inline, |ui| {
                        crate::widgets::section(ui, tr("pic.position"));
                        ui.horizontal(|ui| {
                            for (a, key) in [(Some(Align::Left), "pic.left"), (Some(Align::Center), "pic.center"), (Some(Align::Right), "pic.right"), (None, "pic.offset")] {
                                ui.radio_value(&mut img.align, a, tr(key));
                            }
                        });
                        if img.align.is_none() {
                            let mut v = img.offset_x / 72.0 * 2.54;
                            if ui.add(egui::DragValue::new(&mut v).range(0.0..=50.0).speed(0.05).max_decimals(2).suffix(" cm")).changed() {
                                img.offset_x = v / 2.54 * 72.0;
                            }
                        }
                    });
                }
                super::pic_fx_ui::FX_PAGE => super::pic_fx_ui::fx_page(ui, img),
                _ => {
                    ui.label(tr("pic.alt_hint"));
                    let id = ui.add(egui::TextEdit::multiline(&mut img.alt).desired_rows(5).desired_width(f32::INFINITY)).id;
                    ui.data_mut(|m| m.insert_temp(id.with("multiline"), true));
                    ui.add_space(10.0);
                    crate::widgets::form(ui, "pic-name", |ui| {
                        crate::widgets::form_row(ui, tr("pic.name"), |ui| ui.add(egui::TextEdit::singleline(&mut img.name).desired_width(220.0)));
                    });
                }
            }
        });
        d.tab = tab;
        let ok = res == crate::widgets::DialogResult::Ok;
        if res != crate::widgets::DialogResult::None {
            if let Some(d) = self.pics.props.take().filter(|_| ok) {
                if self.doc.image_at(d.at).is_some_and(|(_, i)| *i != d.img) {
                    self.doc.set_image(d.at, d.img);
                }
            }
            self.refocus = true;
        }
    }
}

/// The natural size (the file's pixels at 96 dpi) with the crop applied.
fn reset_size(img: &mut Image) {
    if let Some((w, h)) = apex_io::pixel_size(&img.data) {
        img.width = w as f32 * 0.75 * (1.0 - img.crop[0] - img.crop[2]).max(0.01);
        img.height = h as f32 * 0.75 * (1.0 - img.crop[1] - img.crop[3]).max(0.01);
    }
    img.rotation = 0.0;
}

fn handle_cursor(h: usize) -> egui::CursorIcon {
    use egui::CursorIcon as C;
    match h {
        0 | 2 => C::ResizeNwSe,
        1 | 3 => C::ResizeNeSw,
        4 | 6 => C::ResizeVertical,
        _ => C::ResizeHorizontal,
    }
}

/// The rect after dragging `handle` by `delta` (corners keep the aspect
/// ratio when `keep`).
fn drag_rect(r: Rect, handle: usize, delta: egui::Vec2, keep: bool) -> Rect {
    let mut n = r;
    match handle {
        0 => n.min += delta,
        1 => {
            n.max.x += delta.x;
            n.min.y += delta.y;
        }
        2 => n.max += delta,
        3 => {
            n.min.x += delta.x;
            n.max.y += delta.y;
        }
        4 => n.min.y += delta.y,
        5 => n.max.x += delta.x,
        6 => n.max.y += delta.y,
        7 => n.min.x += delta.x,
        _ => return r,
    }
    let (w, h) = (n.width().max(4.0), n.height().max(4.0));
    if keep && handle < 4 && r.width() > 0.0 && r.height() > 0.0 {
        // The larger change wins; the opposite corner stays.
        let s = (w / r.width()).max(h / r.height());
        let size = r.size() * s;
        let anchor = handles(r)[(handle + 2) % 4];
        let dir = egui::vec2(if handle == 0 || handle == 3 { -1.0 } else { 1.0 }, if handle < 2 { -1.0 } else { 1.0 });
        let other = anchor + size * dir;
        return Rect::from_two_pos(anchor, other);
    }
    Rect::from_min_size(n.min, egui::vec2(w, h))
}

impl DocView {
    /// Asks for a picture file and inserts it at the caret.
    pub(super) fn pick_picture(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("pic.insert")).add_filter(tr("pic.files"), &["png", "jpg", "jpeg", "gif", "bmp", "webp"]).pick_file(crate::file_dialog::in_view(|v: &mut DocView, path: std::path::PathBuf| v.insert_picture_file(&path)));
    }

    pub(crate) fn insert_picture_file(&mut self, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(data) => {
                if !self.insert_picture_bytes(data) {
                    self.notices.push(crate::app::Notice::error(crate::i18n::trf("pic.bad_file", &[("file", &path.display().to_string())])));
                }
            }
            Err(e) => self.notices.push(crate::app::Notice::error(e.to_string())),
        }
    }

    /// Inserts an encoded picture at the caret, at its natural size (shrunk
    /// to fit the page). False if it isn't a picture.
    pub(crate) fn insert_picture_bytes(&mut self, data: Vec<u8>) -> bool {
        let Some((w, h)) = apex_io::pixel_size(&data).filter(|&(w, h)| w > 0 && h > 0) else { return false };
        let (mut w, mut h) = (w as f32 * 0.75, h as f32 * 0.75);
        let max_w = self.doc.page.content_width();
        let max_h = self.doc.page.content_height();
        let s = (max_w / w).min(max_h / h).min(1.0);
        (w, h) = (w * s, h * s);
        let sel = self.doc.insert_image(self.sel, Image::new(Arc::from(data), w, h));
        self.set_sel(sel);
        true
    }
}

impl DocView {
    /// Insert > Chart: a chart of type `CHART_TYPES[i]` with sample data,
    /// whose data opens in a tab to be typed over (as in Word).
    pub(super) fn insert_chart_object(&mut self, i: usize) {
        let (key, _, kind, grouping, flags) = crate::sheet_view::CHART_TYPES[i];
        let mut wb = apex_io::chart_object::sample(kind, tr("chart.sample_category"), tr("chart.sample_series"));
        if let Some(c) = wb.sheets[0].charts.first_mut() {
            crate::sheet_view::apply_chart_type(c, grouping, flags, key == "chart.t.combo");
        }
        let book = apex_io::chart_object::book(&wb);
        self.insert_chart_book(book, (360.0, 216.0), true);
    }

    /// Inserts a chart object with workbook `book`, `size` points (shrunk
    /// to the page); `edit` opens its data.
    fn insert_chart_book(&mut self, book: Arc<[u8]>, size: (f32, f32), edit: bool) {
        let s = (self.doc.page.content_width() / size.0).min(self.doc.page.content_height() / size.1).min(1.0);
        let obj = Arc::new(apex_doc::Embedded::new(apex_doc::EmbedKind::Chart(apex_doc::ChartObject { book })));
        let img = Image { object: Some(obj.clone()), ..Image::new(Arc::from(&[][..]), size.0 * s, size.1 * s) };
        let sel = self.doc.insert_image(self.sel, img);
        self.set_sel(sel);
        if edit {
            self.pics.object_request = Some(crate::objects::Request::Open(obj));
        }
    }

    /// Looks for a picture on the system clipboard in the background.
    pub(super) fn paste_clipboard_picture(&mut self, ctx: &egui::Context) {
        if self.pics.incoming.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new().name("apex-clipboard".into()).spawn(move || {
            let _ = tx.send(crate::images::read_clipboard_image());
            ctx.request_repaint();
        });
        if started.is_ok() {
            self.pics.incoming = Some(rx);
        }
    }

    /// Inserts the picture read from the clipboard, once it arrives.
    pub(super) fn poll_clipboard_picture(&mut self) {
        let Some(rx) = &self.pics.incoming else { return };
        let got = match rx.try_recv() {
            Ok(v) => v,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        };
        self.pics.incoming = None;
        if let Some(bytes) = got {
            // A chart copied from a sheet comes as a live chart with its data.
            match crate::charts::chart_clip_for(&bytes) {
                Some((book, size)) => self.insert_chart_book(book, size, false),
                None => {
                    self.insert_picture_bytes(bytes);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::barcode::Kind;
    use apex_doc::{Document, Paragraph, Pos, Selection};

    #[test]
    fn codes_open_again_and_keep_their_width() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Hi")]));
        v.sel = Selection::caret(Pos::new(0, 2));
        v.put_barcode(crate::barcode::made(&ctx, Kind::Qr, "hola").unwrap());
        let (at, img) = v.doc.images()[0].clone();
        assert_eq!((img.alt.as_str(), img.name.as_str()), ("QR: hola", "QR"));
        assert!(v.edit_barcode(at, &img));
        assert_eq!(v.pics.barcode.editing(), Some((Kind::Qr, "hola")));
        // Edited: the new code in its place, as wide as the old one.
        let m = crate::barcode::made(&ctx, Kind::Code128, "ABC").unwrap();
        let aspect = m.size.1 / m.size.0;
        v.put_barcode(m);
        let imgs = v.doc.images();
        assert_eq!(imgs.len(), 1);
        assert_eq!((imgs[0].1.alt.as_str(), imgs[0].1.width), ("Code 128: ABC", img.width));
        assert!((imgs[0].1.height - img.width * aspect).abs() < 0.01, "its own shape");
        // A photo isn't a code.
        assert!(!v.edit_barcode(at, &Image::new(img.data.clone(), 10.0, 10.0)));
    }
}
