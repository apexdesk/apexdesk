//! Shapes and text boxes in the document: their place in the layout (in
//! the text like pictures, or floating over or under it relative to the
//! page or their paragraph), painting, drawing them from the gallery,
//! selection, moving, resizing and turning, text, and the Format Shape
//! dialog.

use std::sync::Arc;

use apex_doc::{Align, DocShape, HRel, OBJECT, Paragraph, Pos, Selection, ShapeKind, ShapeWrap, VRel, shapes_of};
use eframe::egui::text::TextFormat;
use eframe::egui::{self, Color32, CursorIcon, Event, Galley, Key, Pos2, Rect, Stroke, pos2, vec2};

use super::sections::PageGeo;
use super::{DocView, Layout, PT, ParaLayout, Segment, images_ui, seg_geometry};
use crate::i18n::tr;
use crate::shapes::{self, BoxGeo, EditResult, FormatDlg, Handle, Template, TextEdit};
use crate::widgets::DialogResult;

/// Space between a shape and the text beside it, in points.
const GAP_PT: f32 = 9.0;
/// Centimeters per point.
const CM: f32 = 2.54 / 72.0;
pub(super) const ACCENT: Color32 = Color32::from_rgb(0x1A, 0x73, 0xE8);

/// A shape of a paragraph layout.
#[derive(Clone)]
pub(super) struct PlacedShape {
    /// Byte offset of its character.
    pub byte: usize,
    pub shape: Arc<DocShape>,
    /// In the text: relative to the galley (floating shapes are placed
    /// on their page when painted).
    pub rect: Rect,
}

fn size_px(s: &DocShape, z: f32) -> egui::Vec2 {
    vec2(s.width, s.height) * PT * z
}

/// In-flow size, shrunk to fit `width`.
fn shown_size(s: &DocShape, z: f32, width: f32) -> egui::Vec2 {
    let v = size_px(s, z);
    let v = if v.x > width && v.x > 0.0 { v * (width / v.x) } else { v };
    vec2(v.x.max(1.0), v.y.max(1.0))
}

fn on_right(s: &DocShape, size: egui::Vec2, z: f32, width: f32) -> bool {
    match s.h_align {
        Some(a) => a == Align::Right,
        None => s.x * PT * z + size.x / 2.0 > width / 2.0,
    }
}

/// Widths taken on the left and right by shapes with text beside them.
pub(super) fn side_widths(p: &Paragraph, z: f32, width: f32) -> (f32, f32) {
    let (mut l, mut r) = (0.0f32, 0.0f32);
    if !apex_doc::has_shapes(p) {
        return (0.0, 0.0);
    }
    for (_, s) in shapes_of(p) {
        if s.wrap != ShapeWrap::Square {
            continue;
        }
        let size = shown_size(s, z, width * 0.6);
        let w = size.x + GAP_PT * PT * z;
        if on_right(s, size, z, width) {
            r = r.max(w);
        } else {
            l = l.max(w);
        }
    }
    (l, r)
}

/// Text format of a shape's character: invisible, as wide as the shape in
/// the line, a whole line for top and bottom wrapping, next to nothing for
/// floating shapes.
pub(super) fn object_format(base: &TextFormat, s: &DocShape, z: f32, width: f32, nbsp: f32) -> TextFormat {
    let mut f = base.clone();
    f.font_id = crate::fonts::font(crate::fonts::Face::Sans, false, 0.5);
    f.color = Color32::TRANSPARENT;
    f.underline = Stroke::NONE;
    f.strikethrough = Stroke::NONE;
    f.background = Color32::TRANSPARENT;
    f.italics = false;
    f.valign = egui::Align::BOTTOM;
    let size = shown_size(s, z, width);
    match s.wrap {
        ShapeWrap::Inline => {
            f.font_id.size = (size.x / nbsp).max(0.5);
            f.line_height = Some(size.y.max(base.line_height.unwrap_or(0.0)));
        }
        ShapeWrap::TopBottom => {
            f.font_id.size = ((width - 1.0) / nbsp).max(0.5);
            f.line_height = Some(size.y);
        }
        _ => f.line_height = base.line_height,
    }
    f
}

/// The shapes of a paragraph; those in the text placed relative to its
/// galley.
pub(super) fn place(p: &Paragraph, galley: &Galley, z: f32, width: f32, align_off: f32, side_l: f32) -> Vec<PlacedShape> {
    let mut out = Vec::new();
    if !apex_doc::has_shapes(p) {
        return out;
    }
    let (mut y_l, mut y_r) = (0.0f32, 0.0f32);
    for (byte, s) in shapes_of(p) {
        let rect = match s.wrap {
            ShapeWrap::InFront | ShapeWrap::Behind => Rect::NOTHING,
            ShapeWrap::Square => {
                let full = width + side_l;
                let size = shown_size(s, z, full * 0.6);
                let gap = GAP_PT * PT * z;
                if on_right(s, size, z, full) {
                    let r = Rect::from_min_size(pos2(width + gap - align_off, y_r), size);
                    y_r += size.y + gap;
                    r
                } else {
                    let r = Rect::from_min_size(pos2(-side_l - align_off, y_l), size);
                    y_l += size.y + gap;
                    r
                }
            }
            wrap => {
                let ci = p.text[..byte].chars().count();
                let Some((x, row_bottom)) = images_ui::glyph_at(galley, ci) else { continue };
                let size = shown_size(s, z, width);
                let x = if wrap == ShapeWrap::Inline {
                    x
                } else {
                    let room = (width - size.x).max(0.0);
                    -align_off
                        + match s.h_align {
                            Some(Align::Center) => room / 2.0,
                            Some(Align::Right) => room,
                            Some(_) => 0.0,
                            None => (s.x * PT * z).clamp(0.0, room),
                        }
                };
                Rect::from_min_size(pos2(x, row_bottom - size.y), size)
            }
        };
        out.push(PlacedShape { byte, shape: s.clone(), rect });
    }
    out
}

/// Height the shapes beside the text need.
pub(super) fn side_height(shapes: &[PlacedShape]) -> f32 {
    shapes.iter().filter(|s| s.shape.wrap == ShapeWrap::Square).map(|s| s.rect.max.y).fold(0.0, f32::max)
}

/// The frame a floating shape's offsets are measured from: left and top.
fn frame(g: &PageGeo, page_min: Pos2, seg: &Segment, s: &DocShape) -> (f32, f32, f32) {
    let content = page_min + g.margin;
    let (x0, span) = match s.h_rel {
        HRel::Margin => (content.x, g.content.x),
        HRel::Page => (page_min.x, g.size.x),
    };
    let y0 = match s.v_rel {
        // (Vertical pages: from the top margin.)
        VRel::Paragraph if g.vertical => content.y,
        VRel::Paragraph => content.y + seg.y,
        VRel::Margin => content.y,
        VRel::Page => page_min.y,
    };
    (x0, span, y0)
}

/// Unturned rect of a floating shape anchored to the paragraph whose
/// first segment is `seg`, on a page with its top-left corner at `page_min`.
fn float_rect(g: &PageGeo, page_min: Pos2, seg: &Segment, s: &DocShape, z: f32) -> Rect {
    let pt = PT * z;
    let size = size_px(s, z);
    let (x0, span, y0) = frame(g, page_min, seg, s);
    let x = match s.h_align {
        Some(Align::Center) => x0 + (span - size.x) / 2.0,
        Some(Align::Right) => x0 + span - size.x,
        Some(_) => x0,
        None => x0 + s.x * pt,
    };
    Rect::from_min_size(pos2(x, y0 + s.y * pt), size)
}

/// A shape on the canvas (canvas coordinates).
#[derive(Clone)]
pub(super) struct Boxed {
    pub at: Pos,
    pub rect: Rect,
    pub shape: Arc<DocShape>,
}

struct ShapeDrag {
    at: Pos,
    handle: Handle,
    start: Pos2,
    orig: BoxGeo,
    shape: Arc<DocShape>,
    cur: BoxGeo,
    moved: bool,
    /// Shapes moving along (its group, the other picked shapes).
    with: Vec<Boxed>,
}

struct Dlg {
    at: Pos,
    orig: Arc<DocShape>,
    d: FormatDlg,
    place: DocShape,
}

#[derive(Default)]
pub(super) struct ShapesState {
    /// The shape picked in the gallery, drawn by the next drag.
    draw: Option<Template>,
    /// Drawing it: the two corners (canvas).
    drawing: Option<(Pos2, Pos2)>,
    drag: Option<ShapeDrag>,
    dialog: Option<Dlg>,
    /// Text being typed into a shape.
    edit: Option<(Pos, TextEdit, DocShape)>,
    /// Shapes picked with Ctrl+click besides the selected one, while the
    /// selection is the one saved with them.
    pub multi: (Selection, Vec<Pos>),
}

impl ShapesState {
    /// The shape the next drag draws, when one was picked.
    pub(super) fn armed(&self) -> Option<&crate::shapes::Template> {
        self.draw.as_ref()
    }

    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// The mouse is drawing or dragging a shape.
    pub fn busy(&self) -> bool {
        self.drawing.is_some() || self.drag.is_some()
    }

    pub fn editing(&self) -> bool {
        self.edit.is_some()
    }
}

impl DocView {
    /// Top-left corner of a page on the canvas.
    pub(super) fn page_min(&self, page: usize) -> Pos2 {
        let g = self.layout.geo_of(page);
        pos2(self.page_x + g.dx, g.top)
    }

    /// The shapes of paragraph `i` on the canvas.
    fn boxes_of(&self, i: usize) -> Vec<Boxed> {
        let l = &self.layout;
        let Some(pl) = l.paras.get(i) else { return Vec::new() };
        if pl.shapes.is_empty() {
            return Vec::new();
        }
        let segs = l.segs_of(i);
        let Some(first) = segs.first() else { return Vec::new() };
        pl.shapes
            .iter()
            .map(|ps| {
                let rect = if ps.shape.wrap.floats() {
                    float_rect(&l.geo_of(first.page), self.page_min(first.page), first, &ps.shape, self.zoom)
                } else {
                    let y = ps.rect.min.y;
                    let seg = segs.iter().rev().find(|s| y >= s.gy0 - 0.5).unwrap_or(first);
                    let (gp, _) = seg_geometry(l, seg, pl, self.seg_origin(seg));
                    let r = ps.rect.translate(gp.to_vec2());
                    self.turn_of(seg.page).map_or(r, |t| t.rect(r))
                };
                Boxed { at: Pos::new(i, ps.byte), rect, shape: ps.shape.clone() }
            })
            .collect()
    }

    /// Every shape on the canvas, the topmost last.
    pub(super) fn all_boxes(&self) -> Vec<Boxed> {
        let mut v: Vec<Boxed> = (0..self.layout.paras.len()).filter(|&i| !self.layout.paras[i].shapes.is_empty()).flat_map(|i| self.boxes_of(i)).collect();
        v.sort_by_key(|b| layer(&b.shape));
        v
    }

    /// The shape under a canvas point (shapes behind the text only by
    /// their edges, so that clicks on the text still place the caret).
    fn shape_hit(&self, q: Pos2) -> Option<Boxed> {
        self.all_boxes().into_iter().rev().filter(|b| !b.shape.shape.hidden).find(|b| {
            let s = &b.shape.shape;
            if !shapes::hit(s, b.rect, q) {
                return false;
            }
            if b.shape.wrap != ShapeWrap::Behind || s.kind.is_line() {
                return true;
            }
            let l = shapes::local(b.rect, s.rotation, q);
            l.x.abs() > b.rect.width() / 2.0 - shapes::GRAB || l.y.abs() > b.rect.height() / 2.0 - shapes::GRAB
        })
    }

    /// The selected shape: the selection is exactly one shape.
    pub(super) fn selected_shape(&self) -> Option<Boxed> {
        let (a, b) = (self.sel.start(), self.sel.end());
        if a.para != b.para || b.offset != a.offset + OBJECT.len_utf8() {
            return None;
        }
        self.doc.shape_at(a)?;
        self.boxes_of(a.para).into_iter().find(|x| x.at == a)
    }

    pub(super) fn select_shape(&mut self, at: Pos) {
        self.set_sel(Selection { anchor: at, cursor: Pos::new(at.para, at.offset + OBJECT.len_utf8()) });
        self.scroll_to_caret = false;
    }

    fn editing_at(&self, at: Pos) -> bool {
        self.shapes.edit.as_ref().is_some_and(|(p, _, _)| *p == at)
    }

    fn dragging_at(&self, at: Pos) -> bool {
        self.shapes.drag.as_ref().is_some_and(|d| d.moved && (d.at == at || d.with.iter().any(|b| b.at == at)))
    }

    // --- painting ----------------------------------------------------------------------

    /// The shape as shown: a linked text box shows its part of its
    /// chain's text.
    fn shown<'a>(&self, p: &egui::Painter, s: &'a apex_doc::Shape) -> std::borrow::Cow<'a, apex_doc::Shape> {
        if s.ink.is_some() {
            return self.ink.shown(s);
        }
        let Some(link) = s.link else { return std::borrow::Cow::Borrowed(s) };
        let chain = self.doc.text_chain(link.chain);
        let Some(idx) = chain.iter().position(|(_, x)| x.shape.link == Some(link)) else { return std::borrow::Cow::Borrowed(s) };
        let sizes: Vec<(f32, f32)> = chain.iter().map(|(_, x)| (x.width, x.height)).collect();
        let mut parts = shapes::flow_chain(p, &chain[0].1.shape, &sizes);
        let mut out = s.clone();
        out.text = std::mem::take(&mut parts[idx]);
        std::borrow::Cow::Owned(out)
    }

    /// Shapes in the text whose top is in the segment.
    pub(super) fn paint_flow_shapes(&self, p: &egui::Painter, pl: &ParaLayout, seg: &Segment, galley_pos: Pos2, z: f32) {
        let last = seg.gy1 >= pl.galley.size().y - 0.5;
        for ps in pl.shapes.iter().filter(|s| !s.shape.wrap.floats() && !s.shape.shape.hidden) {
            let y = ps.rect.min.y;
            if y < seg.gy0 - 0.5 || (y >= seg.gy1 - 0.5 && !last) {
                continue;
            }
            let at = Pos::new(seg.para, ps.byte);
            if self.dragging_at(at) {
                continue;
            }
            shapes::paint(p, ps.rect.translate(galley_pos.to_vec2()), &self.shown(p, &ps.shape.shape), PT * z, !self.editing_at(at));
        }
    }

    /// Floating shapes behind (or in front of) the text of a page whose
    /// top-left corner is at `page_min`, in layout `l`.
    pub(super) fn paint_floats(&self, p: &egui::Painter, l: &Layout, page: usize, page_min: Pos2, z: f32, behind: bool) {
        let g = l.geo_of(page);
        let mut list: Vec<(Pos, Rect, &Arc<DocShape>)> = Vec::new();
        for seg in l.page_segs(page, page) {
            if seg.gy0 != 0.0 || seg.hidden {
                continue;
            }
            for ps in &l.paras[seg.para].shapes {
                if ps.shape.wrap.floats() && !ps.shape.shape.hidden && (ps.shape.wrap == ShapeWrap::Behind) == behind {
                    list.push((Pos::new(seg.para, ps.byte), float_rect(&g, page_min, seg, &ps.shape, z), &ps.shape));
                }
            }
        }
        list.sort_by_key(|(_, _, s)| s.z);
        for (at, rect, s) in list {
            if !self.dragging_at(at) {
                shapes::paint(p, rect, &self.shown(p, &s.shape), PT * z, !self.editing_at(at));
            }
        }
    }

    /// Floats of the pages on screen.
    pub(super) fn paint_screen_floats(&self, p: &egui::Painter, first: usize, last: usize, off: egui::Vec2, behind: bool) {
        for page in first..=last {
            self.paint_floats(p, &self.layout, page, self.page_min(page) + off, self.zoom, behind);
        }
    }

    /// The shape being dragged or drawn, and the selected shape's handles.
    pub(super) fn paint_shape_selection(&self, p: &egui::Painter, off: egui::Vec2) {
        let scale = PT * self.zoom;
        if let (Some(t), Some((a, b))) = (self.shapes.draw, self.shapes.drawing) {
            let bx = shapes::drawn(t.kind, a, b, p.ctx().input(|i| i.modifiers.shift));
            let mut s = t.shape();
            bx.apply(&mut s);
            shapes::paint(p, Rect::from_min_size(pos2(bx.x, bx.y), vec2(bx.w, bx.h)).translate(off), &s, scale, false);
            return;
        }
        if let Some(d) = self.shapes.drag.as_ref().filter(|d| d.moved) {
            let mut s = d.shape.shape.clone();
            d.cur.apply(&mut s);
            let s = self.shown(p, &s).into_owned();
            let rect = Rect::from_min_size(pos2(d.cur.x, d.cur.y), vec2(d.cur.w, d.cur.h)).translate(off);
            let moved = pos2(d.cur.x, d.cur.y) - pos2(d.orig.x, d.orig.y);
            for b in &d.with {
                shapes::paint(p, b.rect.translate(off + moved), &self.shown(p, &b.shape.shape), scale, true);
            }
            shapes::paint(p, rect, &s, scale, true);
            shapes::paint_handles(p, &s, rect, ACCENT);
            if !d.with.is_empty() {
                self.paint_picked(p, off, moved);
            }
            return;
        }
        if let Some(b) = self.selected_shape() {
            shapes::paint_handles(p, &b.shape.shape, b.rect.translate(off), ACCENT);
            self.paint_picked(p, off, egui::Vec2::ZERO);
        }
    }

    // --- mouse -------------------------------------------------------------------------

    pub(super) fn shape_cursor(&self, local: Pos2) -> Option<CursorIcon> {
        if let Some(c) = self.ink_cursor(local) {
            return Some(c);
        }
        if self.shapes.draw.is_some() {
            return Some(CursorIcon::Crosshair);
        }
        if let Some(d) = &self.shapes.drag {
            return Some(shapes::cursor(d.handle, d.orig.rotation));
        }
        if let Some(b) = self.selected_shape() {
            if let Some(h) = shapes::handle_at(&b.shape.shape, b.rect, local) {
                return Some(shapes::cursor(h, b.shape.shape.rotation));
            }
        }
        self.shape_hit(local).map(|_| CursorIcon::Move)
    }

    /// Mouse handling for shapes; true when the event was used.
    /// A click at `local` would take a shape (or a handle of the selected
    /// one).
    pub(super) fn shape_grab_at(&self, local: Pos2) -> bool {
        self.selected_shape().is_some_and(|b| shapes::handle_at(&b.shape.shape, b.rect, local).is_some()) || self.shape_hit(local).is_some()
    }

    pub(super) fn shape_mouse(&mut self, ui: &egui::Ui, resp: &egui::Response, local: Pos2) -> bool {
        let (pressed, down, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.modifiers.shift));
        if let Some(t) = self.shapes.draw {
            if let Some((a, _)) = self.shapes.drawing {
                if down {
                    self.shapes.drawing = Some((a, local));
                    ui.ctx().request_repaint();
                    return true;
                }
                self.shapes.drawing = None;
                self.shapes.draw = None;
                self.insert_drawn(t, a, local, shift);
                return true;
            }
            if pressed && resp.hovered() {
                resp.request_focus();
                self.shapes.drawing = Some((local, local));
                return true;
            }
            return false;
        }
        if let Some(d) = &mut self.shapes.drag {
            if !d.moved && (local - d.start).length() > 3.0 {
                d.moved = true;
            }
            if down {
                if d.moved {
                    d.cur = if d.handle == Handle::Rotate {
                        let c = d.orig.center();
                        BoxGeo { rotation: shapes::rotation_to(c, local, shift), ..d.orig }
                    } else {
                        let min = if d.shape.shape.kind.is_line() { 0.0 } else { 2.0 };
                        shapes::dragged(d.orig, d.handle, local - d.start, shift, min)
                    };
                    ui.ctx().request_repaint();
                }
                return true;
            }
            if let Some(d) = self.shapes.drag.take() {
                if d.moved {
                    self.finish_shape_drag(d);
                } else if resp.double_clicked() {
                    match d.shape.shape.picture.as_ref().and_then(|p| p.object.clone()) {
                        Some(o) => self.open_object(o),
                        None => self.edit_shape_text(None),
                    }
                }
            }
            return true;
        }
        let secondary = resp.secondary_clicked();
        if !(pressed || secondary) || !resp.hovered() {
            return false;
        }
        if let Some(b) = self.selected_shape().filter(|_| pressed) {
            if let Some(h) = shapes::handle_at(&b.shape.shape, b.rect, local) {
                self.start_shape_drag(&b, h, local);
                return true;
            }
        }
        let Some(b) = self.shape_hit(local) else { return false };
        resp.request_focus();
        let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        if pressed && (ctrl || shift) && b.shape.wrap.floats() {
            self.toggle_picked(b.at);
            return true;
        }
        // Picked shapes stay picked, to drag them together.
        if !self.is_picked(b.at) {
            self.select_shape(b.at);
        }
        if pressed {
            self.start_shape_drag(&b, Handle::Move, local);
        }
        true
    }

    fn start_shape_drag(&mut self, b: &Boxed, handle: Handle, start: Pos2) {
        let orig = BoxGeo::of(&b.shape.shape, b.rect.min.x, b.rect.min.y, b.rect.width(), b.rect.height());
        let with = if handle == Handle::Move && b.shape.wrap.floats() { self.moving_with(b.at) } else { Vec::new() };
        self.shapes.drag = Some(ShapeDrag { at: b.at, handle, start, orig, shape: b.shape.clone(), cur: orig, moved: false, with });
    }

    /// Offsets of a floating shape with its top-left corner at canvas
    /// point `m`, anchored to paragraph `para`.
    pub(super) fn float_offsets(&self, para: usize, s: &DocShape, m: Pos2) -> (f32, f32) {
        let l = &self.layout;
        let Some(seg) = l.segs_of(para).first() else { return (s.x, s.y) };
        let (x0, _, y0) = frame(&l.geo_of(seg.page), self.page_min(seg.page), seg, s);
        let pt = PT * self.zoom;
        ((m.x - x0) / pt, (m.y - y0) / pt)
    }

    fn finish_shape_drag(&mut self, d: ShapeDrag) {
        if !d.with.is_empty() {
            let rect = Rect::from_min_size(pos2(d.orig.x, d.orig.y), vec2(d.orig.w, d.orig.h));
            let moved = pos2(d.cur.x, d.cur.y) - pos2(d.orig.x, d.orig.y);
            self.drag_along(Boxed { at: d.at, rect, shape: d.shape }, d.with, moved);
            return;
        }
        let pt = PT * self.zoom;
        let mut s = (*d.shape).clone();
        d.cur.apply(&mut s.shape);
        let resized = d.handle != Handle::Move && d.handle != Handle::Rotate;
        if resized {
            s.width = d.cur.w / pt;
            s.height = d.cur.h / pt;
        }
        if s.wrap.floats() {
            let m = pos2(d.cur.x, d.cur.y);
            // A moved shape anchors to the paragraph it was dropped on.
            let para = if d.handle == Handle::Move { self.pos_at(m + vec2(0.0, 1.0)).para } else { d.at.para };
            s.h_align = None;
            (s.x, s.y) = self.float_offsets(para, &s, m);
            s.sanitize();
            let at = self.doc.move_shape(d.at, para, s);
            self.select_shape(at);
            return;
        }
        if d.handle == Handle::Move {
            let delta = pos2(d.cur.x, d.cur.y) - pos2(d.orig.x, d.orig.y);
            if s.wrap == ShapeWrap::Inline {
                // Moves the shape to where it was dropped.
                let to = self.pos_at(pos2(d.cur.x, d.cur.y + d.cur.h / 2.0));
                let sel = Selection { anchor: d.at, cursor: Pos::new(d.at.para, d.at.offset + OBJECT.len_utf8()) };
                if to >= sel.start() && to <= sel.end() {
                    return;
                }
                self.doc.delete(sel);
                let to = if to.para == d.at.para && to.offset > d.at.offset { Pos::new(to.para, to.offset - OBJECT.len_utf8()) } else { to };
                let sel = self.doc.insert_shape(Selection::caret(self.doc.clamp(to)), s);
                self.set_sel(sel);
                return;
            }
            s.h_align = None;
            s.x = (s.x + delta.x / pt).max(0.0);
        }
        s.sanitize();
        self.doc.set_shape(d.at, s);
        self.select_shape(d.at);
    }

    /// Inserts the shape drawn from `a` to `b` (canvas), floating in front
    /// of the text and anchored to the paragraph under it.
    fn insert_drawn(&mut self, t: Template, a: Pos2, b: Pos2, shift: bool) {
        let pt = PT * self.zoom;
        let bx = if (b - a).length() < 4.0 {
            let (w, h) = shapes::default_size(t.kind);
            shapes::drawn(t.kind, a, a + vec2(w, h) * pt, false)
        } else {
            shapes::drawn(t.kind, a, b, shift)
        };
        let para = self.pos_at(pos2(bx.x, bx.y + 1.0)).para;
        let mut s = DocShape::new(t.shape(), bx.w / pt, bx.h / pt);
        bx.apply(&mut s.shape);
        (s.x, s.y) = self.float_offsets(para, &s, pos2(bx.x, bx.y));
        s.z = self.doc.shapes().iter().map(|(_, x)| x.z + 1).max().unwrap_or(0);
        s.sanitize();
        let sel = self.doc.insert_shape(Selection::caret(Pos::new(para, 0)), s);
        self.set_sel(sel);
        self.scroll_to_caret = false;
        if t.kind == ShapeKind::TextBox {
            self.edit_shape_text(None);
        }
    }

    // --- keyboard ----------------------------------------------------------------------

    /// Keys for the selected shape; true when the text must not see them.
    pub(super) fn shape_keys(&mut self, ui: &egui::Ui) -> bool {
        let events = ui.input(|i| i.events.clone());
        if self.shapes.draw.is_some() && events.iter().any(|e| matches!(e, Event::Key { key: Key::Escape, pressed: true, .. })) {
            self.shapes.draw = None;
            self.shapes.drawing = None;
            return true;
        }
        let Some(b) = self.selected_shape() else { return false };
        let text = b.shape.shape.can_have_text();
        for ev in &events {
            match ev {
                Event::Text(t) if text && !t.chars().all(char::is_control) => {
                    self.edit_shape_text(Some(t));
                    return true;
                }
                // Typing replaces nothing: a shape without text ignores it.
                Event::Text(_) => return true,
                Event::Key { key, pressed: true, modifiers: m, .. } => {
                    let step = if m.shift { 10.0 } else { 1.0 };
                    let nudge = match key {
                        Key::ArrowLeft => Some(vec2(-step, 0.0)),
                        Key::ArrowRight => Some(vec2(step, 0.0)),
                        Key::ArrowUp => Some(vec2(0.0, -step)),
                        Key::ArrowDown => Some(vec2(0.0, step)),
                        _ => None,
                    };
                    match key {
                        _ if nudge.is_some() && b.shape.wrap.floats() && !m.command => {
                            let d = nudge.unwrap_or_default();
                            let mut s = (*b.shape).clone();
                            if s.h_align.is_some() {
                                s.x = self.float_offsets(b.at.para, &s, b.rect.min).0;
                                s.h_align = None;
                            }
                            s.x += d.x;
                            s.y += d.y;
                            s.sanitize();
                            self.doc.set_shape(b.at, s);
                            return true;
                        }
                        Key::Escape => {
                            self.set_sel(Selection::caret(Pos::new(b.at.para, b.at.offset + OBJECT.len_utf8())));
                            return true;
                        }
                        Key::Enter | Key::F2 if !m.command && !m.shift => {
                            self.edit_shape_text(None);
                            return true;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        false
    }

    // --- text ----------------------------------------------------------------------------

    /// Starts typing into the selected shape; `typed`: what was typed.
    fn edit_shape_text(&mut self, typed: Option<&str>) {
        let Some(b) = self.selected_shape() else { return };
        // A linked box's text is typed into the first box of its chain.
        let head = self.doc.text_chain_head(b.at);
        if head != b.at {
            self.select_shape(head);
            self.scroll_to_caret = true;
            return self.edit_shape_text(typed);
        }
        if !b.shape.shape.can_have_text() {
            self.open_shape_dialog(0);
            return;
        }
        let e = TextEdit::new(&b.shape.shape, typed);
        self.shapes.edit = Some((b.at, e, (*b.shape).clone()));
    }

    /// The editor over the shape being typed into; `origin`: the canvas
    /// on screen.
    pub(super) fn shape_editor(&mut self, ui: &mut egui::Ui, origin: Pos2) {
        let Some((at, mut e, mut s)) = self.shapes.edit.take() else { return };
        let Some(b) = self.boxes_of(at.para).into_iter().find(|b| b.at == at) else { return };
        let id = egui::Id::new(("doc-shape-text", self.uid));
        match shapes::text_editor(ui, id, b.rect.translate(origin.to_vec2()), &mut s.shape, &mut e, PT * self.zoom) {
            EditResult::Editing => self.shapes.edit = Some((at, e, s)),
            EditResult::Done => {
                self.doc.set_shape(at, s);
                self.select_shape(at);
                self.focus = true;
            }
        }
    }

    // --- menus ---------------------------------------------------------------------------

    /// The next drag on the page draws `t`.
    pub(super) fn start_drawing(&mut self, t: Template) {
        self.shapes.draw = Some(t);
        self.shapes.drawing = None;
        self.focus = true;
    }

    /// The context menu of the selected shape; false when none is.
    pub(super) fn shape_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(b) = self.selected_shape() else { return false };
        if let Some(o) = b.shape.shape.picture.as_ref().and_then(|p| p.object.clone()) {
            self.object_menu(ui, &o);
        }
        if ui.add(egui::Button::new(tr("edit.cut")).shortcut_text("Ctrl+X")).clicked() {
            self.copy(ui.ctx(), true);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("edit.copy")).shortcut_text("Ctrl+C")).clicked() {
            self.copy(ui.ctx(), false);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("shape.delete")).shortcut_text("Del")).clicked() {
            self.doc.delete(Selection { anchor: b.at, cursor: Pos::new(b.at.para, b.at.offset + OBJECT.len_utf8()) });
            self.set_sel(Selection::caret(b.at));
            ui.close();
        }
        if b.shape.shape.can_have_text() {
            ui.separator();
            if ui.button(tr("shape.edit_text")).clicked() {
                self.edit_shape_text(None);
                ui.close();
            }
            self.text_link_items(ui, b.at);
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("pic.wrap"), |ui| {
            for w in ShapeWrap::ALL {
                if ui.radio(b.shape.wrap == w, tr(w.key())).clicked() {
                    if b.shape.shape.picture.is_some() && !w.floats() {
                        // A floating picture goes back to being a picture.
                        let wrap = match w {
                            ShapeWrap::Square => apex_doc::Wrap::Square,
                            ShapeWrap::TopBottom => apex_doc::Wrap::TopBottom,
                            _ => apex_doc::Wrap::Inline,
                        };
                        self.doc.unfloat_image(b.at, wrap);
                        ui.close();
                        continue;
                    }
                    let mut s = (*b.shape).clone();
                    if w.floats() && !s.wrap.floats() {
                        // Stays where it is on the page.
                        (s.x, s.y) = self.float_offsets(b.at.para, &DocShape { v_rel: VRel::Paragraph, h_rel: HRel::Margin, ..s.clone() }, b.rect.min);
                        (s.h_rel, s.v_rel, s.h_align) = (HRel::Margin, VRel::Paragraph, None);
                    }
                    s.wrap = w;
                    self.doc.set_shape(b.at, s);
                    ui.close();
                }
            }
        });
        if b.shape.wrap.floats() {
            crate::widgets::menu_button(ui, tr("shape.order"), |ui| {
                shapes::order_menu(ui, |to| self.reorder_shape(b.at, to));
            });
            self.arrange_items(ui);
        }
        ui.separator();
        if ui.button(format!("{}…", tr("shape.format"))).clicked() {
            self.open_shape_dialog(0);
            ui.close();
        }
        true
    }

    /// Link to the next text box, and break the link.
    pub(super) fn text_link_items(&mut self, ui: &mut egui::Ui, at: Pos) {
        let targets: Vec<(Pos, String)> = self
            .doc
            .shapes()
            .into_iter()
            .enumerate()
            .filter(|(_, (p, _))| self.doc.can_link_text(at, *p))
            .map(|(i, (p, s))| (p, if s.shape.name.is_empty() { crate::i18n::trf("shape.text_box_n", &[("n", &(i + 1).to_string())]) } else { s.shape.name.clone() }))
            .collect();
        ui.add_enabled_ui(!targets.is_empty(), |ui| {
            crate::widgets::menu_button(ui, tr("shape.link_text"), |ui| {
                for (p, name) in targets {
                    if ui.button(name).clicked() {
                        self.doc.link_text_boxes(at, p);
                        ui.close();
                    }
                }
            });
        });
        let linked = self.doc.shape_at(at).and_then(|(_, s)| s.shape.link).is_some_and(|l| self.doc.text_chain(l.chain).last().is_some_and(|(p, _)| *p != at));
        if ui.add_enabled(linked, egui::Button::new(tr("shape.break_link"))).clicked() {
            self.doc.break_text_link(at);
            ui.close();
        }
    }

    fn reorder_shape(&mut self, at: Pos, to: apex_sheet::shape::Order) {
        use apex_sheet::shape::Order;
        let Some((_, cur)) = self.doc.shape_at(at) else { return };
        let others: Vec<i32> = self.doc.shapes().into_iter().filter(|(p, s)| *p != at && s.wrap.floats()).map(|(_, s)| s.z).collect();
        let z = cur.z;
        let nz = match to {
            Order::Front => others.iter().max().map_or(z, |m| (m + 1).max(z)),
            Order::Back => others.iter().min().map_or(z, |m| (m - 1).min(z)),
            Order::Forward => others.iter().filter(|&&v| v >= z).min().map_or(z, |v| v + 1),
            Order::Backward => others.iter().filter(|&&v| v <= z).max().map_or(z, |v| v - 1),
        };
        if nz != z {
            self.doc.set_shape(at, DocShape { z: nz, ..(*cur).clone() });
        }
    }

    /// Changes the look of the selected shape (undoable).
    pub(super) fn change_shape_look(&mut self, f: impl Fn(&mut apex_doc::Shape)) {
        let Some(b) = self.selected_shape() else { return };
        let mut s = (*b.shape).clone();
        f(&mut s.shape);
        s.sanitize();
        if s != *b.shape {
            self.doc.set_shape(b.at, s);
        }
    }

    // --- dialog --------------------------------------------------------------------------

    pub(super) fn open_shape_dialog(&mut self, tab: usize) {
        let Some(b) = self.selected_shape() else { return };
        let s = &b.shape;
        let d = FormatDlg::new(s.shape.clone(), s.width, s.height, tab);
        self.shapes.dialog = Some(Dlg { at: b.at, orig: s.clone(), d, place: (**s).clone() });
    }

    pub(super) fn shape_dialog(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.shapes.dialog else { return };
        let Dlg { d, place, .. } = dlg;
        let mut res = shapes::format_dialog(ctx, "doc-shape-format", d, CM, (crate::icons::Icon::Wrap, "shape.tab_layout"), |ui| layout_tab(ui, place));
        if std::mem::take(&mut dlg.d.fresh) && res == DialogResult::Cancel && !ctx.input(|i| i.key_pressed(Key::Escape)) {
            res = DialogResult::None;
        }
        match res {
            DialogResult::None => {}
            DialogResult::Ok => {
                if let Some(dlg) = self.shapes.dialog.take() {
                    let mut s = dlg.place;
                    s.shape = dlg.d.shape;
                    s.width = dlg.d.w;
                    s.height = dlg.d.h;
                    s.sanitize();
                    if self.doc.shape_at(dlg.at).is_some_and(|(_, cur)| cur == dlg.orig) {
                        self.doc.set_shape(dlg.at, s);
                    }
                }
                self.refocus = true;
            }
            DialogResult::Cancel => {
                self.shapes.dialog = None;
                self.refocus = true;
            }
        }
    }
}

/// Stacking layer: behind the text, in it, in front of it.
pub(super) fn layer(s: &DocShape) -> (i32, i32) {
    match s.wrap {
        ShapeWrap::Behind => (0, s.z),
        ShapeWrap::InFront => (2, s.z),
        _ => (1, 0),
    }
}

/// The Layout tab: wrapping and position.
fn layout_tab(ui: &mut egui::Ui, s: &mut DocShape) {
    ui.label(egui::RichText::new(tr("pic.wrap")).strong());
    ui.horizontal_wrapped(|ui| {
        for w in ShapeWrap::ALL {
            ui.radio_value(&mut s.wrap, w, tr(w.key()));
        }
    });
    if s.wrap == ShapeWrap::Inline {
        return;
    }
    ui.add_space(6.0);
    let cm_value = |ui: &mut egui::Ui, v: &mut f32| {
        let mut c = *v * CM;
        if ui.add(egui::DragValue::new(&mut c).range(-100.0..=100.0).speed(0.05).max_decimals(2).suffix(" cm")).changed() {
            *v = c / CM;
        }
    };
    egui::Grid::new("shape-place").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(tr("shape.horizontal"));
        ui.horizontal(|ui| {
            for (a, key) in [(Some(Align::Left), "pic.left"), (Some(Align::Center), "pic.center"), (Some(Align::Right), "pic.right"), (None, "pic.offset")] {
                ui.radio_value(&mut s.h_align, a, tr(key));
            }
        });
        ui.end_row();
        if s.h_align.is_none() {
            ui.label("");
            cm_value(ui, &mut s.x);
            ui.end_row();
        }
        if s.wrap.floats() {
            ui.label(tr("shape.rel_to"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.h_rel, HRel::Margin, tr("shape.rel_margin"));
                ui.radio_value(&mut s.h_rel, HRel::Page, tr("shape.rel_page"));
            });
            ui.end_row();
            ui.label(tr("shape.vertical"));
            cm_value(ui, &mut s.y);
            ui.end_row();
            ui.label(tr("shape.rel_to"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.v_rel, VRel::Paragraph, tr("shape.rel_para"));
                ui.radio_value(&mut s.v_rel, VRel::Margin, tr("shape.rel_margin"));
                ui.radio_value(&mut s.v_rel, VRel::Page, tr("shape.rel_page"));
            });
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::{Document, Paragraph};

    /// Frames of `v` in a 1000×800 window, the first with `events`.
    fn frames(v: &mut DocView, ctx: &egui::Context, events: Vec<Event>) {
        let mut shell = crate::common::Shell { cmd: None, show_formula_bar: false, file_name: "n".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None };
        let mut events = Some(events);
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), events: events.take().unwrap_or_default(), ..Default::default() };
            ctx.run_ui(input, |ui| v.ui(ui, &mut shell)).textures_delta.clear();
        }
    }

    /// A shape drawn on a note (Draft, embedded) below its text is where
    /// it was drawn, and moves when dragged: the note took the pointer only
    /// as far down as its text.
    #[test]
    fn a_shape_drawn_on_a_note_lands_where_drawn_and_moves() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut v = DocView::embedded(Document::from_paragraphs(vec![Paragraph::new("Ana llega al puerto de noche.")]));
        frames(&mut v, &ctx, Vec::new());
        v.start_drawing(crate::shapes::Template { kind: ShapeKind::Ellipse, arrow: false, preset: None });
        let m = egui::Modifiers::NONE;
        let drag = |v: &mut DocView, a: Pos2, b: Pos2| {
            frames(v, &ctx, vec![Event::PointerMoved(a), Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: m }]);
            frames(v, &ctx, vec![Event::PointerMoved(b)]);
            frames(v, &ctx, vec![Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: m }]);
        };
        drag(&mut v, pos2(300.0, 300.0), pos2(500.0, 450.0));
        assert_eq!(v.doc.shapes().len(), 1);
        let r = v.selected_shape().expect("the new shape selected").rect;
        assert!((r.min - pos2(300.0, 300.0)).length() < 2.0 && (r.max - pos2(500.0, 450.0)).length() < 2.0, "{r:?}");
        drag(&mut v, pos2(400.0, 375.0), pos2(450.0, 415.0));
        let moved = v.selected_shape().expect("still selected").rect;
        assert!((moved.min - (r.min + vec2(50.0, 40.0))).length() < 2.0, "{moved:?}");
    }
}
