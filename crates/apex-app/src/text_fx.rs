//! Artistic text (see `apex_doc::TextFx`) drawn from a laid out galley:
//! the fill recolors the letters' own meshes, while the outline, shadow,
//! glow and bevel are copies of the galley whose letters take the effect's
//! color, painted behind it a little moved. Being plain text shapes, the
//! copies print and go to PDF too. The reflection is a mirrored mesh of
//! the letters, on screen only.

use std::sync::Arc;

use apex_doc::{TextFill, TextFx};
use eframe::egui::{self, Color32, Galley, Mesh, Painter, Pos2, Rect, Vec2, pos2, vec2};

use crate::gradient::{color32, color_at};

/// Galley characters `from..to` have effects `fx`.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub from: usize,
    pub to: usize,
    pub fx: Arc<TextFx>,
}

/// What is painted around a galley with artistic text.
#[derive(Clone, Default)]
pub struct Layers {
    /// Copies of the galley painted behind it, each moved by its offset.
    pub back: Vec<(Vec2, Arc<Galley>)>,
    /// The mirrored letters (galley coordinates, texel uvs).
    pub mirror: Option<Arc<Mesh>>,
}

impl Layers {
    pub fn is_empty(&self) -> bool {
        self.back.is_empty() && self.mirror.is_none()
    }
}

/// A glyph: its row, index in the row, galley char index and vertices.
struct Glyph {
    row: usize,
    ci: usize,
    verts: std::ops::Range<usize>,
}

fn glyphs(g: &Galley) -> Vec<Glyph> {
    let mut out = Vec::new();
    let mut ci = 0usize;
    for (ri, prow) in g.rows.iter().enumerate() {
        let row = &prow.row;
        let end = row.visuals.glyph_vertex_range.end;
        for (j, gl) in row.glyphs.iter().enumerate() {
            let v0 = gl.first_vertex as usize;
            let v1 = row.glyphs.get(j + 1).map_or(end, |x| x.first_vertex as usize).clamp(v0, end.max(v0));
            out.push(Glyph { row: ri, ci: ci + j, verts: v0..v1 });
        }
        ci += row.glyphs.len() + usize::from(prow.ends_with_newline);
    }
    out
}

/// The same effects make one group (runs split by other formatting).
fn groups(runs: &[Run]) -> Vec<(Arc<TextFx>, Vec<(usize, usize)>)> {
    let mut out: Vec<(Arc<TextFx>, Vec<(usize, usize)>)> = Vec::new();
    for r in runs.iter().filter(|r| r.to > r.from && !r.fx.is_empty()) {
        match out.iter_mut().find(|(fx, _)| **fx == *r.fx) {
            Some((_, spans)) => spans.push((r.from, r.to)),
            None => out.push((r.fx.clone(), vec![(r.from, r.to)])),
        }
    }
    out
}

/// A copy of the effect's color stamped behind the letters.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stamp {
    off: Vec2,
    color: Color32,
}

/// Opacity of each of `n` overlapping copies that together show `a`.
fn share(a: u8, n: u32) -> u8 {
    let a = f32::from(a) / 255.0;
    ((1.0 - (1.0 - a).powf(1.0 / n as f32)) * 255.0).round().clamp(1.0, 255.0) as u8
}

fn rgba(c: u32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied((c >> 16) as u8, (c >> 8) as u8, c as u8, a)
}

/// Offsets around a circle of radius `r`.
fn ring(r: f32, n: usize) -> impl Iterator<Item = Vec2> {
    (0..n).map(move |i| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        vec2(a.cos(), a.sin()) * r
    })
}

/// The copies behind the letters, back to front; `px` is pixels per point.
fn stamps(fx: &TextFx, px: f32) -> Vec<Stamp> {
    let mut out = Vec::new();
    if let Some(s) = fx.shadow {
        let (dx, dy) = s.offset_pt();
        let off = vec2(dx, dy) * px;
        let blur = s.blur_pt() * px;
        if blur < 0.75 {
            out.push(Stamp { off, color: rgba(s.color, s.alpha) });
        } else {
            let a = share(s.alpha, 3);
            out.push(Stamp { off, color: rgba(s.color, a) });
            out.extend(ring(blur * 0.5, 6).map(|d| Stamp { off: off + d, color: rgba(s.color, a / 2 + 1) }));
        }
    }
    if let Some(g) = fx.glow {
        let r = (g.radius_pt() * px).max(1.0);
        let n = if r > 6.0 { 12 } else { 8 };
        out.extend(ring(r, n).map(|d| Stamp { off: d, color: rgba(g.color, share(g.alpha, 4)) }));
        out.extend(ring(r * 0.5, 8).map(|d| Stamp { off: d, color: rgba(g.color, share(g.alpha, 3)) }));
    }
    if fx.bevel {
        let k = (px * 0.6).clamp(0.75, 3.0);
        out.push(Stamp { off: vec2(k, k), color: Color32::from_rgba_unmultiplied(0, 0, 0, 110) });
        out.push(Stamp { off: vec2(-k, -k) * 0.7, color: Color32::from_rgba_unmultiplied(255, 255, 255, 170) });
    }
    if let Some(o) = fx.outline {
        let r = (o.width_pt() * px / 2.0).max(0.5);
        out.extend(ring(r, if r > 2.0 { 12 } else { 8 }).map(|d| Stamp { off: d, color: rgba(o.color, o.alpha) }));
    }
    out
}

/// Galley coordinates of every vertex of the group's letters.
fn bounds(g: &Galley, gl: &[Glyph], inside: &dyn Fn(usize) -> bool) -> Rect {
    let mut b = Rect::NOTHING;
    for x in gl.iter().filter(|x| inside(x.ci)) {
        let prow = &g.rows[x.row];
        for v in &prow.row.visuals.mesh.vertices[x.verts.clone()] {
            b.extend_with(v.pos + prow.pos.to_vec2());
        }
    }
    b
}

/// A copy of `g` showing only the letters `inside`, all in `color`, with
/// no backgrounds or lines (they stay with the text itself).
fn stamp_copy(g: &Galley, gl: &[Glyph], inside: &dyn Fn(usize) -> bool, color: Color32) -> Arc<Galley> {
    let mut c = g.clone();
    for prow in &mut c.rows {
        let row = Arc::make_mut(&mut prow.row);
        for v in &mut row.visuals.mesh.vertices {
            v.color = Color32::TRANSPARENT;
        }
    }
    for x in gl.iter().filter(|x| inside(x.ci)) {
        let row = Arc::make_mut(&mut c.rows[x.row].row);
        for v in &mut row.visuals.mesh.vertices[x.verts.clone()] {
            v.color = color;
        }
    }
    let job = Arc::make_mut(&mut c.job);
    for s in &mut job.sections {
        s.format.background = Color32::TRANSPARENT;
        s.format.underline = egui::Stroke::NONE;
        s.format.strikethrough = egui::Stroke::NONE;
    }
    Arc::new(c)
}

/// The fill's color at galley point `p` of a group spanning `b`; `None`
/// keeps the letter's own. Fills with no color show `bg` (the letters
/// look hollow inside their outline).
fn fill_at(fx: &TextFx, b: Rect, p: Pos2, own: Color32, bg: Color32) -> Option<Color32> {
    let (u, v) = ((p.x - b.min.x) / b.width().max(1e-3), (p.y - b.min.y) / b.height().max(1e-3));
    let base = match &fx.fill {
        TextFill::Auto => own,
        TextFill::None => return Some(if fx.outline.is_some() { bg } else { Color32::TRANSPARENT }),
        TextFill::Solid(c, a) => rgba(*c, *a),
        TextFill::Gradient(g) => color_at(g, u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)),
    };
    if !fx.bevel {
        return (base != own).then_some(base);
    }
    // Lit from the top: lighter above, darker below.
    let k = 0.35 - v.clamp(0.0, 1.0) * 0.6;
    let [r, g, bl, a] = base.to_srgba_unmultiplied();
    let c = apex_doc::gradient::tint(u32::from_be_bytes([0, r, g, bl]), k);
    Some(color32(c, f32::from(a) / 255.0))
}

/// Applies `runs` to `galley`: the letters take their fill, and the
/// copies and reflection around them come back. `px` is pixels per point
/// (for the effects' sizes), `bg` what shows through hollow letters.
pub fn apply(galley: Arc<Galley>, runs: &[Run], px: f32, bg: Color32) -> (Arc<Galley>, Layers) {
    let groups = groups(runs);
    if groups.is_empty() {
        return (galley, Layers::default());
    }
    let gl = glyphs(&galley);
    let mut layers = Layers::default();
    let mut front = (*galley).clone();
    let mut mirror = Mesh::default();
    for (fx, spans) in &groups {
        let inside = |ci: usize| spans.iter().any(|&(a, b)| a <= ci && ci < b);
        let b = bounds(&galley, &gl, &inside);
        if !b.is_positive() {
            continue;
        }
        for s in stamps(fx, px) {
            layers.back.push((s.off, stamp_copy(&galley, &gl, &inside, s.color)));
        }
        for x in gl.iter().filter(|x| inside(x.ci)) {
            let prow = &mut front.rows[x.row];
            let at = prow.pos.to_vec2();
            let row = Arc::make_mut(&mut prow.row);
            for v in &mut row.visuals.mesh.vertices[x.verts.clone()] {
                if let Some(c) = fill_at(fx, b, v.pos + at, v.color, bg) {
                    v.color = c;
                }
            }
        }
        if let Some(r) = fx.reflection {
            // Mirrored under the letters, fading out over `size` percent
            // of their height.
            let axis = b.max.y + r.dist_pt() * px;
            let fade = (b.height() * f32::from(r.size) / 100.0).max(1.0);
            for x in gl.iter().filter(|x| inside(x.ci)) {
                let prow = &front.rows[x.row];
                let m = &prow.row.visuals.mesh;
                let at = prow.pos.to_vec2();
                // Glyph quads: four vertices, two triangles each.
                let first = mirror.vertices.len() as u32;
                for v in &m.vertices[x.verts.clone()] {
                    let p = v.pos + at;
                    let y = axis + (b.max.y - p.y);
                    let k = (1.0 - (y - axis) / fade).clamp(0.0, 1.0) * f32::from(r.alpha) / 255.0;
                    mirror.vertices.push(egui::epaint::Vertex { pos: pos2(p.x, y), uv: v.uv, color: v.color.gamma_multiply(k) });
                }
                for q in 0..x.verts.len() as u32 / 4 {
                    let i = first + q * 4;
                    mirror.indices.extend_from_slice(&[i, i + 1, i + 2, i + 2, i + 1, i + 3]);
                }
                mirror.texture_id = m.texture_id;
            }
        }
    }
    for prow in &mut front.rows {
        let row = Arc::make_mut(&mut prow.row);
        row.visuals.mesh_bounds = row.visuals.mesh.calc_bounds();
    }
    if !mirror.is_empty() {
        layers.mirror = Some(Arc::new(mirror));
    }
    (Arc::new(front), layers)
}

/// Paints the layers of a galley painted at `pos` (before the galley).
pub fn paint(p: &Painter, pos: Pos2, layers: &Layers) {
    for (off, g) in &layers.back {
        p.add(egui::epaint::TextShape::new(pos + *off, g.clone(), Color32::TRANSPARENT));
    }
    if let Some(m) = &layers.mirror {
        let size = p.ctx().fonts(|f| f.font_image_size());
        let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
        let mut m = (**m).clone();
        for v in &mut m.vertices {
            v.pos += pos.to_vec2();
            v.uv = pos2(v.uv.x / w, v.uv.y / h);
        }
        p.add(egui::Shape::mesh(m));
    }
}

/// Paints the copies of a galley turned by `angle` about `pos` (no
/// reflection).
pub fn paint_turned(p: &Painter, pos: Pos2, angle: f32, layers: &Layers) {
    let rot = egui::emath::Rot2::from_angle(angle);
    for (off, g) in &layers.back {
        p.add(egui::epaint::TextShape::new(pos + rot * *off, g.clone(), Color32::TRANSPARENT).with_angle(angle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::text::{LayoutJob, TextFormat};

    fn galley(ctx: &egui::Context, text: &str) -> Arc<Galley> {
        let mut job = LayoutJob::default();
        job.append(text, 0.0, TextFormat { font_id: egui::FontId::proportional(30.0), color: Color32::BLACK, ..Default::default() });
        ctx.fonts_mut(|f| f.layout_job(job))
    }

    #[test]
    fn fills_and_copies() {
        let ctx = egui::Context::default();
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        let g = galley(&ctx, "Hola mundo");
        frame();
        let acc = [0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47];
        // Every gallery style changes the letters or adds something.
        for i in 0..apex_doc::text_fx::STYLES {
            let fx = Arc::new(apex_doc::text_fx::style(i, 0x000000, &acc));
            let (front, layers) = apply(g.clone(), &[Run { from: 0, to: 4, fx: fx.clone() }], 1.33, Color32::WHITE);
            assert!(!layers.back.is_empty() || layers.mirror.is_some() || front.rows[0].row.visuals.mesh != g.rows[0].row.visuals.mesh, "style {i}");
            assert_eq!(layers.mirror.is_some(), fx.reflection.is_some(), "style {i}");
        }
        // A solid fill recolors "Hola" only; the outline's copies show
        // only those letters in its color.
        let fx = TextFx { fill: TextFill::Solid(0xFF0000, 255), outline: Some(apex_doc::text_fx::TextOutline::new(0x0000FF, 2.0)), ..Default::default() };
        let (front, layers) = apply(g.clone(), &[Run { from: 0, to: 4, fx: Arc::new(fx) }], 1.0, Color32::WHITE);
        let gl = glyphs(&front);
        let mesh = &front.rows[0].row.visuals.mesh;
        let color_of = |ci: usize| gl.iter().find(|x| x.ci == ci && !x.verts.is_empty()).map(|x| mesh.vertices[x.verts.start].color);
        assert_eq!(color_of(0), Some(Color32::RED));
        assert_eq!(color_of(6), Some(Color32::BLACK));
        assert_eq!(layers.back.len(), 8);
        let copy = &layers.back[0].1.rows[0].row.visuals.mesh;
        let at = |ci: usize| gl.iter().find(|x| x.ci == ci && !x.verts.is_empty()).map(|x| copy.vertices[x.verts.start].color);
        assert_eq!(at(1), Some(Color32::BLUE));
        assert_eq!(at(7), Some(Color32::TRANSPARENT));
    }
}
