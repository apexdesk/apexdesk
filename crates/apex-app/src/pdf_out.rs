//! Writes print jobs (egui shapes) as PDF with embedded fonts.

use std::collections::HashMap;
use std::sync::Arc;

use apex_pdf::{FontId, ImageId, Pdf};
use eframe::egui::{Color32, FontFamily, Mesh, Pos2, Shape, Stroke, TextureId, Vec2, emath::TSTransform, epaint::TextShape};

use crate::fonts;
use crate::imposition::Side;
use crate::print::{Job, LinkTarget, PT, Page, Paint};

/// Fonts registered in the PDF, found through egui's family chains.
#[derive(Default)]
struct Fonts {
    by_name: HashMap<String, Option<FontId>>,
    chains: HashMap<FontFamily, Vec<FontId>>,
}

impl Fonts {
    fn chain(&mut self, pdf: &mut Pdf, family: &FontFamily) -> &[FontId] {
        if !self.chains.contains_key(family) {
            let mut ids = Vec::new();
            for (name, data) in fonts::chain(family) {
                let id = *self.by_name.entry(name).or_insert_with(|| pdf.add_font(Arc::from(&*data.font), data.index));
                if let Some(id) = id {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            self.chains.insert(family.clone(), ids);
        }
        &self.chains[family]
    }

    /// The first font of the family that has `c` (else the first one).
    fn pick(&mut self, pdf: &mut Pdf, family: &FontFamily, c: char) -> Option<FontId> {
        let chain = self.chain(pdf, family).to_vec();
        chain.iter().copied().find(|f| pdf.has_char(*f, c)).or(chain.first().copied())
    }
}

fn rgb(c: Color32) -> [u8; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r, g, b]
}

fn visible(c: Color32) -> bool {
    c.a() > 0
}

/// A PDF written a page at a time: the pages of a big job are painted,
/// written and dropped one by one (see [`Job::page`]).
pub struct Out {
    pdf: Pdf,
    fonts: Fonts,
    /// Pixels to points.
    k: f32,
    /// Pictures already in the PDF, by texture.
    /// Pictures written, by texture and the part of it shown (none when
    /// the whole), with the part they hold (whole pixels about it).
    images: HashMap<(TextureId, Option<[u32; 4]>), Option<(ImageId, [f32; 4])>>,
    /// Where each page of the job it holds lands: the side of paper in the
    /// PDF, and its placement there (pages per sheet, booklets).
    index: HashMap<usize, (usize, TSTransform)>,
    /// Alternative texts of pictures (tagged PDF), and pictures so far.
    alts: HashMap<u64, String>,
    figures: u64,
}

/// Writes `pages` (indexes into the job) as a PDF document; the pages the
/// job paints when needed are painted by `paint`, one at a time.
pub fn to_pdf(job: &Job, pages: &[usize], paint: &mut Paint<'_>) -> Vec<u8> {
    let mut out = Out::new(job, pages);
    for &pi in pages {
        if let Some(page) = job.page(pi, paint) {
            out.page(&page);
        }
    }
    out.finish(job)
}

impl Out {
    pub fn new(job: &Job, pages: &[usize]) -> Out {
        let sides: Vec<Side> = pages.iter().map(|&p| Side::Page(p)).collect();
        Out::with_sides(job, &sides, Vec2::ZERO)
    }

    /// A PDF of `sides`; `paper` is the size of the job's pages, to place
    /// the targets of links and bookmarks on the sheets.
    pub fn with_sides(job: &Job, sides: &[Side], paper: Vec2) -> Out {
        let mut pdf = Pdf::new();
        pdf.set_info(&job.title, &job.author);
        pdf.set_pdfa(job.pdf.pdfa);
        // Pages placed several to a sheet are not tagged.
        if job.pdf.tagged && sides.iter().all(|s| matches!(s, Side::Page(_))) {
            pdf.set_tagged(&job.lang);
        }
        let mut index = HashMap::new();
        for (i, s) in sides.iter().enumerate() {
            match s {
                Side::Page(p) => {
                    index.entry(*p).or_insert((i, TSTransform::IDENTITY));
                }
                Side::Sheet { cells, .. } => {
                    for (p, cell) in cells {
                        if let Some(p) = p {
                            index.entry(*p).or_insert((i, crate::imposition::fit(*cell, paper)));
                        }
                    }
                }
            }
        }
        Out { pdf, fonts: Fonts::default(), k: 1.0 / PT, images: HashMap::new(), index, alts: job.alts.clone(), figures: 0 }
    }

    /// Writes the next side of paper, painting its pages with `paint`.
    pub fn side(&mut self, job: &Job, side: &Side, paint: &mut Paint<'_>) {
        match side {
            Side::Page(p) => {
                if let Some(page) = job.page(*p, paint) {
                    self.page(&page);
                }
            }
            Side::Sheet { size, cells } => {
                let k = self.k;
                self.pdf.begin_page(size.x * k, size.y * k);
                for (p, cell) in cells {
                    let Some(page) = p.and_then(|p| job.page(p, paint)) else { continue };
                    let t = crate::imposition::fit(*cell, page.size);
                    self.pdf.push_transform([t.scaling, 0.0, 0.0, t.scaling, t.translation.x * k, t.translation.y * k]);
                    self.pdf.push_clip(0.0, 0.0, page.size.x * k, page.size.y * k);
                    self.shapes(&page);
                    self.pdf.pop_clip();
                    self.pdf.pop_transform();
                    self.links(&page, t);
                }
            }
        }
    }

    /// Writes the next page.
    pub fn page(&mut self, page: &Page) {
        let k = self.k;
        self.pdf.begin_page(page.size.x * k, page.size.y * k);
        self.shapes(page);
        self.links(page, TSTransform::IDENTITY);
    }

    fn shapes(&mut self, page: &Page) {
        let k = self.k;
        for c in &page.shapes {
            let bounds = c.shape.visual_bounding_rect();
            let clip = c.clip_rect;
            if !bounds.is_positive() && !matches!(c.shape, Shape::LineSegment { .. }) {
                continue;
            }
            if clip.intersect(bounds).is_negative() {
                continue;
            }
            if self.pdf.is_tagged() {
                self.mark(page, &c.shape, bounds);
            }
            let clipped = !clip.contains_rect(bounds) && clip.is_finite();
            if clipped {
                self.pdf.push_clip(clip.min.x * k, clip.min.y * k, clip.width() * k, clip.height() * k);
            }
            self.shape(&c.shape);
            if clipped {
                self.pdf.pop_clip();
            }
        }
    }

    /// Tagged PDF: pictures are figures, what lies in a part of the
    /// structure belongs to it, the rest is decoration.
    fn mark(&mut self, page: &Page, shape: &Shape, bounds: eframe::egui::Rect) {
        if let Shape::Mesh(m) = shape
            && m.texture_id != TextureId::default()
        {
            let alt = crate::images::source(m.texture_id).and_then(|b| self.alts.get(&crate::images::key(&b)).cloned());
            self.figures += 1;
            self.pdf.tag(1 << 63 | self.figures, "Figure", alt.as_deref());
            return;
        }
        let c = bounds.center();
        match page.tags.iter().find(|t| t.rect.contains(c)) {
            Some(t) => self.pdf.tag(t.key, t.role, None),
            None => self.pdf.artifact(),
        }
    }

    /// The links of a page placed on its side by `t`.
    fn links(&mut self, page: &Page, t: TSTransform) {
        let k = self.k;
        for (r, target) in &page.links {
            let target = match target {
                LinkTarget::Uri(u) => apex_pdf::LinkTarget::Uri(u.clone()),
                LinkTarget::Page(p, y) => match self.index.get(p) {
                    Some(&(i, pt)) => apex_pdf::LinkTarget::Page { page: i, y: (pt.translation.y + y * pt.scaling) * k },
                    None => continue,
                },
            };
            let r = t.mul_rect(*r);
            self.pdf.link(r.min.x * k, r.min.y * k, r.width() * k, r.height() * k, target);
        }
    }

    /// Adds the bookmarks and returns the document.
    pub fn finish(mut self, job: &Job) -> Vec<u8> {
        for (title, level, page, y) in &job.outline {
            if let Some(&(i, t)) = self.index.get(page) {
                self.pdf.outline(title, *level, i, (t.translation.y + y * t.scaling) * self.k);
            }
        }
        self.pdf.finish()
    }

    fn p(&self, p: Pos2) -> (f32, f32) {
        (p.x * self.k, p.y * self.k)
    }

    fn stroke_of(&self, s: Stroke) -> Option<(f32, [u8; 3])> {
        (s.width > 0.0 && visible(s.color)).then(|| (s.width * self.k, rgb(s.color)))
    }

    fn shape(&mut self, shape: &Shape) {
        let k = self.k;
        match shape {
            Shape::Vec(v) => {
                for s in v {
                    self.shape(s);
                }
            }
            Shape::Circle(c) => {
                let (x, y) = self.p(c.center);
                let fill = visible(c.fill).then(|| rgb(c.fill));
                self.pdf.circle(x, y, c.radius * k, fill, self.stroke_of(c.stroke));
            }
            Shape::Ellipse(e) => {
                let pts: Vec<(f32, f32)> = (0..48)
                    .map(|i| {
                        let a = i as f32 / 48.0 * std::f32::consts::TAU;
                        self.p(e.center + eframe::egui::vec2(a.cos() * e.radius.x, a.sin() * e.radius.y))
                    })
                    .collect();
                let fill = visible(e.fill).then(|| rgb(e.fill));
                self.pdf.polygon(&pts, true, fill, self.stroke_of(e.stroke));
            }
            Shape::LineSegment { points, stroke } => {
                if let Some((w, c)) = self.stroke_of(*stroke) {
                    let (a, b) = (self.p(points[0]), self.p(points[1]));
                    self.pdf.line(a.0, a.1, b.0, b.1, w, c);
                }
            }
            Shape::Path(ps) => {
                let pts: Vec<(f32, f32)> = ps.points.iter().map(|p| self.p(*p)).collect();
                // See-through fills (picture shadows and glows).
                if ps.closed && visible(ps.fill) && ps.fill.a() < 255 {
                    self.pdf.fill_polygon_alpha(&pts, rgb(ps.fill), ps.fill.a() as f32 / 255.0);
                    if let eframe::egui::epaint::ColorMode::Solid(c) = ps.stroke.color
                        && ps.stroke.width > 0.0
                        && visible(c)
                    {
                        self.pdf.polygon(&pts, true, None, Some((ps.stroke.width * k, rgb(c))));
                    }
                    return;
                }
                // See-through lines (pencil and highlighter ink).
                if let eframe::egui::epaint::ColorMode::Solid(c) = ps.stroke.color
                    && !ps.closed
                    && !visible(ps.fill)
                    && ps.stroke.width > 0.0
                    && visible(c)
                    && c.a() < 255
                {
                    self.pdf.stroke_path_alpha(&pts, ps.stroke.width * k, rgb(c), c.a() as f32 / 255.0, false);
                    return;
                }
                let fill = visible(ps.fill).then(|| rgb(ps.fill));
                let stroke = match ps.stroke.color {
                    eframe::egui::epaint::ColorMode::Solid(c) if ps.stroke.width > 0.0 && visible(c) => Some((ps.stroke.width * k, rgb(c))),
                    _ => None,
                };
                if pts.len() >= 2 && (fill.is_some() || stroke.is_some()) {
                    self.pdf.polygon(&pts, ps.closed, fill, stroke);
                }
            }
            Shape::Rect(rs) => {
                let r = rs.rect;
                if visible(rs.fill) {
                    let (x, y) = self.p(r.min);
                    if rs.fill.a() < 255 {
                        let a = rs.fill.a() as f32 / 255.0;
                        self.pdf.fill_rect_alpha(x, y, r.width() * k, r.height() * k, rgb(rs.fill), a);
                    } else {
                        self.pdf.fill_rect(x, y, r.width() * k, r.height() * k, rgb(rs.fill));
                    }
                }
                if let Some((w, c)) = self.stroke_of(rs.stroke) {
                    let half = rs.stroke.width / 2.0;
                    let r = match rs.stroke_kind {
                        eframe::egui::StrokeKind::Inside => r.shrink(half),
                        eframe::egui::StrokeKind::Middle => r,
                        eframe::egui::StrokeKind::Outside => r.expand(half),
                    };
                    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|p| self.p(p));
                    self.pdf.polygon(&pts, true, None, Some((w, c)));
                }
            }
            Shape::Text(t) => self.text(t),
            Shape::QuadraticBezier(b) => {
                let pts: Vec<(f32, f32)> = b.flatten(Some(0.1)).into_iter().map(|p| self.p(p)).collect();
                let fill = visible(b.fill).then(|| rgb(b.fill));
                let stroke = match b.stroke.color {
                    eframe::egui::epaint::ColorMode::Solid(c) if b.stroke.width > 0.0 && visible(c) => Some((b.stroke.width * k, rgb(c))),
                    _ => None,
                };
                self.pdf.polygon(&pts, b.closed, fill, stroke);
            }
            Shape::CubicBezier(b) => {
                let pts: Vec<(f32, f32)> = b.flatten(Some(0.1)).into_iter().map(|p| self.p(p)).collect();
                let fill = visible(b.fill).then(|| rgb(b.fill));
                let stroke = match b.stroke.color {
                    eframe::egui::epaint::ColorMode::Solid(c) if b.stroke.width > 0.0 && visible(c) => Some((b.stroke.width * k, rgb(c))),
                    _ => None,
                };
                self.pdf.polygon(&pts, b.closed, fill, stroke);
            }
            // Letters drawn from the font texture (a reflection) have no
            // PDF form; plain meshes use its white texel only.
            Shape::Mesh(m) if m.texture_id == TextureId::default() && m.vertices.iter().any(|v| v.uv != eframe::egui::epaint::WHITE_UV) => {}
            Shape::Mesh(m) if m.texture_id == TextureId::default() => self.flat_mesh(m),
            Shape::Mesh(m) => self.picture(m),
            // Callbacks have no PDF form.
            _ => {}
        }
    }

    /// An untextured mesh (chart slices): filled by its outline, one
    /// polygon per closed loop of edges that belong to a single triangle,
    /// so no seams show between the triangles.
    fn flat_mesh(&mut self, m: &Mesh) {
        let Some(first) = m.vertices.first() else { return };
        if m.vertices.iter().any(|v| v.color != first.color) {
            self.shaded_mesh(m);
            return;
        }
        if !visible(first.color) || m.indices.len() > 300_000 {
            return;
        }
        let n = m.vertices.len() as u32;
        let mut count: HashMap<(u32, u32), u32> = HashMap::new();
        for t in m.indices.chunks_exact(3) {
            if t.iter().any(|&i| i >= n) {
                return;
            }
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        // Boundary edges, walked into loops.
        let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&(a, b), &c) in &count {
            if c == 1 {
                next.entry(a).or_default().push(b);
                next.entry(b).or_default().push(a);
            }
        }
        let color = rgb(first.color);
        let mut starts: Vec<u32> = next.keys().copied().collect();
        starts.sort_unstable();
        for start in starts {
            if next.get(&start).is_none_or(|v| v.is_empty()) {
                continue;
            }
            let mut pts = Vec::new();
            let (mut prev, mut cur) = (u32::MAX, start);
            loop {
                pts.push(self.p(m.vertices[cur as usize].pos));
                let Some(list) = next.get_mut(&cur) else { break };
                let Some(pos) = list.iter().position(|&v| v != prev).or(if list.is_empty() { None } else { Some(0) }) else { break };
                let to = list.swap_remove(pos);
                if let Some(back) = next.get_mut(&to) {
                    if let Some(i) = back.iter().position(|&v| v == cur) {
                        back.swap_remove(i);
                    }
                }
                prev = cur;
                cur = to;
                if cur == start || pts.len() > m.vertices.len() {
                    break;
                }
            }
            if pts.len() >= 3 {
                self.pdf.polygon(&pts, true, Some(color), None);
            }
        }
    }

    /// A mesh whose vertices differ in color (gradients): one smooth
    /// shading (the triangles' own colors for very big meshes).
    fn shaded_mesh(&mut self, m: &Mesh) {
        if m.indices.len() > 300_000 {
            return;
        }
        let n = m.vertices.len() as u32;
        if m.indices.iter().all(|&i| i < n) && m.indices.len() <= 60_000 {
            let tris: Vec<[((f32, f32), [u8; 3], u8); 3]> = m
                .indices
                .chunks_exact(3)
                .map(|t| [t[0], t[1], t[2]].map(|i| {
                    let c = m.vertices[i as usize].color.to_srgba_unmultiplied();
                    (self.p(m.vertices[i as usize].pos), [c[0], c[1], c[2]], c[3])
                }))
                .collect();
            self.pdf.gouraud(&tris);
            return;
        }
        for t in m.indices.chunks_exact(3) {
            if t.iter().any(|&i| i >= n) {
                return;
            }
            let vs = [&m.vertices[t[0] as usize], &m.vertices[t[1] as usize], &m.vertices[t[2] as usize]];
            let mut sum = [0.0f32; 4];
            for v in vs {
                let c = v.color.to_srgba_unmultiplied();
                for k in 0..4 {
                    sum[k] += c[k] as f32 / 3.0;
                }
            }
            if sum[3] < 0.5 {
                continue;
            }
            let color = [sum[0].round() as u8, sum[1].round() as u8, sum[2].round() as u8];
            let pts: Vec<(f32, f32)> = vs.iter().map(|v| self.p(v.pos)).collect();
            if sum[3] >= 254.5 {
                self.pdf.polygon(&pts, true, Some(color), Some((0.3, color)));
            } else {
                self.pdf.fill_polygon_alpha(&pts, color, sum[3] / 255.0);
            }
        }
    }

    /// A textured mesh: a quad from `images::paint` (the picture, placed by
    /// the affine map from texture to page coordinates, clipped to its
    /// crop), or a shape filled with a picture (clipped to its triangles).
    /// Texture coordinates past 0..1 repeat the picture (tiles).
    fn picture(&mut self, m: &Mesh) {
        let quad = m.vertices.len() == 4 && m.indices.len() == 6;
        if m.vertices.len() < 3 || m.indices.len() < 3 || m.indices.len() > 60_000 || m.indices.iter().any(|&i| i as usize >= m.vertices.len()) {
            return;
        }
        let v = &m.vertices;
        let (u0, u1) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.uv.x), b.max(p.uv.x)));
        let (v0, v1) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.uv.y), b.max(p.uv.y)));
        // A cropped picture is cut to what shows: the file would carry the
        // rest (and a JPEG's metadata).
        const EDGE: f32 = 1e-3;
        let part = (quad && u0 >= -EDGE && v0 >= -EDGE && u1 <= 1.0 + EDGE && v1 <= 1.0 + EDGE && (u0 > EDGE || v0 > EDGE || u1 < 1.0 - EDGE || v1 < 1.0 - EDGE)).then(|| [u0.max(0.0), v0.max(0.0), u1.min(1.0), v1.min(1.0)]);
        let key = (m.texture_id, part.map(|p| p.map(f32::to_bits)));
        let img = match self.images.get(&key) {
            Some(i) => *i,
            None => {
                const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
                let i = match (crate::images::source(m.texture_id), part) {
                    (Some(bytes), None) => self.pdf.add_jpeg(&apex_io::picture::strip_jpeg(&bytes).unwrap_or_else(|| bytes.to_vec())).or_else(|| {
                        let d = crate::images::decode(&bytes)?;
                        Some(self.pdf.add_image(d.width, d.height, &d.rgba))
                    }).map(|i| (i, WHOLE)),
                    (bytes, _) => {
                        // Pictures changed by effects have no file.
                        let d = match bytes {
                            Some(b) => crate::images::decode(&b),
                            None => crate::images::pixels(m.texture_id),
                        };
                        d.and_then(|d| match part {
                            None => Some((self.pdf.add_image(d.width, d.height, &d.rgba), WHOLE)),
                            Some([a, b, c, e]) => {
                                let (w, h) = (d.width as usize, d.height as usize);
                                let (x0, x1) = ((a * w as f32).floor() as usize, ((c * w as f32).ceil() as usize).min(w));
                                let (y0, y1) = ((b * h as f32).floor() as usize, ((e * h as f32).ceil() as usize).min(h));
                                if x1 <= x0 || y1 <= y0 || d.rgba.len() < w * h * 4 {
                                    return None;
                                }
                                let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
                                for y in y0..y1 {
                                    px.extend_from_slice(&d.rgba[(y * w + x0) * 4..(y * w + x1) * 4]);
                                }
                                let held = [x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32];
                                Some((self.pdf.add_image((x1 - x0) as u32, (y1 - y0) as u32, &px), held))
                            }
                        })
                    }
                };
                self.images.insert(key, i);
                i
            }
        };
        let Some((img, held)) = img else { return };
        // Map (u, v) -> page from the three vertices spanning the most.
        let mut best: Option<(f32, [usize; 3])> = None;
        let triples: Vec<[usize; 3]> = if quad {
            vec![[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]]
        } else {
            m.indices.chunks_exact(3).map(|t| [t[0] as usize, t[1] as usize, t[2] as usize]).collect()
        };
        for t in triples {
            let (e1, e2) = (v[t[1]].uv - v[t[0]].uv, v[t[2]].uv - v[t[0]].uv);
            let det = e1.x * e2.y - e2.x * e1.y;
            if best.is_none_or(|b| det.abs() > b.0) {
                best = Some((det.abs(), t));
            }
        }
        let Some((det, [i0, i1, i2])) = best else { return };
        if det < 1e-9 {
            return;
        }
        let (e1, e2) = (v[i1].uv - v[i0].uv, v[i2].uv - v[i0].uv);
        let (d1, d2) = (v[i1].pos - v[i0].pos, v[i2].pos - v[i0].pos);
        let det = e1.x * e2.y - e2.x * e1.y;
        // [au av] = [d1 d2] * inverse([e1 e2]).
        let au = (d1 * e2.y - d2 * e1.y) / det * self.k;
        let av = (d2 * e1.x - d1 * e2.x) / det * self.k;
        let origin = (v[i0].pos.to_vec2() * self.k) - au * v[i0].uv.x - av * v[i0].uv.y;
        let (sw, sh) = (au.length(), av.length());
        if !(sw > 1e-6 && sh > 1e-6 && origin.is_finite()) {
            return;
        }
        // Picture space: the whole picture spans (0, 0)..(sw, sh).
        let (ex, ey): (Vec2, Vec2) = (au / sw, av / sh);
        if !quad {
            let tris: Vec<Vec<(f32, f32)>> = m.indices.chunks_exact(3).map(|t| t.iter().map(|&i| self.p(v[i as usize].pos)).collect()).collect();
            self.pdf.push_clip_polygons(&tris);
        }
        self.pdf.push_transform([ex.x, ex.y, ey.x, ey.y, origin.x, origin.y]);
        if quad {
            self.pdf.push_clip(u0 * sw, v0 * sh, (u1 - u0) * sw, (v1 - v0) * sh);
        }
        // One copy per tile the texture coordinates reach.
        let (i0, i1) = (u0.floor() as i32, (u1.ceil() as i32).max(u0.floor() as i32 + 1));
        let (j0, j1) = (v0.floor() as i32, (v1.ceil() as i32).max(v0.floor() as i32 + 1));
        if part.is_some() {
            // The part cut out goes where it was (the clip trims the
            // fraction of a pixel about it).
            let [a, b, c, e] = held;
            self.pdf.image(img, a * sw, b * sh, (c - a) * sw, (e - b) * sh);
        } else if ((i1 - i0) as i64) * ((j1 - j0) as i64) <= 2500 {
            for j in j0..j1 {
                for i in i0..i1 {
                    self.pdf.image(img, i as f32 * sw, j as f32 * sh, sw, sh);
                }
            }
        }
        if quad {
            self.pdf.pop_clip();
        }
        self.pdf.pop_transform();
        if !quad {
            self.pdf.pop_clip();
        }
    }

    fn text(&mut self, t: &TextShape) {
        let k = self.k;
        let g = &t.galley;
        let rotated = t.angle != 0.0;
        if rotated {
            // Rotate about the galley origin (the pivot egui uses).
            let (s, c) = t.angle.sin_cos();
            let (ox, oy) = self.p(t.pos);
            self.pdf.push_transform([c, s, -s, c, ox - c * ox + s * oy, oy - s * ox - c * oy]);
        }
        let text = &g.job.text;
        let sections = &g.job.sections;
        let color_of = |c: Color32| if c == Color32::PLACEHOLDER { t.fallback_color } else { c };
        // Byte offset of every char, to find each glyph's section.
        let bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        let mut ci = 0usize;
        let mut si = 0usize;
        // (font, size, color, skew, baseline y, alpha) and the glyphs of
        // the run.
        type Key = (FontId, u32, [u8; 3], bool, u32, u8);
        let mut run: Option<(Key, Vec<(char, f32)>)> = None;
        let mut lines: Vec<(f32, f32, f32, Stroke)> = Vec::new();
        let flush = |run: &mut Option<(Key, Vec<(char, f32)>)>, pdf: &mut Pdf| {
            if let Some(((font, size, color, skew, y, alpha), glyphs)) = run.take() {
                let (size, y) = (f32::from_bits(size), f32::from_bits(y));
                if alpha < 255 {
                    pdf.glyphs_alpha(font, size, y, &glyphs, color, if skew { 0.2 } else { 0.0 }, f32::from(alpha) / 255.0);
                } else if skew {
                    pdf.glyphs_skewed(font, size, y, &glyphs, color, 0.2);
                } else {
                    pdf.glyphs(font, size, y, &glyphs, color);
                }
            }
        };
        for row in &g.rows {
            let mut prev: Option<(FontId, char, f32, f32)> = None;
            let origin = t.pos + row.pos.to_vec2();
            // Section backgrounds first.
            for (gi, glyph) in row.row.glyphs.iter().enumerate() {
                let b = bytes.get(ci + gi).copied().unwrap_or(text.len());
                let Some(sec) = sections.iter().find(|s| (s.byte_range.start.0..s.byte_range.end.0).contains(&b)) else { continue };
                let bg = color_of(sec.format.background);
                if visible(bg) {
                    let r = glyph.logical_rect().translate(origin.to_vec2());
                    self.pdf.fill_rect(r.min.x * k, r.min.y * k, r.width() * k, r.height() * k, rgb(bg));
                }
            }
            let vis = &row.row.visuals;
            for (gi, glyph) in row.row.glyphs.iter().enumerate() {
                let b = bytes.get(ci).copied().unwrap_or(text.len());
                ci += 1;
                while si + 1 < sections.len() && sections[si].byte_range.end.0 <= b {
                    si += 1;
                }
                let Some(sec) = sections.get(si) else { continue };
                let f = &sec.format;
                let color = t.override_text_color.unwrap_or_else(|| color_of(f.color));
                // Colors set on the glyph's mesh (artistic text fills and
                // effects) win over the section's.
                let v0 = glyph.first_vertex as usize;
                let v1 = row.row.glyphs.get(gi + 1).map_or(vis.glyph_vertex_range.end, |x| x.first_vertex as usize);
                let mesh_color = (t.override_text_color.is_none() && v1 > v0).then(|| vis.mesh.vertices.get(v0).map(|v| v.color)).flatten().filter(|&c| c != Color32::PLACEHOLDER && c != f.color);
                let (color, alpha) = match mesh_color {
                    Some(c) => {
                        let u = c.to_srgba_unmultiplied();
                        (Color32::from_rgb(u[0], u[1], u[2]), u[3])
                    }
                    None => (color, 255),
                };
                if alpha == 0 {
                    continue;
                }
                let r = glyph.logical_rect().translate(origin.to_vec2());
                for (stroke, y) in [(f.underline, r.bottom()), (f.strikethrough, r.center().y)] {
                    if stroke.width > 0.0 {
                        let stroke = Stroke::new(stroke.width, color_of(stroke.color));
                        match lines.last_mut() {
                            Some(l) if l.2 == y && l.3 == stroke && (l.1 - r.min.x).abs() < 0.5 => l.1 = r.max.x,
                            _ => lines.push((r.min.x, r.max.x, y, stroke)),
                        }
                    }
                }
                if glyph.chr.is_whitespace() && glyph.chr != ' ' || !visible(color) {
                    continue;
                }
                let Some(font) = self.fonts.pick(&mut self.pdf, &f.font_id.family, glyph.chr) else { continue };
                let mut x = (origin.x + glyph.pos.x) * k;
                // egui shapes text: a ligature ("fi", "ti") is drawn with the
                // first char's glyph and the rest get no width. We have no
                // ligature glyphs, so each char goes after the previous one.
                if glyph.advance_width == 0.0 && glyph.chr.is_alphanumeric() {
                    if let Some((pf, pc, px, ps)) = prev {
                        x = px + self.pdf.advance(pf, pc) * ps / 1000.0;
                    }
                }
                prev = Some((font, glyph.chr, x, f.font_id.size * k));
                let y = (origin.y + glyph.pos.y) * k;
                let key: Key = (font, (f.font_id.size * k).to_bits(), rgb(color), f.italics, y.to_bits(), alpha);
                if g.job.vertical && eframe::egui::epaint::text::upright_in_vertical(glyph.chr) {
                    // Vertical text: turned back upright about the middle
                    // of its em box (as the galley's mesh has it).
                    flush(&mut run, &mut self.pdf);
                    let (cx, cy) = (x + glyph.advance_width * k / 2.0, y - eframe::egui::epaint::text::EM_BOX_MIDDLE * f.font_id.size * k);
                    self.pdf.push_transform([0.0, -1.0, 1.0, 0.0, cx - cy, cy + cx]);
                    run = Some((key, vec![(glyph.chr, x)]));
                    flush(&mut run, &mut self.pdf);
                    self.pdf.pop_transform();
                    continue;
                }
                match &mut run {
                    Some((rk, glyphs)) if *rk == key => glyphs.push((glyph.chr, x)),
                    _ => {
                        flush(&mut run, &mut self.pdf);
                        run = Some((key, vec![(glyph.chr, x)]));
                    }
                }
            }
            flush(&mut run, &mut self.pdf);
            if row.ends_with_newline {
                ci += 1;
            }
        }
        for (x0, x1, y, s) in lines {
            self.pdf.line(x0 * k, y * k, x1 * k, y * k, s.width * k, rgb(s.color));
        }
        if t.underline.width > 0.0 && visible(t.underline.color) {
            let r = g.rect.translate(t.pos.to_vec2());
            self.pdf.line(r.min.x * k, r.max.y * k, r.max.x * k, r.max.y * k, t.underline.width * k, rgb(t.underline.color));
        }
        if rotated {
            self.pdf.pop_transform();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::{Page, capture};
    use eframe::egui::{Context, Rect, pos2, vec2};

    #[test]
    fn pictures_reach_the_pdf() {
        let ctx = Context::default();
        let mut png = Vec::new();
        {
            let mut e = png::Encoder::new(&mut png, 2, 2);
            e.set_color(png::ColorType::Rgba);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 128]).unwrap();
        }
        let bytes: Arc<[u8]> = png.into();
        let tex = crate::images::texture(&ctx, crate::images::key(&bytes), &bytes, true);
        assert!(tex.is_some());
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0));
        let shapes = capture(&ctx, |p| crate::images::paint(p, tex.as_ref(), rect, 30.0, [0.25, 0.0, 0.0, 0.5]));
        let job = Job {
            pages: vec![Page { size: vec2(800.0, 1000.0), shapes, links: Vec::new(), tags: Vec::new() }],
            ..Default::default()
        };
        let pdf = to_pdf(&job, &[0], &mut crate::print::no_paint);
        let s = String::from_utf8_lossy(&pdf);
        // Cropped: only the part shown (whole pixels) is in the file.
        assert!(s.contains("/Subtype /Image /Width 2 /Height 1"), "{s}");
        assert!(!s.contains("/Height 2 "));
        // The same picture twice is stored once; a gray placeholder (no
        // texture) adds nothing.
        let shapes = capture(&ctx, |p| {
            crate::images::paint(p, tex.as_ref(), rect, 0.0, [0.0; 4]);
            crate::images::paint(p, tex.as_ref(), rect.translate(vec2(0.0, 300.0)), 90.0, [0.0; 4]);
            crate::images::paint(p, None, rect, 0.0, [0.0; 4]);
        });
        let job = Job { pages: vec![Page { size: vec2(800.0, 1000.0), shapes, links: Vec::new(), tags: Vec::new() }], ..Default::default() };
        let s = String::from_utf8_lossy(&to_pdf(&job, &[0], &mut crate::print::no_paint)).into_owned();
        assert_eq!(s.matches("/Height 2 /ColorSpace /DeviceRGB").count(), 1);

        // Tagged PDF/A: the picture is a figure with its alternative text,
        // the rectangle in the heading's place a heading, the other one
        // decoration.
        let shapes = capture(&ctx, |p| {
            crate::images::paint(p, tex.as_ref(), rect, 0.0, [0.0; 4]);
            p.rect_filled(Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 20.0)), 0.0, Color32::RED);
            p.rect_filled(Rect::from_min_size(pos2(10.0, 900.0), vec2(50.0, 20.0)), 0.0, Color32::BLUE);
        });
        let tags = vec![crate::print::Tag { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 40.0)), role: "H1", key: 0 }];
        let mut job = Job { pages: vec![Page { size: vec2(800.0, 1000.0), shapes, links: Vec::new(), tags }], lang: "es-ES".into(), ..Default::default() };
        job.alts.insert(crate::images::key(&bytes), "Un gato".into());
        job.pdf = crate::print::PdfOptions { pdfa: true, tagged: true };
        let s = String::from_utf8_lossy(&to_pdf(&job, &[0], &mut crate::print::no_paint)).into_owned();
        for part in ["/S /Figure", "/S /H1", "/Lang (es-ES)", "/GTS_PDFA1", "pdfaid:part"] {
            assert!(s.contains(part), "{part}");
        }
        let alt: String = "Un gato".encode_utf16().map(|u| format!("{u:04X}")).collect();
        assert!(s.contains(&alt));
    }

    #[test]
    fn pages_per_sheet_and_booklets() {
        use crate::imposition::{Layout, sides};
        let ctx = Context::default();
        let page = |n: usize| {
            let shapes = capture(&ctx, |p| {
                p.rect_filled(Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 50.0)), 0.0, eframe::egui::Color32::RED);
            });
            Page { size: vec2(800.0, 1000.0), shapes, tags: Vec::new(), links: vec![(Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)), LinkTarget::Page((n + 1) % 5, 0.0))] }
        };
        let job = Job { pages: (0..5).map(page).collect(), outline: vec![("Five".into(), 0, 4, 0.0)], ..Default::default() };
        let pages: Vec<usize> = (0..5).collect();
        let write = |layout: Layout| {
            let s = sides(&pages, layout, vec2(800.0, 1000.0));
            let mut out = Out::with_sides(&job, &s, vec2(800.0, 1000.0));
            for side in &s {
                out.side(&job, side, &mut crate::print::no_paint);
            }
            String::from_utf8_lossy(&out.finish(&job)).into_owned()
        };
        // 4 per sheet: 2 sides, each page scaled in a quarter.
        let s = write(Layout { per_sheet: 4, ..Layout::default() });
        assert!(s.contains("/Count 2"), "{s}");
        let media = |s: &str| -> (f32, f32) {
            let i = s.find("/MediaBox [0 0 ").unwrap() + 15;
            let mut it = s[i..].split([' ', ']']).map(|v| v.parse::<f32>().unwrap());
            (it.next().unwrap(), it.next().unwrap())
        };
        assert!(media(&s).0 < media(&s).1);
        // A booklet of 5 pages: 8 pages on 4 sides of turned paper.
        let s = write(Layout { booklet: true, ..Layout::default() });
        assert!(s.contains("/Count 4"), "{s}");
        assert!(media(&s).0 > media(&s).1);
    }

    #[test]
    fn see_through_ink_reaches_the_pdf() {
        use apex_doc::{Ink, InkStroke, InkTool};
        let ctx = Context::default();
        let stroke = |tool, pts: Vec<(f32, f32)>| InkStroke { points: pts, ..InkStroke::new(tool, 0x3050A0, 4.0) };
        let ink = Ink::new(vec![stroke(InkTool::Highlighter, vec![(0.1, 0.5), (0.9, 0.5)]), stroke(InkTool::Pencil, vec![(0.1, 0.1), (0.5, 0.9), (0.9, 0.1)]), stroke(InkTool::Pen, vec![(0.1, 0.9), (0.9, 0.9)])]);
        let shape = apex_doc::Shape::ink(ink);
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0));
        let shapes = capture(&ctx, |p| crate::shapes::paint(p, rect, &shape, 1.0, true));
        let job = Job { pages: vec![Page { size: vec2(800.0, 1000.0), shapes, links: Vec::new(), tags: Vec::new() }], ..Default::default() };
        let s = String::from_utf8_lossy(&to_pdf(&job, &[0], &mut crate::print::no_paint)).into_owned();
        assert!(s.contains("/CA 0.4") && s.contains("/CA 0.851"), "{s}");
    }
}
