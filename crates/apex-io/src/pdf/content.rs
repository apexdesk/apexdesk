//! Runs a page's content streams and collects what they draw: characters
//! with their position, font and colour; paths; and pictures. Coordinates
//! are in points from the top left corner of the page as shown.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use super::file::{PageRef, PdfFile};
use super::font::{Font, Style};
use super::image::{self, Space};
use super::obj::{Dict, Item, Lexer, Obj};

#[derive(Clone, Debug, PartialEq)]
pub struct Char {
    pub ch: char,
    /// Left end of the baseline.
    pub x: f32,
    pub y: f32,
    /// Advance width.
    pub w: f32,
    /// Font size as drawn.
    pub size: f32,
    /// Index in `Page::fonts`.
    pub font: u16,
    pub color: u32,
    /// Horizontal, left to right.
    pub upright: bool,
    /// Drawn invisible (the text layer of a scanned page).
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub points: Vec<[f32; 2]>,
    pub closed: bool,
    /// Drawn with `re`.
    pub rect: bool,
    pub fill: Option<u32>,
    pub stroke: Option<(u32, f32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageImage {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// PNG or JPEG.
    pub data: Arc<[u8]>,
    pub pixels: (u32, u32),
}

/// What a page draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub fonts: Vec<Style>,
    pub chars: Vec<Char>,
    pub shapes: Vec<Shape>,
    pub images: Vec<PageImage>,
    /// Link areas (x0, y0, x1, y1) and their targets.
    pub links: Vec<([f32; 4], Arc<str>)>,
}

type M = [f64; 6];

fn mul(m: &M, n: &M) -> M {
    [m[0] * n[0] + m[1] * n[2], m[0] * n[1] + m[1] * n[3], m[2] * n[0] + m[3] * n[2], m[2] * n[1] + m[3] * n[3], m[4] * n[0] + m[5] * n[2] + n[4], m[4] * n[1] + m[5] * n[3] + n[5]]
}

fn apply(m: &M, x: f64, y: f64) -> [f64; 2] {
    [x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5]]
}

#[derive(Clone)]
struct Gs {
    ctm: M,
    fill: u32,
    stroke: u32,
    fill_space: (Space, usize),
    stroke_space: (Space, usize),
    line_width: f64,
    font: Option<(Rc<Font>, u16)>,
    size: f64,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    rise: f64,
    mode: i64,
}

struct Sub {
    pts: Vec<[f64; 2]>,
    closed: bool,
    rect: bool,
}

struct Run<'a, 'f> {
    f: &'a PdfFile<'f>,
    page: Page,
    fonts: HashMap<u32, (Rc<Font>, u16)>,
    images: HashMap<u32, Option<(Arc<[u8]>, (u32, u32))>>,
    forms: Vec<u32>,
    ops: usize,
    fallback: Option<(Rc<Font>, u16)>,
    /// Pixels of pictures decoded so far (to bound memory).
    pixels: usize,
    /// Characters the file may still give its pages.
    chars: usize,
    /// Fonts written into a resource dictionary (not cached by number)
    /// the page may still load.
    inline_fonts: usize,
    /// Whose resources are read: the form (by number) that has them, or
    /// `u32::MAX` for the page.
    owner: u32,
    /// Resource dictionaries by owner and kind, and resources by owner,
    /// kind and name, each copied once: operators naming them over and
    /// over would copy a large dictionary each time.
    /// Dictionaries written apart are kept by number instead, copied
    /// once whoever names them; each copy costs operators by its size.
    kinds: HashMap<(u32, &'static str), Option<Rc<Dict>>>,
    named: HashMap<(u32, &'static str, Arc<str>), Rc<Obj>>,
    apart: HashMap<u32, Rc<Obj>>,
    apart_dicts: HashMap<u32, Option<Rc<Dict>>>,
    /// The number the next form written inline reads its resources by.
    inline: u32,
    /// Bytes of content the file's pages may still decode.
    content: usize,
    /// Forms' own resources, by number.
    form_res: HashMap<u32, Option<Rc<Dict>>>,
    /// Optional content off as shown, the marked sections open (whether
    /// each is off), and how many of them are: what isn't shown isn't
    /// read either.
    ocg_off: HashSet<u32>,
    marked: Vec<bool>,
    marked_off: usize,
}

/// What a file's pages may still take, all together.
pub struct Budget {
    /// Pixels of pictures decoded.
    pub pixels: usize,
    /// Characters of text.
    pub chars: usize,
    /// Bytes of page content decoded.
    pub content: usize,
}

const MAX_OPS: usize = 4_000_000;
const MAX_SHAPES: usize = 200_000;

/// The matrix from user space to the page as shown (top-left origin).
fn page_matrix(p: &PageRef) -> (M, f32, f32) {
    let [x0, y0, x1, y1] = p.bbox;
    let (w, h) = (x1 - x0, y1 - y0);
    match p.rotate {
        90 => ([0.0, 1.0, 1.0, 0.0, -y0, -x0], h as f32, w as f32),
        180 => ([-1.0, 0.0, 0.0, 1.0, x1, -y0], w as f32, h as f32),
        270 => ([0.0, -1.0, -1.0, 0.0, y1, x1], h as f32, w as f32),
        _ => ([1.0, 0.0, 0.0, -1.0, -x0, y1], w as f32, h as f32),
    }
}

/// Reads what page `p` draws.
pub fn page(f: &PdfFile, p: &PageRef, budget: &mut Budget) -> Page {
    let (m0, width, height) = page_matrix(p);
    let mut run = Run { f, page: Page { width, height, ..Default::default() }, fonts: HashMap::new(), images: HashMap::new(), forms: Vec::new(), ops: 0, fallback: None, pixels: budget.pixels, chars: budget.chars, inline_fonts: 256, owner: u32::MAX, kinds: HashMap::new(), named: HashMap::new(), apart: HashMap::new(), apart_dicts: HashMap::new(), inline: u32::MAX - 1, content: 0, form_res: HashMap::new(), ocg_off: super::render::ocg_off(f), marked: Vec::new(), marked_off: 0 };
    let gs = Gs { ctm: m0, fill: 0, stroke: 0, fill_space: (Space::Gray, 1), stroke_space: (Space::Gray, 1), line_width: 1.0, font: None, size: 0.0, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, rise: 0.0, mode: 0 };
    let content = if budget.content > 0 { f.contents(&p.dict) } else { Vec::new() };
    budget.content = budget.content.saturating_sub(content.len());
    run.content = budget.content;
    run.run(&content, &p.resources, gs);
    // Links.
    if let Obj::Array(annots) = f.key(&p.dict, "Annots") {
        for a in annots.iter().take(5000) {
            let Some(ad) = f.resolve(a).dict().cloned() else { continue };
            if f.key(&ad, "Subtype").name() != Some("Link") {
                continue;
            }
            // Links not shown (hidden, or in optional content off) can't
            // be clicked either.
            let flags = f.key_num(&ad, "F").unwrap_or(0.0) as i64;
            if flags & (2 | 32) != 0 || ad.get("OC").is_some_and(|o| run.oc_off(o)) {
                continue;
            }
            let r = f.nums(ad.get("Rect").unwrap_or(&Obj::Null));
            if r.len() != 4 {
                continue;
            }
            let uri = f.key_dict(&ad, "A").and_then(|act| f.key(&act, "URI").bytes().map(super::text_string));
            let Some(uri) = uri.filter(|u| !u.is_empty()) else { continue };
            let a = apply(&m0, r[0], r[1]);
            let b = apply(&m0, r[2], r[3]);
            run.page.links.push(([a[0].min(b[0]) as f32, a[1].min(b[1]) as f32, a[0].max(b[0]) as f32, a[1].max(b[1]) as f32], Arc::from(uri)));
        }
    }
    budget.pixels = run.pixels;
    budget.chars = run.chars;
    budget.content = run.content;
    run.page
}

impl Run<'_, '_> {
    /// Whether optional content `o` is off as shown.
    fn oc_off(&self, o: &Obj) -> bool {
        let f = self.f;
        super::render::oc_hidden(o, &self.ocg_off, &|o| f.resolve(o))
    }

    /// Reads a content stream; marked sections it leaves open end with it.
    fn run(&mut self, content: &[u8], res: &Dict, gs: Gs) {
        let start = self.marked.len();
        self.run_ops(content, res, gs);
        if let Some(left) = self.marked.get(start..) {
            self.marked_off -= left.iter().filter(|&&b| b).count();
        }
        self.marked.truncate(start);
    }

    fn run_ops(&mut self, content: &[u8], res: &Dict, mut gs: Gs) {
        let f = self.f;
        let mut lx = Lexer::new(content, 0);
        let mut args: Vec<Obj> = Vec::new();
        let mut stack: Vec<Gs> = Vec::new();
        let mut path: Vec<Sub> = Vec::new();
        let mut tm: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut tlm = tm;
        let num = |a: &[Obj], i: usize| a.get(i).and_then(Obj::num).unwrap_or(0.0);
        while let Some(item) = lx.next(false) {
            let op = match item {
                Item::Obj(o) => {
                    if args.len() < 64 {
                        args.push(o);
                    }
                    continue;
                }
                Item::Kw(k) => k,
            };
            self.ops += 1;
            if self.ops > MAX_OPS {
                return;
            }
            let a = &args;
            match op {
                b"q" => {
                    if stack.len() < 256 {
                        stack.push(gs.clone());
                    }
                }
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                b"cm" if a.len() >= 6 => {
                    let m = [num(a, 0), num(a, 1), num(a, 2), num(a, 3), num(a, 4), num(a, 5)];
                    gs.ctm = mul(&m, &gs.ctm);
                }
                b"w" => gs.line_width = num(a, 0),
                // Colours.
                b"g" => (gs.fill, gs.fill_space) = (gray(num(a, 0)), (Space::Gray, 1)),
                b"G" => (gs.stroke, gs.stroke_space) = (gray(num(a, 0)), (Space::Gray, 1)),
                b"rg" => (gs.fill, gs.fill_space) = (color(Space::Rgb, a), (Space::Rgb, 3)),
                b"RG" => (gs.stroke, gs.stroke_space) = (color(Space::Rgb, a), (Space::Rgb, 3)),
                b"k" => (gs.fill, gs.fill_space) = (color(Space::Cmyk, a), (Space::Cmyk, 4)),
                b"K" => (gs.stroke, gs.stroke_space) = (color(Space::Cmyk, a), (Space::Cmyk, 4)),
                b"cs" | b"CS" => {
                    let sp = match a.first() {
                        Some(Obj::Name(n)) => match &**n {
                            "DeviceGray" | "DeviceRGB" | "DeviceCMYK" | "Pattern" => image::space(f, &Obj::Name(n.clone())),
                            _ => image::space(f, &self.resource(res, "ColorSpace", n)),
                        },
                        _ => (Space::Gray, 1, None),
                    };
                    let s = (sp.0, sp.1);
                    let initial = 0;
                    if op == b"cs" {
                        (gs.fill_space, gs.fill) = (s, initial);
                    } else {
                        (gs.stroke_space, gs.stroke) = (s, initial);
                    }
                }
                b"sc" | b"scn" => gs.fill = color(gs.fill_space.0, a),
                b"SC" | b"SCN" => gs.stroke = color(gs.stroke_space.0, a),
                // Paths.
                b"m" => path.push(Sub { pts: vec![apply(&gs.ctm, num(a, 0), num(a, 1))], closed: false, rect: false }),
                b"l" => {
                    let p = apply(&gs.ctm, num(a, 0), num(a, 1));
                    match path.last_mut() {
                        Some(s) if !s.closed => s.pts.push(p),
                        _ => path.push(Sub { pts: vec![p], closed: false, rect: false }),
                    }
                }
                b"c" | b"v" | b"y" => {
                    let Some(s) = path.last_mut() else { continue };
                    let Some(&p0) = s.pts.last() else { continue };
                    let pt = |i: usize| apply(&gs.ctm, num(a, i), num(a, i + 1));
                    let (p1, p2, p3) = match op {
                        b"c" => (pt(0), pt(2), pt(4)),
                        b"v" => (p0, pt(0), pt(2)),
                        _ => (pt(0), pt(2), pt(2)),
                    };
                    for i in 1..=8 {
                        let t = i as f64 / 8.0;
                        let u = 1.0 - t;
                        let b = |k: usize| u * u * u * p0[k] + 3.0 * u * u * t * p1[k] + 3.0 * u * t * t * p2[k] + t * t * t * p3[k];
                        s.pts.push([b(0), b(1)]);
                    }
                }
                b"h" => {
                    if let Some(s) = path.last_mut() {
                        s.closed = true;
                    }
                }
                b"re" => {
                    let (x, y, w, h) = (num(a, 0), num(a, 1), num(a, 2), num(a, 3));
                    let pts = vec![apply(&gs.ctm, x, y), apply(&gs.ctm, x + w, y), apply(&gs.ctm, x + w, y + h), apply(&gs.ctm, x, y + h)];
                    path.push(Sub { pts, closed: true, rect: true });
                }
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    let stroke = matches!(op, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                    let fill = matches!(op, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
                    let close = matches!(op, b"s" | b"b" | b"b*");
                    if (stroke || fill) && self.page.shapes.len() < MAX_SHAPES && self.marked_off == 0 {
                        let scale = (gs.ctm[0] * gs.ctm[3] - gs.ctm[1] * gs.ctm[2]).abs().sqrt();
                        for s in path.drain(..) {
                            if s.pts.len() < 2 && !s.rect {
                                continue;
                            }
                            self.page.shapes.push(Shape {
                                points: s.pts.iter().map(|p| [p[0] as f32, p[1] as f32]).collect(),
                                closed: s.closed || close,
                                rect: s.rect,
                                fill: fill.then_some(gs.fill),
                                stroke: stroke.then_some((gs.stroke, (gs.line_width * scale).max(0.0) as f32)),
                            });
                        }
                    }
                    path.clear();
                }
                // Text.
                b"BT" => {
                    tm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                    tlm = tm;
                }
                b"Tc" => gs.tc = num(a, 0),
                b"Tw" => gs.tw = num(a, 0),
                b"Tz" => gs.th = num(a, 0) / 100.0,
                b"TL" => gs.tl = num(a, 0),
                b"Ts" => gs.rise = num(a, 0),
                b"Tr" => gs.mode = num(a, 0) as i64,
                b"Tf" => {
                    gs.size = num(a, 1);
                    if let Some(Obj::Name(n)) = a.first() {
                        gs.font = self.font(res, n);
                    }
                }
                b"Td" | b"TD" => {
                    if op == b"TD" {
                        gs.tl = -num(a, 1);
                    }
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, num(a, 0), num(a, 1)], &tlm);
                    tm = tlm;
                }
                b"Tm" if a.len() >= 6 => {
                    tlm = [num(a, 0), num(a, 1), num(a, 2), num(a, 3), num(a, 4), num(a, 5)];
                    tm = tlm;
                }
                b"T*" => {
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                    tm = tlm;
                }
                b"Tj" | b"'" | b"\"" | b"TJ" => {
                    if op == b"\"" {
                        gs.tw = num(a, 0);
                        gs.tc = num(a, 1);
                    }
                    if op == b"'" || op == b"\"" {
                        tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                        tm = tlm;
                    }
                    if gs.font.is_none() {
                        gs.font = self.fallback_font();
                    }
                    match (op, a.last()) {
                        (b"TJ", Some(Obj::Array(items))) => {
                            for it in items {
                                match it {
                                    Obj::Str(s) => self.show(s, &gs, &mut tm),
                                    o => {
                                        let d = o.num().unwrap_or(0.0);
                                        tm = mul(&[1.0, 0.0, 0.0, 1.0, -d / 1000.0 * gs.size * gs.th, 0.0], &tm);
                                    }
                                }
                            }
                        }
                        (_, Some(Obj::Str(s))) => self.show(s, &gs, &mut tm),
                        _ => {}
                    }
                }
                b"gs" => {
                    if let Some(Obj::Name(n)) = a.first()
                        && let Obj::Dict(d) = &*self.resource(res, "ExtGState", n)
                        && let Some(Obj::Array(fa)) = d.get("Font")
                        && let (Some(Obj::Ref(fr, _)), Some(sz)) = (fa.first(), fa.get(1).and_then(Obj::num))
                    {
                        gs.font = self.font_ref(*fr);
                        gs.size = sz;
                    }
                }
                b"Do" => {
                    if let Some(Obj::Name(n)) = a.first()
                        && self.marked_off == 0
                    {
                        self.xobject(res, n, &gs);
                    }
                }
                // Optional content.
                b"BMC" => self.marked.push(false),
                b"BDC" => {
                    let off = match (a.first(), a.get(1)) {
                        (Some(Obj::Name(t)), Some(Obj::Name(n))) if &**t == "OC" => {
                            let o = self.kind(res, "Properties").and_then(|d| d.get(n).cloned());
                            o.is_some_and(|o| self.oc_off(&o))
                        }
                        _ => false,
                    };
                    self.marked.push(off);
                    self.marked_off += off as usize;
                }
                b"EMC" => {
                    if self.marked.pop() == Some(true) {
                        self.marked_off -= 1;
                    }
                }
                b"BI" => {
                    // Inline images are skipped up to their end.
                    let d = lx.d;
                    let Some(id) = memchr::memmem::find(&d[lx.p..], b"ID") else {
                        // No image data follows: nothing more to read.
                        lx.p = d.len();
                        continue;
                    };
                    {
                        let start = (lx.p + id + 3).min(d.len());
                        let mut end = d.len();
                        let mut i = start;
                        while let Some(e) = memchr::memmem::find(&d[i..], b"EI") {
                            let at = i + e;
                            if at > 0 && super::obj::is_ws(d[at - 1]) && d.get(at + 2).is_none_or(|&b| super::obj::is_ws(b)) {
                                end = at + 2;
                                break;
                            }
                            i = at + 2;
                        }
                        lx.p = end.min(d.len());
                    }
                }
                _ => {}
            }
            args.clear();
        }
    }

    /// The resource dictionary of `kind` in `res` (see `kinds`).
    fn kind(&mut self, res: &Dict, kind: &'static str) -> Option<Rc<Dict>> {
        if let Some(d) = self.kinds.get(&(self.owner, kind)) {
            return d.clone();
        }
        let d = self.dict_at(res, kind);
        self.kinds.insert((self.owner, kind), d.clone());
        d
    }

    /// The dictionary `d[key]`: one written apart is copied once for the
    /// whole page (by number), others each time; each copy is charged.
    fn dict_at(&mut self, d: &Dict, key: &str) -> Option<Rc<Dict>> {
        let n = match d.get(key) {
            Some(Obj::Ref(n, _)) => *n,
            _ => {
                let d = self.f.key_dict(d, key)?;
                self.charge(d.0.iter().fold(1usize, |w, (_, o)| w.saturating_add(o.weight())));
                return Some(Rc::new(d));
            }
        };
        if let Some(d) = self.apart_dicts.get(&n) {
            return d.clone();
        }
        let o = self.f.resolve(&Obj::Ref(n, 0));
        self.charge(o.weight());
        let d = match o {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(Rc::new(d)),
            _ => None,
        };
        self.apart_dicts.insert(n, d.clone());
        d
    }

    /// Object `n`, copied once for the whole page.
    fn apart_obj(&mut self, n: u32) -> Rc<Obj> {
        if let Some(o) = self.apart.get(&n) {
            return o.clone();
        }
        let o = Rc::new(self.f.resolve(&Obj::Ref(n, 0)));
        self.charge(o.weight());
        self.apart.insert(n, o.clone());
        o
    }

    /// Copying `weight` worth of objects costs operators.
    fn charge(&mut self, weight: usize) {
        self.ops = self.ops.saturating_add(weight / 4);
    }

    /// The resource of `kind` named `name` in `res`, resolved.
    fn resource(&mut self, res: &Dict, kind: &'static str, name: &str) -> Rc<Obj> {
        if let Some(o) = self.named.get(&(self.owner, kind, Arc::from(name))) {
            return o.clone();
        }
        let o = match self.kind(res, kind) {
            None => Rc::new(Obj::Null),
            Some(d) => match d.get(name) {
                Some(Obj::Ref(n, _)) => self.apart_obj(*n),
                Some(o) => {
                    self.charge(o.weight());
                    Rc::new(o.clone())
                }
                None => Rc::new(Obj::Null),
            },
        };
        self.named.insert((self.owner, kind, Arc::from(name)), o.clone());
        o
    }

    /// The font object number `n`.
    fn font_ref(&mut self, n: u32) -> Option<(Rc<Font>, u16)> {
        if let Some((font, _)) = self.fonts.get(&n).cloned() {
            let idx = self.style_index(&font.style);
            return Some((font, idx));
        }
        let o = self.f.get(n);
        self.font_obj(n, &o)
    }

    fn fallback_font(&mut self) -> Option<(Rc<Font>, u16)> {
        if self.fallback.is_none() {
            let font = Font::fallback();
            let idx = self.style_index(&font.style);
            self.fallback = Some((Rc::new(font), idx));
        }
        self.fallback.clone()
    }

    fn style_index(&mut self, s: &Style) -> u16 {
        match self.page.fonts.iter().position(|x| x == s) {
            Some(i) => i as u16,
            None => {
                self.page.fonts.push(s.clone());
                (self.page.fonts.len() - 1) as u16
            }
        }
    }

    fn font(&mut self, res: &Dict, name: &str) -> Option<(Rc<Font>, u16)> {
        let fd = self.kind(res, "Font")?;
        match fd.get(name)? {
            Obj::Ref(n, _) => self.font_ref(*n),
            Obj::Dict(d) => {
                self.inline_fonts = self.inline_fonts.checked_sub(1)?;
                let font = Font::load(self.f, d);
                let idx = self.style_index(&font.style);
                Some((Rc::new(font), idx))
            }
            _ => None,
        }
    }

    fn font_obj(&mut self, n: u32, o: &Obj) -> Option<(Rc<Font>, u16)> {
        if let Some((font, _)) = self.fonts.get(&n).cloned() {
            let idx = self.style_index(&font.style);
            return Some((font, idx));
        }
        let d = o.dict()?;
        let font = Rc::new(Font::load(self.f, d));
        let idx = self.style_index(&font.style);
        self.fonts.insert(n, (font.clone(), idx));
        Some((font, idx))
    }

    fn show(&mut self, s: &[u8], gs: &Gs, tm: &mut M) {
        let Some((font, idx)) = gs.font.clone() else { return };
        let hidden = gs.mode == 3 || gs.mode == 7;
        for g in font.glyphs(s) {
            let trm = mul(&[gs.size * gs.th, 0.0, 0.0, gs.size, 0.0, gs.rise], &mul(tm, &gs.ctm));
            let o = apply(&trm, 0.0, 0.0);
            let xs = trm[0].hypot(trm[1]);
            let size = trm[2].hypot(trm[3]) as f32;
            let upright = trm[0] > 0.0 && trm[1].abs() < 0.05 * trm[0].abs() && trm[3] < 0.0;
            let w = (g.width * xs) as f32;
            let n = g.text.len().max(1) as f32;
            for (i, &ch) in g.text.iter().enumerate() {
                if self.page.chars.len() >= 2_000_000 || self.chars == 0 || self.marked_off > 0 {
                    break;
                }
                self.chars -= 1;
                let dx = w / n * i as f32;
                let (x, y) = if upright { (o[0] as f32 + dx, o[1] as f32) } else { (o[0] as f32 + dx * (trm[0] / xs) as f32, o[1] as f32 + dx * (trm[1] / xs) as f32) };
                self.page.chars.push(Char { ch, x, y, w: w / n, size, font: idx, color: gs.fill, upright, hidden });
            }
            let adv = (g.width * gs.size + gs.tc + if g.space { gs.tw } else { 0.0 }) * gs.th;
            *tm = mul(&[1.0, 0.0, 0.0, 1.0, adv, 0.0], tm);
        }
    }

    fn xobject(&mut self, res: &Dict, name: &str, gs: &Gs) {
        let f = self.f;
        let Some(xd) = self.kind(res, "XObject") else { return };
        let id = match xd.get(name) {
            Some(Obj::Ref(n, _)) => Some(*n),
            Some(_) => None,
            None => return,
        };
        let o = self.resource(res, "XObject", name);
        let Obj::Stream(d, raw) = &*o else { return };
        if d.get("OC").is_some_and(|oc| self.oc_off(oc)) {
            return;
        }
        match f.key(&d, "Subtype").name() {
            Some("Image") => {
                let corners = [apply(&gs.ctm, 0.0, 0.0), apply(&gs.ctm, 1.0, 0.0), apply(&gs.ctm, 0.0, 1.0), apply(&gs.ctm, 1.0, 1.0)];
                let x0 = corners.iter().map(|p| p[0]).fold(f64::MAX, f64::min);
                let x1 = corners.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
                let y0 = corners.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
                let y1 = corners.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
                if x1 - x0 < 0.5 || y1 - y0 < 0.5 {
                    return;
                }
                let decoded = match id.and_then(|n| self.images.get(&n)) {
                    Some(v) => v.clone(),
                    None => {
                        let px = f.key_num(&d, "Width").unwrap_or(0.0) * f.key_num(&d, "Height").unwrap_or(0.0);
                        let v = if (px as usize) < self.pixels {
                            self.pixels -= px as usize;
                            image::image(f, &d, &raw, gs.fill).map(|(data, w, h)| (Arc::from(data), (w, h)))
                        } else {
                            None
                        };
                        if let Some(n) = id {
                            self.images.insert(n, v.clone());
                        }
                        v
                    }
                };
                if let Some((data, pixels)) = decoded {
                    self.page.images.push(PageImage { x: x0 as f32, y: y0 as f32, w: (x1 - x0) as f32, h: (y1 - y0) as f32, data, pixels });
                }
            }
            Some("Form") => {
                if self.forms.len() > 12 || id.is_some_and(|n| self.forms.contains(&n)) {
                    return;
                }
                self.forms.push(id.unwrap_or(0));
                let mut g = gs.clone();
                let m = f.nums(d.get("Matrix").unwrap_or(&Obj::Null));
                if m.len() == 6 {
                    g.ctm = mul(&[m[0], m[1], m[2], m[3], m[4], m[5]], &g.ctm);
                }
                // Its own resources, or those of what draws it.
                let own = match id {
                    Some(n) => match self.form_res.get(&n) {
                        Some(r) => r.clone(),
                        None => {
                            let r = self.dict_at(d, "Resources");
                            self.form_res.insert(n, r.clone());
                            r
                        }
                    },
                    None => self.dict_at(d, "Resources"),
                };
                // Forms' content counts against the file's too.
                if self.content == 0 {
                    self.forms.pop();
                    return;
                }
                let data = f.decode(d, raw).data;
                self.content = self.content.saturating_sub(data.len());
                // Inflating and reading a form costs as its size: one
                // drawn over and over mustn't take hours.
                self.ops += data.len() / 64;
                if self.ops > MAX_OPS {
                    self.forms.pop();
                    return;
                }
                let owner = self.owner;
                match &own {
                    Some(r2) => {
                        // A form written inline has no number: it reads
                        // its resources under one of its own.
                        self.owner = id.unwrap_or_else(|| {
                            self.inline = self.inline.saturating_sub(1);
                            self.inline
                        });
                        self.run(&data, r2, g);
                    }
                    None => self.run(&data, res, g),
                }
                self.owner = owner;
                self.forms.pop();
            }
            _ => {}
        }
    }
}

fn gray(v: f64) -> u32 {
    let b = (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    b << 16 | b << 8 | b
}

fn color(space: Space, a: &[Obj]) -> u32 {
    let c: Vec<f64> = a.iter().filter_map(Obj::num).collect();
    let space = match (space, c.len()) {
        // Pattern colours, and unknown spaces by their number of values.
        (Space::Pattern | Space::Indexed, _) => return 0x808080,
        (Space::Gray, 3) => Space::Rgb,
        (Space::Gray, 4) => Space::Cmyk,
        (s, _) => s,
    };
    let [r, g, b] = image::to_rgb(space, &c);
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}
