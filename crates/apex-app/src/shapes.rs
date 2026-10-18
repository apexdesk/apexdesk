//! Shapes and text boxes, shared by both programs: their outlines, painting
//! (screen, print and PDF all go through egui shapes), handles and the
//! geometry of dragging them, the Insert > Shapes gallery, the Format Shape
//! dialog and the text editor over a shape.

use std::sync::Arc;

use apex_doc::{Align, Dash, Shape, ShapeKind, ShapeList, ShapePara, ShapeRun, VAnchor};
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, CursorIcon, Mesh, Painter, Pos2, Rect, RichText, Stroke, Vec2, pos2, vec2};

use crate::fonts;
use crate::i18n::tr;
use crate::theme;

/// Screen pixels per point at 100% (both programs use 96 dpi).
pub const PX_PT: f32 = 96.0 / 72.0;
/// Text insets of a shape: left/right and top/bottom, points.
const INSET_X: f32 = 7.2;
const INSET_Y: f32 = 3.6;
/// Handle size, hit distance and the rotation handle's arm, screen points.
/// (Bigger and farther with fingers: crate::touch.)
pub const HANDLE: f32 = crate::touch::size(7.0);
pub const GRAB: f32 = 6.0;
pub const ROT_ARM: f32 = if cfg!(target_os = "android") { 34.0 } else { 22.0 };

// --- geometry ------------------------------------------------------------------------

/// The outline of a shape of kind `kind` in a `w` by `h` box with its
/// top-left corner at the origin (lines: their two ends).
pub fn outline(kind: ShapeKind, w: f32, h: f32) -> Vec<Pos2> {
    apex_doc::shape_outline(kind, w, h).into_iter().map(|(x, y)| pos2(x, y)).collect()
}

/// Whether the outline of `kind` is convex (filled directly; others are
/// cut into triangles).
fn convex(kind: ShapeKind) -> bool {
    !matches!(kind, ShapeKind::RightArrow | ShapeKind::LeftArrow | ShapeKind::UpArrow | ShapeKind::DownArrow | ShapeKind::Star5 | ShapeKind::Callout)
}

/// The area for text in a `w` by `h` box (before the insets).
pub fn text_area(kind: ShapeKind, w: f32, h: f32) -> Rect {
    let ss = w.min(h);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0 * w, y0 * h), pos2(x1 * w, y1 * h));
    match kind {
        ShapeKind::Ellipse => r(0.1464, 0.1464, 0.8536, 0.8536),
        ShapeKind::Triangle => r(0.25, 0.5, 0.75, 1.0),
        ShapeKind::RightTriangle => r(1.0 / 12.0, 7.0 / 12.0, 7.0 / 12.0, 11.0 / 12.0),
        ShapeKind::Diamond => r(0.25, 0.25, 0.75, 0.75),
        ShapeKind::Pentagon => r(0.2, 0.3, 0.8, 0.95),
        ShapeKind::Hexagon => Rect::from_min_max(pos2(ss * 0.25 * 0.5, 0.0), pos2(w - ss * 0.25 * 0.5, h)),
        ShapeKind::RightArrow | ShapeKind::LeftArrow => {
            let head = (ss * 0.5).min(w);
            let (x0, x1) = if kind == ShapeKind::RightArrow { (0.0, w - head / 2.0) } else { (head / 2.0, w) };
            Rect::from_min_max(pos2(x0, h * 0.25), pos2(x1, h * 0.75))
        }
        ShapeKind::DownArrow | ShapeKind::UpArrow => {
            let head = (ss * 0.5).min(h);
            let (y0, y1) = if kind == ShapeKind::DownArrow { (0.0, h - head / 2.0) } else { (head / 2.0, h) };
            Rect::from_min_max(pos2(w * 0.25, y0), pos2(w * 0.75, y1))
        }
        ShapeKind::Star5 => r(0.31, 0.39, 0.69, 0.76),
        _ => Rect::from_min_size(Pos2::ZERO, vec2(w, h)),
    }
}

/// Turns local point `q` of a box at `rect` by `deg` degrees clockwise
/// about the box center.
fn place(rect: Rect, deg: f32, q: Pos2) -> Pos2 {
    let c = rect.center();
    let d = rect.min + q.to_vec2() - c;
    if deg == 0.0 {
        return c + d;
    }
    c + turn(deg, d)
}

pub fn turn(deg: f32, v: Vec2) -> Vec2 {
    let (s, c) = deg.to_radians().sin_cos();
    vec2(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Screen point `p` in a turned box's own axes, relative to its center.
pub fn local(rect: Rect, deg: f32, p: Pos2) -> Vec2 {
    turn(-deg, p - rect.center())
}

/// The outline points of a shape drawn in `rect` (screen), turned and
/// flipped as it says.
pub fn screen_outline(shape: &Shape, rect: Rect) -> Vec<Pos2> {
    let (w, h) = (rect.width(), rect.height());
    outline(shape.kind, w, h)
        .into_iter()
        .map(|q| {
            let q = pos2(if shape.flip_h { w - q.x } else { q.x }, if shape.flip_v { h - q.y } else { q.y });
            place(rect, shape.rotation, q)
        })
        .collect()
}

/// The ends of a line shape drawn in `rect`: start and end.
pub fn line_ends(shape: &Shape, rect: Rect) -> (Pos2, Pos2) {
    let pts = screen_outline(shape, rect);
    (pts[0], pts[1])
}

// --- painting ------------------------------------------------------------------------

/// Cuts a simple polygon into triangles (ear clipping); indices into `pts`.
pub fn triangulate(pts: &[Pos2]) -> Vec<u32> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    // Ear clipping is quadratic and more: a long outline is cut as a fan.
    if n > 2048 {
        return (1..n as u32 - 1).flat_map(|i| [0, i, i + 1]).collect();
    }
    let area: f32 = (0..n).map(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        a.x * b.y - b.x * a.y
    }).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let cross = |a: Pos2, b: Pos2, c: Pos2| ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)) * sign;
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity((n - 2) * 3);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut cut = None;
        for i in 0..m {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (pa, pb, pc) = (pts[a], pts[b], pts[c]);
            if cross(pa, pb, pc) <= 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&k| {
                if k == a || k == b || k == c {
                    return false;
                }
                let q = pts[k];
                // Holes are bridged in with repeated points.
                if q == pa || q == pb || q == pc {
                    return false;
                }
                cross(pa, pb, q) >= 0.0 && cross(pb, pc, q) >= 0.0 && cross(pc, pa, q) >= 0.0
            });
            if !inside {
                cut = Some(i);
                break;
            }
        }
        // Degenerate outlines: cut anything so it ends.
        let i = cut.unwrap_or(0);
        let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
        out.extend([a as u32, b as u32, c as u32]);
        idx.remove(i);
    }
    if idx.len() == 3 {
        out.extend(idx.iter().map(|&i| i as u32));
    }
    out
}

/// Stroke segments of `path` with a dash pattern (in stroke widths).
pub(crate) fn dashed(path: &[Pos2], closed: bool, stroke: Stroke, pattern: &[f32], out: &mut Vec<egui::Shape>) {
    let unit = stroke.width.max(1.0);
    let lens: Vec<f32> = pattern.iter().map(|v| v * unit).collect();
    let (mut k, mut left, mut on) = (0usize, lens[0], true);
    let n = path.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let (mut a, b) = (path[i], path[(i + 1) % n]);
        let mut len = (b - a).length();
        if len <= 0.0 {
            continue;
        }
        let dir = (b - a) / len;
        while len > 0.0 {
            let step = left.min(len);
            let e = a + dir * step;
            if on {
                out.push(egui::Shape::line_segment([a, e], stroke));
            }
            a = e;
            len -= step;
            left -= step;
            if left <= 1e-4 {
                k = (k + 1) % lens.len();
                left = lens[k];
                on = k % 2 == 0;
            }
        }
    }
}

/// Automatic text color: dark on light fills, white on dark ones.
pub fn auto_text_color(shape: &Shape) -> u32 {
    shape.auto_text_color()
}

/// Text format of a run at `scale` screen pixels per point.
fn run_format(shape: &Shape, r: &ShapeRun, scale: f32) -> TextFormat {
    let size = r.size.unwrap_or(apex_doc::SHAPE_TEXT_PT) * scale;
    let (font_id, synth) = fonts::font_for(r.font.as_deref(), r.bold, r.italic, size.max(0.5));
    let color = theme::rgb(r.color.unwrap_or_else(|| auto_text_color(shape)));
    TextFormat {
        font_id,
        color,
        italics: synth,
        underline: if r.underline { Stroke::new((size / 16.0).max(1.0), color) } else { Stroke::NONE },
        ..Default::default()
    }
}

/// Lays out the paragraphs of a shape's text `width` wide.
fn layout_text(p: &Painter, shape: &Shape, width: f32, scale: f32) -> Vec<Arc<egui::Galley>> {
    shape.text.iter().map(|para| layout_para(p, shape, para, width, scale)).collect()
}

/// How far a list paragraph's text is from the left of the text area.
fn list_indent(para: &ShapePara, scale: f32) -> f32 {
    para.list.map_or(0.0, |l| ShapeList::INDENT * (l.level as f32 + 1.0) * scale)
}

fn layout_para(p: &Painter, shape: &Shape, para: &ShapePara, width: f32, scale: f32) -> Arc<egui::Galley> {
    let width = width - list_indent(para, scale);
    let mut job = LayoutJob::default();
    if para.runs.iter().all(|r| r.text.is_empty()) {
        // An empty paragraph still takes a line.
        job.append("", 0.0, run_format(shape, &para.first_run(), scale));
    }
    for r in para.runs.iter().filter(|r| !r.text.is_empty()) {
        job.append(&r.text.replace('\t', "    "), 0.0, run_format(shape, r, scale));
    }
    job.wrap.max_width = width.max(1.0);
    job.halign = match para.align {
        Align::Center => egui::Align::Center,
        Align::Right => egui::Align::RIGHT,
        _ => egui::Align::LEFT,
    };
    job.justify = para.align == Align::Justify;
    crate::vtext::prepare(&mut job, crate::vtext::flow(shape));
    p.layout_job(job)
}

/// The artistic text of a paragraph laid out by `layout_para`, by galley
/// characters.
pub fn fx_runs(para: &ShapePara) -> Vec<crate::text_fx::Run> {
    let mut out = Vec::new();
    let mut at = 0;
    for r in para.runs.iter().filter(|r| !r.text.is_empty()) {
        let n: usize = r.text.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum();
        if let Some(fx) = &r.fx {
            out.push(crate::text_fx::Run { from: at, to: at + n, fx: fx.clone() });
        }
        at += n;
    }
    out
}

/// Splits a paragraph after `shown` laid out characters (a tab shows as
/// four).
fn split_para(para: &ShapePara, shown: usize) -> (ShapePara, ShapePara) {
    let (mut a, mut b) = (ShapePara { runs: Vec::new(), ..para.clone() }, ShapePara { runs: Vec::new(), ..para.clone() });
    let mut n = 0;
    for r in &para.runs {
        let mut cut = r.text.len();
        for (i, c) in r.text.char_indices() {
            if n >= shown {
                cut = i;
                break;
            }
            n += if c == '\t' { 4 } else { 1 };
        }
        if cut > 0 {
            a.runs.push(r.with_text(&r.text[..cut]));
        }
        if cut < r.text.len() {
            b.runs.push(r.with_text(&r.text[cut..]));
        }
    }
    if a.runs.is_empty() {
        a.runs.push(para.first_run().with_text(""));
    }
    if b.runs.is_empty() {
        b.runs.push(para.first_run().with_text(""));
    }
    (a, b)
}

/// The text shown in each box of a chain of linked text boxes (`sizes`:
/// width and height in points): `head`'s text, box after box. The last
/// box gets whatever is left.
pub fn flow_chain(p: &Painter, head: &Shape, sizes: &[(f32, f32)]) -> Vec<Vec<ShapePara>> {
    let mut out = Vec::with_capacity(sizes.len());
    let mut rest: std::collections::VecDeque<ShapePara> = head.text.iter().cloned().collect();
    for (k, &(w, h)) in sizes.iter().enumerate() {
        if k + 1 == sizes.len() {
            out.push(rest.drain(..).collect());
            break;
        }
        let area = text_rect(head, Rect::from_min_size(Pos2::ZERO, vec2(w, h)), 1.0);
        let mut used = 0.0;
        let mut taken = Vec::new();
        while let Some(para) = rest.front() {
            let g = layout_para(p, head, para, area.width(), 1.0);
            if used + g.size().y <= area.height() + 0.5 {
                used += g.size().y;
                taken.extend(rest.pop_front());
                continue;
            }
            // The rows that still fit.
            let mut shown = 0;
            for row in &g.rows {
                let rh = row.row.size.y;
                if used + rh > area.height() + 0.5 {
                    break;
                }
                used += rh;
                shown += row.row.glyphs.len() + usize::from(row.ends_with_newline);
            }
            if shown > 0 {
                let (a, b) = split_para(para, shown);
                taken.push(a);
                rest[0] = b;
            }
            break;
        }
        out.push(taken);
    }
    out
}

/// The text area of a shape drawn in `rect` (before turning), with insets.
pub fn text_rect(shape: &Shape, rect: Rect, scale: f32) -> Rect {
    let (w, h) = (rect.width(), rect.height());
    let area = match shape.geometry().and_then(|g| apex_doc::preset_text_rect(g, w, h, &shape.adj)) {
        Some([l, t, r, b]) if l.is_finite() && t.is_finite() && r > l && b > t => Rect::from_min_max(pos2(l, t), pos2(r, b)),
        _ => text_area(shape.kind, w, h),
    };
    let a = area.translate(rect.min.to_vec2());
    let inner = a.shrink2(vec2(INSET_X * scale, INSET_Y * scale));
    if inner.width() < 2.0 || inner.height() < 2.0 { a } else { inner }
}

/// Paints a shape in `rect` (screen, before turning) at `scale` screen
/// pixels per point; `text`: also its text.
pub fn paint(p: &Painter, rect: Rect, shape: &Shape, scale: f32, text: bool) {
    if !rect.is_finite() || !shape.rotation.is_finite() {
        return;
    }
    if let Some(ink) = &shape.ink {
        crate::ink::paint(p, rect, shape, ink, scale);
        return;
    }
    let pts = screen_outline(shape, rect);
    let line = shape.line.map(|c| Stroke::new((shape.line_width * scale).max(1.0), theme::rgb(c)));
    let mut out = Vec::new();
    let preset = preset_paths(shape, rect);
    if let Some(paths) = &preset {
        paint_preset(p, rect, shape, paths, line, scale);
        if text && shape.has_text() {
            paint_text(p, rect, shape, scale);
        }
        return;
    }
    if shape.kind.is_line() {
        let Some(stroke) = line else { return };
        let (mut a, mut b) = (pts[0], pts[1]);
        let head = (stroke.width * 3.0).max(6.0 * scale.min(1.5));
        let dir = (b - a).normalized();
        if dir.is_finite() && (b - a).length() > head * 1.2 {
            if shape.arrow_end {
                arrowhead(b, dir, head, stroke.color, &mut out);
                b -= dir * head * 0.8;
            }
            if shape.arrow_start {
                arrowhead(a, -dir, head, stroke.color, &mut out);
                a += dir * head * 0.8;
            }
        }
        if shape.dash == Dash::Solid {
            out.insert(0, egui::Shape::line_segment([a, b], stroke));
        } else {
            let mut segs = Vec::new();
            dashed(&[a, b], false, stroke, shape.dash.pattern(), &mut segs);
            out.splice(0..0, segs);
        }
        p.extend(out);
        return;
    }
    if let (Some(_), Some(g)) = (shape.fill, &shape.gradient) {
        let (w, h) = (rect.width(), rect.height());
        p.add(egui::Shape::mesh(crate::gradient::mesh(g, &[outline(shape.kind, w, h)], w, h, &placer(shape, rect), &|c| c)));
    } else if let Some(c) = shape.fill {
        let fill = theme::rgb(c);
        if convex(shape.kind) {
            p.add(egui::Shape::convex_polygon(pts.clone(), fill, Stroke::NONE));
        } else {
            let mut mesh = Mesh::default();
            for q in &pts {
                mesh.colored_vertex(*q, fill);
            }
            mesh.indices = triangulate(&pts);
            p.add(egui::Shape::mesh(mesh));
        }
    }
    if let Some(img) = &shape.picture {
        match &img.object {
            Some(o) => crate::objects::paint(p, rect, &img.data, o, shape.rotation, false),
            None => crate::images::paint_picture(p, &img.data, rect, shape.rotation, img.crop, img.fx.as_deref(), img.width, false),
        }
    }
    if let Some(stroke) = line {
        if shape.dash == Dash::Solid {
            p.add(egui::Shape::closed_line(pts, stroke));
        } else {
            let mut segs = Vec::new();
            dashed(&pts, true, stroke, shape.dash.pattern(), &mut segs);
            p.extend(segs);
        }
    }
    if text && shape.has_text() {
        paint_text(p, rect, shape, scale);
    }
}

/// Maps a point of a shape's own box (from its top left, before flips) to
/// the screen.
fn placer(shape: &Shape, rect: Rect) -> impl Fn(Pos2) -> Pos2 + '_ {
    let (w, h, rot, fh, fv) = (rect.width(), rect.height(), shape.rotation, shape.flip_h, shape.flip_v);
    move |q| place(rect, rot, pos2(if fh { w - q.x } else { q.x }, if fv { h - q.y } else { q.y }))
}

/// The paths of a shape drawn with a preset's standard geometry, on the
/// screen (turned and flipped); None: it is drawn by its kind.
fn preset_paths(shape: &Shape, rect: Rect) -> Option<Vec<(Vec<(Vec<Pos2>, bool)>, apex_doc::PresetFill, bool)>> {
    let (w, h) = (rect.width(), rect.height());
    let paths = local_paths(shape, w, h)?;
    let map = |(x, y): (f32, f32)| place(rect, shape.rotation, pos2(if shape.flip_h { w - x } else { x }, if shape.flip_v { h - y } else { y }));
    Some(paths.into_iter().map(|p| (p.subpaths.into_iter().map(|(pts, closed)| (pts.into_iter().map(map).collect(), closed)).collect(), p.fill, p.stroke)).collect())
}

/// The paths of a shape's geometry (a drawn one, or a preset's) in its own
/// `w` by `h` box.
fn local_paths(shape: &Shape, w: f32, h: f32) -> Option<Vec<apex_doc::PresetPath>> {
    if let Some(c) = &shape.custom {
        let v = apex_doc::custom_paths(c, w, h);
        return (!v.is_empty()).then_some(v);
    }
    let g = shape.geometry()?;
    if matches!(g, "line" | "straightConnector1") {
        return None;
    }
    apex_doc::preset_paths(g, w, h, &shape.adj)
}

/// A fill color shaded as a preset's path says (the sides of a cube...).
pub(crate) fn shaded(c: Color32, mode: apex_doc::PresetFill) -> Color32 {
    use apex_doc::PresetFill as F;
    let mix = |to: f32, k: f32| {
        let m = |v: u8| (v as f32 + (to - v as f32) * k).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_unmultiplied(m(c.r()), m(c.g()), m(c.b()), c.a())
    };
    match mode {
        F::Lighten => mix(255.0, 0.4),
        F::LightenLess => mix(255.0, 0.2),
        F::Darken => mix(0.0, 0.4),
        F::DarkenLess => mix(0.0, 0.2),
        _ => c,
    }
}

/// Twice the signed area of a polygon.
fn area2(pts: &[Pos2]) -> f32 {
    (0..pts.len()).map(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        a.x * b.y - b.x * a.y
    }).sum()
}

fn inside(pts: &[Pos2], q: Pos2) -> bool {
    let mut on = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + n - 1) % n]);
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            on = !on;
        }
    }
    on
}

/// The polygons to fill for the subpaths of one path: the ones inside
/// another are its holes, bridged into it.
pub(crate) fn merge_holes(subs: &[(Vec<Pos2>, bool)]) -> Vec<Vec<Pos2>> {
    let mut polys: Vec<Vec<Pos2>> = subs.iter().map(|(pts, _)| pts.clone()).filter(|v| v.len() >= 3).collect();
    polys.sort_by(|a, b| area2(b).abs().total_cmp(&area2(a).abs()));
    let mut outers: Vec<Vec<Pos2>> = Vec::new();
    for mut poly in polys {
        let Some(o) = outers.iter_mut().find(|o| inside(o, poly[0])) else {
            outers.push(poly);
            continue;
        };
        // A hole: turn it against its outline and bridge it in from its
        // rightmost point to the nearest point of the outline.
        if area2(&poly).signum() == area2(o).signum() {
            poly.reverse();
        }
        let hi = (0..poly.len()).max_by(|&a, &b| poly[a].x.total_cmp(&poly[b].x)).unwrap_or(0);
        let q = poly[hi];
        let oj = (0..o.len()).min_by(|&a, &b| o[a].distance_sq(q).total_cmp(&o[b].distance_sq(q))).unwrap_or(0);
        let mut merged = o[..=oj].to_vec();
        merged.extend(poly[hi..].iter().chain(poly[..=hi].iter()).copied());
        merged.extend(o[oj..].iter().copied());
        *o = merged;
    }
    outers
}

/// Fills the subpaths of one path.
pub(crate) fn fill_subpaths(p: &Painter, subs: &[(Vec<Pos2>, bool)], fill: Color32) {
    for o in merge_holes(subs) {
        if convex_poly(&o) {
            p.add(egui::Shape::convex_polygon(o, fill, Stroke::NONE));
            continue;
        }
        let mut mesh = Mesh::default();
        for q in &o {
            mesh.colored_vertex(*q, fill);
        }
        mesh.indices = triangulate(&o);
        p.add(egui::Shape::mesh(mesh));
    }
}

/// Whether a polygon is convex (no turn the other way).
pub(crate) fn convex_poly(pts: &[Pos2]) -> bool {
    let n = pts.len();
    let (mut pos, mut neg) = (false, false);
    for i in 0..n {
        let (a, b, c) = (pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        let x = (b - a).x * (c - b).y - (b - a).y * (c - b).x;
        pos |= x > 1e-3;
        neg |= x < -1e-3;
    }
    !(pos && neg)
}

/// Paints a shape by its preset's paths: fills (shaded as they say), its
/// picture, then the outlines, with arrowheads on the ends of open ones.
fn paint_preset(p: &Painter, rect: Rect, shape: &Shape, paths: &[(Vec<(Vec<Pos2>, bool)>, apex_doc::PresetFill, bool)], line: Option<Stroke>, scale: f32) {
    let gradient = shape.gradient.as_ref().filter(|_| shape.fill.is_some() && !shape.kind.is_line());
    let local = gradient.and_then(|_| local_paths(shape, rect.width(), rect.height()));
    if let (Some(g), Some(local)) = (gradient, local) {
        let (w, h) = (rect.width(), rect.height());
        let place = placer(shape, rect);
        for path in local.iter().filter(|x| x.fill != apex_doc::PresetFill::None) {
            let subs: Vec<(Vec<Pos2>, bool)> = path.subpaths.iter().map(|(pts, c)| (pts.iter().map(|&(x, y)| pos2(x, y)).collect(), *c)).collect();
            let mode = path.fill;
            p.add(egui::Shape::mesh(crate::gradient::mesh(g, &merge_holes(&subs), w, h, &place, &|c| shaded(c, mode))));
        }
    } else if let Some(c) = shape.fill.filter(|_| !shape.kind.is_line()) {
        let fill = theme::rgb(c);
        for (subs, mode, _) in paths {
            if *mode != apex_doc::PresetFill::None {
                fill_subpaths(p, subs, shaded(fill, *mode));
            }
        }
    }
    if let Some(img) = &shape.picture {
        match &img.object {
            Some(o) => crate::objects::paint(p, rect, &img.data, o, shape.rotation, false),
            None => crate::images::paint_picture(p, &img.data, rect, shape.rotation, img.crop, img.fx.as_deref(), img.width, false),
        }
    }
    let Some(stroke) = line else { return };
    let mut open: Vec<Vec<Pos2>> = Vec::new();
    let mut closed: Vec<Vec<Pos2>> = Vec::new();
    for (subs, _, st) in paths {
        if *st {
            for (pts, c) in subs {
                if *c { closed.push(pts.clone()) } else { open.push(pts.clone()) }
            }
        }
    }
    let mut out = Vec::new();
    let head = (stroke.width * 3.0).max(6.0 * scale.min(1.5));
    // The arrowheads go on the first open path's start and the last one's end.
    let n = open.len();
    for (i, want, at_end) in [(0, shape.arrow_start, false), (n.wrapping_sub(1), shape.arrow_end, true)] {
        let Some(pts) = open.get_mut(i).filter(|_| want) else { continue };
        let k = if at_end { pts.len() - 1 } else { 0 };
        let tip = pts[k];
        let back = if at_end { pts.iter().rev().find(|q| q.distance(tip) > head * 0.5) } else { pts.iter().find(|q| q.distance(tip) > head * 0.5) };
        let Some(&from) = back else { continue };
        let dir = (tip - from).normalized();
        if dir.is_finite() {
            arrowhead(tip, dir, head, stroke.color, &mut out);
            pts[k] = tip - dir * head * 0.8;
        }
    }
    for (pts, c) in open.into_iter().map(|v| (v, false)).chain(closed.into_iter().map(|v| (v, true))) {
        if shape.dash != Dash::Solid {
            dashed(&pts, c, stroke, shape.dash.pattern(), &mut out);
        } else if c {
            out.push(egui::Shape::closed_line(pts, stroke));
        } else {
            out.push(egui::Shape::line(pts, stroke));
        }
    }
    p.extend(out);
}

pub(crate) fn arrowhead(tip: Pos2, dir: Vec2, size: f32, color: Color32, out: &mut Vec<egui::Shape>) {
    let n = vec2(-dir.y, dir.x);
    let base = tip - dir * size;
    out.push(egui::Shape::convex_polygon(vec![tip, base + n * size * 0.5, base - n * size * 0.5], color, Stroke::NONE));
}

/// A paragraph of a shape's text laid out: the galley, where it is drawn
/// (unturned) and its bullet or number.
pub(crate) struct ParaBox {
    pub galley: Arc<egui::Galley>,
    pub pos: Pos2,
    pub label: Option<(Arc<egui::Galley>, Pos2)>,
}

/// Lays out the horizontal text of a shape in its text `area` (screen).
pub(crate) fn place_text(p: &Painter, shape: &Shape, area: Rect, scale: f32) -> Vec<ParaBox> {
    let galleys = layout_text(p, shape, area.width(), scale);
    let total: f32 = galleys.iter().map(|g| g.size().y).sum();
    let mut y = match shape.valign {
        VAnchor::Top => area.min.y,
        VAnchor::Middle => area.center().y - total / 2.0,
        VAnchor::Bottom => area.max.y - total,
    };
    let labels = apex_doc::list_labels(&shape.text);
    let mut out = Vec::with_capacity(galleys.len());
    for ((g, para), label) in galleys.into_iter().zip(&shape.text).zip(labels) {
        let indent = list_indent(para, scale);
        // Galleys of centered or right aligned text are laid out around x = 0.
        let x = match g.job.halign {
            egui::Align::Center => (area.min.x + indent + area.max.x) / 2.0,
            egui::Align::Max => area.max.x,
            _ => area.min.x + indent,
        };
        let label = label.map(|t| {
            let mut job = LayoutJob::default();
            job.append(&t, 0.0, run_format(shape, &para.first_run(), scale));
            let lg = p.layout_job(job);
            // On the first line's baseline.
            let dy = g.rows.first().and_then(|r| r.row.glyphs.first()).zip(lg.rows.first().and_then(|r| r.row.glyphs.first())).map_or(0.0, |(a, b)| a.pos.y - b.pos.y);
            (lg, pos2(area.min.x + indent - ShapeList::INDENT * scale, y + dy))
        });
        let h = g.size().y;
        out.push(ParaBox { galley: g, pos: pos2(x, y), label });
        y += h;
    }
    out
}

/// Paints laid out text; `turn_by`: the center to turn it around and the
/// angle in degrees.
pub(crate) fn paint_boxes(p: &Painter, shape: &Shape, boxes: &[ParaBox], scale: f32, turn_by: Option<(Pos2, f32)>) {
    let (center, rotation) = turn_by.unwrap_or((Pos2::ZERO, 0.0));
    let angle = rotation.to_radians();
    let at = |q: Pos2| if rotation == 0.0 { q } else { center + turn(rotation, q - center) };
    let bg = shape.fill.map_or(Color32::WHITE, theme::rgb);
    for (b, para) in boxes.iter().zip(&shape.text) {
        if let Some((lg, lp)) = &b.label {
            p.add(egui::epaint::TextShape::new(at(*lp), lg.clone(), Color32::BLACK).with_angle(angle));
        }
        let pos = at(b.pos);
        let runs = fx_runs(para);
        let (g, fx) = crate::text_fx::apply(b.galley.clone(), &runs, scale, bg);
        if rotation == 0.0 {
            crate::text_fx::paint(p, pos, &fx);
        } else {
            crate::text_fx::paint_turned(p, pos, angle, &fx);
        }
        p.add(egui::epaint::TextShape::new(pos, g, Color32::BLACK).with_angle(angle));
    }
}

/// The text of a shape, turned with it.
fn paint_text(p: &Painter, rect: Rect, shape: &Shape, scale: f32) {
    let area = text_rect(shape, rect, scale);
    let flow = crate::vtext::flow(shape);
    if flow != crate::vtext::Flow::Horizontal {
        // Turned and vertical text wraps at the box's height.
        let galleys = layout_text(p, shape, area.height(), scale);
        crate::vtext::paint(p, area, shape.valign, flow, galleys, rect.center(), shape.rotation);
        return;
    }
    let boxes = place_text(p, shape, area, scale);
    paint_boxes(p, shape, &boxes, scale, Some((rect.center(), shape.rotation)));
}

// --- hit testing and handles ---------------------------------------------------------

/// Whether screen point `q` is on a shape drawn in `rect`.
pub fn hit(shape: &Shape, rect: Rect, q: Pos2) -> bool {
    if shape.ink.is_some() {
        // Ink by its strokes: what is under it stays reachable.
        return !crate::ink::strokes_near(shape, rect, q, GRAB, 1.0).is_empty();
    }
    if shape.kind.is_line() {
        let (a, b) = line_ends(shape, rect);
        return seg_dist(q, a, b) <= GRAB.max(shape.line_width);
    }
    let l = local(rect, shape.rotation, q);
    l.x.abs() <= rect.width() / 2.0 + 2.0 && l.y.abs() <= rect.height() / 2.0 + 2.0
}

fn seg_dist(q: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((q - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t - q).length()
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Handle {
    Move,
    /// Signs of the handle along the width and height.
    Resize(i8, i8),
    Rotate,
    /// The ends of a line.
    Start,
    End,
}

/// Handles of a shape drawn in `rect` (screen).
pub fn handles(shape: &Shape, rect: Rect) -> Vec<(Handle, Pos2)> {
    if shape.kind.is_line() {
        let (a, b) = line_ends(shape, rect);
        return vec![(Handle::Start, a), (Handle::End, b)];
    }
    let c = rect.center();
    let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
    let mut v = Vec::with_capacity(9);
    for sy in -1i8..=1 {
        for sx in -1i8..=1 {
            if sx != 0 || sy != 0 {
                v.push((Handle::Resize(sx, sy), c + turn(shape.rotation, vec2(sx as f32 * hw, sy as f32 * hh))));
            }
        }
    }
    v.push((Handle::Rotate, c + turn(shape.rotation, vec2(0.0, -hh - ROT_ARM))));
    v
}

/// The handle of the selected shape under screen point `q`.
pub fn handle_at(shape: &Shape, rect: Rect, q: Pos2) -> Option<Handle> {
    crate::touch::nearest(handles(shape, rect), q, GRAB)
}

pub fn cursor(h: Handle, rotation: f32) -> CursorIcon {
    match h {
        Handle::Move => CursorIcon::Move,
        Handle::Rotate => CursorIcon::Grab,
        Handle::Start | Handle::End => CursorIcon::Crosshair,
        Handle::Resize(sx, sy) => {
            let a = (sy as f32).atan2(sx as f32).to_degrees() + rotation;
            match ((a / 45.0).round() as i32).rem_euclid(4) {
                0 => CursorIcon::ResizeHorizontal,
                1 => CursorIcon::ResizeNwSe,
                2 => CursorIcon::ResizeVertical,
                _ => CursorIcon::ResizeNeSw,
            }
        }
    }
}

/// Outline and handles of the selected shape drawn in `rect`.
pub fn paint_handles(p: &Painter, shape: &Shape, rect: Rect, accent: Color32) {
    let hs = handles(shape, rect);
    let at = |h: Handle| hs.iter().find(|(k, _)| *k == h).map(|(_, a)| *a);
    let scale = if shape.kind.is_line() { at(Handle::End).zip(at(Handle::Start)) } else { at(Handle::Resize(1, 1)).zip(at(Handle::Resize(-1, -1))) };
    crate::touch::grip(p, hs.iter().fold(rect, |r, (_, a)| r.union(Rect::from_center_size(*a, Vec2::ZERO))), crate::touch::reach(GRAB), scale);
    if !shape.kind.is_line() {
        crate::images::outline(p, rect, shape.rotation, Stroke::new(1.0, accent));
        if let (Some((_, top)), Some((_, rot))) = (hs.iter().find(|(h, _)| *h == Handle::Resize(0, -1)), hs.last()) {
            p.line_segment([*top, *rot], Stroke::new(1.0, accent));
        }
    }
    for (h, at) in hs {
        match h {
            Handle::Rotate => {
                p.circle(at, HANDLE / 2.0 + 1.0, Color32::WHITE, Stroke::new(1.5, accent));
            }
            Handle::Start | Handle::End => {
                p.circle(at, HANDLE / 2.0 + 0.5, Color32::WHITE, Stroke::new(1.0, accent));
            }
            _ => {
                let r = Rect::from_center_size(at, Vec2::splat(HANDLE));
                p.rect(r, 1.0, Color32::WHITE, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
            }
        }
    }
}

/// A shape's box in its container's units: top-left corner (before
/// turning), size, rotation and flips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxGeo {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rotation: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl BoxGeo {
    pub fn of(shape: &Shape, x: f32, y: f32, w: f32, h: f32) -> BoxGeo {
        BoxGeo { x, y, w, h, rotation: shape.rotation, flip_h: shape.flip_h, flip_v: shape.flip_v }
    }

    pub fn center(&self) -> Pos2 {
        pos2(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Copies rotation and flips into `shape`.
    pub fn apply(&self, shape: &mut Shape) {
        shape.rotation = self.rotation;
        shape.flip_h = self.flip_h;
        shape.flip_v = self.flip_v;
    }

    /// The ends of a line in this box, turned: start and end.
    pub fn ends(&self) -> (Pos2, Pos2) {
        let (x0, x1) = if self.flip_h { (self.w, 0.0) } else { (0.0, self.w) };
        let (y0, y1) = if self.flip_v { (self.h, 0.0) } else { (0.0, self.h) };
        let c = self.center();
        let at = |x: f32, y: f32| c + turn(self.rotation, pos2(self.x + x, self.y + y) - c);
        (at(x0, y0), at(x1, y1))
    }

    /// The box of a line from `a` to `b` (not turned).
    pub fn line(a: Pos2, b: Pos2) -> BoxGeo {
        BoxGeo { x: a.x.min(b.x), y: a.y.min(b.y), w: (b.x - a.x).abs(), h: (b.y - a.y).abs(), rotation: 0.0, flip_h: b.x < a.x, flip_v: b.y < a.y }
    }
}

/// The box drawn by dragging from `a` to `b` (container units). Shift
/// makes it square (lines: at a multiple of 45 degrees).
pub fn drawn(kind: ShapeKind, a: Pos2, b: Pos2, shift: bool) -> BoxGeo {
    let mut d = b - a;
    if kind.is_line() {
        if shift {
            d = snap45(d);
        }
        return BoxGeo::line(a, a + d);
    }
    if shift {
        let s = d.x.abs().max(d.y.abs());
        d = vec2(s * d.x.signum(), s * d.y.signum());
    }
    let (p, q) = (a, a + d);
    BoxGeo { x: p.x.min(q.x), y: p.y.min(q.y), w: d.x.abs(), h: d.y.abs(), rotation: 0.0, flip_h: false, flip_v: false }
}

fn snap45(d: Vec2) -> Vec2 {
    let len = d.length();
    let a = (d.y.atan2(d.x).to_degrees() / 45.0).round() * 45.0;
    let r = a.to_radians();
    let v = vec2(r.cos(), r.sin()) * len;
    // Diagonals keep the longer side.
    if (a.rem_euclid(90.0) - 45.0).abs() < 1.0 { vec2(d.x.abs().max(d.y.abs()) * v.x.signum(), d.x.abs().max(d.y.abs()) * v.y.signum()) } else { v }
}

/// The box after dragging `handle` by `d` (container units) from `orig`;
/// `keep`: keep the proportions. `min`: the smallest side.
pub fn dragged(orig: BoxGeo, handle: Handle, d: Vec2, keep: bool, min: f32) -> BoxGeo {
    match handle {
        Handle::Move => {
            let mut d = d;
            if keep {
                if d.x.abs() > d.y.abs() {
                    d.y = 0.0;
                } else {
                    d.x = 0.0;
                }
            }
            BoxGeo { x: orig.x + d.x, y: orig.y + d.y, ..orig }
        }
        Handle::Start | Handle::End => {
            let (a, b) = orig.ends();
            let (fixed, moved) = if handle == Handle::Start { (b, a + d) } else { (a, b + d) };
            let moved = if keep { fixed + snap45(moved - fixed) } else { moved };
            if handle == Handle::Start { BoxGeo::line(moved, fixed) } else { BoxGeo::line(fixed, moved) }
        }
        Handle::Resize(sx, sy) => {
            let l = turn(-orig.rotation, d);
            let (w, h) = (orig.w, orig.h);
            let (fx, fy) = (sx as f32, sy as f32);
            let mut nw = if sx != 0 { (w + fx * l.x).max(min) } else { w };
            let mut nh = if sy != 0 { (h + fy * l.y).max(min) } else { h };
            if keep && w > 0.0 && h > 0.0 {
                let k = match (sx, sy) {
                    (0, _) => nh / h,
                    (_, 0) => nw / w,
                    _ => (nw / w).max(nh / h),
                };
                let k = k.max(min / w.min(h));
                (nw, nh) = (w * k, h * k);
            }
            let shift = vec2(fx * (nw - w) / 2.0, fy * (nh - h) / 2.0);
            let c = orig.center() + turn(orig.rotation, shift);
            BoxGeo { x: c.x - nw / 2.0, y: c.y - nh / 2.0, w: nw, h: nh, ..orig }
        }
        Handle::Rotate => orig,
    }
}

/// Rotation for the rotate handle dragged to `q` around `center` (screen);
/// Shift snaps to 15 degrees.
pub fn rotation_to(center: Pos2, q: Pos2, snap: bool) -> f32 {
    let v = q - center;
    let mut a = (v.y.atan2(v.x).to_degrees() + 90.0).rem_euclid(360.0);
    if snap {
        a = (a / 15.0).round() * 15.0 % 360.0;
    }
    a
}

// --- gallery ---------------------------------------------------------------------------

/// A shape to insert: its kind and, for lines, the arrowhead; `preset`:
/// one of Office's presets we draw by the standard's geometry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Template {
    pub kind: ShapeKind,
    pub arrow: bool,
    pub preset: Option<&'static str>,
}

impl Template {
    pub fn shape(self) -> Shape {
        let mut s = Shape::new(self.kind);
        s.preset = self.preset.map(Arc::from);
        s.arrow_end = self.arrow;
        // Office inserts these as lines, with no fill.
        if self.preset.is_some_and(|p| matches!(p, "arc" | "bracketPair" | "bracePair" | "leftBracket" | "rightBracket" | "leftBrace" | "rightBrace") || p.contains("Connector")) {
            s.fill = None;
        }
        s
    }

    /// The template of an Office preset.
    fn of(name: &'static str) -> Template {
        let (kind, exact) = ShapeKind::from_preset(name);
        Template { kind, arrow: false, preset: (!exact).then_some(name) }
    }

    fn key(self) -> &'static str {
        if self.arrow { "shape.k.arrow_line" } else { self.kind.key() }
    }
}

/// The gallery's groups, as Office has them: title key and presets
/// ("textBox": a text box).
const GROUPS: [(&str, &[&str]); 9] = [
    ("shape.g.lines", &["line", "straightConnector1", "bentConnector3", "bentConnector3>", "curvedConnector3", "curvedConnector3>"]),
    ("shape.g.rects", &["rect", "roundRect", "snip1Rect", "snip2SameRect", "snip2DiagRect", "snipRoundRect", "round1Rect", "round2SameRect", "round2DiagRect"]),
    (
        "shape.g.basic",
        &[
            "textBox", "ellipse", "triangle", "rtTriangle", "parallelogram", "trapezoid", "diamond", "pentagon", "hexagon", "heptagon", "octagon", "decagon", "dodecagon", "pie", "chord",
            "teardrop", "frame", "halfFrame", "corner", "diagStripe", "plus", "plaque", "can", "cube", "bevel", "donut", "noSmoking", "blockArc", "foldedCorner", "smileyFace", "heart",
            "lightningBolt", "sun", "moon", "cloud", "arc", "bracketPair", "bracePair", "leftBracket", "rightBracket", "leftBrace", "rightBrace", "gear6", "gear9", "funnel", "pieWedge",
            "chartX", "chartStar", "chartPlus",
        ],
    ),
    (
        "shape.g.arrows",
        &[
            "rightArrow", "leftArrow", "upArrow", "downArrow", "leftRightArrow", "upDownArrow", "quadArrow", "leftRightUpArrow", "bentArrow", "uturnArrow", "leftUpArrow", "bentUpArrow",
            "curvedRightArrow", "curvedLeftArrow", "curvedUpArrow", "curvedDownArrow", "stripedRightArrow", "notchedRightArrow", "homePlate", "chevron", "rightArrowCallout",
            "downArrowCallout", "leftArrowCallout", "upArrowCallout", "leftRightArrowCallout", "upDownArrowCallout", "quadArrowCallout", "circularArrow", "leftCircularArrow",
            "leftRightCircularArrow", "swooshArrow", "leftRightRibbon",
        ],
    ),
    ("shape.g.equation", &["mathPlus", "mathMinus", "mathMultiply", "mathDivide", "mathEqual", "mathNotEqual"]),
    (
        "shape.g.flowchart",
        &[
            "flowChartProcess", "flowChartAlternateProcess", "flowChartDecision", "flowChartInputOutput", "flowChartPredefinedProcess", "flowChartInternalStorage",
            "flowChartDocument", "flowChartMultidocument", "flowChartTerminator", "flowChartPreparation", "flowChartManualInput", "flowChartManualOperation",
            "flowChartConnector", "flowChartOffpageConnector", "flowChartPunchedCard", "flowChartPunchedTape", "flowChartSummingJunction", "flowChartOr", "flowChartCollate",
            "flowChartSort", "flowChartExtract", "flowChartMerge", "flowChartOnlineStorage", "flowChartDelay", "flowChartMagneticTape", "flowChartMagneticDisk",
            "flowChartMagneticDrum", "flowChartDisplay", "flowChartOfflineStorage",
        ],
    ),
    (
        "shape.g.stars",
        &[
            "irregularSeal1", "irregularSeal2", "star4", "star5", "star6", "star7", "star8", "star10", "star12", "star16", "star24", "star32", "ribbon2", "ribbon", "ellipseRibbon2",
            "ellipseRibbon", "verticalScroll", "horizontalScroll", "wave", "doubleWave",
        ],
    ),
    (
        "shape.g.callouts",
        &[
            "wedgeRectCallout", "wedgeRoundRectCallout", "wedgeEllipseCallout", "cloudCallout", "borderCallout1", "borderCallout2", "borderCallout3", "accentCallout1",
            "accentCallout2", "accentCallout3", "callout1", "callout2", "callout3", "accentBorderCallout1", "accentBorderCallout2", "accentBorderCallout3",
        ],
    ),
    (
        "shape.g.buttons",
        &[
            "actionButtonBackPrevious", "actionButtonForwardNext", "actionButtonBeginning", "actionButtonEnd", "actionButtonHome", "actionButtonInformation",
            "actionButtonReturn", "actionButtonMovie", "actionButtonDocument", "actionButtonSound", "actionButtonHelp", "actionButtonBlank",
        ],
    ),
];

/// The gallery's groups: title key and items.
fn groups() -> Vec<(&'static str, Vec<Template>)> {
    GROUPS
        .iter()
        .map(|&(title, names)| {
            let items = names
                .iter()
                .map(|&n| match n {
                    "textBox" => Template { kind: ShapeKind::TextBox, arrow: false, preset: None },
                    "straightConnector1" => Template { kind: ShapeKind::Line, arrow: true, preset: None },
                    _ => match n.strip_suffix('>') {
                        Some(n) => Template { arrow: true, ..Template::of(n) },
                        None => Template::of(n),
                    },
                })
                .collect();
            (title, items)
        })
        .collect()
}

/// A preset's name for screen readers: "flowChartPunchedTape" is
/// "Flow chart punched tape".
fn spoken(preset: &str) -> String {
    let mut out = String::new();
    for c in preset.chars() {
        if out.is_empty() {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() || (c.is_ascii_digit() && !out.ends_with(|p: char| p.is_ascii_digit())) {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// An icon of a shape in `rect`.
pub fn icon(p: &Painter, rect: Rect, t: Template, color: Color32) {
    let mut s = t.shape();
    let r = if t.kind.is_line() {
        s.flip_v = true;
        rect.shrink(3.0)
    } else if matches!(t.kind, ShapeKind::RightArrow | ShapeKind::LeftArrow) {
        Rect::from_center_size(rect.center(), vec2(rect.width() - 4.0, rect.height() * 0.6))
    } else if matches!(t.kind, ShapeKind::UpArrow | ShapeKind::DownArrow) {
        Rect::from_center_size(rect.center(), vec2(rect.width() * 0.6, rect.height() - 4.0))
    } else if t.kind == ShapeKind::Callout || t.preset.is_some_and(|p| p.ends_with("Callout") && !p.contains("Arrow") || p.contains("allout1") || p.contains("allout2") || p.contains("allout3")) {
        Rect::from_min_size(rect.min + vec2(2.0, 3.0), vec2(rect.width() - 4.0, rect.height() * 0.62))
    } else {
        rect.shrink(3.0)
    };
    s.fill = s.fill.filter(|_| !t.kind.is_line()).map(|_| 0xDDE6F4);
    s.line = Some(((color.r() as u32) << 16) | ((color.g() as u32) << 8) | color.b() as u32);
    s.line_width = 1.0;
    paint(p, r, &s, 1.0, false);
    if t.kind == ShapeKind::TextBox {
        for (i, k) in [0.3f32, 0.5, 0.7].iter().enumerate() {
            let y = r.min.y + r.height() * k;
            let x1 = r.max.x - 4.0 - if i == 2 { r.width() * 0.3 } else { 0.0 };
            p.line_segment([pos2(r.min.x + 4.0, y), pos2(x1, y)], Stroke::new(1.0, color));
        }
    }
}

/// The Insert > Shapes gallery: returns the shape picked.
pub fn gallery(ui: &mut egui::Ui) -> Option<Template> {
    let mut out = None;
    ui.set_min_width(290.0);
    ui.set_max_width(290.0);
    let color = ui.visuals().text_color();
    egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| for (title, items) in groups() {
        ui.label(RichText::new(tr(title)).small().weak());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            for t in items {
                let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::click());
                crate::widgets::name(&resp, egui::WidgetType::Button, &t.preset.map_or_else(|| tr(t.key()).to_string(), spoken), None);
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                icon(ui.painter(), rect.shrink(2.0), t, color);
                // Office's other presets have no names of their own here.
                let resp = if t.preset.is_none() { resp.on_hover_text(tr(t.key())) } else { resp };
                if resp.clicked() {
                    out = Some(t);
                }
            }
        });
    });
    if out.is_some() {
        ui.close();
    }
    out
}

/// Default size of a shape inserted with a click (no drag), points.
pub fn default_size(kind: ShapeKind) -> (f32, f32) {
    match kind {
        ShapeKind::Line => (72.0, 0.0),
        ShapeKind::TextBox => (144.0, 54.0),
        ShapeKind::RightArrow | ShapeKind::LeftArrow => (96.0, 48.0),
        ShapeKind::UpArrow | ShapeKind::DownArrow => (48.0, 96.0),
        _ => (72.0, 72.0),
    }
}

/// Bring forward, to front, send backward, to back.
pub fn order_menu(ui: &mut egui::Ui, mut apply: impl FnMut(apex_sheet::shape::Order)) {
    for (to, key) in [(apex_sheet::shape::Order::Front, "shape.front"), (apex_sheet::shape::Order::Forward, "shape.forward"), (apex_sheet::shape::Order::Backward, "shape.backward"), (apex_sheet::shape::Order::Back, "shape.back")] {
        if ui.button(tr(key)).clicked() {
            apply(to);
            ui.close();
        }
    }
}

// --- Format Shape dialog ---------------------------------------------------------------

/// State of the Format Shape dialog shared by both programs.
pub struct FormatDlg {
    pub shape: Shape,
    /// Size in container units.
    pub w: f32,
    pub h: f32,
    pub lock: bool,
    pub tab: usize,
    /// First frame (the click that opened it must not close it).
    pub fresh: bool,
}

impl FormatDlg {
    pub fn new(shape: Shape, w: f32, h: f32, tab: usize) -> FormatDlg {
        FormatDlg { shape, w, h, lock: false, tab, fresh: true }
    }
}

/// A color button with the color menu; `none_key`: the "no color" entry.
fn color_row(ui: &mut egui::Ui, label: &str, current: Option<u32>, none_key: &'static str) -> Option<Option<u32>> {
    ui.label(label);
    let mut out = None;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
        match current {
            Some(c) => {
                ui.painter().rect_filled(rect, 2.0, theme::rgb(c));
            }
            None => {
                ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                ui.painter().line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, Color32::from_rgb(200, 60, 60)));
            }
        }
        crate::widgets::menu_button(ui, tr("shape.pick_color"), |ui| {
            if let Some(c) = crate::widgets::color_menu(ui, &tr(none_key), current) {
                out = Some(c);
            }
        });
    });
    ui.end_row();
    out
}

/// The Format Shape dialog; `cm`: centimeters per container unit;
/// `extra`: the container's own page (position and wrapping, or cell
/// anchoring), its icon and label key.
pub fn format_dialog(ctx: &egui::Context, id: &str, d: &mut FormatDlg, cm: f32, extra: (crate::icons::Icon, &'static str), extra_ui: impl FnOnce(&mut egui::Ui)) -> crate::widgets::DialogResult {
    use crate::icons::Icon;
    let mut pages = vec![(Icon::Fill, "shape.tab_fill", 0), (Icon::AutoFit, "shape.tab_size", 1)];
    if d.shape.can_have_text() {
        pages.push((Icon::TextBox, "shape.tab_text", 2));
    }
    pages.push((extra.0, extra.1, 3));
    pages.push((Icon::Comment, "shape.tab_alt", 4));
    let list: Vec<(Icon, &'static str)> = pages.iter().map(|p| (p.0, p.1)).collect();
    let mut page = pages.iter().position(|p| p.2 == d.tab).unwrap_or(0);
    let res = crate::widgets::nav_dialog(ctx, id, tr("shape.format"), &list, &mut page, vec2(500.0, 360.0), |ui, i| {
        crate::widgets::section(ui, tr(pages[i].1));
        format_page(ui, d, cm, pages[i].2, extra_ui)
    });
    d.tab = pages[page].2;
    res
}

fn format_page(ui: &mut egui::Ui, d: &mut FormatDlg, cm: f32, tab: usize, extra: impl FnOnce(&mut egui::Ui)) {
    let s = &mut d.shape;
    match tab {
        0 => {
            if !s.kind.is_line() {
                crate::gradient_ui::fill_editor(ui, egui::Id::new("shape-grad"), &mut s.fill, &mut s.gradient, "shape.no_fill");
                ui.separator();
            }
            crate::widgets::form(ui, "shape-fill", |ui| {
                if let Some(c) = color_row(ui, &tr("shape.line"), s.line, "shape.no_line") {
                    s.line = c;
                }
                ui.label(tr("shape.line_width"));
                ui.add(egui::DragValue::new(&mut s.line_width).range(0.0..=50.0).speed(0.25).max_decimals(2).suffix(" pt"));
                ui.end_row();
                ui.label(tr("shape.dash"));
                egui::ComboBox::from_id_salt("shape-dash").selected_text(tr(s.dash.key())).show_ui(ui, |ui| {
                    for k in Dash::ALL {
                        ui.selectable_value(&mut s.dash, k, tr(k.key()));
                    }
                });
                ui.end_row();
                if s.kind.is_line() {
                    ui.label(tr("shape.arrows"));
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.arrow_start, tr("shape.arrow_start"));
                        ui.checkbox(&mut s.arrow_end, tr("shape.arrow_end"));
                    });
                    ui.end_row();
                }
            });
        }
        1 => {
            crate::widgets::form(ui, "shape-size", |ui| {
                let (w0, h0) = (d.w, d.h);
                let min = if s.kind.is_line() { 0.0 } else { 0.05 };
                ui.label(tr("shape.height"));
                let mut v = d.h * cm;
                if ui.add(egui::DragValue::new(&mut v).range(min..=500.0).speed(0.05).max_decimals(2).suffix(" cm")).changed() {
                    d.h = v / cm;
                    if d.lock && h0 > 0.0 {
                        d.w = w0 * d.h / h0;
                    }
                }
                ui.end_row();
                ui.label(tr("shape.width"));
                let mut v = d.w * cm;
                if ui.add(egui::DragValue::new(&mut v).range(min..=500.0).speed(0.05).max_decimals(2).suffix(" cm")).changed() {
                    d.w = v / cm;
                    if d.lock && w0 > 0.0 {
                        d.h = h0 * d.w / w0;
                    }
                }
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut d.lock, tr("image.lock_aspect"));
                ui.end_row();
                ui.label(tr("shape.rotation"));
                let mut r = s.rotation;
                if ui.add(egui::DragValue::new(&mut r).range(-360.0..=360.0).speed(1.0).max_decimals(1).suffix("°")).changed() {
                    s.rotation = r.rem_euclid(360.0);
                }
                ui.end_row();
                ui.label(tr("shape.flip"));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.flip_h, tr("shape.flip_h"));
                    ui.checkbox(&mut s.flip_v, tr("shape.flip_v"));
                });
                ui.end_row();
            });
        }
        2 => text_tab(ui, s),
        3 => extra(ui),
        _ => {
            crate::widgets::form(ui, "shape-alt", |ui| {
                ui.label(tr("image.name"));
                ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(280.0));
                ui.end_row();
                ui.label(tr("image.alt"));
                let id = ui.add(egui::TextEdit::multiline(&mut s.alt).desired_rows(3).desired_width(280.0).hint_text(tr("image.alt_hint"))).id;
                ui.data_mut(|m| m.insert_temp(id.with("multiline"), true));
                ui.end_row();
            });
        }
    }
}

/// The Text tab: the text itself and its formatting (for all of it).
fn text_tab(ui: &mut egui::Ui, s: &mut Shape) {
    let mut text = s.plain_text();
    let id = ui.add(egui::TextEdit::multiline(&mut text).desired_rows(4).desired_width(f32::INFINITY).hint_text(tr("shape.text_hint"))).id;
    ui.data_mut(|m| m.insert_temp(id.with("multiline"), true));
    s.set_plain_text(&text);
    ui.add_space(10.0);
    let cur = s.text_format();
    crate::widgets::form(ui, "shape-text", |ui| {
        ui.label(tr("shape.font"));
        let name = cur.font.as_deref().unwrap_or("").to_string();
        let shown = if name.is_empty() { tr("shape.font_default").to_string() } else { name.clone() };
        if let Some(f) = crate::common::font_picker(ui, egui::Id::new("shape-font"), &shown, 180.0) {
            let f: Option<Arc<str>> = Some(Arc::from(f.as_str()));
            s.format_text(&|r| r.font = f.clone());
        }
        ui.end_row();
        ui.label(tr("shape.font_size"));
        let mut size = cur.size.unwrap_or(apex_doc::SHAPE_TEXT_PT);
        if ui.add(egui::DragValue::new(&mut size).range(1.0..=400.0).speed(0.5).max_decimals(1).suffix(" pt")).changed() {
            s.format_text(&|r| r.size = Some(size));
        }
        ui.end_row();
        ui.label(tr("shape.style"));
        ui.horizontal(|ui| {
            let (mut b, mut i, mut u) = (cur.bold, cur.italic, cur.underline);
            if ui.checkbox(&mut b, tr("format.bold")).changed() {
                s.format_text(&|r| r.bold = b);
            }
            if ui.checkbox(&mut i, tr("format.italic")).changed() {
                s.format_text(&|r| r.italic = i);
            }
            if ui.checkbox(&mut u, tr("format.underline")).changed() {
                s.format_text(&|r| r.underline = u);
            }
        });
        ui.end_row();
        let auto = auto_text_color(s);
        if let Some(c) = color_row(ui, &tr("shape.text_color"), Some(cur.color.unwrap_or(auto)), "color.automatic") {
            s.format_text(&|r| r.color = c);
        }
        ui.label(tr("shape.align"));
        ui.horizontal(|ui| {
            let mut a = s.text_align();
            for (v, key) in [(Align::Left, "pic.left"), (Align::Center, "pic.center"), (Align::Right, "pic.right"), (Align::Justify, "shape.justify")] {
                ui.radio_value(&mut a, v, tr(key));
            }
            if a != s.text_align() {
                s.format_text(&|_| {});
                for p in &mut s.text {
                    p.align = a;
                }
            }
        });
        ui.end_row();
        ui.label(tr("shape.valign"));
        ui.horizontal(|ui| {
            for (v, key) in [(VAnchor::Top, "shape.top"), (VAnchor::Middle, "shape.middle"), (VAnchor::Bottom, "shape.bottom")] {
                ui.radio_value(&mut s.valign, v, tr(key));
            }
        });
        ui.end_row();
    });
}

// --- text editing ----------------------------------------------------------------------

/// Text being typed into a shape.
pub struct TextEdit {
    /// Turned and vertical text is typed as plain text.
    pub text: String,
    /// First frame: takes the focus.
    fresh: bool,
    /// Horizontal text is typed with its formatting.
    pub rich: crate::shape_text::RichEdit,
}

impl TextEdit {
    /// Starts editing `shape`'s text; `typed`: text typed to start it.
    pub fn new(shape: &Shape, typed: Option<&str>) -> TextEdit {
        let mut text = shape.plain_text();
        if let Some(t) = typed {
            text.push_str(t);
        }
        TextEdit { text, fresh: true, rich: crate::shape_text::RichEdit::new(&shape.text, typed) }
    }
}

pub enum EditResult {
    Editing,
    /// Done (Escape or a click elsewhere): keep the text.
    Done,
}

/// The editor over a shape drawn in `rect` (screen, unturned). `shape`
/// gets the text as typed.
pub fn text_editor(ui: &mut egui::Ui, id: egui::Id, rect: Rect, shape: &mut Shape, e: &mut TextEdit, scale: f32) -> EditResult {
    let area = text_rect(shape, rect, scale);
    if crate::vtext::flow(shape) != crate::vtext::Flow::Horizontal {
        // Turned and vertical text: the caret runs along the lines.
        let first = shape.text_format();
        let painter = ui.painter().clone();
        let layout = |t: &str| {
            let para = ShapePara { align: shape.text_align(), runs: vec![first.with_text(t)], list: None };
            layout_para(&painter, shape, &para, area.height(), scale)
        };
        let done = crate::vtext::editor(ui, id, area, shape, &mut e.text, &mut e.fresh, layout);
        shape.set_plain_text(&e.text);
        return if done { EditResult::Done } else { EditResult::Editing };
    }
    if crate::shape_text::editor(ui, id, rect, shape, &mut e.rich, scale) { EditResult::Done } else { EditResult::Editing }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_fit_their_box() {
        for k in ShapeKind::ALL {
            let pts = outline(k, 200.0, 100.0);
            assert!(pts.len() >= 2, "{k:?}");
            for p in &pts {
                // The callout's pointer is out of its box.
                if k != ShapeKind::Callout {
                    assert!(p.x >= -1e-3 && p.x <= 200.001 && p.y >= -1e-3 && p.y <= 100.001, "{k:?} {p:?}");
                }
            }
            if !k.is_line() {
                let t = triangulate(&pts);
                assert_eq!(t.len(), (pts.len() - 2) * 3, "{k:?}");
                // The triangles cover the outline's area.
                let area = |a: Pos2, b: Pos2, c: Pos2| ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.0;
                let tri: f32 = t.chunks(3).map(|c| area(pts[c[0] as usize], pts[c[1] as usize], pts[c[2] as usize])).sum();
                let n = pts.len();
                let poly = (0..n).map(|i| pts[i].x * pts[(i + 1) % n].y - pts[(i + 1) % n].x * pts[i].y).sum::<f32>().abs() / 2.0;
                assert!((tri - poly).abs() < poly * 0.01 + 1.0, "{k:?}: {tri} vs {poly}");
            }
        }
        let star = outline(ShapeKind::Star5, 100.0, 100.0);
        assert!(star.iter().any(|p| p.y.abs() < 1e-3) && star.iter().any(|p| (p.y - 100.0).abs() < 0.01));
        let c = outline(ShapeKind::Callout, 120.0, 60.0);
        assert!(c.iter().any(|p| p.y > 60.0), "the pointer goes below");
    }

    #[test]
    fn office_presets() {
        let mut n = 0;
        for (_, items) in groups() {
            for t in items {
                let s = t.shape();
                if let Some(p) = t.preset {
                    assert!(apex_doc::is_preset(p), "{p}");
                    assert!(s.geometry().is_some(), "{p}");
                    n += 1;
                }
            }
        }
        assert!(n > 150, "{n}");
        assert_eq!(spoken("flowChartPunchedTape"), "Flow chart punched tape");
        assert_eq!(spoken("star12"), "Star 12");
        assert!(Template::of("leftBrace").shape().fill.is_none() && Template::of("heart").shape().fill.is_some());
        // A donut: its hole is bridged in and the ring is what gets filled.
        let mut s = Shape::new(ShapeKind::Ellipse);
        s.preset = Some(Arc::from("donut"));
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 100.0));
        let paths = preset_paths(&s, rect).unwrap();
        let polys = merge_holes(&paths[0].0);
        assert_eq!(polys.len(), 1);
        let pts = &polys[0];
        let t = triangulate(pts);
        let area: f32 = t.chunks(3).map(|c| area2(&[pts[c[0] as usize], pts[c[1] as usize], pts[c[2] as usize]]).abs() / 2.0).sum();
        // Outer radius 50, inner 50 - 25 (the default thickness).
        let ring = std::f32::consts::PI * (50.0f32.powi(2) - 25.0f32.powi(2));
        assert!((area - ring).abs() / ring < 0.03, "{area} {ring}");
        // Text goes in the preset's own rectangle.
        let tr = text_rect(&Shape { preset: Some(Arc::from("flowChartPunchedTape")), ..Shape::new(ShapeKind::Rect) }, rect, 1.0);
        assert!(tr.min.y > rect.min.y + 10.0 && tr.max.y < rect.max.y - 10.0, "{tr:?}");
    }

    #[test]
    fn drawn_geometry_is_painted() {
        let mut s = Shape::new(ShapeKind::Rect);
        s.custom = Some(Arc::from(r#"<a:custGeom><a:pathLst><a:path w="10" h="10"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="10" y="0"/></a:lnTo><a:lnTo><a:pt x="0" y="10"/></a:lnTo><a:close/></a:path></a:pathLst></a:custGeom>"#));
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 50.0));
        let paths = preset_paths(&s, rect).expect("drawn");
        let (pts, closed) = &paths[0].0[0];
        assert!(*closed && pts.len() == 3);
        assert_eq!(pts[1], pos2(110.0, 10.0));
        s.flip_h = true;
        assert_eq!(preset_paths(&s, rect).unwrap()[0].0[0].0[1], pos2(10.0, 10.0));
    }

    #[test]
    fn dragging_boxes() {
        let b = BoxGeo { x: 10.0, y: 10.0, w: 100.0, h: 50.0, rotation: 0.0, flip_h: false, flip_v: false };
        let r = dragged(b, Handle::Resize(1, 0), vec2(20.0, 5.0), false, 1.0);
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 10.0, 120.0, 50.0));
        let r = dragged(b, Handle::Resize(-1, -1), vec2(-20.0, 0.0), true, 1.0);
        assert!((r.w - 120.0).abs() < 1e-3 && (r.h - 60.0).abs() < 1e-3 && (r.x + r.w - 110.0).abs() < 1e-3);
        let m = dragged(b, Handle::Move, vec2(5.0, 30.0), true, 1.0);
        assert_eq!((m.x, m.y), (10.0, 40.0));
        // Lines: the other end stays; dragging past it flips the line.
        let l = BoxGeo::line(pos2(0.0, 0.0), pos2(100.0, 0.0));
        let r = dragged(l, Handle::End, vec2(-150.0, 20.0), false, 0.0);
        assert!(r.flip_h && !r.flip_v);
        let (a, b2) = r.ends();
        assert!((a - pos2(0.0, 0.0)).length() < 1e-3 && (b2 - pos2(-50.0, 20.0)).length() < 1e-3);
        let s = drawn(ShapeKind::Line, pos2(0.0, 0.0), pos2(100.0, 8.0), true);
        assert_eq!(s.h, 0.0);
        let s = drawn(ShapeKind::Ellipse, pos2(50.0, 50.0), pos2(10.0, 30.0), true);
        assert_eq!((s.x, s.y, s.w, s.h), (10.0, 10.0, 40.0, 40.0));
        assert!((rotation_to(pos2(0.0, 0.0), pos2(10.0, 0.0), false) - 90.0).abs() < 1e-3);
    }

    #[test]
    fn hits_and_colors() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 40.0));
        let mut s = Shape::new(ShapeKind::Rect);
        assert!(hit(&s, r, pos2(50.0, 20.0)) && !hit(&s, r, pos2(150.0, 20.0)));
        s.rotation = 90.0;
        assert!(hit(&s, r, pos2(50.0, -20.0)) && !hit(&s, r, pos2(0.0, 20.0)));
        let l = Shape::new(ShapeKind::Line);
        assert!(hit(&l, r, pos2(50.0, 21.0)) && !hit(&l, r, pos2(90.0, 5.0)));
        assert_eq!(auto_text_color(&Shape::new(ShapeKind::Rect)), 0xFFFFFF);
        assert_eq!(auto_text_color(&Shape::new(ShapeKind::TextBox)), 0x000000);
        assert_eq!(handles(&l, r).len(), 2);
        assert_eq!(handles(&s, r).len(), 9);
    }
}
