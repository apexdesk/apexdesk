//! Shadings (smooth colour fills) drawn into pixels of the page.

use vello_cpu::kurbo::{Affine, Point};

use super::super::file::PdfFile;
use super::super::obj::{Dict, Obj};
use super::color::{Cs, Func};

pub struct Shading {
    kind: i64,
    cs: Cs,
    func: Option<Func>,
    coords: Vec<f64>,
    domain: [f64; 2],
    extend: [bool; 2],
    /// Type 1: the domain of x and y, and the matrix to shading space.
    domain2: [f64; 4],
    matrix: Affine,
    pub bbox: Option<[f64; 4]>,
    /// Meshes: triangles with a colour at each corner.
    tris: Vec<[(Point, Vec<f32>); 3]>,
    /// Axial and radial: the colours along the domain, made once.
    lut: std::cell::OnceCell<Vec<[f32; 3]>>,
}

impl Shading {
    pub fn load(f: &PdfFile, o: &Obj, res: Option<&Dict>) -> Option<Shading> {
        let (d, stream) = match f.resolve(o) {
            Obj::Dict(d) => (d, None),
            Obj::Stream(d, raw) => {
                let data = f.decode(&d, &raw).data;
                (d, Some(data))
            }
            _ => return None,
        };
        let kind = f.key(&d, "ShadingType").int()?;
        let cs = Cs::load(f, &f.key(&d, "ColorSpace"), res);
        let func = match f.key(&d, "Function") {
            Obj::Null => None,
            o => Func::load(f, &o),
        };
        // Function-based, axial and radial ones can't be drawn without one
        // (pdfium draws nothing).
        if (1..=3).contains(&kind) && func.is_none() {
            return None;
        }
        let coords = f.nums(d.get("Coords").unwrap_or(&Obj::Null));
        let dm = f.nums(d.get("Domain").unwrap_or(&Obj::Null));
        let domain = if dm.len() >= 2 { [dm[0], dm[1]] } else { [0.0, 1.0] };
        let domain2 = if dm.len() >= 4 { [dm[0], dm[1], dm[2], dm[3]] } else { [0.0, 1.0, 0.0, 1.0] };
        let ex: Vec<bool> = match f.key(&d, "Extend") {
            Obj::Array(a) => a.iter().map(|o| matches!(f.resolve(o), Obj::Bool(true))).collect(),
            _ => Vec::new(),
        };
        let m = f.nums(d.get("Matrix").unwrap_or(&Obj::Null));
        let matrix = if m.len() == 6 { Affine::new([m[0], m[1], m[2], m[3], m[4], m[5]]) } else { Affine::IDENTITY };
        let b = f.nums(d.get("BBox").unwrap_or(&Obj::Null));
        let bbox = (b.len() == 4).then(|| [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])]);
        let mut s = Shading { kind, cs, func, coords, domain, extend: [ex.first().copied().unwrap_or(false), ex.get(1).copied().unwrap_or(false)], domain2, matrix, bbox, tris: Vec::new(), lut: Default::default() };
        if (4..=7).contains(&kind) {
            let data = stream?;
            s.mesh(f, &d, &data);
        }
        Some(s)
    }

    /// The colour (RGB) of components or of `t` through the function.
    fn color(&self, c: &[f32]) -> [f32; 3] {
        match &self.func {
            Some(func) => {
                let mut out = [0.0f32; 8];
                let n = func.eval(c, &mut out);
                self.cs.rgb(&out[..n.max(1)])
            }
            None => self.cs.rgb(c),
        }
    }

    /// About what [`Shading::raster`] of that area takes, in pixels: a
    /// mesh paints each triangle's box, and a colour through a long
    /// function weighs more.
    pub fn work(&self, x0: i32, y0: i32, w: usize, h: usize, to_dev: Affine) -> usize {
        let per = self.func.as_ref().map_or(1, |f| 1 + f.work() / 16);
        let pixels = match self.kind {
            2 | 3 => return w.saturating_mul(h),
            1 => w.saturating_mul(h),
            _ => self
                .tris
                .iter()
                .map(|t| {
                    let p = [to_dev * t[0].0, to_dev * t[1].0, to_dev * t[2].0];
                    let span = |a: f64, b: f64, lo: i32, n: usize| (b.ceil().min((lo + n as i32) as f64) - a.floor().max(lo as f64)).max(0.0) as usize;
                    let (xs, ys) = (p.map(|q| q.x), p.map(|q| q.y));
                    let (lx, hx) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
                    let (ly, hy) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
                    span(lx, hx, x0, w).saturating_mul(span(ly, hy, y0, h))
                })
                .fold(0usize, usize::saturating_add),
        };
        pixels.saturating_mul(per)
    }

    /// Paints the shading into a `w` x `h` RGBA area whose top left is
    /// device pixel (`x0`, `y0`); `to_dev` maps shading space to device.
    pub fn raster(&self, x0: i32, y0: i32, w: usize, h: usize, to_dev: Affine) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        if w == 0 || h == 0 {
            return out;
        }
        let inv = to_dev.inverse();
        let put = |out: &mut Vec<u8>, i: usize, c: [f32; 3]| {
            out[i * 4] = (c[0] * 255.0).round() as u8;
            out[i * 4 + 1] = (c[1] * 255.0).round() as u8;
            out[i * 4 + 2] = (c[2] * 255.0).round() as u8;
            out[i * 4 + 3] = 255;
        };
        match self.kind {
            1 => {
                let inv = (to_dev * self.matrix).inverse();
                let [x_lo, x_hi, y_lo, y_hi] = self.domain2;
                for py in 0..h {
                    for px in 0..w {
                        let p = inv * Point::new(x0 as f64 + px as f64 + 0.5, y0 as f64 + py as f64 + 0.5);
                        if p.x < x_lo || p.x > x_hi || p.y < y_lo || p.y > y_hi {
                            continue;
                        }
                        put(&mut out, py * w + px, self.color(&[p.x as f32, p.y as f32]));
                    }
                }
            }
            2 | 3 => {
                if self.coords.len() < if self.kind == 2 { 4 } else { 6 } {
                    return out;
                }
                // Colours of t along the domain, in pdfium's 256 steps.
                const N: usize = 256;
                let lut = self.lut.get_or_init(|| {
                    (0..N)
                        .map(|i| {
                            let t = self.domain[0] + (self.domain[1] - self.domain[0]) * i as f64 / N as f64;
                            self.color(&[t as f32])
                        })
                        .collect()
                });
                let c = &self.coords;
                for py in 0..h {
                    for px in 0..w {
                        // At the pixel's corner, as pdfium.
                        let p = inv * Point::new((x0 + px as i32) as f64, (y0 + py as i32) as f64);
                        let s = if self.kind == 2 { axial(c, p) } else { radial(c, p, self.extend) };
                        let Some(mut s) = s else { continue };
                        if s < 0.0 {
                            if !self.extend[0] {
                                continue;
                            }
                            s = 0.0;
                        } else if s > 1.0 {
                            if !self.extend[1] {
                                continue;
                            }
                            s = 1.0;
                        }
                        put(&mut out, py * w + px, lut[((s * (N - 1) as f64) as usize).min(N - 1)]);
                    }
                }
            }
            _ => {
                for t in &self.tris {
                    let pts = [to_dev * t[0].0, to_dev * t[1].0, to_dev * t[2].0];
                    self.triangle(&mut out, x0, y0, w, h, pts, [&t[0].1, &t[1].1, &t[2].1]);
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn triangle(&self, out: &mut [u8], x0: i32, y0: i32, w: usize, h: usize, p: [Point; 3], c: [&Vec<f32>; 3]) {
        let min_x = p.iter().map(|q| q.x).fold(f64::MAX, f64::min).floor().max(x0 as f64) as i32;
        let max_x = p.iter().map(|q| q.x).fold(f64::MIN, f64::max).ceil().min((x0 + w as i32) as f64) as i32;
        let min_y = p.iter().map(|q| q.y).fold(f64::MAX, f64::min).floor().max(y0 as f64) as i32;
        let max_y = p.iter().map(|q| q.y).fold(f64::MIN, f64::max).ceil().min((y0 + h as i32) as f64) as i32;
        let area = (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (p[1].y - p[0].y);
        if area.abs() < 1e-9 {
            return;
        }
        let n = c[0].len().min(c[1].len()).min(c[2].len());
        let mut v = vec![0.0f32; n];
        for y in min_y..max_y {
            for x in min_x..max_x {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((p[1].x - px) * (p[2].y - py) - (p[2].x - px) * (p[1].y - py)) / area;
                let w1 = ((p[2].x - px) * (p[0].y - py) - (p[0].x - px) * (p[2].y - py)) / area;
                let w2 = 1.0 - w0 - w1;
                // A little over the edges, so neighbours leave no seams.
                let e = -0.02;
                if w0 < e || w1 < e || w2 < e {
                    continue;
                }
                for k in 0..n {
                    v[k] = (w0 as f32) * c[0][k] + (w1 as f32) * c[1][k] + (w2 as f32) * c[2][k];
                }
                let col = self.color(&v);
                let i = ((y - y0) as usize * w + (x - x0) as usize) * 4;
                out[i] = (col[0] * 255.0).round() as u8;
                out[i + 1] = (col[1] * 255.0).round() as u8;
                out[i + 2] = (col[2] * 255.0).round() as u8;
                out[i + 3] = 255;
            }
        }
    }

    /// Reads the triangles of a mesh (types 4 to 7; patches are split
    /// into triangles).
    fn mesh(&mut self, f: &PdfFile, d: &Dict, data: &[u8]) {
        let bpcoord = f.key_num(d, "BitsPerCoordinate").unwrap_or(16.0) as u32;
        let bpcomp = f.key_num(d, "BitsPerComponent").unwrap_or(16.0) as u32;
        let bpflag = f.key_num(d, "BitsPerFlag").unwrap_or(8.0) as u32;
        let decode = f.nums(d.get("Decode").unwrap_or(&Obj::Null));
        let nc = if self.func.is_some() { 1 } else { self.cs.n() };
        if decode.len() < 4 + 2 * nc || !(1..=32).contains(&bpcoord) || !(1..=16).contains(&bpcomp) {
            return;
        }
        let mut r = Bits { d: data, p: 0 };
        let maxc = ((1u64 << bpcoord) - 1) as f64;
        let maxk = ((1u64 << bpcomp) - 1) as f64;
        let point = |r: &mut Bits| -> Option<Point> {
            let x = r.read(bpcoord)? as f64 / maxc;
            let y = r.read(bpcoord)? as f64 / maxc;
            Some(Point::new(decode[0] + x * (decode[1] - decode[0]), decode[2] + y * (decode[3] - decode[2])))
        };
        let color = |r: &mut Bits| -> Option<Vec<f32>> {
            (0..nc)
                .map(|k| {
                    let v = r.read(bpcomp)? as f64 / maxk;
                    Some((decode[4 + 2 * k] + v * (decode[5 + 2 * k] - decode[4 + 2 * k])) as f32)
                })
                .collect()
        };
        const MAX_TRIS: usize = 400_000;
        match self.kind {
            4 => {
                let mut last: Vec<(Point, Vec<f32>)> = Vec::new();
                while self.tris.len() < MAX_TRIS {
                    let Some(flag) = r.read(bpflag) else { break };
                    let Some(p) = point(&mut r) else { break };
                    let Some(c) = color(&mut r) else { break };
                    r.align();
                    let v = (p, c);
                    match flag {
                        0 => {
                            last.push(v);
                            if last.len() == 3 {
                                self.tris.push([last[0].clone(), last[1].clone(), last[2].clone()]);
                                last.clear();
                            }
                        }
                        _ => {
                            // Shares an edge of the previous triangle.
                            let Some(t) = self.tris.last().cloned() else { continue };
                            let tri = if flag == 1 { [t[1].clone(), t[2].clone(), v] } else { [t[0].clone(), t[2].clone(), v] };
                            self.tris.push(tri);
                        }
                    }
                }
            }
            5 => {
                let per_row = f.key_num(d, "VerticesPerRow").unwrap_or(2.0).max(2.0) as usize;
                let mut verts = Vec::new();
                while verts.len() < MAX_TRIS {
                    let Some(p) = point(&mut r) else { break };
                    let Some(c) = color(&mut r) else { break };
                    verts.push((p, c));
                }
                let rows = verts.len() / per_row;
                for y in 0..rows.saturating_sub(1) {
                    for x in 0..per_row - 1 {
                        let a = &verts[y * per_row + x];
                        let b = &verts[y * per_row + x + 1];
                        let c = &verts[(y + 1) * per_row + x];
                        let e = &verts[(y + 1) * per_row + x + 1];
                        self.tris.push([a.clone(), b.clone(), c.clone()]);
                        self.tris.push([b.clone(), e.clone(), c.clone()]);
                    }
                }
            }
            _ => {
                let tensor = self.kind == 7;
                let npts = if tensor { 16 } else { 12 };
                let mut prev: Option<([Point; 12], [Vec<f32>; 4])> = None;
                while self.tris.len() < MAX_TRIS {
                    let Some(flag) = r.read(bpflag) else { break };
                    let (mut pts, mut cols): (Vec<Point>, Vec<Vec<f32>>) = (Vec::new(), Vec::new());
                    let (newp, newc) = if flag == 0 { (npts, 4) } else { (npts - 4, 2) };
                    let mut ok = true;
                    for _ in 0..newp {
                        match point(&mut r) {
                            Some(p) => pts.push(p),
                            None => ok = false,
                        }
                    }
                    for _ in 0..newc {
                        match color(&mut r) {
                            Some(c) => cols.push(c),
                            None => ok = false,
                        }
                    }
                    r.align();
                    if !ok {
                        break;
                    }
                    // The boundary: 12 points clockwise from the corner, and corner colours.
                    let (b, c): ([Point; 12], [Vec<f32>; 4]) = if flag == 0 {
                        let mut b = [Point::ZERO; 12];
                        b.copy_from_slice(&pts[..12]);
                        (b, [cols[0].clone(), cols[1].clone(), cols[2].clone(), cols[3].clone()])
                    } else {
                        let Some((pb, pc)) = &prev else { continue };
                        // The shared edge of the previous patch.
                        let (e, c0, c1) = match flag {
                            1 => ([pb[3], pb[4], pb[5], pb[6]], pc[1].clone(), pc[2].clone()),
                            2 => ([pb[6], pb[7], pb[8], pb[9]], pc[2].clone(), pc[3].clone()),
                            _ => ([pb[9], pb[10], pb[11], pb[0]], pc[3].clone(), pc[0].clone()),
                        };
                        let mut b = [Point::ZERO; 12];
                        b[..4].copy_from_slice(&e);
                        b[4..].copy_from_slice(&pts[..8]);
                        (b, [c0, c1, cols[0].clone(), cols[1].clone()])
                    };
                    self.patch(&b, &c);
                    prev = Some((b, c));
                }
            }
        }
    }

    /// Splits a Coons patch into triangles.
    fn patch(&mut self, b: &[Point; 12], c: &[Vec<f32>; 4]) {
        let bez = |p: [Point; 4], t: f64| -> Point {
            let u = 1.0 - t;
            let k = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            Point::new(k[0] * p[0].x + k[1] * p[1].x + k[2] * p[2].x + k[3] * p[3].x, k[0] * p[0].y + k[1] * p[1].y + k[2] * p[2].y + k[3] * p[3].y)
        };
        // Curves: bottom (p00 -> p03 ... as stored), using the spec's order.
        let c1 = [b[0], b[1], b[2], b[3]]; // u = 0 edge, v from 0 to 1
        let d2 = [b[3], b[4], b[5], b[6]]; // v = 1 edge, u from 0 to 1
        let c2 = [b[9], b[8], b[7], b[6]]; // u = 1 edge, v from 0 to 1
        let d1 = [b[0], b[11], b[10], b[9]]; // v = 0 edge, u from 0 to 1
        const N: usize = 12;
        let n = c[0].len();
        let mut grid: Vec<(Point, Vec<f32>)> = Vec::with_capacity((N + 1) * (N + 1));
        for j in 0..=N {
            let v = j as f64 / N as f64;
            for i in 0..=N {
                let u = i as f64 / N as f64;
                let sc = bez(c1, v).to_vec2() * (1.0 - u) + bez(c2, v).to_vec2() * u;
                let sd = bez(d1, u).to_vec2() * (1.0 - v) + bez(d2, u).to_vec2() * v;
                let sb = b[0].to_vec2() * ((1.0 - u) * (1.0 - v)) + b[3].to_vec2() * ((1.0 - u) * v) + b[6].to_vec2() * (u * v) + b[9].to_vec2() * (u * (1.0 - v));
                let p = (sc + sd - sb).to_point();
                // Corner colours: c[0] at b0 (u0 v0), c[1] at b3 (u0 v1), c[2] at b6 (u1 v1), c[3] at b9 (u1 v0).
                let col: Vec<f32> = (0..n)
                    .map(|k| {
                        let (a, bb, cc, dd) = (c[0][k], c[1].get(k).copied().unwrap_or(0.0), c[2].get(k).copied().unwrap_or(0.0), c[3].get(k).copied().unwrap_or(0.0));
                        ((1.0 - u) * (1.0 - v)) as f32 * a + ((1.0 - u) * v) as f32 * bb + (u * v) as f32 * cc + (u * (1.0 - v)) as f32 * dd
                    })
                    .collect();
                grid.push((p, col));
            }
        }
        for j in 0..N {
            for i in 0..N {
                let a = &grid[j * (N + 1) + i];
                let bq = &grid[j * (N + 1) + i + 1];
                let cq = &grid[(j + 1) * (N + 1) + i];
                let dq = &grid[(j + 1) * (N + 1) + i + 1];
                self.tris.push([a.clone(), bq.clone(), cq.clone()]);
                self.tris.push([bq.clone(), dq.clone(), cq.clone()]);
            }
        }
    }
}

fn axial(c: &[f64], p: Point) -> Option<f64> {
    let (dx, dy) = (c[2] - c[0], c[3] - c[1]);
    let len = dx * dx + dy * dy;
    if len == 0.0 {
        return None;
    }
    Some(((p.x - c[0]) * dx + (p.y - c[1]) * dy) / len)
}

/// The largest s with `p` on the circle at s (the later circle is drawn
/// over the earlier).
fn radial(c: &[f64], p: Point, extend: [bool; 2]) -> Option<f64> {
    let (x0, y0, r0, x1, y1, r1) = (c[0], c[1], c[2], c[3], c[4], c[5]);
    let (cdx, cdy, dr) = (x1 - x0, y1 - y0, r1 - r0);
    let (pdx, pdy) = (p.x - x0, p.y - y0);
    let a = cdx * cdx + cdy * cdy - dr * dr;
    let b = pdx * cdx + pdy * cdy + r0 * dr;
    let cc = pdx * pdx + pdy * pdy - r0 * r0;
    let ok = |s: f64| r0 + s * dr >= 0.0 && (s >= 0.0 || extend[0]) && (s <= 1.0 || extend[1]);
    if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            return None;
        }
        let s = cc / (2.0 * b);
        return ok(s).then_some(s);
    }
    let disc = b * b - a * cc;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let (s1, s2) = ((b + sq) / a, (b - sq) / a);
    let (hi, lo) = if s1 > s2 { (s1, s2) } else { (s2, s1) };
    if ok(hi) {
        Some(hi)
    } else if ok(lo) {
        Some(lo)
    } else {
        None
    }
}

struct Bits<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Option<u64> {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = *self.d.get(self.p / 8)?;
            v = v << 1 | ((byte >> (7 - self.p % 8)) & 1) as u64;
            self.p += 1;
        }
        Some(v)
    }

    fn align(&mut self) {
        self.p = self.p.div_ceil(8) * 8;
    }
}
