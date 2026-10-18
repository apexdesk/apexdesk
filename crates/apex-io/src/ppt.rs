//! PowerPoint 97-2003 presentations (.ppt, .pps, .pot), read only. The
//! "PowerPoint Document" stream is followed from the last save (the
//! persist directory of its user edits) to the slides: their titles and
//! body text (kept in the slide lists), text boxes and pictures (in their
//! drawings) and notes. What is placed is placed where the file says; the
//! slides take a new Office master, and shapes without text, fonts and
//! bullet levels are not read. Saving asks for another format.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use apex_slides::object::{Frame, ObjKind, Object, Picture};
use apex_slides::{Deck, Notes, TextBody, build, edit};

use crate::Error;
use crate::ole;

// Record types.
const DOCUMENT: u16 = 0x03E8;
const DOCUMENT_ATOM: u16 = 0x03E9;
const SLIDE: u16 = 0x03EE;
const NOTES: u16 = 0x03F0;
const NOTES_ATOM: u16 = 0x03F1;
const SLIDE_PERSIST: u16 = 0x03F3;
const MAIN_MASTER: u16 = 0x03F8;
const DRAWING_GROUP: u16 = 0x040B;
const OUTLINE_REF: u16 = 0x0F9E;
const TEXT_HEADER: u16 = 0x0F9F;
const TEXT_CHARS: u16 = 0x0FA0;
const TEXT_BYTES: u16 = 0x0FA8;
const SLIDE_LIST: u16 = 0x0FF0;
const USER_EDIT: u16 = 0x0FF5;
const PERSIST_DIR: u16 = 0x1772;
const BSE: u16 = 0xF007;
const SP_GROUP: u16 = 0xF003;
const SP: u16 = 0xF004;
const SP_GROUP_ATOM: u16 = 0xF009;
const SHAPE_ATOM: u16 = 0xF00A;
const OPT: u16 = 0xF00B;
const TEXTBOX: u16 = 0xF00D;
const CHILD_ANCHOR: u16 = 0xF00F;
const CLIENT_ANCHOR: u16 = 0xF010;

/// One record: its type, instance and body.
#[derive(Clone, Copy)]
struct Rec<'a> {
    ver: u8,
    typ: u16,
    inst: u16,
    body: &'a [u8],
}

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?))
}

fn i32_at(d: &[u8], p: usize) -> Option<i32> {
    u32_at(d, p).map(|v| v as i32)
}

/// The record starting at `p`.
fn rec_at(d: &[u8], p: usize) -> Option<Rec<'_>> {
    let vi = u16_at(d, p)?;
    let typ = u16_at(d, p + 2)?;
    let len = u32_at(d, p + 4)? as usize;
    let body = d.get(p + 8..(p + 8).checked_add(len)?.min(d.len()))?;
    Some(Rec { ver: (vi & 0xF) as u8, typ, inst: vi >> 4, body })
}

/// The records one after another in `d`.
fn recs(d: &[u8]) -> Vec<Rec<'_>> {
    let mut out = Vec::new();
    let mut p = 0;
    while let Some(r) = rec_at(d, p) {
        out.push(r);
        p += 8 + r.body.len();
    }
    out
}

fn child(d: &[u8], typ: u16) -> Option<Rec<'_>> {
    recs(d).into_iter().find(|r| r.typ == typ)
}

/// Every record of type `typ` inside `d`, at any depth.
fn find_all<'a>(d: &'a [u8], typ: u16, out: &mut Vec<Rec<'a>>, depth: u8) {
    if depth > 12 {
        return;
    }
    for r in recs(d) {
        if r.typ == typ {
            out.push(r);
        } else if is_container(r) {
            find_all(r.body, typ, out, depth + 1);
        }
    }
}

/// Containers are the records of version 0xF.
fn is_container(r: Rec) -> bool {
    r.ver == 0xF
}

/// The offsets of the persist objects of the last save.
fn persist_dir(doc: &[u8], user: &[u8]) -> Option<(HashMap<u32, usize>, u32)> {
    let mut edit = u32_at(user, 16)? as usize;
    let mut dir = HashMap::new();
    let mut seen = HashSet::new();
    // Each directory is read once, however many saves name it.
    let mut dirs = HashSet::new();
    let mut doc_ref = None;
    // Directories may overlap: words read, at most a few times the stream's.
    let mut words = doc.len() + 1024;
    while seen.insert(edit) {
        let e = rec_at(doc, edit).filter(|r| r.typ == USER_EDIT)?;
        doc_ref.get_or_insert(u32_at(e.body, 16)?);
        if let Some(pd) = u32_at(e.body, 12).filter(|&o| dirs.insert(o)).and_then(|o| rec_at(doc, o as usize)).filter(|r| r.typ == PERSIST_DIR) {
            let mut p = 0;
            while let Some(info) = u32_at(pd.body, p) {
                let (first, n) = (info & 0xF_FFFF, info >> 20);
                p += 4;
                words = words.saturating_sub(n as usize + 1);
                if words == 0 {
                    break;
                }
                for i in 0..n {
                    if let Some(off) = u32_at(pd.body, p) {
                        dir.entry(first + i).or_insert(off as usize);
                    }
                    p += 4;
                }
            }
        }
        match u32_at(e.body, 8) {
            Some(0) | None => break,
            Some(o) => edit = o as usize,
        }
    }
    Some((dir, doc_ref?))
}

/// Text of a chars or bytes atom: paragraphs split by '\r', line breaks
/// ('\u{b}') kept as new lines.
fn atom_text(r: Rec) -> Option<String> {
    let s: String = match r.typ {
        TEXT_CHARS => char::decode_utf16(r.body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]))).map(|c| c.unwrap_or('\u{FFFD}')).collect(),
        TEXT_BYTES => r.body.iter().map(|&b| ole::cp1252(b)).collect(),
        _ => return None,
    };
    Some(s.replace(['\r', '\u{b}'], "\n").chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect())
}

/// A slide list entry: the slide's persist reference and id, and the texts
/// of its placeholders with their kinds.
#[derive(Default)]
struct Listed {
    persist: u32,
    id: u32,
    texts: Vec<(u32, String)>,
}

fn slide_list(r: Rec) -> Vec<Listed> {
    let mut out: Vec<Listed> = Vec::new();
    let mut kind = 4;
    for k in recs(r.body) {
        match k.typ {
            SLIDE_PERSIST => out.push(Listed { persist: u32_at(k.body, 0).unwrap_or(0), id: u32_at(k.body, 12).unwrap_or(0), texts: Vec::new() }),
            TEXT_HEADER => kind = u32_at(k.body, 0).unwrap_or(4),
            TEXT_CHARS | TEXT_BYTES => {
                if let (Some(l), Some(t)) = (out.last_mut(), atom_text(k)) {
                    l.texts.push((kind, t));
                }
            }
            _ => {}
        }
    }
    out
}

/// A shape found in a drawing: where it is and what it holds.
enum Found {
    Text { kind: u32, text: String, frame: Option<Frame> },
    Ref { index: usize, frame: Option<Frame> },
    Picture { pib: u32, frame: Frame },
}

/// Maps drawing coordinates to points (master units are 1/576 inch).
#[derive(Clone, Copy)]
struct Map {
    sx: f32,
    sy: f32,
    ox: f32,
    oy: f32,
}

impl Map {
    fn rect(&self, l: f32, t: f32, r: f32, b: f32) -> Frame {
        Frame::new(self.ox + l * self.sx, self.oy + t * self.sy, (r - l) * self.sx, (b - t) * self.sy)
    }
}

fn anchor(sp: &[u8], m: Map) -> Option<Frame> {
    if let Some(a) = child(sp, CLIENT_ANCHOR) {
        let v: Vec<f32> = if a.body.len() >= 16 {
            (0..4).filter_map(|i| i32_at(a.body, i * 4)).map(|v| v as f32).collect()
        } else {
            (0..4).filter_map(|i| u16_at(a.body, i * 2)).map(|v| v as i16 as f32).collect()
        };
        // top, left, right, bottom
        return (v.len() == 4).then(|| Map { sx: 0.125, sy: 0.125, ox: 0.0, oy: 0.0 }.rect(v[1], v[0], v[2], v[3]));
    }
    let a = child(sp, CHILD_ANCHOR)?;
    let v: Vec<f32> = (0..4).filter_map(|i| i32_at(a.body, i * 4)).map(|v| v as f32).collect();
    // left, top, right, bottom
    (v.len() == 4).then(|| m.rect(v[0], v[1], v[2], v[3]))
}

fn walk_group(g: &[u8], m: Map, out: &mut Vec<Found>, depth: u8) {
    if depth > 8 {
        return;
    }
    let mut inner = m;
    for (i, r) in recs(g).into_iter().enumerate() {
        match r.typ {
            SP_GROUP => walk_group(r.body, inner, out, depth + 1),
            SP if i == 0 => {
                // The group's own shape: its box and the child coordinates.
                if let (Some(f), Some(sg)) = (anchor(r.body, m), child(r.body, SP_GROUP_ATOM)) {
                    let v: Vec<f32> = (0..4).filter_map(|k| i32_at(sg.body, k * 4)).map(|v| v as f32).collect();
                    if v.len() == 4 && v[2] > v[0] && v[3] > v[1] {
                        let (sx, sy) = (f.w / (v[2] - v[0]), f.h / (v[3] - v[1]));
                        inner = Map { sx, sy, ox: f.x - v[0] * sx, oy: f.y - v[1] * sy };
                    }
                }
            }
            SP => shape(r.body, inner, out),
            _ => {}
        }
    }
}

fn shape(sp: &[u8], m: Map, out: &mut Vec<Found>) {
    let flags = child(sp, SHAPE_ATOM).and_then(|a| u32_at(a.body, 4)).unwrap_or(0);
    // Deleted shapes (fDeleted) and the background.
    if flags & 0x100 != 0 || flags & 0x400 != 0 {
        return;
    }
    let frame = anchor(sp, m);
    if let Some(tb) = child(sp, TEXTBOX) {
        let mut kind = 4;
        for k in recs(tb.body) {
            match k.typ {
                TEXT_HEADER => kind = u32_at(k.body, 0).unwrap_or(4),
                TEXT_CHARS | TEXT_BYTES => {
                    if let Some(text) = atom_text(k) {
                        out.push(Found::Text { kind, text, frame });
                        return;
                    }
                }
                OUTLINE_REF => {
                    out.push(Found::Ref { index: u32_at(k.body, 0).unwrap_or(0) as usize, frame });
                    return;
                }
                _ => {}
            }
        }
    }
    if let (Some(opt), Some(frame)) = (child(sp, OPT), frame) {
        // A shape shows one picture.
        let n = opt.inst as usize;
        for i in 0..n {
            let id = u16_at(opt.body, i * 6).unwrap_or(0);
            if id & 0x3FFF == 0x0104 && id & 0x8000 == 0 {
                if let Some(pib) = u32_at(opt.body, i * 6 + 2).filter(|p| *p > 0) {
                    out.push(Found::Picture { pib, frame });
                    break;
                }
            }
        }
    }
}

/// The shapes of a slide or notes container.
fn drawing(c: &[u8]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut groups = Vec::new();
    find_all(c, SP_GROUP, &mut groups, 0);
    // The top group holds the others (reached through it).
    if let Some(top) = groups.first() {
        walk_group(top.body, Map { sx: 0.125, sy: 0.125, ox: 0.0, oy: 0.0 }, &mut out, 0);
    }
    out
}

/// A picture's data from its blip record (PNG and JPEG; metafiles and
/// device-independent bitmaps are skipped).
fn blip(r: Rec) -> Option<Arc<[u8]>> {
    if !matches!(r.typ, 0xF01D | 0xF01E | 0xF02A) {
        return None;
    }
    let skip = 16 * (1 + (r.inst as usize & 1)) + 1;
    Some(Arc::from(r.body.get(skip..)?))
}

/// The pictures of the drawing group, by their 1-based index.
fn pictures(doc_c: &[u8], pics: &[u8]) -> Vec<Option<Arc<[u8]>>> {
    let mut bses = Vec::new();
    if let Some(dg) = child(doc_c, DRAWING_GROUP) {
        find_all(dg.body, BSE, &mut bses, 0);
    }
    // Entries pointing at the same place share the picture; those pointing
    // into each other copy at most a few times what the file holds.
    let mut kept: HashMap<usize, Option<Arc<[u8]>>> = HashMap::new();
    let mut left = 2 * (pics.len() + doc_c.len()) + (16 << 20);
    let mut take = |b: Option<Arc<[u8]>>| -> Option<Arc<[u8]>> {
        let b = b?;
        left = left.checked_sub(b.len())?;
        Some(b)
    };
    bses.iter()
        .map(|b| {
            let name_len = *b.body.get(33)? as usize;
            if let Some(r) = rec_at(b.body, 36 + name_len) {
                return take(blip(r));
            }
            let at = u32_at(b.body, 28)? as usize;
            if let Some(k) = kept.get(&at) {
                return k.clone();
            }
            let p = take(rec_at(pics, at).and_then(blip));
            kept.insert(at, p.clone());
            p
        })
        .collect()
}

fn picture(id: u32, data: Arc<[u8]>, frame: Frame) -> Object {
    let mut o = Object::new(id, format!("Picture {}", id.saturating_sub(1)), ObjKind::Picture(Box::new(Picture { image: apex_doc::Image::new(data, frame.w, frame.h), media: None })));
    o.sp.frame = Some(frame);
    o
}

fn is_title(kind: u32) -> bool {
    matches!(kind, 0 | 6)
}

pub fn read(data: &[u8]) -> Result<Deck, Error> {
    let mut deck = read_all(data)?;
    apex_slides::bound(&mut deck);
    Ok(deck)
}

fn read_all(data: &[u8]) -> Result<Deck, Error> {
    let mut c = ole::open(data)?;
    let doc = ole::stream(&mut c, "/PowerPoint Document").ok_or_else(|| Error::new("not a PowerPoint file"))?;
    let user = ole::stream(&mut c, "/Current User").unwrap_or_default();
    let pics = ole::stream(&mut c, "/Pictures").unwrap_or_default();
    let meta = ole::read_meta(&mut c);
    let (dir, doc_ref) = persist_dir(&doc, &user).unwrap_or_default();
    // The document container: the persist directory's, else the last one.
    let doc_c = dir
        .get(&doc_ref)
        .and_then(|&o| rec_at(&doc, o))
        .filter(|r| r.typ == DOCUMENT)
        .or_else(|| recs(&doc).into_iter().filter(|r| r.typ == DOCUMENT).last())
        .ok_or_else(|| Error::new("not a PowerPoint file"))?;
    let (mut w, mut h) = (720.0, 540.0);
    if let Some(a) = child(doc_c.body, DOCUMENT_ATOM) {
        let (x, y) = (i32_at(a.body, 0).unwrap_or(0), i32_at(a.body, 4).unwrap_or(0));
        if x > 0 && y > 0 {
            (w, h) = (x as f32 / 8.0, y as f32 / 8.0);
        }
    }
    let lists: Vec<Rec> = recs(doc_c.body).into_iter().filter(|r| r.typ == SLIDE_LIST).collect();
    let slides = lists.iter().find(|r| r.inst == 0).map(|r| slide_list(*r)).unwrap_or_default();
    let notes_list = lists.iter().find(|r| r.inst == 2).map(|r| slide_list(*r)).unwrap_or_default();
    let pics = pictures(doc_c.body, &pics);
    let container = |persist: u32, typ: u16| dir.get(&persist).and_then(|&o| rec_at(&doc, o)).filter(|r| r.typ == typ);
    // Lists whose entries share one container read it again each time:
    // what is read adds up to a few times the stream at most.
    let mut left = doc.len().saturating_mul(4).saturating_add(16 << 20);
    let mut charge = |len: usize| match left.checked_sub(len) {
        Some(l) => {
            left = l;
            true
        }
        None => false,
    };
    // Notes by the id of their slide.
    let mut notes: HashMap<u32, String> = HashMap::new();
    for n in &notes_list {
        let Some(nc) = container(n.persist, NOTES) else { continue };
        if !charge(nc.body.len()) {
            break;
        }
        let Some(slide) = child(nc.body, NOTES_ATOM).and_then(|a| u32_at(a.body, 0)) else { continue };
        // A text pointed at over and over is taken once.
        let mut used = HashSet::new();
        let text = drawing(nc.body)
            .into_iter()
            .filter_map(|f| match f {
                Found::Text { kind: 2, text, .. } => Some(text),
                Found::Ref { index, .. } if used.insert(index) => n.texts.get(index).filter(|t| t.0 == 2).map(|t| t.1.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            notes.insert(slide, text);
        }
    }

    let mut deck = build::new_deck(0);
    deck.width = w;
    deck.height = h;
    deck.masters = vec![Arc::new(build::new_master(0, w, h))];
    deck.slides.clear();
    deck.meta = meta;
    // The first master's pictures (logos and the like).
    let master = lists.iter().find(|r| r.inst == 1).map(|r| slide_list(*r)).unwrap_or_default();
    if let Some(mc) = master.first().and_then(|m| container(m.persist, MAIN_MASTER)) {
        let m = deck.master_mut(0);
        let mut id = m.objects.iter().map(Object::max_id).max().unwrap_or(1);
        let mut front = Vec::new();
        for f in drawing(mc.body) {
            let Found::Picture { pib, frame } = f else { continue };
            let Some(Some(data)) = pics.get((pib as usize).wrapping_sub(1)) else { continue };
            id += 1;
            front.push(picture(id, data.clone(), frame));
        }
        // Each one went in front of those before.
        front.reverse();
        m.objects.splice(0..0, front);
    }
    for s in slides.iter().take(apex_slides::MAX_SLIDES) {
        let c = container(s.persist, SLIDE);
        if !charge(c.map_or(0, |c| c.body.len()) + 64) {
            break;
        }
        let found = c.map(|c| drawing(c.body)).unwrap_or_default();
        // Placed texts; list texts no shape points at go in the placeholders.
        let mut placed: Vec<(u32, String, Option<Frame>)> = Vec::new();
        let mut used = HashSet::new();
        let mut picts = Vec::new();
        for f in found {
            match f {
                Found::Text { kind, text, frame } => placed.push((kind, text, frame)),
                // A text pointed at over and over is placed once.
                Found::Ref { index, frame } => {
                    if let Some((k, t)) = s.texts.get(index).filter(|_| used.insert(index)) {
                        placed.push((*k, t.clone(), frame));
                    }
                }
                Found::Picture { pib, frame } => picts.push((pib, frame)),
            }
        }
        let mut have: HashSet<(u32, String)> = placed.iter().map(|p| (p.0, p.1.clone())).collect();
        for (i, (k, t)) in s.texts.iter().enumerate() {
            if !used.contains(&i) && have.insert((*k, t.clone())) {
                placed.push((*k, t.clone(), None));
            }
        }
        let center = placed.iter().any(|p| p.0 == 6);
        let at = deck.slides.len();
        edit::add_slide(&mut deck, at, 0, if center { 0 } else { 1 });
        let slide = deck.slide_mut(at);
        let mut filled = HashSet::new();
        // The placeholders still free, titles and others, in order.
        let mut free: [std::collections::VecDeque<usize>; 2] = Default::default();
        for (i, o) in slide.objects.iter().enumerate().filter(|(_, o)| o.ph.is_some()) {
            free[o.is_title() as usize].push_back(i);
        }
        let mut next = slide.next_id();
        for (kind, text, frame) in placed {
            let ph = if kind == 4 { None } else { free[is_title(kind) as usize].pop_front() };
            match ph {
                Some(i) => {
                    let o = &mut slide.objects[i];
                    filled.insert(o.id);
                    if let Some(f) = frame {
                        o.sp.frame = Some(f);
                    }
                    if let Some(t) = o.text_or_new() {
                        t.set_plain(text.trim_end_matches('\n'));
                    }
                }
                None => {
                    let id = next;
                    next += 1;
                    let f = frame.unwrap_or_else(|| Frame::new(w * 0.1, h * 0.3, w * 0.8, h * 0.4));
                    let mut o = Object::text_box(id, f, TextBody::plain(text.trim_end_matches('\n')));
                    if let ObjKind::Shape { text: Some(t), .. } = &mut o.kind {
                        t.body.wrap = Some(true);
                    }
                    slide.objects.push(o);
                }
            }
        }
        // Placeholders the slide does not fill are left out.
        slide.objects.retain(|o| o.ph.is_none() || filled.contains(&o.id));
        let mut id = slide.next_id();
        let mut front = Vec::new();
        for (pib, frame) in picts {
            let Some(Some(data)) = pics.get((pib as usize).wrapping_sub(1)) else { continue };
            front.push(picture(id, data.clone(), frame));
            id += 1;
        }
        front.reverse();
        slide.objects.splice(0..0, front);
        if let Some(n) = notes.get(&s.id) {
            slide.notes = Some(Notes { text: TextBody::plain(n), origin: None });
        }
    }
    if deck.slides.is_empty() {
        edit::add_slide(&mut deck, 0, 0, 0);
    }
    Ok(deck)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(typ: u16, inst: u16, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        let ver = if matches!(typ, SLIDE_LIST | SP | SP_GROUP) { 0xF } else { 0 };
        v.extend_from_slice(&((inst << 4) | ver).to_le_bytes());
        v.extend_from_slice(&typ.to_le_bytes());
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn slide_list_texts() {
        let mut persist = vec![0u8; 20];
        persist[0] = 7;
        persist[12] = 256u32.to_le_bytes()[0];
        persist[13] = 1;
        let mut body = rec(SLIDE_PERSIST, 0, &persist);
        body.extend(rec(TEXT_HEADER, 0, &0u32.to_le_bytes()));
        body.extend(rec(TEXT_BYTES, 0, b"Hello\rworld"));
        body.extend(rec(TEXT_HEADER, 0, &1u32.to_le_bytes()));
        let chars: Vec<u8> = "Caf\u{e9}".encode_utf16().flat_map(u16::to_le_bytes).collect();
        body.extend(rec(TEXT_CHARS, 0, &chars));
        let all = rec(SLIDE_LIST, 0, &body);
        let l = slide_list(rec_at(&all, 0).unwrap());
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].persist, l[0].id), (7, 256));
        assert_eq!(l[0].texts, vec![(0, "Hello\nworld".to_string()), (1, "Caf\u{e9}".to_string())]);
    }

    #[test]
    fn anchors_in_points() {
        let mut a = Vec::new();
        for v in [80i16, 160, 800, 480] {
            a.extend_from_slice(&v.to_le_bytes());
        }
        let sp = rec(CLIENT_ANCHOR, 0, &a);
        let f = anchor(&sp, Map { sx: 1.0, sy: 1.0, ox: 0.0, oy: 0.0 }).unwrap();
        assert_eq!((f.x, f.y, f.w, f.h), (20.0, 10.0, 80.0, 50.0));
    }

    /// Reads every .ppt under $PPT_CORPUS.
    #[test]
    #[ignore]
    fn corpus() {
        let Ok(dir) = std::env::var("PPT_CORPUS") else { return };
        let mut stack = vec![std::path::PathBuf::from(dir)];
        let (mut ok, mut bad, mut empty) = (0, 0, 0);
        while let Some(d) = stack.pop() {
            for e in apex_fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if !p.extension().is_some_and(|x| x == "ppt") {
                    continue;
                }
                let data = apex_fs::read(&p).unwrap();
                match std::panic::catch_unwind(|| read(&data)) {
                    Ok(Ok(d)) => {
                        ok += 1;
                        if d.slides.iter().all(|s| s.objects.is_empty()) {
                            empty += 1;
                        }
                    }
                    Ok(Err(e)) => {
                        bad += 1;
                        eprintln!("{}: {e}", p.display());
                    }
                    Err(_) => {
                        bad += 1;
                        eprintln!("{}: PANIC", p.display());
                    }
                }
            }
        }
        eprintln!("ppt corpus: {ok} ok ({empty} without objects), {bad} bad");
    }
}
