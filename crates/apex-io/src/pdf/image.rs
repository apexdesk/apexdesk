//! Image XObjects to PNG (or the JPEG they hold).

use super::file::PdfFile;
use super::obj::{Dict, Obj};

/// Pixels as 8-bit RGB (3 channels) or RGBA (4).
struct Pixels {
    w: usize,
    h: usize,
    channels: usize,
    data: Vec<u8>,
}

/// The image of `d`: PNG or JPEG bytes and its size in pixels. `fill` is
/// the colour of stencil masks.
pub fn image(f: &PdfFile, d: &Dict, raw: &[u8], fill: u32) -> Option<(Vec<u8>, u32, u32)> {
    let w = f.key_num(d, "Width")?.max(0.0) as usize;
    let h = f.key_num(d, "Height")?.max(0.0) as usize;
    if w == 0 || h == 0 || w.saturating_mul(h) > 40_000_000 {
        return None;
    }
    let dec = f.decode(d, raw);
    match dec.codec.as_deref() {
        // A JPEG is kept as it is (a soft mask is left out).
        Some("DCTDecode" | "DCT") => return Some((dec.data, w as u32, h as u32)),
        Some(_) => return None,
        None => {}
    }
    let px = pixels_of(f, d, &dec.data, w, h, fill)?;
    Some((png(&px), w as u32, h as u32))
}

/// The decoded pixels of `d` (not a JPEG or other codec): the samples as
/// RGB or RGBA, width, height and channels.
pub fn pixels(f: &PdfFile, d: &Dict, raw: &[u8], fill: u32) -> Option<(Vec<u8>, usize, usize, usize)> {
    let w = f.key_num(d, "Width")?.max(0.0) as usize;
    let h = f.key_num(d, "Height")?.max(0.0) as usize;
    if w == 0 || h == 0 || w.saturating_mul(h) > 40_000_000 {
        return None;
    }
    let dec = f.decode(d, raw);
    if dec.codec.is_some() {
        return None;
    }
    pixels_of(f, d, &dec.data, w, h, fill).map(|p| (p.data, p.w, p.h, p.channels))
}

fn pixels_of(f: &PdfFile, d: &Dict, data: &[u8], w: usize, h: usize, fill: u32) -> Option<Pixels> {
    let smask = match f.key(d, "SMask") {
        Obj::Stream(sd, sraw) => gray_of(f, &sd, &sraw),
        _ => None,
    };
    let mask = f.key(d, "ImageMask") == Obj::Bool(true);
    let bpc = if mask { 1 } else { f.key_num(d, "BitsPerComponent").unwrap_or(8.0) as usize };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    let decode = f.nums(d.get("Decode").unwrap_or(&Obj::Null));
    let mut px = if mask {
        let inverted = decode.first().is_some_and(|&v| v > 0.5);
        let samples = unpack(data, w, h, 1, 1);
        let [r, g, b] = [(fill >> 16) as u8, (fill >> 8) as u8, fill as u8];
        let mut data = Vec::with_capacity(w * h * 4);
        for s in samples {
            let paint = (s == 0) != inverted;
            data.extend_from_slice(&[r, g, b, if paint { 255 } else { 0 }]);
        }
        data.resize(w * h * 4, 0);
        Pixels { w, h, channels: 4, data }
    } else {
        let cs = f.key(d, "ColorSpace");
        rgb(f, &cs, data, w, h, bpc, &decode)?
    };
    if let Some((sw, sh, alpha)) = smask {
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * px.channels;
                data.extend_from_slice(&px.data[i..i + 3]);
                let (sx, sy) = (x * sw / w, y * sh / h);
                data.push(alpha.get(sy * sw + sx).copied().unwrap_or(255));
            }
        }
        px = Pixels { w, h, channels: 4, data };
    }
    Some(px)
}

fn gray_of(f: &PdfFile, d: &Dict, raw: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let w = f.key_num(d, "Width")?.max(0.0) as usize;
    let h = f.key_num(d, "Height")?.max(0.0) as usize;
    if w == 0 || h == 0 || w.saturating_mul(h) > 40_000_000 {
        return None;
    }
    let dec = f.decode(d, raw);
    if dec.codec.is_some() {
        return None;
    }
    let bpc = f.key_num(d, "BitsPerComponent").unwrap_or(8.0) as usize;
    let max = ((1u32 << bpc.min(16)) - 1) as f32;
    let mut v: Vec<u8> = unpack(&dec.data, w, h, 1, bpc).into_iter().map(|s| (s as f32 / max * 255.0) as u8).collect();
    v.resize(w * h, 0);
    Some((w, h, v))
}

/// Samples of `n` components of `bpc` bits, rows padded to bytes.
fn unpack(data: &[u8], w: usize, h: usize, n: usize, bpc: usize) -> Vec<u16> {
    // Samples take two bytes each: small ones (1 bit) would make the
    // decoded data many times larger, so their count is bounded too.
    const MAX_SAMPLES: usize = 1 << 27;
    let row = w.saturating_mul(n);
    if row > MAX_SAMPLES || bpc == 0 || bpc > 16 {
        return Vec::new();
    }
    let row_bytes = (row * bpc).div_ceil(8);
    // Only the rows the data holds: a short stream mustn't claim memory
    // for all the image says it is (callers pad the rest).
    let h = h.min(data.len().div_ceil(row_bytes.max(1))).min(MAX_SAMPLES / row.max(1));
    let mut out = Vec::with_capacity(w * h * n);
    for y in 0..h {
        let row = data.get(y * row_bytes..(y + 1) * row_bytes).unwrap_or(&[]);
        for i in 0..w * n {
            let v = match bpc {
                8 => row.get(i).copied().unwrap_or(0) as u16,
                16 => row.get(i * 2..i * 2 + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]])),
                _ => {
                    let bit = i * bpc;
                    let byte = row.get(bit / 8).copied().unwrap_or(0);
                    let shift = 8 - bpc - bit % 8;
                    ((byte >> shift) & ((1u16 << bpc) - 1) as u8) as u16
                }
            };
            out.push(v);
        }
    }
    out
}

fn rgb(f: &PdfFile, cs: &Obj, data: &[u8], w: usize, h: usize, bpc: usize, decode: &[f64]) -> Option<Pixels> {
    let max = ((1u32 << bpc) - 1) as f64;
    let (kind, n, extra) = space(f, cs);
    if kind == Space::Indexed {
        let (base, hival, lookup) = extra?;
        let (bk, bn, _) = space(f, &base);
        let samples = unpack(data, w, h, 1, bpc);
        let mut out = Vec::with_capacity(w * h * 3);
        for s in samples {
            let i = (s as usize).min(hival) * bn;
            let comps: Vec<f64> = (0..bn).map(|k| lookup.get(i + k).copied().unwrap_or(0) as f64 / 255.0).collect();
            out.extend_from_slice(&to_rgb(bk, &comps));
        }
        out.resize(w * h * 3, 0);
        return Some(Pixels { w, h, channels: 3, data: out });
    }
    let samples = unpack(data, w, h, n, bpc);
    let mut out = Vec::with_capacity(w * h * 3);
    let mut comps = vec![0.0; n];
    for px in samples.chunks(n) {
        for k in 0..n {
            let v = *px.get(k).unwrap_or(&0) as f64 / max;
            comps[k] = match (decode.get(2 * k), decode.get(2 * k + 1)) {
                (Some(lo), Some(hi)) => lo + v * (hi - lo),
                _ => v,
            };
        }
        out.extend_from_slice(&to_rgb(kind, &comps));
    }
    out.resize(w * h * 3, 0);
    Some(Pixels { w, h, channels: 3, data: out })
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Separation or DeviceN: tints of ink.
    Tint,
    Indexed,
    Pattern,
}

type IndexedParts = Option<(Obj, usize, Vec<u8>)>;

/// A colour space's kind, number of components and, for indexed spaces,
/// the base space, highest index and lookup table.
pub fn space(f: &PdfFile, cs: &Obj) -> (Space, usize, IndexedParts) {
    space_in(f, cs, 0)
}

/// `space` `depth` spaces down: one naming itself as its base stops.
fn space_in(f: &PdfFile, cs: &Obj, depth: usize) -> (Space, usize, IndexedParts) {
    if depth > 8 {
        return (Space::Rgb, 3, None);
    }
    match f.resolve(cs) {
        Obj::Name(n) => match &*n {
            "DeviceRGB" | "RGB" | "CalRGB" => (Space::Rgb, 3, None),
            "DeviceCMYK" | "CMYK" => (Space::Cmyk, 4, None),
            "Pattern" => (Space::Pattern, 1, None),
            _ => (Space::Gray, 1, None),
        },
        Obj::Array(a) => {
            let kind = a.first().and_then(Obj::name).unwrap_or("");
            match kind {
                "ICCBased" => {
                    let n = a.get(1).map(|o| f.resolve(o)).and_then(|o| o.dict().and_then(|d| f.key_num(d, "N"))).unwrap_or(3.0) as usize;
                    match n {
                        1 => (Space::Gray, 1, None),
                        4 => (Space::Cmyk, 4, None),
                        _ => (Space::Rgb, 3, None),
                    }
                }
                "CalRGB" | "Lab" => (Space::Rgb, 3, None),
                "CalGray" => (Space::Gray, 1, None),
                "Indexed" | "I" => {
                    let base = a.get(1).cloned().unwrap_or(Obj::Null);
                    let hival = a.get(2).and_then(|o| f.resolve(o).int()).unwrap_or(255).clamp(0, 255) as usize;
                    let lookup = match a.get(3).map(|o| f.resolve(o)) {
                        Some(Obj::Str(s)) => s,
                        Some(Obj::Stream(d, raw)) => f.decode(&d, &raw).data,
                        _ => Vec::new(),
                    };
                    (Space::Indexed, 1, Some((base, hival, lookup)))
                }
                "Separation" => (Space::Tint, 1, None),
                // At least one component: none would split the samples by zero; at
                // most the 32 the format allows.
                "DeviceN" => (Space::Tint, a.get(1).map(|o| f.resolve(o)).and_then(|o| o.array().map(<[Obj]>::len)).unwrap_or(1).clamp(1, 32), None),
                "Pattern" => (Space::Pattern, 1, None),
                _ => space_in(f, a.first().unwrap_or(&Obj::Null), depth + 1),
            }
        }
        _ => (Space::Gray, 1, None),
    }
}

/// Components (0 to 1) as 8-bit RGB.
pub fn to_rgb(kind: Space, c: &[f64]) -> [u8; 3] {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = |i: usize| c.get(i).copied().unwrap_or(0.0);
    match kind {
        Space::Rgb => [b(g(0)), b(g(1)), b(g(2))],
        Space::Cmyk => {
            let k = g(3);
            [b((1.0 - g(0)) * (1.0 - k)), b((1.0 - g(1)) * (1.0 - k)), b((1.0 - g(2)) * (1.0 - k))]
        }
        Space::Tint => {
            let t = c.iter().copied().fold(0.0, f64::max);
            [b(1.0 - t); 3]
        }
        Space::Pattern => [128; 3],
        _ => [b(g(0)); 3],
    }
}

/// RGBA8 pixels as a PNG file.
pub(crate) fn png_rgba(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    png(&Pixels { w, h, channels: 4, data: rgba.to_vec() })
}

fn png(px: &Pixels) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crate::zip::crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let mut raw = Vec::with_capacity(px.h * (px.w * px.channels + 1));
    for row in px.data.chunks(px.w * px.channels).take(px.h) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(px.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(px.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if px.channels == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_space_naming_itself_stops() {
        let mut objs = HashMap::new();
        objs.insert(1, Obj::Array(vec![Obj::Ref(1, 0)]));
        let f = PdfFile::of_objects(objs, Dict(Vec::new()));
        assert_eq!(space(&f, &Obj::Ref(1, 0)).1, 3);
    }
}
