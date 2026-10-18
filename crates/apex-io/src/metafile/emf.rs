//! EMF records.

use vello_cpu::kurbo::{Affine, Point, Rect, Vec2};

use super::{Brush, Gdi, Header, LogFont, Obj, Pen, Rc, colorref, dib};

pub(super) fn is_emf(d: &[u8]) -> bool {
    d.len() >= 88 && d[..4] == [1, 0, 0, 0] && d.get(40..44) == Some(b" EMF")
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn i32_at(d: &[u8], o: usize) -> i32 {
    u32_at(d, o) as i32
}

fn f32_at(d: &[u8], o: usize) -> f64 {
    let v = f32::from_bits(u32_at(d, o)) as f64;
    if v.is_finite() { v } else { 0.0 }
}

fn rect_at(d: &[u8], o: usize) -> Rect {
    Rect::new(i32_at(d, o) as f64, i32_at(d, o + 4) as f64, i32_at(d, o + 8) as f64, i32_at(d, o + 12) as f64)
}

fn point_at(d: &[u8], o: usize) -> Point {
    Point::new(i32_at(d, o) as f64, i32_at(d, o + 4) as f64)
}

pub(super) fn header(d: &[u8]) -> Option<Header> {
    if !is_emf(d) {
        return None;
    }
    let bounds = rect_at(d, 8);
    let frame = rect_at(d, 24);
    let (dev_w, dev_h) = (i32_at(d, 72) as f64, i32_at(d, 76) as f64);
    let (mm_w, mm_h) = (i32_at(d, 80) as f64, i32_at(d, 84) as f64);
    let px_per_mm = if dev_w > 0.0 && dev_h > 0.0 && mm_w > 0.0 && mm_h > 0.0 { (dev_w / mm_w, dev_h / mm_h) } else { (96.0 / 25.4, 96.0 / 25.4) };
    // The frame (0.01 mm, inclusive) on the reference device; else the
    // bounds (pixels, inclusive).
    let (box_, size_pt) = if frame.width() > 0.0 && frame.height() > 0.0 {
        let r = Rect::new(frame.x0 * px_per_mm.0 / 100.0, frame.y0 * px_per_mm.1 / 100.0, (frame.x1 + 1.0) * px_per_mm.0 / 100.0, (frame.y1 + 1.0) * px_per_mm.1 / 100.0);
        (r, ((frame.width() + 1.0) / 2540.0 * 72.0, (frame.height() + 1.0) / 2540.0 * 72.0))
    } else if bounds.width() >= 0.0 && bounds.height() >= 0.0 {
        let r = Rect::new(bounds.x0, bounds.y0, bounds.x1 + 1.0, bounds.y1 + 1.0);
        (r, (r.width() / px_per_mm.0 / 25.4 * 72.0, r.height() / px_per_mm.1 / 25.4 * 72.0))
    } else {
        return None;
    };
    Some(Header { frame: box_, size_pt, px_per_mm })
}

/// Whether the file draws by EMF+ records only (its header record says it
/// isn't dual).
pub(super) fn plus_only(d: &[u8]) -> bool {
    let mut p = 0;
    while p + 8 <= d.len() {
        let (t, s) = (u32_at(d, p), u32_at(d, p + 4) as usize);
        if s < 8 {
            return false;
        }
        if t == 70 && d.get(p + 12..p + 16) == Some(b"EMF+") {
            // The first EMF+ record: the header (0x4001), flags bit 0 dual.
            let flags = d.get(p + 18..p + 20).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]));
            return u16::from_le_bytes([d.get(p + 16).copied().unwrap_or(0), d.get(p + 17).copied().unwrap_or(0)]) == 0x4001 && flags & 1 == 0;
        }
        if t == 14 {
            return false;
        }
        p += s;
    }
    false
}

pub(super) fn play(d: &[u8], g: &mut Gdi) -> Option<()> {
    if plus_only(d) {
        return None;
    }
    let mut p = 0;
    let mut n = 0;
    while p + 8 <= d.len() && n < super::MAX_RECORDS {
        let (t, s) = (u32_at(d, p), u32_at(d, p + 4) as usize);
        if s < 8 || s % 4 != 0 {
            break;
        }
        let r = &d[p..(p + s).min(d.len())];
        if t == 14 {
            break;
        }
        record(g, t, r);
        p += s;
        n += 1;
    }
    Some(())
}

/// Points of a poly record: 32-bit or 16-bit pairs from `o`.
fn points(r: &[u8], o: usize, count: usize, short: bool) -> Vec<Point> {
    let size = if short { 4 } else { 8 };
    let count = count.min(r.len().saturating_sub(o) / size);
    (0..count)
        .map(|i| {
            let q = o + i * size;
            if short {
                let v = |k: usize| i16::from_le_bytes([r[q + k], r[q + k + 1]]) as f64;
                Point::new(v(0), v(2))
            } else {
                point_at(r, q)
            }
        })
        .collect()
}

/// A bitmap of a record: BITMAPINFO at `off_bmi` (`cb_bmi` bytes), bits
/// at `off_bits`.
fn bitmap(r: &[u8], off_bmi: u32, cb_bmi: u32, off_bits: u32, cb_bits: u32) -> Option<super::Bitmap> {
    let bmi = r.get(off_bmi as usize..(off_bmi as usize).checked_add(cb_bmi as usize)?)?;
    let bits = r.get(off_bits as usize..(off_bits as usize).saturating_add(cb_bits as usize).min(r.len()))?;
    dib(bmi, bits)
}

fn record(g: &mut Gdi, t: u32, r: &[u8]) {
    let u = |o: usize| u32_at(r, o);
    let i = |o: usize| i32_at(r, o) as f64;
    match t {
        // Poly records: bounds, count, points.
        2..=6 | 85..=89 => {
            let short = t >= 85;
            let pts = points(r, 28, u(24) as usize, short);
            match if short { t - 83 } else { t } {
                2 => g.bezier(&pts, false),
                3 => g.polygon(&pts, true),
                4 => match pts.split_first() {
                    Some((first, rest)) if g.path.is_some() => {
                        g.move_to(*first);
                        g.polyline_to(rest);
                    }
                    _ => g.polygon(&pts, false),
                },
                5 => g.bezier(&pts, true),
                _ => g.polyline_to(&pts),
            }
        }
        // PolyPolyline, PolyPolygon (32 and 16-bit).
        7 | 8 | 90 | 91 => {
            let short = t >= 90;
            let (np, total) = (u(24) as usize, u(28) as usize);
            if np > 1 << 20 || r.len() < 32 + np * 4 {
                return;
            }
            let counts: Vec<usize> = (0..np).map(|k| u(32 + k * 4) as usize).collect();
            let pts = points(r, 32 + np * 4, total, short);
            let mut polys = Vec::with_capacity(np);
            let mut at = 0usize;
            for c in counts {
                let end = at.saturating_add(c).min(pts.len());
                polys.push(pts[at..end].to_vec());
                at = end;
            }
            g.poly_polygon(&polys, t == 8 || t == 91);
        }
        9 => g.dc.win_ext = Vec2::new(i(8), i(12)),
        10 => g.dc.win_org = Vec2::new(i(8), i(12)),
        11 => g.dc.vp_ext = Vec2::new(i(8), i(12)),
        12 => g.dc.vp_org = Vec2::new(i(8), i(12)),
        15 => {
            let q = point_at(r, 8);
            let c = colorref(u(16));
            g.fill_rect_color(Rect::new(q.x, q.y, q.x + 1.0, q.y + 1.0), c);
        }
        17 => g.dc.map_mode = u(8),
        18 => g.dc.bk_opaque = u(8) == 2,
        19 => g.dc.winding = u(8) == 2,
        22 => g.dc.text_align = u(8),
        24 => g.dc.text_color = colorref(u(8)),
        25 => g.dc.bk_color = colorref(u(8)),
        27 => g.move_to(point_at(r, 8)),
        29 => g.exclude_rect(rect_at(r, 8)),
        30 => g.intersect_rect(rect_at(r, 8)),
        31 | 32 => {
            let (xn, xd, yn, yd) = (i(8), i(12), i(16), i(20));
            if xd != 0.0 && yd != 0.0 {
                let e = if t == 31 { &mut g.dc.vp_ext } else { &mut g.dc.win_ext };
                *e = Vec2::new(e.x * xn / xd, e.y * yn / yd);
            }
        }
        33 => g.save(),
        34 => g.restore(i32_at(r, 8)),
        35 => g.dc.world = xform(r, 8),
        36 => {
            let x = xform(r, 8);
            g.dc.world = match u(32) {
                1 => Affine::IDENTITY,
                2 => g.dc.world * x,
                3 => x * g.dc.world,
                4 => x,
                _ => g.dc.world,
            };
        }
        37 => g.select(u(8)),
        38 => g.put(u(8), Obj::Pen(Pen { style: u(12), width: i(16), color: colorref(u(24)), dashes: Vec::new() })),
        39 => {
            let b = brush(u(12), u(16), u(20));
            g.put(u(8), Obj::Brush(b));
        }
        40 => g.delete(u(8)),
        41 => g.angle_arc(point_at(r, 8), u(16) as f64, f32_at(r, 20), f32_at(r, 24)),
        42 => g.shape(vello_cpu::kurbo::Ellipse::from_rect(rect_at(r, 8)), true),
        43 => g.shape(rect_at(r, 8), true),
        44 => g.round_rect(rect_at(r, 8), Vec2::new(i(24), i(28))),
        45..=47 | 55 => {
            let kind = match t {
                45 => 0,
                46 => 1,
                47 => 2,
                _ => 3,
            };
            g.arc(rect_at(r, 8), point_at(r, 24), point_at(r, 32), kind);
        }
        54 => g.line_to(point_at(r, 8)),
        // PolyDraw (32 and 16-bit): count, points, then a kind each.
        56 | 92 => {
            let n = u(24) as usize;
            let short = t == 92;
            let pts = points(r, 28, n, short);
            let at = 28 + pts.len() * if short { 4 } else { 8 };
            let kinds = r.get(at..at + pts.len()).unwrap_or(&[]);
            g.poly_draw(&pts, kinds);
        }
        57 => g.dc.clockwise = u(8) == 2,
        58 => g.dc.miter = u(8) as f64,
        59 => g.begin_path(),
        60 => g.end_path(),
        61 => g.close_figure(),
        62 => g.draw_path(1),
        63 => g.draw_path(3),
        64 => g.draw_path(2),
        66 => g.widened = true,
        67 => g.clip_path(u(8)),
        68 => {
            g.path = None;
            g.done_path = None;
        }
        // FillRgn, PaintRgn: the region's rectangles (device units).
        71 | 74 => {
            let (cb, data) = if t == 71 { (u(24), 32) } else { (u(24), 28) };
            let rects = region(r.get(data..data + cb as usize).unwrap_or(&[]));
            let b = if t == 71 {
                match g.objects.get(u(28) as usize).cloned().flatten() {
                    Some(Obj::Brush(b)) => b,
                    _ => g.dc.brush.clone(),
                }
            } else {
                g.dc.brush.clone()
            };
            let mut p = vello_cpu::kurbo::BezPath::new();
            for q in &rects {
                p.extend(g.base * vello_cpu::kurbo::Shape::to_path(q, 0.1));
            }
            g.fill_with(&p, &b, vello_cpu::peniko::Fill::NonZero);
        }
        75 => {
            let cb = u(8) as usize;
            let rects = region(r.get(16..16 + cb).unwrap_or(&[]));
            g.clip_region(u(12), &rects);
        }
        // BitBlt, StretchBlt, AlphaBlend, TransparentBlt.
        76 | 77 | 114 | 116 => {
            let dest = Rect::new(i(24), i(28), i(24) + i(32), i(28) + i(36));
            let rop = u(40);
            let (xs, ys) = (i(44), i(48));
            let (cxs, cys) = if t == 76 { (i(32), i(36)) } else { (i(100), i(104)) };
            let src = Rect::new(xs, ys, xs + cxs, ys + cys);
            let (off_bmi, cb_bmi, off_bits, cb_bits) = (u(84), u(88), u(92), u(96));
            if cb_bmi == 0 {
                return g.pat_blt(dest, rop);
            }
            let Some(mut b) = bitmap(r, off_bmi, cb_bmi, off_bits, cb_bits) else { return };
            // The source transform beyond a plain offset is left aside.
            match t {
                114 => {
                    let f = u(40);
                    let constant = (f >> 16) as u8;
                    let src_alpha = (f >> 24) & 1 != 0;
                    if !src_alpha {
                        b.pixels.iter_mut().for_each(|p| p.a = 255);
                    }
                    g.blit(b, src, dest, 0, Some(constant));
                }
                116 => {
                    // The color given is transparent.
                    let c = colorref(u(40));
                    for p in &mut b.pixels {
                        if p.r == c[0] && p.g == c[1] && p.b == c[2] {
                            *p = vello_cpu::color::PremulRgba8 { r: 0, g: 0, b: 0, a: 0 };
                        }
                    }
                    g.blit(b, src, dest, 0, Some(255));
                }
                _ => g.blit(b, src, dest, rop, None),
            }
        }
        // SetDIBitsToDevice.
        80 => {
            let (xd, yd, xs, ys, cx, cy) = (i(24), i(28), i(32), i(36), i(40), i(44));
            let Some(b) = bitmap(r, u(48), u(52), u(56), u(60)) else { return };
            g.blit(b, Rect::new(xs, ys, xs + cx, ys + cy), Rect::new(xd, yd, xd + cx, yd + cy), super::SRCCOPY, None);
        }
        // StretchDIBits.
        81 => {
            let dest = Rect::new(i(24), i(28), i(24) + i(72), i(28) + i(76));
            let src = Rect::new(i(32), i(36), i(32) + i(40), i(36) + i(44));
            let rop = u(68);
            if u(52) == 0 {
                return g.pat_blt(dest, rop);
            }
            let Some(b) = bitmap(r, u(48), u(52), u(56), u(60)) else { return };
            // The bitmap's rows from the bottom: the source rectangle counts
            // from its bottom left (the usual whole bitmap is the same).
            let src = Rect::new(src.x0, b.height as f64 - src.y1, src.x1, b.height as f64 - src.y0);
            g.blit(b, src, dest, rop, None);
        }
        82 => {
            let f = logfont(r);
            g.put(u(8), Obj::Font(Rc::new(f)));
        }
        // ExtTextOutA / W.
        83 | 84 => {
            let at = point_at(r, 36);
            let nchars = (u(44) as usize).min(1 << 16);
            let off = u(48) as usize;
            let opts = u(52);
            let rect = rect_at(r, 56);
            let off_dx = u(72) as usize;
            let rect = (opts & 6 != 0 && rect.width() > 0.0 && rect.height() > 0.0).then_some(rect);
            let dx: Vec<f64> = if off_dx > 0 {
                // ETO_PDY (0x2000): pairs of x and y advances.
                let step = if opts & 0x2000 != 0 { 8 } else { 4 };
                (0..nchars).map_while(|k| r.get(off_dx + k * step..off_dx + k * step + 4).map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64)).collect()
            } else {
                Vec::new()
            };
            if t == 84 {
                let units: Vec<u16> = (0..nchars).map_while(|k| r.get(off + k * 2..off + k * 2 + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))).collect();
                if opts & 0x10 != 0 {
                    g.text(at, &[], Some(&units), &dx, opts, rect);
                } else {
                    let text: Vec<char> = char::decode_utf16(units.iter().copied()).map(|c| c.unwrap_or('\u{FFFD}')).collect();
                    // Advances are per UTF-16 unit: a pair's second is
                    // folded into the first.
                    let dx = if text.len() == units.len() { dx } else { fold_dx(&units, &dx) };
                    g.text(at, &text, None, &dx, opts, rect);
                }
            } else {
                let bytes = r.get(off..off + nchars).unwrap_or(&[]);
                let charset = g.dc.font.charset;
                let text = super::ansi(bytes, charset);
                let dx = if text.len() == bytes.len() { dx } else { Vec::new() };
                g.text(at, &text, None, &dx, opts, rect);
            }
        }
        // CreateMonoBrush, CreateDIBPatternBrushPt.
        93 | 94 => {
            let b = bitmap(r, u(16), u(20), u(24), u(28)).map(|b| Brush::Pattern(std::sync::Arc::new(vello_cpu::Pixmap::from_parts(b.pixels, b.width, b.height)))).unwrap_or(Brush::Solid([128; 3]));
            g.put(u(8), Obj::Brush(b));
        }
        95 => {
            let (style, width, bstyle, color) = (u(28), u(32) as f64, u(36), u(40));
            let n = (u(48) as usize).min(64);
            let dashes = (0..n).map(|k| u(52 + 4 * k) as f64).collect();
            let color = if bstyle == 1 { [0; 3] } else { colorref(color) };
            let style = if bstyle == 1 { 5 } else { style };
            g.put(u(8), Obj::Pen(Pen { style, width, color, dashes }));
        }
        // GradientFill: vertices, then rectangles or triangles.
        118 => {
            let (nv, nt, mode) = (u(24) as usize, u(28) as usize, u(32));
            if nv > 1 << 16 || nt > 1 << 16 {
                return;
            }
            let verts: Vec<(Point, [f64; 4])> = (0..nv)
                .map(|k| {
                    let o = 36 + k * 16;
                    let c = |j: usize| r.get(o + 8 + j * 2 + 1).copied().unwrap_or(0) as f64;
                    (point_at(r, o), [c(0), c(1), c(2), c(3)])
                })
                .collect();
            let at = 36 + nv * 16;
            let per = if mode == 2 { 3 } else { 2 };
            let idx: Vec<Vec<u32>> = (0..nt).map(|k| (0..per).map(|j| u(at + (k * per + j) * 4)).collect()).collect();
            g.gradient(&verts, &idx, mode);
        }
        _ => {}
    }
}

/// An XFORM's six floats as an affine map.
fn xform(r: &[u8], o: usize) -> Affine {
    let f = |k: usize| f32_at(r, o + 4 * k);
    Affine::new([f(0), f(1), f(2), f(3), f(4), f(5)])
}

pub(super) fn brush(style: u32, color: u32, hatch: u32) -> Brush {
    match style {
        1 => Brush::Null,
        2 => Brush::Hatch(hatch, colorref(color)),
        _ => Brush::Solid(colorref(color)),
    }
}

fn logfont(r: &[u8]) -> LogFont {
    let i = |o: usize| i32_at(r, o) as f64;
    let b = |o: usize| r.get(o).copied().unwrap_or(0);
    let units: Vec<u16> = (0..32).map(|k| r.get(40 + k * 2..42 + k * 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))).take_while(|&c| c != 0).collect();
    LogFont {
        height: i(12),
        width: i(16),
        escapement: i(20),
        weight: i32_at(r, 28),
        italic: b(32) != 0,
        underline: b(33) != 0,
        strike: b(34) != 0,
        charset: b(35),
        pitch_family: b(39),
        face: String::from_utf16_lossy(&units),
    }
}

/// RGNDATA: its rectangles.
fn region(d: &[u8]) -> Vec<Rect> {
    let n = (u32_at(d, 8) as usize).min(d.len().saturating_sub(32) / 16);
    (0..n).map(|k| rect_at(d, 32 + k * 16)).collect()
}

/// Advances of UTF-16 units folded to characters: the second unit of a
/// surrogate pair adds to the first's.
fn fold_dx(units: &[u16], dx: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(dx.len());
    for (k, &u) in units.iter().enumerate() {
        let d = dx.get(k).copied().unwrap_or(0.0);
        if (0xDC00..0xE000).contains(&u) && !out.is_empty() {
            *out.last_mut().unwrap() += d;
        } else {
            out.push(d);
        }
    }
    out
}
