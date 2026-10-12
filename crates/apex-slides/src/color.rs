//! Colors as DrawingML keeps them: plain RGB, a slot of the theme (seen
//! through the master's color map) or the color a style reference passes
//! down ("phClr"), with changes (lighter, darker, transparent) applied
//! when the color is resolved.

use apex_doc::Theme;

/// A theme slot as named in files: the mapped names (bg1, tx1...) go
/// through the color map, the others name the theme's colors directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Bg1,
    Tx1,
    Bg2,
    Tx2,
    Dk1,
    Lt1,
    Dk2,
    Lt2,
    /// Accent 1 to 6 (0-based).
    Accent(u8),
    Hlink,
    FolHlink,
}

impl Scheme {
    pub fn parse(s: &str) -> Option<Scheme> {
        Some(match s {
            "bg1" => Scheme::Bg1,
            "tx1" => Scheme::Tx1,
            "bg2" => Scheme::Bg2,
            "tx2" => Scheme::Tx2,
            "dk1" => Scheme::Dk1,
            "lt1" => Scheme::Lt1,
            "dk2" => Scheme::Dk2,
            "lt2" => Scheme::Lt2,
            "hlink" => Scheme::Hlink,
            "folHlink" => Scheme::FolHlink,
            _ => {
                let n: u8 = s.strip_prefix("accent")?.parse().ok()?;
                if !(1..=6).contains(&n) {
                    return None;
                }
                Scheme::Accent(n - 1)
            }
        })
    }

    pub fn name(self) -> String {
        match self {
            Scheme::Bg1 => "bg1".into(),
            Scheme::Tx1 => "tx1".into(),
            Scheme::Bg2 => "bg2".into(),
            Scheme::Tx2 => "tx2".into(),
            Scheme::Dk1 => "dk1".into(),
            Scheme::Lt1 => "lt1".into(),
            Scheme::Dk2 => "dk2".into(),
            Scheme::Lt2 => "lt2".into(),
            Scheme::Accent(n) => format!("accent{}", n + 1),
            Scheme::Hlink => "hlink".into(),
            Scheme::FolHlink => "folHlink".into(),
        }
    }

    /// The theme slot (apex_doc order: dk1 lt1 dk2 lt2 accent1-6 hlink
    /// folHlink) through the color map.
    pub fn slot(self, map: &ClrMap) -> u8 {
        match self {
            Scheme::Bg1 => map.0[0],
            Scheme::Tx1 => map.0[1],
            Scheme::Bg2 => map.0[2],
            Scheme::Tx2 => map.0[3],
            Scheme::Dk1 => 0,
            Scheme::Lt1 => 1,
            Scheme::Dk2 => 2,
            Scheme::Lt2 => 3,
            Scheme::Accent(n) => map.0[4 + n.min(5) as usize],
            Scheme::Hlink => map.0[10],
            Scheme::FolHlink => map.0[11],
        }
    }
}

/// The master's color map: which theme slot bg1, tx1, bg2, tx2,
/// accent1-6, hlink and folHlink stand for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClrMap(pub [u8; 12]);

/// Names of the mapped slots, in `ClrMap` order.
pub const MAP_NAMES: [&str; 12] = ["bg1", "tx1", "bg2", "tx2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hlink", "folHlink"];

impl Default for ClrMap {
    fn default() -> Self {
        ClrMap([1, 0, 3, 2, 4, 5, 6, 7, 8, 9, 10, 11])
    }
}

impl ClrMap {
    /// Dark backgrounds: light text on dark slides.
    pub fn inverted() -> ClrMap {
        ClrMap([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])
    }

    /// Reads the attributes of a `p:clrMap` (name, value pairs).
    pub fn from_attrs<'a>(attrs: impl IntoIterator<Item = (&'a str, &'a str)>) -> ClrMap {
        let mut m = ClrMap::default();
        for (k, v) in attrs {
            if let Some(i) = MAP_NAMES.iter().position(|n| *n == k) {
                if let Some(s) = apex_doc::theme::SLOT_NAMES.iter().position(|n| *n == v) {
                    m.0[i] = s as u8;
                }
            }
        }
        m
    }

    /// The attributes of a `p:clrMap`.
    pub fn attrs(&self) -> String {
        let mut s = String::new();
        for (i, n) in MAP_NAMES.iter().enumerate() {
            let v = apex_doc::theme::SLOT_NAMES.get(self.0[i] as usize).copied().unwrap_or("dk1");
            s.push_str(&format!(r#" {n}="{v}""#));
        }
        s
    }
}

/// A change to a color, in DrawingML's thousandths of a percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mod {
    LumMod(i32),
    LumOff(i32),
    Tint(i32),
    Shade(i32),
    SatMod(i32),
    Alpha(i32),
    /// Anything else, kept by name to write back.
    Other(&'static str, i32),
}

impl Mod {
    pub fn parse(name: &str, val: i32) -> Mod {
        match name {
            "lumMod" => Mod::LumMod(val),
            "lumOff" => Mod::LumOff(val),
            "tint" => Mod::Tint(val),
            "shade" => Mod::Shade(val),
            "satMod" => Mod::SatMod(val),
            "alpha" => Mod::Alpha(val),
            "hueMod" => Mod::Other("hueMod", val),
            "hueOff" => Mod::Other("hueOff", val),
            "satOff" => Mod::Other("satOff", val),
            "alphaMod" => Mod::Other("alphaMod", val),
            "alphaOff" => Mod::Other("alphaOff", val),
            "comp" => Mod::Other("comp", val),
            "inv" => Mod::Other("inv", val),
            "gray" => Mod::Other("gray", val),
            _ => Mod::Other("gamma", val),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mod::LumMod(_) => "lumMod",
            Mod::LumOff(_) => "lumOff",
            Mod::Tint(_) => "tint",
            Mod::Shade(_) => "shade",
            Mod::SatMod(_) => "satMod",
            Mod::Alpha(_) => "alpha",
            Mod::Other(n, _) => n,
        }
    }

    pub fn value(self) -> i32 {
        match self {
            Mod::LumMod(v) | Mod::LumOff(v) | Mod::Tint(v) | Mod::Shade(v) | Mod::SatMod(v) | Mod::Alpha(v) | Mod::Other(_, v) => v,
        }
    }
}

/// What a color starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Base {
    Rgb(u32),
    Scheme(Scheme),
    /// The color of the style reference that uses this fill ("phClr").
    Placeholder,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SColor {
    pub base: Base,
    pub mods: Vec<Mod>,
}

/// Colors resolved for drawing: 0xRRGGBB and opacity 0..=255.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub rgb: u32,
    pub alpha: u8,
}

impl Rgba {
    pub const fn opaque(rgb: u32) -> Rgba {
        Rgba { rgb, alpha: 255 }
    }
}

/// What resolving a color needs: the theme, the color map, and the color
/// a style reference passes to "phClr".
#[derive(Clone, Copy)]
pub struct ColorCx<'a> {
    pub theme: &'a Theme,
    pub map: ClrMap,
    pub ph: Option<Rgba>,
}

impl SColor {
    pub fn rgb(c: u32) -> SColor {
        SColor { base: Base::Rgb(c & 0xFF_FFFF), mods: Vec::new() }
    }

    pub fn scheme(s: Scheme) -> SColor {
        SColor { base: Base::Scheme(s), mods: Vec::new() }
    }

    pub fn with(mut self, m: Mod) -> SColor {
        self.mods.push(m);
        self
    }

    pub fn resolve(&self, cx: &ColorCx) -> Rgba {
        let (mut rgb, mut alpha) = match self.base {
            Base::Rgb(c) => (c, 255),
            Base::Scheme(s) => (cx.theme.color(s.slot(&cx.map)), 255),
            Base::Placeholder => cx.ph.map_or((0, 255), |p| (p.rgb, p.alpha)),
        };
        if !self.mods.is_empty() {
            (rgb, alpha) = apply_mods(rgb, alpha, &self.mods);
        }
        Rgba { rgb, alpha }
    }
}

/// sRGB to linear light.
fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Linear light to sRGB.
fn to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Applies color changes: luminance ones in HSL, tint and shade toward
/// white and black in linear light (as PowerPoint does: black with a 75%
/// tint is #898989, not #404040), alpha as opacity.
pub fn apply_mods(rgb: u32, alpha: u8, mods: &[Mod]) -> (u32, u8) {
    let (mut h, mut s, mut l) = to_hsl(rgb);
    let mut out_alpha = alpha as f64 / 255.0;
    let mut rgb_now = rgb;
    let mut hsl_dirty = false;
    let f = |v: i32| v as f64 / 100_000.0;
    for m in mods {
        match *m {
            Mod::LumMod(v) => {
                l *= f(v);
                hsl_dirty = true;
            }
            Mod::LumOff(v) => {
                l += f(v);
                hsl_dirty = true;
            }
            Mod::SatMod(v) => {
                s *= f(v);
                hsl_dirty = true;
            }
            Mod::Alpha(v) => out_alpha = f(v).clamp(0.0, 1.0),
            Mod::Tint(v) | Mod::Shade(v) => {
                if hsl_dirty {
                    rgb_now = from_hsl(h, s, l);
                    hsl_dirty = false;
                }
                let t = f(v).clamp(0.0, 1.0);
                let ch = |c: u32| {
                    let x = to_linear(c as f64 / 255.0);
                    let y = if matches!(m, Mod::Tint(_)) { x * t + (1.0 - t) } else { x * t };
                    ((to_srgb(y.clamp(0.0, 1.0)) * 255.0).round() as u32).min(255)
                };
                rgb_now = (ch((rgb_now >> 16) & 0xFF) << 16) | (ch((rgb_now >> 8) & 0xFF) << 8) | ch(rgb_now & 0xFF);
                (h, s, l) = to_hsl(rgb_now);
            }
            Mod::Other("inv", _) => {
                rgb_now = !rgb_now & 0xFF_FFFF;
                (h, s, l) = to_hsl(rgb_now);
            }
            Mod::Other("gray", _) => {
                let (r, g, b) = ((rgb_now >> 16) & 0xFF, (rgb_now >> 8) & 0xFF, rgb_now & 0xFF);
                let y = ((r * 299 + g * 587 + b * 114) / 1000).min(255);
                rgb_now = (y << 16) | (y << 8) | y;
                (h, s, l) = to_hsl(rgb_now);
            }
            Mod::Other("alphaMod", v) => out_alpha = (out_alpha * f(v)).clamp(0.0, 1.0),
            Mod::Other("alphaOff", v) => out_alpha = (out_alpha + f(v)).clamp(0.0, 1.0),
            Mod::Other("hueOff", v) => {
                h = (h + v as f64 / 60_000.0 / 360.0).rem_euclid(1.0);
                hsl_dirty = true;
            }
            Mod::Other("hueMod", v) => {
                h = (h * f(v)).rem_euclid(1.0);
                hsl_dirty = true;
            }
            Mod::Other("satOff", v) => {
                s += f(v);
                hsl_dirty = true;
            }
            Mod::Other(..) => {}
        }
    }
    if hsl_dirty {
        rgb_now = from_hsl(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    }
    (rgb_now, (out_alpha * 255.0).round() as u8)
}

/// Hue (0..1), saturation and lightness of an RGB color.
pub fn to_hsl(c: u32) -> (f64, f64, f64) {
    let r = ((c >> 16) & 0xFF) as f64 / 255.0;
    let g = ((c >> 8) & 0xFF) as f64 / 255.0;
    let b = (c & 0xFF) as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-12 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

/// The RGB color of a hue (0..1), saturation and lightness.
pub fn from_hsl(h: f64, s: f64, l: f64) -> u32 {
    let l = l.clamp(0.0, 1.0);
    let ch = |v: f64| ((v.clamp(0.0, 1.0) * 255.0).round() as u32).min(255);
    if s <= 0.0 {
        let v = ch(l);
        return (v << 16) | (v << 8) | v;
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |t: f64| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (ch(hue(h + 1.0 / 3.0)) << 16) | (ch(hue(h)) << 8) | ch(hue(h - 1.0 / 3.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_and_shade_in_linear_light() {
        // PowerPoint's subtitle gray: black with a 75% tint.
        assert_eq!(apply_mods(0x000000, 255, &[Mod::Tint(75000)]).0, 0x898989);
        assert_eq!(apply_mods(0xFFFFFF, 255, &[Mod::Shade(50000)]).0, 0xBCBCBC);
        assert_eq!(apply_mods(0x4472C4, 255, &[Mod::Tint(100000)]).0, 0x4472C4);
    }

    #[test]
    fn scheme_colors_through_the_map() {
        let theme = Theme::default();
        let cx = ColorCx { theme: &theme, map: ClrMap::default(), ph: None };
        assert_eq!(SColor::scheme(Scheme::Tx1).resolve(&cx).rgb, 0x000000);
        assert_eq!(SColor::scheme(Scheme::Bg1).resolve(&cx).rgb, 0xFFFFFF);
        let dark = ColorCx { map: ClrMap::inverted(), ..cx };
        assert_eq!(SColor::scheme(Scheme::Tx1).resolve(&dark).rgb, 0xFFFFFF);
        assert_eq!(SColor::scheme(Scheme::Accent(0)).resolve(&cx).rgb, 0x4472C4);
        // Office's "Accent 1, Lighter 40%": lumMod 60000, lumOff 40000.
        let c = SColor::scheme(Scheme::Accent(0)).with(Mod::LumMod(60000)).with(Mod::LumOff(40000));
        assert_eq!(c.resolve(&cx).rgb, 0x8FAADC);
        let a = SColor::rgb(0xFF0000).with(Mod::Alpha(50000)).resolve(&cx);
        assert_eq!((a.rgb, a.alpha), (0xFF0000, 128));
        let m = ClrMap::from_attrs([("bg1", "dk1"), ("tx1", "lt1")]);
        assert_eq!(m.0[0], 0);
        assert!(m.attrs().contains(r#"bg1="dk1""#));
    }
}
