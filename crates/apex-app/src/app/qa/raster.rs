//! A window drawn into a picture without a screen: the shapes of the
//! last frame, tessellated by egui into triangles, filled on the CPU
//! (no GPU, no display: the same pixels on every run of one machine).
//! Textures are the font atlas and the pictures the app decoded; any
//! other texture draws as its vertices' color.

use std::collections::HashMap;

use eframe::egui::{self, Color32, ColorImage, TextureId, epaint::Primitive};

use crate::app::audit::Rig;

/// A texture's pixels, premultiplied.
struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

impl Tex {
    fn sample(&self, u: f32, v: f32) -> Color32 {
        let x = ((u * self.w as f32) as isize).clamp(0, self.w as isize - 1) as usize;
        let y = ((v * self.h as f32) as isize).clamp(0, self.h as isize - 1) as usize;
        self.px[y * self.w + x]
    }
}

fn texture(id: TextureId, rig: &Rig) -> Option<Tex> {
    if id == TextureId::default() {
        let img = rig.ctx.fonts(|f| f.image());
        return Some(Tex { w: img.size[0], h: img.size[1], px: img.pixels });
    }
    if let Some(img) = rig.textures.get(&id) {
        return Some(Tex { w: img.size[0], h: img.size[1], px: img.pixels.clone() });
    }
    let bytes = crate::images::source(id)?;
    let d = crate::images::decode(&bytes)?;
    let px = d.rgba.chunks_exact(4).map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])).collect();
    Some(Tex { w: d.width as usize, h: d.height as usize, px })
}

fn mul(a: Color32, b: Color32) -> [u32; 4] {
    let m = |x: u8, y: u8| (x as u32 * y as u32 + 127) / 255;
    [m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a())]
}

/// The last frame of `rig`, `size` points wide and high, one pixel a
/// point.
pub(super) fn render(rig: &Rig, size: egui::Vec2) -> ColorImage {
    let (w, h) = (size.x as usize, size.y as usize);
    let mut px = vec![[255u32, 255, 255, 255]; w * h];
    let prims = rig.ctx.tessellate(rig.shapes.clone(), 1.0);
    let mut textures: HashMap<TextureId, Option<Tex>> = HashMap::new();
    for p in prims {
        let Primitive::Mesh(mesh) = &p.primitive else { continue };
        let tex = textures.entry(mesh.texture_id).or_insert_with(|| texture(mesh.texture_id, rig));
        let clip = p.clip_rect;
        let (cx0, cy0) = (clip.min.x.max(0.0) as usize, clip.min.y.max(0.0) as usize);
        let (cx1, cy1) = ((clip.max.x.ceil().max(0.0) as usize).min(w), (clip.max.y.ceil().max(0.0) as usize).min(h));
        for t in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [mesh.vertices[t[0] as usize], mesh.vertices[t[1] as usize], mesh.vertices[t[2] as usize]];
            let area = (b.pos.x - a.pos.x) * (c.pos.y - a.pos.y) - (b.pos.y - a.pos.y) * (c.pos.x - a.pos.x);
            if area.abs() < 1e-6 {
                continue;
            }
            let x0 = (a.pos.x.min(b.pos.x).min(c.pos.x).floor().max(0.0) as usize).max(cx0);
            let y0 = (a.pos.y.min(b.pos.y).min(c.pos.y).floor().max(0.0) as usize).max(cy0);
            let x1 = (a.pos.x.max(b.pos.x).max(c.pos.x).ceil().max(0.0) as usize).min(cx1);
            let y1 = (a.pos.y.max(b.pos.y).max(c.pos.y).ceil().max(0.0) as usize).min(cy1);
            for y in y0..y1 {
                let py = y as f32 + 0.5;
                for x in x0..x1 {
                    let pxf = x as f32 + 0.5;
                    // Barycentric weights; inside when all have the area's sign.
                    let wa = ((b.pos.x - pxf) * (c.pos.y - py) - (b.pos.y - py) * (c.pos.x - pxf)) / area;
                    let wb = ((c.pos.x - pxf) * (a.pos.y - py) - (c.pos.y - py) * (a.pos.x - pxf)) / area;
                    let wc = 1.0 - wa - wb;
                    if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                        continue;
                    }
                    let lerp = |f: fn(Color32) -> u8| (f(a.color) as f32 * wa + f(b.color) as f32 * wb + f(c.color) as f32 * wc).round().clamp(0.0, 255.0) as u8;
                    let col = Color32::from_rgba_premultiplied(lerp(|c| c.r()), lerp(|c| c.g()), lerp(|c| c.b()), lerp(|c| c.a()));
                    let src = match tex {
                        Some(t) => mul(col, t.sample(a.uv.x * wa + b.uv.x * wb + c.uv.x * wc, a.uv.y * wa + b.uv.y * wb + c.uv.y * wc)),
                        None => [col.r() as u32, col.g() as u32, col.b() as u32, col.a() as u32],
                    };
                    // Premultiplied "over".
                    let d = &mut px[y * w + x];
                    let keep = 255 - src[3];
                    for i in 0..4 {
                        d[i] = (src[i] + (d[i] * keep + 127) / 255).min(255);
                    }
                }
            }
        }
    }
    let pixels = px.into_iter().map(|p| Color32::from_rgb(p[0] as u8, p[1] as u8, p[2] as u8)).collect();
    ColorImage::new([w, h], pixels)
}
