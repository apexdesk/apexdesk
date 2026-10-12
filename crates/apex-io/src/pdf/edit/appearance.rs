//! Appearance streams: what an annotation or a form field looks like,
//! drawn once so that every viewer shows the same. Text uses Helvetica
//! (one of the standard fonts every viewer has) in WinAnsi, so characters
//! outside Western European ones show as "?"; the annotation's own text
//! keeps them all.

use std::sync::Arc;

use super::super::obj::{Dict, Obj};
use super::super::tables;
use super::write::deflate;

/// Font resource names.
pub const HELV: &str = "Helv";
pub const HELV_BOLD: &str = "HeBo";

/// The resources with Helvetica and Helvetica-Bold (and a graphics state
/// `GS0` with opacity `alpha` and blend mode `blend`, when given).
pub fn resources(alpha: Option<f64>, blend: Option<&str>) -> Dict {
    let font = |base: &str| Obj::Dict(Dict::of(vec![("Type", Obj::name_of("Font")), ("Subtype", Obj::name_of("Type1")), ("BaseFont", Obj::name_of(base)), ("Encoding", Obj::name_of("WinAnsiEncoding"))]));
    let mut r = Dict::of(vec![("Font", Obj::Dict(Dict::of(vec![(HELV, font("Helvetica")), (HELV_BOLD, font("Helvetica-Bold"))])))]);
    if alpha.is_some() || blend.is_some() {
        let mut gs = Dict::of(vec![("Type", Obj::name_of("ExtGState"))]);
        if let Some(a) = alpha {
            gs.set("CA", Obj::Real(a));
            gs.set("ca", Obj::Real(a));
        }
        if let Some(b) = blend {
            gs.set("BM", Obj::name_of(b));
        }
        r.set("ExtGState", Obj::Dict(Dict::of(vec![("GS0", Obj::Dict(gs))])));
    }
    r
}

/// A form XObject `w` by `h` drawn with `content`, placed by `matrix`.
pub fn form(w: f64, h: f64, matrix: [f64; 6], content: &[u8], res: Dict) -> Obj {
    let d = Dict::of(vec![
        ("Type", Obj::name_of("XObject")),
        ("Subtype", Obj::name_of("Form")),
        ("BBox", Obj::Array(vec![Obj::Int(0), Obj::Int(0), Obj::num_of(round(w)), Obj::num_of(round(h))])),
        ("Matrix", Obj::Array(matrix.iter().map(|v| Obj::num_of(*v)).collect())),
        ("Resources", Obj::Dict(res)),
    ]);
    if content.len() > 256 { deflate(d, content) } else { Obj::Stream(d, Arc::from(content)) }
}

pub fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// A number for a content stream.
pub fn n(v: f64) -> String {
    let v = round(v);
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// An RGB colour's operands.
pub fn rgb(c: u32) -> String {
    let f = |v: u32| n((v & 0xFF) as f64 / 255.0);
    format!("{} {} {}", f(c >> 16), f(c >> 8), f(c))
}

/// A colour as a PDF array.
pub fn color_obj(c: u32) -> Obj {
    let f = |v: u32| Obj::Real(round((v & 0xFF) as f64 / 255.0));
    Obj::Array(vec![f(c >> 16), f(c >> 8), f(c)])
}

/// A PDF colour array as RGB.
pub fn color_of(v: &[f64]) -> Option<u32> {
    let b = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    match v.len() {
        1 => Some(b(v[0]) * 0x010101),
        3 => Some(b(v[0]) << 16 | b(v[1]) << 8 | b(v[2])),
        4 => {
            let k = 1.0 - v[3];
            Some(b((1.0 - v[0]) * k) << 16 | b((1.0 - v[1]) * k) << 8 | b((1.0 - v[2]) * k))
        }
        _ => None,
    }
}

/// WinAnsi code of a character (`?` when it has none).
pub fn win_ansi(c: char) -> u8 {
    let u = c as u32;
    if (32..127).contains(&u) {
        return u as u8;
    }
    tables::WIN_ANSI.iter().position(|&x| x as u32 == u && x != 0).map_or(b'?', |i| i as u8)
}

/// The text as a WinAnsi string operand.
pub fn string(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars() {
        match win_ansi(c) {
            b'(' => out.push_str("\\("),
            b')' => out.push_str("\\)"),
            b'\\' => out.push_str("\\\\"),
            b @ 32..=126 => out.push(b as char),
            b => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push(')');
    out
}

/// Width of `s` in Helvetica (bold) at `size`.
pub fn width(s: &str, bold: bool, size: f64) -> f64 {
    let table = &tables::WIDTHS[if bold { 1 } else { 0 }];
    s.chars().map(|c| {
        let b = win_ansi(c) as usize;
        if b >= 32 { table[b - 32] as f64 } else { 556.0 }
    }).sum::<f64>() * size / 1000.0
}

/// `text` broken into lines of at most `w` wide.
pub fn wrap(text: &str, w: f64, size: f64, bold: bool) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if width(&cand, bold, size) <= w || line.is_empty() {
                line = cand;
                // A word longer than the line is broken by characters.
                while width(&line, bold, size) > w && line.chars().count() > 1 {
                    let mut cut = line.len();
                    while cut > 0 && width(&line[..cut], bold, size) > w {
                        cut = line[..cut].char_indices().next_back().map_or(0, |(i, _)| i);
                    }
                    if cut == 0 {
                        break;
                    }
                    out.push(line[..cut].to_string());
                    line = line[cut..].to_string();
                }
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        out.push(line);
    }
    out
}

/// Text lines at the top left of a `w` by `h` box with `pad` around.
pub fn text_block(lines: &[String], w: f64, h: f64, size: f64, color: u32, pad: f64, align: u8) -> String {
    let mut s = format!("BT /{HELV} {} Tf {} rg\n", n(size), rgb(color));
    let lead = size * 1.15;
    let mut y = h - pad - size * 0.9;
    for l in lines {
        if y < -size {
            break;
        }
        let tw = width(l, false, size);
        let x = match align {
            1 => (w - tw) / 2.0,
            2 => w - pad - tw,
            _ => pad,
        };
        s.push_str(&format!("1 0 0 1 {} {} Tm {} Tj\n", n(x), n(y), string(l)));
        y -= lead;
    }
    s.push_str("ET\n");
    s
}

/// An ellipse inside (x0, y0, x1, y1) as Bézier curves.
pub fn ellipse(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let (cx, cy, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let k = 0.552_284_75;
    let (ox, oy) = (rx * k, ry * k);
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h\n",
        n(cx + rx), n(cy),
        n(cx + rx), n(cy + oy), n(cx + ox), n(cy + ry), n(cx), n(cy + ry),
        n(cx - ox), n(cy + ry), n(cx - rx), n(cy + oy), n(cx - rx), n(cy),
        n(cx - rx), n(cy - oy), n(cx - ox), n(cy - ry), n(cx), n(cy - ry),
        n(cx + ox), n(cy - ry), n(cx + rx), n(cy - oy), n(cx + rx), n(cy),
    )
}

/// A rounded rectangle path.
pub fn rounded(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> String {
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    let k = r * 0.447_715;
    format!(
        "{} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c h\n",
        n(x0 + r), n(y0),
        n(x1 - r), n(y0), n(x1 - k), n(y0), n(x1), n(y0 + k), n(x1), n(y0 + r),
        n(x1), n(y1 - r), n(x1), n(y1 - k), n(x1 - k), n(y1), n(x1 - r), n(y1),
        n(x0 + r), n(y1), n(x0 + k), n(y1), n(x0), n(y1 - k), n(x0), n(y1 - r),
        n(x0), n(y0 + r), n(x0), n(y0 + k), n(x0 + k), n(y0), n(x0 + r), n(y0),
    )
}

/// An image XObject from RGBA pixels (with a soft mask when some are not
/// opaque).
pub fn image(w: u32, h: u32, rgba: &[u8]) -> (Obj, Option<Obj>) {
    let px = (w as usize) * (h as usize);
    let mut rgb = Vec::with_capacity(px * 3);
    let mut alpha = Vec::with_capacity(px);
    for p in rgba.chunks_exact(4).take(px) {
        rgb.extend_from_slice(&p[..3]);
        alpha.push(p[3]);
    }
    let d = |cs: &str| Dict::of(vec![("Type", Obj::name_of("XObject")), ("Subtype", Obj::name_of("Image")), ("Width", Obj::Int(w as i64)), ("Height", Obj::Int(h as i64)), ("ColorSpace", Obj::name_of(cs)), ("BitsPerComponent", Obj::Int(8))]);
    let mask = alpha.iter().any(|&a| a < 255).then(|| deflate(d("DeviceGray"), &alpha));
    (deflate(d("DeviceRGB"), &rgb), mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_helpers() {
        assert_eq!(win_ansi('A'), b'A');
        assert_eq!(win_ansi('é'), 0xE9);
        assert_eq!(win_ansi('€'), 0x80);
        assert_eq!(win_ansi('中'), b'?');
        assert_eq!(string("a(b)é"), "(a\\(b\\)\\351)");
        assert!((width("Hello", false, 10.0) - 22.78).abs() < 0.01);
        let lines = wrap("one two three four", width("one two", false, 10.0) + 0.1, 10.0, false);
        assert_eq!(lines, ["one two", "three", "four"]);
        assert_eq!(n(1.5), "1.5");
        assert_eq!(n(2.0), "2");
        assert_eq!(rgb(0xFF0000), "1 0 0");
        assert_eq!(color_of(&[1.0, 0.0, 0.0]), Some(0xFF0000));
    }
}
