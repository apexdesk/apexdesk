//! Windows metafiles (EMF and WMF), the pictures Word keeps for the
//! previews of objects (equations, Visio drawings, charts) and for old clip
//! art: drawn to pixels by the GDI records they hold. EMF+ records are left
//! aside: a dual file is drawn by its GDI copy, and a file of EMF+ records
//! only is not drawn.

mod emf;
mod wmf;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use vello_cpu::color::{AlphaColor, PremulRgba8, Srgb};
use vello_cpu::kurbo::{Affine, Arc as KArc, BezPath, Cap, Join, PathEl, Point, Rect, Shape, Stroke, Vec2};
use vello_cpu::peniko::{Extend, Fill, ImageQuality, ImageSampler};
use vello_cpu::{Image, ImageSource, PaintType, Pixmap, RenderContext};

pub use crate::pdf::render::Fallback;
use crate::pdf::Style;

/// A metafile drawn: RGBA8, not premultiplied, row major; transparent
/// where nothing was drawn.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Records one file may run: past that the rest is left out.
const MAX_RECORDS: usize = 2_000_000;
/// Points the paths of one file may hold, all told.
const MAX_POINTS: usize = 20_000_000;
/// Pixels of the bitmaps one file may draw, all told.
const MAX_BITMAP_PIXELS: u64 = 200_000_000;
/// Glyphs one file may draw.
const MAX_GLYPHS: usize = 2_000_000;
/// Clips a drawing may stack at once.
const MAX_CLIPS: usize = 64;
/// Longest side of the pixels drawn.
const MAX_SIDE: f64 = 4000.0;

/// Whether the bytes are an EMF or WMF file [`render`] draws.
pub fn drawable(data: &[u8]) -> bool {
    if emf::is_emf(data) {
        emf::header(data).is_some() && !emf::plus_only(data)
    } else {
        wmf::header(data).is_some()
    }
}

/// The size the metafile asks for, in points.
pub fn size(data: &[u8]) -> Option<(f64, f64)> {
    let h = if emf::is_emf(data) { emf::header(data)? } else { wmf::header(data)? };
    Some(h.size_pt)
}

/// Draws a metafile `scale` pixels to the point (at most 4000 pixels a
/// side). `fallback` gives the system fonts for its text. `None` when it
/// isn't one, is broken or draws by EMF+ records only.
pub fn render(data: &[u8], scale: f64, fallback: Fallback) -> Option<Picture> {
    let emf = emf::is_emf(data);
    let h = if emf { emf::header(data)? } else { wmf::header(data)? };
    let (wpt, hpt) = h.size_pt;
    if !(wpt > 0.0 && hpt > 0.0 && wpt.is_finite() && hpt.is_finite() && scale > 0.0) {
        return None;
    }
    let s = scale.min(MAX_SIDE / wpt.max(hpt));
    let (w, hh) = ((wpt * s).round().clamp(1.0, MAX_SIDE) as u16, (hpt * s).round().clamp(1.0, MAX_SIDE) as u16);
    let f = h.frame;
    if f.width() == 0.0 || f.height() == 0.0 {
        return None;
    }
    let base = Affine::scale_non_uniform(w as f64 / f.width(), hh as f64 / f.height()) * Affine::translate((-f.x0, -f.y0));
    let mut g = Gdi::new(w, hh, base, h.px_per_mm, fallback, !emf);
    g.wmf_box = f;
    if emf {
        emf::play(data, &mut g)?;
    } else {
        wmf::play(data, &mut g);
    }
    Some(g.finish())
}

/// What a header tells: the box the records draw in, in the units that are
/// mapped to the picture (EMF: the reference device's pixels; WMF: logical
/// units), and the size in points.
struct Header {
    frame: Rect,
    size_pt: (f64, f64),
    /// Pixels of the reference device to the millimeter (EMF).
    px_per_mm: (f64, f64),
}

/// A GDI object.
#[derive(Clone)]
enum Obj {
    Pen(Pen),
    Brush(Brush),
    Font(Rc<LogFont>),
    Other,
}

#[derive(Clone)]
struct Pen {
    style: u32,
    /// Logical units; 0: one device pixel.
    width: f64,
    color: [u8; 3],
    /// PS_USERSTYLE dashes, in logical units.
    dashes: Vec<f64>,
}

impl Pen {
    fn null(&self) -> bool {
        self.style & 0xF == 5
    }
}

#[derive(Clone)]
enum Brush {
    Null,
    Solid([u8; 3]),
    /// A hatch style (0-5) in a color.
    Hatch(u32, [u8; 3]),
    Pattern(Arc<Pixmap>),
}

#[derive(Clone, Default)]
struct LogFont {
    height: f64,
    width: f64,
    /// Tenths of a degree.
    escapement: f64,
    weight: i32,
    italic: bool,
    underline: bool,
    strike: bool,
    charset: u8,
    pitch_family: u8,
    face: String,
}

/// The device context: what the records set and SaveDC keeps.
#[derive(Clone)]
struct Dc {
    pen: Pen,
    brush: Brush,
    font: Rc<LogFont>,
    text_color: [u8; 3],
    bk_color: [u8; 3],
    bk_opaque: bool,
    text_align: u32,
    winding: bool,
    map_mode: u32,
    win_org: Vec2,
    win_ext: Vec2,
    vp_org: Vec2,
    vp_ext: Vec2,
    /// Whether the window extent was set (WMF).
    win_set: bool,
    world: Affine,
    cur: Point,
    clockwise: bool,
    miter: f64,
    /// Paths that clip, in pixels (all of them at once).
    clip: Rc<Vec<(BezPath, Fill)>>,
    clip_id: u64,
}

impl Dc {
    fn new() -> Dc {
        Dc {
            pen: Pen { style: 0, width: 0.0, color: [0; 3], dashes: Vec::new() },
            brush: Brush::Solid([255; 3]),
            font: Rc::new(LogFont { height: -12.0, weight: 400, face: "Arial".into(), ..Default::default() }),
            text_color: [0; 3],
            bk_color: [255; 3],
            bk_opaque: true,
            text_align: 0,
            winding: false,
            map_mode: 1,
            win_org: Vec2::ZERO,
            win_ext: Vec2::new(1.0, 1.0),
            vp_org: Vec2::ZERO,
            vp_ext: Vec2::new(1.0, 1.0),
            win_set: false,
            world: Affine::IDENTITY,
            cur: Point::ZERO,
            clockwise: false,
            miter: 10.0,
            clip: Rc::new(Vec::new()),
            clip_id: 0,
        }
    }
}

/// A font of the system, loaded.
struct Face {
    data: Arc<[u8]>,
    index: u32,
    upem: f64,
    /// Ascent and descent of the cell (Windows metrics), font units.
    ascent: f64,
    descent: f64,
    avg_width: f64,
    glyphs: HashMap<u32, Option<Rc<BezPath>>>,
}

impl Face {
    fn load(data: Arc<[u8]>, index: u32) -> Option<Face> {
        let f = FontRef::from_index(&data, index).ok()?;
        let upem = f.head().ok()?.units_per_em() as f64;
        let (mut ascent, mut descent, mut avg) = (upem * 0.9, upem * 0.25, upem * 0.5);
        if let Ok(os2) = f.os2() {
            ascent = os2.us_win_ascent() as f64;
            descent = os2.us_win_descent() as f64;
            avg = os2.x_avg_char_width() as f64;
        } else if let Ok(hh) = f.hhea() {
            ascent = hh.ascender().to_i16() as f64;
            descent = -(hh.descender().to_i16() as f64);
        }
        if upem <= 0.0 || ascent + descent <= 0.0 {
            return None;
        }
        Some(Face { data, index, upem, ascent, descent, avg_width: avg.max(1.0), glyphs: HashMap::new() })
    }

    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.data, self.index).ok()
    }

    fn gid(&self, ch: char) -> u32 {
        let Some(f) = self.font() else { return 0 };
        let cm = f.charmap();
        let g = cm.map(ch).map_or(0, |g| g.to_u32());
        // Symbol fonts map their codes from U+F000.
        if g == 0 && (ch as u32) < 0x100 {
            return cm.map(0xF000 + ch as u32).map_or(0, |g| g.to_u32());
        }
        g
    }

    fn advance(&self, gid: u32) -> f64 {
        let Some(f) = self.font() else { return 0.0 };
        f.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(GlyphId::new(gid)).unwrap_or(0.0) as f64
    }

    fn outline(&mut self, gid: u32) -> Option<Rc<BezPath>> {
        if let Some(p) = self.glyphs.get(&gid) {
            return p.clone();
        }
        let p = (|| {
            let f = self.font()?;
            let g = f.outline_glyphs().get(GlyphId::new(gid))?;
            let mut pen = Pen2(BezPath::new());
            g.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
            Some(Rc::new(pen.0))
        })();
        self.glyphs.insert(gid, p.clone());
        p
    }
}

struct Pen2(BezPath);

impl OutlinePen for Pen2 {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((cx0 as f64, cy0 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// Raster operations of the bitmap records that are drawn.
const SRCCOPY: u32 = 0x00CC_0020;
const SRCAND: u32 = 0x0088_00C6;
const SRCPAINT: u32 = 0x00EE_0086;
const SRCINVERT: u32 = 0x0066_0046;
const PATCOPY: u32 = 0x00F0_0021;
const PATINVERT: u32 = 0x005A_0049;
const BLACKNESS: u32 = 0x0000_0042;
const WHITENESS: u32 = 0x00FF_0062;

/// Draws the records of both kinds of metafile.
struct Gdi<'f> {
    ctx: RenderContext,
    resources: vello_cpu::Resources,
    width: u16,
    height: u16,
    /// The reference device (EMF) or the window's box (WMF) to pixels.
    base: Affine,
    px_per_mm: (f64, f64),
    /// WMF: the window is mapped onto `wmf_box` (the placeable header's).
    wmf: bool,
    wmf_box: Rect,
    fallback: Fallback<'f>,
    objects: Vec<Option<Obj>>,
    dc: Dc,
    saved: Vec<Dc>,
    /// Inside BeginPath: the path so far, in pixels.
    path: Option<BezPath>,
    /// After EndPath.
    done_path: Option<BezPath>,
    widened: bool,
    /// The clip pushed on the context (its id and how many paths).
    clip_applied: (u64, usize),
    next_clip: u64,
    faces: HashMap<(String, bool, bool), Option<Rc<std::cell::RefCell<Face>>>>,
    points: usize,
    glyphs: usize,
    bitmap_pixels: u64,
}

impl<'f> Gdi<'f> {
    fn new(width: u16, height: u16, base: Affine, px_per_mm: (f64, f64), fallback: Fallback<'f>, wmf: bool) -> Gdi<'f> {
        let ctx = RenderContext::new_with(width, height, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
        Gdi {
            ctx,
            resources: vello_cpu::Resources::new(),
            width,
            height,
            base,
            px_per_mm,
            wmf,
            wmf_box: Rect::ZERO,
            fallback,
            objects: Vec::new(),
            dc: Dc::new(),
            saved: Vec::new(),
            path: None,
            done_path: None,
            widened: false,
            clip_applied: (0, 0),
            next_clip: 1,
            faces: HashMap::new(),
            points: 0,
            glyphs: 0,
            bitmap_pixels: 0,
        }
    }

    fn finish(mut self) -> Picture {
        for _ in 0..self.clip_applied.1 {
            self.ctx.pop_clip_path();
        }
        let mut pix = Pixmap::new(self.width, self.height);
        self.ctx.flush();
        self.ctx.render(&mut pix, &mut self.resources);
        let mut rgba = pix.data_as_u8_slice().to_vec();
        for p in rgba.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a != 0 && a != 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Picture { width: self.width as u32, height: self.height as u32, rgba }
    }

    // --- Coordinates ---------------------------------------------------

    /// Logical units to the reference device (EMF) or the window's box (WMF).
    fn page(&self) -> Affine {
        let d = &self.dc;
        if self.wmf {
            if !d.win_set || d.win_ext.x == 0.0 || d.win_ext.y == 0.0 {
                return Affine::IDENTITY;
            }
            let b = self.wmf_box;
            return Affine::translate((b.x0, b.y0)) * Affine::scale_non_uniform(b.width() / d.win_ext.x, b.height() / d.win_ext.y) * Affine::translate(-d.win_org);
        }
        let (sx, sy) = match d.map_mode {
            7 | 8 => {
                let (mut sx, mut sy) = (d.vp_ext.x / d.win_ext.x, d.vp_ext.y / d.win_ext.y);
                if !sx.is_finite() || !sy.is_finite() {
                    (sx, sy) = (1.0, 1.0);
                }
                if d.map_mode == 7 {
                    let m = sx.abs().min(sy.abs());
                    (sx, sy) = (m.copysign(sx), m.copysign(sy));
                }
                (sx, sy)
            }
            2..=6 => {
                let mm = match d.map_mode {
                    2 => 0.1,
                    3 => 0.01,
                    4 => 0.254,
                    5 => 0.0254,
                    _ => 25.4 / 1440.0,
                };
                (mm * self.px_per_mm.0, -mm * self.px_per_mm.1)
            }
            _ => (1.0, 1.0),
        };
        Affine::translate(d.vp_org) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-d.win_org)
    }

    /// Logical units to pixels.
    fn full(&self) -> Affine {
        self.base * self.page() * self.dc.world
    }

    /// The pixels of one unit of the reference device (a cosmetic pen).
    fn device_px(&self) -> f64 {
        let c = self.base.as_coeffs();
        (c[0].hypot(c[1]) * c[2].hypot(c[3])).sqrt().max(0.5)
    }

    // --- Objects -------------------------------------------------------

    /// EMF: puts an object at its index.
    fn put(&mut self, i: u32, o: Obj) {
        let i = i as usize;
        if i >= 65_536 {
            return;
        }
        if self.objects.len() <= i {
            self.objects.resize(i + 1, None);
        }
        self.objects[i] = Some(o);
    }

    /// WMF: puts an object in the first free slot.
    fn add(&mut self, o: Obj) {
        match self.objects.iter().position(Option::is_none) {
            Some(i) => self.objects[i] = Some(o),
            None if self.objects.len() < 65_536 => self.objects.push(Some(o)),
            None => {}
        }
    }

    fn delete(&mut self, i: u32) {
        if let Some(o) = self.objects.get_mut(i as usize) {
            *o = None;
        }
    }

    fn select(&mut self, i: u32) {
        if i & 0x8000_0000 != 0 {
            return self.select_stock(i & 0x7FFF_FFFF);
        }
        match self.objects.get(i as usize).cloned().flatten() {
            Some(Obj::Pen(p)) => self.dc.pen = p,
            Some(Obj::Brush(b)) => self.dc.brush = b,
            Some(Obj::Font(f)) => self.dc.font = f,
            _ => {}
        }
    }

    fn select_stock(&mut self, i: u32) {
        let gray = |v: u8| Brush::Solid([v; 3]);
        let pen = |c: u8, style: u32| Pen { style, width: 0.0, color: [c; 3], dashes: Vec::new() };
        match i {
            0 => self.dc.brush = gray(255),
            1 => self.dc.brush = gray(192),
            2 => self.dc.brush = gray(128),
            3 => self.dc.brush = gray(64),
            4 => self.dc.brush = gray(0),
            5 => self.dc.brush = Brush::Null,
            6 => self.dc.pen = pen(255, 0),
            7 => self.dc.pen = pen(0, 0),
            8 => self.dc.pen = pen(0, 5),
            10..=17 => {
                let face = if i == 11 || i == 10 { "Courier New" } else { "Arial" };
                self.dc.font = Rc::new(LogFont { height: -12.0, weight: 400, face: face.into(), ..Default::default() });
            }
            18 => self.dc.brush = gray(255),
            19 => self.dc.pen = pen(0, 0),
            _ => {}
        }
    }

    fn save(&mut self) {
        if self.saved.len() < 1024 {
            self.saved.push(self.dc.clone());
        }
    }

    /// RestoreDC: `n` < 0 goes back that many saves; > 0 to that save.
    fn restore(&mut self, n: i32) {
        let k = if n < 0 { self.saved.len().checked_sub((-(n as i64)) as usize) } else { (n as usize).checked_sub(1) };
        if let Some(k) = k.filter(|&k| k < self.saved.len()) {
            self.dc = self.saved[k].clone();
            self.saved.truncate(k);
        }
    }

    // --- Clipping ------------------------------------------------------

    fn set_clip(&mut self, clip: Vec<(BezPath, Fill)>) {
        self.dc.clip = Rc::new(clip);
        self.dc.clip_id = self.next_clip;
        self.next_clip += 1;
    }

    fn add_clip(&mut self, p: BezPath, rule: Fill) {
        let mut c = (*self.dc.clip).clone();
        if c.len() < MAX_CLIPS {
            c.push((p, rule));
        }
        self.set_clip(c);
    }

    /// IntersectClipRect: logical units.
    fn intersect_rect(&mut self, r: Rect) {
        let p = self.full() * r.to_path(0.1);
        self.add_clip(p, Fill::NonZero);
    }

    /// ExcludeClipRect: logical units.
    fn exclude_rect(&mut self, r: Rect) {
        let mut p = Rect::new(-1e7, -1e7, 1e7, 1e7).to_path(0.1);
        p.extend(self.full() * r.to_path(0.1));
        self.add_clip(p, Fill::EvenOdd);
    }

    /// A region's rectangles (in reference device units) with a mode
    /// (1 AND, 4 DIFF, 5 COPY).
    fn clip_region(&mut self, mode: u32, rects: &[Rect]) {
        let mut p = BezPath::new();
        for r in rects {
            p.extend(self.base * r.to_path(0.1));
        }
        match mode {
            5 if rects.is_empty() => self.set_clip(Vec::new()),
            5 => self.set_clip(vec![(p, Fill::NonZero)]),
            1 => self.add_clip(p, Fill::NonZero),
            4 => {
                let mut q = Rect::new(-1e7, -1e7, 1e7, 1e7).to_path(0.1);
                q.extend(p);
                self.add_clip(q, Fill::EvenOdd);
            }
            _ => {}
        }
    }

    /// SelectClipPath with a mode.
    fn clip_path(&mut self, mode: u32) {
        let Some(p) = self.done_path.take() else { return };
        let rule = if self.dc.winding { Fill::NonZero } else { Fill::EvenOdd };
        match mode {
            5 => self.set_clip(vec![(p, rule)]),
            1 => self.add_clip(p, rule),
            _ => {}
        }
    }

    /// Brings the context's clip to the device context's.
    fn sync_clip(&mut self) {
        if self.clip_applied.0 == self.dc.clip_id {
            return;
        }
        for _ in 0..self.clip_applied.1 {
            self.ctx.pop_clip_path();
        }
        self.ctx.set_transform(Affine::IDENTITY);
        for (p, rule) in self.dc.clip.iter() {
            self.ctx.set_fill_rule(*rule);
            self.ctx.push_clip_path(p);
        }
        self.clip_applied = (self.dc.clip_id, self.dc.clip.len());
    }

    // --- Paths ---------------------------------------------------------

    fn count(&mut self, n: usize) -> bool {
        self.points = self.points.saturating_add(n);
        self.points <= MAX_POINTS
    }

    /// Adds a figure (in pixels) to the open path, or draws it.
    fn figure(&mut self, p: BezPath, fill: bool) {
        if let Some(open) = &mut self.path {
            open.extend(p);
            return;
        }
        if fill {
            self.fill(&p);
        }
        self.stroke(&p);
    }

    /// A figure of logical units.
    fn shape(&mut self, s: impl Shape, fill: bool) {
        let p = self.full() * s.to_path(0.1);
        self.figure(p, fill);
    }

    fn polygon(&mut self, pts: &[Point], close: bool) {
        if pts.is_empty() || !self.count(pts.len()) {
            return;
        }
        let t = self.full();
        let mut p = BezPath::new();
        p.move_to(t * pts[0]);
        for q in &pts[1..] {
            p.line_to(t * *q);
        }
        if close {
            p.close_path();
        }
        self.figure(p, close);
    }

    fn poly_polygon(&mut self, polys: &[Vec<Point>], close: bool) {
        let n = polys.iter().map(Vec::len).sum();
        if !self.count(n) {
            return;
        }
        let t = self.full();
        let mut p = BezPath::new();
        for poly in polys.iter().filter(|p| !p.is_empty()) {
            p.move_to(t * poly[0]);
            for q in &poly[1..] {
                p.line_to(t * *q);
            }
            if close {
                p.close_path();
            }
        }
        self.figure(p, close);
    }

    /// PolyBezier (from the first point) or PolyBezierTo (from the current
    /// position).
    fn bezier(&mut self, pts: &[Point], to: bool) {
        if !self.count(pts.len()) {
            return;
        }
        let mut all = Vec::with_capacity(pts.len() + 1);
        if to {
            all.push(self.dc.cur);
        }
        all.extend_from_slice(pts);
        if all.len() < 4 {
            return;
        }
        let t = self.full();
        if let Some(open) = &mut self.path {
            if !to || open.elements().is_empty() {
                open.move_to(t * all[0]);
            }
            for c in all[1..].chunks_exact(3) {
                open.curve_to(t * c[0], t * c[1], t * c[2]);
            }
        } else {
            let mut p = BezPath::new();
            p.move_to(t * all[0]);
            for c in all[1..].chunks_exact(3) {
                p.curve_to(t * c[0], t * c[1], t * c[2]);
            }
            self.stroke(&p);
        }
        if to {
            self.dc.cur = *all.last().unwrap_or(&self.dc.cur);
        }
    }

    /// PolylineTo.
    fn polyline_to(&mut self, pts: &[Point]) {
        for &q in pts {
            self.line_to(q);
        }
    }

    fn move_to(&mut self, q: Point) {
        self.dc.cur = q;
        let t = self.full();
        if let Some(open) = &mut self.path {
            open.move_to(t * q);
        }
    }

    fn line_to(&mut self, q: Point) {
        if !self.count(1) {
            return;
        }
        let t = self.full();
        let from = self.dc.cur;
        self.dc.cur = q;
        if let Some(open) = &mut self.path {
            if open.elements().is_empty() || matches!(open.elements().last(), Some(PathEl::ClosePath)) {
                open.move_to(t * from);
            }
            open.line_to(t * q);
        } else {
            let mut p = BezPath::new();
            p.move_to(t * from);
            p.line_to(t * q);
            self.stroke(&p);
        }
    }

    /// PolyDraw: points with their kinds (move 6, line 2, bezier 4; bit 1
    /// closes the figure).
    fn poly_draw(&mut self, pts: &[Point], kinds: &[u8]) {
        let mut i = 0;
        while i < pts.len().min(kinds.len()) {
            let k = kinds[i];
            match k & !1 {
                6 => self.move_to(pts[i]),
                2 => self.line_to(pts[i]),
                4 if i + 2 < pts.len() => {
                    self.bezier(&pts[i..i + 3], true);
                    i += 2;
                }
                _ => {}
            }
            if kinds.get(i).is_some_and(|k| k & 1 != 0) {
                self.close_figure();
            }
            i += 1;
        }
    }

    /// The ellipse in `r` from the angle of `start` to that of `end`
    /// (logical units), in the arc direction.
    fn arc_path(&self, r: Rect, start: Point, end: Point) -> KArc {
        let c = r.center();
        let (rx, ry) = (r.width().abs() / 2.0, r.height().abs() / 2.0);
        let ang = |p: Point| ((p.y - c.y) / ry.max(1e-9)).atan2((p.x - c.x) / rx.max(1e-9));
        let (a0, a1) = (ang(start), ang(end));
        // Counterclockwise as seen: with y down (the usual), the angle
        // falls.
        let y_down = self.full().determinant() > 0.0;
        let ccw_falls = y_down != self.dc.clockwise;
        let tau = std::f64::consts::TAU;
        let mut sweep = a1 - a0;
        if ccw_falls {
            while sweep >= 0.0 {
                sweep -= tau;
            }
        } else {
            while sweep <= 0.0 {
                sweep += tau;
            }
        }
        if sweep.abs() > tau || !sweep.is_finite() {
            sweep = tau.copysign(sweep);
        }
        KArc::new(c, (rx, ry), a0, sweep, 0.0)
    }

    /// Arc (0), Chord (1) or Pie (2), or ArcTo (3).
    fn arc(&mut self, r: Rect, start: Point, end: Point, kind: u8) {
        let a = self.arc_path(r, start, end);
        let t = self.full();
        let mut p = BezPath::new();
        let first = a.center + Vec2::new(a.radii.x * a.start_angle.cos(), a.radii.y * a.start_angle.sin());
        match kind {
            3 => {
                if self.path.is_some() {
                    self.line_to(first);
                } else {
                    p.move_to(t * self.dc.cur);
                    p.line_to(t * first);
                }
            }
            2 => {
                p.move_to(t * a.center);
                p.line_to(t * first);
            }
            _ => p.move_to(t * first),
        }
        a.append_iter(0.1).for_each(|el| p.push(t * el));
        let last = a.center + Vec2::new(a.radii.x * (a.start_angle + a.sweep_angle).cos(), a.radii.y * (a.start_angle + a.sweep_angle).sin());
        match kind {
            1 | 2 => {
                if kind == 2 {
                    p.line_to(t * a.center);
                }
                p.close_path();
                self.figure(p, true);
            }
            3 => {
                self.dc.cur = last;
                if let Some(open) = &mut self.path {
                    open.extend(p.elements().iter().copied().filter(|e| !matches!(e, PathEl::MoveTo(_))));
                } else {
                    self.stroke(&p);
                }
            }
            _ => self.figure(p, false),
        }
    }

    /// AngleArc: a line to the arc's start, then the arc (counterclockwise
    /// for a positive sweep), degrees.
    fn angle_arc(&mut self, c: Point, radius: f64, start: f64, sweep: f64) {
        let (s, w) = (start.to_radians(), sweep.to_radians());
        let y_down = self.full().determinant() > 0.0;
        let k = if y_down { -1.0 } else { 1.0 };
        let a = KArc::new(c, (radius, radius), k * s, k * w, 0.0);
        let first = c + Vec2::new(radius * (k * s).cos(), radius * (k * s).sin());
        self.line_to(first);
        let t = self.full();
        let mut p = BezPath::new();
        p.move_to(t * first);
        a.append_iter(0.1).for_each(|el| p.push(t * el));
        self.dc.cur = c + Vec2::new(radius * (k * (s + w)).cos(), radius * (k * (s + w)).sin());
        if let Some(open) = &mut self.path {
            open.extend(p.elements().iter().copied().skip(1));
        } else {
            self.stroke(&p);
        }
    }

    fn round_rect(&mut self, r: Rect, corner: Vec2) {
        let r = r.abs();
        let (rx, ry) = ((corner.x.abs() / 2.0).min(r.width() / 2.0), (corner.y.abs() / 2.0).min(r.height() / 2.0));
        if rx <= 0.0 || ry <= 0.0 {
            return self.shape(r, true);
        }
        let mut p = BezPath::new();
        let h = std::f64::consts::FRAC_PI_2;
        let corners = [(Point::new(r.x1 - rx, r.y0 + ry), -h), (Point::new(r.x1 - rx, r.y1 - ry), 0.0), (Point::new(r.x0 + rx, r.y1 - ry), h), (Point::new(r.x0 + rx, r.y0 + ry), 2.0 * h)];
        p.move_to((r.x0 + rx, r.y0));
        for (c, a0) in corners {
            let a = KArc::new(c, (rx, ry), a0, h, 0.0);
            p.line_to(c + Vec2::new(rx * a0.cos(), ry * a0.sin()));
            a.append_iter(0.1).for_each(|el| p.push(el));
        }
        p.close_path();
        let p = self.full() * p;
        self.figure(p, true);
    }

    fn begin_path(&mut self) {
        self.path = Some(BezPath::new());
        self.done_path = None;
        self.widened = false;
    }

    fn end_path(&mut self) {
        self.done_path = self.path.take();
    }

    fn close_figure(&mut self) {
        if let Some(open) = &mut self.path {
            open.close_path();
        }
    }

    /// FillPath (1), StrokePath (2), StrokeAndFillPath (3).
    fn draw_path(&mut self, how: u8) {
        let p = self.done_path.take().or_else(|| self.path.take());
        let Some(p) = p else { return };
        // A widened path is filled as its outline would be: stroked.
        if self.widened && how == 1 {
            let brush = self.dc.brush.clone();
            if let Brush::Solid(c) = brush {
                let pen = Pen { color: c, ..self.dc.pen.clone() };
                let old = std::mem::replace(&mut self.dc.pen, pen);
                self.stroke(&p);
                self.dc.pen = old;
            }
            return;
        }
        if how & 1 != 0 {
            self.fill(&p);
        }
        if how & 2 != 0 {
            self.stroke(&p);
        }
    }

    // --- Painting ------------------------------------------------------

    fn color(c: [u8; 3]) -> AlphaColor<Srgb> {
        AlphaColor::from_rgba8(c[0], c[1], c[2], 255)
    }

    /// Fills a path (pixels) with the brush.
    fn fill(&mut self, p: &BezPath) {
        let brush = self.dc.brush.clone();
        let rule = if self.dc.winding { Fill::NonZero } else { Fill::EvenOdd };
        self.fill_with(p, &brush, rule);
    }

    fn fill_with(&mut self, p: &BezPath, brush: &Brush, rule: Fill) {
        self.sync_clip();
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_fill_rule(rule);
        match brush {
            Brush::Null => {}
            Brush::Solid(c) => {
                self.ctx.set_paint(Self::color(*c));
                self.ctx.fill_path(p);
            }
            Brush::Hatch(style, c) => {
                if self.dc.bk_opaque {
                    self.ctx.set_paint(Self::color(self.dc.bk_color));
                    self.ctx.fill_path(p);
                }
                let tile = hatch_tile(*style, *c, (8.0 * self.device_px()).round().clamp(4.0, 64.0) as u16);
                self.paint_pattern(p, Arc::new(tile), Affine::IDENTITY);
            }
            Brush::Pattern(pm) => {
                let k = self.device_px();
                self.paint_pattern(p, pm.clone(), Affine::scale(k));
            }
        }
    }

    fn paint_pattern(&mut self, p: &BezPath, pm: Arc<Pixmap>, t: Affine) {
        self.ctx.set_paint_transform(t);
        let sampler = ImageSampler { x_extend: Extend::Repeat, y_extend: Extend::Repeat, quality: ImageQuality::Low, alpha: 1.0 };
        self.ctx.set_paint(PaintType::Image(Image { image: ImageSource::Pixmap(pm), sampler }));
        self.ctx.fill_path(p);
        self.ctx.reset_paint_transform();
    }

    /// Strokes a path (pixels) with the pen.
    fn stroke(&mut self, p: &BezPath) {
        let pen = &self.dc.pen;
        if pen.null() {
            return;
        }
        let full = self.full();
        let c = full.as_coeffs();
        let k = (c[0].hypot(c[1]) * c[2].hypot(c[3])).sqrt();
        let geometric = pen.style & 0x0001_0000 != 0 || pen.width > 1.0;
        let w = if pen.width <= 0.0 || !geometric && pen.width <= 1.0 { self.device_px() } else { pen.width * k };
        let w = w.clamp(0.1, 1e5);
        let cap = match pen.style & 0xF00 {
            0x100 => Cap::Square,
            0x200 => Cap::Butt,
            _ if geometric => Cap::Round,
            _ => Cap::Butt,
        };
        let join = match pen.style & 0xF000 {
            0x1000 => Join::Bevel,
            0x2000 => Join::Miter,
            _ if geometric => Join::Round,
            _ => Join::Miter,
        };
        let unit = w.max(self.device_px());
        let dashes: Vec<f64> = match pen.style & 0xF {
            1 => vec![18.0, 6.0],
            2 => vec![3.0, 3.0],
            3 => vec![9.0, 6.0, 3.0, 6.0],
            4 => vec![9.0, 3.0, 3.0, 3.0, 3.0, 3.0],
            7 if !pen.dashes.is_empty() => pen.dashes.iter().map(|d| d * k / unit).collect(),
            _ => Vec::new(),
        };
        let dashes: Vec<f64> = dashes.into_iter().map(|d| (d * unit).max(0.1)).collect();
        let mut s = Stroke::new(w).with_caps(cap).with_join(join).with_miter_limit(self.dc.miter.max(1.0));
        if !dashes.is_empty() && dashes.len() % 2 == 0 && p.perimeter(1.0) / dashes.iter().sum::<f64>() < 100_000.0 {
            s = s.with_dashes(0.0, dashes);
        }
        let color = pen.color;
        self.sync_clip();
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_stroke(s);
        self.ctx.set_paint(Self::color(color));
        self.ctx.stroke_path(p);
    }

    /// Fills a logical rectangle with a color.
    fn fill_rect_color(&mut self, r: Rect, c: [u8; 3]) {
        let p = self.full() * r.to_path(0.1);
        self.fill_with(&p, &Brush::Solid(c), Fill::NonZero);
    }

    /// PatBlt and the bitmap records without a bitmap.
    fn pat_blt(&mut self, r: Rect, rop: u32) {
        match rop {
            PATCOPY | PATINVERT => {
                let b = self.dc.brush.clone();
                let p = self.full() * r.to_path(0.1);
                self.fill_with(&p, &b, Fill::NonZero);
            }
            BLACKNESS => self.fill_rect_color(r, [0; 3]),
            WHITENESS => self.fill_rect_color(r, [255; 3]),
            _ => {}
        }
    }

    /// Draws `src` (a part of a bitmap, in its pixels) into `dest`
    /// (logical units) with a raster operation.
    fn blit(&mut self, bmp: Bitmap, src: Rect, dest: Rect, rop: u32, alpha: Option<u8>) {
        if dest.width() == 0.0 || dest.height() == 0.0 || src.width() == 0.0 || src.height() == 0.0 {
            return;
        }
        self.bitmap_pixels = self.bitmap_pixels.saturating_add(bmp.width as u64 * bmp.height as u64);
        if self.bitmap_pixels > MAX_BITMAP_PIXELS {
            return;
        }
        let mut px = bmp.pixels;
        match rop {
            // A mask (white shows through) and the picture drawn over it
            // (black shows through), as in the transparent-picture trick.
            SRCAND => px.iter_mut().filter(|p| p.r == 255 && p.g == 255 && p.b == 255).for_each(|p| *p = PremulRgba8 { r: 0, g: 0, b: 0, a: 0 }),
            SRCPAINT | SRCINVERT => px.iter_mut().filter(|p| p.r == 0 && p.g == 0 && p.b == 0).for_each(|p| *p = PremulRgba8 { r: 0, g: 0, b: 0, a: 0 }),
            _ => {}
        }
        if let Some(a) = alpha.filter(|&a| a < 255) {
            for p in &mut px {
                let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
                *p = PremulRgba8 { r: m(p.r), g: m(p.g), b: m(p.b), a: m(p.a) };
            }
        }
        let pm = Arc::new(Pixmap::from_parts(px, bmp.width, bmp.height));
        let t = self.full() * Affine::translate((dest.x0, dest.y0)) * Affine::scale_non_uniform(dest.width() / src.width(), dest.height() / src.height()) * Affine::translate((-src.x0, -src.y0));
        self.sync_clip();
        self.ctx.set_transform(t);
        self.ctx.set_fill_rule(Fill::NonZero);
        let c = t.as_coeffs();
        let up = c[0].hypot(c[1]) >= 2.0;
        let quality = if up { ImageQuality::Medium } else { ImageQuality::High };
        let sampler = ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality, alpha: 1.0 };
        self.ctx.set_paint(PaintType::Image(Image { image: ImageSource::Pixmap(pm), sampler }));
        self.ctx.fill_rect(&src);
    }

    /// GradientFill: rectangles shaded across (mode 0) or down (1), or
    /// triangles (2, drawn in the mean of their colors).
    fn gradient(&mut self, verts: &[(Point, [f64; 4])], idx: &[Vec<u32>], mode: u32) {
        let t = self.full();
        for ix in idx {
            let v: Vec<_> = ix.iter().filter_map(|&i| verts.get(i as usize)).collect();
            if mode == 2 {
                if v.len() == 3 {
                    let mut c = [0.0; 3];
                    for q in &v {
                        for k in 0..3 {
                            c[k] += q.1[k] / 3.0;
                        }
                    }
                    let mut p = BezPath::new();
                    p.move_to(t * v[0].0);
                    p.line_to(t * v[1].0);
                    p.line_to(t * v[2].0);
                    p.close_path();
                    self.fill_with(&p, &Brush::Solid(c.map(|x| x.round() as u8)), Fill::NonZero);
                }
                continue;
            }
            if v.len() != 2 {
                continue;
            }
            let (a, b) = (v[0], v[1]);
            const STEPS: usize = 64;
            for s in 0..STEPS {
                let (f0, f1) = (s as f64 / STEPS as f64, (s + 1) as f64 / STEPS as f64);
                let m = (f0 + f1) / 2.0;
                let c = [0, 1, 2].map(|k| (a.1[k] + (b.1[k] - a.1[k]) * m).round() as u8);
                let r = if mode == 0 {
                    Rect::new(a.0.x + (b.0.x - a.0.x) * f0, a.0.y, a.0.x + (b.0.x - a.0.x) * f1, b.0.y)
                } else {
                    Rect::new(a.0.x, a.0.y + (b.0.y - a.0.y) * f0, b.0.x, a.0.y + (b.0.y - a.0.y) * f1)
                };
                let p = t * r.abs().to_path(0.1);
                self.fill_with(&p, &Brush::Solid(c), Fill::NonZero);
            }
        }
    }

    // --- Text ----------------------------------------------------------

    fn face(&mut self, f: &LogFont) -> Option<Rc<std::cell::RefCell<Face>>> {
        let bold = f.weight >= 600;
        let key = (f.face.to_lowercase(), bold, f.italic);
        if let Some(x) = self.faces.get(&key) {
            return x.clone();
        }
        let style = Style {
            family: Arc::from(f.face.as_str()),
            bold,
            italic: f.italic,
            mono: f.pitch_family & 3 == 1 || f.pitch_family & 0xF0 == 0x30,
            serif: f.pitch_family & 0xF0 == 0x10,
        };
        let face = (self.fallback)(&style, &f.face).and_then(|(d, i)| Face::load(d, i)).map(|f| Rc::new(std::cell::RefCell::new(f)));
        self.faces.insert(key, face.clone());
        face
    }

    /// ExtTextOut: `text` at `at` (logical units), with the advances `dx`
    /// (logical units, maybe none), options and their rectangle.
    fn text(&mut self, at: Point, text: &[char], glyph_ids: Option<&[u16]>, dx: &[f64], options: u32, rect: Option<Rect>) {
        let font = self.dc.font.clone();
        // ETO_OPAQUE fills the rectangle; ETO_CLIPPED clips to it.
        if let Some(r) = rect {
            if options & 2 != 0 {
                let bk = self.dc.bk_color;
                self.fill_rect_color(r, bk);
            }
        }
        let n = glyph_ids.map_or(text.len(), <[u16]>::len);
        if n == 0 {
            return;
        }
        self.glyphs += n;
        if self.glyphs > MAX_GLYPHS {
            return;
        }
        let Some(fr) = self.face(&font) else { return };
        // Characters the font lacks, from a font that has them.
        let missing = glyph_ids.is_none() && text.iter().any(|&c| !c.is_whitespace() && fr.borrow().gid(c) == 0);
        let fallback_face = if missing { self.face(&LogFont { face: String::new(), ..(*font).clone() }) } else { None };
        let mut face = fr.borrow_mut();
        let em = {
            let h = font.height.abs();
            if font.height > 0.0 {
                h * face.upem / (face.ascent + face.descent)
            } else if h > 0.0 {
                h
            } else {
                12.0
            }
        };
        let unit = em / face.upem;
        let xscale = if font.width > 0.0 { font.width / (face.avg_width * unit) } else { 1.0 };
        // Glyphs and their advances (logical units).
        let mut glyphs: Vec<(u32, bool, f64)> = Vec::with_capacity(n);
        for i in 0..n {
            let (gid, alt) = match glyph_ids {
                Some(g) => (g[i] as u32, false),
                None => {
                    let g = face.gid(text[i]);
                    match (&fallback_face, g) {
                        (Some(ff), 0) if !text[i].is_whitespace() => (ff.borrow().gid(text[i]), true),
                        _ => (g, false),
                    }
                }
            };
            let adv = match dx.get(i) {
                Some(&d) => d,
                None if alt => fallback_face.as_ref().map_or(0.0, |ff| {
                    let ff = ff.borrow();
                    ff.advance(gid) * em / ff.upem
                }) * xscale,
                None => face.advance(gid) * unit * xscale,
            };
            glyphs.push((gid, alt, adv));
        }
        let total: f64 = glyphs.iter().map(|g| g.2).sum();
        let align = self.dc.text_align;
        let update = align & 1 != 0;
        let origin = if update { self.dc.cur } else { at };
        let dxa = match align & 6 {
            2 => -total,
            6 => -total / 2.0,
            _ => 0.0,
        };
        let ascent = face.ascent * em / face.upem;
        let descent = face.descent * em / face.upem;
        let dya = match align & 24 {
            24 => 0.0,
            8 => -descent,
            _ => ascent,
        };
        // In the text's own frame: x along the baseline, y down, logical
        // units; turned by the escapement as seen.
        let full = self.full();
        let c = full.as_coeffs();
        let (sx, sy) = (c[0].hypot(c[1]), c[2].hypot(c[3]));
        let flip_x = c[0] < 0.0;
        let o = full * origin;
        let esc = font.escapement.to_radians() / 10.0;
        let frame = Affine::translate(o.to_vec2()) * Affine::rotate(-esc) * Affine::scale_non_uniform(if flip_x { -sx } else { sx }, sy);
        if update {
            self.dc.cur = origin + Vec2::new(total * if c[0] < 0.0 { -1.0 } else { 1.0 }, 0.0);
        }
        // Clip to the rectangle.
        let clipped = options & 4 != 0 && rect.is_some();
        let saved_clip = self.dc.clip.clone();
        let saved_id = self.dc.clip_id;
        if clipped {
            if let Some(r) = rect {
                self.intersect_rect(r);
            }
        }
        if self.dc.bk_opaque && options & 2 == 0 {
            let r = Rect::new(dxa, dya - ascent, dxa + total, dya + descent);
            let p = frame * r.to_path(0.1);
            let bk = self.dc.bk_color;
            self.fill_with(&p, &Brush::Solid(bk), Fill::NonZero);
        }
        let mut out = BezPath::new();
        let mut x = dxa;
        for &(gid, alt, adv) in &glyphs {
            let (outline, upem) = if alt {
                match &fallback_face {
                    Some(ff) => {
                        let mut ff = ff.borrow_mut();
                        let u = ff.upem;
                        (ff.outline(gid), u)
                    }
                    None => (None, face.upem),
                }
            } else {
                (face.outline(gid), face.upem)
            };
            if let Some(g) = outline {
                let k = em / upem;
                let gt = frame * Affine::translate((x, dya)) * Affine::scale_non_uniform(k * xscale, -k);
                out.extend(gt * (*g).clone());
            }
            x += adv;
        }
        if font.underline || font.strike {
            let th = (em / 14.0).max(1.0 / sy.max(1e-9));
            for (on, y) in [(font.underline, dya + em * 0.12), (font.strike, dya - em * 0.28)] {
                if on {
                    out.extend(frame * Rect::new(dxa, y - th / 2.0, dxa + total, y + th / 2.0).to_path(0.1));
                }
            }
        }
        let color = self.dc.text_color;
        self.fill_with(&out, &Brush::Solid(color), Fill::NonZero);
        if clipped {
            self.dc.clip = saved_clip;
            self.dc.clip_id = saved_id;
        }
    }
}

/// An 8×8 hatch tile (scaled to `size` pixels): horizontal (0), vertical
/// (1), down (2), up (3), cross (4) and diagonal cross (5) lines.
fn hatch_tile(style: u32, c: [u8; 3], size: u16) -> Pixmap {
    let n = size as usize;
    let on = PremulRgba8 { r: c[0], g: c[1], b: c[2], a: 255 };
    let off = PremulRgba8 { r: 0, g: 0, b: 0, a: 0 };
    let mut px = vec![off; n * n];
    let line = (n / 8).max(1);
    for y in 0..n {
        for x in 0..n {
            let h = y < line;
            let v = x < line;
            let d1 = (x + n - y) % n < line;
            let d2 = (x + y) % n < line;
            let hit = match style {
                0 => h,
                1 => v,
                2 => d1,
                3 => d2,
                4 => h || v,
                _ => d1 || d2,
            };
            if hit {
                px[y * n + x] = on;
            }
        }
    }
    Pixmap::from_parts(px, size, size)
}

/// A device-independent bitmap decoded, premultiplied, top row first.
struct Bitmap {
    width: u16,
    height: u16,
    pixels: Vec<PremulRgba8>,
}

/// Decodes a DIB: `bmi` the BITMAPINFO (header and colors), `bits` the
/// pixels. The alpha of 32-bit pixels is kept (most are opaque).
fn dib(bmi: &[u8], bits: &[u8]) -> Option<Bitmap> {
    let u32_at = |b: &[u8], o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    let u16_at = |b: &[u8], o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap()));
    let hsize = u32_at(bmi, 0)? as usize;
    let (w, h, bpp, comp, used, core) = if hsize == 12 {
        (u16_at(bmi, 4)? as i64, u16_at(bmi, 6)? as i16 as i64, u16_at(bmi, 10)?, 0, 0, true)
    } else if hsize >= 40 {
        (u32_at(bmi, 4)? as i32 as i64, u32_at(bmi, 8)? as i32 as i64, u16_at(bmi, 14)?, u32_at(bmi, 16)?, u32_at(bmi, 32)? as usize, false)
    } else {
        return None;
    };
    // JPEG or PNG inside.
    if comp == 4 || comp == 5 {
        let (pw, ph, rgba) = crate::picture::decode(bits)?;
        if pw == 0 || ph == 0 || pw > 30_000 || ph > 30_000 || rgba.len() < pw as usize * ph as usize * 4 {
            return None;
        }
        let pixels = rgba.chunks_exact(4).map(|p| premul(p[0], p[1], p[2], p[3])).collect();
        return Some(Bitmap { width: pw as u16, height: ph as u16, pixels });
    }
    let (aw, ah) = (w.unsigned_abs(), h.unsigned_abs());
    if aw == 0 || ah == 0 || aw > 30_000 || ah > 30_000 || aw * ah > 100_000_000 {
        return None;
    }
    let (aw, ah) = (aw as usize, ah as usize);
    let bottom_up = h > 0;
    let stride = (aw * bpp as usize).div_ceil(32) * 4;
    let entry = if core { 3 } else { 4 };
    let ncolors = if bpp <= 8 { if used > 0 { used.min(256) } else { 1 << bpp } } else { 0 };
    let mut masks = None;
    let mut pal_at = hsize;
    if comp == 3 || comp == 6 {
        let m = if hsize >= 52 { (u32_at(bmi, 40)?, u32_at(bmi, 44)?, u32_at(bmi, 48)?) } else { let m = (u32_at(bmi, hsize)?, u32_at(bmi, hsize + 4)?, u32_at(bmi, hsize + 8)?); pal_at += 12; m };
        masks = Some(m);
    }
    let palette: Vec<[u8; 3]> = (0..ncolors).map(|i| bmi.get(pal_at + i * entry..pal_at + i * entry + 3).map_or([0; 3], |c| [c[2], c[1], c[0]])).collect();
    let rle = matches!(comp, 1 | 2);
    let raw;
    let data: &[u8] = if rle {
        raw = unrle(bits, aw, ah, comp == 2)?;
        &raw
    } else {
        bits
    };
    let (stride, bpp) = if rle { (aw, 8) } else { (stride, bpp) };
    let mut straight = vec![[0u8, 0, 0, 255]; aw * ah];
    let mut any_alpha = false;
    for row in 0..ah {
        let src_row = if bottom_up { ah - 1 - row } else { row };
        let Some(line) = data.get(src_row * stride..src_row * stride + stride.min(data.len().saturating_sub(src_row * stride))) else { break };
        for x in 0..aw {
            let px = &mut straight[row * aw + x];
            let idx = |i: usize| palette.get(i).copied().unwrap_or([0; 3]);
            let c: Option<([u8; 3], u8)> = match bpp {
                1 => line.get(x / 8).map(|b| (idx(((b >> (7 - x % 8)) & 1) as usize), 255)),
                4 => line.get(x / 2).map(|b| (idx(if x % 2 == 0 { b >> 4 } else { b & 15 } as usize), 255)),
                8 => line.get(x).map(|&b| (idx(b as usize), 255)),
                16 => line.get(x * 2..x * 2 + 2).map(|b| {
                    let v = u16::from_le_bytes([b[0], b[1]]) as u32;
                    let (rm, gm, bm) = masks.unwrap_or((0x7C00, 0x03E0, 0x001F));
                    ([field(v, rm), field(v, gm), field(v, bm)], 255)
                }),
                24 => line.get(x * 3..x * 3 + 3).map(|b| ([b[2], b[1], b[0]], 255)),
                32 => line.get(x * 4..x * 4 + 4).map(|b| match masks {
                    Some((rm, gm, bm)) => {
                        let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                        ([field(v, rm), field(v, gm), field(v, bm)], 255)
                    }
                    None => {
                        if b[3] != 0 {
                            any_alpha = true;
                        }
                        ([b[2], b[1], b[0]], b[3])
                    }
                }),
                _ => return None,
            };
            if let Some((c, a)) = c {
                *px = [c[0], c[1], c[2], a];
            }
        }
    }
    // 32-bit pixels with no alpha at all are opaque.
    let opaque = bpp == 32 && !any_alpha;
    let pixels = straight.into_iter().map(|[r, g, b, a]| premul(r, g, b, if opaque { 255 } else { a })).collect();
    Some(Bitmap { width: aw as u16, height: ah as u16, pixels })
}

fn premul(r: u8, g: u8, b: u8, a: u8) -> PremulRgba8 {
    let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
    PremulRgba8 { r: m(r), g: m(g), b: m(b), a }
}

/// A channel of a 16 or 32-bit pixel by its mask, as 8 bits.
fn field(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let x = (v & mask) >> shift;
    let max = (1u64 << bits) - 1;
    ((x as u64 * 255 + max / 2) / max) as u8
}

/// RLE8 or RLE4 bits to one byte a pixel, rows bottom first as stored.
fn unrle(bits: &[u8], w: usize, h: usize, four: bool) -> Option<Vec<u8>> {
    let mut out = vec![0u8; w * h];
    let (mut x, mut y) = (0usize, 0usize);
    let mut i = 0;
    let mut put = |x: &mut usize, y: usize, v: u8| {
        if *x < w && y < h {
            out[y * w + *x] = v;
        }
        *x += 1;
    };
    while i + 1 < bits.len() && y < h {
        let (n, v) = (bits[i] as usize, bits[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                let c = if four { if k % 2 == 0 { v >> 4 } else { v & 15 } } else { v };
                put(&mut x, y, c);
            }
            continue;
        }
        match v {
            0 => {
                x = 0;
                y += 1;
            }
            1 => break,
            2 => {
                x += *bits.get(i)? as usize;
                y += *bits.get(i + 1)? as usize;
                i += 2;
            }
            m => {
                let m = m as usize;
                let bytes = if four { m.div_ceil(2) } else { m };
                let run = bits.get(i..i + bytes)?;
                for k in 0..m {
                    let c = if four { let b = run[k / 2]; if k % 2 == 0 { b >> 4 } else { b & 15 } } else { run[k] };
                    put(&mut x, y, c);
                }
                i += bytes + (bytes & 1);
            }
        }
    }
    Some(out)
}

/// A COLORREF (red, green, blue, flags).
fn colorref(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

/// The code page of a font's character set.
fn charset_cp(cs: u8) -> u16 {
    match cs {
        128 => 932,
        129 => 949,
        134 => 936,
        136 => 950,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        _ => 1252,
    }
}

/// Bytes of a font's character set as text: a symbol font's codes as they
/// are (they are looked up from U+F000 too).
fn ansi(bytes: &[u8], charset: u8) -> Vec<char> {
    if charset == 2 {
        return bytes.iter().map(|&b| b as char).collect();
    }
    crate::ole::decode_cp(bytes, charset_cp(charset)).chars().collect()
}

#[cfg(test)]
mod tests;
