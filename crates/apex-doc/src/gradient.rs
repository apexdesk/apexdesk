//! Gradient fills of shapes, text boxes, text and page backgrounds (Office's
//! Format Shape > Fill > Gradient fill): color stops with their
//! transparency, along a line at an angle, out from the center or toward
//! the box's edges; and the gallery of preset gradients made from a color.

use std::hash::{Hash, Hasher};

/// How the stops spread over the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum GradKind {
    /// Along a line at the gradient's angle.
    #[default]
    Linear,
    /// In circles from the center (the first stop) out.
    Radial,
    /// In rectangles from the center out.
    Rect,
    /// Following the shape's outline (drawn as rectangles).
    Path,
}

impl GradKind {
    pub const ALL: [GradKind; 4] = [GradKind::Linear, GradKind::Radial, GradKind::Rect, GradKind::Path];

    /// DrawingML's `a:path` value (None: linear).
    pub fn path(self) -> Option<&'static str> {
        match self {
            GradKind::Linear => None,
            GradKind::Radial => Some("circle"),
            GradKind::Rect => Some("rect"),
            GradKind::Path => Some("shape"),
        }
    }

    pub fn from_path(p: Option<&str>) -> GradKind {
        match p {
            None => GradKind::Linear,
            Some("circle") => GradKind::Radial,
            Some("shape") => GradKind::Path,
            Some(_) => GradKind::Rect,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            GradKind::Linear => "grad.linear",
            GradKind::Radial => "grad.radial",
            GradKind::Rect => "grad.rect",
            GradKind::Path => "grad.path",
        }
    }
}

/// A color at a place of the gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradStop {
    /// 0..1 from the start (the center, for the others than linear).
    pub pos: f32,
    pub color: u32,
    /// Opacity, 0 (clear) ..1.
    pub alpha: f32,
}

impl GradStop {
    pub fn new(pos: f32, color: u32) -> GradStop {
        GradStop { pos, color, alpha: 1.0 }
    }
}

/// A gradient: at least two stops, sorted by place.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradKind,
    /// Degrees clockwise, 0 from left to right (linear gradients).
    pub angle: f32,
    pub stops: Vec<GradStop>,
}

impl Eq for Gradient {}

impl Hash for Gradient {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (self.kind, self.angle.to_bits()).hash(h);
        for s in &self.stops {
            (s.pos.to_bits(), s.color, s.alpha.to_bits()).hash(h);
        }
    }
}

/// Most stops a gradient keeps (Office's limit).
pub const MAX_STOPS: usize = 10;

fn mix(a: u32, b: u32, k: f32) -> u32 {
    let m = |s: u32| {
        let (x, y) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((x + (y - x) * k).round().clamp(0.0, 255.0) as u32) << s
    };
    m(16) | m(8) | m(0)
}

/// `c` made lighter (`k` > 0, toward white) or darker (toward black).
pub fn tint(c: u32, k: f32) -> u32 {
    if k >= 0.0 { mix(c, 0xFFFFFF, k.min(1.0)) } else { mix(c, 0, (-k).min(1.0)) }
}

impl Gradient {
    /// A linear gradient from `a` to `b`.
    pub fn two(a: u32, b: u32, angle: f32) -> Gradient {
        Gradient { kind: GradKind::Linear, angle, stops: vec![GradStop::new(0.0, a), GradStop::new(1.0, b)] }
    }

    /// Keeps values usable whatever a file said: finite, in range, sorted,
    /// two to ten stops.
    pub fn sanitize(&mut self) {
        self.angle = if self.angle.is_finite() { self.angle.rem_euclid(360.0) } else { 0.0 };
        if self.angle >= 360.0 {
            self.angle = 0.0;
        }
        for s in &mut self.stops {
            s.pos = if s.pos.is_finite() { s.pos.clamp(0.0, 1.0) } else { 0.0 };
            s.alpha = if s.alpha.is_finite() { s.alpha.clamp(0.0, 1.0) } else { 1.0 };
            s.color &= 0xFF_FFFF;
        }
        self.stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        self.stops.truncate(MAX_STOPS);
        match self.stops.len() {
            0 => self.stops = vec![GradStop::new(0.0, 0xFFFFFF), GradStop::new(1.0, 0)],
            1 => {
                let s = self.stops[0];
                self.stops = vec![GradStop { pos: 0.0, ..s }, GradStop { pos: 1.0, ..s }];
            }
            _ => {}
        }
    }

    /// The color and opacity at `t` (0..1).
    pub fn at(&self, t: f32) -> (u32, f32) {
        let Some(first) = self.stops.first() else { return (0, 0.0) };
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        if t <= first.pos {
            return (first.color, first.alpha);
        }
        let mut prev = first;
        for s in &self.stops[1..] {
            if t <= s.pos {
                let k = if s.pos > prev.pos { (t - prev.pos) / (s.pos - prev.pos) } else { 1.0 };
                return (mix(prev.color, s.color, k), prev.alpha + (s.alpha - prev.alpha) * k);
            }
            prev = s;
        }
        (prev.color, prev.alpha)
    }

    /// Where point (u, v) of the box (0..1 each, from its top left) falls
    /// along the gradient, 0..1.
    pub fn t_at(&self, u: f32, v: f32) -> f32 {
        let (dx, dy) = (u - 0.5, v - 0.5);
        match self.kind {
            GradKind::Radial => ((dx * dx + dy * dy).sqrt() / std::f32::consts::FRAC_1_SQRT_2).min(1.0),
            GradKind::Rect | GradKind::Path => (dx.abs().max(dy.abs()) * 2.0).min(1.0),
            GradKind::Linear => {
                let a = self.angle.to_radians();
                let (c, s) = (a.cos(), a.sin());
                // Projected on the direction, scaled so the corners are 0 and 1.
                let span = c.abs() + s.abs();
                (dx * c + dy * s) / span.max(1e-6) + 0.5
            }
        }
    }

    /// One color standing for the gradient (automatic text color, a solid
    /// fill in its place): the mean of its colors.
    pub fn main_color(&self) -> u32 {
        let n = 16;
        let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
        for i in 0..=n {
            let c = self.at(i as f32 / n as f32).0;
            r += (c >> 16) & 0xFF;
            g += (c >> 8) & 0xFF;
            b += c & 0xFF;
        }
        let d = n + 1;
        ((r / d) << 16) | ((g / d) << 8) | (b / d)
    }

    /// Adds a stop at `pos` with the color the gradient has there; its index.
    pub fn add_stop(&mut self, pos: f32) -> Option<usize> {
        if self.stops.len() >= MAX_STOPS {
            return None;
        }
        let pos = pos.clamp(0.0, 1.0);
        let (color, alpha) = self.at(pos);
        let i = self.stops.iter().position(|s| s.pos > pos).unwrap_or(self.stops.len());
        self.stops.insert(i, GradStop { pos, color, alpha });
        Some(i)
    }

    /// Removes stop `i` (two always stay).
    pub fn remove_stop(&mut self, i: usize) -> bool {
        if self.stops.len() <= 2 || i >= self.stops.len() {
            return false;
        }
        self.stops.remove(i);
        true
    }

    /// Moves stop `i` to `pos`, keeping the order; its new index.
    pub fn move_stop(&mut self, i: usize, pos: f32) -> usize {
        let Some(mut s) = self.stops.get(i).copied() else { return i };
        s.pos = if pos.is_finite() { pos.clamp(0.0, 1.0) } else { s.pos };
        self.stops.remove(i);
        let j = self.stops.iter().position(|x| x.pos > s.pos).unwrap_or(self.stops.len());
        self.stops.insert(j, s);
        j
    }

    /// Whether every stop is opaque.
    pub fn opaque(&self) -> bool {
        self.stops.iter().all(|s| s.alpha >= 1.0)
    }
}

/// The directions of Office's gradient variations: a name key, the kind
/// and the angle.
pub const VARIATIONS: [(&str, GradKind, f32); 9] = [
    ("grad.v.diag_down_right", GradKind::Linear, 45.0),
    ("grad.v.down", GradKind::Linear, 90.0),
    ("grad.v.diag_down_left", GradKind::Linear, 135.0),
    ("grad.v.right", GradKind::Linear, 0.0),
    ("grad.v.from_center", GradKind::Radial, 0.0),
    ("grad.v.left", GradKind::Linear, 180.0),
    ("grad.v.diag_up_right", GradKind::Linear, 315.0),
    ("grad.v.up", GradKind::Linear, 270.0),
    ("grad.v.diag_up_left", GradKind::Linear, 225.0),
];

/// Office's light (`dark` false) or dark variation `i` of `VARIATIONS` made
/// from color `c`.
pub fn variation(c: u32, dark: bool, i: usize) -> Gradient {
    let (_, kind, angle) = VARIATIONS[i.min(VARIATIONS.len() - 1)];
    let (a, b) = if dark { (c, tint(c, -0.45)) } else { (tint(c, 0.55), c) };
    // From the center: the center is the first stop.
    let (a, b) = if kind == GradKind::Linear { (a, b) } else { (b, a) };
    Gradient { kind, angle, stops: vec![GradStop::new(0.0, a), GradStop::new(1.0, b)] }
}

/// Office's preset gradients of a theme color (Format Shape > Preset
/// gradients): light, soft, medium, spotlights and radial.
pub fn preset(c: u32, i: usize) -> Gradient {
    let s = |pos: f32, k: f32| GradStop::new(pos, tint(c, k));
    match i % 5 {
        0 => Gradient { kind: GradKind::Linear, angle: 90.0, stops: vec![s(0.0, 0.8), s(0.74, 0.4), s(0.83, 0.4), s(1.0, 0.25)] },
        1 => Gradient { kind: GradKind::Linear, angle: 90.0, stops: vec![s(0.0, 0.4), s(0.5, 0.0), s(1.0, -0.25)] },
        2 => Gradient { kind: GradKind::Radial, angle: 0.0, stops: vec![s(0.0, 0.6), s(1.0, -0.1)] },
        3 => Gradient { kind: GradKind::Linear, angle: 90.0, stops: vec![s(0.0, -0.3), s(0.5, 0.2), s(1.0, -0.3)] },
        _ => Gradient { kind: GradKind::Rect, angle: 0.0, stops: vec![s(0.0, 0.7), s(1.0, 0.0)] },
    }
}

/// Number of presets per color.
pub const PRESETS: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_along_stops() {
        let mut g = Gradient::two(0x000000, 0xFFFFFF, 0.0);
        assert_eq!(g.at(0.5).0, 0x808080);
        assert_eq!(g.at(-1.0).0, 0);
        assert_eq!(g.t_at(0.0, 0.5), 0.0);
        assert_eq!(g.t_at(1.0, 0.5), 1.0);
        g.angle = 90.0;
        assert!((g.t_at(0.5, 1.0) - 1.0).abs() < 1e-5);
        let i = g.add_stop(0.25).unwrap();
        assert_eq!((i, g.stops.len()), (1, 3));
        assert_eq!(g.stops[1].color, 0x404040);
        assert_eq!(g.move_stop(1, 0.9), 1);
        assert_eq!(g.move_stop(0, 0.95), 1);
        assert!(g.remove_stop(0));
        assert_eq!(g.stops.len(), 2);
        assert!(!g.remove_stop(0));
    }

    #[test]
    fn sanitizes() {
        let mut g = Gradient { kind: GradKind::Radial, angle: f32::NAN, stops: vec![GradStop { pos: 2.0, color: 0xFF00_0000, alpha: -1.0 }] };
        g.sanitize();
        assert_eq!(g.angle, 0.0);
        assert_eq!(g.stops.len(), 2);
        assert_eq!((g.stops[1].pos, g.stops[0].color, g.stops[0].alpha), (1.0, 0, 0.0));
        let v = variation(0x4472C4, false, 4);
        assert_eq!((v.kind, v.stops[0].color), (GradKind::Radial, 0x4472C4));
        assert!(variation(0x4472C4, true, 1).main_color() < 0x4472C4);
    }
}
