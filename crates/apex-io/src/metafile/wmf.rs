//! WMF records (16-bit; with or without the placeable header).

use vello_cpu::kurbo::{Ellipse, Point, Rect, Vec2};

use super::{Brush, Gdi, Header, LogFont, Obj, Pen, Rc, colorref, dib};

const PLACEABLE: [u8; 4] = [0xD7, 0xCD, 0xC6, 0x9A];

pub(super) fn is_wmf(d: &[u8]) -> bool {
    d.starts_with(&PLACEABLE) || (d.len() >= 18 && matches!(u16_at(d, 0), 1 | 2) && u16_at(d, 2) == 9 && matches!(u16_at(d, 4), 0x100 | 0x300))
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    d.get(o..o + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn i16_at(d: &[u8], o: usize) -> f64 {
    u16_at(d, o) as i16 as f64
}

/// Where the records start.
fn records_at(d: &[u8]) -> usize {
    let h = if d.starts_with(&PLACEABLE) { 22 } else { 0 };
    h + 2 * u16_at(d, h + 2).max(9) as usize
}

pub(super) fn header(d: &[u8]) -> Option<Header> {
    if !is_wmf(d) {
        return None;
    }
    if d.starts_with(&PLACEABLE) {
        let r = Rect::new(i16_at(d, 6), i16_at(d, 8), i16_at(d, 10), i16_at(d, 12)).abs();
        let inch = match u16_at(d, 14) {
            0 => 1440.0,
            n => n as f64,
        };
        if r.width() > 0.0 && r.height() > 0.0 {
            return Some(Header { frame: r, size_pt: (r.width() / inch * 72.0, r.height() / inch * 72.0), px_per_mm: (96.0 / 25.4, 96.0 / 25.4) });
        }
    }
    // No size given: the first window, in twips.
    let r = window(d)?;
    Some(Header { frame: r, size_pt: (r.width() / 20.0, r.height() / 20.0), px_per_mm: (96.0 / 25.4, 96.0 / 25.4) })
}

/// The first window the records set, in logical units.
fn window(d: &[u8]) -> Option<Rect> {
    let (mut org, mut ext) = (Vec2::ZERO, None);
    let mut p = records_at(d);
    let mut n = 0;
    while p + 6 <= d.len() && n < 10_000 {
        let size = u32::from_le_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]) as usize * 2;
        let f = u16_at(d, p + 4);
        match f {
            0x020B => org = Vec2::new(i16_at(d, p + 8), i16_at(d, p + 6)),
            0x020C => {
                ext = Some(Vec2::new(i16_at(d, p + 8), i16_at(d, p + 6)));
                break;
            }
            0 => break,
            _ => {}
        }
        if size < 6 {
            break;
        }
        p += size;
        n += 1;
    }
    let e = ext?;
    let r = Rect::from_origin_size(org.to_point(), (e.x, e.y)).abs();
    (r.width() > 0.0 && r.height() > 0.0).then_some(r)
}

/// A WMF without the placeable header (what RTF and the clipboard hold)
/// with one saying its size, `wpt` points wide: its window alone doesn't.
pub(super) fn placeable(d: &[u8], wpt: f64) -> Option<Vec<u8>> {
    if d.starts_with(&PLACEABLE) || !is_wmf(d) || !(wpt > 0.0) {
        return None;
    }
    let r = window(d)?;
    let inch = (r.width() / (wpt / 72.0)).round();
    let fits = |v: f64| (i16::MIN as f64..=i16::MAX as f64).contains(&v);
    if !(1.0..=u16::MAX as f64).contains(&inch) || ![r.x0, r.y0, r.x1, r.y1].into_iter().all(fits) {
        return None;
    }
    let mut words = [0x9AC6CDD7u32 as u16, (0x9AC6CDD7u32 >> 16) as u16, 0, r.x0 as i16 as u16, r.y0 as i16 as u16, r.x1 as i16 as u16, r.y1 as i16 as u16, inch as u16, 0, 0, 0];
    words[10] = words[..10].iter().fold(0, |a, w| a ^ w);
    let mut out: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    out.extend_from_slice(d);
    Some(out)
}

pub(super) fn play(d: &[u8], g: &mut Gdi) {
    let mut p = records_at(d);
    let mut n = 0;
    while p + 6 <= d.len() && n < super::MAX_RECORDS {
        let size = u32::from_le_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]) as usize * 2;
        let f = u16_at(d, p + 4);
        if f == 0 || size < 6 {
            break;
        }
        let r = &d[p..(p + size).min(d.len())];
        record(g, f, r);
        p += size;
        n += 1;
    }
}

/// A packed DIB (BITMAPINFO then the bits).
fn packed_dib(d: &[u8]) -> Option<super::Bitmap> {
    let hsize = u32::from_le_bytes(d.get(0..4)?.try_into().ok()?) as usize;
    let colors = if hsize == 12 {
        let bpp = u16_at(d, 10);
        if bpp <= 8 { 3 << bpp } else { 0 }
    } else {
        let bpp = u16_at(d, 14);
        let comp = u32::from_le_bytes(d.get(16..20)?.try_into().ok()?);
        let used = u32::from_le_bytes(d.get(32..36)?.try_into().ok()?) as usize;
        let n = if bpp <= 8 { if used > 0 { used.min(256) } else { 1 << bpp } } else { 0 };
        n * 4 + if comp == 3 && hsize == 40 { 12 } else { 0 }
    };
    let split = hsize.checked_add(colors)?;
    dib(d.get(..split)?, d.get(split..)?)
}

fn record(g: &mut Gdi, f: u16, r: &[u8]) {
    // Parameters: 16-bit words from offset 6.
    let w = |k: usize| i16_at(r, 6 + 2 * k);
    let uw = |k: usize| u16_at(r, 6 + 2 * k);
    let dw = |k: usize| uw(k) as u32 | (uw(k + 1) as u32) << 16;
    let words = r.len().saturating_sub(6) / 2;
    match f {
        0x0102 => g.dc.bk_opaque = uw(0) == 2,
        0x0106 => g.dc.winding = uw(0) == 2,
        0x0201 => g.dc.bk_color = colorref(dw(0)),
        0x0209 => g.dc.text_color = colorref(dw(0)),
        0x020B => g.dc.win_org = Vec2::new(w(1), w(0)),
        0x020C => {
            g.dc.win_ext = Vec2::new(w(1), w(0));
            g.dc.win_set = true;
        }
        0x020F => g.dc.win_org += Vec2::new(w(1), w(0)),
        0x0410 => {
            let (yd, yn, xd, xn) = (w(0), w(1), w(2), w(3));
            if xd != 0.0 && yd != 0.0 {
                g.dc.win_ext = Vec2::new(g.dc.win_ext.x * xn / xd, g.dc.win_ext.y * yn / yd);
            }
        }
        0x0213 => g.line_to(Point::new(w(1), w(0))),
        0x0214 => g.move_to(Point::new(w(1), w(0))),
        0x0415 => g.exclude_rect(rect(w(3), w(2), w(1), w(0))),
        0x0416 => g.intersect_rect(rect(w(3), w(2), w(1), w(0))),
        0x0418 => g.shape(Ellipse::from_rect(rect(w(3), w(2), w(1), w(0))), true),
        0x041B => g.shape(rect(w(3), w(2), w(1), w(0)), true),
        0x061C => g.round_rect(rect(w(5), w(4), w(3), w(2)), Vec2::new(w(1), w(0))),
        0x0817 | 0x081A | 0x0830 => {
            let kind = match f {
                0x0817 => 0,
                0x0830 => 1,
                _ => 2,
            };
            g.arc(rect(w(7), w(6), w(5), w(4)), Point::new(w(3), w(2)), Point::new(w(1), w(0)), kind);
        }
        0x061D => g.pat_blt(Rect::new(w(5), w(4), w(5) + w(3), w(4) + w(2)), dw(0)),
        0x001E => g.save(),
        0x0127 => g.restore(w(0) as i32),
        0x041F => {
            let c = colorref(dw(0));
            let (y, x) = (w(2), w(3));
            g.fill_rect_color(Rect::new(x, y, x + 1.0, y + 1.0), c);
        }
        0x0324 | 0x0325 => {
            let n = (uw(0) as usize).min(words.saturating_sub(1) / 2);
            let pts: Vec<Point> = (0..n).map(|k| Point::new(w(1 + 2 * k), w(2 + 2 * k))).collect();
            g.polygon(&pts, f == 0x0324);
        }
        0x0538 => {
            let np = (uw(0) as usize).min(words.saturating_sub(1));
            let counts: Vec<usize> = (0..np).map(|k| uw(1 + k) as usize).collect();
            let mut at = 1 + np;
            let mut polys = Vec::with_capacity(np);
            for c in counts {
                let c = c.min(words.saturating_sub(at) / 2);
                polys.push((0..c).map(|k| Point::new(w(at + 2 * k), w(at + 2 * k + 1))).collect());
                at += 2 * c;
            }
            g.poly_polygon(&polys, true);
        }
        0x012D => g.select(uw(0) as u32),
        0x01F0 => g.delete(uw(0) as u32),
        0x012E => g.dc.text_align = uw(0) as u32,
        0x02FA => g.add(Obj::Pen(Pen { style: uw(0) as u32, width: w(1), color: colorref(dw(3)), dashes: Vec::new() })),
        0x02FC => g.add(Obj::Brush(super::emf::brush(uw(0) as u32, dw(1), uw(3) as u32))),
        0x02FB => g.add(Obj::Font(Rc::new(logfont(r)))),
        0x0142 => {
            let b = r.get(10..).and_then(packed_dib).map(|b| Brush::Pattern(std::sync::Arc::new(vello_cpu::Pixmap::from_parts(b.pixels, b.width, b.height)))).unwrap_or(Brush::Solid([128; 3]));
            g.add(Obj::Brush(b));
        }
        // Objects that take a slot but aren't drawn with: patterns of old
        // bitmaps, palettes, regions.
        0x01F9 => g.add(Obj::Brush(Brush::Solid([128; 3]))),
        0x00F7 | 0x06FF => g.add(Obj::Other),
        0x0521 => {
            let n = uw(0) as usize;
            let bytes = r.get(8..8 + n).unwrap_or(&[]);
            let k = 1 + n.div_ceil(2);
            let charset = g.dc.font.charset;
            let text = super::ansi(bytes, charset);
            g.text(Point::new(w(k + 1), w(k)), &text, None, &[], 0, None);
        }
        0x0A32 => {
            let (y, x, n, opts) = (w(0), w(1), uw(2) as usize, uw(3) as u32);
            let mut at = 4;
            let mut rc = None;
            if opts & 6 != 0 && words >= 8 {
                rc = Some(rect(w(4), w(5), w(6), w(7)));
                at = 8;
            }
            let start = 6 + 2 * at;
            let bytes = r.get(start..start + n).unwrap_or(&[]);
            let dx_at = start + n.div_ceil(2) * 2;
            let dx: Vec<f64> = (0..n).map_while(|k| r.get(dx_at + 2 * k..dx_at + 2 * k + 2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f64)).collect();
            let charset = g.dc.font.charset;
            let text = super::ansi(bytes, charset);
            let dx = if dx.len() == bytes.len() && text.len() == bytes.len() {
                dx
            } else if dx.len() == bytes.len() {
                // Double-byte text: a character's advance is its bytes'.
                fold_dbcs(bytes, &dx, charset)
            } else {
                Vec::new()
            };
            g.text(Point::new(x, y), &text, None, &dx, opts, rc);
        }
        // DIBBitBlt.
        0x0940 => {
            let rop = dw(0);
            if words <= 9 {
                return g.pat_blt(Rect::new(w(8), w(7), w(8) + w(6), w(7) + w(5)), rop);
            }
            let (ys, xs, h, wd, yd, xd) = (w(2), w(3), w(4), w(5), w(6), w(7));
            let Some(b) = r.get(22..).and_then(packed_dib) else { return };
            g.blit(b, Rect::new(xs, ys, xs + wd, ys + h), Rect::new(xd, yd, xd + wd, yd + h), rop, None);
        }
        // DIBStretchBlt.
        0x0B41 => {
            let rop = dw(0);
            let (sh, sw, ys, xs) = (w(2), w(3), w(4), w(5));
            if words <= 11 {
                return g.pat_blt(Rect::new(w(10), w(9), w(10) + w(8), w(9) + w(7)), rop);
            }
            let (dh, dwd, yd, xd) = (w(6), w(7), w(8), w(9));
            let Some(b) = r.get(26..).and_then(packed_dib) else { return };
            g.blit(b, Rect::new(xs, ys, xs + sw, ys + sh), Rect::new(xd, yd, xd + dwd, yd + dh), rop, None);
        }
        // StretchDIB.
        0x0F43 => {
            let rop = dw(0);
            let (sh, sw, ys, xs, dh, dwd, yd, xd) = (w(3), w(4), w(5), w(6), w(7), w(8), w(9), w(10));
            let Some(b) = r.get(28..).and_then(packed_dib) else { return };
            let src = Rect::new(xs, b.height as f64 - ys - sh, xs + sw, b.height as f64 - ys);
            g.blit(b, src, Rect::new(xd, yd, xd + dwd, yd + dh), rop, None);
        }
        // SetDIBitsToDevice.
        0x0D33 => {
            let (ys, xs, h, wd, yd, xd) = (w(3), w(4), w(5), w(6), w(7), w(8));
            let Some(b) = r.get(24..).and_then(packed_dib) else { return };
            let src = Rect::new(xs, b.height as f64 - ys - h, xs + wd, b.height as f64 - ys);
            g.blit(b, src, Rect::new(xd, yd, xd + wd, yd + h), super::SRCCOPY, None);
        }
        _ => {}
    }
}

fn rect(l: f64, t: f64, r: f64, b: f64) -> Rect {
    Rect::new(l, t, r, b)
}

fn logfont(r: &[u8]) -> LogFont {
    let w = |k: usize| i16_at(r, 6 + 2 * k);
    let b = |o: usize| r.get(16 + o).copied().unwrap_or(0);
    let name = r.get(24..(24 + 32).min(r.len())).unwrap_or(&[]);
    let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
    let charset = b(3);
    LogFont {
        height: w(0),
        width: w(1),
        escapement: w(2),
        weight: w(4) as i32,
        italic: b(0) != 0,
        underline: b(1) != 0,
        strike: b(2) != 0,
        charset,
        pitch_family: b(7),
        face: crate::ole::decode_cp(name, super::charset_cp(charset)),
    }
}

/// Advances of double-byte text by character: a lead byte's and its
/// trail's together.
fn fold_dbcs(bytes: &[u8], dx: &[f64], charset: u8) -> Vec<f64> {
    let cp = super::charset_cp(charset);
    let lead = |b: u8| match cp {
        932 => (0x81..=0x9F).contains(&b) || (0xE0..=0xFC).contains(&b),
        936 | 949 | 950 => b >= 0x81,
        _ => false,
    };
    let mut out = Vec::with_capacity(dx.len());
    let mut k = 0;
    while k < bytes.len() {
        if lead(bytes[k]) && k + 1 < bytes.len() {
            out.push(dx[k] + dx[k + 1]);
            k += 2;
        } else {
            out.push(dx[k]);
            k += 1;
        }
    }
    out
}
