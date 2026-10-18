use super::*;

fn no_fonts(_: &Style, _: &str) -> Option<(Arc<[u8]>, u32)> {
    None
}

fn rec(t: u32, body: &[u8]) -> Vec<u8> {
    let mut v = t.to_le_bytes().to_vec();
    v.extend(((body.len() + 8) as u32).to_le_bytes());
    v.extend_from_slice(body);
    v
}

fn ints(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// An EMF 100×50 pixels on a 96 dpi device, with `records`.
fn emf(records: &[Vec<u8>]) -> Vec<u8> {
    let mut h = ints(&[0, 0, 99, 49]);
    // Frame in 0.01 mm: 100 px at 96 dpi.
    h.extend(ints(&[0, 0, 2645, 1322]));
    h.extend(b" EMF");
    h.extend(ints(&[0x10000, 0, 0, 0, 0, 0, 0]));
    // Device 1000×1000 px, 264.58 mm.
    h.extend(ints(&[1000, 1000, 265, 265]));
    let mut d = rec(1, &h);
    for r in records {
        d.extend(r);
    }
    d.extend(rec(14, &ints(&[0, 16, 20])));
    let n = d.len() as u32;
    d[48..52].copy_from_slice(&n.to_le_bytes());
    d
}

fn pixel(p: &Picture, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * p.width + x) * 4) as usize;
    p.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn emf_rectangles_are_drawn_where_they_are() {
    let d = emf(&[
        // A red brush and a null pen, then a rectangle on the left half.
        rec(39, &ints(&[1, 0, 0x0000FF, 0])),
        rec(37, &ints(&[1])),
        rec(37, &ints(&[0x8000_0008u32 as i32])),
        rec(43, &ints(&[0, 0, 50, 50])),
    ]);
    assert!(drawable(&d));
    let (w, h) = size(&d).unwrap();
    assert!((w - 75.0).abs() < 0.5 && (h - 37.5).abs() < 0.5, "{w}×{h}");
    let p = render(&d, 1.0, &no_fonts).unwrap();
    assert_eq!((p.width, p.height), (75, 38));
    assert_eq!(pixel(&p, 10, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&p, 60, 10)[3], 0, "the right half is left clear");
}

#[test]
fn emf_window_and_viewport_map_the_drawing() {
    // An anisotropic map: logical 0..1000 onto the 100 device pixels.
    let d = emf(&[
        rec(17, &ints(&[8])),
        rec(9, &ints(&[1000, 1000])),
        rec(11, &ints(&[100, 100])),
        rec(39, &ints(&[1, 0, 0xFF0000, 0])),
        rec(37, &ints(&[1])),
        rec(43, &ints(&[500, 0, 1000, 500])),
    ]);
    let p = render(&d, 1.0, &no_fonts).unwrap();
    assert_eq!(pixel(&p, 60, 10), [0, 0, 255, 255]);
    assert_eq!(pixel(&p, 10, 10)[3], 0);
}

#[test]
fn emf_clips_and_bitmaps() {
    // A 2×1 bitmap (blue, green), bottom-up 24-bit, stretched over all.
    let mut bmi = ints(&[40, 2, 1]);
    bmi.extend(1u16.to_le_bytes());
    bmi.extend(24u16.to_le_bytes());
    bmi.extend(ints(&[0, 8, 0, 0, 0, 0]));
    let bits = [255u8, 0, 0, 0, 255, 0, 0, 0];
    let mut body = ints(&[0, 0, 99, 49, 0, 0, 0, 0, 2, 1]);
    let off_bmi = 8 + body.len() as i32 + 32;
    body.extend(ints(&[off_bmi, 40, off_bmi + 40, 8, 0, 0x00CC0020, 100, 50]));
    body.extend(&bmi);
    body.extend(bits);
    let d = emf(&[rec(30, &ints(&[0, 0, 100, 25])), rec(81, &body)]);
    let p = render(&d, 1.0, &no_fonts).unwrap();
    assert_eq!(pixel(&p, 5, 5), [0, 0, 255, 255]);
    assert_eq!(pixel(&p, 70, 5), [0, 255, 0, 255]);
    assert_eq!(pixel(&p, 5, 30)[3], 0, "clipped away");
}

#[test]
fn placeable_wmf_polygons_fill() {
    let w = |v: &[i16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let rec = |f: u16, p: &[i16]| {
        let mut v = (3 + p.len() as u32).to_le_bytes().to_vec();
        v.extend(f.to_le_bytes());
        v.extend(w(p));
        v
    };
    // A box of 1000×1000 units at 1000 to the inch: 72 points.
    let mut d = vec![0xD7, 0xCD, 0xC6, 0x9A, 0, 0];
    d.extend(w(&[0, 0, 1000, 1000, 1000, 0, 0, 0]));
    d.extend(w(&[1, 9, 0x300, 0, 0, 1, 0, 0, 0]));
    d.extend(rec(0x020B, &[0, 0]));
    d.extend(rec(0x020C, &[1000, 1000]));
    // A green brush (COLORREF 0x00FF00) selected, a triangle over the
    // top left half.
    d.extend(rec(0x02FC, &[0, 0xFF00u16 as i16, 0, 0]));
    d.extend(rec(0x012D, &[0]));
    d.extend(rec(0x0324, &[3, 0, 0, 1000, 0, 0, 1000]));
    d.extend(rec(0, &[]));
    let (pw, ph) = size(&d).unwrap();
    assert_eq!((pw.round(), ph.round()), (72.0, 72.0));
    let p = render(&d, 1.0, &no_fonts).unwrap();
    assert_eq!(pixel(&p, 10, 10), [0, 255, 0, 255]);
    assert_eq!(pixel(&p, 65, 65)[3], 0);
}

#[test]
fn broken_metafiles_are_not_drawn() {
    assert!(render(b"not a metafile", 1.0, &no_fonts).is_none());
    let mut d = emf(&[]);
    d.truncate(100);
    let _ = render(&d, 1.0, &no_fonts);
}
