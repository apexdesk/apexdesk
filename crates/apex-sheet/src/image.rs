//! Pictures floating over the cells of a sheet.

use std::sync::Arc;

use crate::{CellRef, MAX_COLS, MAX_ROWS, Sheet};

/// Encoding of a picture's bytes, which are kept exactly as read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Tiff,
    Webp,
    Svg,
    Emf,
    Wmf,
}

impl ImageFormat {
    /// Recognizes the format from the first bytes.
    pub fn sniff(data: &[u8]) -> Option<ImageFormat> {
        let starts = |p: &[u8]| data.starts_with(p);
        Some(if starts(b"\x89PNG\r\n\x1a\n") {
            ImageFormat::Png
        } else if starts(&[0xFF, 0xD8, 0xFF]) {
            ImageFormat::Jpeg
        } else if starts(b"GIF87a") || starts(b"GIF89a") {
            ImageFormat::Gif
        } else if starts(b"BM") && data.len() > 26 {
            ImageFormat::Bmp
        } else if starts(b"II*\0") || starts(b"MM\0*") {
            ImageFormat::Tiff
        } else if starts(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
            ImageFormat::Webp
        } else if data.len() > 44 && data[0..4] == [1, 0, 0, 0] && data.get(40..44) == Some(b" EMF") {
            ImageFormat::Emf
        } else if starts(&[0xD7, 0xCD, 0xC6, 0x9A]) || starts(&[1, 0, 9, 0]) || starts(&[2, 0, 9, 0]) {
            ImageFormat::Wmf
        } else {
            let head = String::from_utf8_lossy(&data[..data.len().min(512)]).to_ascii_lowercase();
            if head.contains("<svg") {
                ImageFormat::Svg
            } else {
                return None;
            }
        })
    }

    /// Format from the bytes, else from a file extension.
    pub fn detect(data: &[u8], ext: &str) -> Option<ImageFormat> {
        ImageFormat::sniff(data).or_else(|| ImageFormat::from_ext(ext))
    }

    pub fn from_ext(ext: &str) -> Option<ImageFormat> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" | "jpe" | "jfif" => ImageFormat::Jpeg,
            "gif" => ImageFormat::Gif,
            "bmp" | "dib" => ImageFormat::Bmp,
            "tif" | "tiff" => ImageFormat::Tiff,
            "webp" => ImageFormat::Webp,
            "svg" => ImageFormat::Svg,
            "emf" => ImageFormat::Emf,
            "wmf" => ImageFormat::Wmf,
            _ => return None,
        })
    }

    /// File extension used when saving.
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Gif => "gif",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Webp => "webp",
            ImageFormat::Svg => "svg",
            ImageFormat::Emf => "emf",
            ImageFormat::Wmf => "wmf",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Bmp => "image/bmp",
            ImageFormat::Tiff => "image/tiff",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Svg => "image/svg+xml",
            ImageFormat::Emf => "image/x-emf",
            ImageFormat::Wmf => "image/x-wmf",
        }
    }
}

/// How a picture follows the cells under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Anchor {
    /// Moves with its top-left cell, keeps its size (Excel's default for
    /// pictures).
    #[default]
    OneCell,
    /// Moves and sizes with the cells.
    TwoCell,
    /// Fixed position on the sheet.
    Absolute,
}

/// A picture over the cells. Sizes and offsets are sheet pixels (96 dpi).
#[derive(Clone, PartialEq)]
pub struct SheetImage {
    /// Unique within its sheet (0 = not assigned yet).
    pub id: u32,
    pub name: String,
    /// Alternative text (description).
    pub alt: String,
    pub data: Arc<[u8]>,
    pub format: ImageFormat,
    /// Cell holding the top-left corner and the offset inside it.
    pub from: CellRef,
    pub dx: f32,
    pub dy: f32,
    /// Shown size, before rotation.
    pub width: f32,
    pub height: f32,
    pub anchor: Anchor,
    /// Clockwise, in degrees, about the center.
    pub rotation: f32,
    /// Cropped fraction of the source on each side: left, top, right, bottom.
    pub crop: [f32; 4],
    pub lock_aspect: bool,
    /// An embedded object or a diagram shown by this picture.
    pub object: Option<Arc<apex_doc::Embedded>>,
}

impl std::fmt::Debug for SheetImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetImage")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("format", &self.format)
            .field("bytes", &self.data.len())
            .field("from", &self.from)
            .field("offset", &(self.dx, self.dy))
            .field("size", &(self.width, self.height))
            .field("anchor", &self.anchor)
            .field("rotation", &self.rotation)
            .field("crop", &self.crop)
            .field("object", &self.object.as_ref().map(|o| o.id))
            .finish()
    }
}

impl SheetImage {
    pub fn new(data: Arc<[u8]>, format: ImageFormat, from: CellRef, width: f32, height: f32) -> SheetImage {
        SheetImage {
            id: 0,
            name: String::new(),
            alt: String::new(),
            data,
            format,
            from,
            dx: 0.0,
            dy: 0.0,
            width: sane(width, 1.0, 1.0e6, 100.0),
            height: sane(height, 1.0, 1.0e6, 100.0),
            anchor: Anchor::OneCell,
            rotation: 0.0,
            crop: [0.0; 4],
            lock_aspect: true,
            object: None,
        }
    }

    /// Top-left corner (before rotation) in sheet pixels.
    pub fn pos(&self, sh: &Sheet) -> (f32, f32) {
        (sh.col_x(self.from.col) + self.dx, sh.row_y(self.from.row) + self.dy)
    }

    /// Moves the top-left corner to `(x, y)` sheet pixels, re-anchoring it
    /// to the cell there.
    pub fn set_pos(&mut self, sh: &Sheet, x: f32, y: f32) {
        let (x, y) = (sane(x, 0.0, f32::MAX, 0.0), sane(y, 0.0, f32::MAX, 0.0));
        let col = sh.col_at(x).min(MAX_COLS - 1);
        let row = sh.row_at(y).min(MAX_ROWS - 1);
        self.from = CellRef::new(row, col);
        self.dx = (x - sh.col_x(col)).max(0.0);
        self.dy = (y - sh.row_y(row)).max(0.0);
    }

    /// Unrotated rectangle: x, y, width, height.
    pub fn rect(&self, sh: &Sheet) -> [f32; 4] {
        let (x, y) = self.pos(sh);
        [x, y, self.width, self.height]
    }

    /// Bounding box of the rotated picture: min x, min y, max x, max y.
    pub fn bounds(&self, sh: &Sheet) -> [f32; 4] {
        let [x, y, w, h] = self.rect(sh);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (hw, hh) = ((w / 2.0 * c).abs() + (h / 2.0 * s).abs(), (w / 2.0 * s).abs() + (h / 2.0 * c).abs());
        [cx - hw, cy - hh, cx + hw, cy + hh]
    }

    /// Last cell the picture covers.
    pub fn last_cell(&self, sh: &Sheet) -> CellRef {
        let [_, _, x1, y1] = self.bounds(sh);
        CellRef::new(sh.row_at((y1 - 0.5).max(0.0)).min(MAX_ROWS - 1), sh.col_at((x1 - 0.5).max(0.0)).min(MAX_COLS - 1))
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        self.width = sane(self.width, 1.0, 1.0e6, 100.0);
        self.height = sane(self.height, 1.0, 1.0e6, 100.0);
        self.dx = sane(self.dx, 0.0, 1.0e7, 0.0);
        self.dy = sane(self.dy, 0.0, 1.0e7, 0.0);
        let r = if self.rotation.is_finite() { self.rotation.rem_euclid(360.0) + 0.0 } else { 0.0 };
        // Tiny negative angles round up to 360.
        self.rotation = if r >= 360.0 { 0.0 } else { r };
        for c in &mut self.crop {
            *c = sane(*c, 0.0, 0.99, 0.0);
        }
        // Opposite sides must leave something of the picture.
        for (a, b) in [(0, 2), (1, 3)] {
            let sum = self.crop[a] + self.crop[b];
            if sum > 0.99 {
                self.crop[a] *= 0.99 / sum;
                self.crop[b] *= 0.99 / sum;
            }
        }
        self.from = CellRef::new(self.from.row.min(MAX_ROWS - 1), self.from.col.min(MAX_COLS - 1));
    }

    /// Moves the anchor for rows (or columns) inserted (`count` > 0) or
    /// deleted at `at`. A picture anchored in deleted cells moves to the
    /// first cell after them.
    pub(crate) fn shift(&mut self, rows: bool, at: u32, count: i64) {
        if self.anchor == Anchor::Absolute {
            return;
        }
        let (v, max) = if rows { (self.from.row, MAX_ROWS - 1) } else { (self.from.col, MAX_COLS - 1) };
        if v < at {
            return;
        }
        let nv = if count >= 0 {
            (v as i64 + count).min(max as i64) as u32
        } else if (v as i64) < at as i64 - count {
            if rows {
                self.dy = 0.0;
            } else {
                self.dx = 0.0;
            }
            at
        } else {
            (v as i64 + count) as u32
        };
        if rows {
            self.from.row = nv;
        } else {
            self.from.col = nv;
        }
    }
}

/// A picture that is the value of a cell (Excel's Place in Cell): drawn
/// fitted to the cell, keeping its proportions.
#[derive(Clone, PartialEq)]
pub struct CellPicture {
    pub data: Arc<[u8]>,
    pub format: ImageFormat,
    /// Alternative text.
    pub alt: String,
}

impl std::fmt::Debug for CellPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellPicture").field("format", &self.format).field("bytes", &self.data.len()).field("alt", &self.alt).finish()
    }
}

impl CellPicture {
    pub fn new(data: Arc<[u8]>, format: ImageFormat) -> CellPicture {
        CellPicture { data, format, alt: String::new() }
    }

    /// Where the picture goes in a `w`×`h` box: the largest size that fits
    /// with its proportions, centered. Returns x, y offsets and size.
    pub fn fit(&self, w: f32, h: f32) -> [f32; 4] {
        let (pw, ph) = natural_size(&self.data).unwrap_or((w, h));
        let k = (w / pw).min(h / ph);
        let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
        let (fw, fh) = (pw * k, ph * k);
        [(w - fw) / 2.0, (h - fh) / 2.0, fw, fh]
    }
}

fn sane(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

/// Pixel size of a PNG, JPEG, GIF or BMP from its header.
pub fn pixel_size(data: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| data.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le16 = |i: usize| data.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32);
    let le32 = |i: usize| data.get(i..i + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]).unsigned_abs());
    let size = match ImageFormat::sniff(data)? {
        ImageFormat::Png => (be32(16)?, be32(20)?),
        ImageFormat::Gif => (le16(6)?, le16(8)?),
        ImageFormat::Bmp => (le32(18)?, le32(22)?),
        ImageFormat::Jpeg => {
            let mut i = 2;
            loop {
                // Skip fill bytes, then read the marker and its length.
                while data.get(i) == Some(&0xFF) && data.get(i + 1) == Some(&0xFF) {
                    i += 1;
                }
                let (&ff, &m) = (data.get(i)?, data.get(i + 1)?);
                // Not a marker (decoders differ on what follows).
                if ff != 0xFF || matches!(m, 0x00 | 0x01 | 0xD0..=0xD9) {
                    return None;
                }
                let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
                if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                    let h = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]) as u32;
                    let w = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]) as u32;
                    break (w, h);
                }
                if len < 2 {
                    return None;
                }
                i += 2 + len;
            }
        }
        ImageFormat::Webp => {
            let le24 = |i: usize| data.get(i..i + 3).map(|b| u32::from_le_bytes([b[0], b[1], b[2], 0]));
            match data.get(12..16)? {
                b"VP8 " => (le16(26)? & 0x3FFF, le16(28)? & 0x3FFF),
                b"VP8L" => {
                    let b = data.get(21..25)?;
                    let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
                }
                b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
                _ => return None,
            }
        }
        _ => return None,
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// Dots per inch stored in a PNG (pHYs) or JPEG (JFIF) header.
pub fn dpi(data: &[u8]) -> Option<(f32, f32)> {
    let dpi = match ImageFormat::sniff(data)? {
        ImageFormat::Png => {
            // Chunks after the signature: length, type, data, crc.
            let mut i = 8;
            loop {
                let len = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
                let kind = data.get(i + 4..i + 8)?;
                if kind == b"pHYs" {
                    let d = data.get(i + 8..i + 17)?;
                    if d[8] != 1 {
                        return None;
                    }
                    let per_m = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 * 0.0254;
                    break (per_m(&d[0..4]), per_m(&d[4..8]));
                }
                if kind == b"IDAT" || kind == b"IEND" {
                    return None;
                }
                i = i.checked_add(12 + len)?;
            }
        }
        ImageFormat::Jpeg => {
            let d = data.get(2..20)?;
            if d[0..2] != [0xFF, 0xE0] || &d[4..9] != b"JFIF\0" {
                return None;
            }
            let (x, y) = (u16::from_be_bytes([d[12], d[13]]) as f32, u16::from_be_bytes([d[14], d[15]]) as f32);
            match d[11] {
                1 => (x, y),
                2 => (x * 2.54, y * 2.54),
                _ => return None,
            }
        }
        _ => return None,
    };
    (dpi.0 >= 1.0 && dpi.1 >= 1.0).then_some(dpi)
}

/// Size the picture asks for, in sheet pixels (96 dpi): its pixel size
/// scaled by its resolution.
pub fn natural_size(data: &[u8]) -> Option<(f32, f32)> {
    let (w, h) = pixel_size(data)?;
    let (dx, dy) = dpi(data).unwrap_or((96.0, 96.0));
    Some((w as f32 * 96.0 / dx, h as f32 * 96.0 / dy))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 3, 0, 0, 0, 2, 8, 6, 0, 0, 0,
    ];

    #[test]
    fn webp_sizes_read_from_each_header() {
        let lossy = [b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0".as_slice(), &[0x70, 0x0E, 0, 0x9D, 1, 0x2A, 0xC8, 0, 0x78, 0]].concat();
        assert_eq!(pixel_size(&lossy), Some((200, 120)));
        // 14 bits each, minus one: 200 and 120.
        let bits: u32 = 199 | (119 << 14);
        let lossless = [b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2F".as_slice(), &bits.to_le_bytes()].concat();
        assert_eq!(pixel_size(&lossless), Some((200, 120)));
        let extended = [b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0".as_slice(), &[199, 0, 0, 119, 0, 0]].concat();
        assert_eq!(pixel_size(&extended), Some((200, 120)));
    }

    #[test]
    fn formats_and_sizes() {
        assert_eq!(ImageFormat::sniff(PNG_1X1), Some(ImageFormat::Png));
        assert_eq!(pixel_size(PNG_1X1), Some((3, 2)));
        assert_eq!(ImageFormat::sniff(b"GIF89a\x05\x00\x07\x00"), Some(ImageFormat::Gif));
        assert_eq!(pixel_size(b"GIF89a\x05\x00\x07\x00"), Some((5, 7)));
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8, 0, 20, 0, 30, 3];
        assert_eq!(pixel_size(&jpeg), Some((30, 20)));
        assert_eq!(pixel_size(&jpeg[..10]), None);
        assert_eq!(ImageFormat::sniff(b"<?xml?><svg/>"), Some(ImageFormat::Svg));
        assert_eq!(ImageFormat::detect(b"junk", "JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::detect(b"junk", "txt"), None);
        assert_eq!(pixel_size(b""), None);
        let jfif = [0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 1, 0, 72, 0, 72, 0, 0];
        assert_eq!(dpi(&jfif), Some((72.0, 72.0)));
        let mut png = PNG_1X1.to_vec();
        png.extend_from_slice(&[0, 0, 0, 0]);
        png.extend_from_slice(&[0, 0, 0, 9, b'p', b'H', b'Y', b's', 0, 0, 0x0B, 0x13, 0, 0, 0x0B, 0x13, 1, 0, 0, 0, 0]);
        let (x, _) = dpi(&png).unwrap();
        assert!((x - 72.0).abs() < 0.1, "{x}");
        assert_eq!(natural_size(PNG_1X1), Some((3.0, 2.0)));
    }

    #[test]
    fn geometry_and_shift() {
        let sh = Sheet::new("S");
        let mut img = SheetImage::new(Arc::from(PNG_1X1), ImageFormat::Png, CellRef::new(2, 1), 100.0, 50.0);
        img.dx = 5.0;
        let (x, y) = img.pos(&sh);
        assert_eq!((x, y), (sh.col_x(1) + 5.0, sh.row_y(2)));
        img.set_pos(&sh, x + sh.col_width(1), y + 3.0);
        assert_eq!(img.from, CellRef::new(2, 2));
        assert!((img.dx - 5.0).abs() < 1e-3 && (img.dy - 3.0).abs() < 1e-3);
        img.rotation = 90.0;
        let b = img.bounds(&sh);
        assert!(((b[2] - b[0]) - 50.0).abs() < 1e-3);
        img.shift(true, 0, 3);
        assert_eq!(img.from.row, 5);
        img.shift(true, 4, -2);
        assert_eq!((img.from.row, img.dy), (4, 0.0));
        img.shift(false, 10, 1);
        assert_eq!(img.from.col, 2);
        img.rotation = f32::NAN;
        img.crop = [0.8, 0.0, 0.8, f32::INFINITY];
        img.sanitize();
        assert_eq!(img.rotation, 0.0);
        assert!(img.crop[0] + img.crop[2] <= 0.99 + 1e-6);
    }
}
