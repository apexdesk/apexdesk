//! Paints a slide at any size: its background, the master's and the
//! layout's decoration, then its own objects (shapes with their text,
//! pictures, groups, tables, charts, diagrams, equations and the objects
//! only shown). The canvas, the thumbnails, the show, printing and PDF
//! all paint through here.

use std::sync::Arc;

use apex_doc::PresetFill;
use apex_slides::fill::{Arrow, Dash, Gradient};
use apex_slides::object::Media;
use apex_slides::{Chain, Deck, Fill, Frame, Geometry, ObjKind, Object, Slide};
use eframe::egui::{self, Color32, Mesh, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use super::text::{self, Over, c32};
use crate::i18n::tr;

/// Maps slide points to the screen: `x' = ox + x * sx`.
#[derive(Clone, Copy, Debug)]
pub struct Xf {
    pub ox: f32,
    pub oy: f32,
    pub sx: f32,
    pub sy: f32,
}

impl Xf {
    /// The whole slide in `rect`.
    pub fn for_slide(deck: &Deck, rect: Rect) -> Xf {
        Xf { ox: rect.min.x, oy: rect.min.y, sx: rect.width() / deck.width.max(1.0), sy: rect.height() / deck.height.max(1.0) }
    }

    pub fn pos(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.ox + x * self.sx, self.oy + y * self.sy)
    }

    /// Slide point of screen point `q`.
    pub fn back(&self, q: Pos2) -> (f32, f32) {
        ((q.x - self.ox) / self.sx, (q.y - self.oy) / self.sy)
    }

    pub fn rect(&self, f: &Frame) -> Rect {
        Rect::from_min_max(self.pos(f.x, f.y), self.pos(f.x + f.w, f.y + f.h))
    }

    /// Inside a group with frame `f` whose children are placed in `child`.
    pub fn group(&self, f: &Frame, child: &Frame) -> Xf {
        let kx = if child.w > 0.0 { f.w / child.w } else { 1.0 };
        let ky = if child.h > 0.0 { f.h / child.h } else { 1.0 };
        Xf { ox: self.ox + (f.x - child.x * kx) * self.sx, oy: self.oy + (f.y - child.y * ky) * self.sy, sx: self.sx * kx, sy: self.sy * ky }
    }
}

/// How a slide is painted.
#[derive(Clone, Copy, Default)]
pub struct Opts {
    /// Pictures are decoded before painting (printing, PDF); else they
    /// show once decoded.
    pub wait: bool,
    /// Editing: empty placeholders show their prompt and a dotted outline.
    pub edit: bool,
    /// The object whose text is being typed: no prompt on it.
    pub typing: Option<u32>,
}

/// Paints `slide` of `deck` filling `rect`.
pub fn paint_slide(p: &Painter, deck: &Deck, slide: &Slide, rect: Rect, o: Opts) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let ch = Chain::for_slide(deck, slide);
    let xf = Xf::for_slide(deck, rect);
    let scale = xf.sx;
    paint_fill_rect(&p, &ch, &ch.background(), rect, xf.sx, o.wait);
    apex_slides::resolve::memo(|| {
        for (obj, c) in ch.decoration() {
            paint_object(&p, &c, obj, xf, scale, o);
        }
        for obj in &slide.objects {
            paint_object(&p, &ch, obj, xf, scale, o);
        }
    });
}

/// Paints the master `m` (or its layout `l`) as in the slide master view.
pub fn paint_layout(p: &Painter, deck: &Deck, m: usize, l: Option<usize>, rect: Rect, o: Opts) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let ch = match l {
        Some(l) => Chain::for_layout(deck, m, l),
        None => Chain::for_master(deck, m),
    };
    let xf = Xf::for_slide(deck, rect);
    paint_fill_rect(&p, &ch, &ch.background(), rect, xf.sx, o.wait);
    let master = &deck.masters[m];
    if let Some(l) = l.and_then(|l| master.layouts.get(l)) {
        if l.show_master {
            let mc = Chain::for_master(deck, m);
            for obj in master.objects.iter().filter(|x| x.ph.is_none()) {
                paint_object(&p, &mc, obj, xf, xf.sx, o);
            }
        }
        for obj in &l.objects {
            paint_object(&p, &ch, obj, xf, xf.sx, o);
        }
    } else {
        for obj in &master.objects {
            paint_object(&p, &ch, obj, xf, xf.sx, o);
        }
    }
}

// --- colors and fills -------------------------------------------------------------------

/// A plain color for a fill (patterns are their two colors mixed).
fn flat(ch: &Chain, f: &Fill) -> Option<Color32> {
    f.main_color(&ch.cx()).map(c32)
}

/// Screen pixels of one tile pixel: pictures tile at 96 dpi, and a slide
/// point is `k` screen pixels.
fn tile_px(k: f32) -> f32 {
    k * 72.0 / 96.0
}

/// Fills `rect` (not turned) with `f`; `k` is screen pixels per point.
pub fn paint_fill_rect(p: &Painter, ch: &Chain, f: &Fill, rect: Rect, k: f32, wait: bool) {
    match f {
        Fill::Gradient(g) => {
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
            paint_grad_quad(p, ch, g, corners);
        }
        Fill::Picture(pf) => {
            if pf.tile {
                if let Some(t) = crate::images::tiled_texture(p.ctx(), crate::images::key(&pf.data), &pf.data) {
                    // PDF output reads the texture coordinates as tiles.
                    let [w, h] = t.size().map(|v| v as f32);
                    let s = tile_px(k);
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(rect.width() / (w * s).max(1e-3), rect.height() / (h * s).max(1e-3)));
                    p.image(t.id(), rect, uv, Color32::WHITE);
                }
            } else {
                crate::images::paint_picture(p, &pf.data, rect, 0.0, pf.crop, None, rect.width(), wait);
            }
        }
        _ => {
            if let Some(c) = flat(ch, f) {
                p.rect_filled(rect, 0.0, c);
            }
        }
    }
}

/// A gradient over a quad (its corners in order: top left, top right,
/// bottom right, bottom left of the unturned box).
fn paint_grad_quad(p: &Painter, ch: &Chain, g: &Gradient, q: [Pos2; 4]) {
    let g = g.resolve(&ch.cx());
    let place = |r: Pos2| {
        let top = q[0] + (q[1] - q[0]) * r.x;
        let bottom = q[3] + (q[2] - q[3]) * r.x;
        top + (bottom - top) * r.y
    };
    let unit = vec![pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)];
    p.add(egui::Shape::mesh(crate::gradient::mesh(&g, &[unit], 1.0, 1.0, &place, &|c| c)));
}

// --- objects ----------------------------------------------------------------------------

/// Maps a point of a shape's box (pixels from its unturned top left) to
/// the screen, with its flips and rotation.
fn placer(rect: Rect, f: &Frame) -> impl Fn((f32, f32)) -> Pos2 + '_ {
    let (w, h) = (rect.width(), rect.height());
    let c = rect.center();
    let rot = egui::emath::Rot2::from_angle(f.rot.to_radians());
    move |(x, y)| {
        let x = if f.flip_h { w - x } else { x };
        let y = if f.flip_v { h - y } else { y };
        let q = rect.min + vec2(x, y);
        if f.rot == 0.0 { q } else { c + rot * (q - c) }
    }
}

/// The paths of a shape in a `w` by `h` box.
fn paths_of(g: &Geometry, w: f32, h: f32) -> Vec<apex_doc::PresetPath> {
    let rect = || vec![apex_doc::PresetPath { subpaths: vec![(vec![(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)], true)], fill: PresetFill::Norm, stroke: true }];
    match g {
        // Straight lines aren't in the preset table: corner to corner.
        Geometry::Preset(name, _) if matches!(&**name, "line" | "straightConnector1") => {
            vec![apex_doc::PresetPath { subpaths: vec![(vec![(0.0, 0.0), (w, h)], false)], fill: PresetFill::None, stroke: true }]
        }
        Geometry::Preset(name, adj) => apex_doc::preset_paths(name, w, h, adj).unwrap_or_else(rect),
        Geometry::Custom(xml) => {
            thread_local! {
                /// Paths of custom geometries lately painted, by source and
                /// size (reading them again each frame is slow for big ones).
                static SEEN: std::cell::RefCell<Vec<(std::sync::Weak<str>, u32, u32, Vec<apex_doc::PresetPath>)>> = const { std::cell::RefCell::new(Vec::new()) };
            }
            let hit = SEEN.with(|s| s.borrow().iter().find(|(x, sw, sh, _)| *sw == w.to_bits() && *sh == h.to_bits() && x.upgrade().is_some_and(|x| std::sync::Arc::ptr_eq(&x, xml))).map(|e| e.3.clone()));
            let v = hit.unwrap_or_else(|| {
                let v = apex_slides::geom::custom_paths(xml, w, h);
                SEEN.with(|s| {
                    let mut s = s.borrow_mut();
                    s.retain(|e| e.0.strong_count() > 0);
                    if s.len() >= 256 {
                        s.remove(0);
                    }
                    s.push((std::sync::Arc::downgrade(xml), w.to_bits(), h.to_bits(), v.clone()));
                });
                v
            });
            if v.is_empty() { rect() } else { v }
        }
    }
}

/// Paints object `o` (and a group's children).
pub fn paint_object(p: &Painter, ch: &Chain, o: &Object, xf: Xf, scale: f32, opts: Opts) {
    if o.hidden {
        return;
    }
    let f = ch.frame(o);
    let rect = xf.rect(&f);
    if !rect.is_finite() {
        return;
    }
    match &o.kind {
        ObjKind::Group(g) => {
            let inner = xf.group(&f, &g.child);
            for c in &g.children {
                paint_object(p, ch, c, inner, scale, opts);
            }
        }
        ObjKind::Shape { text, .. } => paint_shape(p, ch, o, &f, rect, text.as_ref(), scale, opts),
        ObjKind::Picture(pic) => {
            let img = &pic.image;
            crate::images::paint_picture(p, &img.data, rect, f.rot, img.crop, img.fx.as_deref(), rect.width(), opts.wait);
            if let Some(m) = &pic.media {
                media_badge(p, rect, m);
            }
            if let Some(line) = ch.line(o) {
                stroke_paths(p, ch, &line, &paths_of(&Geometry::default(), rect.width(), rect.height()), placer(rect, &f), scale);
            }
        }
        ObjKind::Table(t) => paint_table(p, ch, o, t, rect, scale),
        ObjKind::Chart(c) => {
            let obj = apex_doc::Embedded { id: 0, kind: apex_doc::EmbedKind::Chart(c.clone()) };
            crate::objects::paint(p, rect, &Arc::from(Vec::new()), &obj, f.rot, opts.wait);
        }
        ObjKind::Diagram(d) => {
            let (sx, sy) = (rect.width() / d.size.0.max(1.0), rect.height() / d.size.1.max(1.0));
            if d.shapes.is_empty() {
                placeholder_box(p, rect, &d.texts.join(" · "));
            }
            for sh in &d.shapes {
                let r = Rect::from_min_size(rect.min + vec2(sh.x * sx, sh.y * sy), vec2(sh.w * sx, sh.h * sy));
                crate::shapes::paint(p, r, &sh.shape, sx, true);
            }
        }
        ObjKind::Math(eq) => paint_math(p, eq, rect, scale),
        ObjKind::Other { preview, what, .. } => match preview {
            Some(b) => crate::images::paint_picture(p, b, rect, f.rot, [0.0; 4], None, rect.width(), opts.wait),
            None => placeholder_box(p, rect, what),
        },
    }
}

/// A play mark on a video or sound.
fn media_badge(p: &Painter, rect: Rect, m: &Media) {
    let s = (rect.width().min(rect.height()) * 0.25).clamp(10.0, 48.0);
    let c = if m.video { rect.center() } else { rect.right_bottom() - vec2(s * 0.7, s * 0.7) };
    p.circle_filled(c, s * 0.5, Color32::from_black_alpha(140));
    let t = s * 0.22;
    p.add(egui::Shape::convex_polygon(vec![c + vec2(-t * 0.7, -t), c + vec2(t, 0.0), c + vec2(-t * 0.7, t)], Color32::WHITE, Stroke::NONE));
}

/// A framed box with a label, for what isn't drawn.
fn placeholder_box(p: &Painter, rect: Rect, label: &str) {
    p.rect_filled(rect, 2.0, Color32::from_gray(238));
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_gray(170)), egui::StrokeKind::Inside);
    if !label.is_empty() {
        let size = (rect.height() * 0.12).clamp(6.0, 16.0);
        p.with_clip_rect(rect.intersect(p.clip_rect())).text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(size), Color32::from_gray(90));
    }
}

pub(super) fn paint_math(p: &Painter, eq: &apex_doc::math::Equation, rect: Rect, scale: f32) {
    let size = 24.0 * scale;
    let color = Color32::BLACK;
    let (w, a, d, _) = crate::doc_view::render_math(p.ctx(), eq, size, true, color);
    let k = (rect.width() / w.max(1.0)).min(rect.height() / (a + d).max(1.0)).min(1.0);
    let size = size * k;
    let (w, a, d, shapes) = crate::doc_view::render_math(p.ctx(), eq, size, true, color);
    let origin = pos2(rect.center().x - w / 2.0, rect.center().y - (a + d) / 2.0 + a);
    for mut s in shapes {
        s.translate(origin.to_vec2());
        p.add(s);
    }
}

/// The screen stroke of an outline.
fn stroke_of(ch: &Chain, line: &apex_slides::Line, scale: f32) -> Option<(Stroke, Dash)> {
    let c = line.fill.as_ref().and_then(|f| flat(ch, f))?;
    let w = (line.width.unwrap_or(0.75) * scale).max(0.6);
    Some((Stroke::new(w, c), line.dash.unwrap_or(Dash::Solid)))
}

/// An arrowhead of kind `kind` with its tip at `tip`, pointing along `dir`.
fn arrow_end(kind: Arrow, tip: Pos2, dir: Vec2, size: f32, stroke: Stroke, out: &mut Vec<egui::Shape>) {
    let n = vec2(-dir.y, dir.x);
    let base = tip - dir * size;
    let c = stroke.color;
    match kind {
        Arrow::Open => out.push(egui::Shape::line(vec![base + n * size * 0.5, tip, base - n * size * 0.5], stroke)),
        Arrow::Stealth => out.push(egui::Shape::convex_polygon(vec![tip, base + n * size * 0.5, tip - dir * size * 0.6, base - n * size * 0.5], c, Stroke::NONE)),
        Arrow::Diamond => {
            let mid = tip - dir * size * 0.5;
            out.push(egui::Shape::convex_polygon(vec![tip + dir * size * 0.5, mid + n * size * 0.5, mid - dir * size * 0.5, mid - n * size * 0.5], c, Stroke::NONE));
        }
        Arrow::Oval => out.push(egui::Shape::circle_filled(tip, size * 0.5, c)),
        Arrow::Triangle | Arrow::None => crate::shapes::arrowhead(tip, dir, size, c, out),
    }
}

/// Strokes the stroked paths, with arrowheads at the ends of open ones.
fn stroke_paths(p: &Painter, ch: &Chain, line: &apex_slides::Line, paths: &[apex_doc::PresetPath], place: impl Fn((f32, f32)) -> Pos2, scale: f32) {
    let Some((stroke, dash)) = stroke_of(ch, line, scale) else { return };
    let mut open: Vec<Vec<Pos2>> = Vec::new();
    let mut closed: Vec<Vec<Pos2>> = Vec::new();
    for path in paths.iter().filter(|x| x.stroke) {
        for (pts, c) in &path.subpaths {
            let v: Vec<Pos2> = pts.iter().map(|&q| place(q)).collect();
            if *c { closed.push(v) } else { open.push(v) }
        }
    }
    let mut out = Vec::new();
    let head = (stroke.width * 3.0).max(5.0 * scale);
    let n = open.len();
    for (i, arrow, at_end) in [(0, line.head, false), (n.wrapping_sub(1), line.tail, true)] {
        let want = arrow.is_some_and(|a| a != Arrow::None);
        let Some(pts) = open.get_mut(i).filter(|_| want) else { continue };
        if pts.len() < 2 {
            continue;
        }
        let k = if at_end { pts.len() - 1 } else { 0 };
        let tip = pts[k];
        let back = if at_end { pts.iter().rev().find(|q| q.distance(tip) > head * 0.5) } else { pts.iter().find(|q| q.distance(tip) > head * 0.5) };
        let Some(&from) = back else { continue };
        let dir = (tip - from).normalized();
        if dir.is_finite() {
            let kind = arrow.unwrap_or(Arrow::Triangle);
            arrow_end(kind, tip, dir, head, stroke, &mut out);
            // The line stops inside a filled head; an open one needs the
            // line up to its tip.
            if kind != Arrow::Open {
                pts[k] = tip - dir * head * if kind == Arrow::Oval { 0.0 } else { 0.8 };
            }
        }
    }
    for (pts, c) in open.into_iter().map(|v| (v, false)).chain(closed.into_iter().map(|v| (v, true))) {
        if dash != Dash::Solid {
            crate::shapes::dashed(&pts, c, stroke, dash.pattern(), &mut out);
        } else if c {
            out.push(egui::Shape::closed_line(pts, stroke));
        } else {
            out.push(egui::Shape::line(pts, stroke));
        }
    }
    p.extend(out);
}

/// Fills the paths of a shape with `fill`.
fn fill_paths(p: &Painter, ch: &Chain, fill: &Fill, paths: &[apex_doc::PresetPath], rect: Rect, f: &Frame, wait: bool, is_rect: bool) {
    let place = placer(rect, f);
    let (w, h) = (rect.width().max(1e-3), rect.height().max(1e-3));
    match fill {
        Fill::Picture(pf) if is_rect && !pf.tile => {
            crate::images::paint_picture(p, &pf.data, rect, f.rot, pf.crop, None, rect.width(), wait);
        }
        Fill::Picture(pf) => {
            // The picture stretched over the box (less its crop) or tiled
            // at its own size, clipped to the shape by its triangles.
            let key = crate::images::key(&pf.data);
            let tex = if pf.tile { crate::images::tiled_texture(p.ctx(), key, &pf.data) } else { crate::images::texture(p.ctx(), key, &pf.data, wait) };
            let Some(tex) = tex else { return };
            let s = tile_px(if f.w > 0.0 { rect.width() / f.w } else { 1.0 });
            let [tw, th] = tex.size().map(|v| v as f32 * s);
            let [cl, ct, cr, cb] = pf.crop;
            let uv = |x: f32, y: f32| {
                if pf.tile {
                    pos2(x / tw.max(1e-3), y / th.max(1e-3))
                } else {
                    pos2(cl + x / w * (1.0 - cl - cr), ct + y / h * (1.0 - ct - cb))
                }
            };
            for path in paths.iter().filter(|x| x.fill != PresetFill::None) {
                let local: Vec<(Vec<Pos2>, bool)> = path.subpaths.iter().map(|(pts, c)| (pts.iter().map(|&(x, y)| pos2(x, y)).collect(), *c)).collect();
                for poly in crate::shapes::merge_holes(&local) {
                    let mut mesh = Mesh::with_texture(tex.id());
                    for q in &poly {
                        mesh.vertices.push(egui::epaint::Vertex { pos: place((q.x, q.y)), uv: uv(q.x, q.y), color: Color32::WHITE.gamma_multiply(pf.alpha.clamp(0.0, 1.0)) });
                    }
                    mesh.indices = crate::shapes::triangulate(&poly);
                    p.add(egui::Shape::mesh(mesh));
                }
            }
        }
        Fill::Gradient(g) => {
            let g = g.resolve(&ch.cx());
            for path in paths.iter().filter(|x| x.fill != PresetFill::None) {
                let local: Vec<(Vec<Pos2>, bool)> = path.subpaths.iter().map(|(pts, c)| (pts.iter().map(|&(x, y)| pos2(x, y)).collect(), *c)).collect();
                let mode = path.fill;
                let polys = crate::shapes::merge_holes(&local);
                p.add(egui::Shape::mesh(crate::gradient::mesh(&g, &polys, w, h, &|q| place((q.x, q.y)), &|c| crate::shapes::shaded(c, mode))));
            }
        }
        _ => {
            let Some(c) = flat(ch, fill) else { return };
            for path in paths.iter().filter(|x| x.fill != PresetFill::None) {
                let subs: Vec<(Vec<Pos2>, bool)> = path.subpaths.iter().map(|(pts, cl)| (pts.iter().map(|&q| place(q)).collect(), *cl)).collect();
                crate::shapes::fill_subpaths(p, &subs, crate::shapes::shaded(c, path.fill));
            }
        }
    }
}

/// The prompt an empty placeholder shows while editing.
fn prompt(o: &Object) -> &'static str {
    use apex_slides::PhType as P;
    match o.ph.map(|p| p.kind) {
        Some(P::Title | P::CtrTitle) => tr("slides.ph.title"),
        Some(P::SubTitle) => tr("slides.ph.subtitle"),
        Some(P::Pic) => tr("slides.ph.picture"),
        Some(P::Table) => tr("slides.ph.table"),
        Some(P::Chart) => tr("slides.ph.chart"),
        _ => tr("slides.ph.text"),
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_shape(p: &Painter, ch: &Chain, o: &Object, f: &Frame, rect: Rect, t: Option<&apex_slides::TextBody>, scale: f32, opts: Opts) {
    let geom = ch.geometry(o);
    let (w, h) = (rect.width(), rect.height());
    let paths = paths_of(&geom, w, h);
    let is_rect = matches!(geom.preset_name(), Some("rect"));
    if !geom.is_line() {
        if let Some(fill) = ch.fill(o) {
            fill_paths(p, ch, &fill, &paths, rect, f, opts.wait, is_rect);
        }
    }
    if let Some(line) = ch.line(o) {
        stroke_paths(p, ch, &line, &paths, placer(rect, f), scale);
    }
    let empty = t.is_none_or(|t| t.is_empty());
    // Footers, dates and numbers of the layout show only when the slide has them.
    if empty && o.ph.is_some() {
        if opts.edit && opts.typing != Some(o.id) && !o.ph.is_some_and(|x| x.kind.is_hf()) {
            let dotted = Stroke::new(1.0, Color32::from_gray(150));
            let r = rect;
            let pts = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
            let place = placer(rect, f);
            let pts: Vec<Pos2> = pts.into_iter().map(|q| place((q.x - r.min.x, q.y - r.min.y))).collect();
            let mut out = Vec::new();
            crate::shapes::dashed(&pts, true, dotted, &[2.0, 2.0], &mut out);
            p.extend(out);
            // The prompt, in the placeholder's own style but gray.
            let mut body = t.cloned().unwrap_or_default();
            body.set_plain(prompt(o));
            let over = Over { color: Some(apex_slides::Rgba { rgb: 0x808080, alpha: 255 }), bold: false };
            let area = text_area(&geom, rect);
            text::paint_body(p, ch, o, &body, area, f.rot, rect.center(), scale, over);
        }
        return;
    }
    let Some(t) = t else { return };
    let area = text_area(&geom, rect);
    // The box being typed in keeps its field marks, where the caret goes.
    let t = if opts.typing == Some(o.id) { std::borrow::Cow::Borrowed(t) } else { text::with_fields(ch, t) };
    text::paint_body(p, ch, o, &t, area, f.rot, rect.center(), scale, Over::default());
}

/// Where the text `t` of top-level object `o` shows, as it is painted.
pub fn text_placed(p: &Painter, ch: &Chain, o: &Object, t: &apex_slides::TextBody, xf: Xf) -> text::Placed {
    let f = ch.frame(o);
    let rect = xf.rect(&f);
    let area = text_area(&ch.geometry(o), rect);
    text::place_body(p, ch, o, t, area, f.rot, rect.center(), xf.sx, Over::default())
}

/// The text box of a shape's geometry, unturned.
fn text_area(g: &Geometry, rect: Rect) -> Rect {
    let (w, h) = (rect.width(), rect.height());
    if let Geometry::Preset(name, adj) = g
        && let Some([l, t, r, b]) = apex_doc::preset_text_rect(name, w, h, adj)
    {
        return Rect::from_min_max(rect.min + vec2(l, t), rect.min + vec2(r, b));
    }
    rect
}

// --- tables -----------------------------------------------------------------------------

/// Row heights of a table drawn at `scale`: at least their own height, or
/// what their text needs.
pub fn row_heights(p: &Painter, ch: &Chain, o: &Object, t: &apex_slides::Table, scale: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(t.rows.len());
    for (r, row) in t.rows.iter().enumerate() {
        let mut h = row.height * scale;
        for (c, cell) in row.cells.iter().enumerate() {
            if cell.covered() || cell.rspan > 1 {
                continue;
            }
            let w: f32 = t.cols.iter().skip(c).take(cell.span.max(1) as usize).sum::<f32>() * scale;
            let m = margins(cell);
            let look = t.cell_look(r, c);
            let over = Over { color: look.text.as_ref().map(|x| ch.color(x)), bold: look.bold };
            let need = text::height(p, ch, o, &cell.text, (w - (m[0] + m[1]) * scale).max(1.0), scale, over) + (m[2] + m[3]) * scale;
            h = h.max(need);
        }
        out.push(h);
    }
    out
}

fn margins(cell: &apex_slides::table::Cell) -> [f32; 4] {
    let d = [7.2, 7.2, 3.6, 3.6];
    [0, 1, 2, 3].map(|i| cell.margins[i].unwrap_or(d[i]))
}

/// A cell of a table as drawn: row, column, its box, the box of its text
/// (inside the margins) and the look of its text.
pub struct CellBox {
    pub r: usize,
    pub c: usize,
    pub rect: Rect,
    pub area: Rect,
    pub over: Over,
}

/// The cells of table `t` drawn in `rect` (the ones merged into another left out).
pub fn table_cells(p: &Painter, ch: &Chain, o: &Object, t: &apex_slides::Table, rect: Rect, scale: f32) -> Vec<CellBox> {
    let heights = row_heights(p, ch, o, t, scale);
    let xs: Vec<f32> = std::iter::once(0.0).chain(t.cols.iter().scan(0.0, |a, w| {
        *a += w * scale;
        Some(*a)
    })).collect();
    let ys: Vec<f32> = std::iter::once(0.0).chain(heights.iter().scan(0.0, |a, h| {
        *a += h;
        Some(*a)
    })).collect();
    let cell_rect = |r: usize, c: usize, rs: usize, cs: usize| {
        let x1 = xs.get(c + cs).or(xs.last()).copied().unwrap_or(0.0);
        let y1 = ys.get(r + rs).or(ys.last()).copied().unwrap_or(0.0);
        Rect::from_min_max(rect.min + vec2(xs[c.min(xs.len() - 1)], ys[r.min(ys.len() - 1)]), rect.min + vec2(x1, y1))
    };
    let mut out = Vec::new();
    for (r, row) in t.rows.iter().enumerate() {
        for (c, cell) in row.cells.iter().enumerate() {
            if cell.covered() || c >= t.cols.len() {
                continue;
            }
            let cr = cell_rect(r, c, cell.rspan.max(1) as usize, cell.span.max(1) as usize);
            let m = margins(cell);
            let area = Rect::from_min_max(cr.min + vec2(m[0] * scale, m[2] * scale), cr.max - vec2(m[1] * scale, m[3] * scale));
            let look = t.cell_look(r, c);
            let over = Over { color: look.text.as_ref().map(|x| ch.color(x)), bold: look.bold };
            out.push(CellBox { r, c, rect: cr, area, over });
        }
    }
    out
}

/// The text of cell `b` laid out and placed as it is painted.
pub fn cell_placed(p: &Painter, ch: &Chain, o: &Object, cell: &apex_slides::table::Cell, b: &CellBox, scale: f32) -> text::Placed {
    let empty;
    let t = if cell.text.paras.is_empty() {
        empty = apex_slides::text::TextBody::plain("");
        &empty
    } else {
        &cell.text
    };
    let laid = text::layout(p, ch, o, t, b.area.width().max(1.0), true, scale, 1.0, 0.0, b.over);
    let off = match cell.anchor.unwrap_or(apex_doc::VAnchor::Top) {
        apex_doc::VAnchor::Top => 0.0,
        apex_doc::VAnchor::Middle => (b.area.height() - laid.height) / 2.0,
        apex_doc::VAnchor::Bottom => b.area.height() - laid.height,
    };
    text::Placed { laid, at: pos2(b.area.min.x, b.area.min.y + off), angle: 0.0 }
}

/// The cells of table object `o` as drawn by `xf` (none for other objects).
pub fn cells_of(p: &Painter, ch: &Chain, o: &Object, xf: Xf) -> Vec<CellBox> {
    match &o.kind {
        ObjKind::Table(t) => table_cells(p, ch, o, t, xf.rect(&ch.frame(o)), xf.sx),
        _ => Vec::new(),
    }
}

/// The cell of table `o` under screen point `q`.
pub fn cell_at(p: &Painter, ch: &Chain, o: &Object, xf: Xf, q: Pos2) -> Option<(usize, usize)> {
    cells_of(p, ch, o, xf).into_iter().find(|b| b.rect.contains(q)).map(|b| (b.r, b.c))
}

/// Where the text of cell `rc` of table `o` shows, as drawn by `xf`.
pub fn cell_text_placed(p: &Painter, ch: &Chain, o: &Object, xf: Xf, rc: (usize, usize)) -> Option<text::Placed> {
    let ObjKind::Table(t) = &o.kind else { return None };
    let cell = t.rows.get(rc.0)?.cells.get(rc.1)?;
    let b = cells_of(p, ch, o, xf).into_iter().find(|b| (b.r, b.c) == rc)?;
    Some(cell_placed(p, ch, o, cell, &b, xf.sx))
}

fn paint_table(p: &Painter, ch: &Chain, o: &Object, t: &apex_slides::Table, rect: Rect, scale: f32) {
    let mut lines = Vec::new();
    for b in table_cells(p, ch, o, t, rect, scale) {
        let cell = &t.rows[b.r].cells[b.c];
        let cr = b.rect;
        let look = t.cell_look(b.r, b.c);
        let fill = cell.fill.clone().or(look.fill.clone());
        if let Some(fl) = fill.filter(|x| *x != Fill::None) {
            paint_fill_rect(p, ch, &fl, cr, scale, false);
        }
        // Borders: the cell's own, else the style's.
        let sides = [(cr.left_top(), cr.left_bottom()), (cr.right_top(), cr.right_bottom()), (cr.left_top(), cr.right_top()), (cr.left_bottom(), cr.right_bottom())];
        for (k, (a, b)) in sides.into_iter().enumerate() {
            let line = cell.borders[k].clone().or(look.border.clone());
            if let Some((s, _)) = line.as_ref().and_then(|l| stroke_of(ch, l, scale)) {
                lines.push(egui::Shape::line_segment([a, b], s));
            }
        }
        let placed = cell_placed(p, ch, o, cell, &b, scale);
        text::paint(&p.with_clip_rect(cr.intersect(p.clip_rect())), &placed.laid, placed.at, 0.0, cr.center());
    }
    p.extend(lines);
}
