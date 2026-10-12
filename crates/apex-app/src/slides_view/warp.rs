//! Pictures painted and taken back, to be shown again in pieces (dissolve,
//! blinds) or bent in 3D (cube, flip, doors): the transitions and effects
//! a plain painter can't do.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, epaint, layers::ShapeIdx, pos2};
use epaint::{ClippedShape, Mesh, Primitive, Vertex};

/// What `paint` paints, taken off the painter's layer (to be shown with
/// `replay` or `warp`).
pub fn capture(p: &Painter, paint: impl FnOnce(&Painter)) -> Vec<ClippedShape> {
    let layer = p.layer_id();
    let start = p.ctx().graphics_mut(|g| g.entry(layer).next_idx().0);
    paint(p);
    p.ctx().graphics_mut(|g| {
        let l = g.entry(layer);
        let end = l.next_idx().0;
        let out: Vec<ClippedShape> = l.all_entries().skip(start).cloned().collect();
        for i in start..end {
            l.reset_shape(ShapeIdx(i));
        }
        out
    })
}

/// Paints `shapes` again, shown only within `clip`.
pub fn replay(p: &Painter, shapes: &[ClippedShape], clip: Rect) {
    let clip = clip.intersect(p.clip_rect());
    if !clip.is_positive() {
        return;
    }
    let layer = p.layer_id();
    p.ctx().graphics_mut(|g| {
        let l = g.entry(layer);
        for s in shapes {
            let c = s.clip_rect.intersect(clip);
            if c.is_positive() {
                l.add(c, s.shape.clone());
            }
        }
    });
}

/// A picture turned in space: about `pivot` (from the view's center, z
/// going away) by `roll` (in its plane), `pitch` (positive: its bottom
/// goes away) and `yaw` (positive: its left side goes away), then moved
/// by `off`. Radians and points.
#[derive(Clone, Copy, Debug, Default)]
pub struct Turn {
    pub pivot: [f32; 3],
    pub roll: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub off: [f32; 3],
}

impl Turn {
    /// Where a point of the flat picture goes, seen from `dist` in front
    /// of `center`.
    pub fn see(&self, center: Pos2, dist: f32, q: Pos2) -> Pos2 {
        let [px, py, pz] = self.pivot;
        let (mut x, mut y, mut z) = (q.x - center.x - px, q.y - center.y - py, -pz);
        let (s, c) = self.roll.sin_cos();
        (x, y) = (x * c - y * s, x * s + y * c);
        let (s, c) = self.pitch.sin_cos();
        (y, z) = (y * c - z * s, y * s + z * c);
        let (s, c) = self.yaw.sin_cos();
        (x, z) = (x * c - z * s, x * s + z * c);
        let (x, y, z) = (x + px + self.off[0], y + py + self.off[1], z + pz + self.off[2]);
        let k = dist / (dist + z).max(dist * 0.05);
        pos2(center.x + x * k, center.y + y * k)
    }

    /// How much the picture faces the viewer: 1 straight on, 0 edge on,
    /// below 0 its back.
    pub fn facing(&self) -> f32 {
        self.yaw.cos() * self.pitch.cos()
    }
}

/// Paints `shapes` with every point moved by `f`, the colors darkened by
/// `shade` and faded by `alpha` (1: as they are).
pub fn warp(p: &Painter, shapes: Vec<ClippedShape>, f: &dyn Fn(Pos2) -> Pos2, shade: f32, alpha: f32) {
    if shapes.is_empty() || alpha <= 0.0 {
        return;
    }
    let ctx = p.ctx();
    let prims = ctx.tessellate(shapes, ctx.pixels_per_point());
    let (shade, alpha) = (shade.clamp(0.0, 1.0), alpha.clamp(0.0, 1.0));
    for prim in prims {
        let Primitive::Mesh(m) = prim.primitive else { continue };
        let mut out = Mesh::with_texture(m.texture_id);
        for t in m.indices.chunks_exact(3) {
            let tri = [m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]];
            let poly = clip_poly(&tri, prim.clip_rect);
            for i in 1..poly.len().saturating_sub(1) {
                split(&mut out, [poly[0], poly[i], poly[i + 1]], f, shade, alpha, 0);
            }
        }
        if !out.is_empty() {
            p.add(Shape::mesh(out));
        }
    }
}

fn lerp(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    let c = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    let (ca, cb) = (a.color, b.color);
    Vertex {
        pos: a.pos + (b.pos - a.pos) * t,
        uv: a.uv + (b.uv - a.uv) * t,
        color: Color32::from_rgba_premultiplied(c(ca.r(), cb.r()), c(ca.g(), cb.g()), c(ca.b(), cb.b()), c(ca.a(), cb.a())),
    }
}

/// The part of a triangle within `r` (Sutherland–Hodgman).
fn clip_poly(tri: &[Vertex; 3], r: Rect) -> Vec<Vertex> {
    let mut poly = tri.to_vec();
    let edges: [(fn(&Vertex) -> f32, f32, bool); 4] = [(|v| v.pos.x, r.min.x, true), (|v| v.pos.x, r.max.x, false), (|v| v.pos.y, r.min.y, true), (|v| v.pos.y, r.max.y, false)];
    for (get, lim, low) in edges {
        if poly.is_empty() {
            break;
        }
        let inside = |v: &Vertex| if low { get(v) >= lim } else { get(v) <= lim };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (a, b) = (&poly[i], &poly[(i + 1) % poly.len()]);
            let (ia, ib) = (inside(a), inside(b));
            if ia {
                out.push(*a);
            }
            if ia != ib {
                let t = (lim - get(a)) / (get(b) - get(a));
                out.push(lerp(a, b, t));
            }
        }
        poly = out;
    }
    poly
}

/// Adds a triangle moved by `f`, cut smaller first when large (a bent
/// picture isn't straight between far corners).
fn split(m: &mut Mesh, t: [Vertex; 3], f: &dyn Fn(Pos2) -> Pos2, shade: f32, alpha: f32, depth: u32) {
    let long = (0..3).map(|i| t[i].pos.distance_sq(t[(i + 1) % 3].pos)).fold(0.0, f32::max);
    if depth < 5 && long > 40.0 * 40.0 {
        let (ab, bc, ca) = (lerp(&t[0], &t[1], 0.5), lerp(&t[1], &t[2], 0.5), lerp(&t[2], &t[0], 0.5));
        for q in [[t[0], ab, ca], [ab, t[1], bc], [ca, bc, t[2]], [ab, bc, ca]] {
            split(m, q, f, shade, alpha, depth + 1);
        }
        return;
    }
    let base = m.vertices.len() as u32;
    for v in t {
        let c = v.color;
        let k = |x: u8| (x as f32 * shade * alpha).round() as u8;
        m.vertices.push(Vertex { pos: f(v.pos), uv: v.uv, color: Color32::from_rgba_premultiplied(k(c.r()), k(c.g()), k(c.b()), (c.a() as f32 * alpha).round() as u8) });
    }
    m.indices.extend([base, base + 1, base + 2]);
}

/// A number from 0 to 1 that looks random, the same for the same `i`.
pub fn noise(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x % 10_000) as f32 / 10_000.0
}
