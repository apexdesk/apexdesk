//! Colour spaces and the functions of tint transforms and shadings.

use super::super::file::PdfFile;
use super::super::obj::{Dict, Item, Lexer, Obj};

/// A colour space, as far as turning its colours into RGB goes.
#[derive(Clone, Debug)]
pub enum Cs {
    Gray,
    Rgb,
    Cmyk,
    Lab { white: [f32; 3], range: [f32; 4] },
    /// CalRGB: gamma, then the matrix to XYZ, seen from the white point.
    CalRgb { gamma: [f32; 3], matrix: [f32; 9], white: [f32; 3] },
    Indexed { base: Box<Cs>, hival: usize, lookup: Vec<u8> },
    /// An ICC profile of the matrix and curves kind, RGB or gray.
    Icc(std::sync::Arc<Icc>),
    /// Separation and DeviceN: `n` tints through a function into another space.
    Tint { n: usize, alt: Box<Cs>, func: Func, none: bool },
    /// Patterns (with the space of an uncoloured pattern's colour).
    Pattern(Option<Box<Cs>>),
}

impl Cs {
    pub fn load(f: &PdfFile, o: &Obj, res: Option<&Dict>) -> Cs {
        Self::load_depth(f, o, res, 0)
    }

    fn load_depth(f: &PdfFile, o: &Obj, res: Option<&Dict>, depth: u32) -> Cs {
        if depth > 8 {
            return Cs::Gray;
        }
        match f.resolve(o) {
            Obj::Name(n) => match &*n {
                "DeviceGray" | "G" | "CalGray" => Cs::Gray,
                "DeviceRGB" | "RGB" | "CalRGB" => Cs::Rgb,
                "DeviceCMYK" | "CMYK" => Cs::Cmyk,
                "Pattern" => Cs::Pattern(None),
                // A name in the resources.
                other => match res.and_then(|r| f.key_dict(r, "ColorSpace")).map(|d| f.key(&d, other)) {
                    Some(o2) if o2 != Obj::Null && !matches!(&o2, Obj::Name(m) if **m == *other) => Self::load_depth(f, &o2, None, depth + 1),
                    _ => Cs::Gray,
                },
            },
            Obj::Array(a) => {
                let kind = a.first().map(|o| f.resolve(o)).and_then(|o| o.name().map(str::to_string)).unwrap_or_default();
                match kind.as_str() {
                    "ICCBased" => {
                        let d = a.get(1).map(|o| f.resolve(o));
                        let d = d.as_ref().and_then(Obj::dict);
                        let n = d.and_then(|d| f.key_num(d, "N")).unwrap_or(3.0) as usize;
                        // The profile, converted as pdfium's lcms does.
                        if let Some(Obj::Stream(sd, raw)) = a.get(1).map(|o| f.resolve(o))
                            && let Some(icc) = Icc::parse(&f.decode(&sd, &raw).data)
                            && icc.n() == n
                        {
                            return Cs::Icc(std::sync::Arc::new(icc));
                        }
                        match n {
                            1 => Cs::Gray,
                            4 => Cs::Cmyk,
                            3 => Cs::Rgb,
                            _ => match d.map(|d| f.key(d, "Alternate")) {
                                Some(alt) if alt != Obj::Null => Self::load_depth(f, &alt, res, depth + 1),
                                _ => Cs::Rgb,
                            },
                        }
                    }
                    "CalRGB" => {
                        let d = a.get(1).map(|o| f.resolve(o)).and_then(|o| o.dict().cloned()).unwrap_or_default();
                        let w = f.nums(d.get("WhitePoint").unwrap_or(&Obj::Null));
                        let g = f.nums(d.get("Gamma").unwrap_or(&Obj::Null));
                        let m = f.nums(d.get("Matrix").unwrap_or(&Obj::Null));
                        let white = if w.len() == 3 { [w[0] as f32, w[1] as f32, w[2] as f32] } else { [0.9505, 1.0, 1.089] };
                        let gamma = if g.len() == 3 { [g[0] as f32, g[1] as f32, g[2] as f32] } else { [1.0; 3] };
                        let mut matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
                        if m.len() == 9 {
                            for (o, v) in matrix.iter_mut().zip(m) {
                                *o = v as f32;
                            }
                        }
                        Cs::CalRgb { gamma, matrix, white }
                    }
                    "CalGray" => Cs::Gray,
                    "Lab" => {
                        let d = a.get(1).map(|o| f.resolve(o)).and_then(|o| o.dict().cloned()).unwrap_or_default();
                        let w = f.nums(d.get("WhitePoint").unwrap_or(&Obj::Null));
                        let r = f.nums(d.get("Range").unwrap_or(&Obj::Null));
                        let white = if w.len() == 3 { [w[0] as f32, w[1] as f32, w[2] as f32] } else { [0.9505, 1.0, 1.089] };
                        let range = if r.len() == 4 { [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32] } else { [-100.0, 100.0, -100.0, 100.0] };
                        Cs::Lab { white, range }
                    }
                    "Indexed" | "I" => {
                        let base = Self::load_depth(f, a.get(1).unwrap_or(&Obj::Null), res, depth + 1);
                        let hival = a.get(2).and_then(|o| f.resolve(o).int()).unwrap_or(255).clamp(0, 255) as usize;
                        let lookup = match a.get(3).map(|o| f.resolve(o)) {
                            Some(Obj::Str(s)) => s,
                            Some(Obj::Stream(d, raw)) => f.decode(&d, &raw).data,
                            _ => Vec::new(),
                        };
                        Cs::Indexed { base: Box::new(base), hival, lookup }
                    }
                    "Separation" | "DeviceN" => {
                        let names: Vec<String> = if kind == "Separation" {
                            a.get(1).map(|o| f.resolve(o)).and_then(|o| o.name().map(str::to_string)).into_iter().collect()
                        } else {
                            a.get(1).map(|o| f.resolve(o)).and_then(|o| o.array().map(|v| v.iter().filter_map(|x| f.resolve(x).name().map(str::to_string)).collect())).unwrap_or_default()
                        };
                        let alt = Self::load_depth(f, a.get(2).unwrap_or(&Obj::Null), res, depth + 1);
                        let func = Func::load(f, a.get(3).unwrap_or(&Obj::Null)).unwrap_or(Func::Identity);
                        let none = !names.is_empty() && names.iter().all(|n| n == "None");
                        Cs::Tint { n: names.len().clamp(1, 32), alt: Box::new(alt), func, none }
                    }
                    "Pattern" => Cs::Pattern(a.get(1).map(|o| Box::new(Self::load_depth(f, o, res, depth + 1)))),
                    _ => Self::load_depth(f, a.first().unwrap_or(&Obj::Null), res, depth + 1),
                }
            }
            _ => Cs::Gray,
        }
    }

    /// The "None" separation, which paints nothing.
    pub fn invisible(&self) -> bool {
        matches!(self, Cs::Tint { none: true, .. })
    }

    pub fn n(&self) -> usize {
        match self {
            Cs::Gray | Cs::Indexed { .. } | Cs::Pattern(_) => 1,
            Cs::Rgb | Cs::Lab { .. } | Cs::CalRgb { .. } => 3,
            Cs::Icc(icc) => icc.n(),
            Cs::Cmyk => 4,
            Cs::Tint { n, .. } => *n,
        }
    }

    /// The initial colour of the space.
    pub fn initial(&self) -> Vec<f32> {
        match self {
            Cs::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Cs::Tint { n, .. } => vec![1.0; *n],
            Cs::Lab { .. } => vec![0.0, 0.0, 0.0],
            s => vec![0.0; s.n()],
        }
    }

    /// The default Decode range of component `i` for image samples.
    pub fn decode_range(&self, i: usize, bpc: usize) -> (f32, f32) {
        match self {
            Cs::Indexed { .. } => (0.0, ((1u32 << bpc) - 1) as f32),
            Cs::Lab { range, .. } => match i {
                0 => (0.0, 100.0),
                1 => (range[0], range[1]),
                _ => (range[2], range[3]),
            },
            _ => (0.0, 1.0),
        }
    }

    /// The colour as RGB (0 to 1).
    pub fn rgb(&self, c: &[f32]) -> [f32; 3] {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0);
        match self {
            Cs::Gray => {
                let v = g(0).clamp(0.0, 1.0);
                [v, v, v]
            }
            Cs::Rgb => [g(0).clamp(0.0, 1.0), g(1).clamp(0.0, 1.0), g(2).clamp(0.0, 1.0)],
            Cs::Cmyk => cmyk(g(0), g(1), g(2), g(3)),
            Cs::CalRgb { gamma, matrix: m, white } => {
                let (a, b, c) = (g(0).clamp(0.0, 1.0).powf(gamma[0]), g(1).clamp(0.0, 1.0).powf(gamma[1]), g(2).clamp(0.0, 1.0).powf(gamma[2]));
                let xyz = [m[0] * a + m[3] * b + m[6] * c, m[1] * a + m[4] * b + m[7] * c, m[2] * a + m[5] * b + m[8] * c];
                xyz_to_srgb(xyz, *white)
            }
            Cs::Icc(icc) => icc.rgb(c),
            Cs::Lab { white, range } => lab(g(0), g(1).max(range[0]).min(range[1]), g(2).max(range[2]).min(range[3]), *white),
            Cs::Indexed { base, hival, lookup } => {
                let i = (g(0).round().max(0.0) as usize).min(*hival);
                let bn = base.n();
                let comps: Vec<f32> = (0..bn).map(|k| lookup.get(i * bn + k).copied().unwrap_or(0) as f32 / 255.0).collect();
                // Lab lookups hold the components scaled to the range.
                if let Cs::Lab { range, .. } = &**base {
                    let v = [comps[0] * 100.0, range[0] + comps[1] * (range[1] - range[0]), range[2] + comps[2] * (range[3] - range[2])];
                    return base.rgb(&v);
                }
                base.rgb(&comps)
            }
            Cs::Tint { alt, func, .. } => {
                let mut out = [0.0f32; 8];
                let n = func.eval(c, &mut out);
                alt.rgb(&out[..n.max(alt.n()).min(8)])
            }
            Cs::Pattern(_) => [0.5, 0.5, 0.5],
        }
    }
}

/// An ICC profile made of a tone curve per channel and (RGB) a matrix to
/// XYZ: what lcms does with it into sRGB.
#[derive(Debug)]
pub struct Icc {
    gray: bool,
    /// The curves, sampled at 1024 points from 0 to 1.
    trc: [Vec<f32>; 3],
    /// Linear RGB of the profile to linear sRGB.
    m: [f32; 9],
}

impl Icc {
    pub fn parse(d: &[u8]) -> Option<Icc> {
        let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        let gray = match d.get(16..20)? {
            b"RGB " => false,
            b"GRAY" => true,
            _ => return None,
        };
        if d.get(20..24)? != b"XYZ " {
            return None;
        }
        let count = u32_at(128)? as usize;
        let mut tags = std::collections::HashMap::new();
        for i in 0..count.min(256) {
            let at = 132 + 12 * i;
            let sig = d.get(at..at + 4)?;
            let (off, len) = (u32_at(at + 4)? as usize, u32_at(at + 8)? as usize);
            tags.insert(<[u8; 4]>::try_from(sig).ok()?, d.get(off..off.checked_add(len)?)?);
        }
        // Tables (A2B0) are what lcms would use; not these.
        if tags.contains_key(b"A2B0") {
            return None;
        }
        let fixed = |b: &[u8], o: usize| b.get(o..o + 4).map(|v| i32::from_be_bytes([v[0], v[1], v[2], v[3]]) as f32 / 65536.0);
        let xyz = |t: &[u8; 4]| -> Option<[f32; 3]> {
            let b = tags.get(t)?;
            (b.get(..4)? == b"XYZ ").then_some(())?;
            Some([fixed(b, 8)?, fixed(b, 12)?, fixed(b, 16)?])
        };
        let curve = |t: &[u8; 4]| -> Option<Vec<f32>> { trc(tags.get(t)?) };
        if gray {
            let k = curve(b"kTRC")?;
            return Some(Icc { gray, trc: [k.clone(), k.clone(), k], m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] });
        }
        let (r, g, b) = (xyz(b"rXYZ")?, xyz(b"gXYZ")?, xyz(b"bXYZ")?);
        // The profile's columns to XYZ (D50), then XYZ (D50) to linear sRGB.
        let to_xyz = [r[0], g[0], b[0], r[1], g[1], b[1], r[2], g[2], b[2]];
        let srgb = [3.133_856, -1.616_867, -0.490_615, -0.978_768, 1.916_142, 0.033_454, 0.071_945, -0.228_991, 1.405_243];
        let mut m = [0.0f32; 9];
        for i in 0..3 {
            for j in 0..3 {
                m[3 * i + j] = (0..3).map(|k| srgb[3 * i + k] * to_xyz[3 * k + j]).sum();
            }
        }
        Some(Icc { gray, trc: [curve(b"rTRC")?, curve(b"gTRC")?, curve(b"bTRC")?], m })
    }

    pub fn n(&self) -> usize {
        if self.gray { 1 } else { 3 }
    }

    pub fn rgb(&self, c: &[f32]) -> [f32; 3] {
        let at = |t: &[f32], v: f32| {
            let x = v.clamp(0.0, 1.0) * (t.len() - 1) as f32;
            let i = (x as usize).min(t.len() - 2);
            t[i] + (t[i + 1] - t[i]) * (x - i as f32)
        };
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0);
        if self.gray {
            let v = encode_srgb(at(&self.trc[0], g(0)));
            return [v, v, v];
        }
        let lin = [at(&self.trc[0], g(0)), at(&self.trc[1], g(1)), at(&self.trc[2], g(2))];
        mul3(self.m, lin).map(encode_srgb)
    }

    /// 8-bit RGB pixels (RGBA) from the profile's space into sRGB.
    pub fn convert_rgba(&self, px: &mut [u8]) {
        let gray: Vec<u8> = if self.gray { (0..256).map(|v| (self.rgb(&[v as f32 / 255.0])[0] * 255.0).round() as u8).collect() } else { Vec::new() };
        let lin: [Vec<f32>; 3] = std::array::from_fn(|k| (0..256).map(|v| {
            let x = v as f32 / 255.0 * (self.trc[k].len() - 1) as f32;
            let i = (x as usize).min(self.trc[k].len() - 2);
            self.trc[k][i] + (self.trc[k][i + 1] - self.trc[k][i]) * (x - i as f32)
        }).collect());
        let enc = srgb_table();
        let to8 = |v: f32| enc[(v.clamp(0.0, 1.0) * 4095.0).round() as usize];
        for p in px.chunks_exact_mut(4) {
            if self.gray {
                let v = gray[p[0] as usize];
                p[..3].copy_from_slice(&[v, v, v]);
                continue;
            }
            let l = [lin[0][p[0] as usize], lin[1][p[1] as usize], lin[2][p[2] as usize]];
            let o = mul3(self.m, l);
            p[..3].copy_from_slice(&[to8(o[0]), to8(o[1]), to8(o[2])]);
        }
    }
}

/// A curv or para tag's curve, sampled.
fn trc(b: &[u8]) -> Option<Vec<f32>> {
    const N: usize = 1024;
    let u16_at = |o: usize| b.get(o..o + 2).map(|v| u16::from_be_bytes([v[0], v[1]]));
    let fixed = |o: usize| b.get(o..o + 4).map(|v| i32::from_be_bytes([v[0], v[1], v[2], v[3]]) as f32 / 65536.0);
    let sample = |f: &dyn Fn(f32) -> f32| (0..N).map(|i| f(i as f32 / (N - 1) as f32).clamp(0.0, 1.0)).collect::<Vec<f32>>();
    match b.get(..4)? {
        b"curv" => {
            let n = u32::from_be_bytes(b.get(8..12)?.try_into().ok()?) as usize;
            match n {
                0 => Some(sample(&|x| x)),
                1 => {
                    let g = u16_at(12)? as f32 / 256.0;
                    Some(sample(&|x| x.powf(g)))
                }
                _ => {
                    let t: Vec<f32> = (0..n).map(|i| u16_at(12 + 2 * i).map(|v| v as f32 / 65535.0)).collect::<Option<_>>()?;
                    Some(sample(&|x| {
                        let p = x * (n - 1) as f32;
                        let i = (p as usize).min(n - 2);
                        t[i] + (t[i + 1] - t[i]) * (p - i as f32)
                    }))
                }
            }
        }
        b"para" => {
            let kind = u16_at(8)?;
            let count = [1, 3, 4, 5, 7].get(kind as usize).copied()?;
            let p: Vec<f32> = (0..count).map(|i| fixed(12 + 4 * i)).collect::<Option<_>>()?;
            let pw = |v: f32, g: f32| if v > 0.0 { v.powf(g) } else { 0.0 };
            Some(match kind {
                0 => sample(&|x| pw(x, p[0])),
                1 => sample(&|x| if x >= -p[2] / p[1] { pw(p[1] * x + p[2], p[0]) } else { 0.0 }),
                2 => sample(&|x| if x >= -p[2] / p[1] { pw(p[1] * x + p[2], p[0]) + p[3] } else { p[3] }),
                3 => sample(&|x| if x >= p[4] { pw(p[1] * x + p[2], p[0]) } else { p[3] * x }),
                _ => sample(&|x| if x >= p[4] { pw(p[1] * x + p[2], p[0]) + p[5] } else { p[3] * x + p[6] }),
            })
        }
        _ => None,
    }
}

fn encode_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Linear values (4096 steps) to 8-bit sRGB.
fn srgb_table() -> &'static [u8; 4096] {
    static T: std::sync::OnceLock<[u8; 4096]> = std::sync::OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|i| (encode_srgb(i as f32 / 4095.0) * 255.0).round() as u8))
}

/// DeviceCMYK to RGB the way pdfium draws it: its colours at the points
/// of a 9x9x9x9 grid (`cmyk.lut`, measured from pdfium), interpolated.
pub fn cmyk(c: f32, m: f32, y: f32, k: f32) -> [f32; 3] {
    static LUT: &[u8; 9 * 9 * 9 * 9 * 3] = include_bytes!("cmyk.lut");
    let split = |v: f32| {
        let v = v.clamp(0.0, 1.0) * 8.0;
        let i = (v.floor() as usize).min(7);
        (i, v - i as f32)
    };
    let (ci, cf) = split(c);
    let (mi, mf) = split(m);
    let (yi, yf) = split(y);
    let (ki, kf) = split(k);
    let mut out = [0.0f32; 3];
    for corner in 0..16 {
        let bit = |b: usize| (corner >> b) & 1;
        let w = [(cf, 3), (mf, 2), (yf, 1), (kf, 0)].iter().fold(1.0, |acc, &(f, b)| acc * if bit(b) == 1 { f } else { 1.0 - f });
        if w == 0.0 {
            continue;
        }
        let at = ((((ci + bit(3)) * 9 + mi + bit(2)) * 9 + yi + bit(1)) * 9 + ki + bit(0)) * 3;
        for (o, v) in out.iter_mut().zip(&LUT[at..at + 3]) {
            *o += w * *v as f32;
        }
    }
    out.map(|v| (v / 255.0).clamp(0.0, 1.0))
}

fn inverse3(m: [f32; 9]) -> [f32; 9] {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
    if det.abs() < 1e-12 {
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    }
    let i = 1.0 / det;
    [
        (m[4] * m[8] - m[5] * m[7]) * i,
        (m[2] * m[7] - m[1] * m[8]) * i,
        (m[1] * m[5] - m[2] * m[4]) * i,
        (m[5] * m[6] - m[3] * m[8]) * i,
        (m[0] * m[8] - m[2] * m[6]) * i,
        (m[2] * m[3] - m[0] * m[5]) * i,
        (m[3] * m[7] - m[4] * m[6]) * i,
        (m[1] * m[6] - m[0] * m[7]) * i,
        (m[0] * m[4] - m[1] * m[3]) * i,
    ]
}

fn mul3(m: [f32; 9], v: [f32; 3]) -> [f32; 3] {
    [m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]]
}

/// XYZ to sRGB with sRGB's primaries scaled so that white is `white`
/// (as pdfium does it).
fn xyz_to_srgb(xyz: [f32; 3], white: [f32; 3]) -> [f32; 3] {
    // Rows of the primaries' chromaticities (x, y, z).
    let p = [0.64, 0.30, 0.15, 0.33, 0.60, 0.06, 0.03, 0.10, 0.79];
    let sum = mul3(inverse3(p), white);
    let rgb = mul3(inverse3(p), xyz);
    let s = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    };
    let d = |v: f32, k: f32| if k.abs() > 1e-6 { v / k } else { v };
    [s(d(rgb[0], sum[0])), s(d(rgb[1], sum[1])), s(d(rgb[2], sum[2]))]
}

/// Lab to sRGB as pdfium does it: a fixed white (about D65) whatever the
/// space's WhitePoint, so a and b of 0 are grey.
fn lab(l: f32, a: f32, b: f32, _w: [f32; 3]) -> [f32; 3] {
    let m = (l + 16.0) / 116.0;
    let fl = m + a / 500.0;
    let nl = m - b / 200.0;
    let g = |x: f32| if x < 0.2069 { 0.12842 * (x - 0.1379) } else { x * x * x };
    let (x, y, z) = (0.957 * g(fl), g(m), 1.0889 * g(nl));
    let r = 3.2410 * x - 1.5374 * y - 0.4986 * z;
    let gr = -0.9692 * x + 1.8760 * y + 0.0416 * z;
    let bl = 0.0556 * x - 0.2040 * y + 1.0570 * z;
    let s = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    };
    [s(r), s(gr), s(bl)]
}

/// A PDF function (types 0, 2, 3 and 4).
#[derive(Clone, Debug)]
pub enum Func {
    Identity,
    Sampled { domain: Vec<f32>, range: Vec<f32>, size: Vec<usize>, encode: Vec<f32>, decode: Vec<f32>, samples: Vec<f32>, outs: usize },
    Exp { domain: [f32; 2], c0: Vec<f32>, c1: Vec<f32>, n: f32 },
    Stitch { domain: [f32; 2], funcs: Vec<Func>, bounds: Vec<f32>, encode: Vec<f32> },
    Ps { domain: Vec<f32>, range: Vec<f32>, code: Vec<Op> },
    /// An array of 1-output functions.
    Many(Vec<Func>),
}

#[derive(Clone, Debug)]
pub enum Op {
    Num(f32),
    Op(String),
    /// A procedure, run by `if` or `ifelse`.
    Block(Vec<Op>),
    IfElse(Vec<Op>, Vec<Op>),
}

fn floats(f: &PdfFile, d: &Dict, k: &str) -> Vec<f32> {
    f.nums(d.get(k).unwrap_or(&Obj::Null)).into_iter().map(|v| v as f32).collect()
}

impl Func {
    pub fn load(f: &PdfFile, o: &Obj) -> Option<Func> {
        Self::load_depth(f, o, 0, &mut 256)
    }

    /// `budget`: how many functions may still be read in all (one named
    /// many times in a stitching function would multiply at each level).
    fn load_depth(f: &PdfFile, o: &Obj, depth: u32, budget: &mut u32) -> Option<Func> {
        if depth > 6 || *budget == 0 {
            return None;
        }
        *budget -= 1;
        let o = f.resolve(o);
        if let Obj::Array(a) = &o {
            let fs: Vec<Func> = a.iter().filter_map(|x| Self::load_depth(f, x, depth + 1, budget)).collect();
            return (fs.len() == a.len()).then_some(Func::Many(fs));
        }
        if let Obj::Name(n) = &o
            && &**n == "Identity"
        {
            return Some(Func::Identity);
        }
        let (d, stream) = match &o {
            Obj::Dict(d) => (d.clone(), None),
            Obj::Stream(d, raw) => (d.clone(), Some(f.decode(d, raw).data)),
            _ => return None,
        };
        let domain = floats(f, &d, "Domain");
        let range = floats(f, &d, "Range");
        match f.key(&d, "FunctionType").int()? {
            0 => {
                let data = stream?;
                let size: Vec<usize> = f.nums(d.get("Size").unwrap_or(&Obj::Null)).into_iter().map(|v| v.max(1.0) as usize).collect();
                let m = domain.len() / 2;
                let outs = range.len() / 2;
                if size.len() != m || m == 0 || outs == 0 || m > 4 {
                    return None;
                }
                let bps = f.key_num(&d, "BitsPerSample").unwrap_or(8.0) as usize;
                if !matches!(bps, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
                    return None;
                }
                let total = size.iter().try_fold(outs, |a, &s| a.checked_mul(s))?;
                if total > 4_000_000 {
                    return None;
                }
                let mut encode = floats(f, &d, "Encode");
                if encode.len() != 2 * m {
                    encode = size.iter().flat_map(|&s| [0.0, (s - 1) as f32]).collect();
                }
                let mut decode = floats(f, &d, "Decode");
                if decode.len() != 2 * outs {
                    decode = range.clone();
                }
                let max = ((1u64 << bps) - 1) as f32;
                let mut samples = Vec::with_capacity(total);
                let mut bit = 0usize;
                for _ in 0..total {
                    let mut v: u64 = 0;
                    for _ in 0..bps {
                        let byte = data.get(bit / 8).copied().unwrap_or(0);
                        v = v << 1 | ((byte >> (7 - bit % 8)) & 1) as u64;
                        bit += 1;
                    }
                    samples.push(v as f32 / max);
                }
                Some(Func::Sampled { domain, range, size, encode, decode, samples, outs })
            }
            2 => {
                let c0 = d.get("C0").map(|_| floats(f, &d, "C0")).unwrap_or_else(|| vec![0.0]);
                let c1 = d.get("C1").map(|_| floats(f, &d, "C1")).unwrap_or_else(|| vec![1.0]);
                let n = f.key_num(&d, "N").unwrap_or(1.0) as f32;
                let dm = if domain.len() >= 2 { [domain[0], domain[1]] } else { [0.0, 1.0] };
                Some(Func::Exp { domain: dm, c0, c1, n })
            }
            3 => {
                let funcs: Vec<Func> = match f.key(&d, "Functions") {
                    Obj::Array(a) => a.iter().filter_map(|x| Self::load_depth(f, x, depth + 1, budget)).collect(),
                    _ => return None,
                };
                let dm = if domain.len() >= 2 { [domain[0], domain[1]] } else { [0.0, 1.0] };
                let bounds = floats(f, &d, "Bounds");
                let mut encode = floats(f, &d, "Encode");
                if encode.len() < funcs.len() * 2 {
                    encode = funcs.iter().flat_map(|_| [0.0, 1.0]).collect();
                }
                if funcs.is_empty() {
                    return None;
                }
                Some(Func::Stitch { domain: dm, funcs, bounds, encode })
            }
            4 => {
                let data = stream?;
                let code = ps_parse(&data)?;
                Some(Func::Ps { domain, range, code })
            }
            _ => None,
        }
    }

    /// About how many steps one evaluation takes.
    pub fn work(&self) -> usize {
        fn ops(code: &[Op]) -> usize {
            code.iter()
                .map(|o| match o {
                    Op::Block(b) => 1 + ops(b),
                    Op::IfElse(a, b) => 1 + ops(a).max(ops(b)),
                    _ => 1,
                })
                .fold(0usize, usize::saturating_add)
        }
        match self {
            Func::Ps { code, .. } => ops(code),
            Func::Stitch { funcs, .. } => funcs.iter().map(Func::work).max().unwrap_or(1),
            Func::Many(fs) => fs.iter().map(Func::work).fold(0usize, usize::saturating_add),
            _ => 1,
        }
    }

    /// Evaluates the function: the outputs go to `out`, their count is
    /// returned.
    pub fn eval(&self, input: &[f32], out: &mut [f32]) -> usize {
        match self {
            Func::Identity => {
                let n = input.len().min(out.len());
                out[..n].copy_from_slice(&input[..n]);
                n
            }
            Func::Many(fs) => {
                let mut k = 0;
                for f in fs {
                    if k >= out.len() {
                        break;
                    }
                    let mut o = [0.0f32; 8];
                    f.eval(input, &mut o);
                    out[k] = o[0];
                    k += 1;
                }
                k
            }
            Func::Exp { domain, c0, c1, n } => {
                let x = input.first().copied().unwrap_or(0.0).max(domain[0]).min(domain[1]);
                let k = c0.len().max(c1.len()).min(out.len());
                let xn = if *n == 1.0 { x } else { x.powf(*n) };
                for i in 0..k {
                    let a = c0.get(i).copied().unwrap_or(0.0);
                    let b = c1.get(i).copied().unwrap_or(1.0);
                    out[i] = a + xn * (b - a);
                }
                k
            }
            Func::Stitch { domain, funcs, bounds, encode } => {
                let x = input.first().copied().unwrap_or(0.0).max(domain[0]).min(domain[1]);
                let i = bounds.partition_point(|&b| x >= b).min(funcs.len() - 1);
                let lo = if i == 0 { domain[0] } else { bounds[i - 1] };
                let hi = if i < bounds.len() { bounds[i] } else { domain[1] };
                let (e0, e1) = (encode[2 * i], encode[2 * i + 1]);
                let t = if hi > lo { e0 + (x - lo) / (hi - lo) * (e1 - e0) } else { e0 };
                funcs[i].eval(&[t], out)
            }
            Func::Sampled { domain, range, size, encode, decode, samples, outs } => {
                let m = size.len();
                let k = (*outs).min(out.len());
                // The position in each input dimension.
                let mut idx = [0usize; 4];
                let mut frac = [0f32; 4];
                for j in 0..m {
                    let x = input.get(j).copied().unwrap_or(0.0).max(domain[2 * j]).min(domain[2 * j + 1]);
                    let (d0, d1) = (domain[2 * j], domain[2 * j + 1]);
                    let e = if d1 > d0 { encode[2 * j] + (x - d0) / (d1 - d0) * (encode[2 * j + 1] - encode[2 * j]) } else { encode[2 * j] };
                    let e = e.clamp(0.0, (size[j] - 1) as f32);
                    idx[j] = (e.floor() as usize).min(size[j] - 1);
                    frac[j] = if idx[j] + 1 < size[j] { e - idx[j] as f32 } else { 0.0 };
                }
                // The sample at the lower corner plus each dimension's step
                // on its own, added up: pdfium's interpolation, not a
                // multilinear one.
                let mut stride = [0usize; 4];
                let mut base = 0usize;
                let mut st = 1usize;
                for j in 0..m {
                    stride[j] = st;
                    base += idx[j] * st;
                    st *= size[j];
                }
                let at = |off: usize, o: usize| samples.get(off * outs + o).copied().unwrap_or(0.0);
                for (o, slot) in out.iter_mut().enumerate().take(k) {
                    let s0 = at(base, o);
                    let mut acc = s0;
                    for j in 0..m {
                        if frac[j] > 0.0 {
                            acc += frac[j] * (at(base + stride[j], o) - s0);
                        }
                    }
                    let v = decode[2 * o] + acc * (decode[2 * o + 1] - decode[2 * o]);
                    *slot = v.max(range[2 * o].min(range[2 * o + 1])).min(range[2 * o].max(range[2 * o + 1]));
                }
                k
            }
            Func::Ps { domain, range, code } => {
                let mut st: Vec<f32> = Vec::with_capacity(32);
                for (j, x) in input.iter().enumerate() {
                    let (a, b) = (domain.get(2 * j).copied().unwrap_or(0.0), domain.get(2 * j + 1).copied().unwrap_or(1.0));
                    st.push(x.max(a.min(b)).min(a.max(b)));
                }
                ps_run(code, &mut st, 0);
                let k = (range.len() / 2).min(out.len());
                let base = st.len().saturating_sub(k);
                for i in 0..k {
                    let v = st.get(base + i).copied().unwrap_or(0.0);
                    out[i] = v.max(range[2 * i]).min(range[2 * i + 1]);
                }
                k
            }
        }
    }
}

fn ps_parse(data: &[u8]) -> Option<Vec<Op>> {
    let mut lx = Lexer::new(data, 0);
    fn block(lx: &mut Lexer, depth: u32) -> Option<Vec<Op>> {
        let mut out = Vec::new();
        loop {
            lx.skip_ws();
            let p = lx.p;
            match lx.d.get(p) {
                None => return Some(out),
                Some(b'{') => {
                    lx.p += 1;
                    if depth > 64 {
                        return None;
                    }
                    let inner = block(lx, depth + 1)?;
                    out.push(Op::Block(inner));
                }
                Some(b'}') => {
                    lx.p += 1;
                    return Some(out);
                }
                _ => match lx.next(false)? {
                    Item::Obj(o) => out.push(Op::Num(o.num().unwrap_or(0.0) as f32)),
                    Item::Kw(k) => {
                        let k = String::from_utf8_lossy(k).into_owned();
                        match k.as_str() {
                            "if" => {
                                if let Some(Op::Block(b)) = out.pop() {
                                    out.push(Op::IfElse(b, Vec::new()));
                                }
                                continue;
                            }
                            "ifelse" => {
                                let b = out.pop();
                                let a = out.pop();
                                if let (Some(Op::Block(a)), Some(Op::Block(b))) = (a, b) {
                                    out.push(Op::IfElse(a, b));
                                }
                                continue;
                            }
                            _ => {}
                        }
                        out.push(Op::Op(k));
                    }
                },
            }
        }
    }
    let ops = block(&mut lx, 0)?;
    // The whole program is one block.
    match ops.into_iter().next() {
        Some(Op::Block(b)) => Some(b),
        _ => None,
    }
}

thread_local! {
    /// Function steps the page being drawn may still run.
    static FUNC_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

/// Sets the steps functions may run, giving back what was left before.
pub(crate) fn set_func_budget(n: usize) -> usize {
    FUNC_STEPS.with(|c| c.replace(n))
}

/// Takes `n` more steps from the budget (an operator moving many values).
fn charge(n: usize) {
    FUNC_STEPS.with(|c| c.set(c.get().saturating_sub(n)));
}

fn ps_run(code: &[Op], st: &mut Vec<f32>, depth: u32) {
    if depth > 64 {
        return;
    }
    // Past the page's budget a function gives nothing more.
    let left = FUNC_STEPS.with(|c| {
        let left = c.get();
        c.set(left.saturating_sub(code.len()));
        left
    });
    if left < code.len() {
        return;
    }
    let pop = |st: &mut Vec<f32>| st.pop().unwrap_or(0.0);
    for op in code {
        if st.len() > 1000 {
            return;
        }
        match op {
            Op::Num(v) => st.push(*v),
            // A bare procedure is not run.
            Op::Block(_) => {}
            Op::IfElse(a, b) => {
                let c = pop(st);
                ps_run(if c != 0.0 { a } else { b }, st, depth + 1);
            }
            Op::Op(k) => {
                let b2 = |st: &mut Vec<f32>, f: &dyn Fn(f32, f32) -> f32| {
                    let b = pop(st);
                    let a = pop(st);
                    st.push(f(a, b));
                };
                let b1 = |st: &mut Vec<f32>, f: &dyn Fn(f32) -> f32| {
                    let a = pop(st);
                    st.push(f(a));
                };
                let t = |c: bool| if c { 1.0 } else { 0.0 };
                match k.as_str() {
                    "add" => b2(st, &|a, b| a + b),
                    "sub" => b2(st, &|a, b| a - b),
                    "mul" => b2(st, &|a, b| a * b),
                    "div" => b2(st, &|a, b| if b == 0.0 { 0.0 } else { a / b }),
                    "idiv" => b2(st, &|a, b| if b as i64 == 0 { 0.0 } else { (a as i64).wrapping_div(b as i64) as f32 }),
                    "mod" => b2(st, &|a, b| if b as i64 == 0 { 0.0 } else { (a as i64).wrapping_rem(b as i64) as f32 }),
                    "neg" => b1(st, &|a| -a),
                    "abs" => b1(st, &|a| a.abs()),
                    "ceiling" => b1(st, &|a| a.ceil()),
                    "floor" => b1(st, &|a| a.floor()),
                    "round" => b1(st, &|a| (a + 0.5).floor()),
                    "truncate" => b1(st, &|a| a.trunc()),
                    "sqrt" => b1(st, &|a| a.max(0.0).sqrt()),
                    "sin" => b1(st, &|a| a.to_radians().sin()),
                    "cos" => b1(st, &|a| a.to_radians().cos()),
                    "atan" => b2(st, &|a, b| {
                        let d = a.atan2(b).to_degrees();
                        if d < 0.0 { d + 360.0 } else { d }
                    }),
                    "exp" => b2(st, &|a, b| a.powf(b)),
                    "ln" => b1(st, &|a| a.max(1e-30).ln()),
                    "log" => b1(st, &|a| a.max(1e-30).log10()),
                    "cvi" => b1(st, &|a| a.trunc()),
                    "cvr" => {}
                    "eq" => b2(st, &|a, b| t(a == b)),
                    "ne" => b2(st, &|a, b| t(a != b)),
                    "gt" => b2(st, &|a, b| t(a > b)),
                    "ge" => b2(st, &|a, b| t(a >= b)),
                    "lt" => b2(st, &|a, b| t(a < b)),
                    "le" => b2(st, &|a, b| t(a <= b)),
                    "and" => b2(st, &|a, b| (a as i64 & b as i64) as f32),
                    "or" => b2(st, &|a, b| (a as i64 | b as i64) as f32),
                    "xor" => b2(st, &|a, b| (a as i64 ^ b as i64) as f32),
                    "not" => b1(st, &|a| if a == 0.0 { 1.0 } else if a == 1.0 { 0.0 } else { !(a as i64) as f32 }),
                    "bitshift" => b2(st, &|a, b| if b >= 0.0 { ((a as i64) << (b as i64).min(62)) as f32 } else { ((a as i64) >> (-b as i64).min(62)) as f32 }),
                    "true" => st.push(1.0),
                    "false" => st.push(0.0),
                    "pop" => {
                        pop(st);
                    }
                    "dup" => {
                        let a = st.last().copied().unwrap_or(0.0);
                        st.push(a);
                    }
                    "exch" => {
                        let b = pop(st);
                        let a = pop(st);
                        st.push(b);
                        st.push(a);
                    }
                    "copy" => {
                        let n = pop(st).max(0.0) as usize;
                        let l = st.len();
                        if n <= l {
                            charge(n);
                            for i in l - n..l {
                                st.push(st[i]);
                            }
                        }
                    }
                    "index" => {
                        let n = pop(st).max(0.0) as usize;
                        let v = st.len().checked_sub(n.saturating_add(1)).map_or(0.0, |i| st[i]);
                        st.push(v);
                    }
                    "roll" => {
                        let j = pop(st) as i64;
                        let n = pop(st).max(0.0) as usize;
                        let l = st.len();
                        if n > 0 && n <= l {
                            charge(n);
                            let s = &mut st[l - n..];
                            let j = j.rem_euclid(n as i64) as usize;
                            s.rotate_right(j);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
