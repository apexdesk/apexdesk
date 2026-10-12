//! Fills, outlines, backgrounds and the theme's style lists that shapes
//! refer to (`p:style`: "the theme's second fill in accent 1").

use std::sync::Arc;

use crate::color::{ColorCx, Rgba, SColor};

/// A gradient: stops at 0..1 and a direction.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub stops: Vec<(f32, SColor)>,
    /// Degrees, clockwise from left to right (linear gradients).
    pub angle: f32,
    /// Radial and rectangular gradients: "circle", "rect", "shape".
    pub path: Option<Arc<str>>,
}

impl Gradient {
    /// The gradient with its colors worked out (for painting, and formats
    /// without theme colors).
    pub fn resolve(&self, cx: &ColorCx) -> apex_doc::Gradient {
        let stops = self.stops.iter().map(|(p, c)| {
            let c = c.resolve(cx);
            apex_doc::GradStop { pos: *p, color: c.rgb, alpha: c.alpha as f32 / 255.0 }
        });
        let mut g = apex_doc::Gradient { kind: apex_doc::GradKind::from_path(self.path.as_deref()), angle: self.angle, stops: stops.collect() };
        g.sanitize();
        g
    }

    /// A gradient of plain colors (opacity as an alpha change).
    pub fn from_doc(g: &apex_doc::Gradient) -> Gradient {
        let stops = g.stops.iter().map(|s| {
            let c = SColor::rgb(s.color);
            (s.pos, if s.alpha < 1.0 { c.with(crate::color::Mod::Alpha((s.alpha * 100_000.0).round() as i32)) } else { c })
        });
        Gradient { stops: stops.collect(), angle: g.angle, path: g.kind.path().map(Arc::from) }
    }
}

/// A picture filling a shape or the background.
#[derive(Clone, Debug, PartialEq)]
pub struct PictureFill {
    pub data: Arc<[u8]>,
    /// Repeated at its size instead of stretched.
    pub tile: bool,
    /// Cropped fractions: left, top, right, bottom.
    pub crop: [f32; 4],
    /// Opacity 0..1.
    pub alpha: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    None,
    Solid(SColor),
    Gradient(Gradient),
    Picture(PictureFill),
    /// A hatch ("pct50", "dkDnDiag"...): drawn as its two colors mixed.
    Pattern { preset: Arc<str>, fg: SColor, bg: SColor },
    /// The group's fill.
    Group,
}

impl Fill {
    /// One color standing for the fill (for text contrast, thumbnails).
    pub fn main_color(&self, cx: &ColorCx) -> Option<Rgba> {
        match self {
            Fill::Solid(c) => Some(c.resolve(cx)),
            Fill::Gradient(g) => g.stops.first().map(|s| s.1.resolve(cx)),
            Fill::Pattern { fg, bg, .. } => {
                let (a, b) = (fg.resolve(cx), bg.resolve(cx));
                let mix = |s: u32| (((a.rgb >> s) & 0xFF) + ((b.rgb >> s) & 0xFF)) / 2;
                Some(Rgba::opaque((mix(16) << 16) | (mix(8) << 8) | mix(0)))
            }
            _ => None,
        }
    }
}

/// Dash patterns of outlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dash {
    Solid,
    Dot,
    Dash,
    DashDot,
    LongDash,
    LongDashDot,
    LongDashDotDot,
    SysDash,
    SysDot,
    SysDashDot,
    SysDashDotDot,
}

impl Dash {
    pub const ALL: [(Dash, &'static str); 11] = [
        (Dash::Solid, "solid"),
        (Dash::Dot, "dot"),
        (Dash::Dash, "dash"),
        (Dash::DashDot, "dashDot"),
        (Dash::LongDash, "lgDash"),
        (Dash::LongDashDot, "lgDashDot"),
        (Dash::LongDashDotDot, "lgDashDotDot"),
        (Dash::SysDash, "sysDash"),
        (Dash::SysDot, "sysDot"),
        (Dash::SysDashDot, "sysDashDot"),
        (Dash::SysDashDotDot, "sysDashDotDot"),
    ];

    pub fn parse(s: &str) -> Dash {
        Dash::ALL.iter().find(|(_, n)| *n == s).map_or(Dash::Solid, |d| d.0)
    }

    pub fn name(self) -> &'static str {
        Dash::ALL.iter().find(|(d, _)| *d == self).map_or("solid", |d| d.1)
    }

    /// Dash and gap lengths in line widths (empty: solid).
    pub fn pattern(self) -> &'static [f32] {
        match self {
            Dash::Solid => &[],
            Dash::Dot => &[1.0, 3.0],
            Dash::Dash => &[4.0, 3.0],
            Dash::DashDot => &[4.0, 3.0, 1.0, 3.0],
            Dash::LongDash => &[8.0, 3.0],
            Dash::LongDashDot => &[8.0, 3.0, 1.0, 3.0],
            Dash::LongDashDotDot => &[8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
            Dash::SysDash => &[3.0, 1.0],
            Dash::SysDot => &[1.0, 1.0],
            Dash::SysDashDot => &[3.0, 1.0, 1.0, 1.0],
            Dash::SysDashDotDot => &[3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        }
    }

    /// The document shapes' dash closest to this one.
    pub fn to_doc(self) -> apex_doc::Dash {
        match self {
            Dash::Solid => apex_doc::Dash::Solid,
            Dash::Dot | Dash::SysDot => apex_doc::Dash::Dot,
            Dash::Dash | Dash::SysDash => apex_doc::Dash::Dash,
            Dash::DashDot | Dash::SysDashDot | Dash::SysDashDotDot | Dash::LongDashDot | Dash::LongDashDotDot => apex_doc::Dash::DashDot,
            Dash::LongDash => apex_doc::Dash::LongDash,
        }
    }
}

/// Arrowheads at line ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arrow {
    None,
    Triangle,
    Stealth,
    Diamond,
    Oval,
    Open,
}

impl Arrow {
    pub fn parse(s: &str) -> Arrow {
        match s {
            "triangle" => Arrow::Triangle,
            "stealth" => Arrow::Stealth,
            "diamond" => Arrow::Diamond,
            "oval" => Arrow::Oval,
            "arrow" => Arrow::Open,
            _ => Arrow::None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Arrow::None => "none",
            Arrow::Triangle => "triangle",
            Arrow::Stealth => "stealth",
            Arrow::Diamond => "diamond",
            Arrow::Oval => "oval",
            Arrow::Open => "arrow",
        }
    }
}

/// An outline (`a:ln`): unset values come from the style reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    /// Points.
    pub width: Option<f32>,
    pub fill: Option<Fill>,
    pub dash: Option<Dash>,
    pub head: Option<Arrow>,
    pub tail: Option<Arrow>,
}

impl Line {
    pub fn solid(c: SColor, width: f32) -> Line {
        Line { width: Some(width), fill: Some(Fill::Solid(c)), ..Default::default() }
    }

    pub fn none() -> Line {
        Line { fill: Some(Fill::None), ..Default::default() }
    }

    pub fn overlay(&mut self, o: &Line) {
        if o.width.is_some() {
            self.width = o.width;
        }
        if o.fill.is_some() {
            self.fill = o.fill.clone();
        }
        if o.dash.is_some() {
            self.dash = o.dash;
        }
        if o.head.is_some() {
            self.head = o.head;
        }
        if o.tail.is_some() {
            self.tail = o.tail;
        }
    }
}

/// A slide, layout or master background.
#[derive(Clone, Debug, PartialEq)]
pub enum Background {
    Fill(Fill),
    /// The theme's background fill `idx` (1001...) in a color.
    Ref(u32, SColor),
}

/// A drop shadow, the effect drawn (others are kept in the file).
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub color: SColor,
    /// Blur radius, distance (points) and direction (degrees).
    pub blur: f32,
    pub dist: f32,
    pub dir: f32,
}

/// A shape's references to the theme's styles (`p:style`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleRefs {
    pub line: Option<(u32, SColor)>,
    pub fill: Option<(u32, SColor)>,
    pub effect: Option<(u32, SColor)>,
    /// "major", "minor" or "none", and the text color.
    pub font: Option<(Arc<str>, Option<SColor>)>,
}

/// The theme's fills, outlines and effects that styles refer to, by
/// 1-based index (backgrounds from 1001).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatScheme {
    pub fills: Vec<Fill>,
    pub lines: Vec<Line>,
    pub bg_fills: Vec<Fill>,
    pub shadows: Vec<Option<Shadow>>,
}

impl FormatScheme {
    /// Office's: solid, a subtle and an intense gradient; thin to thick lines.
    pub fn office() -> FormatScheme {
        use crate::color::{Base, Mod};
        let ph = || SColor { base: Base::Placeholder, mods: Vec::new() };
        let grad = |a: [(i32, i32); 3]| {
            Fill::Gradient(Gradient { stops: vec![(0.0, ph().with(Mod::LumMod(a[0].0)).with(Mod::LumOff(a[0].1))), (0.5, ph().with(Mod::LumMod(a[1].0)).with(Mod::LumOff(a[1].1))), (1.0, ph().with(Mod::LumMod(a[2].0)).with(Mod::LumOff(a[2].1)))], angle: 90.0, path: None })
        };
        FormatScheme {
            fills: vec![Fill::Solid(ph()), grad([(110000, 0), (105000, 0), (100000, 0)]), grad([(102000, 0), (103000, 0), (94000, 0)])],
            lines: vec![Line::solid(ph(), 0.5), Line::solid(ph(), 1.0), Line::solid(ph(), 1.5)],
            bg_fills: vec![Fill::Solid(ph()), Fill::Solid(ph().with(Mod::Tint(95000))), grad([(100000, 0), (100000, 0), (93000, 0)])],
            shadows: vec![None, None, None],
        }
    }

    /// The fill a style reference stands for.
    pub fn fill(&self, idx: u32) -> Option<&Fill> {
        match idx {
            0 => None,
            1..=999 => self.fills.get(idx as usize - 1).or_else(|| self.fills.last()),
            _ => self.bg_fills.get(idx as usize - 1001).or_else(|| self.bg_fills.last()),
        }
    }

    pub fn line(&self, idx: u32) -> Option<&Line> {
        if idx == 0 { None } else { self.lines.get(idx as usize - 1).or_else(|| self.lines.last()) }
    }
}
