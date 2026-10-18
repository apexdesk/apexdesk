//! Painting gradient fills, shared by documents, sheets and slides (shapes,
//! text boxes, slide and page backgrounds): the polygons are cut into
//! triangles small enough that each corner taking the gradient's color at
//! its place shows the stops. Screen, printing and PDF (which fills each
//! triangle) all draw these meshes.

use apex_doc::Gradient;
use eframe::egui::{self, Color32, Mesh, Painter, Pos2, Rect, pos2};

/// A color with an opacity (0..1).
pub fn color32(c: u32, alpha: f32) -> Color32 {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied((c >> 16) as u8, (c >> 8) as u8, c as u8, a)
}

/// The gradient's color at point (u, v) of its box (0..1 each).
pub fn color_at(g: &Gradient, u: f32, v: f32) -> Color32 {
    let (c, a) = g.at(g.t_at(u, v));
    color32(c, a)
}

/// Pieces per side of a triangle whose longest side is `len` in a box
/// whose longest side is `size`.
fn pieces(len: f32, size: f32) -> u32 {
    if !(len.is_finite() && size.is_finite()) || size <= 0.0 {
        return 1;
    }
    ((len / size * 20.0).ceil() as u32).clamp(1, 24)
}

/// A mesh filling `polys` (simple polygons in the box's own coordinates,
/// `w` by `h` from its top left) with `g`; `place` puts a point of the box
/// on the screen (turned, flipped), `tweak` changes the colors (shading).
pub fn mesh(g: &Gradient, polys: &[Vec<Pos2>], w: f32, h: f32, place: &dyn Fn(Pos2) -> Pos2, tweak: &dyn Fn(Color32) -> Color32) -> Mesh {
    let mut mesh = Mesh::default();
    let (w, h) = (w.max(1e-3), h.max(1e-3));
    let size = w.max(h);
    for poly in polys {
        let idx = crate::shapes::triangulate(poly);
        // One count for the whole polygon: triangles sharing a side cut it
        // at the same points (no cracks between them).
        let longest = idx.chunks_exact(3).fold(0.0f32, |m, t| {
            let [a, b, c] = [poly[t[0] as usize], poly[t[1] as usize], poly[t[2] as usize]];
            m.max(a.distance(b)).max(b.distance(c)).max(c.distance(a))
        });
        // At most some 6000 pieces a polygon (PDF files fill each).
        let tris = (idx.len() / 3).max(1) as f32;
        let n = pieces(longest, size).min(((6000.0 / tris).sqrt() as u32).max(1));
        for t in idx.chunks_exact(3) {
            let [a, b, c] = [poly[t[0] as usize], poly[t[1] as usize], poly[t[2] as usize]];
            // Points a + (b - a) * i / n + (c - a) * j / n, row by row.
            let base = mesh.vertices.len() as u32;
            for j in 0..=n {
                for i in 0..=(n - j) {
                    let q = a + (b - a) * (i as f32 / n as f32) + (c - a) * (j as f32 / n as f32);
                    mesh.colored_vertex(place(q), tweak(color_at(g, q.x / w, q.y / h)));
                }
            }
            // First index of row j: the rows before hold n + 1, n, ... points.
            let row = |j: u32| -> u32 { (0..j).map(|r| n + 1 - r).sum() };
            for j in 0..n {
                let (r0, r1) = (base + row(j), base + row(j + 1));
                for i in 0..(n - j) {
                    mesh.add_triangle(r0 + i, r0 + i + 1, r1 + i);
                    if i + 1 < n - j {
                        mesh.add_triangle(r0 + i + 1, r1 + i + 1, r1 + i);
                    }
                }
            }
        }
    }
    mesh
}

/// Fills `rect` (not turned) with `g`.
pub fn paint_rect(p: &Painter, g: &Gradient, rect: Rect) {
    let (w, h) = (rect.width(), rect.height());
    let poly = vec![pos2(0.0, 0.0), pos2(w, 0.0), pos2(w, h), pos2(0.0, h)];
    p.add(egui::Shape::mesh(mesh(g, &[poly], w, h, &|q| rect.min + q.to_vec2(), &|c| c)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::GradKind;

    #[test]
    fn meshes_follow_the_stops() {
        let g = Gradient::two(0x000000, 0xFFFFFF, 0.0);
        let poly = vec![pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(100.0, 50.0), pos2(0.0, 50.0)];
        let m = mesh(&g, &[poly], 100.0, 50.0, &|q| q, &|c| c);
        assert!(m.is_valid());
        assert!(m.vertices.len() > 100);
        for v in &m.vertices {
            let want = (v.pos.x / 100.0 * 255.0).round();
            assert!((v.color.r() as f32 - want).abs() <= 1.5, "{v:?}");
        }
        // A triangle: every piece inside it.
        let tri = vec![pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(0.0, 10.0)];
        let r = Gradient { kind: GradKind::Radial, ..g };
        let m = mesh(&r, &[tri], 10.0, 10.0, &|q| q, &|c| c);
        assert!(m.is_valid() && m.vertices.iter().all(|v| v.pos.x + v.pos.y <= 10.001));
        let area: f32 = m.indices.chunks_exact(3).map(|t| {
            let [a, b, c] = [m.vertices[t[0] as usize].pos, m.vertices[t[1] as usize].pos, m.vertices[t[2] as usize].pos];
            ((b - a).x * (c - a).y - (b - a).y * (c - a).x).abs() / 2.0
        }).sum();
        assert!((area - 50.0).abs() < 0.01, "{area}");
    }
}
