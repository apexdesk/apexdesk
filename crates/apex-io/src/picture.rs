//! Pictures as written where a crop can't be kept aside (web pages,
//! e-books, Markdown) or the file is copied as is (PDF): cut to what
//! shows, without the camera's metadata (where it was taken, the camera's
//! serial number, a thumbnail of the uncropped photo).

use std::sync::{Arc, OnceLock};

use apex_doc::Image;

/// Decodes a picture file to RGBA8 pixels: width, height, pixels.
pub type Decoder = fn(&[u8]) -> Option<(u32, u32, Vec<u8>)>;

static DECODE: OnceLock<Decoder> = OnceLock::new();

/// Sets the decoder used to cut cropped pictures (the app's, which reads
/// every picture format). Without one, cropped pictures are left out.
pub fn set_decoder(f: Decoder) {
    let _ = DECODE.set(f);
}

/// Decodes a picture with the decoder set (none: `None`).
pub(crate) fn decode(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    DECODE.get()?(data)
}

/// The file to write for `img`: the part shown, as PNG, when cropped;
/// else its own file, a JPEG without its metadata. `None` when it is
/// cropped and can't be cut.
pub fn shown(img: &Image) -> Option<Arc<[u8]>> {
    let [l, t, r, b] = img.crop.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 });
    if l + t + r + b <= 0.0 {
        return Some(strip_jpeg(&img.data).map_or_else(|| img.data.clone(), Arc::from));
    }
    let (w, h, rgba) = DECODE.get()?(&img.data)?;
    let (wf, hf) = (w as f32, h as f32);
    let (x0, x1) = ((l * wf).floor() as usize, (((1.0 - r) * wf).ceil() as usize).min(w as usize));
    let (y0, y1) = ((t * hf).floor() as usize, (((1.0 - b) * hf).ceil() as usize).min(h as usize));
    if x1 <= x0 || y1 <= y0 || rgba.len() < w as usize * h as usize * 4 {
        return None;
    }
    let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        let row = y * w as usize * 4;
        data.extend_from_slice(&rgba[row + x0 * 4..row + x1 * 4]);
    }
    Some(crate::pdf::image::png_rgba(x1 - x0, y1 - y0, &data).into())
}

/// [`shown`], as a PNG or JPEG file (what LaTeX and most readers take):
/// other formats are decoded and written again as PNG.
pub fn shown_png_or_jpeg(img: &Image) -> Option<Arc<[u8]>> {
    let data = shown(img)?;
    if matches!(crate::docx_img::image_type(&data).0, "png" | "jpeg") {
        return Some(data);
    }
    let (w, h, rgba) = DECODE.get()?(&data)?;
    if rgba.len() < w as usize * h as usize * 4 {
        return None;
    }
    Some(crate::pdf::image::png_rgba(w as usize, h as usize, &rgba).into())
}

/// A JPEG file without the segments that carry metadata (EXIF, XMP,
/// IPTC, comments); the color profile and Adobe's color marker stay.
/// `None` when it isn't a JPEG or has none of those.
pub fn strip_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..2]);
    let mut at = 2;
    let mut dropped = false;
    loop {
        if data.get(at) != Some(&0xFF) {
            return None;
        }
        let marker = *data.get(at + 1)?;
        // Fill bytes.
        if marker == 0xFF {
            at += 1;
            continue;
        }
        // Start of scan: the rest is image data.
        if marker == 0xDA {
            out.extend_from_slice(&data[at..]);
            break;
        }
        let len = u16::from_be_bytes([*data.get(at + 2)?, *data.get(at + 3)?]) as usize;
        let end = at.checked_add(2 + len).filter(|&e| len >= 2 && e <= data.len())?;
        let meta = matches!(marker, 0xE1 | 0xE3..=0xED | 0xEF | 0xFE);
        if meta {
            dropped = true;
        } else {
            out.extend_from_slice(&data[at..end]);
        }
        at = end;
    }
    dropped.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_and_cropped_parts_are_left_out() {
        // SOI, APP0 (JFIF), APP1 (EXIF), COM, SOS and data.
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 1, 2, 0xFF, 0xE1, 0, 6];
        j.extend_from_slice(b"GPS!");
        j.extend_from_slice(&[0xFF, 0xFE, 0, 3, b'x', 0xFF, 0xDA, 0, 2, 9, 9, 0xFF, 0xD9]);
        let s = strip_jpeg(&j).unwrap();
        assert_eq!(s, [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 1, 2, 0xFF, 0xDA, 0, 2, 9, 9, 0xFF, 0xD9]);
        assert!(strip_jpeg(&s).is_none() && strip_jpeg(b"\x89PNG").is_none());
        // A cropped picture with no decoder isn't written.
        let img = Image { crop: [0.5, 0.0, 0.0, 0.0], ..Image::new(Arc::from(&j[..]), 10.0, 10.0) };
        assert!(DECODE.get().is_some() || shown(&img).is_none());
        let whole = Image::new(Arc::from(&j[..]), 10.0, 10.0);
        assert_eq!(&*shown(&whole).unwrap(), &s[..]);
    }
}
