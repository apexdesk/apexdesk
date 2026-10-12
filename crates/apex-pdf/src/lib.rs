//! Low-level PDF 1.7 writer for ApexDesk's export and print.
//!
//! The app lays text out itself and hands over positioned glyphs, so this
//! crate only has to place them exactly: fonts are embedded (subset) as
//! Type0/CID fonts and each glyph lands on the pen position it was given.
//!
//! Optionally the document is tagged (`tags`, for screen readers) and
//! PDF/A-2b (`pdfa`, for archiving).
//!
//! Coordinates are PDF points with the origin at the TOP-LEFT of the page
//! and y growing down; the crate flips them to PDF space internally.

mod cff;
mod font;
mod pdfa;
pub use pdfa::srgb_icc;
mod tags;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use font::{Font, Outlines};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontId(usize);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ImageId(usize);

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Uri(String),
    /// A position in this document: page index (0-based) and y from the top.
    Page { page: usize, y: f32 },
}

const A4: (f32, f32) = (595.276, 841.89);

struct FontEntry {
    font: Font,
    /// Code written in content streams -> (glyph id, first char shown with it).
    used: BTreeMap<u16, (u16, char)>,
    codes: HashMap<char, u16>,
    next_code: u32,
}

impl FontEntry {
    /// Code for a char. TrueType fonts get their own codes (mapped to glyph
    /// ids through CIDToGIDMap) so every char, even one missing from the font
    /// or sharing a glyph with another, keeps its own ToUnicode entry. CFF
    /// fonts can't remap and use the glyph's CID.
    fn code(&mut self, c: char) -> (u16, f32) {
        if let Some(&code) = self.codes.get(&c) {
            let gid = self.used.get(&code).map_or(0, |u| u.0);
            return (code, self.font.width(gid));
        }
        let gid = self.font.glyph_id(c);
        let code = match self.font.outlines {
            Outlines::Glyf if self.next_code <= 0xFFFF => {
                self.next_code += 1;
                (self.next_code - 1) as u16
            }
            Outlines::Glyf => 0,
            Outlines::Cff => self.font.cff_cid(gid),
        };
        let gid = self.used.entry(code).or_insert((gid, c)).0;
        self.codes.insert(c, code);
        (code, self.font.width(gid))
    }
}

/// A free-form triangle mesh shading (type 4): smooth color between the
/// corners of each triangle (gradients), with its bounds.
struct ShadingEntry {
    data: Vec<u8>,
    bounds: [f32; 4],
    /// Opacities (a soft mask's) instead of colors.
    gray: bool,
}

struct ImageEntry {
    width: u32,
    height: u32,
    /// Color data, compressed as `format` says.
    rgb: Vec<u8>,
    alpha: Option<Vec<u8>>,
    /// Color space, bit depth and filter keys of the image dictionary.
    format: String,
}

const RGB_FLATE: &str = "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode";

/// Size, components and Adobe marker of a JPEG PDF readers can show
/// (baseline or progressive Huffman, 8 bits, 1/3/4 components).
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8, bool)> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let (mut i, mut adobe) = (2, false);
    loop {
        // Markers may be padded with extra 0xFF bytes.
        while *data.get(i)? == 0xFF && *data.get(i + 1)? == 0xFF {
            i += 1;
        }
        if *data.get(i)? != 0xFF {
            return None;
        }
        let m = *data.get(i + 1)?;
        let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
        let seg = data.get(i + 4..(i + 2 + len).max(i + 4))?;
        match m {
            0xEE if seg.starts_with(b"Adobe") => adobe = true,
            0xC0..=0xC2 => {
                let (bits, h, w, n) = (*seg.first()?, u16::from_be_bytes([*seg.get(1)?, *seg.get(2)?]), u16::from_be_bytes([*seg.get(3)?, *seg.get(4)?]), *seg.get(5)?);
                return (bits == 8 && w > 0 && h > 0 && matches!(n, 1 | 3 | 4)).then_some((w.into(), h.into(), n, adobe));
            }
            // Other frame types (lossless, arithmetic, hierarchical).
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDA | 0xD9 => return None,
            _ => {}
        }
        i += 2 + len;
    }
}

struct Link {
    rect: [f32; 4],
    target: LinkTarget,
}

struct Page {
    width: f32,
    height: f32,
    content: Vec<u8>,
    links: Vec<Link>,
    /// Structure element of each marked-content id (tagged PDF).
    mcids: Vec<usize>,
}

/// Graphics state already set in the content stream, to avoid repeating it.
#[derive(Clone, Copy, Default, PartialEq)]
struct GState {
    fill: Option<[u8; 3]>,
    stroke: Option<[u8; 3]>,
    line_width: Option<f32>,
}

struct PageBuild {
    width: f32,
    height: f32,
    ops: String,
    links: Vec<Link>,
    gs: GState,
    saved: Vec<GState>,
    /// The marked-content sequence open (tagged PDF).
    mark: Option<tags::Mark>,
    mcids: Vec<usize>,
}

impl PageBuild {
    fn fy(&self, y: f32) -> f32 {
        self.height - y
    }

    fn fill_color(&mut self, c: [u8; 3]) {
        if self.gs.fill != Some(c) {
            let _ = writeln!(self.ops, "{} rg", color(c));
            self.gs.fill = Some(c);
        }
    }

    fn stroke(&mut self, width: f32, c: [u8; 3]) {
        if self.gs.stroke != Some(c) {
            let _ = writeln!(self.ops, "{} RG", color(c));
            self.gs.stroke = Some(c);
        }
        let width = if width.is_finite() { width.max(0.0) } else { 1.0 };
        if self.gs.line_width != Some(width) {
            let _ = writeln!(self.ops, "{} w", num(width));
            self.gs.line_width = Some(width);
        }
    }

    fn save(&mut self) {
        self.ops.push_str("q\n");
        self.saved.push(self.gs);
    }

    fn restore(&mut self) -> bool {
        match self.saved.pop() {
            Some(gs) => {
                self.ops.push_str("Q\n");
                self.gs = gs;
                true
            }
            None => false,
        }
    }

    fn path(&mut self, pts: &[(f32, f32)], closed: bool) {
        for (i, &(x, y)) in pts.iter().enumerate() {
            let op = if i == 0 { "m" } else { "l" };
            let _ = writeln!(self.ops, "{} {} {op}", num(x), num(self.fy(y)));
        }
        if closed {
            self.ops.push_str("h\n");
        }
    }

    fn paint(&mut self, fill: Option<[u8; 3]>, stroke: Option<(f32, [u8; 3])>) {
        if let Some(c) = fill {
            self.fill_color(c);
        }
        if let Some((w, c)) = stroke {
            self.stroke(w, c);
        }
        self.ops.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B\n",
            (true, false) => "f\n",
            (false, true) => "S\n",
            (false, false) => "n\n",
        });
    }
}

struct OutlineEntry {
    title: String,
    level: u8,
    page: usize,
    y: f32,
}

/// A PDF document being built. See the crate docs for coordinates.
pub struct Pdf {
    title: String,
    author: String,
    fonts: Vec<FontEntry>,
    images: Vec<ImageEntry>,
    /// Distinct alpha values (in 1/1000) used by `fill_rect_alpha`.
    alphas: Vec<u16>,
    shadings: Vec<ShadingEntry>,
    /// Soft masks: the gray shading each paints and its bounds.
    masks: Vec<(usize, [f32; 4])>,
    pages: Vec<Page>,
    cur: Option<PageBuild>,
    outlines: Vec<OutlineEntry>,
    tags: Option<tags::Tree>,
    pdfa: bool,
}

impl Default for Pdf {
    fn default() -> Self {
        Self::new()
    }
}

impl Pdf {
    pub fn new() -> Pdf {
        Pdf {
            title: String::new(),
            author: String::new(),
            fonts: Vec::new(),
            images: Vec::new(),
            alphas: Vec::new(),
            shadings: Vec::new(),
            masks: Vec::new(),
            pages: Vec::new(),
            cur: None,
            outlines: Vec::new(),
            tags: None,
            pdfa: false,
        }
    }

    /// Makes the document PDF/A-2b (see `pdfa`).
    pub fn set_pdfa(&mut self, on: bool) {
        self.pdfa = on;
    }

    pub fn set_info(&mut self, title: &str, author: &str) {
        self.title = title.to_string();
        self.author = author.to_string();
    }

    /// Registers a TrueType/OpenType font (.ttf/.otf, or face `index` of a
    /// .ttc collection). None if it can't be parsed. Registering the same
    /// bytes and index twice returns the same id.
    pub fn add_font(&mut self, data: Arc<[u8]>, index: u32) -> Option<FontId> {
        if let Some(i) = self
            .fonts
            .iter()
            .position(|f| f.font.index == index && (Arc::ptr_eq(&f.font.data, &data) || *f.font.data == *data))
        {
            return Some(FontId(i));
        }
        let font = Font::parse(data, index)?;
        self.fonts.push(FontEntry { font, used: BTreeMap::new(), codes: HashMap::new(), next_code: 1 });
        Some(FontId(self.fonts.len() - 1))
    }

    /// Whether the font has a glyph for `c`.
    pub fn has_char(&self, font: FontId, c: char) -> bool {
        self.fonts.get(font.0).is_some_and(|f| f.font.has_char(c))
    }

    /// Advance width of `c` in 1/1000 em.
    pub fn advance(&self, font: FontId, c: char) -> f32 {
        self.fonts.get(font.0).map_or(0.0, |f| f.font.width(f.font.glyph_id(c)))
    }

    /// Raw image: RGBA8 pixels, row major. Alpha becomes an SMask. Missing
    /// pixels (short buffer) are transparent.
    pub fn add_image(&mut self, width: u32, height: u32, rgba: &[u8]) -> ImageId {
        let (mut w, mut h) = (width.max(1), height.max(1));
        let n = match (w as usize).checked_mul(h as usize) {
            Some(n) if n <= 1 << 28 => n,
            _ => {
                (w, h) = (1, 1);
                1
            }
        };
        let mut rgb = Vec::with_capacity(n * 3);
        let mut alpha = Vec::with_capacity(n);
        for i in 0..n {
            match rgba.get(4 * i..4 * i + 4) {
                Some(p) => {
                    rgb.extend_from_slice(&p[..3]);
                    alpha.push(p[3]);
                }
                None => {
                    rgb.extend_from_slice(&[0, 0, 0]);
                    alpha.push(0);
                }
            }
        }
        let alpha = if alpha.iter().any(|&a| a != 255) { Some(deflate(&alpha)) } else { None };
        self.images.push(ImageEntry { width: w, height: h, rgb: deflate(&rgb), alpha, format: RGB_FLATE.into() });
        ImageId(self.images.len() - 1)
    }

    /// JPEG file stored as is (/DCTDecode): smaller than `add_image` and
    /// without recompression. `None` when readers may not show it
    /// (arithmetic coding, 12 bits, odd components); decode it and use
    /// `add_image` then.
    pub fn add_jpeg(&mut self, data: &[u8]) -> Option<ImageId> {
        let (width, height, n, adobe) = jpeg_info(data)?;
        // PDF/A with an RGB output intent takes no device CMYK.
        if self.pdfa && n == 4 {
            return None;
        }
        let space = match n {
            1 => "/DeviceGray",
            3 => "/DeviceRGB",
            // Adobe writes CMYK inverted.
            _ if adobe => "/DeviceCMYK /Decode [1 0 1 0 1 0 1 0]",
            _ => "/DeviceCMYK",
        };
        let format = format!("/ColorSpace {space} /BitsPerComponent 8 /Filter /DCTDecode");
        self.images.push(ImageEntry { width, height, rgb: data.to_vec(), alpha: None, format });
        Some(ImageId(self.images.len() - 1))
    }

    fn end_page(&mut self) {
        if let Some(mut p) = self.cur.take() {
            if p.mark.is_some() {
                p.ops.push_str("EMC\n");
            }
            while p.restore() {}
            self.pages.push(Page { width: p.width, height: p.height, content: deflate(p.ops.as_bytes()), links: p.links, mcids: p.mcids });
        }
    }

    /// Starts a new page (size in points), finishing the current one.
    pub fn begin_page(&mut self, width: f32, height: f32) {
        self.end_page();
        let sane = |v: f32, d: f32| if v.is_finite() && v >= 1.0 { v.min(14400.0) } else { d };
        self.cur = Some(PageBuild {
            width: sane(width, A4.0),
            height: sane(height, A4.1),
            ops: String::new(),
            links: Vec::new(),
            gs: GState::default(),
            saved: Vec::new(),
            mark: None,
            mcids: Vec::new(),
        });
    }

    /// Glyphs of one run: each char drawn with its pen position x on a shared
    /// baseline y.
    pub fn glyphs(&mut self, font: FontId, size: f32, y: f32, glyphs: &[(char, f32)], color: [u8; 3]) {
        self.glyphs_skewed(font, size, y, glyphs, color, 0.0);
    }

    /// Like `glyphs`, slanted by `skew` (x shift per unit of height, e.g.
    /// 0.2) to synthesize an italic for faces without one.
    pub fn glyphs_skewed(&mut self, font: FontId, size: f32, y: f32, glyphs: &[(char, f32)], color: [u8; 3], skew: f32) {
        let (Some(page), Some(fe)) = (self.cur.as_mut(), self.fonts.get_mut(font.0)) else { return };
        if glyphs.is_empty() || !size.is_finite() || size <= 0.0 || !y.is_finite() {
            return;
        }
        let size = size.min(10000.0);
        let skew = if skew.is_finite() { skew.clamp(-10.0, 10.0) } else { 0.0 };
        let x0 = if glyphs[0].1.is_finite() { glyphs[0].1 } else { 0.0 };
        page.fill_color(color);
        let _ = write!(page.ops, "BT\n/F{} {} Tf\n1 0 {} 1 {} {} Tm\n[<", font.0 + 1, num(size), num(skew), num(x0), num(page.fy(y)));
        // The pen after each glyph advances by its width; the TJ adjustment
        // moves it to where the caller wants the next glyph.
        let mut pen = x0;
        let mut first = true;
        for &(c, x) in glyphs {
            let (code, width) = fe.code(c);
            if !first && x.is_finite() {
                let adj = ((pen - x) * 1000.0 / size * 10.0).round() / 10.0;
                if adj != 0.0 {
                    let _ = write!(page.ops, ">{}<", num(adj));
                    pen -= adj * size / 1000.0;
                }
            }
            first = false;
            let _ = write!(page.ops, "{code:04X}");
            pen += width * size / 1000.0;
        }
        page.ops.push_str(">]TJ\nET\n");
    }

    /// Like `glyphs_skewed`, `alpha` (0..1) opaque.
    pub fn glyphs_alpha(&mut self, font: FontId, size: f32, y: f32, glyphs: &[(char, f32)], color: [u8; 3], skew: f32, alpha: f32) {
        let a = if alpha.is_finite() { (alpha.clamp(0.0, 1.0) * 1000.0).round() as u16 } else { 1000 };
        if a >= 1000 {
            return self.glyphs_skewed(font, size, y, glyphs, color, skew);
        }
        if a == 0 {
            return;
        }
        let gs = self.alpha_state(a);
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        let _ = writeln!(p.ops, "/GS{} gs", gs + 1);
        self.glyphs_skewed(font, size, y, glyphs, color, skew);
        if let Some(p) = self.cur.as_mut() {
            p.restore();
        }
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 3]) {
        let Some(p) = self.cur.as_mut() else { return };
        p.fill_color(color);
        let _ = writeln!(p.ops, "{} {} {} {} re f", num(x), num(p.fy(y) - h), num(w), num(h));
    }

    /// Translucent rectangle (alpha 0..1) through an ExtGState.
    pub fn fill_rect_alpha(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 3], alpha: f32) {
        let a = if alpha.is_finite() { (alpha.clamp(0.0, 1.0) * 1000.0).round() as u16 } else { 1000 };
        if a >= 1000 {
            return self.fill_rect(x, y, w, h, color);
        }
        let gs = self.alpha_state(a);
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        let _ = writeln!(p.ops, "/GS{} gs", gs + 1);
        p.fill_color(color);
        let _ = writeln!(p.ops, "{} {} {} {} re f", num(x), num(p.fy(y) - h), num(w), num(h));
        p.restore();
    }

    /// The graphics state for an alpha in 1/1000.
    fn alpha_state(&mut self, a: u16) -> usize {
        match self.alphas.iter().position(|&v| v == a) {
            Some(i) => i,
            None => {
                self.alphas.push(a);
                self.alphas.len() - 1
            }
        }
    }

    /// Fills a closed polygon, `alpha` opaque.
    pub fn fill_polygon_alpha(&mut self, pts: &[(f32, f32)], color: [u8; 3], alpha: f32) {
        let a = if alpha.is_finite() { (alpha.clamp(0.0, 1.0) * 1000.0).round() as u16 } else { 1000 };
        if a >= 1000 {
            return self.polygon(pts, true, Some(color), None);
        }
        if pts.len() < 3 {
            return;
        }
        let gs = self.alpha_state(a);
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        let _ = writeln!(p.ops, "/GS{} gs", gs + 1);
        p.fill_color(color);
        p.path(pts, true);
        p.ops.push_str("f\n");
        p.restore();
    }

    /// Strokes an open line, `alpha` opaque, with round joints (and round
    /// ends when `round`), as ink is drawn.
    pub fn stroke_path_alpha(&mut self, pts: &[(f32, f32)], width: f32, color: [u8; 3], alpha: f32, round: bool) {
        let a = if alpha.is_finite() { (alpha.clamp(0.0, 1.0) * 1000.0).round() as u16 } else { 1000 };
        if pts.len() < 2 || a == 0 {
            return;
        }
        let gs = (a < 1000).then(|| self.alpha_state(a));
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        if let Some(gs) = gs {
            let _ = writeln!(p.ops, "/GS{} gs", gs + 1);
        }
        let _ = writeln!(p.ops, "1 j {} J", if round { 1 } else { 0 });
        p.stroke(width, color);
        p.path(pts, false);
        p.ops.push_str("S\n");
        p.restore();
    }

    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: [u8; 3]) {
        let Some(p) = self.cur.as_mut() else { return };
        p.stroke(width, color);
        p.path(&[(x0, y0), (x1, y1)], false);
        p.ops.push_str("S\n");
    }

    pub fn dashed_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: [u8; 3], dash: &[f32]) {
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        let pattern: Vec<String> = dash.iter().filter(|d| d.is_finite() && **d >= 0.0).take(16).map(|&d| num(d)).collect();
        // All-zero dash arrays are an error in PDF; draw solid instead.
        if dash.iter().any(|&d| d > 0.0) {
            let _ = writeln!(p.ops, "[{}] 0 d", pattern.join(" "));
        }
        p.stroke(width, color);
        p.path(&[(x0, y0), (x1, y1)], false);
        p.ops.push_str("S\n");
        p.restore();
    }

    /// Paints triangles whose colors (and opacities, 0..255) blend smoothly
    /// between their corners (a gradient), as one shading; see-through
    /// ones through a soft mask shaded the same way.
    pub fn gouraud(&mut self, tris: &[[((f32, f32), [u8; 3], u8); 3]]) {
        let Some(p) = self.cur.as_mut() else { return };
        let pts = || tris.iter().flat_map(|t| t.iter().map(|v| (v.0.0, p.fy(v.0.1))));
        if tris.is_empty() || pts().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
            return;
        }
        let mut b = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
        for (x, y) in pts() {
            b = [b[0].min(x), b[1].max(x), b[2].min(y), b[3].max(y)];
        }
        b[1] = b[1].max(b[0] + 1e-3);
        b[3] = b[3].max(b[2] + 1e-3);
        // Flag, x and y in 32 bits over the bounds, then the color.
        let q = |v: f32, lo: f32, hi: f32| (((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f64 * u32::MAX as f64).round() as u32;
        let data = |color: &dyn Fn(&((f32, f32), [u8; 3], u8)) -> Vec<u8>| {
            let mut d = Vec::with_capacity(tris.len() * 3 * 12);
            for t in tris {
                for v in t {
                    d.push(0);
                    d.extend_from_slice(&q(v.0.0, b[0], b[1]).to_be_bytes());
                    d.extend_from_slice(&q(p.fy(v.0.1), b[2], b[3]).to_be_bytes());
                    d.extend_from_slice(&color(v));
                }
            }
            deflate(&d)
        };
        let see_through = tris.iter().flatten().any(|v| v.2 < 255);
        let mask = see_through.then(|| {
            self.shadings.push(ShadingEntry { data: data(&|v| vec![v.2]), bounds: b, gray: true });
            self.masks.push((self.shadings.len() - 1, b));
            self.masks.len()
        });
        self.shadings.push(ShadingEntry { data: data(&|v| v.1.to_vec()), bounds: b, gray: false });
        let n = self.shadings.len();
        p.save();
        if let Some(m) = mask {
            let _ = writeln!(p.ops, "/SM{m} gs");
        }
        let _ = writeln!(p.ops, "/Sh{n} sh");
        p.restore();
    }

    pub fn polygon(&mut self, pts: &[(f32, f32)], closed: bool, fill: Option<[u8; 3]>, stroke: Option<(f32, [u8; 3])>) {
        let Some(p) = self.cur.as_mut() else { return };
        if pts.len() < 2 {
            return;
        }
        p.path(pts, closed);
        p.paint(fill, stroke);
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Option<[u8; 3]>, stroke: Option<(f32, [u8; 3])>) {
        let Some(p) = self.cur.as_mut() else { return };
        if !r.is_finite() || r <= 0.0 {
            return;
        }
        // Four cubic Béziers; k is the usual circle approximation constant.
        let k = 0.552_284_8 * r;
        let cy = p.fy(cy);
        let pt = |x: f32, y: f32| format!("{} {}", num(x), num(y));
        let _ = writeln!(p.ops, "{} m", pt(cx + r, cy));
        let _ = writeln!(p.ops, "{} {} {} c", pt(cx + r, cy + k), pt(cx + k, cy + r), pt(cx, cy + r));
        let _ = writeln!(p.ops, "{} {} {} c", pt(cx - k, cy + r), pt(cx - r, cy + k), pt(cx - r, cy));
        let _ = writeln!(p.ops, "{} {} {} c", pt(cx - r, cy - k), pt(cx - k, cy - r), pt(cx, cy - r));
        let _ = writeln!(p.ops, "{} {} {} c", pt(cx + k, cy - r), pt(cx + r, cy - k), pt(cx + r, cy));
        p.ops.push_str("h\n");
        p.paint(fill, stroke);
    }

    /// Draws an image stretched to the rectangle.
    pub fn image(&mut self, img: ImageId, x: f32, y: f32, w: f32, h: f32) {
        let Some(p) = self.cur.as_mut() else { return };
        if img.0 >= self.images.len() {
            return;
        }
        let _ = writeln!(p.ops, "q {} 0 0 {} {} {} cm /Im{} Do Q", num(w), num(h), num(x), num(p.fy(y) - h), img.0 + 1);
    }

    /// Clips everything drawn until the matching `pop_clip` to a rectangle.
    pub fn push_clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        let _ = writeln!(p.ops, "{} {} {} {} re W n", num(x), num(p.fy(y) - h), num(w), num(h));
    }

    /// Clips everything drawn until the matching `pop_clip` to the union
    /// of closed polygons.
    pub fn push_clip_polygons(&mut self, polys: &[Vec<(f32, f32)>]) {
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        let mut any = false;
        for poly in polys.iter().filter(|q| q.len() >= 3) {
            p.path(poly, true);
            any = true;
        }
        if any {
            p.ops.push_str("W n\n");
        }
    }

    pub fn pop_clip(&mut self) {
        if let Some(p) = self.cur.as_mut() {
            p.restore();
        }
    }

    /// Applies an affine transform, in the crate's top-left/y-down space
    /// (x' = a*x + c*y + e, y' = b*x + d*y + f), to everything drawn until
    /// the matching `pop_transform`. Links are not transformed.
    pub fn push_transform(&mut self, m: [f32; 6]) {
        let Some(p) = self.cur.as_mut() else { return };
        p.save();
        if m.iter().all(|v| v.is_finite()) {
            // Conjugate by the page flip F(x, y) = (x, H - y): F * M * F.
            let [a, b, c, d, e, f] = m;
            let hh = p.height;
            let t = [a, -b, -c, d, c * hh + e, hh - d * hh - f];
            let _ = writeln!(p.ops, "{} cm", t.map(num).join(" "));
        }
    }

    pub fn pop_transform(&mut self) {
        if let Some(p) = self.cur.as_mut() {
            p.restore();
        }
    }

    /// Clickable area on the current page.
    pub fn link(&mut self, x: f32, y: f32, w: f32, h: f32, target: LinkTarget) {
        let Some(p) = self.cur.as_mut() else { return };
        if ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return;
        }
        let fy = p.fy(y);
        p.links.push(Link { rect: [x, fy - h, x + w, fy], target });
    }

    /// Document outline (bookmarks). Levels start at 0 and nest by level in
    /// call order.
    pub fn outline(&mut self, title: &str, level: u8, page: usize, y: f32) {
        self.outlines.push(OutlineEntry { title: title.to_string(), level, page, y });
    }

    pub fn page_count(&self) -> usize {
        self.pages.len() + usize::from(self.cur.is_some())
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.end_page();
        if self.pages.is_empty() {
            self.begin_page(A4.0, A4.1);
            self.end_page();
        }
        Writer::default().write(&self)
    }
}

/// Serializer with object offsets for the xref table.
#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    offsets: Vec<usize>,
    /// Each page's structure elements by MCID (tagged PDF).
    page_mcids: Vec<Vec<usize>>,
}

impl Writer {
    fn alloc(&mut self) -> usize {
        self.offsets.push(0);
        self.offsets.len()
    }

    fn obj(&mut self, id: usize, body: &str) {
        self.offsets[id - 1] = self.out.len();
        let _ = write!(self, "{id} 0 obj\n{body}\nendobj\n");
    }

    fn stream(&mut self, id: usize, dict: &str, data: &[u8]) {
        self.offsets[id - 1] = self.out.len();
        let _ = write!(self, "{id} 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len());
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    fn write(mut self, pdf: &Pdf) -> Vec<u8> {
        self.out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        let catalog = self.alloc();
        let pages_id = self.alloc();
        let info = self.alloc();
        let resources = self.alloc();
        let page_ids: Vec<usize> = pdf.pages.iter().map(|_| self.alloc()).collect();
        let page_ref = |i: usize| page_ids[i.min(page_ids.len() - 1)];
        self.page_mcids = pdf.pages.iter().map(|p| p.mcids.clone()).collect();

        // Pages with their content and link annotations.
        for (i, page) in pdf.pages.iter().enumerate() {
            let content = self.alloc();
            self.stream(content, "/Filter /FlateDecode", &page.content);
            let mut annots = Vec::new();
            for link in &page.links {
                // Only links that open a page or a mail (no file:, script
                // or handler schemes).
                if let LinkTarget::Uri(uri) = &link.target {
                    let scheme = uri.split_once(':').map(|(s, _)| s.trim().to_ascii_lowercase());
                    if !matches!(scheme.as_deref(), Some("http" | "https" | "mailto" | "ftp" | "tel")) {
                        continue;
                    }
                }
                let action = match &link.target {
                    LinkTarget::Uri(uri) => format!("/A << /S /URI /URI {} >>", pdf_string_ascii(uri)),
                    LinkTarget::Page { page: target, y } => {
                        let t = (*target).min(pdf.pages.len() - 1);
                        format!("/Dest {}", dest(page_ref(t), &pdf.pages[t], *y))
                    }
                };
                let id = self.alloc();
                let r = link.rect.map(num).join(" ");
                self.obj(id, &format!("<< /Type /Annot /Subtype /Link /Rect [{r}] /Border [0 0 0] /F 4 {action} >>"));
                annots.push(format!("{id} 0 R"));
            }
            let mut annots = if annots.is_empty() { String::new() } else { format!(" /Annots [{}]", annots.join(" ")) };
            if pdf.tags.is_some() {
                let _ = write!(annots, " /StructParents {i} /Tabs /S");
            }
            self.obj(
                page_ids[i],
                &format!(
                    "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Resources {resources} 0 R /Contents {content} 0 R{annots} >>",
                    num(page.width),
                    num(page.height)
                ),
            );
        }
        let kids: Vec<String> = page_ids.iter().map(|id| format!("{id} 0 R")).collect();
        self.obj(pages_id, &format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), page_ids.len()));

        // Shared resources: fonts, images and alpha states.
        let mut res = String::from("<< /ProcSet [/PDF /Text /ImageC]");
        let fonts: Vec<String> = pdf
            .fonts
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.used.is_empty())
            .map(|(i, f)| format!("/F{} {} 0 R", i + 1, self.font(f)))
            .collect();
        if !fonts.is_empty() {
            let _ = write!(res, " /Font << {} >>", fonts.join(" "));
        }
        if !pdf.images.is_empty() {
            let ims: Vec<String> =
                pdf.images.iter().enumerate().map(|(i, img)| format!("/Im{} {} 0 R", i + 1, self.image(img))).collect();
            let _ = write!(res, " /XObject << {} >>", ims.join(" "));
        }
        let mut gs: Vec<String> = pdf
            .alphas
            .iter()
            .enumerate()
            .map(|(i, &a)| {
                let a = num(a as f32 / 1000.0);
                format!("/GS{} << /Type /ExtGState /ca {a} /CA {a} >>", i + 1)
            })
            .collect();
        if !pdf.shadings.is_empty() {
            let mut sh = Vec::new();
            let mut ids = Vec::new();
            for (i, e) in pdf.shadings.iter().enumerate() {
                let id = self.alloc();
                let b = e.bounds.map(num).join(" ");
                let (cs, decode) = if e.gray { ("DeviceGray", "0 1") } else { ("DeviceRGB", "0 1 0 1 0 1") };
                self.stream(id, &format!("/ShadingType 4 /ColorSpace /{cs} /BitsPerCoordinate 32 /BitsPerComponent 8 /BitsPerFlag 8 /Decode [{b} {decode}] /Filter /FlateDecode"), &e.data);
                sh.push(format!("/Sh{} {id} 0 R", i + 1));
                ids.push(id);
            }
            let _ = write!(res, " /Shading << {} >>", sh.join(" "));
            // Soft masks: a form painting the opacities as grays.
            for (i, (k, b)) in pdf.masks.iter().enumerate() {
                let form = self.alloc();
                let bbox = format!("{} {} {} {}", num(b[0]), num(b[2]), num(b[1]), num(b[3]));
                self.stream(form, &format!("/Type /XObject /Subtype /Form /BBox [{bbox}] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Shading << /S0 {} 0 R >> >>", ids[*k]), b"/S0 sh");
                gs.push(format!("/SM{} << /Type /ExtGState /SMask << /S /Luminosity /G {form} 0 R >> >>", i + 1));
            }
        }
        if !gs.is_empty() {
            let _ = write!(res, " /ExtGState << {} >>", gs.join(" "));
        }
        res.push_str(" >>");
        self.obj(resources, &res);

        let outlines = self.outlines(pdf, &page_ref);
        let outlines = match outlines {
            Some(id) => format!(" /Outlines {id} 0 R /PageMode /UseOutlines"),
            None => String::new(),
        };
        let mut extra = String::new();
        if let Some(t) = &pdf.tags {
            let root = self.structure(t, &page_ids);
            let _ = write!(extra, " /StructTreeRoot {root} 0 R /MarkInfo << /Marked true >> /ViewerPreferences << /DisplayDocTitle true >>");
            if !t.lang.is_empty() {
                let _ = write!(extra, " /Lang {}", pdf_string_ascii(&t.lang));
            }
        }
        let date = pdf_date();
        if pdf.pdfa {
            let meta = self.alloc();
            let lang = pdf.tags.as_ref().map_or("", |t| t.lang.as_str());
            self.stream(meta, "/Type /Metadata /Subtype /XML", pdfa::xmp(&pdf.title, &pdf.author, "ApexDesk", &date, lang).as_bytes());
            let icc = self.alloc();
            self.stream(icc, "/N 3 /Filter /FlateDecode", &deflate(&pdfa::srgb_icc()));
            let _ = write!(extra, " /Metadata {meta} 0 R /OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB IEC61966-2.1) /Info (sRGB IEC61966-2.1) /DestOutputProfile {icc} 0 R >>]");
        }
        self.obj(catalog, &format!("<< /Type /Catalog /Pages {pages_id} 0 R{outlines}{extra} >>"));
        let mut info_dict = String::from("<<");
        if !pdf.title.is_empty() || !pdf.pdfa {
            let _ = write!(info_dict, " /Title {}", pdf_text(&pdf.title));
        }
        if !pdf.author.is_empty() || !pdf.pdfa {
            let _ = write!(info_dict, " /Author {}", pdf_text(&pdf.author));
        }
        let _ = write!(info_dict, " /Producer {} /CreationDate ({date}) >>", pdf_text("ApexDesk"));
        self.obj(info, &info_dict);

        // Cross-reference table and trailer.
        let id = file_id(&self.out);
        let xref = self.out.len();
        let size = self.offsets.len() + 1;
        let _ = write!(self, "xref\n0 {size}\n0000000000 65535 f \n");
        for off in std::mem::take(&mut self.offsets) {
            let _ = write!(self, "{off:010} 00000 n \n");
        }
        let _ = write!(
            self,
            "trailer\n<< /Size {size} /Root {catalog} 0 R /Info {info} 0 R /ID [<{id}> <{id}>] >>\nstartxref\n{xref}\n%%EOF\n"
        );
        self.out
    }

    /// Writes the structure tree of a tagged PDF; returns its root.
    fn structure(&mut self, t: &tags::Tree, page_ids: &[usize]) -> usize {
        let (root, doc, tree) = (self.alloc(), self.alloc(), self.alloc());
        let ids: Vec<usize> = t.elems.iter().map(|_| self.alloc()).collect();
        for (e, &id) in t.elems.iter().zip(&ids) {
            let mut k = String::new();
            for &(page, mcid) in &e.kids {
                let _ = write!(k, "<< /Type /MCR /Pg {} 0 R /MCID {mcid} >> ", page_ids[page.min(page_ids.len() - 1)]);
            }
            let alt = e.alt.as_deref().map(|a| format!(" /Alt {}", pdf_text(a))).unwrap_or_default();
            self.obj(id, &format!("<< /Type /StructElem /S /{} /P {doc} 0 R /K [{}]{alt} >>", e.role, k.trim_end()));
        }
        let kids: Vec<String> = ids.iter().map(|id| format!("{id} 0 R")).collect();
        self.obj(doc, &format!("<< /Type /StructElem /S /Document /P {root} 0 R /K [{}] >>", kids.join(" ")));
        // Parent tree: for each page, the element of each MCID.
        let mut nums = String::new();
        for (i, p) in self.page_mcids.iter().enumerate() {
            let refs: Vec<String> = p.iter().map(|&e| format!("{} 0 R", ids[e])).collect();
            let _ = write!(nums, "{i} [{}] ", refs.join(" "));
        }
        self.obj(tree, &format!("<< /Nums [{}] >>", nums.trim_end()));
        self.obj(root, &format!("<< /Type /StructTreeRoot /K {doc} 0 R /ParentTree {tree} 0 R /ParentTreeNextKey {} >>", page_ids.len()));
        root
    }

    /// Writes a used font (Type0 + descendant + descriptor + program +
    /// ToUnicode); returns the Type0 font object id.
    fn font(&mut self, f: &FontEntry) -> usize {
        let font = &f.font;
        let gids: BTreeSet<u16> = f.used.values().map(|u| u.0).collect();
        let tag = subset_tag(font, &gids);
        let base = format!("{tag}+{}", font.name);
        let (type0, cid, desc, file, tounicode) = (self.alloc(), self.alloc(), self.alloc(), self.alloc(), self.alloc());

        let (program, file_key, file_dict) = match font.outlines {
            Outlines::Glyf => {
                let sub = font.subset_glyf(&gids);
                let len = sub.len();
                (sub, "FontFile2", format!("/Length1 {len} /Filter /FlateDecode"))
            }
            Outlines::Cff => (font.subset_cff(&gids), "FontFile3", "/Subtype /OpenType /Filter /FlateDecode".to_string()),
        };
        self.stream(file, &file_dict, &deflate(&program));

        let m = &font.metrics;
        self.obj(
            desc,
            &format!(
                "<< /Type /FontDescriptor /FontName /{base} /Flags {} /FontBBox [{}] /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV {} /{file_key} {file} 0 R >>",
                m.flags,
                m.bbox.map(num).join(" "),
                num(m.italic_angle),
                num(m.ascent),
                num(m.descent),
                num(m.cap_height),
                num(m.stem_v.round()),
            ),
        );

        // /W: runs of consecutive codes share one array.
        let mut w = String::new();
        let mut prev: Option<u16> = None;
        for (&code, &(gid, _)) in f.used.iter().filter(|(c, _)| **c != 0 || font.outlines == Outlines::Cff) {
            if prev.is_some_and(|p| p as u32 + 1 == code as u32) {
                w.push(' ');
            } else {
                if prev.is_some() {
                    w.push_str("] ");
                }
                let _ = write!(w, "{code} [");
            }
            w.push_str(&num(font.width(gid)));
            prev = Some(code);
        }
        if prev.is_some() {
            w.push(']');
        }

        let sysinfo = "/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>";
        let cid_dict = match font.outlines {
            Outlines::Glyf => {
                let map_id = self.alloc();
                let max = f.used.keys().next_back().copied().unwrap_or(0) as usize;
                let mut map = vec![0u8; 2 * (max + 1)];
                for (&code, &(gid, _)) in &f.used {
                    map[2 * code as usize..2 * code as usize + 2].copy_from_slice(&gid.to_be_bytes());
                }
                self.stream(map_id, "/Filter /FlateDecode", &deflate(&map));
                format!("/Subtype /CIDFontType2 /CIDToGIDMap {map_id} 0 R")
            }
            Outlines::Cff => "/Subtype /CIDFontType0".to_string(),
        };
        self.obj(
            cid,
            &format!("<< /Type /Font {cid_dict} /BaseFont /{base} {sysinfo} /FontDescriptor {desc} 0 R /DW 1000 /W [{w}] >>"),
        );
        self.stream(tounicode, "/Filter /FlateDecode", &deflate(to_unicode(&f.used).as_bytes()));
        self.obj(
            type0,
            &format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{base} /Encoding /Identity-H /DescendantFonts [{cid} 0 R] /ToUnicode {tounicode} 0 R >>"
            ),
        );
        type0
    }

    fn image(&mut self, img: &ImageEntry) -> usize {
        let id = self.alloc();
        let smask = match &img.alpha {
            Some(a) => {
                let sid = self.alloc();
                self.stream(
                    sid,
                    &format!(
                        "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
                        img.width, img.height
                    ),
                    a,
                );
                format!(" /SMask {sid} 0 R")
            }
            None => String::new(),
        };
        self.stream(
            id,
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} {}{smask}",
                img.width, img.height, img.format
            ),
            &img.rgb,
        );
        id
    }

    fn outlines(&mut self, pdf: &Pdf, page_ref: &dyn Fn(usize) -> usize) -> Option<usize> {
        let items = &pdf.outlines;
        if items.is_empty() {
            return None;
        }
        let root = self.alloc();
        let ids: Vec<usize> = items.iter().map(|_| self.alloc()).collect();
        // Parent of each entry: the latest entry with a lower level.
        let mut parent: Vec<Option<usize>> = Vec::with_capacity(items.len());
        let mut stack: Vec<(usize, u8)> = Vec::new();
        for (i, it) in items.iter().enumerate() {
            while stack.last().is_some_and(|&(_, l)| l >= it.level) {
                stack.pop();
            }
            parent.push(stack.last().map(|&(p, _)| p));
            stack.push((i, it.level));
        }
        // Each entry's children, in one pass (looking them up is quadratic).
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
        let mut top_kids = Vec::new();
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => kids[*p].push(i),
                None => top_kids.push(i),
            }
        }
        let children = |p: Option<usize>| -> &[usize] { p.map_or(&top_kids[..], |p| &kids[p][..]) };
        // Every entry is open, so /Count is the number of descendants.
        let mut count = vec![0usize; items.len()];
        for i in (0..items.len()).rev() {
            if let Some(p) = parent[i] {
                count[p] += 1 + count[i];
            }
        }
        let first_last = |kids: &[usize]| match (kids.first(), kids.last()) {
            (Some(&f), Some(&l)) => format!(" /First {} 0 R /Last {} 0 R", ids[f], ids[l]),
            _ => String::new(),
        };
        let top = children(None);
        self.obj(root, &format!("<< /Type /Outlines{} /Count {} >>", first_last(top), items.len()));
        for (i, it) in items.iter().enumerate() {
            let siblings = children(parent[i]);
            let pos = siblings.binary_search(&i).unwrap_or(0);
            let mut d = format!("<< /Title {} /Parent {} 0 R", pdf_text(&it.title), parent[i].map_or(root, |p| ids[p]));
            if pos > 0 {
                let _ = write!(d, " /Prev {} 0 R", ids[siblings[pos - 1]]);
            }
            if let Some(&n) = siblings.get(pos + 1) {
                let _ = write!(d, " /Next {} 0 R", ids[n]);
            }
            d.push_str(&first_last(children(Some(i))));
            if count[i] > 0 {
                let _ = write!(d, " /Count {}", count[i]);
            }
            let t = it.page.min(pdf.pages.len() - 1);
            let _ = write!(d, " /Dest {} >>", dest(page_ref(t), &pdf.pages[t], it.y));
            self.obj(ids[i], &d);
        }
        Some(root)
    }
}

impl std::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.out.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

fn dest(page_id: usize, page: &Page, y: f32) -> String {
    let y = if y.is_finite() { (page.height - y).clamp(0.0, page.height) } else { page.height };
    format!("[{page_id} 0 R /XYZ 0 {} null]", num(y))
}

fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// Short decimal: at most 3 decimals, no trailing zeros, finite.
fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let s = format!("{:.3}", v.clamp(-1e7, 1e7));
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".to_string() } else { s.to_string() }
}

fn color(c: [u8; 3]) -> String {
    c.map(|v| num(v as f32 / 255.0)).join(" ")
}

/// Text string as UTF-16BE with BOM (hex), valid for any Unicode.
fn pdf_text(s: &str) -> String {
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        let _ = write!(out, "{u:04X}");
    }
    out.push('>');
    out
}

/// ASCII literal string (URIs): escapes delimiters, percent-encodes the rest.
fn pdf_string_ascii(s: &str) -> String {
    let mut out = String::from("(");
    for b in s.bytes() {
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            0x21..=0x7E => out.push(b as char),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out.push(')');
    out
}

fn to_unicode(used: &BTreeMap<u16, (u16, char)>) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<(&u16, &(u16, char))> = used.iter().collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(s, "{} beginbfchar", chunk.len());
        for (code, (_, c)) in chunk {
            let mut buf = [0u16; 2];
            let hex: String = c.encode_utf16(&mut buf).iter().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(s, "<{code:04X}> <{hex}>");
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s
}

fn fnv(data: &[u8], seed: u64) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325 ^ seed, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn subset_tag(font: &Font, gids: &BTreeSet<u16>) -> String {
    let mut bytes: Vec<u8> = font.name.bytes().collect();
    bytes.extend(gids.iter().flat_map(|g| g.to_be_bytes()));
    let mut h = fnv(&bytes, font.index as u64);
    (0..6)
        .map(|_| {
            let c = (b'A' + (h % 26) as u8) as char;
            h /= 26;
            c
        })
        .collect()
}

fn file_id(body: &[u8]) -> String {
    format!("{:016X}{:016X}", fnv(body, 0), fnv(body, 0x5bd1_e995))
}

/// Current UTC time as a PDF date.
fn pdf_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(12.3456), "12.346");
        assert_eq!(num(f32::NAN), "0");
        assert_eq!(color([255, 0, 128]), "1 0 0.502");
    }

    #[test]
    fn strings() {
        assert_eq!(pdf_text("Añ"), "<FEFF004100F1>");
        assert_eq!(pdf_string_ascii("a(b) ñ"), "(a\\(b\\)%20%C3%B1)");
    }

    #[test]
    fn gradients_are_shadings() {
        let mut pdf = Pdf::new();
        pdf.begin_page(200.0, 100.0);
        let tri = |a: u8| [((0.0, 0.0), [255, 0, 0], 255), ((100.0, 0.0), [0, 0, 255], a), ((0.0, 50.0), [0, 255, 0], 255)];
        pdf.gouraud(&[tri(255)]);
        pdf.gouraud(&[tri(0)]);
        let out = pdf.finish();
        let s = String::from_utf8_lossy(&out).into_owned();
        assert_eq!(s.matches("/ShadingType 4").count(), 3, "{s}");
        assert!(s.contains("/ColorSpace /DeviceGray") && s.contains("/SMask << /S /Luminosity"));
        assert!(s.contains("/Sh1 ") && s.contains("/SM1 "));
    }

    #[test]
    fn empty_document_is_valid() {
        let out = Pdf::new().finish();
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("%PDF-1.7"));
        assert!(s.contains("/Count 1"));
        assert!(s.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        let mut pdf = Pdf::new();
        pdf.begin_page(200.0, 100.0);
        pdf.fill_rect(10.0, 10.0, 20.0, 20.0, [255, 0, 0]);
        pdf.fill_rect_alpha(10.0, 10.0, 20.0, 20.0, [0, 0, 255], 0.5);
        let img = pdf.add_image(2, 2, &[255, 0, 0, 255, 0, 255, 0, 128]);
        pdf.image(img, 0.0, 0.0, 10.0, 10.0);
        pdf.link(0.0, 0.0, 10.0, 10.0, LinkTarget::Uri("https://example.com".into()));
        pdf.outline("A", 0, 0, 0.0);
        pdf.outline("B", 1, 5, 50.0);
        pdf.outline("C", 0, 0, 0.0);
        let out = pdf.finish();
        // Binary streams make the file non-UTF-8: work on bytes.
        let s = String::from_utf8_lossy(&out).into_owned();
        let startxref: usize = s.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(out[startxref..].starts_with(b"xref"));
        let tail = String::from_utf8_lossy(&out[startxref..]).into_owned();
        let lines: Vec<&str> = tail.lines().collect();
        let n: usize = lines[1].split(' ').nth(1).unwrap().parse().unwrap();
        for (i, line) in lines[3..2 + n].iter().enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(out[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1);
            assert_eq!(line.len(), 19); // + "\n" = the required 20 bytes
        }
        assert!(s.contains("/PageMode /UseOutlines"));
        assert!(s.contains("/SMask"));
    }

    #[test]
    fn glyph_runs_and_weird_input() {
        let mut pdf = Pdf::new();
        let f = pdf.add_font(font::tests::tiny_font().into(), 0).unwrap();
        // Same bytes register once.
        assert_eq!(pdf.add_font(font::tests::tiny_font().into(), 0), Some(f));
        assert_eq!(pdf.advance(f, 'A'), 600.0);
        // Drawing before any page is ignored, not a panic.
        pdf.glyphs(f, 12.0, 10.0, &[('A', 0.0)], [0, 0, 0]);
        pdf.begin_page(f32::NAN, -5.0);
        pdf.glyphs(f, 10.0, 20.0, &[('A', 5.0), ('B', 11.0), ('x', 30.0), ('y', f32::NAN)], [0, 0, 0]);
        pdf.glyphs(f, f32::NAN, 20.0, &[('A', 5.0)], [0, 0, 0]);
        pdf.glyphs(f, 10.0, 20.0, &[], [0, 0, 0]);
        pdf.glyphs(FontId(99), 10.0, 20.0, &[('A', 0.0)], [0, 0, 0]);
        pdf.pop_clip();
        pdf.pop_transform();
        pdf.push_transform([0.0, 1.0, -1.0, 0.0, 100.0, 100.0]);
        pdf.circle(f32::INFINITY, 0.0, 5.0, Some([1, 2, 3]), None);
        pdf.polygon(&[], true, None, None);
        pdf.dashed_line(0.0, 0.0, 1.0, 1.0, 1.0, [0, 0, 0], &[0.0, f32::NAN]);
        pdf.image(ImageId(7), 0.0, 0.0, 1.0, 1.0);
        let _ = pdf.add_image(0, 0, &[]);
        let _ = pdf.add_image(u32::MAX, u32::MAX, &[1, 2, 3]);
        pdf.link(f32::NAN, 0.0, 1.0, 1.0, LinkTarget::Page { page: 9, y: 0.0 });
        pdf.outline("x", 3, 99, f32::NAN);
        assert_eq!(pdf.page_count(), 1);
        let ops = &pdf.cur.as_ref().unwrap().ops;
        // 'A' (600) at 5 puts the pen at 11 where 'B' goes: no adjustment;
        // 'B' (700) leaves it at 18 and 'x' (missing, .notdef 500) is at 30.
        assert!(ops.contains("[<00010002>-1200<00030004>]TJ"), "{ops}");
        let out = pdf.finish();
        let s = String::from_utf8_lossy(&out);
        // 'A' at 5, width 600 -> pen at 11 = next position: no adjustment.
        assert!(s.contains("/CIDFontType2"));
        assert!(s.contains("/ToUnicode"));
        // Missing 'x' and 'y' still get their own ToUnicode entries.
        let tu_pos = s.find("/ToUnicode").unwrap();
        assert!(tu_pos > 0);
    }

    #[test]
    fn jpeg_passthrough() {
        // SOI, APP14 Adobe, SOF0 8 bits 3x2 with 4 components, SOS.
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xEE, 0, 7, b'A', b'd', b'o', b'b', b'e'];
        jpg.extend_from_slice(&[0xFF, 0xC0, 0, 20, 8, 0, 2, 0, 3, 4]);
        jpg.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0]);
        jpg.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        assert_eq!(jpeg_info(&jpg), Some((3, 2, 4, true)));
        let mut pdf = Pdf::new();
        pdf.begin_page(100.0, 100.0);
        let id = pdf.add_jpeg(&jpg).unwrap();
        pdf.image(id, 0.0, 0.0, 30.0, 20.0);
        let s = String::from_utf8_lossy(&pdf.finish()).into_owned();
        assert!(s.contains("/Width 3 /Height 2 /ColorSpace /DeviceCMYK /Decode [1 0 1 0 1 0 1 0] /BitsPerComponent 8 /Filter /DCTDecode"), "{s}");
        // 12 bits, arithmetic, truncated and junk are refused.
        let mut bits12 = jpg.clone();
        bits12[15] = 12;
        let mut arith = jpg.clone();
        arith[12] = 0xC9;
        let bad: [&[u8]; 6] = [&bits12, &arith, &jpg[..14], b"\xFF\xD8\xFF", b"", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]];
        for bad in bad {
            assert_eq!(Pdf::new().add_jpeg(bad), None);
        }
    }

    #[test]
    fn transform_conjugates_the_flip() {
        let mut pdf = Pdf::new();
        pdf.begin_page(100.0, 200.0);
        // Pure translation by (10, 20) in top-left space: y down 20 means
        // PDF y down 20 too.
        pdf.push_transform([1.0, 0.0, 0.0, 1.0, 10.0, 20.0]);
        let ops = &pdf.cur.as_ref().unwrap().ops;
        assert!(ops.ends_with("1 0 0 1 10 -20 cm\n"), "{ops}");
    }
}
