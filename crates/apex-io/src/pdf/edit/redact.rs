//! True redaction: what lies under the marked areas is taken out of the
//! page's content, not just covered. Glyphs whose middle falls in an area
//! leave their strings (the gap is kept with a `TJ` offset, so the rest of
//! the line stays where it was), images lose the covered pixels (or go
//! whole when their codec cannot be rewritten), drawings inside an area go
//! and those across its edge are clipped. Black boxes are drawn over the
//! areas and the file must then be written whole, so no earlier revision
//! keeps what was removed. [`PdfDoc::sanitize`] drops metadata, scripts,
//! attachments and other hidden content.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use super::super::file::PdfFile;
use super::super::font::Font;
use super::super::image;
use super::super::obj::{Dict, Obj};
use super::annots::Kind;
use super::content::{self, Op};
use super::{PdfDoc, deflate};

type M = [f64; 6];

const ID: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(a: &M, b: &M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn apply(m: &M, x: f64, y: f64) -> [f64; 2] {
    [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]
}

fn invert(m: &M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

/// The bounding box of the unit square under `m`.
fn unit_box(m: &M) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        grow(&mut b, apply(m, x, y));
    }
    b
}

fn grow(b: &mut [f64; 4], p: [f64; 2]) {
    b[0] = b[0].min(p[0]);
    b[1] = b[1].min(p[1]);
    b[2] = b[2].max(p[0]);
    b[3] = b[3].max(p[1]);
}

/// What a redaction took out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Removed {
    pub glyphs: usize,
    pub images: usize,
    pub paths: usize,
    pub annots: usize,
}

/// Areas in a page's user space: x0, y0, x1, y1.
struct Areas(Vec<[f64; 4]>);

impl Areas {
    fn has(&self, p: [f64; 2]) -> bool {
        self.0.iter().any(|r| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3])
    }

    fn touch(&self, b: &[f64; 4]) -> bool {
        self.0.iter().any(|r| b[0] < r[2] && b[2] > r[0] && b[1] < r[3] && b[3] > r[1])
    }

    fn cover(&self, b: &[f64; 4]) -> bool {
        self.0.iter().any(|r| b[0] >= r[0] && b[2] <= r[2] && b[1] >= r[1] && b[3] <= r[3])
    }
}

#[derive(Clone)]
struct Gs {
    ctm: M,
    font: Option<Rc<Font>>,
    size: f64,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    rise: f64,
}

struct Walker<'a> {
    file: &'a PdfFile<'static>,
    areas: &'a Areas,
    fonts: HashMap<u32, Rc<Font>>,
    /// New objects by the numbers given out for them.
    new: Vec<(u32, Obj)>,
    next: u32,
    removed: Removed,
    depth: u32,
    /// The XObjects removed or replaced, as they were.
    touched: HashSet<u32>,
}

/// Where a path being built started in the output, and its box so far.
struct Path {
    start: usize,
    bbox: [f64; 4],
}

impl Walker<'_> {
    fn number(&mut self, o: Obj) -> u32 {
        let n = self.next;
        self.next += 1;
        self.new.push((n, o));
        n
    }

    fn font(&mut self, res: &Dict, name: &str) -> Option<Rc<Font>> {
        let fd = self.file.key_dict(res, "Font")?;
        let r = fd.get(name)?;
        let key = match r {
            Obj::Ref(n, _) => Some(*n),
            _ => None,
        };
        if let Some(f) = key.and_then(|n| self.fonts.get(&n)) {
            return Some(f.clone());
        }
        let d = self.file.resolve(r);
        let f = Rc::new(Font::load(self.file, d.dict()?));
        if let Some(n) = key {
            self.fonts.insert(n, f.clone());
        }
        Some(f)
    }

    /// Runs a content stream; returns the new operations and resources
    /// when something was removed.
    fn run(&mut self, ops: Vec<Op>, res: &Dict, ctm: M) -> Option<(Vec<Op>, Dict)> {
        let mut out: Vec<Op> = Vec::with_capacity(ops.len());
        let mut res = res.clone();
        let mut changed = false;
        let mut gs = Gs { ctm, font: None, size: 0.0, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, rise: 0.0 };
        let mut stack: Vec<Gs> = Vec::new();
        let (mut tm, mut tlm) = (ID, ID);
        let mut path: Option<Path> = None;
        for mut op in ops {
            match op.op.as_str() {
                "q" => stack.push(gs.clone()),
                "Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                "cm" if op.args.len() == 6 => {
                    let m = [op.num(0), op.num(1), op.num(2), op.num(3), op.num(4), op.num(5)];
                    gs.ctm = mul(&m, &gs.ctm);
                }
                "BT" => {
                    tm = ID;
                    tlm = ID;
                }
                "Tf" => {
                    gs.font = op.args.first().and_then(Obj::name).and_then(|n| self.font(&res, n));
                    gs.size = op.num(1);
                }
                "Tc" => gs.tc = op.num(0),
                "Tw" => gs.tw = op.num(0),
                "Tz" => gs.th = op.num(0) / 100.0,
                "TL" => gs.tl = op.num(0),
                "Ts" => gs.rise = op.num(0),
                "Td" | "TD" => {
                    if op.op == "TD" {
                        gs.tl = -op.num(1);
                    }
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, op.num(0), op.num(1)], &tlm);
                    tm = tlm;
                }
                "Tm" if op.args.len() == 6 => {
                    tlm = [op.num(0), op.num(1), op.num(2), op.num(3), op.num(4), op.num(5)];
                    tm = tlm;
                }
                "T*" => {
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                    tm = tlm;
                }
                "Tj" | "'" | "\"" | "TJ" => {
                    if op.op != "TJ" && op.op != "Tj" {
                        if op.op == "\"" {
                            gs.tw = op.num(0);
                            gs.tc = op.num(1);
                        }
                        tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                        tm = tlm;
                    }
                    let items: Vec<Obj> = match (op.op.as_str(), op.args.last()) {
                        ("TJ", Some(Obj::Array(a))) => a.clone(),
                        (_, Some(s @ Obj::Str(_))) => vec![s.clone()],
                        _ => Vec::new(),
                    };
                    if let Some(items) = self.text(&items, &gs, &mut tm) {
                        changed = true;
                        match op.op.as_str() {
                            "'" => out.push(Op::new("T*", Vec::new())),
                            "\"" => {
                                out.push(Op::new("Tw", vec![op.args[0].clone()]));
                                out.push(Op::new("Tc", vec![op.args[1].clone()]));
                                out.push(Op::new("T*", Vec::new()));
                            }
                            _ => {}
                        }
                        out.push(Op::new("TJ", vec![Obj::Array(items)]));
                        continue;
                    }
                }
                "m" | "l" | "c" | "v" | "y" | "re" | "h" => {
                    let p = path.get_or_insert(Path { start: out.len(), bbox: [f64::MAX, f64::MAX, f64::MIN, f64::MIN] });
                    if op.op == "re" {
                        let (x, y, w, h) = (op.num(0), op.num(1), op.num(2), op.num(3));
                        for (px, py) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
                            grow(&mut p.bbox, apply(&gs.ctm, px, py));
                        }
                    } else {
                        for pair in op.args.chunks(2).filter(|c| c.len() == 2) {
                            grow(&mut p.bbox, apply(&gs.ctm, pair[0].num().unwrap_or(0.0), pair[1].num().unwrap_or(0.0)));
                        }
                    }
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                    if let Some(p) = path.take()
                        && op.op != "n"
                        && p.start <= out.len()
                        && self.areas.touch(&p.bbox)
                    {
                        let clips = out[p.start..].iter().any(|o| o.op == "W" || o.op == "W*");
                        if clips {
                            // A clip is also set here: only its painting goes.
                            if self.areas.cover(&p.bbox) {
                                op = Op::new("n", Vec::new());
                                self.removed.paths += 1;
                                changed = true;
                            }
                        } else if self.areas.cover(&p.bbox) {
                            out.truncate(p.start);
                            self.removed.paths += 1;
                            changed = true;
                            continue;
                        } else if let Some(clip) = self.clip(&gs.ctm) {
                            let tail: Vec<Op> = out.drain(p.start..).collect();
                            out.push(Op::new("q", Vec::new()));
                            out.extend(clip);
                            out.extend(tail);
                            out.push(op);
                            out.push(Op::new("Q", Vec::new()));
                            self.removed.paths += 1;
                            changed = true;
                            continue;
                        }
                    }
                    path = None;
                }
                "BI" => {
                    if self.areas.touch(&unit_box(&gs.ctm)) {
                        self.removed.images += 1;
                        changed = true;
                        continue;
                    }
                }
                "scn" | "SCN" => {
                    if let Some(name) = op.args.last().and_then(Obj::name).map(str::to_string)
                        && let Some(new) = self.pattern(&res, &name)
                    {
                        let n = self.number(new);
                        let key = format!("Rp{n}");
                        let mut pd = self.file.key_dict(&res, "Pattern").unwrap_or_default();
                        pd.set(&key, Obj::Ref(n, 0));
                        res.set("Pattern", Obj::Dict(pd));
                        if let Some(last) = op.args.last_mut() {
                            *last = Obj::name_of(&key);
                        }
                        changed = true;
                    }
                }
                "Do" => {
                    if let Some(name) = op.args.first().and_then(Obj::name).map(str::to_string) {
                        let was = self.xobject(&res, &name, &gs.ctm);
                        if was.is_some()
                            && let Some(Obj::Ref(o, _)) = self.file.key_dict(&res, "XObject").and_then(|x| x.get(&name).cloned())
                        {
                            self.touched.insert(o);
                        }
                        match was {
                            Some(None) => {
                                changed = true;
                                continue;
                            }
                            Some(Some(new)) => {
                                let n = self.number(new);
                                let key = format!("Rd{n}");
                                let mut xd = self.file.key_dict(&res, "XObject").unwrap_or_default();
                                xd.set(&key, Obj::Ref(n, 0));
                                res.set("XObject", Obj::Dict(xd));
                                op.args[0] = Obj::name_of(&key);
                                changed = true;
                            }
                            None => {}
                        }
                    }
                }
                // Marked content may repeat the text it marks (for copying
                // and screen readers): it goes with the text.
                "BDC" | "DP" => {
                    if let Some(Obj::Dict(d)) = op.args.last_mut() {
                        strip_alternates(d);
                    }
                }
                _ => {}
            }
            out.push(op);
        }
        if !changed {
            return None;
        }
        if let Some(mut props) = self.file.key_dict(&res, "Properties") {
            for (_, v) in &mut props.0 {
                let mut d = match v {
                    Obj::Dict(d) => d.clone(),
                    Obj::Ref(..) => match self.file.resolve(v) {
                        Obj::Dict(d) => d.clone(),
                        _ => continue,
                    },
                    _ => continue,
                };
                if strip_alternates(&mut d) {
                    *v = Obj::Dict(d);
                }
            }
            res.set("Properties", Obj::Dict(props));
        }
        // XObjects no longer drawn leave the resources.
        if let Some(mut xd) = self.file.key_dict(&res, "XObject") {
            xd.0.retain(|(k, _)| out.iter().any(|o| o.op == "Do" && o.args.first().and_then(Obj::name) == Some(k)));
            res.set("XObject", Obj::Dict(xd));
        }
        Some((out, res))
    }

    /// The strings of one text operation without the glyphs in the areas;
    /// none when nothing was removed. Moves the text matrix on.
    fn text(&mut self, items: &[Obj], gs: &Gs, tm: &mut M) -> Option<Vec<Obj>> {
        let font = gs.font.clone().unwrap_or_else(|| Rc::new(Font::fallback()));
        let mut out: Vec<Obj> = Vec::new();
        let mut any = false;
        let push_num = |out: &mut Vec<Obj>, n: f64| {
            if let Some(Obj::Real(v)) = out.last_mut() {
                *v += n;
            } else {
                out.push(Obj::Real(n));
            }
        };
        for it in items {
            match it {
                Obj::Str(s) => {
                    let glyphs = font.glyphs(s);
                    let mut lens = font.code_lens(s);
                    if lens.len() != glyphs.len() || lens.iter().sum::<usize>() != s.len() {
                        // Codes that cannot be told apart go together.
                        lens = Vec::new();
                    }
                    let mut keep: Vec<u8> = Vec::new();
                    let mut at = 0;
                    let mut drop_all = false;
                    let mut gap = 0.0;
                    for (i, g) in glyphs.iter().enumerate() {
                        let trm = mul(&[gs.size * gs.th, 0.0, 0.0, gs.size, 0.0, gs.rise], &mul(tm, &gs.ctm));
                        let mid = apply(&trm, g.width / 2.0, 0.3);
                        let adv = (g.width * gs.size + gs.tc + if g.space { gs.tw } else { 0.0 }) * gs.th;
                        *tm = mul(&[1.0, 0.0, 0.0, 1.0, adv, 0.0], tm);
                        let gone = self.areas.has(mid);
                        if lens.is_empty() {
                            drop_all |= gone;
                            gap += adv;
                            continue;
                        }
                        let bytes = &s[at..at + lens[i]];
                        at += lens[i];
                        if gone {
                            if !keep.is_empty() {
                                out.push(Obj::Str(std::mem::take(&mut keep)));
                            }
                            if gs.size.abs() > 1e-9 && gs.th.abs() > 1e-9 {
                                push_num(&mut out, -adv / (gs.size * gs.th) * 1000.0);
                            }
                            self.removed.glyphs += 1;
                            any = true;
                        } else {
                            keep.extend_from_slice(bytes);
                        }
                    }
                    if lens.is_empty() {
                        if drop_all {
                            if gs.size.abs() > 1e-9 && gs.th.abs() > 1e-9 {
                                push_num(&mut out, -gap / (gs.size * gs.th) * 1000.0);
                            }
                            self.removed.glyphs += glyphs.len();
                            any = true;
                        } else {
                            out.push(it.clone());
                        }
                    } else if !keep.is_empty() {
                        out.push(Obj::Str(keep));
                    }
                }
                o => {
                    let n = o.num().unwrap_or(0.0);
                    *tm = mul(&[1.0, 0.0, 0.0, 1.0, -n / 1000.0 * gs.size * gs.th, 0.0], tm);
                    push_num(&mut out, n);
                }
            }
        }
        any.then_some(out)
    }

    /// A clip that leaves out the areas, in the space of `ctm`.
    fn clip(&self, ctm: &M) -> Option<Vec<Op>> {
        let inv = invert(ctm)?;
        let mut ops = Vec::new();
        let big = 1.0e5;
        let quad = |ops: &mut Vec<Op>, r: [f64; 4]| {
            let pts = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
            for (i, p) in pts.iter().enumerate() {
                let q = apply(&inv, p[0], p[1]);
                ops.push(Op::new(if i == 0 { "m" } else { "l" }, vec![Obj::Real(q[0]), Obj::Real(q[1])]));
            }
            ops.push(Op::new("h", Vec::new()));
        };
        quad(&mut ops, [-big, -big, big, big]);
        for r in &self.areas.0 {
            quad(&mut ops, *r);
        }
        ops.push(Op::new("W*", Vec::new()));
        ops.push(Op::new("n", Vec::new()));
        Some(ops)
    }

    /// An XObject drawn under `ctm`: none when untouched, `Some(None)` when
    /// it goes, or its replacement.
    fn xobject(&mut self, res: &Dict, name: &str, ctm: &M) -> Option<Option<Obj>> {
        let xd = self.file.key_dict(res, "XObject")?;
        let Obj::Stream(d, raw) = self.file.resolve(xd.get(name)?) else { return None };
        match self.file.key(&d, "Subtype").name() {
            Some("Image") => {
                let b = unit_box(ctm);
                if !self.areas.touch(&b) {
                    return None;
                }
                self.removed.images += 1;
                if self.areas.cover(&b) {
                    return Some(None);
                }
                Some(self.blank(&d, &raw, ctm))
            }
            Some("Form") => {
                let m = self.file.nums(d.get("Matrix").unwrap_or(&Obj::Null));
                let m: M = if m.len() == 6 { [m[0], m[1], m[2], m[3], m[4], m[5]] } else { ID };
                let inner = mul(&m, ctm);
                let bb = self.file.nums(d.get("BBox").unwrap_or(&Obj::Null));
                if bb.len() == 4 {
                    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                    for (x, y) in [(bb[0], bb[1]), (bb[2], bb[1]), (bb[0], bb[3]), (bb[2], bb[3])] {
                        grow(&mut b, apply(&inner, x, y));
                    }
                    if !self.areas.touch(&b) {
                        return None;
                    }
                }
                // Too deep to walk: it goes whole rather than keep what is under the area.
                if self.depth > 8 {
                    return Some(None);
                }
                let fres = self.file.key_dict(&d, "Resources").unwrap_or_else(|| res.clone());
                let data = self.file.decode(&d, &raw).data;
                self.depth += 1;
                let done = self.run(content::parse(&data), &fres, inner);
                self.depth -= 1;
                let (ops, fres) = done?;
                let mut nd = d.clone();
                for k in ["Length", "Filter", "DecodeParms"] {
                    nd.remove(k);
                }
                nd.set("Resources", Obj::Dict(fres));
                Some(Some(deflate(nd, &content::write(&ops))))
            }
            _ => None,
        }
    }

    /// Tiling pattern `name` emptied when its cell draws text or other
    /// objects, which are not walked; none when it only paints shapes.
    fn pattern(&mut self, res: &Dict, name: &str) -> Option<Obj> {
        let pd = self.file.key_dict(res, "Pattern")?;
        let Obj::Stream(d, raw) = self.file.resolve(pd.get(name)?) else { return None };
        if self.file.key(&d, "PatternType").int() != Some(1) {
            return None;
        }
        let ops = content::parse(&self.file.decode(&d, &raw).data);
        if !ops.iter().any(|o| matches!(o.op.as_str(), "Tj" | "TJ" | "'" | "\"" | "Do" | "BI")) {
            return None;
        }
        let mut nd = d.clone();
        for k in ["Length", "Filter", "DecodeParms"] {
            nd.remove(k);
        }
        Some(deflate(nd, b""))
    }

    /// An image with the covered pixels made black; none when its pixels
    /// cannot be read (it then goes whole).
    fn blank(&mut self, d: &Dict, raw: &[u8], ctm: &M) -> Option<Obj> {
        let (mut px, w, h, ch) = image::pixels(self.file, d, raw, 0)?;
        for y in 0..h {
            for x in 0..w {
                let p = apply(ctm, (x as f64 + 0.5) / w as f64, 1.0 - (y as f64 + 0.5) / h as f64);
                if self.areas.has(p) {
                    let i = (y * w + x) * ch;
                    px[i..i + 3].fill(0);
                    if ch == 4 {
                        px[i + 3] = 255;
                    }
                }
            }
        }
        let rgb: Vec<u8> = if ch == 4 { px.chunks(4).flat_map(|c| [c[0], c[1], c[2]]).collect() } else { px.clone() };
        let mut nd = Dict::of(vec![
            ("Type", Obj::name_of("XObject")),
            ("Subtype", Obj::name_of("Image")),
            ("Width", Obj::Int(w as i64)),
            ("Height", Obj::Int(h as i64)),
            ("ColorSpace", Obj::name_of("DeviceRGB")),
            ("BitsPerComponent", Obj::Int(8)),
        ]);
        if ch == 4 {
            let alpha: Vec<u8> = px.chunks(4).map(|c| c[3]).collect();
            let sd = Dict::of(vec![
                ("Type", Obj::name_of("XObject")),
                ("Subtype", Obj::name_of("Image")),
                ("Width", Obj::Int(w as i64)),
                ("Height", Obj::Int(h as i64)),
                ("ColorSpace", Obj::name_of("DeviceGray")),
                ("BitsPerComponent", Obj::Int(8)),
            ]);
            let s = self.number(deflate(sd, &alpha));
            nd.set("SMask", Obj::Ref(s, 0));
        }
        Some(deflate(nd, &rgb))
    }
}

impl PdfDoc {
    /// A read-only file over the objects as they are now, for fonts and
    /// images.
    fn as_file(&self) -> PdfFile<'static> {
        PdfFile::of_objects(self.objs.iter().map(|(&n, (_, o))| (n, o.clone())).collect(), self.trailer.clone())
    }

    /// Removes what lies under `areas` (page index and rectangles in view
    /// coordinates) and draws black boxes there; the file is written
    /// whole from now on.
    pub fn redact(&mut self, areas: &[(usize, Vec<[f32; 4]>)]) -> Removed {
        let pages = self.pages();
        let mut removed = Removed::default();
        let mut widgets = HashSet::new();
        let mut touched = HashSet::new();
        let mut any = false;
        self.begin("pdf.undo.redact");
        for (i, rects) in areas {
            let Some(&pn) = pages.get(*i) else { continue };
            if rects.is_empty() {
                continue;
            }
            any = true;
            let g = self.geom(pn);
            let user = Areas(rects.iter().map(|r| g.rect_to_user(*r)).collect());
            // Annotations over the areas go too.
            for a in self.annots(*i) {
                let b = g.rect_to_user(a.rect);
                if a.kind != Kind::Redact && user.touch(&b) {
                    if a.kind == Kind::Widget {
                        widgets.insert(a.id);
                    }
                    self.delete_annot(a.id);
                    removed.annots += 1;
                }
            }
            let file = self.as_file();
            let res = self.resources(pn);
            let ops = content::parse(&self.page_content(pn));
            let mut w = Walker { file: &file, areas: &user, fonts: HashMap::new(), new: Vec::new(), next: self.max_n() + 1, removed: Removed::default(), depth: 0, touched: HashSet::new() };
            let done = w.run(ops.clone(), &res, ID);
            let (new, wr) = (std::mem::take(&mut w.new), w.removed);
            touched.extend(w.touched.drain());
            drop(w);
            for (n, o) in new {
                self.set(n, o);
            }
            removed.glyphs += wr.glyphs;
            removed.images += wr.images;
            removed.paths += wr.paths;
            let (ops, res) = done.unwrap_or((ops, res));
            let mut data = b"q\n".to_vec();
            data.extend_from_slice(&content::write(&ops));
            data.extend_from_slice(b"\nQ\nq 0 g\n");
            for r in &user.0 {
                data.extend_from_slice(format!("{:.3} {:.3} {:.3} {:.3} re f\n", r[0], r[1], r[2] - r[0], r[3] - r[1]).as_bytes());
            }
            data.extend_from_slice(b"Q\n");
            let s = self.add(deflate(Dict::default(), &data));
            if let Some(mut d) = self.dict(pn) {
                d.set("Contents", Obj::Ref(s, 0));
                d.set("Resources", Obj::Dict(res));
                // The thumbnail still pictures the page as it was.
                d.remove("Thumb");
                self.set(pn, Obj::Dict(d));
            }
        }
        // Fields whose widgets went keep their values in the form.
        if !widgets.is_empty()
            && let Some((_, cat)) = self.catalog()
            && let Some(mut form) = self.key_dict(&cat, "AcroForm")
        {
            if let Obj::Array(fields) = self.key(&form, "Fields")
                && let Some(kept) = self.prune_fields(&fields, &widgets, 0)
            {
                form.set("Fields", Obj::Array(kept));
            }
            form.remove("XFA");
            self.set_catalog("AcroForm", Some(Obj::Dict(form)));
        }
        // What was redacted stays in the file while resources the page
        // inherits or shares still name it: unless some page shows it,
        // it goes.
        if !touched.is_empty() {
            let drawn = self.drawn_xobjects();
            for n in touched {
                if !drawn.contains(&n) {
                    self.remove(n);
                }
            }
        }
        // Tags may repeat the text as alternate or actual text.
        if any {
            self.set_catalog("StructTreeRoot", None);
            self.set_catalog("MarkInfo", None);
        }
        self.need_whole();
        self.end();
        removed
    }

    /// The XObjects the pages draw, forms' own included.
    fn drawn_xobjects(&self) -> HashSet<u32> {
        fn walk(doc: &PdfDoc, data: &[u8], res: &Dict, depth: u32, out: &mut HashSet<u32>) {
            let Some(xd) = doc.key_dict(res, "XObject") else { return };
            for op in content::parse(data) {
                if op.op != "Do" {
                    continue;
                }
                let Some(Obj::Ref(n, _)) = op.args.first().and_then(Obj::name).and_then(|k| xd.get(k)) else { continue };
                if !out.insert(*n) || depth > 8 {
                    continue;
                }
                if let Obj::Stream(d, raw) = doc.resolve(&Obj::Ref(*n, 0))
                    && doc.key(&d, "Subtype").name() == Some("Form")
                {
                    let fres = doc.key_dict(&d, "Resources").unwrap_or_else(|| res.clone());
                    walk(doc, &doc.decode(&d, &raw), &fres, depth + 1, out);
                }
            }
        }
        let mut out = HashSet::new();
        for pn in self.pages() {
            walk(self, &self.page_content(pn), &self.resources(pn), 0, &mut out);
        }
        out
    }

    /// `list` (a form's `Fields` or a field's `Kids`) without the widgets
    /// in `gone`: a field left with none goes too, and one that lost some
    /// forgets its value. None when nothing changed.
    pub(super) fn prune_fields(&mut self, list: &[Obj], gone: &HashSet<u32>, depth: u32) -> Option<Vec<Obj>> {
        let mut changed = false;
        let mut out = Vec::with_capacity(list.len());
        for o in list {
            let Obj::Ref(n, _) = o else {
                out.push(o.clone());
                continue;
            };
            if gone.contains(n) {
                changed = true;
                continue;
            }
            if depth < 32
                && let Some(mut d) = self.dict(*n)
                && let Obj::Array(kids) = self.key(&d, "Kids")
                && let Some(kept) = self.prune_fields(&kids, gone, depth + 1)
            {
                changed = true;
                if kept.is_empty() {
                    continue;
                }
                d.set("Kids", Obj::Array(kept));
                d.remove("V");
                self.set(*n, Obj::Dict(d));
            }
            out.push(o.clone());
        }
        changed.then_some(out)
    }

    /// The areas marked for redaction (Redact annotations) by page.
    pub fn redaction_marks(&self) -> Vec<(usize, Vec<[f32; 4]>)> {
        let mut out: Vec<(usize, Vec<[f32; 4]>)> = Vec::new();
        for i in 0..self.pages().len() {
            let rects: Vec<[f32; 4]> = self.annots(i).into_iter().filter(|a| a.kind == Kind::Redact).flat_map(|a| if a.quads.is_empty() { vec![a.rect] } else { a.quads }).collect();
            if !rects.is_empty() {
                out.push((i, rects));
            }
        }
        out
    }

    /// Applies the marks: removes what is under them and the marks
    /// themselves, in one step.
    pub fn apply_redactions(&mut self) -> Removed {
        let marks = self.redaction_marks();
        self.begin("pdf.undo.redact");
        for a in self.all_annots() {
            if a.kind == Kind::Redact {
                self.delete_annot(a.id);
            }
        }
        let r = self.redact(&marks);
        self.end();
        r
    }

    /// Readies a copy for another program to print: what [`sanitize`]
    /// drops, and the printout showing what is shown here. Annotations not
    /// shown go and the others print; optional content is set as shown,
    /// with nothing that changes it for printing.
    ///
    /// [`sanitize`]: PdfDoc::sanitize
    pub fn for_print(&mut self) {
        self.sanitize();
        let Some((_, cat)) = self.catalog() else { return };
        let off = crate::pdf::render::ocg_off_in(&cat, &|o| self.resolve(o));
        for pn in self.pages() {
            let (_, list) = self.annots_of(pn);
            let mut keep = Vec::new();
            let mut changed = false;
            for o in list {
                let Some(mut d) = self.resolve(&o).dict().cloned() else {
                    keep.push(o);
                    continue;
                };
                let flags = self.key(&d, "F").int().unwrap_or(0);
                let popup = self.key(&d, "Subtype").name() == Some("Popup");
                let hidden = flags & (2 | 32) != 0 || d.get("OC").is_some_and(|oc| crate::pdf::render::oc_hidden(oc, &off, &|o| self.resolve(o)));
                if popup || hidden {
                    changed = true;
                    continue;
                }
                if flags & 4 == 0 {
                    d.set("F", Obj::Int(flags | 4));
                    match o {
                        Obj::Ref(n, _) => self.set(n, Obj::Dict(d)),
                        _ => {
                            keep.push(Obj::Dict(d));
                            changed = true;
                            continue;
                        }
                    }
                }
                keep.push(o);
            }
            if changed {
                self.set_annots_of(pn, keep);
            }
        }
        if let Some((_, cat)) = self.catalog()
            && let Some(mut ocp) = self.key_dict(&cat, "OCProperties")
        {
            ocp.remove("Configs");
            let mut d = self.key_dict(&ocp, "D").unwrap_or_default();
            for k in ["AS", "BaseState", "ON", "OFF"] {
                d.remove(k);
            }
            let mut off: Vec<u32> = off.into_iter().collect();
            off.sort_unstable();
            d.set("OFF", Obj::Array(off.into_iter().map(|n| Obj::Ref(n, 0)).collect()));
            ocp.set("D", Obj::Dict(d));
            self.set_catalog("OCProperties", Some(Obj::Dict(ocp)));
        }
    }

    /// Drops what a reader does not show: document information and XMP
    /// metadata, scripts and automatic actions, attached files, page
    /// thumbnails, private application data and hidden annotations.
    pub fn sanitize(&mut self) {
        self.begin("pdf.undo.sanitize");
        let mut t = self.trailer.clone();
        if t.remove("Info").is_some() {
            self.set_trailer(t);
        }
        for k in ["Metadata", "OpenAction", "AA", "PieceInfo", "SpiderInfo", "Threads", "AF", "Collection"] {
            self.set_catalog(k, None);
        }
        if let Some((_, cat)) = self.catalog()
            && let Some(mut names) = self.key_dict(&cat, "Names")
        {
            names.remove("JavaScript");
            names.remove("EmbeddedFiles");
            self.set_catalog("Names", Some(Obj::Dict(names)));
        }
        let hidden: Vec<u32> = self.all_annots().into_iter().filter(|a| a.hidden).map(|a| a.id).collect();
        for id in hidden {
            self.delete_annot(id);
        }
        for pn in self.pages() {
            if let Some(mut d) = self.dict(pn) {
                let before = d.0.len();
                d.0.retain(|(k, _)| !matches!(&**k, "Thumb" | "PieceInfo" | "Metadata" | "AA" | "AF"));
                if d.0.len() != before {
                    self.set(pn, Obj::Dict(d));
                }
            }
        }
        // Scripts and attachments anywhere else: actions that run code or
        // programs, automatic actions, form scripts, attached files.
        if let Some((_, cat)) = self.catalog()
            && let Some(mut form) = self.key_dict(&cat, "AcroForm")
            && form.remove("XFA").is_some()
        {
            self.set_catalog("AcroForm", Some(Obj::Dict(form)));
        }
        let attached: Vec<u32> = self
            .objs
            .iter()
            .filter(|(_, (_, o))| matches!(o, Obj::Dict(d) if d.get("Subtype").and_then(Obj::name) == Some("FileAttachment")))
            .map(|(&n, _)| n)
            .collect();
        for id in attached {
            self.delete_annot(id);
        }
        let runs = |d: &Dict| matches!(d.get("S").and_then(Obj::name), Some("JavaScript" | "Launch" | "ImportData" | "SubmitForm" | "Rendition" | "GoToE"));
        let ids: Vec<u32> = self.objs.keys().copied().collect();
        for n in ids {
            let Some(o) = self.objs.get(&n).map(|e| e.1.clone()) else { continue };
            let (Obj::Dict(mut d) | Obj::Stream(mut d, _)) = o.clone() else { continue };
            let before = d.0.len();
            let ran = runs(&d);
            d.remove("AA");
            // Files associated with anything, and private application data
            // (an editor's copy of the original) anywhere.
            d.remove("AF");
            d.remove("PieceInfo");
            // Actions that follow (not the next bookmark).
            if d.get("S").is_some() {
                d.remove("Next");
            }
            let action = match d.get("A") {
                Some(Obj::Dict(a)) => runs(a),
                Some(r @ Obj::Ref(..)) => matches!(self.resolve(r), Obj::Dict(a) if runs(&a)),
                _ => false,
            };
            if action {
                d.remove("A");
            }
            // An action of its own that runs something does nothing now.
            if ran {
                d.remove("JS");
                d.remove("F");
                d.remove("Win");
                d.remove("Mac");
                d.remove("Unix");
                d.set("S", Obj::name_of("GoTo"));
                d.set("D", Obj::Array(Vec::new()));
            }
            if d.0.len() != before || action || ran {
                self.set(n, match o {
                    Obj::Stream(_, raw) => Obj::Stream(d, raw),
                    _ => Obj::Dict(d),
                });
            }
        }
        // Metadata streams of any object (images, fonts, forms).
        let with_meta: Vec<u32> = self.objs.iter().filter(|(_, (_, o))| matches!(o, Obj::Dict(d) | Obj::Stream(d, _) if d.get("Metadata").is_some())).map(|(&n, _)| n).collect();
        for n in with_meta {
            let o = match self.objs.get(&n).map(|e| e.1.clone()) {
                Some(Obj::Dict(mut d)) => {
                    d.remove("Metadata");
                    Obj::Dict(d)
                }
                Some(Obj::Stream(mut d, raw)) => {
                    d.remove("Metadata");
                    Obj::Stream(d, Arc::clone(&raw))
                }
                _ => continue,
            };
            self.set(n, o);
        }
        self.need_whole();
        self.end();
    }
}

/// Removes the text a marked-content property list gives in place of what
/// it marks; whether there was any.
fn strip_alternates(d: &mut Dict) -> bool {
    let before = d.0.len();
    d.0.retain(|(k, _)| !matches!(&**k, "ActualText" | "Alt" | "E"));
    d.0.len() != before
}
