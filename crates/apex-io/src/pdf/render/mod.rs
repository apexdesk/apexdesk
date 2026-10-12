//! Draws PDF pages into pixels: paths with their strokes, dashes and
//! clips; text with the shapes of the fonts the PDF embeds; pictures;
//! shadings and patterns; transparency, blend modes and soft masks; and
//! the appearance of annotations.

mod color;
mod fields;
mod fonts;
mod images;
mod shade;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Rect, Shape, Stroke};
use vello_cpu::peniko::{BlendMode, Compose, Extend, Fill, ImageQuality, ImageSampler, Mix};
use vello_cpu::{Image, ImageSource, Mask, PaintType, Pixmap, RenderContext, Resources};

pub use fonts::Fallback;

use super::file::{PageRef, PdfFile};
use super::font::Font;
use super::obj::{Dict, Item, Lexer, Obj};
use color::Cs;
use fonts::Glyphs;
use shade::Shading;

/// A page drawn: its size in pixels and RGBA pixels (opaque).
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const MAX_OPS: usize = 6_000_000;
const PIXEL_BUDGET: usize = 300_000_000;
/// Most dashes one stroke, and all of a page's, are cut into: past that a
/// line is drawn whole (dashes of a millionth along a long line would
/// take all the memory there is).
const DASH_STROKE: f64 = 200_000.0;
const DASH_BUDGET: usize = 4_000_000;
/// Path elements the page's glyphs may draw: a small file repeating a
/// glyph of a million points would otherwise take hours (or, clipping
/// with text, all the memory there is).
const GLYPH_BUDGET: usize = 20_000_000;
/// Clips and layers a page may hold at once.
const MAX_LAYERS: usize = 1024;
/// What a page's streams may inflate to, all told: colour spaces,
/// functions and inline images are read again each time they're named.
const DECODE_BUDGET: usize = (1usize << 30).saturating_mul(4);
/// Function (PostScript calculator) steps a page may run.
const FUNC_BUDGET: usize = 1 << 28;

/// Draws page `page` of a PDF `width` pixels wide. `fallback` gives the
/// font of a system for fonts the PDF does not embed.
pub fn render(data: &[u8], password: Option<&str>, page: usize, width: u32, fallback: Fallback) -> Option<Rendered> {
    let f = PdfFile::open(data, password).ok()?;
    let pages = f.pages();
    let p = pages.get(page)?;
    let mut shared = Shared::default();
    render_page(&f, p, width, fallback, &ocg_off(&f), &mut shared)
}

/// What the pages of a document share, kept from one page to the next.
#[derive(Default)]
struct Shared {
    fonts: HashMap<u32, Rc<FontEntry>>,
    images: HashMap<(u32, u32), Option<Arc<Pixmap>>>,
    /// The width the images were shrunk for, and the bytes they take.
    images_width: u32,
    images_bytes: usize,
}

/// Bytes of shrunk images kept between pages.
const IMAGE_CACHE: usize = 256 << 20;

/// A PDF opened once to draw several of its pages: the file's objects,
/// its pages, and the fonts and images they share stay loaded.
pub struct Document {
    // Before `_data`: dropped first, as it borrows those bytes.
    file: PdfFile<'static>,
    pages: Vec<PageRef>,
    ocg_off: HashSet<u32>,
    shared: Shared,
    _data: Arc<[u8]>,
}

impl Document {
    pub fn open(data: Arc<[u8]>, password: Option<&str>) -> Option<Document> {
        // SAFETY: the bytes live in `data`'s allocation, which never moves
        // and is kept in the document, dropped after the file borrowing it.
        let bytes: &'static [u8] = unsafe { &*(&*data as *const [u8]) };
        let file = PdfFile::open(bytes, password).ok()?;
        let pages = file.pages();
        let ocg_off = ocg_off(&file);
        Some(Document { file, pages, ocg_off, shared: Shared::default(), _data: data })
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Draws page `page` `width` pixels wide (see [`render`]).
    pub fn render(&mut self, page: usize, width: u32, fallback: Fallback) -> Option<Rendered> {
        let p = self.pages.get(page)?;
        render_page(&self.file, p, width, fallback, &self.ocg_off, &mut self.shared)
    }
}

/// The size of each page in points as shown (rotation applied).
pub fn page_sizes(data: &[u8], password: Option<&str>) -> Option<Vec<(f64, f64)>> {
    let f = PdfFile::open(data, password).ok()?;
    Some(f.pages().iter().map(|p| {
        let (_, w, h) = page_matrix(p);
        (w, h)
    }).collect())
}

/// The matrix from user space to the page as shown (points, top-left
/// origin) and the page's size.
fn page_matrix(p: &PageRef) -> (Affine, f64, f64) {
    let [x0, y0, x1, y1] = p.bbox;
    let (w, h) = ((x1 - x0).abs().max(1.0), (y1 - y0).abs().max(1.0));
    let (x0, y0, x1, y1) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
    match p.rotate.rem_euclid(360) {
        90 => (Affine::new([0.0, 1.0, 1.0, 0.0, -y0, -x0]), h, w),
        180 => (Affine::new([-1.0, 0.0, 0.0, 1.0, x1, -y0]), w, h),
        270 => (Affine::new([0.0, -1.0, -1.0, 0.0, y1, x1]), h, w),
        _ => (Affine::new([1.0, 0.0, 0.0, -1.0, -x0, y1]), w, h),
    }
}

/// The optional content groups the document's default configuration has
/// off.
pub(crate) fn ocg_off(f: &PdfFile) -> HashSet<u32> {
    f.catalog().map(|cat| ocg_off_in(&cat, &|o| f.resolve(o))).unwrap_or_default()
}

/// The optional content groups the default configuration of the catalog
/// `cat` has off once the document is opened: its states, then those its
/// groups ask for when viewed (as Acrobat applies them).
pub(crate) fn ocg_off_in(cat: &Dict, resolve: &dyn Fn(&Obj) -> Obj) -> HashSet<u32> {
    let mut off = HashSet::new();
    let key = |d: &Dict, k: &str| d.get(k).map(resolve).unwrap_or(Obj::Null);
    let Some(ocp) = key(cat, "OCProperties").dict().cloned() else { return off };
    let Some(d) = key(&ocp, "D").dict().cloned() else { return off };
    let refs = |o: Obj| -> Vec<u32> {
        match o {
            Obj::Array(a) => a.iter().take(10_000).filter_map(|g| if let Obj::Ref(n, _) = g { Some(*n) } else { None }).collect(),
            _ => Vec::new(),
        }
    };
    if key(&d, "BaseState").name() == Some("OFF") {
        let on: HashSet<u32> = refs(key(&d, "ON")).into_iter().collect();
        off.extend(refs(key(&ocp, "OCGs")).into_iter().filter(|n| !on.contains(n)));
    }
    off.extend(refs(key(&d, "OFF")));
    if let Obj::Array(auto) = key(&d, "AS") {
        for a in auto.iter().take(100) {
            let Some(a) = resolve(a).dict().cloned() else { continue };
            let viewed = matches!(key(&a, "Category"), Obj::Array(c) if c.iter().any(|c| c.name() == Some("View")));
            if key(&a, "Event").name() != Some("View") || !viewed {
                continue;
            }
            for n in refs(key(&a, "OCGs")) {
                let group = resolve(&Obj::Ref(n, 0));
                let state = group.dict().and_then(|g| key(g, "Usage").dict().cloned()).and_then(|u| key(&u, "View").dict().cloned()).and_then(|v| key(&v, "ViewState").name().map(str::to_string));
                match state.as_deref() {
                    Some("OFF") => {
                        off.insert(n);
                    }
                    Some("ON") => {
                        off.remove(&n);
                    }
                    _ => {}
                }
            }
        }
    }
    off
}

/// Whether the optional content `o` (a group or a membership dictionary)
/// is off when the groups `off` are.
pub(crate) fn oc_hidden(o: &Obj, off: &HashSet<u32>, resolve: &dyn Fn(&Obj) -> Obj) -> bool {
    if off.is_empty() {
        return false;
    }
    if let Obj::Ref(n, _) = o
        && off.contains(n)
    {
        return true;
    }
    let Some(d) = resolve(o).dict().cloned() else { return false };
    if d.get("Type").map(resolve).and_then(|t| t.name().map(str::to_string)).as_deref() != Some("OCMD") {
        return false;
    }
    let groups: Vec<bool> = match d.get("OCGs") {
        Some(Obj::Ref(n, _)) => vec![off.contains(n)],
        Some(Obj::Array(a)) => a.iter().filter_map(|g| if let Obj::Ref(n, _) = g { Some(off.contains(n)) } else { None }).collect(),
        _ => return false,
    };
    if groups.is_empty() {
        return false;
    }
    match d.get("P").map(resolve).and_then(|p| p.name().map(str::to_string)).as_deref() {
        Some("AllOn") => groups.iter().any(|&off| off),
        Some("AnyOff") => !groups.iter().any(|&off| off),
        Some("AllOff") => !groups.iter().all(|&off| off),
        _ => groups.iter().all(|&off| off),
    }
}

fn render_page(f: &PdfFile, p: &PageRef, width: u32, fallback: Fallback, off: &HashSet<u32>, shared: &mut Shared) -> Option<Rendered> {
    let (m0, pw, ph) = page_matrix(p);
    let width = width.clamp(1, 16_000);
    let scale = width as f64 / pw;
    let height = ((ph * scale).round() as u32).clamp(1, 16_000);
    // The page onto the whole pixels, as pdfium maps it.
    let base = Affine::scale_non_uniform(scale, height as f64 / ph) * m0;
    let mut r = R::new(f, width as u16, height as u16, fallback);
    r.ocg_off = off.clone();
    // Images were shrunk for the width drawn: kept while it stays.
    if shared.images_width != width || shared.images_bytes > IMAGE_CACHE {
        shared.images.clear();
        shared.images_width = width;
        shared.images_bytes = 0;
    }
    r.fonts = std::mem::take(&mut shared.fonts);
    r.images = std::mem::take(&mut shared.images);
    r.page_base = base;
    // The paper (blend modes blend with it, as in pdfium).
    r.ctx.set_paint(AlphaColor::<Srgb>::new([1.0, 1.0, 1.0, 1.0]));
    r.ctx.fill_rect(&Rect::new(0.0, 0.0, width as f64, height as f64));
    let gs = Gs::new(base, Rect::new(0.0, 0.0, width as f64, height as f64));
    let content = f.contents(&p.dict);
    r.run(&content, &p.resources, gs.clone());
    r.annotations(&p.dict, &gs);
    r.pop_to(0);
    let mut pix = Pixmap::new(width as u16, height as u16);
    r.ctx.flush();
    r.ctx.render(&mut pix, &mut r.resources);
    shared.fonts = std::mem::take(&mut r.fonts);
    shared.images = std::mem::take(&mut r.images);
    shared.images_bytes = shared.images.values().flatten().map(|p| p.width() as usize * p.height() as usize * 4).sum();
    // The paper is opaque: the pixels are as they are, not premultiplied.
    let rgba = pix.data_as_u8_slice().to_vec();
    Some(Rendered { width, height, rgba })
}

/// A fill or stroke colour.
#[derive(Clone)]
enum Ink {
    Color([f32; 3]),
    /// The "None" separation: paints nothing.
    None,
    /// A pattern (and the colour of an uncoloured one).
    Pattern(Obj, Option<[f32; 3]>),
}

#[derive(Clone)]
struct Gs {
    ctm: Affine,
    /// The pattern space of the stream being run.
    base: Affine,
    /// The bounds (device pixels) of the clip.
    clip: Rect,
    fill: Ink,
    stroke: Ink,
    fill_cs: Rc<Cs>,
    stroke_cs: Rc<Cs>,
    fill_alpha: f32,
    stroke_alpha: f32,
    blend: BlendMode,
    smask: Option<Mask>,
    line_width: f64,
    cap: Cap,
    join: Join,
    miter: f64,
    dash: (Vec<f64>, f64),
    font: Option<Rc<FontEntry>>,
    size: f64,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    rise: f64,
    mode: i64,
    /// Inside a Type 3 glyph that takes the colour of the text.
    uncolored: bool,
    /// Inside a `d1` glyph drawn without a soft mask: what it draws under
    /// one of its own is in its own colour, black (pdfium renders it
    /// apart, without the text's colour).
    t3_bare: bool,
}

impl Gs {
    fn new(base: Affine, clip: Rect) -> Gs {
        Gs {
            ctm: base,
            base,
            clip,
            fill: Ink::Color([0.0; 3]),
            stroke: Ink::Color([0.0; 3]),
            fill_cs: Rc::new(Cs::Gray),
            stroke_cs: Rc::new(Cs::Gray),
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend: BlendMode::default(),
            smask: None,
            line_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter: 10.0,
            dash: (Vec::new(), 0.0),
            font: None,
            size: 0.0,
            tc: 0.0,
            tw: 0.0,
            th: 1.0,
            tl: 0.0,
            rise: 0.0,
            mode: 0,
            uncolored: false,
            t3_bare: false,
        }
    }
}

struct FontEntry {
    font: Font,
    glyphs: Glyphs,
    type3: Option<Type3>,
}

struct Type3 {
    procs: Dict,
    matrix: Affine,
    res: Option<Dict>,
    /// Glyph names by code.
    names: HashMap<u32, String>,
}

struct R<'a, 'f> {
    f: &'a PdfFile<'f>,
    ctx: RenderContext,
    resources: Resources,
    /// Layers (false) and clip paths (true) pushed, not yet popped.
    layers: Vec<bool>,
    width: u16,
    height: u16,
    fonts: HashMap<u32, Rc<FontEntry>>,
    images: HashMap<(u32, u32), Option<Arc<Pixmap>>>,
    /// Forms, patterns and glyphs being run (against loops).
    nesting: Vec<u32>,
    ops: usize,
    pixels: usize,
    /// Dashes the page's strokes may still be cut into.
    dashes: usize,
    /// Glyph path elements the page may still draw.
    glyph_elems: usize,
    /// The file's limit on inflating before this page (given back after).
    decode_before: Option<usize>,
    /// The function steps left before this page (given back after).
    func_before: usize,
    fallback: Fallback<'a>,
    /// Optional content groups that are off.
    ocg_off: HashSet<u32>,
    /// Marked content entered: whether each hides what it holds.
    marked: Vec<bool>,
    /// How many of those hide.
    marked_off: usize,
    /// Drawing a glyph (not snapped to pixels like other rectangles).
    glyph: bool,
    /// The page's own base: patterns inside a tiling cell are placed
    /// from it, not from the form the cell was painted in (as pdfium).
    page_base: Affine,
    /// Tiling cells being drawn.
    cells: u32,
    /// Objects used again and again, loaded once: shadings, colour
    /// spaces, the decoded bytes of forms, cells and Type 3 glyphs, and
    /// soft masks (by their group and where they are drawn).
    shadings: HashMap<u32, Option<Rc<Shading>>>,
    spaces: HashMap<u32, Rc<Cs>>,
    /// Spaces written in a resource dictionary (not as objects), by the
    /// dictionary and name, and how many were read: each read can inflate
    /// a lookup table or profile.
    inline_spaces: HashMap<(usize, Arc<str>), Rc<Cs>>,
    inline_loads: usize,
    streams: HashMap<u32, Rc<[u8]>>,
    stream_bytes: usize,
    masks: HashMap<(u32, bool, Vec<u64>), Option<Mask>>,
}

/// Decoded bytes kept for the page.
const STREAM_CACHE: usize = 64 << 20;

/// Threads to draw a page with besides this one: up to 4, leaving one
/// of the processor's for the rest of the program.
fn threads() -> u16 {
    std::thread::available_parallelism().map_or(0, |n| n.get().saturating_sub(2).min(4) as u16)
}

/// Rendering with `threads` helper threads (0: all in this one, as for
/// the small pictures drawn offscreen).
fn settings(threads: u16) -> vello_cpu::RenderSettings {
    vello_cpu::RenderSettings { num_threads: threads, ..Default::default() }
}

/// `path` in straight pieces within `tolerance`.
fn kurbo_flatten(path: &BezPath, tolerance: f64, out: &mut BezPath) {
    vello_cpu::kurbo::flatten(path.iter(), tolerance, |el| out.push(el));
}

/// PDF matrix operands in kurbo's order.
fn pdf_m(m: &[f64]) -> Affine {
    if m.len() >= 6 { Affine::new([m[0], m[1], m[2], m[3], m[4], m[5]]) } else { Affine::IDENTITY }
}

/// A path that is one upright rectangle on the device: that rectangle.
fn upright_rect(path: &BezPath, m: Affine) -> Option<Rect> {
    let els = path.elements();
    let mut pts: Vec<Point> = Vec::with_capacity(5);
    for (i, el) in els.iter().enumerate() {
        match el {
            PathEl::MoveTo(p) if i == 0 => pts.push(m * *p),
            PathEl::LineTo(p) if i > 0 => pts.push(m * *p),
            PathEl::ClosePath if i == els.len() - 1 => {}
            _ => return None,
        }
    }
    if pts.len() == 5 && (pts[4] - pts[0]).hypot() < 1e-3 {
        pts.pop();
    }
    if pts.len() != 4 {
        return None;
    }
    let level = |a: Point, b: Point| (a.y - b.y).abs() < 1e-3;
    let plumb = |a: Point, b: Point| (a.x - b.x).abs() < 1e-3;
    let ok = (level(pts[0], pts[1]) && plumb(pts[1], pts[2]) && level(pts[2], pts[3]) && plumb(pts[3], pts[0]))
        || (plumb(pts[0], pts[1]) && level(pts[1], pts[2]) && plumb(pts[2], pts[3]) && level(pts[3], pts[0]));
    if !ok {
        return None;
    }
    let r = Rect::from_points(pts[0], pts[2]);
    if !(r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite()) || r.width() > 1e6 || r.height() > 1e6 {
        return None;
    }
    Some(r)
}

/// A path that is one upright rectangle on the device, on the whole pixels
/// pdfium fills for it (without antialiasing; at least a pixel wide, and
/// no wider than its width rounded up).
fn pixel_rect(path: &BezPath, m: Affine) -> Option<Rect> {
    let r = upright_rect(path, m)?;
    let snap = |lo: f64, hi: f64| {
        let (mut a, mut b) = (lo.floor(), hi.ceil());
        let mut size = (hi - lo).ceil();
        if size < 1.0 {
            size = 1.0;
            if a == b {
                b += 1.0;
            }
        }
        if b - a >= size + 1.0 {
            if lo - a > b - hi {
                a += 1.0;
            } else {
                b -= 1.0;
            }
        }
        (a, b)
    };
    let (x0, x1) = snap(r.x0, r.x1);
    let (y0, y1) = snap(r.y0, r.y1);
    Some(Rect::new(x0, y0, x1, y1))
}

/// The path with closed subpaths of fewer than three distinct points left
/// open, as pdfium (agg) strokes them: once, with caps, not out and back.
fn open_short_subpaths(path: &BezPath) -> Option<BezPath> {
    let els = path.elements();
    if !els.iter().any(|e| matches!(e, PathEl::ClosePath)) {
        return None;
    }
    let mut out = BezPath::new();
    let mut pts: Vec<Point> = Vec::new();
    let mut lines = true;
    let mut changed = false;
    for el in els {
        match el {
            PathEl::MoveTo(p) => {
                pts.clear();
                pts.push(*p);
                lines = true;
            }
            PathEl::LineTo(p) => {
                if pts.last() != Some(p) {
                    pts.push(*p);
                }
            }
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => lines = false,
            PathEl::ClosePath => {
                if pts.len() > 1 && pts.first() == pts.last() {
                    pts.pop();
                }
                if lines && pts.len() < 3 {
                    changed = true;
                    continue;
                }
            }
        }
        out.push(*el);
    }
    changed.then_some(out)
}

fn scale_of(m: &Affine) -> f64 {
    let c = m.as_coeffs();
    (c[0] * c[3] - c[1] * c[2]).abs().sqrt()
}

impl Drop for R<'_, '_> {
    fn drop(&mut self) {
        self.f.set_decode_budget(self.decode_before);
        color::set_func_budget(self.func_before);
    }
}

impl<'a, 'f> R<'a, 'f> {
    fn new(f: &'a PdfFile<'f>, width: u16, height: u16, fallback: Fallback<'a>) -> Self {
        let decode_before = f.set_decode_budget(Some(DECODE_BUDGET));
        let func_before = color::set_func_budget(FUNC_BUDGET);
        R { decode_before, func_before, marked_off: 0, f, ctx: RenderContext::new_with(width, height, settings(threads())), resources: Resources::new(), layers: Vec::new(), width, height, fonts: HashMap::new(), images: HashMap::new(), nesting: Vec::new(), ops: 0, pixels: PIXEL_BUDGET, dashes: DASH_BUDGET, glyph_elems: GLYPH_BUDGET, fallback, ocg_off: HashSet::new(), marked: Vec::new(), glyph: false, page_base: Affine::IDENTITY, cells: 0, shadings: HashMap::new(), spaces: HashMap::new(), inline_spaces: HashMap::new(), inline_loads: 0, streams: HashMap::new(), stream_bytes: 0, masks: HashMap::new() }
    }

    /// The resource dictionary of `kind` in `res`. Copying it costs as
    /// its size: operators naming a huge one over and over would take hours.
    fn res_dict(&mut self, res: &Dict, kind: &str) -> Option<Dict> {
        let d = self.f.key_dict(res, kind)?;
        self.ops = self.ops.saturating_add(d.0.iter().fold(0usize, |w, (_, o)| w.saturating_add(o.weight())) / 16);
        Some(d)
    }

    fn resource(&mut self, res: &Dict, kind: &str, name: &str) -> Obj {
        match self.res_dict(res, kind) {
            Some(d) => self.f.key(&d, name),
            None => Obj::Null,
        }
    }

    /// A stream's decoded bytes, kept by its object number.
    fn decoded(&mut self, id: Option<u32>, d: &Dict, raw: &[u8]) -> Rc<[u8]> {
        if let Some(b) = id.and_then(|n| self.streams.get(&n)).cloned() {
            // Reading it again costs as its size too.
            self.ops += b.len() / 64;
            return b;
        }
        let b: Rc<[u8]> = Rc::from(self.f.decode(d, raw).data);
        // Decoding and reading cost as the size (a stream too big to keep
        // is decoded again each time it's drawn).
        self.ops += b.len() / 64;
        if let Some(n) = id
            && self.stream_bytes + b.len() <= STREAM_CACHE
        {
            self.stream_bytes += b.len();
            self.streams.insert(n, b.clone());
        }
        b
    }

    /// A shading (`o` as the resources give it), kept by its object number.
    fn shading(&mut self, o: &Obj, res: Option<&Dict>) -> Option<Rc<Shading>> {
        let id = if let Obj::Ref(n, _) = o { Some(*n) } else { None };
        if let Some(s) = id.and_then(|n| self.shadings.get(&n)) {
            return s.clone();
        }
        let s = Shading::load(self.f, o, res).map(Rc::new);
        if let Some(n) = id {
            self.shadings.insert(n, s.clone());
        }
        s
    }

    /// The colour space `o` names in `res`, kept by its object number.
    fn space(&mut self, o: &Obj, res: &Dict) -> Rc<Cs> {
        let id = match o {
            Obj::Name(n) if !matches!(&**n, "DeviceGray" | "G" | "DeviceRGB" | "RGB" | "DeviceCMYK" | "CMYK" | "Pattern") => {
                match self.res_dict(res, "ColorSpace").and_then(|d| d.get(&**n).cloned()) {
                    Some(Obj::Ref(r, _)) => Some(r),
                    _ => None,
                }
            }
            Obj::Ref(r, _) => Some(*r),
            _ => None,
        };
        if let Some(c) = id.and_then(|n| self.spaces.get(&n)) {
            return c.clone();
        }
        let inline = match o {
            Obj::Name(n) if id.is_none() && !matches!(&**n, "DeviceGray" | "G" | "DeviceRGB" | "RGB" | "DeviceCMYK" | "CMYK" | "Pattern") => Some((res as *const Dict as usize, n.clone())),
            _ => None,
        };
        if let Some(k) = &inline {
            if let Some(c) = self.inline_spaces.get(k) {
                return c.clone();
            }
            self.inline_loads += 1;
            if self.inline_loads > 64 {
                return Rc::new(Cs::load(self.f, &Obj::Name("DeviceRGB".into()), None));
            }
        }
        let c = Rc::new(Cs::load(self.f, o, Some(res)));
        if let Some(n) = id {
            self.spaces.insert(n, c.clone());
        }
        if let Some(k) = inline {
            self.inline_spaces.insert(k, c.clone());
        }
        c
    }

    fn pop_to(&mut self, depth: usize) {
        while self.layers.len() > depth {
            match self.layers.pop() {
                Some(true) => self.ctx.pop_clip_path(),
                _ => self.ctx.pop_layer(),
            }
        }
    }

    /// Settings shared by every drawing: blend mode and soft mask.
    fn prepare(&mut self, gs: &Gs) {
        self.ctx.set_blend_mode(gs.blend);
        match &gs.smask {
            Some(m) if m.width() == self.width && m.height() == self.height => self.ctx.set_mask(m.clone()),
            _ => self.ctx.reset_mask(),
        }
        self.ctx.reset_paint_transform();
    }

    fn run(&mut self, content: &[u8], res: &Dict, mut gs: Gs) {
        let f = self.f;
        let start_depth = self.layers.len();
        let start_marked = self.marked.len();
        let mut lx = Lexer::new(content, 0);
        let mut args: Vec<Obj> = Vec::new();
        let mut stack: Vec<(Gs, usize)> = Vec::new();
        let mut path = BezPath::new();
        let mut cur: Option<Point> = None;
        let mut start: Option<Point> = None;
        let mut pending_clip: Option<Fill> = None;
        let mut tm = Affine::IDENTITY;
        let mut tlm = Affine::IDENTITY;
        let mut text_clip: Option<BezPath> = None;
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
                break;
            }
            let a = &args;
            match op {
                b"q" => {
                    if stack.len() < 512 {
                        stack.push((gs.clone(), self.layers.len()));
                    }
                }
                b"Q" => {
                    if let Some((g, depth)) = stack.pop() {
                        self.pop_to(depth);
                        gs = g;
                    }
                }
                b"cm" if a.len() >= 6 => {
                    let m = [num(a, 0), num(a, 1), num(a, 2), num(a, 3), num(a, 4), num(a, 5)];
                    gs.ctm = gs.ctm * pdf_m(&m);
                }
                b"w" => gs.line_width = num(a, 0).abs(),
                b"J" => {
                    gs.cap = match num(a, 0) as i64 {
                        1 => Cap::Round,
                        2 => Cap::Square,
                        _ => Cap::Butt,
                    }
                }
                b"j" => {
                    gs.join = match num(a, 0) as i64 {
                        1 => Join::Round,
                        2 => Join::Bevel,
                        _ => Join::Miter,
                    }
                }
                b"M" => gs.miter = num(a, 0).max(1.0),
                b"d" => {
                    let pat = a.first().and_then(Obj::array).map(|v| v.iter().filter_map(Obj::num).map(f64::abs).collect()).unwrap_or_default();
                    gs.dash = (pat, num(a, 1));
                }
                b"ri" | b"i" => {}
                b"gs" => {
                    if let Some(Obj::Name(n)) = a.first() {
                        let d = self.resource(res, "ExtGState", n);
                        if let Some(d) = f.resolve(&d).dict() {
                            self.ext_gstate(d, &mut gs);
                        }
                    }
                }
                // Colours.
                b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" => {
                    if gs.uncolored {
                        args.clear();
                        continue;
                    }
                    let cs = match op {
                        b"g" | b"G" => Cs::Gray,
                        b"rg" | b"RG" => Cs::Rgb,
                        _ => Cs::Cmyk,
                    };
                    let c: Vec<f32> = a.iter().filter_map(Obj::num).map(|v| v as f32).collect();
                    let ink = Ink::Color(cs.rgb(&c));
                    if op[0].is_ascii_lowercase() {
                        gs.fill = ink;
                        gs.fill_cs = Rc::new(cs);
                    } else {
                        gs.stroke = ink;
                        gs.stroke_cs = Rc::new(cs);
                    }
                }
                b"cs" | b"CS" => {
                    if gs.uncolored {
                        args.clear();
                        continue;
                    }
                    let cs = self.space(a.first().unwrap_or(&Obj::Null), res);
                    let ink = match &*cs {
                        Cs::Pattern(_) => Ink::Pattern(Obj::Null, None),
                        c if c.invisible() => Ink::None,
                        c => Ink::Color(c.rgb(&c.initial())),
                    };
                    if op == b"cs" {
                        (gs.fill_cs, gs.fill) = (cs, ink);
                    } else {
                        (gs.stroke_cs, gs.stroke) = (cs, ink);
                    }
                }
                b"sc" | b"scn" | b"SC" | b"SCN" => {
                    if gs.uncolored {
                        args.clear();
                        continue;
                    }
                    let fill = op[0] == b's';
                    let cs = if fill { gs.fill_cs.clone() } else { gs.stroke_cs.clone() };
                    let c: Vec<f32> = a.iter().filter_map(Obj::num).map(|v| v as f32).collect();
                    let ink = match (&*cs, a.last()) {
                        (Cs::Pattern(under), Some(Obj::Name(n))) => {
                            let pat = self.resource(res, "Pattern", n);
                            Ink::Pattern(pat, under.as_ref().map(|u| u.rgb(&c)))
                        }
                        // A pattern named in another space: pdfium paints with
                        // it all the same (and leaves the ink when there is none).
                        (_, Some(Obj::Name(n))) if op.len() == 3 => match self.resource(res, "Pattern", n) {
                            Obj::Null => {
                                args.clear();
                                continue;
                            }
                            pat => Ink::Pattern(pat, None),
                        },
                        (Cs::Pattern(_), _) => Ink::Pattern(Obj::Null, None),
                        (cs, _) if cs.invisible() => Ink::None,
                        // An index truncated, as pdfium takes it; out of
                        // the table, nothing is painted.
                        (Cs::Indexed { hival, .. }, _) => match c.first().map(|v| v.trunc()) {
                            Some(i) if i >= 0.0 && i <= *hival as f32 => Ink::Color(cs.rgb(&[i])),
                            _ => Ink::None,
                        },
                        (cs, _) => Ink::Color(cs.rgb(&c)),
                    };
                    if fill {
                        gs.fill = ink;
                    } else {
                        gs.stroke = ink;
                    }
                }
                // Paths.
                b"m" => {
                    let p = Point::new(num(a, 0), num(a, 1));
                    path.move_to(p);
                    cur = Some(p);
                    start = Some(p);
                }
                b"l" => {
                    let p = Point::new(num(a, 0), num(a, 1));
                    if cur.is_none() {
                        path.move_to(p);
                        start = Some(p);
                    } else {
                        path.line_to(p);
                    }
                    cur = Some(p);
                }
                b"c" | b"v" | b"y" => {
                    let Some(p0) = cur else {
                        args.clear();
                        continue;
                    };
                    let pt = |i: usize| Point::new(num(a, i), num(a, i + 1));
                    let (p1, p2, p3) = match op {
                        b"c" => (pt(0), pt(2), pt(4)),
                        b"v" => (p0, pt(0), pt(2)),
                        _ => (pt(0), pt(2), pt(2)),
                    };
                    path.curve_to(p1, p2, p3);
                    cur = Some(p3);
                }
                b"h" => {
                    if cur.is_some() {
                        path.close_path();
                        cur = start;
                    }
                }
                b"re" => {
                    let (x, y, w, h) = (num(a, 0), num(a, 1), num(a, 2), num(a, 3));
                    path.move_to((x, y));
                    path.line_to((x + w, y));
                    path.line_to((x + w, y + h));
                    path.line_to((x, y + h));
                    path.close_path();
                    cur = Some(Point::new(x, y));
                    start = cur;
                }
                b"W" => pending_clip = Some(Fill::NonZero),
                b"W*" => pending_clip = Some(Fill::EvenOdd),
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    let close = matches!(op, b"s" | b"b" | b"b*");
                    if close && cur.is_some() {
                        path.close_path();
                    }
                    let even = matches!(op, b"f*" | b"B*" | b"b*");
                    let rule = if even { Fill::EvenOdd } else { Fill::NonZero };
                    let fill = matches!(op, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
                    let stroke = matches!(op, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                    if fill {
                        self.fill_path(&path, rule, &gs);
                    }
                    if stroke {
                        self.stroke_path(&path, &gs);
                    }
                    if let Some(rule) = pending_clip.take() {
                        self.clip(&path, rule, &mut gs);
                    }
                    path = BezPath::new();
                    cur = None;
                    start = None;
                }
                // Text.
                b"BT" => {
                    tm = Affine::IDENTITY;
                    tlm = tm;
                }
                b"ET" => {
                    if let Some(p) = text_clip.take() {
                        let ctm = gs.ctm;
                        gs.ctm = Affine::IDENTITY;
                        self.clip(&p, Fill::NonZero, &mut gs);
                        gs.ctm = ctm;
                    }
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
                    tlm = tlm * Affine::translate((num(a, 0), num(a, 1)));
                    tm = tlm;
                }
                b"Tm" if a.len() >= 6 => {
                    tlm = pdf_m(&[num(a, 0), num(a, 1), num(a, 2), num(a, 3), num(a, 4), num(a, 5)]);
                    tm = tlm;
                }
                b"T*" => {
                    tlm = tlm * Affine::translate((0.0, -gs.tl));
                    tm = tlm;
                }
                b"Tj" | b"'" | b"\"" | b"TJ" => {
                    if op == b"\"" {
                        gs.tw = num(a, 0);
                        gs.tc = num(a, 1);
                    }
                    if op == b"'" || op == b"\"" {
                        tlm = tlm * Affine::translate((0.0, -gs.tl));
                        tm = tlm;
                    }
                    if gs.mode >= 4 && text_clip.is_none() {
                        text_clip = Some(BezPath::new());
                    }
                    match (op, a.last()) {
                        (b"TJ", Some(Obj::Array(items))) => {
                            for it in items {
                                match it {
                                    Obj::Str(s) => self.show(s, &gs, &mut tm, &mut text_clip),
                                    o => {
                                        let d = -o.num().unwrap_or(0.0) / 1000.0 * gs.size;
                                        let vertical = gs.font.as_ref().is_some_and(|f| f.font.is_vertical());
                                        tm = tm * Affine::translate(if vertical { (0.0, d) } else { (d * gs.th, 0.0) });
                                    }
                                }
                            }
                        }
                        (_, Some(Obj::Str(s))) => self.show(s, &gs, &mut tm, &mut text_clip),
                        _ => {}
                    }
                }
                b"d0" | b"d1" => {}
                b"sh" => {
                    if let Some(Obj::Name(n)) = a.first() {
                        let o = self.res_dict(res, "Shading").and_then(|d| d.get(&**n).cloned()).unwrap_or(Obj::Null);
                        if let Some(sh) = self.shading(&o, Some(res)) {
                            self.shade(&sh, None, gs.ctm, &gs);
                        }
                    }
                }
                b"Do" => {
                    if let Some(Obj::Name(n)) = a.first() {
                        // The reference kept: images are cached by it.
                        let o = self.res_dict(res, "XObject").and_then(|d| d.get(&**n).cloned()).unwrap_or(Obj::Null);
                        self.xobject(&o, res, &gs);
                    }
                }
                b"BI" => self.inline_image(&mut lx, res, &gs),
                // Optional content.
                b"BMC" => self.marked.push(false),
                b"BDC" => {
                    let off = match (a.first(), a.get(1)) {
                        (Some(Obj::Name(t)), Some(Obj::Name(n))) if &**t == "OC" => {
                            let o = self.res_dict(res, "Properties").and_then(|d| d.get(n).cloned());
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
                _ => {}
            }
            args.clear();
        }
        self.pop_to(start_depth);
        if let Some(left) = self.marked.get(start_marked..) {
            self.marked_off -= left.iter().filter(|&&b| b).count();
        }
        self.marked.truncate(start_marked);
    }

    fn ext_gstate(&mut self, d: &Dict, gs: &mut Gs) {
        let f = self.f;
        for (k, v) in &d.0 {
            let v = f.resolve(v);
            match &**k {
                "LW" => gs.line_width = v.num().unwrap_or(1.0).abs(),
                "LC" => {
                    gs.cap = match v.int().unwrap_or(0) {
                        1 => Cap::Round,
                        2 => Cap::Square,
                        _ => Cap::Butt,
                    }
                }
                "LJ" => {
                    gs.join = match v.int().unwrap_or(0) {
                        1 => Join::Round,
                        2 => Join::Bevel,
                        _ => Join::Miter,
                    }
                }
                "ML" => gs.miter = v.num().unwrap_or(10.0).max(1.0),
                "D" => {
                    if let Obj::Array(a) = &v {
                        let pat = a.first().map(|o| f.nums(o)).unwrap_or_default().into_iter().map(f64::abs).collect();
                        gs.dash = (pat, a.get(1).and_then(Obj::num).unwrap_or(0.0));
                    }
                }
                "CA" => gs.stroke_alpha = v.num().unwrap_or(1.0).clamp(0.0, 1.0) as f32,
                "ca" => gs.fill_alpha = v.num().unwrap_or(1.0).clamp(0.0, 1.0) as f32,
                "BM" => {
                    let name = match &v {
                        Obj::Array(a) => a.first().and_then(|o| f.resolve(o).name().map(str::to_string)),
                        o => o.name().map(str::to_string),
                    };
                    let mix = match name.as_deref() {
                        Some("Multiply") => Mix::Multiply,
                        Some("Screen") => Mix::Screen,
                        Some("Overlay") => Mix::Overlay,
                        Some("Darken") => Mix::Darken,
                        Some("Lighten") => Mix::Lighten,
                        Some("ColorDodge") => Mix::ColorDodge,
                        Some("ColorBurn") => Mix::ColorBurn,
                        Some("HardLight") => Mix::HardLight,
                        Some("SoftLight") => Mix::SoftLight,
                        Some("Difference") => Mix::Difference,
                        Some("Exclusion") => Mix::Exclusion,
                        Some("Hue") => Mix::Hue,
                        Some("Saturation") => Mix::Saturation,
                        Some("Color") => Mix::Color,
                        Some("Luminosity") => Mix::Luminosity,
                        _ => Mix::Normal,
                    };
                    gs.blend = BlendMode::new(mix, Compose::SrcOver);
                }
                "SMask" => {
                    gs.smask = match &v {
                        Obj::Dict(sm) => self.soft_mask(sm, gs),
                        _ => None,
                    };
                }
                // pdfium takes only the size, and the text falls to its
                // default font.
                "Font" => {
                    if let Obj::Array(fa) = &v
                        && let Some(sz) = fa.get(1).and_then(|o| f.resolve(o).num())
                    {
                        gs.size = sz;
                        gs.font = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// The colour of a solid ink with the alpha.
    fn solid(c: [f32; 3], alpha: f32) -> AlphaColor<Srgb> {
        // pdfium's opacity is a whole number out of 255, cut short.
        let alpha = if alpha < 1.0 { (alpha * 255.0).floor() / 255.0 } else { alpha };
        AlphaColor::new([c[0], c[1], c[2], alpha])
    }

    /// Inside optional content that is off.
    fn hidden(&self) -> bool {
        self.marked_off > 0
    }

    /// Whether optional content `o` (a group or a membership dictionary)
    /// is off.
    fn oc_off(&self, o: &Obj) -> bool {
        let f = self.f;
        oc_hidden(o, &self.ocg_off, &|o| f.resolve(o))
    }

    fn fill_path(&mut self, path: &BezPath, rule: Fill, gs: &Gs) {
        if path.elements().is_empty() || self.hidden() {
            return;
        }
        self.prepare(gs);
        let own;
        let gs = if gs.t3_bare && gs.smask.is_some() {
            own = Gs { fill: Ink::Color([0.0; 3]), ..gs.clone() };
            &own
        } else {
            gs
        };
        match &gs.fill {
            Ink::None => {}
            Ink::Color(c) => {
                self.ctx.set_fill_rule(rule);
                self.ctx.set_paint(Self::solid(*c, gs.fill_alpha));
                if let Some(r) = (!self.glyph).then(|| pixel_rect(path, gs.ctm)).flatten() {
                    self.ctx.set_transform(Affine::IDENTITY);
                    self.ctx.fill_rect(&r);
                } else {
                    self.ctx.set_transform(gs.ctm);
                    self.ctx.fill_path(path);
                }
            }
            Ink::Pattern(p, under) => {
                let p = p.clone();
                let mut dev = path.clone();
                dev.apply_affine(gs.ctm);
                self.pattern_fill(&p, *under, &dev, rule, gs, gs.fill_alpha);
            }
        }
    }

    fn stroke_style(&mut self, gs: &Gs, path: &BezPath) -> Stroke {
        // At least a pixel wide, as pdfium draws lines: by the mean of the
        // two axes' scales.
        let c = gs.ctm.as_coeffs();
        let s = (c[0].hypot(c[1]) + c[2].hypot(c[3])) / 2.0;
        let mut w = gs.line_width;
        if s > 0.0 && w * s < 1.0 {
            w = 1.0 / s;
        }
        let mut st = Stroke::new(w).with_caps(gs.cap).with_join(gs.join).with_miter_limit(gs.miter);
        let (pat, off) = &gs.dash;
        if !pat.is_empty() && pat.iter().any(|&v| v > 0.0) && pat.len() < 64 && self.dashes_for(path, pat) {
            let mut p = pat.clone();
            if p.len() % 2 == 1 {
                p.extend_from_within(..);
            }
            st = st.with_dashes(*off, p);
        }
        st
    }

    /// Whether `path` may be cut into dashes of `pat`; they come from the
    /// page's budget.
    fn dashes_for(&mut self, path: &BezPath, pat: &[f64]) -> bool {
        let period: f64 = pat.iter().map(|v| v.abs()).sum();
        let n = path.perimeter(1.0) / period * pat.len() as f64;
        if !n.is_finite() || n > DASH_STROKE || n as usize > self.dashes {
            return false;
        }
        self.dashes -= n as usize;
        true
    }

    fn stroke_path(&mut self, path: &BezPath, gs: &Gs) {
        if path.elements().is_empty() || self.hidden() {
            return;
        }
        self.prepare(gs);
        let st = self.stroke_style(gs, path);
        let open = open_short_subpaths(path);
        let path = open.as_ref().unwrap_or(path);
        let own;
        let gs = if gs.t3_bare && gs.smask.is_some() {
            own = Gs { stroke: Ink::Color([0.0; 3]), ..gs.clone() };
            &own
        } else {
            gs
        };
        match &gs.stroke {
            Ink::None => {}
            Ink::Color(c) => {
                self.ctx.set_transform(gs.ctm);
                self.ctx.set_stroke(st);
                self.ctx.set_paint(Self::solid(*c, gs.stroke_alpha));
                self.ctx.stroke_path(path);
            }
            Ink::Pattern(p, under) => {
                // The stroke's outline, filled with the pattern.
                let outline = vello_cpu::kurbo::stroke(path.iter(), &st, &Default::default(), 0.1);
                let mut dev = outline;
                dev.apply_affine(gs.ctm);
                let p = p.clone();
                self.pattern_fill(&p, *under, &dev, Fill::NonZero, gs, gs.stroke_alpha);
            }
        }
    }

    fn clip(&mut self, path: &BezPath, rule: Fill, gs: &mut Gs) {
        let b = if path.elements().is_empty() { Rect::ZERO } else { gs.ctm.transform_rect_bbox(path.bounding_box()) };
        // Each clip held keeps a copy of the page's coverage: past this
        // many, only their bounds narrow what is drawn.
        if self.layers.len() >= MAX_LAYERS {
            gs.clip = gs.clip.intersect(b);
            return;
        }
        self.ctx.set_fill_rule(rule);
        // A clip path, not a layer: what is drawn inside blends with what
        // is under it. An upright rectangle clips on the whole pixels
        // around it, as pdfium's does.
        if let Some(r) = upright_rect(path, gs.ctm) {
            self.ctx.set_transform(Affine::IDENTITY);
            self.ctx.push_clip_path(&Rect::new(r.x0.floor(), r.y0.floor(), r.x1.ceil(), r.y1.ceil()).to_path(0.1));
        } else {
            self.ctx.set_transform(gs.ctm);
            self.ctx.push_clip_path(path);
        }
        self.layers.push(true);
        gs.clip = gs.clip.intersect(b);
    }

    fn font(&mut self, res: &Dict, name: &str) -> Option<Rc<FontEntry>> {
        let fd = self.res_dict(res, "Font")?;
        match fd.get(name)? {
            Obj::Ref(n, _) => {
                let n = *n;
                if let Some(e) = self.fonts.get(&n) {
                    return Some(e.clone());
                }
                let o = self.f.get(n);
                self.font_obj(Some(n), &o)
            }
            o @ Obj::Dict(_) => self.font_obj(None, &o.clone()),
            _ => None,
        }
    }

    fn font_obj(&mut self, n: Option<u32>, o: &Obj) -> Option<Rc<FontEntry>> {
        if let Some(n) = n
            && let Some(e) = self.fonts.get(&n)
        {
            return Some(e.clone());
        }
        let f = self.f;
        let d = f.resolve(o).dict()?.clone();
        let font = Font::load(f, &d);
        let glyphs = Glyphs::load(f, &d, &font.style, self.fallback);
        let type3 = (f.key(&d, "Subtype").name() == Some("Type3")).then(|| {
            let procs = f.key_dict(&d, "CharProcs").unwrap_or_default();
            let m = f.nums(d.get("FontMatrix").unwrap_or(&Obj::Null));
            let matrix = if m.len() == 6 { pdf_m(&m) } else { Affine::scale(0.001) };
            let res = f.key_dict(&d, "Resources");
            let mut names = HashMap::new();
            if let Some(ed) = f.key_dict(&d, "Encoding")
                && let Obj::Array(diffs) = f.key(&ed, "Differences")
            {
                let mut code = 0u32;
                for o in diffs {
                    match f.resolve(&o) {
                        Obj::Int(c) => code = c.clamp(0, 255) as u32,
                        Obj::Name(nm) => {
                            names.insert(code, nm.to_string());
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
            Type3 { procs, matrix, res, names }
        });
        let e = Rc::new(FontEntry { font, glyphs, type3 });
        if let Some(n) = n {
            self.fonts.insert(n, e.clone());
        }
        Some(e)
    }

    fn show(&mut self, s: &[u8], gs: &Gs, tm: &mut Affine, text_clip: &mut Option<BezPath>) {
        let font = match &gs.font {
            Some(f) => f.clone(),
            None => match self.fallback_font() {
                Some(f) => f,
                None => return,
            },
        };
        let mode = gs.mode;
        let fill = matches!(mode, 0 | 2 | 4 | 6);
        let stroke = matches!(mode, 1 | 2 | 5 | 6);
        let clip = mode >= 4;
        for mut g in font.font.glyphs(s) {
            // No widths in the PDF: a system font's own, as pdfium takes them.
            if g.guessed
                && font.glyphs.substitute()
                && let Some(adv) = font.glyphs.advance(g.code, g.cid, &g.text)
                && adv > 0.0
            {
                g.width = adv;
            }
            let vert = font.font.vertical(&g);
            // Text space to user space (a vertical glyph hung from its origin).
            let mut trm = *tm * Affine::new([gs.size * gs.th, 0.0, 0.0, gs.size, 0.0, gs.rise]);
            if let Some((_, vx, vy)) = vert {
                trm = trm * Affine::translate((-vx * 0.001, -vy * 0.001));
            }
            if let Some(t3) = &font.type3 {
                if fill || stroke {
                    self.type3_glyph(&font, t3, g.code, trm, gs);
                }
            } else if let Some(outline) = font.glyphs.outline(g.code, g.cid, &g.text) {
                let n = outline.elements().len();
                if n > self.glyph_elems {
                    self.glyph_elems = 0;
                } else {
                    self.glyph_elems -= n;
                }
                if self.glyph_elems == 0 {
                    continue;
                }
                let mut p = (*outline).clone();
                // A system font, as pdfium fits it to the PDF's width:
                // narrowed, or centred in a wider one (not glyphs taken by number).
                let mut m = trm;
                if font.glyphs.fits()
                    && let Some(adv) = font.glyphs.advance(g.code, g.cid, &g.text)
                    && adv > 0.0
                    && g.width > 0.0
                {
                    if g.width > adv + 0.001 {
                        if !font.glyphs.by_gid() {
                            m = trm * Affine::translate(((g.width - adv) / 2.0, 0.0));
                        }
                    } else if g.width < adv {
                        m = trm * Affine::scale_non_uniform(g.width / adv, 1.0);
                    }
                }
                // Text up to 50 pixels: pdfium draws each glyph with its
                // baseline rounded to a pixel.
                let dm = gs.ctm * m;
                let c = dm.as_coeffs();
                if fill && !stroke && c[0].abs() + c[1].abs() <= 50.0 {
                    let o = dm * Point::ZERO;
                    let snap = Affine::translate((0.0, o.y.round() - o.y));
                    m = gs.ctm.inverse() * snap * gs.ctm * m;
                }
                p.apply_affine(m);
                if fill {
                    self.glyph = true;
                    self.fill_path(&p, Fill::NonZero, gs);
                    self.glyph = false;
                    self.embolden(&p, gs);
                }
                if stroke {
                    self.stroke_path(&p, gs);
                }
                if clip && let Some(tc) = text_clip.as_mut() {
                    let mut d = p.clone();
                    d.apply_affine(gs.ctm);
                    tc.extend(d);
                }
            }
            let spacing = gs.tc + if g.space { gs.tw } else { 0.0 };
            *tm = *tm
                * match vert {
                    Some((w1, ..)) => Affine::translate((0.0, w1 * 0.001 * gs.size + spacing)),
                    None => Affine::translate(((g.width * gs.size + spacing) * gs.th, 0.0)),
                };
        }
    }

    /// Filled glyphs a hair bolder (a twenty-fifth of a pixel round
    /// them), as pdfium's come out.
    fn embolden(&mut self, p: &BezPath, gs: &Gs) {
        let w = 0.04;
        let s = scale_of(&gs.ctm);
        if s <= 0.0 || self.hidden() || gs.fill_alpha < 1.0 {
            return;
        }
        let Ink::Color(c) = gs.fill else { return };
        self.prepare(gs);
        // In straight pieces on the device: a stroke this thin looks the
        // same, and offsetting curves would take longer than the fill.
        let mut flat = BezPath::new();
        kurbo_flatten(&(gs.ctm * p.clone()), 0.05, &mut flat);
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_stroke(Stroke::new(w));
        self.ctx.set_paint(Self::solid(c, 1.0));
        self.ctx.stroke_path(&flat);
    }

    fn fallback_font(&mut self) -> Option<Rc<FontEntry>> {
        let d = Dict::of(vec![("Type", Obj::name_of("Font")), ("Subtype", Obj::name_of("Type1")), ("BaseFont", Obj::name_of("Helvetica"))]);
        // Kept as an object no file has.
        self.font_obj(Some(u32::MAX), &Obj::Dict(d))
    }

    fn type3_glyph(&mut self, font: &FontEntry, t3: &Type3, code: u32, trm: Affine, gs: &Gs) {
        let Some(name) = t3.names.get(&code) else { return };
        let id = if let Some(Obj::Ref(n, _)) = t3.procs.get(name) { Some(*n) } else { None };
        let Obj::Stream(d, raw) = self.f.key(&t3.procs, name) else { return };
        if self.nesting.len() > 12 {
            return;
        }
        let data = self.decoded(id, &d, &raw);
        let mut g = gs.clone();
        g.ctm = gs.ctm * trm * t3.matrix;
        g.font = None;
        // `d1` glyphs take the colour of the text.
        g.uncolored = data.windows(2).any(|w| w == b"d1");
        g.t3_bare = g.uncolored && gs.smask.is_none();
        // Filled text: its strokes too take the fill colour, as in pdfium.
        if g.uncolored && gs.mode == 0 {
            g.stroke = g.fill.clone();
            g.stroke_alpha = g.fill_alpha;
        }
        g.mode = 0;
        let res = t3.res.clone().unwrap_or_default();
        let _ = font;
        self.nesting.push(u32::MAX);
        self.run(&data, &res, g);
        self.nesting.pop();
    }

    fn xobject(&mut self, o: &Obj, res: &Dict, gs: &Gs) {
        let f = self.f;
        let id = match o {
            Obj::Ref(n, _) => Some(*n),
            _ => None,
        };
        let Obj::Stream(d, raw) = f.resolve(o) else { return };
        if let Some(oc) = d.get("OC")
            && self.oc_off(oc)
        {
            return;
        }
        match f.key(&d, "Subtype").name() {
            Some("Image") => {
                let fill = match &gs.fill {
                    Ink::Color(c) => *c,
                    Ink::Pattern(_, Some(c)) => *c,
                    Ink::None => return,
                    _ => [0.5; 3],
                };
                let mask = matches!(f.key(&d, "ImageMask"), Obj::Bool(true));
                // Pixels the image takes on the page, to shrink big pictures.
                let key_fill = if mask { (fill[0] * 255.0) as u32 * 65536 + (fill[1] * 255.0) as u32 * 256 + (fill[2] * 255.0) as u32 } else { 0 };
                let pix = match id.and_then(|n| self.images.get(&(n, key_fill))) {
                    Some(p) => p.clone(),
                    None => {
                        let p = self.image_pixmap(&d, &raw, fill, None, gs.ctm);
                        if let Some(n) = id {
                            self.images.insert((n, key_fill), p.clone());
                        }
                        p
                    }
                };
                if let Some(p) = pix {
                    if mask && matches!(gs.fill, Ink::Pattern(_, None)) && gs.smask.is_none() {
                        self.pattern_stencil(p, gs);
                    } else {
                        self.draw_image(p, gs);
                    }
                }
            }
            Some("Form") => {
                if self.nesting.len() > 24 || id.is_some_and(|n| self.nesting.contains(&n)) {
                    return;
                }
                self.form(&d, &raw, res, gs, id);
            }
            _ => {}
        }
    }

    fn form(&mut self, d: &Dict, raw: &[u8], res: &Dict, gs: &Gs, id: Option<u32>) {
        let f = self.f;
        let mut g = gs.clone();
        g.ctm = gs.ctm * pdf_m(&f.nums(d.get("Matrix").unwrap_or(&Obj::Null)));
        g.base = g.ctm;
        let r2 = f.key_dict(d, "Resources").unwrap_or_else(|| res.clone());
        self.ops = self.ops.saturating_add(r2.0.iter().fold(0usize, |w, (_, o)| w.saturating_add(o.weight())) / 16);
        let data = self.decoded(id, d, raw);
        self.nesting.push(id.unwrap_or(0));
        let depth = self.layers.len();
        // Clipped to its box.
        let bb = f.nums(d.get("BBox").unwrap_or(&Obj::Null));
        if bb.len() == 4 {
            let mut p = BezPath::new();
            p.move_to((bb[0], bb[1]));
            p.line_to((bb[2], bb[1]));
            p.line_to((bb[2], bb[3]));
            p.line_to((bb[0], bb[3]));
            p.close_path();
            self.clip(&p, Fill::NonZero, &mut g);
        }
        // A transparency group: composited as a whole.
        let group = f.key_dict(d, "Group").is_some_and(|gd| f.key(&gd, "S").name() == Some("Transparency"));
        if group && (gs.fill_alpha < 1.0 || gs.blend != BlendMode::default() || gs.smask.is_some()) {
            let mask = gs.smask.clone().filter(|m| m.width() == self.width && m.height() == self.height);
            self.ctx.reset_mask();
            self.ctx.push_layer(None, Some(gs.blend), Some(gs.fill_alpha), mask, None);
            self.layers.push(false);
            g.fill_alpha = 1.0;
            g.stroke_alpha = 1.0;
            g.blend = BlendMode::default();
            g.smask = None;
        }
        self.run(&data, &r2, g);
        self.pop_to(depth);
        self.nesting.pop();
    }

    /// An image's pixels, shrunk when it is far bigger than drawn.
    fn image_pixmap(&mut self, d: &Dict, raw: &[u8], fill: [f32; 3], res: Option<&Dict>, ctm: Affine) -> Option<Arc<Pixmap>> {
        let c = ctm.as_coeffs();
        let (dw, dh) = (c[0].hypot(c[1]), c[2].hypot(c[3]));
        let f = self.f;
        let px = f.key_num(d, "Width").unwrap_or(0.0) * f.key_num(d, "Height").unwrap_or(0.0);
        if px as usize > self.pixels {
            return None;
        }
        self.pixels -= px as usize;
        let fill = [(fill[0] * 255.0).round() as u8, (fill[1] * 255.0).round() as u8, (fill[2] * 255.0).round() as u8, 255];
        let img = images::decode(f, d, raw, fill, res, (dw, dh))?;
        let (mut w, mut h, mut data) = (img.w, img.h, img.data);
        // Box filter down to about the size drawn (bicubic does the rest).
        let fx = ((w as f64 / dw.max(1.0)).floor() as usize).max(1);
        let fy = ((h as f64 / dh.max(1.0)).floor() as usize).max(1);
        if fx > 1 || fy > 1 {
            let (nw, nh) = (w.div_ceil(fx), h.div_ceil(fy));
            let mut out = vec![0u8; nw * nh * 4];
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0u32; 4];
                    let mut n = 0u32;
                    for yy in y * fy..((y + 1) * fy).min(h) {
                        for xx in x * fx..((x + 1) * fx).min(w) {
                            let i = (yy * w + xx) * 4;
                            let a = data[i + 3] as u32;
                            acc[0] += data[i] as u32 * a;
                            acc[1] += data[i + 1] as u32 * a;
                            acc[2] += data[i + 2] as u32 * a;
                            acc[3] += a;
                            n += 1;
                        }
                    }
                    let o = (y * nw + x) * 4;
                    if acc[3] > 0 {
                        out[o] = (acc[0] / acc[3]) as u8;
                        out[o + 1] = (acc[1] / acc[3]) as u8;
                        out[o + 2] = (acc[2] / acc[3]) as u8;
                    }
                    out[o + 3] = (acc[3] / n.max(1)) as u8;
                }
            }
            (w, h, data) = (nw, nh, out);
        }
        if w > 65535 || h > 65535 {
            return None;
        }
        Some(Arc::new(pixmap(w, h, &data)))
    }

    fn draw_image(&mut self, p: Arc<Pixmap>, gs: &Gs) {
        if self.hidden() {
            return;
        }
        let (w, h) = (p.width() as f64, p.height() as f64);
        self.prepare(gs);
        // Upright: on the whole pixels around it, as pdfium places it
        // (not in Type 3 glyphs, which pdfium draws its own way).
        let c = gs.ctm.as_coeffs();
        let ctm = if c[1] == 0.0 && c[2] == 0.0 && c[0] != 0.0 && c[3] != 0.0 && !self.nesting.contains(&u32::MAX) {
            let r = gs.ctm.transform_rect_bbox(Rect::new(0.0, 0.0, 1.0, 1.0));
            let r = Rect::new(r.x0.floor(), r.y0.floor(), r.x1.ceil(), r.y1.ceil());
            let (a, e) = if c[0] > 0.0 { (r.width(), r.x0) } else { (-r.width(), r.x1) };
            let (d, f) = if c[3] > 0.0 { (r.height(), r.y0) } else { (-r.height(), r.y1) };
            Affine::new([a, 0.0, 0.0, d, e, f])
        } else {
            gs.ctm
        };
        // The unit square, from the image's top row down.
        let t = ctm * Affine::new([1.0 / w, 0.0, 0.0, -1.0 / h, 0.0, 1.0]);
        self.ctx.set_transform(t);
        self.ctx.set_fill_rule(Fill::NonZero);
        // Bicubic, as pdfium resamples; enlarged a lot, pixels stay sharp
        // as there.
        let up = c[0].hypot(c[1]) >= 4.0 * w && c[2].hypot(c[3]) >= 4.0 * h;
        let quality = if up { ImageQuality::Low } else { ImageQuality::High };
        let sampler = ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality, alpha: 1.0 };
        self.ctx.set_paint(PaintType::Image(Image { image: ImageSource::Pixmap(p), sampler }));
        self.faded(gs.fill_alpha, |s| s.ctx.fill_rect(&Rect::new(0.0, 0.0, w, h)));
    }

    /// A stencil mask painted with a pattern: the pattern through the
    /// mask's shape.
    fn pattern_stencil(&mut self, p: Arc<Pixmap>, gs: &Gs) {
        if self.hidden() {
            return;
        }
        let (w, h) = (self.width, self.height);
        let mut g = gs.clone();
        g.fill_alpha = 1.0;
        let pm = self.offscreen(w, h, |r| r.draw_image(p, &g));
        let mut g = gs.clone();
        g.smask = Some(Mask::new_alpha(&pm));
        let unit = Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1);
        self.fill_path(&unit, Fill::NonZero, &g);
    }

    fn inline_image(&mut self, lx: &mut Lexer, res: &Dict, gs: &Gs) {
        let mut d = Dict::default();
        let mut key: Option<String> = None;
        let mut abbr: Vec<String> = Vec::new();
        // An image's own entries are a handful.
        let mut entries = 0;
        loop {
            entries += 1;
            if entries > 512 {
                return;
            }
            match lx.next(false) {
                Some(Item::Kw(b"ID")) => break,
                Some(Item::Kw(_)) | None => return,
                Some(Item::Obj(o)) => match key.take() {
                    None => key = o.name().map(str::to_string),
                    Some(k) => {
                        let full = match k.as_str() {
                            "W" => "Width",
                            "H" => "Height",
                            "BPC" => "BitsPerComponent",
                            "CS" => "ColorSpace",
                            "D" => "Decode",
                            "DP" => "DecodeParms",
                            "F" => "Filter",
                            "IM" => "ImageMask",
                            "I" => "Interpolate",
                            other => other,
                        };
                        // Both names given: the abbreviation's value (as pdfium).
                        if full == k.as_str() {
                            if !abbr.iter().any(|a| a == full) {
                                d.set(full, o);
                            }
                        } else {
                            abbr.push(full.to_string());
                            d.set(full, o);
                        }
                    }
                },
            }
        }
        let data = lx.d;
        let begin = (lx.p + 1).min(data.len());
        let f = self.f;
        // Unfiltered data has a known length; else up to "EI".
        let w = f.key_num(&d, "Width").unwrap_or(0.0) as usize;
        let h = f.key_num(&d, "Height").unwrap_or(0.0) as usize;
        let mask = matches!(f.key(&d, "ImageMask"), Obj::Bool(true));
        let bpc = if mask { 1 } else { f.key_num(&d, "BitsPerComponent").unwrap_or(8.0) as usize };
        let filtered = d.get("Filter").is_some();
        let mut end = None;
        if !filtered {
            let n = if mask { 1 } else { Cs::load(f, d.get("ColorSpace").unwrap_or(&Obj::Null), Some(res)).n() };
            let len = w.checked_mul(n).and_then(|x| x.checked_mul(bpc)).map(|x| x.div_ceil(8)).and_then(|x| x.checked_mul(h));
            if let Some(e) = len.and_then(|l| begin.checked_add(l)).filter(|e| *e <= data.len()) {
                end = Some(e);
            }
        }
        let stop = |from: usize| -> usize {
            let mut i = from;
            while let Some(e) = memchr::memmem::find(&data[i..], b"EI") {
                let at = i + e;
                if at > 0 && super::obj::is_ws(data[at - 1]) && data.get(at + 2).is_none_or(|&b| super::obj::is_ws(b)) {
                    return at;
                }
                i = at + 2;
            }
            data.len()
        };
        let img_end = match end {
            Some(e) => e,
            None => stop(begin).saturating_sub(1).max(begin),
        };
        let after = stop(img_end.min(data.len()));
        lx.p = (after + 2).min(data.len());
        let raw = &data[begin..img_end.min(data.len())];
        let fill = match &gs.fill {
            Ink::Color(c) => *c,
            _ => [0.0; 3],
        };
        if let Some(p) = self.image_pixmap(&d, raw, fill, Some(res), gs.ctm) {
            self.draw_image(p, gs);
        }
    }

    /// Paints a shading over the clip (`area`: a device-space path to
    /// keep it in). `to_dev` maps shading space to device pixels.
    fn shade(&mut self, sh: &Shading, area: Option<(&BezPath, Fill)>, to_dev: Affine, gs: &Gs) {
        if self.hidden() {
            return;
        }
        let mut bounds = gs.clip.intersect(Rect::new(0.0, 0.0, self.width as f64, self.height as f64));
        if let Some((a, _)) = area {
            bounds = bounds.intersect(a.bounding_box());
        }
        if let Some(b) = sh.bbox {
            bounds = bounds.intersect(to_dev.transform_rect_bbox(Rect::new(b[0], b[1], b[2], b[3])));
        }
        let b = bounds.expand();
        if b.width() < 1.0 || b.height() < 1.0 {
            return;
        }
        let (x0, y0, w, h) = (b.x0 as i32, b.y0 as i32, b.width() as usize, b.height() as usize);
        // What drawing it takes: each pixel, by the work of its colour.
        let work = sh.work(x0, y0, w, h, to_dev);
        if w * h > self.pixels.min(64_000_000) || work > self.pixels {
            return;
        }
        self.pixels -= work;
        let px = sh.raster(x0, y0, w, h, to_dev);
        let pm = Arc::new(pixmap(w, h, &px));
        self.prepare(gs);
        let depth = self.layers.len();
        if let Some((a, rule)) = area {
            self.ctx.set_transform(Affine::IDENTITY);
            self.ctx.set_fill_rule(rule);
            self.ctx.push_clip_path(a);
            self.layers.push(true);
            self.prepare(gs);
        }
        self.ctx.set_transform(Affine::translate((x0 as f64, y0 as f64)));
        let sampler = ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality: ImageQuality::Low, alpha: 1.0 };
        self.ctx.set_paint(PaintType::Image(Image { image: ImageSource::Pixmap(pm), sampler }));
        self.faded(gs.fill_alpha, |s| s.ctx.fill_rect(&Rect::new(0.0, 0.0, w as f64, h as f64)));
        self.pop_to(depth);
    }

    /// Fills `dev` (a path in device pixels) with a pattern.
    fn pattern_fill(&mut self, p: &Obj, under: Option<[f32; 3]>, dev: &BezPath, rule: Fill, gs: &Gs, alpha: f32) {
        let f = self.f;
        let id = match p {
            Obj::Ref(n, _) => Some(*n),
            _ => None,
        };
        let pat = f.resolve(p);
        let Some(d) = pat.dict().cloned().or_else(|| if let Obj::Stream(d, _) = &pat { Some(d.clone()) } else { None }) else { return };
        let m = pdf_m(&f.nums(d.get("Matrix").unwrap_or(&Obj::Null)));
        let to_dev = gs.base * m;
        match f.key(&d, "PatternType").int() {
            Some(2) => {
                let Some(sh) = self.shading(d.get("Shading").unwrap_or(&Obj::Null), None) else { return };
                let mut g = gs.clone();
                g.fill_alpha = alpha;
                self.shade(&sh, Some((dev, rule)), to_dev, &g);
            }
            Some(1) => {
                let Obj::Stream(_, raw) = &pat else { return };
                if self.nesting.len() > 12 || id.is_some_and(|n| self.nesting.contains(&n)) {
                    return;
                }
                let bb = f.nums(d.get("BBox").unwrap_or(&Obj::Null));
                if bb.len() != 4 {
                    return;
                }
                let (bx0, by0, bx1, by1) = (bb[0].min(bb[2]), bb[1].min(bb[3]), bb[0].max(bb[2]), bb[1].max(bb[3]));
                let xs = f.key_num(&d, "XStep").unwrap_or(bx1 - bx0).abs();
                let ys = f.key_num(&d, "YStep").unwrap_or(by1 - by0).abs();
                if xs <= 0.0 || ys <= 0.0 || !xs.is_finite() || !ys.is_finite() {
                    return;
                }
                let c = to_dev.as_coeffs();
                if c[1] == 0.0 && c[2] == 0.0 && self.tiles((&pat, id.unwrap_or(0)), &d, (bx0, by0, bx1, by1), (xs, ys), to_dev, under, dev, rule, gs, alpha) {
                    return;
                }
                let (sx, sy) = (c[0].hypot(c[1]), c[2].hypot(c[3]));
                let mut tw = (xs * sx).ceil();
                let mut th = (ys * sy).ceil();
                if tw < 1.0 || th < 1.0 {
                    // Tiles under a pixel: their average colour would do; skip.
                    tw = tw.max(1.0);
                    th = th.max(1.0);
                }
                let k = (2048.0 / tw.max(th)).min(1.0);
                let (tw, th) = (((tw * k).round()).max(1.0), ((th * k).round()).max(1.0));
                if tw * th > 8_000_000.0 || (tw * th) as usize > self.pixels {
                    return;
                }
                self.pixels -= (tw * th) as usize;
                // Pattern space to tile pixels.
                let tile = Affine::new([tw / xs, 0.0, 0.0, -th / ys, -bx0 * tw / xs, (by0 + ys) * th / ys]);
                let uncolored = f.key(&d, "PaintType").int() == Some(2);
                let res = f.key_dict(&d, "Resources").unwrap_or_default();
                let data = self.decoded(id, &d, raw);
                let mut g = Gs::new(tile, Rect::new(0.0, 0.0, tw, th));
                g.base = self.cell_base(tile, to_dev, gs);
                if uncolored {
                    let c = under.unwrap_or([0.0; 3]);
                    g.fill = Ink::Color(c);
                    g.stroke = Ink::Color(c);
                    g.uncolored = true;
                }
                self.nesting.push(id.unwrap_or(0));
                self.cells += 1;
                let pm = self.offscreen(tw as u16, th as u16, |r| r.run(&data, &res, g));
                self.cells -= 1;
                self.nesting.pop();
                // Tile pixels to device.
                let t = to_dev * tile.inverse();
                self.prepare(gs);
                self.ctx.set_transform(Affine::IDENTITY);
                self.ctx.set_fill_rule(rule);
                self.ctx.set_paint_transform(t);
                let sampler = ImageSampler { x_extend: Extend::Repeat, y_extend: Extend::Repeat, quality: ImageQuality::Medium, alpha: 1.0 };
                self.ctx.set_paint(PaintType::Image(Image { image: ImageSource::Pixmap(Arc::new(pm)), sampler }));
                self.faded(alpha, |s| s.ctx.fill_path(dev));
                self.ctx.reset_paint_transform();
            }
            _ => {}
        }
    }

    /// The base for patterns inside a tiling cell: pdfium places them by
    /// the page's base and the cell pattern's Matrix, leaving aside the
    /// form the cell was painted in. On the page only; deeper, the tile.
    fn cell_base(&self, tile: Affine, to_dev: Affine, gs: &Gs) -> Affine {
        if self.cells > 0 {
            return tile;
        }
        tile * to_dev.inverse() * self.page_base * gs.base.inverse() * to_dev
    }

    /// A tiling pattern on an upright page as pdfium draws it: the cell
    /// (its box stretched to whole pixels) copied at each step's position
    /// rounded to a pixel. False when there would be too many cells.
    #[allow(clippy::too_many_arguments)]
    fn tiles(&mut self, (pat, id): (&Obj, u32), d: &Dict, bb: (f64, f64, f64, f64), (xs, ys): (f64, f64), to_dev: Affine, under: Option<[f32; 3]>, dev: &BezPath, rule: Fill, gs: &Gs, alpha: f32) -> bool {
        let f = self.f;
        let Obj::Stream(_, raw) = pat else { return true };
        let cell = to_dev.transform_rect_bbox(Rect::new(bb.0, bb.1, bb.2, bb.3));
        let (tw, th) = (cell.width().ceil().max(1.0), cell.height().ceil().max(1.0));
        let area = dev.bounding_box().intersect(Rect::new(0.0, 0.0, self.width as f64, self.height as f64));
        if area.width() <= 0.0 || area.height() <= 0.0 {
            return true;
        }
        let (ax0, ay0, ax1, ay1) = (area.x0.floor(), area.y0.floor(), area.x1.ceil(), area.y1.ceil());
        if tw > 2048.0 || th > 2048.0 {
            return false;
        }
        // The cells over the area.
        let inv = to_dev.inverse();
        let pr = inv.transform_rect_bbox(Rect::new(ax0, ay0, ax1, ay1));
        let (c0, c1) = (((pr.x0 - bb.2) / xs).floor() - 1.0, ((pr.x1 - bb.0) / xs).ceil() + 1.0);
        let (r0, r1) = (((pr.y0 - bb.3) / ys).floor() - 1.0, ((pr.y1 - bb.1) / ys).ceil() + 1.0);
        if (c1 - c0 + 1.0) * (r1 - r0 + 1.0) > 40_000.0 {
            return false;
        }
        // Each fill copies the cell that many times: from the page's
        // pixel budget, or a big cell at small steps filled again and
        // again would take hours.
        let work = ((c1 - c0 + 1.0) * (r1 - r0 + 1.0) * tw * th) as usize;
        if work > self.pixels {
            return false;
        }
        self.pixels -= work;
        // The cell's box onto the tile's pixels.
        let tile = Affine::scale_non_uniform(tw / cell.width().max(1e-9), th / cell.height().max(1e-9)) * Affine::translate((-cell.x0, -cell.y0)) * to_dev;
        let res = f.key_dict(d, "Resources").unwrap_or_default();
        let data = self.decoded(Some(id).filter(|&n| n != 0), d, raw);
        let mut g = Gs::new(tile, Rect::new(0.0, 0.0, tw, th));
        g.base = self.cell_base(tile, to_dev, gs);
        if f.key(d, "PaintType").int() == Some(2) {
            let c = under.unwrap_or([0.0; 3]);
            g.fill = Ink::Color(c);
            g.stroke = Ink::Color(c);
            g.uncolored = true;
        }
        self.nesting.push(id);
        self.cells += 1;
        let pm = self.offscreen(tw as u16, th as u16, |r| r.run(&data, &res, g));
        self.cells -= 1;
        self.nesting.pop();
        let (tw, th) = (tw as i64, th as i64);
        let (aw, ah) = ((ax1 - ax0) as i64, (ay1 - ay0) as i64);
        let mut out = vec![vello_cpu::color::PremulRgba8 { r: 0, g: 0, b: 0, a: 0 }; (aw * ah) as usize];
        let src = pm.data();
        let e = to_dev.as_coeffs();
        let (lo, to) = (cell.x0 - e[4], cell.y0 - e[5]);
        let mut row = r0;
        while row <= r1 {
            let mut col = c0;
            while col <= c1 {
                let o = to_dev * Point::new(col * xs, row * ys);
                let (sx, sy) = ((o.x + lo).round() as i64 - ax0 as i64, (o.y + to).round() as i64 - ay0 as i64);
                if sx < aw && sy < ah && sx + tw > 0 && sy + th > 0 {
                    for y in sy.max(0)..(sy + th).min(ah) {
                        for x in sx.max(0)..(sx + tw).min(aw) {
                            let s = src[((y - sy) * tw + (x - sx)) as usize];
                            if s.a == 0 {
                                continue;
                            }
                            let dpx = &mut out[(y * aw + x) as usize];
                            let k = 255 - s.a as u16;
                            let m = |a: u8, b: u8| (a as u16 + (b as u16 * k + 127) / 255).min(255) as u8;
                            *dpx = vello_cpu::color::PremulRgba8 { r: m(s.r, dpx.r), g: m(s.g, dpx.g), b: m(s.b, dpx.b), a: m(s.a, dpx.a) };
                        }
                    }
                }
                col += 1.0;
            }
            row += 1.0;
        }
        let mut big = Pixmap::from_parts(out, aw as u16, ah as u16);
        big.set_may_have_transparency(true);
        self.prepare(gs);
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_fill_rule(rule);
        self.ctx.set_paint_transform(Affine::translate((ax0, ay0)));
        let sampler = ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality: ImageQuality::Low, alpha: 1.0 };
        self.ctx.set_paint(PaintType::Image(Image { image: ImageSource::Pixmap(Arc::new(big)), sampler }));
        self.faded(alpha, |s| s.ctx.fill_path(dev));
        self.ctx.reset_paint_transform();
        true
    }

    /// Paints with `draw` at `alpha` (vello can't fade image paints).
    fn faded(&mut self, alpha: f32, draw: impl FnOnce(&mut Self)) {
        if alpha >= 1.0 {
            return draw(self);
        }
        self.ctx.push_opacity_layer((alpha * 255.0).floor() / 255.0);
        draw(self);
        self.ctx.pop_layer();
    }

    /// Runs `draw` into a transparent `w` x `h` canvas of its own.
    fn offscreen(&mut self, w: u16, h: u16, draw: impl FnOnce(&mut Self)) -> Pixmap {
        let ctx = std::mem::replace(&mut self.ctx, RenderContext::new_with(w, h, settings(0)));
        let layers = std::mem::take(&mut self.layers);
        let (ow, oh) = (self.width, self.height);
        (self.width, self.height) = (w, h);
        draw(self);
        self.pop_to(0);
        let mut pm = Pixmap::new(w, h);
        self.ctx.flush();
        self.ctx.render(&mut pm, &mut self.resources);
        self.ctx = ctx;
        self.layers = layers;
        (self.width, self.height) = (ow, oh);
        pm
    }

    /// A soft mask of an ExtGState, drawn for the page.
    fn soft_mask(&mut self, sm: &Dict, gs: &Gs) -> Option<Mask> {
        // The same group drawn the same way: the mask made before.
        let key = match sm.get("G") {
            Some(Obj::Ref(n, _)) => {
                let bits = |m: &Affine| m.as_coeffs().map(f64::to_bits);
                let mut k: Vec<u64> = bits(&gs.ctm).into_iter().chain(bits(&gs.base)).collect();
                k.extend(self.f.nums(sm.get("BC").unwrap_or(&Obj::Null)).into_iter().map(f64::to_bits));
                k.push(self.width as u64 * 65536 + self.height as u64);
                k.push(self.cells as u64);
                Some((*n, self.f.key(sm, "S").name() != Some("Alpha"), k))
            }
            _ => None,
        };
        if let Some(m) = key.as_ref().and_then(|k| self.masks.get(k)) {
            return m.clone();
        }
        let m = self.make_soft_mask(sm, gs);
        if let Some(k) = key
            && self.masks.len() < 32
        {
            self.masks.insert(k, m.clone());
        }
        m
    }

    fn make_soft_mask(&mut self, sm: &Dict, gs: &Gs) -> Option<Mask> {
        let f = self.f;
        let Obj::Stream(gd, raw) = f.key(sm, "G") else { return None };
        if self.nesting.len() > 12 {
            return None;
        }
        let luminosity = f.key(sm, "S").name() != Some("Alpha");
        let (w, h) = (self.width, self.height);
        // Each mask is a page of pixels.
        let page = w as usize * h as usize;
        if page > self.pixels {
            return None;
        }
        self.pixels -= page;
        let bc: Vec<f32> = f.nums(sm.get("BC").unwrap_or(&Obj::Null)).into_iter().map(|v| v as f32).collect();
        let cs = f.key_dict(&gd, "Group").map(|g| Cs::load(f, &f.key(&g, "CS"), None)).unwrap_or(Cs::Gray);
        // No backdrop given: zero in the group's space (white in CMYK).
        let backdrop = if bc.len() == cs.n() { cs.rgb(&bc) } else { cs.rgb(&vec![0.0; cs.n()]) };
        let mut g = Gs::new(gs.ctm, Rect::new(0.0, 0.0, w as f64, h as f64));
        g.ctm = gs.ctm;
        g.base = gs.base;
        let res = f.key_dict(&gd, "Resources").unwrap_or_default();
        let pm = self.offscreen(w, h, |r| {
            if luminosity {
                r.ctx.set_paint(Self::solid(backdrop, 1.0));
                r.ctx.fill_rect(&Rect::new(0.0, 0.0, w as f64, h as f64));
            }
            r.form(&gd, &raw, &res, &g, None);
        });
        Some(if luminosity { Mask::new_luminance(&pm) } else { Mask::new_alpha(&pm) })
    }

    /// The appearances of the page's annotations.
    fn annotations(&mut self, page: &Dict, gs: &Gs) {
        let f = self.f;
        let Obj::Array(annots) = f.key(page, "Annots") else { return };
        // A form that asks for appearances: pdfium makes its fields'.
        let regenerate = f.key_dict(&f.trailer, "Root").and_then(|r| f.key_dict(&r, "AcroForm")).is_some_and(|a| matches!(f.key(&a, "NeedAppearances"), Obj::Bool(true)));
        for a in annots.iter().take(5000) {
            let Some(ad) = f.resolve(a).dict().cloned() else { continue };
            let flags = f.key_num(&ad, "F").unwrap_or(0.0) as u32;
            if flags & (2 | 32) != 0 || f.key(&ad, "Subtype").name() == Some("Popup") || ad.get("OC").is_some_and(|oc| self.oc_off(oc)) {
                continue;
            }
            let given = f.key_dict(&ad, "AP").map(|ap| match f.key(&ap, "N") {
                s @ Obj::Stream(..) => s,
                Obj::Dict(states) => {
                    // No state of that name: nothing drawn.
                    let st = f.key(&ad, "AS").name().map(str::to_string).unwrap_or_default();
                    states.get(&st).map_or(Obj::Bool(false), |s| f.resolve(s))
                }
                o => o,
            });
            // A form field asked to, or without a normal appearance (of
            // its state): pdfium makes one.
            let valid = matches!(given, Some(Obj::Stream(..)));
            // (A list box without one is drawn as below.)
            let list = self.field_attr(&ad, "FT").name() == Some("Ch") && self.field_attr(&ad, "Ff").int().unwrap_or(0) & (1 << 17) == 0;
            let field = if (regenerate || !valid && !list) && f.key(&ad, "Subtype").name() == Some("Widget") {
                self.text_field_appearance(&ad).or_else(|| if !valid { self.check_box_appearance(&ad) } else { None })
            } else {
                None
            };
            let (n, made) = match (field, given) {
                (Some(s), _) => (s, true),
                (None, Some(Obj::Null) | None) => match self.default_appearance(&ad) {
                    Some(s) => (s, true),
                    None => continue,
                },
                (None, Some(s)) => (s, false),
            };
            let Obj::Stream(mut d, raw) = n else { continue };
            let mut rect = f.nums(ad.get("Rect").unwrap_or(&Obj::Null));
            // A text markup appearance pdfium made covers its quadrilaterals,
            // an ink one the rectangle grown by half the line's width.
            if made && matches!(f.key(&ad, "Subtype").name(), Some("Highlight" | "Underline" | "StrikeOut" | "Squiggly" | "Ink")) {
                rect = f.nums(d.get("BBox").unwrap_or(&Obj::Null));
            }
            if rect.len() != 4 {
                continue;
            }
            let bb = f.nums(d.get("BBox").unwrap_or(&Obj::Null));
            if bb.len() != 4 {
                // No box: drawn from the rectangle's corner, unclipped, as
                // pdfium does.
                let mut g = gs.clone();
                g.ctm = gs.ctm * Affine::translate((rect[0].min(rect[2]), rect[1].min(rect[3])));
                d.set("BBox", Obj::Array([-1e5, -1e5, 1e5, 1e5].map(Obj::Real).to_vec()));
                self.form(&d, &raw, &Dict::default(), &g, None);
                continue;
            }
            let m = pdf_m(&f.nums(d.get("Matrix").unwrap_or(&Obj::Null)));
            let b = m.transform_rect_bbox(Rect::new(bb[0].min(bb[2]), bb[1].min(bb[3]), bb[0].max(bb[2]), bb[1].max(bb[3])));
            let r = Rect::new(rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3]));
            if b.width() <= 0.0 || b.height() <= 0.0 {
                continue;
            }
            // The appearance's box onto the annotation's rectangle.
            let fit = Affine::translate((r.x0, r.y0)) * Affine::scale_non_uniform(r.width() / b.width(), r.height() / b.height()) * Affine::translate((-b.x0, -b.y0));
            let mut g = gs.clone();
            g.ctm = gs.ctm * fit;
            self.form(&d, &raw, &Dict::default(), &g, None);
        }
    }
}

impl R<'_, '_> {
    /// The appearance pdfium makes for a square or circle annotation that
    /// has none: its border in C, its inside in IC.
    fn default_appearance(&self, ad: &Dict) -> Option<Obj> {
        let f = self.f;
        let circle = match f.key(ad, "Subtype").name()? {
            "Square" => false,
            "Circle" => true,
            "FreeText" => return self.free_text_appearance(ad),
            "Widget" => return self.list_box_appearance(ad),
            k @ ("Highlight" | "Underline" | "StrikeOut" | "Squiggly") => return self.markup_appearance(ad, k),
            "Ink" => return self.ink_appearance(ad),
            _ => return None,
        };
        let r = f.nums(ad.get("Rect")?);
        if r.len() != 4 {
            return None;
        }
        let (x0, y0, x1, y1) = (r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3]));
        let bs = f.key_dict(ad, "BS");
        let w = bs.as_ref().and_then(|b| f.key_num(b, "W")).or_else(|| f.nums(ad.get("Border").unwrap_or(&Obj::Null)).get(2).copied()).unwrap_or(1.0).max(0.0);
        let color = |key: &str, stroke: bool| -> Option<String> {
            let c = f.nums(ad.get(key)?);
            let op = match c.len() {
                1 => "g",
                3 => "rg",
                4 => "k",
                _ => return None,
            };
            let nums: Vec<String> = c.iter().map(|v| format!("{v}")).collect();
            Some(format!("{} {}\n", nums.join(" "), if stroke { op.to_uppercase() } else { op.to_string() }))
        };
        let fill = color("IC", false);
        let stroke = w > 0.0;
        if fill.is_none() && !stroke {
            return None;
        }
        let mut s = String::new();
        let ca = f.key_num(ad, "CA").filter(|a| *a < 1.0);
        if ca.is_some() {
            s.push_str("/GS gs\n");
        }
        s.push_str(fill.as_deref().unwrap_or(""));
        if stroke {
            s.push_str(&color("C", true).unwrap_or_else(|| "0 G\n".into()));
            s.push_str(&format!("{w} w\n"));
            if bs.as_ref().and_then(|b| f.key(b, "S").name().map(str::to_string)).as_deref() == Some("D") {
                let d = bs.as_ref().map(|b| f.nums(b.get("D").unwrap_or(&Obj::Null))).filter(|d| !d.is_empty()).unwrap_or(vec![3.0]);
                let d: Vec<String> = d.iter().map(|v| format!("{v}")).collect();
                s.push_str(&format!("[{}] 0 d\n", d.join(" ")));
            }
        }
        let h = w / 2.0;
        let (a, b, c, d) = (x0 + h, y0 + h, x1 - h, y1 - h);
        if circle {
            let (cx, cy, rx, ry) = ((a + c) / 2.0, (b + d) / 2.0, (c - a) / 2.0, (d - b) / 2.0);
            let (kx, ky) = (rx * 0.5523, ry * 0.5523);
            s.push_str(&format!("{} {cy} m\n", cx - rx));
            s.push_str(&format!("{} {} {} {} {cx} {} c\n", cx - rx, cy + ky, cx - kx, cy + ry, cy + ry));
            s.push_str(&format!("{} {} {} {} {} {cy} c\n", cx + kx, cy + ry, cx + rx, cy + ky, cx + rx));
            s.push_str(&format!("{} {} {} {} {cx} {} c\n", cx + rx, cy - ky, cx + kx, cy - ry, cy - ry));
            s.push_str(&format!("{} {} {} {} {} {cy} c\n", cx - kx, cy - ry, cx - rx, cy - ky, cx - rx));
        } else {
            s.push_str(&format!("{a} {b} {} {} re\n", c - a, d - b));
        }
        s.push_str(match (fill.is_some(), stroke) {
            (true, true) => "b\n",
            (true, false) => "f\n",
            _ => "S\n",
        });
        let mut dict = Dict::default();
        dict.set("BBox", Obj::Array(vec![Obj::Real(x0), Obj::Real(y0), Obj::Real(x1), Obj::Real(y1)]));
        if let Some(ca) = ca {
            let mut gs = Dict::default();
            gs.set("CA", Obj::Real(ca));
            gs.set("ca", Obj::Real(ca));
            let mut eg = Dict::default();
            eg.set("GS", Obj::Dict(gs));
            let mut res = Dict::default();
            res.set("ExtGState", Obj::Dict(eg));
            dict.set("Resources", Obj::Dict(res));
        }
        Some(Obj::Stream(dict, Arc::from(s.into_bytes())))
    }
}

impl R<'_, '_> {
    /// A free text annotation's appearance as pdfium makes it: the box in C
    /// with a black border, the text in Helvetica centred on its height.
    /// The size and colour (an operator) of the text DA gives.
    fn da_text(&self, ad: &Dict) -> (f64, String) {
        let f = self.f;
        let mut da = Obj::Null;
        let mut node = Some(ad.clone());
        for _ in 0..16 {
            let Some(d) = node else { break };
            da = f.key(&d, "DA");
            if !matches!(da, Obj::Null) {
                break;
            }
            node = f.key_dict(&d, "Parent");
        }
        let da = match da {
            Obj::Str(b) => String::from_utf8_lossy(&b).into_owned(),
            _ => String::new(),
        };
        let toks: Vec<&str> = da.split_whitespace().collect();
        let mut size = 12.0;
        let mut color = "0 g".to_string();
        for (i, t) in toks.iter().enumerate() {
            match *t {
                "Tf" if i >= 1 => size = toks[i - 1].parse().ok().filter(|s: &f64| *s > 0.0).unwrap_or(12.0),
                "g" if i >= 1 => color = toks[i - 1..=i].join(" "),
                "rg" if i >= 3 => color = toks[i - 3..=i].join(" "),
                "k" if i >= 4 => color = toks[i - 4..=i].join(" "),
                _ => {}
            }
        }
        (size, color)
    }

    /// A list box without an appearance, as pdfium's form filler draws
    /// it: rows from the top index (TI), the values (V) selected.
    /// An ink annotation's appearance as pdfium makes it: each path of
    /// InkList stroked in C (black) with the border's width, in the
    /// rectangle grown by half that width.
    fn ink_appearance(&self, ad: &Dict) -> Option<Obj> {
        use super::edit::appearance as ap;
        let f = self.f;
        let Obj::Array(paths) = f.key(ad, "InkList") else { return None };
        let r = f.nums(ad.get("Rect")?);
        if r.len() != 4 {
            return None;
        }
        let bs = f.key_dict(ad, "BS");
        let w = bs.as_ref().and_then(|b| f.key_num(b, "W")).or_else(|| f.nums(ad.get("Border").unwrap_or(&Obj::Null)).get(2).copied()).unwrap_or(1.0).max(0.0);
        let c = ad.get("C").map(|c| f.nums(c)).unwrap_or_else(|| vec![0.0]);
        let op = match c.len() {
            1 => "G",
            3 => "RG",
            4 => "K",
            _ => return None,
        };
        let nums: Vec<String> = c.iter().map(|v| format!("{v}")).collect();
        let mut s = format!("/GS0 gs\n{} {op}\n{w} w\n", nums.join(" "));
        if bs.as_ref().is_some_and(|b| f.key(b, "S").name() == Some("D")) {
            let d = bs.as_ref().map(|b| f.nums(b.get("D").unwrap_or(&Obj::Null))).filter(|d| !d.is_empty()).unwrap_or_else(|| vec![3.0]);
            let d: Vec<String> = d.iter().map(|v| format!("{v}")).collect();
            s.push_str(&format!("[{}] 0 d\n", d.join(" ")));
        }
        for p in paths.iter() {
            let p = f.nums(p);
            for (i, xy) in p.chunks_exact(2).enumerate() {
                s.push_str(&format!("{} {} {}\n", xy[0], xy[1], if i == 0 { "m" } else { "l" }));
            }
            if p.len() >= 2 {
                s.push_str("S\n");
            }
        }
        let h = w / 2.0;
        let mut dict = Dict::default();
        dict.set("BBox", Obj::Array(vec![Obj::Real(r[0].min(r[2]) - h), Obj::Real(r[1].min(r[3]) - h), Obj::Real(r[0].max(r[2]) + h), Obj::Real(r[1].max(r[3]) + h)]));
        dict.set("Resources", Obj::Dict(ap::resources(f.key_num(ad, "CA").filter(|a| *a < 1.0), Some("Normal"))));
        Some(Obj::Stream(dict, Arc::from(s.into_bytes())))
    }

    /// A text markup annotation's appearance as pdfium makes it, over each
    /// quadrilateral of QuadPoints in C: a highlight fills it (yellow,
    /// Multiply), an underline, strike out or squiggly strokes a 1pt line.
    fn markup_appearance(&self, ad: &Dict, kind: &str) -> Option<Obj> {
        use super::edit::appearance as ap;
        let f = self.f;
        let q = f.nums(ad.get("QuadPoints")?);
        if q.len() < 8 {
            return None;
        }
        let highlight = kind == "Highlight";
        let c = ad.get("C").map(|c| f.nums(c)).unwrap_or_else(|| if highlight { vec![1.0, 1.0, 0.0] } else { vec![0.0] });
        let op = match c.len() {
            1 => "g",
            3 => "rg",
            4 => "k",
            _ => return None,
        };
        let op = if highlight { op.to_string() } else { op.to_uppercase() };
        let nums: Vec<String> = c.iter().map(|v| format!("{v}")).collect();
        let mut s = format!("/GS0 gs\n{} {op}\n", nums.join(" "));
        if !highlight {
            s.push_str("1 w\n");
        }
        for p in q.chunks_exact(8) {
            let (l, r) = (p.iter().step_by(2).fold(f64::MAX, |a, &b| a.min(b)), p.iter().step_by(2).fold(f64::MIN, |a, &b| a.max(b)));
            let (b, t) = (p.iter().skip(1).step_by(2).fold(f64::MAX, |a, &b| a.min(b)), p.iter().skip(1).step_by(2).fold(f64::MIN, |a, &b| a.max(b)));
            match kind {
                "Highlight" => s.push_str(&format!("{l} {t} m {r} {t} l {r} {b} l {l} {b} l h f\n")),
                "Underline" => s.push_str(&format!("{l} {} m {r} {} l S\n", b + 1.0, b + 1.0)),
                "StrikeOut" => s.push_str(&format!("{l} {} m {r} {} l S\n", (b + t) / 2.0, (b + t) / 2.0)),
                _ => {
                    // A zigzag a sixth of the height tall, in 2pt steps.
                    let top = b + (t - b) / 6.0;
                    s.push_str(&format!("{l} {top} m"));
                    let mut x = l + 2.0;
                    let mut up = false;
                    while x < r {
                        s.push_str(&format!(" {x} {} l", if up { top } else { b }));
                        up = !up;
                        x += 2.0;
                    }
                    s.push_str(&format!(" {r} {} l S\n", if up { top } else { b }));
                }
            }
        }
        let alpha = f.key_num(ad, "CA").filter(|a| *a < 1.0);
        let xs = q.chunks_exact(2).map(|p| p[0]);
        let ys = q.chunks_exact(2).map(|p| p[1]);
        let (x0, x1) = (xs.clone().fold(f64::MAX, f64::min), xs.fold(f64::MIN, f64::max));
        let (y0, y1) = (ys.clone().fold(f64::MAX, f64::min), ys.fold(f64::MIN, f64::max));
        let mut dict = Dict::default();
        dict.set("BBox", Obj::Array(vec![Obj::Real(x0), Obj::Real(y0), Obj::Real(x1), Obj::Real(y1)]));
        dict.set("Resources", Obj::Dict(ap::resources(alpha, Some(if highlight { "Multiply" } else { "Normal" }))));
        Some(Obj::Stream(dict, Arc::from(s.into_bytes())))
    }

    fn list_box_appearance(&self, ad: &Dict) -> Option<Obj> {
        use super::edit::appearance as ap;
        let f = self.f;
        let field = |key: &str| -> Obj {
            let mut node = Some(ad.clone());
            for _ in 0..16 {
                let Some(d) = node else { break };
                let v = f.key(&d, key);
                if !matches!(v, Obj::Null) {
                    return v;
                }
                node = f.key_dict(&d, "Parent");
            }
            Obj::Null
        };
        if field("FT").name() != Some("Ch") || field("Ff").int().unwrap_or(0) & (1 << 17) != 0 {
            return None;
        }
        let r = f.nums(ad.get("Rect")?);
        if r.len() != 4 {
            return None;
        }
        let (x0, y0, x1, y1) = (r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3]));
        let text_of = |o: &Obj| match f.resolve(o) {
            Obj::Str(b) => Some(super::text_string(&b)),
            _ => None,
        };
        // (export value, shown text) of each option.
        let opts: Vec<(String, String)> = match field("Opt") {
            Obj::Array(a) => a
                .iter()
                .filter_map(|o| match f.resolve(o) {
                    Obj::Array(p) if p.len() >= 2 => Some((text_of(&p[0])?, text_of(&p[1])?)),
                    o => text_of(&o).map(|t| (t.clone(), t)),
                })
                .collect(),
            _ => return None,
        };
        let values: Vec<String> = match field("V") {
            Obj::Array(a) => a.iter().filter_map(text_of).collect(),
            o => text_of(&o).into_iter().collect(),
        };
        let (size, color) = self.da_text(ad);
        // Rows as tall as pdfium's sans from ascent to descent (0.9 and
        // 0.225), inside a 1 point border.
        let row = size * 1.125;
        let top = f.key_num(ad, "TI").unwrap_or(0.0).max(0.0) as usize;
        let mut s = String::new();
        let mut y = y1 - 1.0;
        for (exp, shown) in opts.iter().skip(top) {
            if y <= y0 {
                break;
            }
            let selected = values.iter().any(|v| v == exp);
            if selected {
                s.push_str(&format!("0 0.2 0.443 rg {} {} {} {} re f\n", ap::n(x0 + 1.0), ap::n(y - row), ap::n(x1 - x0 - 2.0), ap::n(row)));
            }
            let text: String = shown.chars().filter(|&c| c == '?' || ap::win_ansi(c) != b'?').collect();
            s.push_str(&format!("BT /{} {} Tf {}\n", ap::HELV, ap::n(size), if selected { "1 g" } else { &color }));
            s.push_str(&format!("1 0 0 1 {} {} Tm {} Tj ET\n", ap::n(x0 + 1.0), ap::n(y - size * 0.9), ap::string(&text)));
            y -= row;
        }
        let mut dict = Dict::default();
        dict.set("BBox", Obj::Array(vec![Obj::Real(x0), Obj::Real(y0), Obj::Real(x1), Obj::Real(y1)]));
        dict.set("Resources", Obj::Dict(ap::resources(None, None)));
        Some(Obj::Stream(dict, Arc::from(s.into_bytes())))
    }

    fn free_text_appearance(&self, ad: &Dict) -> Option<Obj> {
        use super::edit::appearance as ap;
        let f = self.f;
        let r = f.nums(ad.get("Rect")?);
        if r.len() != 4 {
            return None;
        }
        let (x0, y0, x1, y1) = (r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3]));
        let (size, color) = self.da_text(ad);
        let w = 1.0;
        let mut s = String::new();
        let c = f.nums(ad.get("C").unwrap_or(&Obj::Null));
        let fill = match c.len() {
            1 => Some(format!("{} g", c[0])),
            3 => Some(format!("{} {} {} rg", c[0], c[1], c[2])),
            4 => Some(format!("{} {} {} {} k", c[0], c[1], c[2], c[3])),
            _ => None,
        };
        if let Some(fill) = &fill {
            s.push_str(&format!("{fill}\n{x0} {y0} {} {} re f\n", x1 - x0, y1 - y0));
        }
        s.push_str(&format!("0 G {w} w\n{} {} {} {} re S\n", x0 + w / 2.0, y0 + w / 2.0, x1 - x0 - w, y1 - y0 - w));
        // One line, without the characters Helvetica hasn't.
        let text: String = match f.key(ad, "Contents") {
            Obj::Str(b) => super::text_string(&b).chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|&c| c == '?' || ap::win_ansi(c) != b'?').collect(),
            _ => String::new(),
        };
        let y = (y0 + y1) / 2.0 - size * 0.718 / 2.0;
        s.push_str(&format!("BT /{} {} Tf {color}\n", ap::HELV, ap::n(size)));
        s.push_str(&format!("1 0 0 1 {} {} Tm {} Tj\n", ap::n(x0 + w), ap::n(y), ap::string(&text)));
        s.push_str("ET\n");
        let ca = f.key_num(ad, "CA").filter(|a| *a < 1.0);
        if ca == Some(0.0) {
            return None;
        }
        if ca.is_some() {
            s.insert_str(0, "/GS0 gs\n");
        }
        let mut dict = Dict::default();
        dict.set("BBox", Obj::Array(vec![Obj::Real(x0), Obj::Real(y0), Obj::Real(x1), Obj::Real(y1)]));
        dict.set("Resources", Obj::Dict(ap::resources(ca, None)));
        Some(Obj::Stream(dict, Arc::from(s.into_bytes())))
    }
}

/// A pixmap of RGBA pixels (not premultiplied).
fn pixmap(w: usize, h: usize, rgba: &[u8]) -> Pixmap {
    use vello_cpu::color::PremulRgba8;
    let mut transparent = false;
    let px: Vec<PremulRgba8> = rgba
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as u16;
            transparent |= a != 255;
            let m = |c: u8| ((c as u16 * a + 127) / 255) as u8;
            PremulRgba8 { r: m(p[0]), g: m(p[1]), b: m(p[2]), a: p[3] }
        })
        .collect();
    let mut pm = Pixmap::from_parts(px, w as u16, h as u16);
    pm.set_may_have_transparency(transparent);
    pm
}
