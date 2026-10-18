//! Drawings of .xls files (OfficeArt): the pictures kept once for the
//! workbook (`MSODRAWINGGROUP`, a BStore of BLIPs) and the shapes of each
//! sheet (`MSODRAWING`) with their cell anchors. Metafile pictures (EMF,
//! WMF, PICT) are not read.

use std::sync::Arc;

use apex_sheet::{Anchor, CellRef, ImageFormat, Sheet, SheetImage};

use crate::biff::Cur;

/// Records inside one container at most (hostile files).
const MAX_RECORDS: usize = 100_000;

/// A record header: version, instance, type and length.
fn header(c: &mut Cur) -> Option<(u16, u16, u16, usize)> {
    let vi = c.u16()?;
    let ty = c.u16()?;
    let len = c.u32()? as usize;
    Some((vi & 15, vi >> 4, ty, len))
}

/// The records of a container: (instance, type, data).
fn children(d: &[u8]) -> Vec<(u16, u16, &[u8])> {
    let mut c = Cur::new(d);
    let mut out = Vec::new();
    while out.len() < MAX_RECORDS {
        let Some((_, inst, ty, len)) = header(&mut c) else { break };
        let data = c.bytes(len.min(c.left())).unwrap_or(&[]);
        out.push((inst, ty, data));
    }
    out
}

/// A device-independent bitmap as a .bmp file.
fn bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let mut c = Cur::new(dib);
    let size = c.u32()?;
    c.skip(10)?;
    let bits = c.u16()?;
    let compression = c.u32()?;
    c.skip(12)?;
    let used = c.u32()?;
    let colors = if used > 0 { used } else if bits <= 8 { 1 << bits } else { 0 };
    let masks = if size == 40 && compression == 3 { 12 } else { 0 };
    let offset = 14u32.checked_add(size)?.checked_add(masks)?.checked_add(colors.checked_mul(4)?)?;
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&(14 + dib.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

/// The pictures of the workbook's BStore, by `pib` - 1.
pub(crate) fn blips(dgg: &[u8]) -> Vec<Option<(Arc<[u8]>, ImageFormat)>> {
    let mut out = Vec::new();
    for (_, ty, data) in children(dgg) {
        let data = if ty == 0xF000 { data } else { continue };
        for (_, ty, store) in children(data) {
            if ty != 0xF001 {
                continue;
            }
            for (_, ty, bse) in children(store) {
                if ty == 0xF007 {
                    out.push(blip(bse));
                }
            }
        }
    }
    out
}

/// The picture embedded in a BStore entry (`OfficeArtFBSE`).
fn blip(bse: &[u8]) -> Option<(Arc<[u8]>, ImageFormat)> {
    let name = *bse.get(33)? as usize;
    let mut c = Cur::new(bse.get(36 + name..)?);
    let (_, inst, ty, len) = header(&mut c)?;
    let data = c.bytes(len.min(c.left()))?;
    // One or two ids, then a tag byte.
    let skip = 16 + usize::from(inst & 1) * 16 + 1;
    let pic = data.get(skip..)?;
    let (bytes, format) = match ty {
        0xF01D | 0xF02A => (pic.to_vec(), ImageFormat::Jpeg),
        0xF01E => (pic.to_vec(), ImageFormat::Png),
        0xF029 => (pic.to_vec(), ImageFormat::Tiff),
        0xF01F => (bmp(pic)?, ImageFormat::Bmp),
        _ => return None,
    };
    let format = ImageFormat::detect(&bytes, "").unwrap_or(format);
    Some((Arc::from(bytes), format))
}

/// One shape of a sheet drawing.
#[derive(Default)]
pub(crate) struct Shape {
    /// `OfficeArtClientAnchorSheet`: flags, left column and offset (1/1024
    /// of the column), top row and offset (1/256 of the row), right column,
    /// offset, bottom row, offset.
    pub anchor: Option<[u16; 9]>,
    /// Inside a group (placed by a child anchor, not kept).
    pub child: bool,
    /// Has client data, so an `OBJ` record goes with it.
    pub client: bool,
    /// The picture (1-based index in the BStore).
    pub pib: Option<u32>,
    pub name: String,
    pub descr: String,
    /// Left, top, right, bottom, fractions.
    pub crop: [f32; 4],
}

/// The shapes of a sheet's drawing (all its `MSODRAWING` data), in order.
pub(crate) fn shapes(dg: &[u8]) -> Vec<Shape> {
    let mut out = Vec::new();
    for (_, ty, data) in children(dg) {
        if ty == 0xF002 {
            walk(data, false, 0, &mut out);
        }
    }
    out
}

/// Groups nested deeper than 32 are skipped: they would overflow the stack.
fn walk(d: &[u8], child: bool, depth: u32, out: &mut Vec<Shape>) {
    if depth > 32 {
        return;
    }
    let mut first = true;
    for (_, ty, data) in children(d) {
        match ty {
            0xF003 => walk(data, child || !first, depth + 1, out),
            0xF004 => {
                // A group's first shape stands for the group itself.
                out.push(shape(data, child && !first));
                first = false;
            }
            _ => {}
        }
    }
}

fn shape(d: &[u8], child: bool) -> Shape {
    let mut s = Shape { child, ..Default::default() };
    for (inst, ty, data) in children(d) {
        match ty {
            0xF010 if data.len() >= 18 => {
                let mut c = Cur::new(data);
                let mut a = [0u16; 9];
                for v in &mut a {
                    *v = c.u16().unwrap_or(0);
                }
                s.anchor = Some(a);
            }
            0xF00F => s.child = true,
            0xF011 => s.client = true,
            0xF00B | 0xF122 => props(data, inst, &mut s),
            _ => {}
        }
    }
    s
}

/// Shape properties (`OfficeArtFOPT`): `inst` fixed entries, then the
/// complex ones' data in the same order.
fn props(d: &[u8], n: u16, s: &mut Shape) {
    let mut c = Cur::new(d);
    let mut fixed = Vec::new();
    for _ in 0..n {
        let (Some(id), Some(v)) = (c.u16(), c.u32()) else { return };
        fixed.push((id, v));
    }
    let fix = |v: u32| (v as i32 as f32 / 65536.0).clamp(0.0, 0.99);
    for (id, v) in fixed {
        let complex = if id & 0x8000 != 0 { c.bytes((v as usize).min(c.left())) } else { None };
        let text = || complex.map(|b| String::from_utf16_lossy(&b.chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]])).collect::<Vec<_>>()).trim_end_matches('\0').to_string());
        match id & 0x3FFF {
            0x0104 => s.pib = Some(v),
            0x0100 => s.crop[1] = fix(v),
            0x0101 => s.crop[3] = fix(v),
            0x0102 => s.crop[0] = fix(v),
            0x0103 => s.crop[2] = fix(v),
            0x0380 => s.name = text().unwrap_or_default(),
            0x0381 => s.descr = text().unwrap_or_default(),
            _ => {}
        }
    }
}

/// Where an anchor puts a shape: its cell, the offset inside it (sheet
/// pixels), its size (real pixels, as .xlsx anchors are read) and how it
/// follows the cells.
pub(crate) fn place(sh: &Sheet, a: &[u16; 9]) -> (CellRef, f32, f32, f32, f32, Anchor) {
    let (col1, row1, col2, row2) = (u32::from(a[1]), u32::from(a[3]), u32::from(a[5]), u32::from(a[7]));
    let x = |col: u32, off: u16| {
        let w = sh.col_x(col + 1) - sh.col_x(col);
        (sh.col_x(col) + w * f32::from(off.min(1024)) / 1024.0, w * f32::from(off.min(1024)) / 1024.0)
    };
    let y = |row: u32, off: u16| {
        let h = sh.row_y(row + 1) - sh.row_y(row);
        (sh.row_y(row) + h * f32::from(off.min(256)) / 256.0, h * f32::from(off.min(256)) / 256.0)
    };
    let ((x1, dx), (y1, dy)) = (x(col1, a[2]), y(row1, a[4]));
    let ((x2, _), (y2, _)) = (x(col2, a[6]), y(row2, a[8]));
    let w = (x2 - x1) / crate::xlsx_drawing::SX;
    let h = (y2 - y1) / crate::xlsx_drawing::SY;
    let anchor = match a[0] & 3 {
        2 => Anchor::OneCell,
        3 => Anchor::Absolute,
        _ => Anchor::TwoCell,
    };
    (CellRef::new(row1, col1), dx, dy, w, h, anchor)
}

/// The picture of a shape, placed on the sheet.
pub(crate) fn image(sh: &Sheet, s: &Shape, blips: &[Option<(Arc<[u8]>, ImageFormat)>]) -> Option<SheetImage> {
    let (data, format) = blips.get((s.pib? as usize).checked_sub(1)?)?.clone()?;
    if s.child {
        return None;
    }
    let (from, dx, dy, w, h, anchor) = place(sh, s.anchor.as_ref()?);
    let mut img = SheetImage::new(data, format, from, w, h);
    (img.dx, img.dy, img.anchor) = (dx, dy, anchor);
    img.name = s.name.clone();
    img.alt = s.descr.clone();
    img.crop = s.crop;
    img.sanitize();
    Some(img)
}
