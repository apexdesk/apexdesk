//! Image XObjects and inline images as RGBA pixels.

use std::io::Cursor;

use super::super::file::PdfFile;
use super::super::obj::{Dict, Obj};
use super::color::Cs;

/// Pixels as RGBA (not premultiplied).
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

const MAX_PIXELS: usize = 40_000_000;
/// The most pixels a soft mask (and its image) is kept at.
const MASK_PIXELS: usize = 4_000_000;
/// No size drawn to read an image smaller for.
const NO_SIZE: (f64, f64) = (f64::INFINITY, f64::INFINITY);

/// The pixels of image `d`. `fill` is the colour (RGBA) of stencil masks;
/// `res` the resources for named colour spaces (inline images); `size`
/// about the pixels it is drawn at (huge images are read smaller).
pub fn decode(f: &PdfFile, d: &Dict, raw: &[u8], fill: [u8; 4], res: Option<&Dict>, size: (f64, f64)) -> Option<Rgba> {
    let w = f.key_num(d, "Width").or_else(|| f.key_num(d, "W"))?.max(0.0) as usize;
    let h = f.key_num(d, "Height").or_else(|| f.key_num(d, "H"))?.max(0.0) as usize;
    let mask = matches!(f.key(d, "ImageMask"), Obj::Bool(true)) || matches!(f.key(d, "IM"), Obj::Bool(true));
    // Too many pixels, unless JPEG 2000 (read at a lower resolution).
    let jpx_filter = match f.key(d, "Filter") {
        Obj::Name(n) => &*n == "JPXDecode",
        Obj::Array(a) => a.last().is_some_and(|o| f.resolve(o).name() == Some("JPXDecode")),
        _ => false,
    };
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS && (mask || !jpx_filter) {
        return None;
    }
    let decode_arr: Vec<f32> = f.nums(d.get("Decode").or_else(|| d.get("D")).unwrap_or(&Obj::Null)).into_iter().map(|v| v as f32).collect();
    let mut dec = f.decode(d, raw);
    // Only JPEG 2000 itself is read smaller: another codec before it in the
    // chain would take every pixel.
    if w.saturating_mul(h) > MAX_PIXELS && dec.codec.as_deref() != Some("JPXDecode") {
        return None;
    }
    let bilevel = bilevel(f, d, &mut dec, w, h)?;
    let mut out = if mask {
        let inverted = decode_arr.first().is_some_and(|&v| v > 0.5);
        let samples = unpack(&dec.data, w, h, 1, 1);
        let mut data = Vec::with_capacity(w * h * 4);
        for s in samples {
            let paint = (s == 0) != inverted;
            data.extend_from_slice(&[fill[0], fill[1], fill[2], if paint { fill[3] } else { 0 }]);
        }
        data.resize(w * h * 4, 0);
        Rgba { w, h, data }
    } else {
        let cs_obj = d.get("ColorSpace").or_else(|| d.get("CS")).cloned().unwrap_or(Obj::Null);
        let cs = Cs::load(f, &cs_obj, res);
        match dec.codec.as_deref() {
            Some("DCTDecode" | "DCT") => jpeg(&dec.data, &cs, &decode_arr, size)?,
            Some("JPXDecode") => jpx(&dec.data, &cs, cs_obj != Obj::Null, &decode_arr, !matches!(f.key(d, "SMask"), Obj::Stream(..)), size)?,
            Some(_) => return None,
            None => {
                let bpc = if bilevel { 1 } else { f.key_num(d, "BitsPerComponent").or_else(|| f.key_num(d, "BPC")).unwrap_or(8.0) as usize };
                if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
                    return None;
                }
                let px = samples_rgba(&cs, &dec.data, w, h, bpc, &decode_arr);
                // A colour key mask.
                let mut px = px;
                if let Obj::Array(keys) = f.key(d, "Mask") {
                    let k: Vec<i64> = keys.iter().filter_map(|o| f.resolve(o).int()).collect();
                    let n = cs.n();
                    if k.len() >= 2 * n {
                        let s = unpack(&dec.data, w, h, n, bpc);
                        for (i, pxs) in s.chunks(n).enumerate() {
                            if pxs.iter().enumerate().all(|(j, &v)| (k[2 * j]..=k[2 * j + 1]).contains(&(v as i64))) {
                                px.data[i * 4 + 3] = 0;
                            }
                        }
                    }
                }
                px
            }
        }
    };
    // Soft mask, or a stencil mask.
    let alpha = match f.key(d, "SMask") {
        Obj::Stream(sd, sraw) => gray(f, &sd, &sraw, false),
        _ => match f.key(d, "Mask") {
            Obj::Stream(sd, sraw) => gray(f, &sd, &sraw, true),
            _ => None,
        },
    };
    if let Some(a) = alpha {
        apply_alpha(&mut out, &a);
    }
    Some(out)
}

/// A soft mask's (or stencil mask's) values, 0 to 255.
fn gray(f: &PdfFile, d: &Dict, raw: &[u8], stencil: bool) -> Option<Rgba> {
    let w = f.key_num(d, "Width")?.max(0.0) as usize;
    let h = f.key_num(d, "Height")?.max(0.0) as usize;
    if w == 0 || h == 0 {
        return None;
    }
    // A huge mask is averaged down by a whole factor as it is read.
    let k = if w.saturating_mul(h) > MAX_PIXELS { (w as f64 * h as f64 / MASK_PIXELS as f64).sqrt().ceil() as usize } else { 1 };
    // Reading still goes over every pixel (and CCITT and JBIG2 masks are
    // decoded whole first): past 64 times the kept size, no mask.
    if k > 8 {
        return None;
    }
    let decode_arr: Vec<f32> = f.nums(d.get("Decode").unwrap_or(&Obj::Null)).into_iter().map(|v| v as f32).collect();
    let mut dec = f.decode(d, raw);
    let bilevel = bilevel(f, d, &mut dec, w, h)?;
    if stencil {
        let inverted = decode_arr.first().is_some_and(|&v| v > 0.5);
        return Some(reduce(&dec.data, w, h, 1, k, |s| if (s == 0) != inverted { 255 } else { 0 }));
    }
    match dec.codec.as_deref() {
        Some("DCTDecode" | "DCT") if k == 1 => Some(Rgba { w, h, data: jpeg(&dec.data, &Cs::Gray, &decode_arr, NO_SIZE)?.data.chunks_exact(4).map(|p| p[0]).collect() }),
        Some("JPXDecode") if k == 1 => Some(Rgba { w, h, data: jpx(&dec.data, &Cs::Gray, true, &decode_arr, false, NO_SIZE)?.data.chunks_exact(4).map(|p| p[0]).collect() }),
        Some(_) => None,
        None => {
            let bpc = if bilevel { 1 } else { f.key_num(d, "BitsPerComponent").unwrap_or(8.0) as usize };
            if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
                return None;
            }
            let max = ((1u32 << bpc) - 1) as f32;
            let (lo, hi) = (decode_arr.first().copied().unwrap_or(0.0), decode_arr.get(1).copied().unwrap_or(1.0));
            Some(reduce(&dec.data, w, h, bpc, k, |s| ((lo + s as f32 / max * (hi - lo)).clamp(0.0, 1.0) * 255.0).round() as u8))
        }
    }
}

/// One-channel samples mapped to 0 to 255, each `k` x `k` block averaged.
fn reduce(data: &[u8], w: usize, h: usize, bpc: usize, k: usize, map: impl Fn(u16) -> u8) -> Rgba {
    // Samples of 8 bits and fewer: mapped once each.
    let lut: Vec<u8> = if bpc <= 8 { (0..1u16 << bpc).map(&map).collect() } else { Vec::new() };
    let map = |v: u16| lut.get(v as usize).copied().unwrap_or_else(|| map(v));
    if k == 1 {
        let mut data: Vec<u8> = unpack(data, w, h, 1, bpc).into_iter().map(map).collect();
        data.resize(w * h, map(0));
        return Rgba { w, h, data };
    }
    let (nw, nh) = (w.div_ceil(k), h.div_ceil(k));
    let mut sum = vec![0u32; nw * nh];
    let row_bytes = (w * bpc).div_ceil(8);
    for y in 0..h {
        let row = data.get(y * row_bytes..(y + 1) * row_bytes).unwrap_or(&[]);
        let base = y / k * nw;
        for x in 0..w {
            let v = match bpc {
                8 => row.get(x).copied().unwrap_or(0) as u16,
                16 => row.get(x * 2..x * 2 + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]])),
                _ => {
                    let bit = x * bpc;
                    let byte = row.get(bit / 8).copied().unwrap_or(0);
                    ((byte >> (8 - bpc - bit % 8)) & ((1u16 << bpc) - 1) as u8) as u16
                }
            };
            sum[base + x / k] += map(v) as u32;
        }
    }
    let data = (0..nw * nh)
        .map(|i| {
            let (x, y) = (i % nw, i / nw);
            let n = (((x + 1) * k).min(w) - x * k) * (((y + 1) * k).min(h) - y * k);
            (sum[i] / n.max(1) as u32) as u8
        })
        .collect();
    Rgba { w: nw, h: nh, data }
}

/// Sets the alpha of `px` from mask values (one byte per pixel), scaled to
/// the image's size.
fn apply_alpha(px: &mut Rgba, a: &Rgba) {
    // A finer mask: the image enlarged to it first, so the mask's shape
    // stays (as pdfium does).
    if a.w > px.w && a.h > px.h && a.w * a.h <= MASK_PIXELS {
        let mut big = vec![0u8; a.w * a.h * 4];
        for y in 0..a.h {
            let sy = y * px.h / a.h;
            for x in 0..a.w {
                let s = (sy * px.w + x * px.w / a.w) * 4;
                big[(y * a.w + x) * 4..][..4].copy_from_slice(&px.data[s..s + 4]);
            }
        }
        *px = Rgba { w: a.w, h: a.h, data: big };
    }
    for y in 0..px.h {
        let sy = (y * a.h / px.h).min(a.h - 1);
        for x in 0..px.w {
            let sx = (x * a.w / px.w).min(a.w - 1);
            let v = a.data[sy * a.w + sx] as u32;
            let i = (y * px.w + x) * 4 + 3;
            px.data[i] = (px.data[i] as u32 * v / 255) as u8;
        }
    }
}

fn jpeg(data: &[u8], cs: &Cs, decode: &[f32], size: (f64, f64)) -> Option<Rgba> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    // From the start of image marker, as pdfium looks for it.
    let data = memchr::memmem::find(data, &[0xFF, 0xD8]).map_or(data, |i| &data[i..]);
    let opts = DecoderOptions::default().set_max_width(1 << 16).set_max_height(1 << 16).set_strict_mode(false);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(data), opts);
    dec.decode_headers().ok()?;
    let input = dec.input_colorspace()?;
    let info = dec.info()?;
    let (w, h) = (info.width as usize, info.height as usize);
    if w.saturating_mul(h) > MAX_PIXELS {
        return None;
    }
    let invert = decode.first().is_some_and(|&v| v > 0.5);
    if input == ColorSpace::CMYK || input == ColorSpace::YCCK {
        // Raw samples (zune's YCCK conversion is broken).
        let opts = DecoderOptions::default().set_max_width(1 << 16).set_max_height(1 << 16).set_strict_mode(false).jpeg_set_out_colorspace(input);
        let mut dec = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(data), opts);
        let mut raw = dec.decode().ok()?;
        if input == ColorSpace::YCCK {
            // YCC to RGB, which is the inverted CMY.
            for p in raw.chunks_exact_mut(4) {
                let (y, cb, cr) = (p[0] as f32, p[1] as f32 - 128.0, p[2] as f32 - 128.0);
                p[0] = 255 - (y + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
                p[1] = 255 - (y - 0.344136 * cb - 0.714136 * cr).round().clamp(0.0, 255.0) as u8;
                p[2] = 255 - (y + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
            }
        }
        if raw.len() != w * h * 4 {
            return None;
        }
        // Drawn much smaller: the inks averaged down first (to twice the
        // size drawn), not each pixel converted.
        let (fx, fy) = (shrink(w, size.0), shrink(h, size.1));
        let (w, h, raw) = if fx > 1 || fy > 1 { box_down(&raw, w, h, fx, fy) } else { (w, h, raw) };
        // Inverted inks (Adobe's) come with a Decode array that says so,
        // as pdf.js and pdfium take it.
        let flip = invert;
        let mut out = Vec::with_capacity(w * h * 4);
        // Recently converted inks, by a hash of them.
        let mut seen = vec![(u32::MAX, [0u8; 3]); 4096];
        for p in raw.chunks_exact(4) {
            let key = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            let slot = &mut seen[(key.wrapping_mul(0x9E37_79B1) >> 20) as usize];
            if slot.0 != key || key == u32::MAX {
                let v = |i: usize| {
                    let b = if flip { 255 - p[i] } else { p[i] };
                    b as f32 / 255.0
                };
                let [r, g, b] = super::color::cmyk(v(0), v(1), v(2), v(3));
                *slot = (key, [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8]);
            }
            let c = slot.1;
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        return Some(Rgba { w, h, data: out });
    }
    let decode_as = |out: ColorSpace| {
        let opts = DecoderOptions::default().set_max_width(1 << 16).set_max_height(1 << 16).set_strict_mode(false).jpeg_set_out_colorspace(out);
        zune_jpeg::JpegDecoder::new_with_options(Cursor::new(data), opts).decode().ok()
    };
    // Components named R, G and B (not converted from YCbCr): zune makes
    // only RGB of them.
    let mut rgba = match decode_as(ColorSpace::RGBA) {
        Some(p) => p,
        None => decode_as(ColorSpace::RGB)?.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
    };
    if rgba.len() != w * h * 4 {
        return None;
    }
    if invert {
        for p in rgba.chunks_exact_mut(4) {
            p[0] = 255 - p[0];
            p[1] = 255 - p[1];
            p[2] = 255 - p[2];
        }
    }
    if let Cs::Icc(icc) = cs {
        icc.convert_rgba(&mut rgba);
    }
    // A JPEG in a colour space of its own (an indexed or separation one is
    // rare): its gray level as the tint.
    if let Cs::Tint { .. } = cs {
        for p in rgba.chunks_exact_mut(4) {
            let [r, g, b] = cs.rgb(&[p[0] as f32 / 255.0]);
            p[0] = (r * 255.0) as u8;
            p[1] = (g * 255.0) as u8;
            p[2] = (b * 255.0) as u8;
        }
    }
    Some(Rgba { w, h, data: rgba })
}

/// The whole factor a side of `n` pixels drawn at `drawn` is read smaller
/// by: to no fewer than twice the pixels drawn.
fn shrink(n: usize, drawn: f64) -> usize {
    if !drawn.is_finite() {
        return 1;
    }
    ((n as f64 / (2.0 * drawn.max(1.0))).floor() as usize).max(1)
}

/// Four-byte pixels averaged over `fx` x `fy` blocks.
fn box_down(px: &[u8], w: usize, h: usize, fx: usize, fy: usize) -> (usize, usize, Vec<u8>) {
    let (nw, nh) = (w.div_ceil(fx), h.div_ceil(fy));
    let mut sum = vec![0u32; nw * nh * 4];
    for y in 0..h {
        let row = &px[y * w * 4..(y + 1) * w * 4];
        let base = y / fy * nw;
        for (x, p) in row.chunks_exact(4).enumerate() {
            let s = &mut sum[(base + x / fx) * 4..][..4];
            for k in 0..4 {
                s[k] += p[k] as u32;
            }
        }
    }
    let mut out = vec![0u8; nw * nh * 4];
    for (i, o) in out.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i % nw, i / nw);
        let n = ((((x + 1) * fx).min(w) - x * fx) * (((y + 1) * fy).min(h) - y * fy)).max(1) as u32;
        for k in 0..4 {
            o[k] = ((sum[i * 4 + k] + n / 2) / n) as u8;
        }
    }
    (nw, nh, out)
}

/// The parameters of the image's codec (the last filter).
fn codec_parms(f: &PdfFile, d: &Dict) -> Dict {
    let o = match f.key(d, "DecodeParms") {
        Obj::Null => f.key(d, "DP"),
        o => o,
    };
    match o {
        Obj::Dict(p) => p,
        Obj::Array(a) => a.last().and_then(|o| f.resolve(o).dict().cloned()).unwrap_or_default(),
        _ => Dict::default(),
    }
}

/// Decodes CCITT and JBIG2 data into 1-bit samples (0 black), in place.
/// Whether it did; None when the data is broken beyond use.
fn bilevel(f: &PdfFile, d: &Dict, dec: &mut super::super::file::Decoded, w: usize, h: usize) -> Option<bool> {
    let codec = match dec.codec.as_deref() {
        Some(c @ ("CCITTFaxDecode" | "CCF" | "JBIG2Decode")) => c.to_string(),
        _ => return Some(false),
    };
    let p = codec_parms(f, d);
    let row = w.div_ceil(8);
    let mut bits = Bits { out: vec![0; row * h], row, w, h, x: 0, y: 0, black_is_1: false };
    if codec == "JBIG2Decode" {
        let globals = match f.key(&p, "JBIG2Globals") {
            Obj::Stream(gd, graw) => Some(f.decode(&gd, &graw).data),
            _ => None,
        };
        let img = hayro_jbig2::Image::new_embedded(&dec.data, globals.as_deref()).ok()?;
        let _ = img.decode(&mut bits);
    } else {
        use hayro_ccitt::{DecodeSettings, DecoderContext, EncodingMode};
        let k = f.key_num(&p, "K").unwrap_or(0.0) as i64;
        let flag = |key: &str, def: bool| match f.key(&p, key) {
            Obj::Bool(b) => b,
            _ => def,
        };
        bits.black_is_1 = flag("BlackIs1", false);
        let columns = f.key_num(&p, "Columns").unwrap_or(1728.0).max(1.0) as u32;
        let rows = f.key_num(&p, "Rows").filter(|&r| r > 0.0).map_or(h as u32, |r| r as u32);
        let settings = DecodeSettings {
            columns,
            rows,
            end_of_block: flag("EndOfBlock", true),
            end_of_line: flag("EndOfLine", false),
            rows_are_byte_aligned: flag("EncodedByteAlign", false),
            encoding: if k < 0 {
                EncodingMode::Group4
            } else if k == 0 {
                EncodingMode::Group3_1D
            } else {
                EncodingMode::Group3_2D { k: k as u32 }
            },
            invert_black: false,
        };
        bits.w = (columns as usize).min(w);
        // Rows decoded before an error still count.
        let _ = hayro_ccitt::decode(&dec.data, &mut bits, &mut DecoderContext::new(settings));
        bits.w = w;
    }
    dec.data = bits.out;
    dec.codec = None;
    Some(true)
}

/// Bilevel pixels into packed rows: the bit is 1 for white (black with
/// BlackIs1), the way the image's samples read.
struct Bits {
    out: Vec<u8>,
    row: usize,
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    black_is_1: bool,
}

impl Bits {
    fn push(&mut self, black: bool, count: usize) {
        let one = black == self.black_is_1;
        if self.y >= self.h {
            return;
        }
        let end = (self.x + count).min(self.w);
        if one {
            let base = self.y * self.row;
            for x in self.x..end {
                self.out[base + x / 8] |= 0x80 >> (x % 8);
            }
        }
        self.x += count;
    }

    fn line(&mut self) {
        self.x = 0;
        self.y += 1;
    }
}

impl hayro_ccitt::Decoder for Bits {
    fn push_pixels(&mut self, white: bool, count: u32) {
        self.push(!white, count as usize);
    }
    fn next_line(&mut self) {
        self.line();
    }
}

impl hayro_jbig2::Decoder for Bits {
    fn push_pixel(&mut self, black: bool) {
        self.push(black, 1);
    }
    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        self.push(black, chunk_count as usize * 8);
    }
    fn next_line(&mut self) {
        self.line();
    }
}

/// A JPEG 2000 image. Without a colour space of the PDF's, the one the
/// image has; its own alpha unless the PDF gives a soft mask.
fn jpx(data: &[u8], cs: &Cs, pdf_cs: bool, decode: &[f32], own_alpha: bool, size: (f64, f64)) -> Option<Rgba> {
    // An indexed space in the PDF: the image's own palette left out, its
    // samples the indices (as pdfium decodes it).
    let indexed = pdf_cs && matches!(cs, Cs::Indexed { .. });
    let base = hayro_jpeg2000::DecodeSettings { resolve_palette_indices: !indexed, ..Default::default() };
    let mut img = hayro_jpeg2000::Image::new(data, &base).ok()?;
    let (w, h) = (img.width(), img.height());
    // A huge image, or one drawn much smaller: at a resolution level of
    // few enough pixels (no fewer than those drawn; a level is already
    // filtered down).
    let mut k = 1;
    if w as usize * h as usize > MAX_PIXELS {
        k = ((w as f64 * h as f64 / MAX_PIXELS as f64).sqrt().ceil() as u32).next_power_of_two();
    }
    let fit = shrink(w as usize, size.0 / 2.0).min(shrink(h as usize, size.1 / 2.0)).min(1 << 10) as u32;
    k = k.max(if fit > 1 { 1 << fit.ilog2() } else { 1 });
    if k > 1 {
        let settings = hayro_jpeg2000::DecodeSettings { target_resolution: Some((w / k, h / k)), ..base };
        img = hayro_jpeg2000::Image::new(data, &settings).ok()?;
    }
    let depth = img.original_bit_depth().clamp(1, 8) as u32;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
        return None;
    }
    let alpha = img.has_alpha();
    let mut ctx = hayro_jpeg2000::DecoderContext::default();
    let mut buf = img.decode(&mut ctx).ok()?.data_u8();
    // Indices of fewer bits came scaled to 8.
    if indexed && depth < 8 {
        let max = (1u32 << depth) - 1;
        buf.iter_mut().for_each(|v| *v = ((*v as u32 * max + 127) / 255) as u8);
    }
    let n = buf.len() / (w * h);
    if n == 0 {
        return None;
    }
    let cn = if alpha && n > 1 { n - 1 } else { n };
    let space = if pdf_cs && cs.n() == cn {
        cs.clone()
    } else {
        match cn {
            1 => Cs::Gray,
            4 => Cs::Cmyk,
            _ => Cs::Rgb,
        }
    };
    let color: Vec<u8> = if cn == n { std::mem::take(&mut buf) } else { buf.chunks_exact(n).flat_map(|p| p[..cn].iter().copied()).collect() };
    let mut out = if space.n() == cn {
        samples_rgba(&space, &color, w, h, 8, decode)
    } else {
        // Fewer components than the space wants: as gray or RGB.
        samples_rgba(if cn >= 3 { &Cs::Rgb } else { &Cs::Gray }, &color, w, h, 8, decode)
    };
    if alpha && own_alpha && n > cn {
        for (o, p) in out.data.chunks_exact_mut(4).zip(buf.chunks_exact(n)) {
            o[3] = p[n - 1];
        }
    }
    Some(out)
}

/// Samples of `n` components of `bpc` bits, rows padded to bytes.
pub fn unpack(data: &[u8], w: usize, h: usize, n: usize, bpc: usize) -> Vec<u16> {
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

fn samples_rgba(cs: &Cs, data: &[u8], w: usize, h: usize, bpc: usize, decode: &[f32]) -> Rgba {
    let n = cs.n();
    let max = ((1u32 << bpc) - 1) as f32;
    let ranges: Vec<(f32, f32)> = (0..n)
        .map(|k| match (decode.get(2 * k), decode.get(2 * k + 1)) {
            (Some(&lo), Some(&hi)) => (lo, hi),
            _ => cs.decode_range(k, bpc),
        })
        .collect();
    // 8-bit RGB as it is (or through its profile's tables).
    if bpc == 8 && n == 3 && matches!(cs, Cs::Rgb | Cs::Icc(_)) && ranges.iter().all(|&r| r == (0.0, 1.0)) && data.len() >= w * h * 3 {
        let mut out: Vec<u8> = data[..w * h * 3].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        if let Cs::Icc(icc) = cs {
            icc.convert_rgba(&mut out);
        }
        return Rgba { w, h, data: out };
    }
    let samples = unpack(data, w, h, n, bpc);
    let mut out = Vec::with_capacity(w * h * 4);
    // Few distinct values (8 bits and less, one component): a table.
    let table: Option<Vec<[u8; 3]>> = (n == 1 && bpc <= 8).then(|| {
        (0..=max as u32)
            .map(|s| {
                let (lo, hi) = ranges[0];
                let v = lo + s as f32 / max * (hi - lo);
                let [r, g, b] = cs.rgb(&[v]);
                [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8]
            })
            .collect()
    });
    let mut comps = vec![0.0f32; n];
    // The last pixel converted.
    let mut memo: (Vec<u16>, [u8; 3]) = (Vec::new(), [0; 3]);
    for px in samples.chunks(n) {
        if let Some(t) = &table {
            let c = t[(px[0] as usize).min(t.len() - 1)];
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
            continue;
        }
        if memo.0.as_slice() == px {
            let c = memo.1;
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
            continue;
        }
        for k in 0..n {
            let v = *px.get(k).unwrap_or(&0) as f32 / max;
            let (lo, hi) = ranges[k];
            comps[k] = lo + v * (hi - lo);
        }
        let [r, g, b] = cs.rgb(&comps);
        let c = [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8];
        out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        memo.0.clear();
        memo.0.extend_from_slice(px);
        memo.1 = c;
    }
    out.resize(w * h * 4, 0);
    Rgba { w, h, data: out }
}

