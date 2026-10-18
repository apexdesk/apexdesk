//! Floating shapes and pictures of .doc files. The OfficeArt drawing
//! (`DggInfo` in the table stream) holds each shape's kind, look and
//! picture (in the drawing's store, the pictures themselves usually in the
//! WordDocument stream); the `PlcSpaMom` anchors tie shapes to `0x08`
//! characters of the text with their place, size and wrapping; text boxes'
//! text is a story of its own. Groups, shapes in headers and EMF/WMF
//! pictures are left out.

use std::collections::HashMap;
use std::sync::Arc;

use apex_doc::{Align, DocShape, HRel, Image, OBJECT, Paragraph, Shape, ShapeKind, ShapePara, ShapeRun, ShapeWrap, VAnchor, VRel, Wrap};

use super::{Doc, Fib, TW, find_blip, plc, u16_at, u32_at};

const PLC_SPA_MOM: usize = 40;
const DGG_INFO: usize = 50;
const PLCF_TXBX_TXT: usize = 56;

/// What an anchor character shows.
pub(super) enum Float {
    Shape(Arc<DocShape>),
    /// A picture the text wraps around.
    Picture(Arc<Image>),
}

/// An OfficeArt record: its instance, type and body.
struct Rec<'a> {
    inst: u16,
    ty: u16,
    body: &'a [u8],
}

fn records(d: &[u8]) -> Vec<Rec<'_>> {
    let mut out = Vec::new();
    let mut p = 0;
    while let (Some(vi), Some(ty), Some(len)) = (u16_at(d, p), u16_at(d, p + 2), u32_at(d, p + 4)) {
        let end = (p + 8).saturating_add(len as usize).min(d.len());
        out.push(Rec { inst: vi >> 4, ty, body: &d[p + 8..end] });
        p = end;
    }
    out
}

/// A shape of the drawing: its kind (`msospt`), flags and properties.
#[derive(Default)]
struct Sp {
    kind: u16,
    flags: u32,
    props: HashMap<u16, u32>,
    complex: HashMap<u16, Vec<u8>>,
}

impl Sp {
    fn prop(&self, id: u16) -> Option<u32> {
        self.props.get(&id).copied()
    }

    /// Bit `bit` of a set of boolean properties, if set explicitly.
    fn flag(&self, id: u16, bit: u32) -> Option<bool> {
        let v = self.prop(id)?;
        (v & (1 << (bit + 16)) != 0).then_some(v & (1 << bit) != 0)
    }

    /// An RGB color property (not ones from the scheme or the system).
    fn color(&self, id: u16) -> Option<u32> {
        let v = self.prop(id)?;
        (v >> 24 & 0x18 == 0).then_some((v & 0xFF) << 16 | (v >> 8 & 0xFF) << 8 | (v >> 16 & 0xFF))
    }

    /// A string property (UTF-16, zero-terminated).
    fn text(&self, id: u16) -> String {
        let units: Vec<u16> = self.complex.get(&id).map_or(Vec::new(), |d| d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).take(4096).collect());
        String::from_utf16_lossy(&units)
    }
}

fn read_sp(body: &[u8]) -> Option<(u32, Sp)> {
    let mut sp = Sp::default();
    let mut spid = None;
    for r in records(body) {
        match r.ty {
            0xF00A => {
                spid = u32_at(r.body, 0);
                sp.flags = u32_at(r.body, 4).unwrap_or(0);
                sp.kind = r.inst;
            }
            // Properties: primary, secondary and tertiary tables.
            0xF00B | 0xF121 | 0xF122 => {
                let n = r.inst as usize;
                let mut extra = n * 6;
                for i in 0..n {
                    let (Some(id), Some(op)) = (u16_at(r.body, i * 6), u32_at(r.body, i * 6 + 2)) else { break };
                    if id & 0x8000 != 0 {
                        let end = extra.saturating_add(op as usize);
                        sp.complex.insert(id & 0x3FFF, r.body.get(extra..end).unwrap_or(&[]).to_vec());
                        extra = end;
                    } else if id & 0x3F == 0x3F {
                        // Sets of booleans may come in more than one table.
                        *sp.props.entry(id & 0x3FFF).or_default() |= op;
                    } else {
                        sp.props.insert(id & 0x3FFF, op);
                    }
                }
            }
            _ => {}
        }
    }
    Some((spid?, sp))
}

/// The picture store (its `BSE` records) and the main document's shapes by
/// id (members of groups are left out).
fn read_drawing(d: &[u8]) -> (Vec<&[u8]>, HashMap<u32, Sp>) {
    let mut store = Vec::new();
    let mut shapes = HashMap::new();
    let Some(len) = u32_at(d, 4) else { return (store, shapes) };
    let dgg = d.get(8..(8 + len as usize).min(d.len())).unwrap_or(&[]);
    for r in records(dgg) {
        if r.ty == 0xF001 {
            store = records(r.body).into_iter().map(|b| if b.ty == 0xF007 { b.body } else { &[][..] }).collect();
        }
    }
    // Then each drawing, after a byte saying whose it is (0 the main text).
    let mut p = 8 + len as usize;
    while let (Some(&whose), Some(len)) = (d.get(p), u32_at(d, p + 5)) {
        let body = d.get(p + 9..(p + 9).saturating_add(len as usize).min(d.len())).unwrap_or(&[]);
        p = p + 9 + len as usize;
        if whose != 0 {
            continue;
        }
        for group in records(body).into_iter().filter(|r| r.ty == 0xF003) {
            for s in records(group.body).into_iter().filter(|r| r.ty == 0xF004) {
                if let Some((id, sp)) = read_sp(s.body) {
                    shapes.insert(id, sp);
                }
            }
        }
    }
    (store, shapes)
}

/// Picture `pib` (from 1) of the store, if PNG, JPEG or a bitmap.
fn blip(store: &[&[u8]], wd: &[u8], pib: u32) -> Option<Vec<u8>> {
    let bse = store.get((pib as usize).checked_sub(1)?)?;
    let name = *bse.get(33)? as usize;
    if bse.len() > 36 + name {
        return find_blip(&bse[36 + name..], 0);
    }
    // Kept in the WordDocument stream.
    let (size, at) = (u32_at(bse, 20)? as usize, u32_at(bse, 28)? as usize);
    find_blip(wd.get(at..at.checked_add(size)?.min(wd.len()))?, 0)
}

fn shape_kind(spt: u16) -> ShapeKind {
    match spt {
        2 => ShapeKind::RoundRect,
        3 => ShapeKind::Ellipse,
        4 => ShapeKind::Diamond,
        5 => ShapeKind::Triangle,
        6 => ShapeKind::RightTriangle,
        9 => ShapeKind::Hexagon,
        12 => ShapeKind::Star5,
        13 => ShapeKind::RightArrow,
        20 | 32 => ShapeKind::Line,
        56 => ShapeKind::Pentagon,
        61 | 62 | 63 => ShapeKind::Callout,
        66 => ShapeKind::LeftArrow,
        67 => ShapeKind::DownArrow,
        68 => ShapeKind::UpArrow,
        202 => ShapeKind::TextBox,
        _ => ShapeKind::Rect,
    }
}

/// Text box paragraphs as shape text.
fn shape_text(ps: Vec<Paragraph>) -> Vec<ShapePara> {
    let mut out: Vec<ShapePara> = ps
        .into_iter()
        .map(|p| {
            let mut at = 0;
            let mut runs = Vec::new();
            for r in &p.runs {
                let t = p.text.get(at..at + r.len).unwrap_or("");
                at += r.len;
                let t: String = t.chars().filter(|&c| c != OBJECT).map(|c| if c == apex_doc::LINE_BREAK { ' ' } else { c }).collect();
                if t.is_empty() {
                    continue;
                }
                let a = &r.attrs;
                let size = f32::from(a.size.unwrap_or(p.style.kind.base_size())) / 2.0;
                runs.push(ShapeRun { text: t, bold: a.bold, italic: a.italic, underline: a.underline, size: (size != apex_doc::SHAPE_TEXT_PT).then_some(size), color: a.color, font: a.font.clone(), fx: a.text_fx.clone() });
            }
            ShapePara { align: p.style.align, runs, list: None }
        })
        .collect();
    while out.last().is_some_and(|p| p.runs.is_empty()) {
        out.pop();
    }
    out
}

/// The floating shapes and pictures by the position of their anchor;
/// `story` is where the text boxes' text starts.
pub(super) fn read(doc: &mut Doc, fib: &Fib, table: &[u8], wd: &[u8], story: usize) -> HashMap<u32, Float> {
    let mut out = HashMap::new();
    let (cps, spas) = plc(fib.part(table, PLC_SPA_MOM), 26);
    if spas.is_empty() {
        return out;
    }
    let (store, shapes) = read_drawing(fib.part(table, DGG_INFO));
    // Shapes showing the same picture share its bytes.
    let mut blips: HashMap<u32, Option<Arc<[u8]>>> = HashMap::new();
    // Text box text by shape id.
    let (tcps, txbxs) = plc(fib.part(table, PLCF_TXBX_TXT), 22);
    let mut texts: HashMap<u32, (usize, usize)> = HashMap::new();
    for (i, t) in txbxs.iter().enumerate() {
        if let (Some(lid), Some(&a), Some(&b)) = (u32_at(t, 14), tcps.get(i), tcps.get(i + 1)) {
            texts.insert(lid, (story + a as usize, story + b as usize));
        }
    }
    for (z, (spa, &cp)) in spas.iter().zip(&cps).enumerate() {
        let Some(sp) = u32_at(spa, 0).and_then(|id| shapes.get(&id)) else { continue };
        // Deleted shapes and the page background.
        if sp.flags & 0x408 != 0 {
            continue;
        }
        let at = |p: usize| u32_at(spa, p).map_or(0.0, |v| v as i32 as f32 / TW);
        let (x, y) = (at(4), at(8));
        let (w, h) = (at(12) - x, at(16) - y);
        let flags = u16_at(spa, 20).unwrap_or(0);
        let wrap = match (flags >> 5) & 0xF {
            1 => ShapeWrap::TopBottom,
            3 if flags & 0x4000 != 0 || sp.flag(0x03BF, 5) == Some(true) => ShapeWrap::Behind,
            3 => ShapeWrap::InFront,
            _ => ShapeWrap::Square,
        };
        // What the offsets are from (margin, page, text...): Word 2000+
        // properties, else the anchor's.
        let h_rel = match sp.prop(0x0390).unwrap_or(u32::from((flags >> 1) & 3)) {
            1 => HRel::Page,
            _ => HRel::Margin,
        };
        let v_rel = match sp.prop(0x0392).unwrap_or(u32::from((flags >> 3) & 3)) {
            0 => VRel::Margin,
            1 => VRel::Page,
            _ => VRel::Paragraph,
        };
        let h_align = match sp.prop(0x038F) {
            Some(1) => Some(Align::Left),
            Some(2) => Some(Align::Center),
            Some(3) => Some(Align::Right),
            _ => None,
        };
        let rotation = sp.prop(0x0004).map_or(0.0, |v| v as i32 as f32 / 65536.0);
        let place = |s: &mut DocShape| {
            (s.wrap, s.x, s.y, s.h_rel, s.v_rel, s.h_align, s.z) = (wrap, x, y, h_rel, v_rel, h_align, z as i32);
            s.sanitize();
        };
        if let Some(data) = sp.prop(0x0104).and_then(|pib| blips.entry(pib).or_insert_with(|| blip(&store, wd, pib).map(Arc::from)).clone()) {
            let mut img = Image::new(data, w.max(1.0), h.max(1.0));
            let crop = |id: u16| sp.prop(id).map_or(0.0, |v| (v as i32 as f32 / 65536.0).clamp(0.0, 0.99));
            img.crop = [crop(0x0102), crop(0x0100), crop(0x0103), crop(0x0101)];
            (img.rotation, img.name, img.alt) = (rotation, sp.text(0x0380), sp.text(0x0381));
            if wrap.floats() {
                let mut s = DocShape::new(Shape::picture(img), w, h);
                place(&mut s);
                out.insert(cp, Float::Shape(Arc::new(s)));
            } else {
                img.wrap = if wrap == ShapeWrap::TopBottom { Wrap::TopBottom } else { Wrap::Square };
                (img.offset_x, img.align) = (x.max(0.0), h_align);
                out.insert(cp, Float::Picture(Arc::new(img)));
            }
            continue;
        }
        // Pictures we can't show (EMF, WMF).
        if sp.kind == 75 || sp.prop(0x0104).is_some() {
            continue;
        }
        let mut shape = Shape::new(shape_kind(sp.kind));
        // Office 97's defaults: a white fill and a thin black outline.
        shape.fill = (!shape.kind.is_line() && sp.flag(0x01BF, 4).unwrap_or(true)).then(|| sp.color(0x0181).unwrap_or(0xFFFFFF));
        shape.line = sp.flag(0x01FF, 3).unwrap_or(true).then(|| sp.color(0x01C0).unwrap_or(0));
        shape.line_width = sp.prop(0x01CB).map_or(0.75, |v| v as f32 / 12700.0);
        (shape.rotation, shape.flip_h, shape.flip_v) = (rotation, sp.flags & 0x40 != 0, sp.flags & 0x80 != 0);
        (shape.name, shape.alt) = (sp.text(0x0380), sp.text(0x0381));
        shape.valign = match sp.prop(0x0087) {
            Some(1 | 4 | 6 | 8) => VAnchor::Middle,
            Some(2 | 5 | 7 | 9) => VAnchor::Bottom,
            _ => VAnchor::Top,
        };
        if let Some(&(a, b)) = u32_at(spa, 0).and_then(|id| texts.get(&id)) {
            shape.text = shape_text(doc.story(a, b));
        }
        let mut s = DocShape::new(shape, w, h);
        place(&mut s);
        out.insert(cp, Float::Shape(Arc::new(s)));
    }
    out
}
