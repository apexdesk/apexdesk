//! Color and font themes shared by documents and workbooks: twelve scheme
//! colors (the DrawingML order dk1, lt1, dk2, lt2, accent1-6, hlink,
//! folHlink) plus a heading ("major") and a body ("minor") font.

use std::sync::Arc;

/// Scheme color slots.
pub mod slot {
    pub const DK1: u8 = 0;
    pub const LT1: u8 = 1;
    pub const DK2: u8 = 2;
    pub const LT2: u8 = 3;
    pub const ACCENT1: u8 = 4;
    pub const HLINK: u8 = 10;
    pub const FOL_HLINK: u8 = 11;
}

/// Names of the slots in theme files, in slot order.
pub const SLOT_NAMES: [&str; 12] = ["dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hlink", "folHlink"];

/// Font tokens standing for the theme fonts (as in DrawingML).
pub const MAJOR_TOKEN: &str = "+mj-lt";
pub const MINOR_TOKEN: &str = "+mn-lt";
/// Tokens for the theme's East Asian fonts (picked by the text's language).
pub const EA_MAJOR_TOKEN: &str = "+mj-ea";
pub const EA_MINOR_TOKEN: &str = "+mn-ea";
/// Tokens for the theme's complex script fonts (picked by the language).
pub const CS_MAJOR_TOKEN: &str = "+mj-cs";
pub const CS_MINOR_TOKEN: &str = "+mn-cs";

/// A theme color with a tint (positive, lighter) or shade (negative,
/// darker), in ten-thousandths: 4000 is "Lighter 40%".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct ThemeColor {
    pub slot: u8,
    pub tint: i16,
}

impl ThemeColor {
    pub const fn new(slot: u8, tint: i16) -> ThemeColor {
        ThemeColor { slot, tint }
    }

    /// The tint as a fraction in -1..=1.
    pub fn tint_f(&self) -> f64 {
        self.tint as f64 / 10_000.0
    }

    pub fn from_tint_f(slot: u8, t: f64) -> ThemeColor {
        ThemeColor { slot: slot.min(11), tint: (t.clamp(-1.0, 1.0) * 10_000.0).round() as i16 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColorScheme {
    pub name: Arc<str>,
    pub colors: [u32; 12],
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontScheme {
    pub name: Arc<str>,
    pub major: Arc<str>,
    pub minor: Arc<str>,
    /// Fonts of other scripts: (major?, script, typeface), with "ea" and
    /// "cs" for the scheme's East Asian and complex script fonts and
    /// "Jpan", "Hang", "Hans"... for the fonts by script.
    pub scripts: Vec<(bool, Arc<str>, Arc<str>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: Arc<str>,
    pub colors: ColorScheme,
    pub fonts: FontScheme,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::builtin(0)
    }
}

const COLOR_SCHEMES: &[(&str, [u32; 12])] = &[
    ("Office", [0x000000, 0xFFFFFF, 0x44546A, 0xE7E6E6, 0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47, 0x0563C1, 0x954F72]),
    ("Office 2007 - 2010", [0x000000, 0xFFFFFF, 0x1F497D, 0xEEECE1, 0x4F81BD, 0xC0504D, 0x9BBB59, 0x8064A2, 0x4BACC6, 0xF79646, 0x0000FF, 0x800080]),
    ("Grayscale", [0x000000, 0xFFFFFF, 0x000000, 0xF8F8F8, 0xDDDDDD, 0xB2B2B2, 0x969696, 0x808080, 0x5F5F5F, 0x4D4D4D, 0x5F5F5F, 0x919191]),
    ("Blue Warm", [0x000000, 0xFFFFFF, 0x242852, 0xACCBF9, 0x4A66AC, 0x629DD1, 0x297FD5, 0x7F8FA9, 0x5AA2AE, 0x9D90A0, 0x9454C3, 0x3EBBF0]),
    ("Blue", [0x000000, 0xFFFFFF, 0x17406D, 0xDBEFF9, 0x0F6FC6, 0x009DD9, 0x0BD0D9, 0x10CF9B, 0x7CCA62, 0xA5C249, 0xF49100, 0x85DFD0]),
    ("Blue II", [0x000000, 0xFFFFFF, 0x335B74, 0xDFE3E5, 0x1CADE4, 0x2683C6, 0x27CED7, 0x42BA97, 0x3E8853, 0x62A39F, 0x6B9F25, 0xB26B02]),
    ("Blue Green", [0x000000, 0xFFFFFF, 0x373545, 0xCEDBE6, 0x3494BA, 0x58B6C0, 0x75BDA7, 0x7A8C8E, 0x84ACB6, 0x2683C6, 0x6B9F25, 0x9F6715]),
    ("Green", [0x000000, 0xFFFFFF, 0x455F51, 0xE3DED1, 0x549E39, 0x8AB833, 0xC0CF3A, 0x029676, 0x4AB5C4, 0x0989B1, 0x6B9F25, 0xBA6906]),
    ("Yellow", [0x000000, 0xFFFFFF, 0x39302A, 0xE5DEDB, 0xFFCA08, 0xF8931D, 0xCE8D3E, 0xEC7016, 0xE64823, 0x9C6A6A, 0x2998E3, 0x7F723D]),
    ("Orange", [0x000000, 0xFFFFFF, 0x637052, 0xCCDDEA, 0xE48312, 0xBD582C, 0x865640, 0x9B8357, 0xC2BC80, 0x94A088, 0x2998E3, 0x8C8C8C]),
    ("Red Orange", [0x000000, 0xFFFFFF, 0x505046, 0xEEECE1, 0xE84C22, 0xFFBD47, 0xB64926, 0xFF8427, 0xCC9900, 0xB22600, 0xCC9900, 0x666699]),
    ("Violet", [0x000000, 0xFFFFFF, 0x373545, 0xDCD8DC, 0xAD84C6, 0x8784C7, 0x5D739A, 0x6997AF, 0x84ACB6, 0x6F8183, 0x69A020, 0x8C8C8C]),
    ("Paper", [0x000000, 0xFFFFFF, 0x444D26, 0xFEFAC9, 0xA5B592, 0xF3A447, 0xE7BC29, 0xD092A7, 0x9C85C0, 0x809EC2, 0x8E58B6, 0x7F6F6F]),
];

const FONT_SCHEMES: &[(&str, &str, &str)] = &[
    ("Office", "Calibri Light", "Calibri"),
    ("Office 2007 - 2010", "Cambria", "Calibri"),
    ("Aptos", "Aptos Display", "Aptos"),
    ("Calibri", "Calibri", "Calibri"),
    ("Arial", "Arial", "Arial"),
    ("Corbel", "Corbel", "Corbel"),
    ("Candara", "Candara", "Candara"),
    ("Franklin Gothic", "Franklin Gothic Medium", "Franklin Gothic Book"),
    ("Century Gothic", "Century Gothic", "Century Gothic"),
    ("Tw Cen MT", "Tw Cen MT", "Tw Cen MT"),
    ("Cambria", "Cambria", "Cambria"),
    ("Garamond", "Garamond", "Garamond"),
    ("Georgia", "Georgia", "Georgia"),
    ("Trebuchet MS", "Trebuchet MS", "Trebuchet MS"),
    ("Verdana", "Verdana", "Verdana"),
    ("Times New Roman", "Times New Roman", "Times New Roman"),
    ("Constantia", "Constantia", "Constantia"),
];

/// Built-in themes: (name, color scheme, font scheme).
const THEMES: &[(&str, usize, usize)] = &[
    ("Office", 0, 0),
    ("Office 2007 - 2010", 1, 1),
    ("Grayscale", 2, 0),
    ("Blue Warm", 3, 5),
    ("Blue", 4, 3),
    ("Blue II", 5, 8),
    ("Blue Green", 6, 6),
    ("Green", 7, 7),
    ("Yellow", 8, 13),
    ("Orange", 9, 9),
    ("Red Orange", 10, 14),
    ("Violet", 11, 12),
    ("Paper", 12, 16),
];

pub fn color_schemes() -> Vec<ColorScheme> {
    COLOR_SCHEMES.iter().map(|(n, c)| ColorScheme { name: Arc::from(*n), colors: *c }).collect()
}

pub fn font_schemes() -> Vec<FontScheme> {
    FONT_SCHEMES.iter().map(|(n, a, b)| FontScheme { name: Arc::from(*n), major: Arc::from(*a), minor: Arc::from(*b), scripts: Vec::new() }).collect()
}

pub fn themes() -> Vec<Theme> {
    (0..THEMES.len()).map(Theme::builtin).collect()
}

impl Theme {
    /// The built-in theme number `i` (Office when out of range).
    pub fn builtin(i: usize) -> Theme {
        let (name, c, f) = THEMES.get(i).copied().unwrap_or(THEMES[0]);
        let (cn, colors) = COLOR_SCHEMES[c];
        let (fname, major, minor) = FONT_SCHEMES[f];
        Theme {
            name: Arc::from(name),
            colors: ColorScheme { name: Arc::from(cn), colors },
            fonts: FontScheme { name: Arc::from(fname), major: Arc::from(major), minor: Arc::from(minor), scripts: Vec::new() },
        }
    }

    pub fn color(&self, slot: u8) -> u32 {
        self.colors.colors.get(slot as usize).copied().unwrap_or(0)
    }

    /// RGB of a theme color.
    pub fn resolve(&self, c: ThemeColor) -> u32 {
        apply_tint(self.color(c.slot), c.tint_f())
    }

    /// The font a token stands for (`+mj-lt` / `+mn-lt`), if it is one.
    pub fn token_font(&self, name: &str) -> Option<&Arc<str>> {
        match name {
            MAJOR_TOKEN => Some(&self.fonts.major),
            MINOR_TOKEN => Some(&self.fonts.minor),
            _ => None,
        }
    }

    /// The East Asian font of a token (`+mj-ea` / `+mn-ea`) for text in
    /// language `lang`: the font of its script, else the scheme's.
    pub fn ea_font(&self, name: &str, lang: Option<&str>) -> Option<&Arc<str>> {
        let major = match name {
            EA_MAJOR_TOKEN => true,
            EA_MINOR_TOKEN => false,
            _ => return None,
        };
        let l = lang.unwrap_or("").to_ascii_lowercase();
        let script = if l.starts_with("ko") {
            "Hang"
        } else if l.starts_with("zh-tw") || l.starts_with("zh-hk") || l.starts_with("zh-mo") {
            "Hant"
        } else if l.starts_with("zh") {
            "Hans"
        } else {
            "Jpan"
        };
        let find = |s: &str| self.fonts.scripts.iter().find(|(m, k, f)| *m == major && &**k == s && !f.is_empty()).map(|x| &x.2);
        find(script).or_else(|| find("ea"))
    }

    /// The complex script font of a token (`+mj-cs` / `+mn-cs`) for text
    /// in language `lang`: the font of its script, else the scheme's (and
    /// Word's stand-in, the Arabic one).
    pub fn cs_font(&self, name: &str, lang: Option<&str>) -> Option<&Arc<str>> {
        let major = match name {
            CS_MAJOR_TOKEN => true,
            CS_MINOR_TOKEN => false,
            _ => return None,
        };
        let l = lang.unwrap_or("").to_ascii_lowercase();
        let script = match l.get(..2).unwrap_or("") {
            "he" | "yi" => "Hebr",
            "th" => "Thai",
            "hi" | "mr" | "ne" | "sa" => "Deva",
            _ => "Arab",
        };
        let find = |s: &str| self.fonts.scripts.iter().find(|(m, k, f)| *m == major && &**k == s && !f.is_empty()).map(|x| &x.2);
        find("cs").or_else(|| find(script)).or_else(|| find("Arab"))
    }

    /// `name` with theme font tokens replaced by the font they stand for.
    pub fn font_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.token_font(name).map_or(name, |f| f)
    }

    /// The Office color gallery: ten columns (Background 1, Text 1,
    /// Background 2, Text 2, Accent 1-6) of a base color and five tints or
    /// shades.
    pub fn palette(&self) -> [[ThemeColor; 10]; 6] {
        let mut out = [[ThemeColor::default(); 10]; 6];
        for (col, s) in [slot::LT1, slot::DK1, slot::LT2, slot::DK2, 4, 5, 6, 7, 8, 9].into_iter().enumerate() {
            out[0][col] = ThemeColor::new(s, 0);
            let tints = palette_tints(self.color(s));
            for (row, t) in tints.iter().enumerate() {
                out[row + 1][col] = ThemeColor::new(s, *t);
            }
        }
        out
    }

    /// The gallery entry (or base theme color) with exactly this RGB.
    pub fn match_rgb(&self, rgb: u32) -> Option<ThemeColor> {
        let p = self.palette();
        // Accents first: black and white also match the text/background slots.
        let order = [4usize, 5, 6, 7, 8, 9, 1, 0, 3, 2];
        for row in p.iter() {
            for &col in &order {
                if self.resolve(row[col]) == rgb & 0xFF_FFFF {
                    return Some(row[col]);
                }
            }
        }
        None
    }
}

/// Tints (ten-thousandths) of the five gallery rows under a base color, as
/// Office picks them from its lightness.
pub fn palette_tints(rgb: u32) -> [i16; 5] {
    let (_, l, _) = rgb_to_hls(rgb);
    if l >= 0.999 {
        [-500, -1500, -2500, -3500, -5000]
    } else if l <= 0.001 {
        [5000, 3500, 2500, 1500, 500]
    } else if l > 0.8 {
        [-1000, -2500, -5000, -7500, -9000]
    } else if l < 0.2 {
        [9000, 7500, 5000, 2500, 1000]
    } else {
        [8000, 6000, 4000, -2500, -5000]
    }
}

fn rgb_to_hls(c: u32) -> (f64, f64, f64) {
    let r = ((c >> 16) & 0xFF) as f64 / 255.0;
    let g = ((c >> 8) & 0xFF) as f64 / 255.0;
    let b = (c & 0xFF) as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-12 {
        return (0.0, l, 0.0);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, l, s)
}

fn hls_to_rgb(h: f64, l: f64, s: f64) -> u32 {
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

/// Excel's tint: lightness moves toward white (positive) or black
/// (negative) by the given fraction. Like Excel, the HLS values are
/// rounded to a 0..=240 scale.
pub fn apply_tint(rgb: u32, tint: f64) -> u32 {
    if tint == 0.0 {
        return rgb & 0xFF_FFFF;
    }
    const M: f64 = 240.0;
    let (h, l, s) = rgb_to_hls(rgb);
    let (h, l, s) = ((h * M).round(), (l * M).round(), (s * M).round());
    let l = if tint < 0.0 { (l * (1.0 + tint)).round() } else { (l * (1.0 - tint) + M * tint).round() };
    hls_to_rgb(h / M, l / M, s / M)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_gallery() {
        let t = Theme::default();
        // Blue, Accent 1, Lighter 80% and Darker 50%.
        assert_eq!(t.resolve(ThemeColor::new(4, 8000)), 0xDAE3F3);
        assert_eq!(t.resolve(ThemeColor::new(4, -5000)), 0x203864);
        // White darker 5%, black lighter 50%.
        assert_eq!(t.resolve(ThemeColor::new(slot::LT1, -500)), 0xF2F2F2);
        assert_eq!(t.resolve(ThemeColor::new(slot::DK1, 5000)), 0x808080);
        let p = t.palette();
        assert_eq!(p[0][0], ThemeColor::new(slot::LT1, 0));
        assert_eq!(p[1][3], ThemeColor::new(slot::DK2, 8000));
        // Light gray Background 2 gets shades.
        assert_eq!(p[1][2].tint, -1000);
        assert_eq!(t.match_rgb(0xDAE3F3), Some(ThemeColor::new(4, 8000)));
        assert_eq!(t.match_rgb(0x123456), None);
    }

    #[test]
    fn builtins() {
        let all = themes();
        assert!(all.len() >= 8);
        assert!(all.iter().any(|t| &*t.name == "Paper"));
        assert_eq!(&*Theme::builtin(1).fonts.major, "Cambria");
        assert_eq!(Theme::default().font_name(MAJOR_TOKEN), "Calibri Light");
        assert_eq!(Theme::default().font_name("Arial"), "Arial");
    }
}
