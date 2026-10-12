//! Picture effects: corrections, recoloring, transparency and the effects
//! drawn around a picture (border, shadow, glow, soft edges, reflection).

/// How a picture is recolored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum PicColor {
    #[default]
    Normal,
    Grayscale,
    /// Black and white at a 50 % threshold.
    BlackWhite,
    /// Light and low contrast, for watermarks.
    Washout,
    Sepia,
}

impl PicColor {
    pub const ALL: [PicColor; 5] = [PicColor::Normal, PicColor::Grayscale, PicColor::BlackWhite, PicColor::Washout, PicColor::Sepia];
}

/// Offset of a picture's shadow down and to the right, and its blur, in
/// points.
pub const SHADOW_OFFSET: f32 = 3.0;
pub const SHADOW_BLUR: f32 = 2.0;
/// Space between a picture and its reflection, in points.
pub const REFLECTION_GAP: f32 = 1.0;

/// Effects of a picture. The default has none.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PicFx {
    /// -100..=100 %.
    pub brightness: i8,
    /// -100..=100 %.
    pub contrast: i8,
    pub color: PicColor,
    /// 0..=100 %.
    pub transparency: u8,
    /// Border color and width in points.
    pub border: Option<(u32, f32)>,
    /// A shadow down and to the right.
    pub shadow: bool,
    /// Glow color and size in points.
    pub glow: Option<(u32, f32)>,
    /// Soft edges size in points (0: none).
    pub soft_edges: f32,
    /// A reflection under the picture.
    pub reflection: bool,
}

impl Eq for PicFx {}

impl std::hash::Hash for PicFx {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        (self.brightness, self.contrast, self.color, self.transparency, self.shadow, self.reflection).hash(h);
        self.border.map(|(c, w)| (c, w.to_bits())).hash(h);
        self.glow.map(|(c, w)| (c, w.to_bits())).hash(h);
        self.soft_edges.to_bits().hash(h);
    }
}

impl PicFx {
    pub fn is_default(&self) -> bool {
        *self == PicFx::default()
    }

    /// Whether the pixels change (not only what's drawn around them).
    pub fn changes_pixels(&self) -> bool {
        self.brightness != 0 || self.contrast != 0 || self.color != PicColor::Normal || self.transparency > 0 || self.soft_edges > 0.0
    }

    /// The picture styles gallery: name key and effects.
    pub fn styles() -> Vec<(&'static str, PicFx)> {
        let white = Some((0xFFFFFF, 6.0));
        vec![
            ("picfx.style_none", PicFx::default()),
            ("picfx.style_frame", PicFx { border: white, shadow: true, ..Default::default() }),
            ("picfx.style_thin", PicFx { border: Some((0x000000, 1.0)), ..Default::default() }),
            ("picfx.style_shadow", PicFx { shadow: true, ..Default::default() }),
            ("picfx.style_soft", PicFx { soft_edges: 10.0, ..Default::default() }),
            ("picfx.style_reflected", PicFx { reflection: true, ..Default::default() }),
            ("picfx.style_glow", PicFx { glow: Some((0x4472C4, 8.0)), ..Default::default() }),
            ("picfx.style_bevel", PicFx { border: Some((0x404040, 4.0)), shadow: true, reflection: true, ..Default::default() }),
        ]
    }

    /// Room the effects take around a picture `w` x `h` points: left, top,
    /// right and bottom, in points.
    pub fn extent(&self, h: f32) -> [f32; 4] {
        let mut e = [0.0f32; 4];
        let mut grow = |l: f32, t: f32, r: f32, b: f32| {
            for (x, v) in e.iter_mut().zip([l, t, r, b]) {
                *x = x.max(v);
            }
        };
        if let Some((_, w)) = self.border {
            grow(w / 2.0, w / 2.0, w / 2.0, w / 2.0);
        }
        if let Some((_, r)) = self.glow {
            grow(r, r, r, r);
        }
        if self.shadow {
            grow(0.0, 0.0, SHADOW_OFFSET + SHADOW_BLUR, SHADOW_OFFSET + SHADOW_BLUR);
        }
        if self.reflection {
            grow(0.0, 0.0, 0.0, REFLECTION_GAP + h * 0.5);
        }
        e
    }

    /// Brightness and contrast as fractions (-1..=1), with the washout
    /// recoloring folded in.
    pub fn levels(&self) -> (f32, f32) {
        let (mut b, mut c) = (self.brightness as f32 / 100.0, self.contrast as f32 / 100.0);
        if self.color == PicColor::Washout {
            b = (b + 0.7).min(1.0);
            c = (c - 0.7).max(-1.0);
        }
        (b, c)
    }

    /// Applies the pixel effects to RGBA8 pixels (not premultiplied) of a
    /// picture `w` x `h` shown `shown_w` points wide.
    pub fn apply(&self, w: u32, h: u32, rgba: &mut [u8], shown_w: f32) {
        let (b, c) = self.levels();
        let k = if c >= 0.0 { 1.0 / (1.0 - c * 0.99) } else { 1.0 + c };
        let level = |v: f32| -> f32 {
            let v = if b >= 0.0 { v + (255.0 - v) * b } else { v * (1.0 + b) };
            ((v - 128.0) * k + 128.0).clamp(0.0, 255.0)
        };
        let alpha = 1.0 - self.transparency.min(100) as f32 / 100.0;
        let soft = if self.soft_edges > 0.0 && shown_w > 0.0 { self.soft_edges / shown_w * w as f32 } else { 0.0 };
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            let [r, g, bl] = [px[0] as f32, px[1] as f32, px[2] as f32];
            let gray = 0.299 * r + 0.587 * g + 0.114 * bl;
            let (r, g, bl) = match self.color {
                PicColor::Grayscale => (gray, gray, gray),
                PicColor::BlackWhite => {
                    let v = if gray >= 128.0 { 255.0 } else { 0.0 };
                    (v, v, v)
                }
                PicColor::Sepia => (gray * 1.07, gray * 0.87, gray * 0.68),
                _ => (r, g, bl),
            };
            px[0] = level(r).round() as u8;
            px[1] = level(g).round() as u8;
            px[2] = level(bl).round() as u8;
            let mut a = px[3] as f32 * alpha;
            if soft > 0.0 {
                let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
                let d = x.min(y).min(w as f32 - x).min(h as f32 - y);
                let t = (d / soft).clamp(0.0, 1.0);
                a *= t * t * (3.0 - 2.0 * t);
            }
            px[3] = a.round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_effects() {
        let mut px = [200, 100, 50, 255, 10, 20, 30, 255];
        PicFx { color: PicColor::Grayscale, ..Default::default() }.apply(2, 1, &mut px, 2.0);
        assert_eq!((px[0], px[1], px[2]), (124, 124, 124));
        let mut px = [200, 100, 50, 255];
        PicFx { transparency: 50, ..Default::default() }.apply(1, 1, &mut px, 1.0);
        assert_eq!(px, [200, 100, 50, 128]);
        let mut px = [100, 100, 100, 255];
        PicFx { brightness: 100, ..Default::default() }.apply(1, 1, &mut px, 1.0);
        assert_eq!(px[0], 255);
        let mut px = [100, 100, 100, 255, 200, 200, 200, 255];
        PicFx { color: PicColor::BlackWhite, ..Default::default() }.apply(2, 1, &mut px, 2.0);
        assert_eq!((px[0], px[4]), (0, 255));
        // Soft edges fade the border pixels.
        let mut px = vec![255u8; 10 * 10 * 4];
        PicFx { soft_edges: 3.0, ..Default::default() }.apply(10, 10, &mut px, 10.0);
        assert!(px[3] < 60 && px[(5 * 10 + 5) * 4 + 3] == 255);
        assert!(!PicFx { shadow: true, ..Default::default() }.changes_pixels());
    }
}
