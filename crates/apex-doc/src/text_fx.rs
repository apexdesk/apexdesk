//! Artistic text effects of characters: a solid or gradient fill, an
//! outline, a shadow, a glow, a reflection and a simple bevel (Word's
//! `w14` text effects, DrawingML's run fill, outline and effect list).
//! Lengths are in hundredths of a point and opacities 0..255, so the
//! effects hash and compare exactly.

use crate::{Dash, Gradient};

/// How the letters are filled.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextFill {
    /// The run's color.
    #[default]
    Auto,
    /// Only the outline shows.
    None,
    /// A color with an opacity.
    Solid(u32, u8),
    Gradient(Gradient),
}

/// A line around the letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextOutline {
    pub color: u32,
    pub alpha: u8,
    /// Hundredths of a point.
    pub width: u16,
    pub dash: Dash,
}

/// A copy of the letters behind them, moved and blurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextShadow {
    pub color: u32,
    pub alpha: u8,
    /// Hundredths of a point.
    pub blur: u16,
    pub dist: u16,
    /// Degrees clockwise from the right.
    pub angle: u16,
}

/// A soft colored edge around the letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextGlow {
    pub color: u32,
    pub alpha: u8,
    /// Hundredths of a point.
    pub radius: u16,
}

/// The letters mirrored under them, fading out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextReflection {
    /// Opacity at the letters.
    pub alpha: u8,
    /// Gap, hundredths of a point.
    pub dist: u16,
    /// How much of the letters shows, percent.
    pub size: u8,
}

/// The artistic effects of a run of text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct TextFx {
    pub fill: TextFill,
    pub outline: Option<TextOutline>,
    pub shadow: Option<TextShadow>,
    pub glow: Option<TextGlow>,
    pub reflection: Option<TextReflection>,
    /// Raised edges (lit from the top left).
    pub bevel: bool,
}

impl TextFx {
    pub fn is_empty(&self) -> bool {
        *self == TextFx::default()
    }

    /// The color the letters mostly have (what formats without the effect
    /// keep), if the fill says.
    pub fn main_color(&self) -> Option<u32> {
        match &self.fill {
            TextFill::Solid(c, _) => Some(*c),
            TextFill::Gradient(g) => Some(g.main_color()),
            TextFill::None => self.outline.map(|o| o.color),
            TextFill::Auto => None,
        }
    }
}

impl TextOutline {
    pub fn new(color: u32, width_pt: f32) -> TextOutline {
        TextOutline { color, alpha: 255, width: hundredths(width_pt), dash: Dash::Solid }
    }

    pub fn width_pt(&self) -> f32 {
        self.width as f32 / 100.0
    }
}

impl TextShadow {
    /// An outer shadow `dist_pt` away at `angle` degrees.
    pub fn new(color: u32, alpha: u8, blur_pt: f32, dist_pt: f32, angle: u16) -> TextShadow {
        TextShadow { color, alpha, blur: hundredths(blur_pt), dist: hundredths(dist_pt), angle: angle % 360 }
    }

    /// Its offset in points (x right, y down).
    pub fn offset_pt(&self) -> (f32, f32) {
        let a = (self.angle as f32).to_radians();
        let d = self.dist as f32 / 100.0;
        (d * a.cos(), d * a.sin())
    }

    pub fn blur_pt(&self) -> f32 {
        self.blur as f32 / 100.0
    }
}

impl TextGlow {
    pub fn new(color: u32, alpha: u8, radius_pt: f32) -> TextGlow {
        TextGlow { color, alpha, radius: hundredths(radius_pt) }
    }

    pub fn radius_pt(&self) -> f32 {
        self.radius as f32 / 100.0
    }
}

impl TextReflection {
    pub fn new(alpha: u8, dist_pt: f32, size: u8) -> TextReflection {
        TextReflection { alpha, dist: hundredths(dist_pt), size: size.clamp(1, 100) }
    }

    pub fn dist_pt(&self) -> f32 {
        self.dist as f32 / 100.0
    }
}

/// Points as hundredths (0..655).
pub fn hundredths(pt: f32) -> u16 {
    if pt.is_finite() { (pt * 100.0).round().clamp(0.0, u16::MAX as f32) as u16 } else { 0 }
}

/// Number of styles in the gallery.
pub const STYLES: usize = 15;

/// Style `i` of the gallery, made from the theme's dark text color and
/// accents (`accents[0]` is accent 1).
pub fn style(i: usize, dark: u32, accents: &[u32; 6]) -> TextFx {
    use crate::gradient::tint;
    let [a1, a2, _a3, a4, a5, a6] = *accents;
    let shadow = Some(TextShadow::new(0x000000, 100, 3.0, 2.0, 45));
    let grad = |c: u32| Gradient::two(tint(c, 0.45), tint(c, -0.35), 90.0);
    let mut fx = TextFx::default();
    match i % STYLES {
        0 => {
            fx.fill = TextFill::Solid(dark, 255);
            fx.shadow = shadow;
        }
        1 => {
            fx.fill = TextFill::Solid(a1, 255);
            fx.shadow = shadow;
        }
        2 => {
            fx.fill = TextFill::Solid(tint(a2, 0.6), 255);
            fx.outline = Some(TextOutline::new(a2, 1.0));
        }
        3 => {
            fx.fill = TextFill::Solid(0xFFFFFF, 255);
            fx.outline = Some(TextOutline::new(a1, 0.75));
            fx.glow = Some(TextGlow::new(a1, 110, 5.0));
        }
        4 => {
            fx.fill = TextFill::Gradient(Gradient::two(0xFFE699, 0xBF8F00, 90.0));
            fx.outline = Some(TextOutline::new(0x7F6000, 0.5));
            fx.shadow = shadow;
        }
        5 => {
            fx.fill = TextFill::Gradient(grad(a1));
            fx.reflection = Some(TextReflection::new(140, 1.0, 50));
        }
        6 => {
            fx.fill = TextFill::Solid(0xFFFFFF, 255);
            fx.outline = Some(TextOutline::new(a2, 1.5));
            fx.shadow = Some(TextShadow::new(a2, 200, 0.0, 3.0, 45));
        }
        7 => {
            fx.fill = TextFill::Gradient(grad(a1));
            fx.bevel = true;
            fx.shadow = shadow;
        }
        8 => {
            fx.fill = TextFill::None;
            fx.outline = Some(TextOutline::new(dark, 1.0));
        }
        9 => {
            fx.fill = TextFill::Gradient(Gradient::two(0xF2F2F2, 0x7F7F7F, 90.0));
            fx.outline = Some(TextOutline::new(0x595959, 0.5));
            fx.shadow = shadow;
        }
        10 => {
            fx.fill = TextFill::Solid(a6, 255);
            fx.glow = Some(TextGlow::new(tint(a6, 0.5), 150, 8.0));
        }
        11 => {
            fx.fill = TextFill::Solid(dark, 255);
            fx.glow = Some(TextGlow::new(a4, 150, 6.0));
        }
        12 => {
            fx.fill = TextFill::Gradient(grad(a5));
            fx.outline = Some(TextOutline { dash: Dash::Dash, ..TextOutline::new(tint(a5, -0.5), 1.0) });
        }
        13 => {
            fx.fill = TextFill::Gradient(Gradient::two(0xFFC000, a2, 90.0));
            fx.shadow = shadow;
            fx.reflection = Some(TextReflection::new(120, 0.5, 40));
        }
        _ => {
            fx.fill = TextFill::Solid(tint(dark, 0.25), 255);
            fx.bevel = true;
            fx.reflection = Some(TextReflection::new(110, 0.0, 60));
        }
    }
    fx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn styles_differ_and_hash() {
        let acc = [0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47];
        let all: HashSet<TextFx> = (0..STYLES).map(|i| style(i, 0x000000, &acc)).collect();
        assert_eq!(all.len(), STYLES);
        assert!(all.iter().all(|s| !s.is_empty()));
        let s = TextShadow::new(0, 255, 0.0, 2.0, 90);
        let (x, y) = s.offset_pt();
        assert!(x.abs() < 1e-4 && (y - 2.0).abs() < 1e-4);
        assert_eq!(style(8, 0x112233, &acc).main_color(), Some(0x112233));
    }
}
