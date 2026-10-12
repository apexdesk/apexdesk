//! Office's preset shape geometries, read from the definitions of the
//! standard (ECMA-376, `presetShapeDefinitions.xml`, kept in
//! `data/preset-shapes.xml`): guides are evaluated for a box and its
//! adjust values, and the paths are flattened into polygons, so every
//! preset draws as it should in documents, sheets and slides.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::OnceLock;

const DATA: &str = include_str!("../data/preset-shapes.xml");

/// How a path is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetFill {
    None,
    Norm,
    Lighten,
    LightenLess,
    Darken,
    DarkenLess,
}

/// One path of a preset, flattened: its subpaths (points in the box, and
/// whether each is closed), how it is filled and whether it is stroked.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetPath {
    pub subpaths: Vec<(Vec<(f32, f32)>, bool)>,
    pub fill: PresetFill,
    pub stroke: bool,
}

#[derive(Clone, Copy, Debug)]
enum Cmd {
    Move(usize),
    Line(usize),
    Arc(usize),
    Quad(usize),
    Cubic(usize),
    Close,
}

#[derive(Debug, Default)]
struct PathDef {
    w: f64,
    h: f64,
    fill: Option<PresetFill>,
    stroke: bool,
    cmds: Vec<Cmd>,
    /// Operands of the commands: points (x, y) or arc values, as guide names or numbers.
    args: Vec<[Box<str>; 2]>,
}

#[derive(Debug, Default)]
struct Def {
    av: Vec<(Box<str>, Box<str>)>,
    gd: Vec<(Box<str>, Box<str>)>,
    cxn: Vec<[Box<str>; 3]>,
    rect: Option<[Box<str>; 4]>,
    paths: Vec<PathDef>,
}

struct Defs {
    order: Vec<Box<str>>,
    map: HashMap<Box<str>, Def>,
}

/// A tag of the data: name, attributes, and whether it opens, closes or is empty.
struct Tag<'a> {
    name: &'a str,
    attrs: &'a str,
    close: bool,
    empty: bool,
}

fn tags(s: &str) -> impl Iterator<Item = Tag<'_>> {
    let mut rest = s;
    std::iter::from_fn(move || loop {
        let i = rest.find('<')?;
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix("!--") {
            let e = r.find("-->")?;
            rest = &r[e + 3..];
            continue;
        }
        let e = rest.find('>')?;
        let mut body = &rest[..e];
        rest = &rest[e + 1..];
        let close = body.starts_with('/');
        if close {
            body = &body[1..];
        }
        let empty = body.ends_with('/');
        if empty {
            body = &body[..body.len() - 1];
        }
        let (name, attrs) = body.split_once(' ').unwrap_or((body, ""));
        return Some(Tag { name, attrs, close, empty });
    })
}

fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while let Some(eq) = rest.find("=\"") {
        let k = rest[..eq].trim();
        let r = &rest[eq + 2..];
        let end = r.find('"')?;
        if k == key {
            return Some(&r[..end]);
        }
        rest = &r[end + 1..];
    }
    None
}

fn boxed(attrs: &str, key: &str) -> Box<str> {
    attr(attrs, key).unwrap_or("0").into()
}

fn parse() -> Defs {
    let mut defs = Defs { order: Vec::new(), map: HashMap::new() };
    let mut depth = 0;
    let mut cur: Option<(Box<str>, Def)> = None;
    // Which list a `gd` belongs to.
    let mut in_av = false;
    for t in tags(DATA) {
        if t.close {
            depth -= 1;
            if depth == 1 {
                if let Some((n, d)) = cur.take() {
                    defs.order.push(n.clone());
                    defs.map.insert(n, d);
                }
            }
            if t.name == "avLst" {
                in_av = false;
            }
            continue;
        }
        if !t.empty {
            depth += 1;
        }
        if depth == 2 && !t.empty {
            cur = Some((t.name.into(), Def::default()));
            continue;
        }
        let Some((_, d)) = cur.as_mut() else { continue };
        let a = t.attrs;
        match t.name {
            "avLst" => in_av = !t.empty,
            "gd" => {
                let g = (boxed(a, "name"), boxed(a, "fmla"));
                if in_av { d.av.push(g) } else { d.gd.push(g) }
            }
            "cxn" => d.cxn.push([boxed(a, "ang"), "".into(), "".into()]),
            "pos" => {
                if let Some(c) = d.cxn.last_mut() {
                    c[1] = boxed(a, "x");
                    c[2] = boxed(a, "y");
                }
            }
            "rect" => d.rect = Some([boxed(a, "l"), boxed(a, "t"), boxed(a, "r"), boxed(a, "b")]),
            "path" => {
                let num = |k| attr(a, k).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let fill = match attr(a, "fill") {
                    Some("none") => Some(PresetFill::None),
                    Some("lighten") => Some(PresetFill::Lighten),
                    Some("lightenLess") => Some(PresetFill::LightenLess),
                    Some("darken") => Some(PresetFill::Darken),
                    Some("darkenLess") => Some(PresetFill::DarkenLess),
                    _ => None,
                };
                d.paths.push(PathDef { w: num("w"), h: num("h"), fill, stroke: attr(a, "stroke") != Some("false"), ..Default::default() });
            }
            "moveTo" | "lnTo" | "quadBezTo" | "cubicBezTo" | "arcTo" | "close" => {
                let Some(p) = d.paths.last_mut() else { continue };
                let at = p.args.len();
                p.cmds.push(match t.name {
                    "moveTo" => Cmd::Move(at),
                    "lnTo" => Cmd::Line(at),
                    "quadBezTo" => Cmd::Quad(at),
                    "cubicBezTo" => Cmd::Cubic(at),
                    "close" => Cmd::Close,
                    _ => {
                        p.args.push([boxed(a, "wR"), boxed(a, "hR")]);
                        p.args.push([boxed(a, "stAng"), boxed(a, "swAng")]);
                        Cmd::Arc(at)
                    }
                });
            }
            "pt" => {
                if let Some(p) = d.paths.last_mut() {
                    p.args.push([boxed(a, "x"), boxed(a, "y")]);
                }
            }
            _ => {}
        }
    }
    defs
}

fn defs() -> &'static Defs {
    static DEFS: OnceLock<Defs> = OnceLock::new();
    DEFS.get_or_init(parse)
}

/// The names of all the presets, in the standard's order.
pub fn preset_names() -> impl Iterator<Item = &'static str> {
    defs().order.iter().map(|n| &**n)
}

/// Whether `name` is a preset we have the geometry of.
pub fn is_preset(name: &str) -> bool {
    defs().map.contains_key(name)
}

/// The adjust values of a preset and their defaults, in order.
pub fn preset_adjusts(name: &str) -> Vec<(&'static str, i64)> {
    let Some(d) = defs().map.get(name) else { return Vec::new() };
    d.av.iter().map(|(n, f)| (&**n, f.strip_prefix("val ").and_then(|v| v.trim().parse().ok()).unwrap_or(0))).collect()
}

/// Guide values of a preset for a `w` by `h` box.
struct Guides<'a> {
    vals: Vec<(&'a str, f64)>,
    w: f64,
    h: f64,
}

impl<'a> Guides<'a> {
    fn new(d: &'a Def, w: f64, h: f64, adj: &[(impl AsRef<str>, i64)]) -> Guides<'a> {
        let mut g = Guides { vals: Vec::with_capacity(d.av.len() + d.gd.len()), w, h };
        for (n, f) in &d.av {
            let v = match adj.iter().find(|(k, _)| k.as_ref() == &**n) {
                Some((_, v)) => *v as f64,
                None => g.formula(f),
            };
            g.vals.push((n, v));
        }
        for (n, f) in &d.gd {
            let v = g.formula(f);
            g.vals.push((n, v));
        }
        g
    }

    fn get(&self, s: &str) -> f64 {
        if let Some(&(_, v)) = self.vals.iter().rev().find(|(n, _)| *n == s) {
            return v;
        }
        let (w, h) = (self.w, self.h);
        let ss = w.min(h);
        let div = |pre: &str, base: f64| s.strip_prefix(pre).and_then(|d| d.parse::<f64>().ok()).map(|d| base / d);
        match s {
            "w" | "r" => w,
            "h" | "b" => h,
            "l" | "t" => 0.0,
            "hc" => w / 2.0,
            "vc" => h / 2.0,
            "ss" => ss,
            "ls" => w.max(h),
            "cd2" => 10_800_000.0,
            "cd4" => 5_400_000.0,
            "cd8" => 2_700_000.0,
            "3cd4" => 16_200_000.0,
            "3cd8" => 8_100_000.0,
            "5cd8" => 13_500_000.0,
            "7cd8" => 18_900_000.0,
            _ => div("wd", w).or_else(|| div("hd", h)).or_else(|| div("ssd", ss)).or_else(|| s.parse().ok()).unwrap_or(0.0),
        }
    }

    fn formula(&self, f: &str) -> f64 {
        let mut it = f.split_whitespace();
        let op = it.next().unwrap_or("");
        let a: Vec<f64> = it.map(|x| self.get(x)).collect();
        let x = a.first().copied().unwrap_or(0.0);
        let y = a.get(1).copied().unwrap_or(0.0);
        let z = a.get(2).copied().unwrap_or(0.0);
        let rad = |v: f64| v / 60_000.0 * PI / 180.0;
        let q = |n: f64, d: f64| if d == 0.0 { 0.0 } else { n / d };
        let v = match op {
            "val" => x,
            "*/" => q(x * y, z),
            "+-" => x + y - z,
            "+/" => q(x + y, z),
            "?:" => if x > 0.0 { y } else { z },
            "abs" => x.abs(),
            "at2" => y.atan2(x) * 180.0 / PI * 60_000.0,
            "cat2" => x * z.atan2(y).cos(),
            "sat2" => x * z.atan2(y).sin(),
            "cos" => x * rad(y).cos(),
            "sin" => x * rad(y).sin(),
            "tan" => x * rad(y).tan(),
            "max" => x.max(y),
            "min" => x.min(y),
            "mod" => (x * x + y * y + z * z).sqrt(),
            "pin" => if y < x { x } else if y > z { z } else { y },
            "sqrt" => x.max(0.0).sqrt(),
            _ => 0.0,
        };
        if v.is_finite() { v } else { 0.0 }
    }
}

/// The paths of preset `name` in a `w` by `h` box (points from its
/// top-left corner), with adjust values `adj` (the others take their
/// defaults); None for a name we don't know.
pub fn preset_paths(name: &str, w: f32, h: f32, adj: &[(impl AsRef<str>, i64)]) -> Option<Vec<PresetPath>> {
    let d = defs().map.get(name)?;
    let g = Guides::new(d, w as f64, h as f64, adj);
    Some(d.paths.iter().map(|p| flatten(p, &g)).collect())
}

fn flatten(p: &PathDef, g: &Guides) -> PresetPath {
    // Paths with their own size are scaled into the box.
    let sx = if p.w > 0.0 { g.w / p.w } else { 1.0 };
    let sy = if p.h > 0.0 { g.h / p.h } else { 1.0 };
    let pt = |i: usize| {
        let [x, y] = &p.args[i];
        (g.get(x), g.get(y))
    };
    let mut subs: Vec<(Vec<(f64, f64)>, bool)> = Vec::new();
    let mut cur = (0.0, 0.0);
    let push = |subs: &mut Vec<(Vec<(f64, f64)>, bool)>, q: (f64, f64)| {
        if subs.is_empty() || subs.last().unwrap().1 {
            let start = subs.last().and_then(|s| s.0.first().copied()).unwrap_or(q);
            subs.push((vec![start], false));
        }
        subs.last_mut().unwrap().0.push(q);
    };
    for c in &p.cmds {
        match *c {
            Cmd::Move(i) => {
                cur = pt(i);
                subs.push((vec![cur], false));
            }
            Cmd::Line(i) => {
                cur = pt(i);
                push(&mut subs, cur);
            }
            Cmd::Quad(i) => {
                let (c1, e) = (pt(i), pt(i + 1));
                for k in 1..=12 {
                    let t = k as f64 / 12.0;
                    let u = 1.0 - t;
                    push(&mut subs, (u * u * cur.0 + 2.0 * u * t * c1.0 + t * t * e.0, u * u * cur.1 + 2.0 * u * t * c1.1 + t * t * e.1));
                }
                cur = e;
            }
            Cmd::Cubic(i) => {
                let (c1, c2, e) = (pt(i), pt(i + 1), pt(i + 2));
                for k in 1..=16 {
                    let t = k as f64 / 16.0;
                    let u = 1.0 - t;
                    let (a, b, cc, dd) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    push(&mut subs, (a * cur.0 + b * c1.0 + cc * c2.0 + dd * e.0, a * cur.1 + b * c1.1 + cc * c2.1 + dd * e.1));
                }
                cur = e;
            }
            Cmd::Arc(i) => {
                let (wr, hr) = pt(i);
                let (st, sw) = pt(i + 1);
                let deg = |v: f64| v / 60_000.0 * PI / 180.0;
                // The angles are seen on the ellipse: turn them into its parameter.
                let param = |a: f64| (wr * a.sin()).atan2(hr * a.cos());
                let (a0, a1) = (deg(st), deg(st + sw));
                let t0 = param(a0);
                let mut dt = param(a1) - t0;
                let full = deg(sw.abs()) >= 2.0 * PI - 1e-9;
                if full {
                    dt = 2.0 * PI * sw.signum();
                } else if sw > 0.0 && dt < 0.0 {
                    dt += 2.0 * PI;
                } else if sw < 0.0 && dt > 0.0 {
                    dt -= 2.0 * PI;
                }
                let center = (cur.0 - wr * t0.cos(), cur.1 - hr * t0.sin());
                // Steps short enough to stay within a fifth of a unit of the curve.
                let r = (wr.abs() * sx).max(hr.abs() * sy).max(1.0);
                let step = 2.0 * (1.0 - 0.2 / r).max(0.0).acos();
                let n = ((dt.abs() / step.max(0.01)).ceil() as usize).clamp(2, 96);
                for k in 1..=n {
                    let t = t0 + dt * k as f64 / n as f64;
                    cur = (center.0 + wr * t.cos(), center.1 + hr * t.sin());
                    push(&mut subs, cur);
                }
            }
            Cmd::Close => {
                if let Some(s) = subs.last_mut() {
                    s.1 = true;
                    if let Some(&f) = s.0.first() {
                        cur = f;
                    }
                }
            }
        }
    }
    let fin = |v: f64| if v.is_finite() { v as f32 } else { 0.0 };
    PresetPath {
        subpaths: subs.into_iter().filter(|s| s.0.len() > 1).map(|(pts, closed)| (pts.into_iter().map(|(x, y)| (fin(x * sx), fin(y * sy))).collect(), closed)).collect(),
        fill: p.fill.unwrap_or(PresetFill::Norm),
        stroke: p.stroke,
    }
}

/// The text rectangle of preset `name` in a `w` by `h` box: left, top,
/// right, bottom (the whole box when the preset says nothing).
pub fn preset_text_rect(name: &str, w: f32, h: f32, adj: &[(impl AsRef<str>, i64)]) -> Option<[f32; 4]> {
    let d = defs().map.get(name)?;
    let g = Guides::new(d, w as f64, h as f64, adj);
    Some(match &d.rect {
        Some(r) => r.clone().map(|v| g.get(&v) as f32),
        None => [0.0, 0.0, w, h],
    })
}

/// The connection sites of preset `name` in a `w` by `h` box: each point,
/// and the direction a connector leaves it in (degrees, clockwise from
/// the right).
pub fn preset_sites(name: &str, w: f32, h: f32, adj: &[(impl AsRef<str>, i64)]) -> Vec<((f32, f32), f32)> {
    let Some(d) = defs().map.get(name) else { return Vec::new() };
    let g = Guides::new(d, w as f64, h as f64, adj);
    d.cxn.iter().map(|[a, x, y]| ((g.get(x) as f32, g.get(y) as f32), (g.get(a) / 60_000.0) as f32)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: &[(&str, i64)] = &[];

    #[test]
    fn every_preset_draws_inside_reason() {
        let names: Vec<_> = preset_names().collect();
        assert!(names.len() > 180, "{}", names.len());
        for n in &names {
            for (w, h) in [(100.0, 60.0), (40.0, 120.0), (0.0, 50.0)] {
                let paths = preset_paths(n, w, h, NONE).unwrap();
                assert!(!paths.is_empty(), "{n}");
                for p in &paths {
                    for (pts, _) in &p.subpaths {
                        for &(x, y) in pts {
                            assert!(x.is_finite() && y.is_finite(), "{n}");
                            // Callouts point out of their box, but not far.
                            let l: f32 = w.max(h);
                            assert!(x > -3.0 * l && x < 4.0 * l, "{n} {x}");
                            assert!(y > -3.0 * l && y < 4.0 * l, "{n} {y}");
                        }
                    }
                }
            }
            let _ = preset_text_rect(n, 100.0, 60.0, NONE).unwrap();
            let _ = preset_sites(n, 100.0, 60.0, NONE);
        }
        assert!(preset_paths("nothing", 1.0, 1.0, NONE).is_none());
    }

    #[test]
    fn shapes_come_out_right() {
        // A rectangle is its box.
        let r = preset_paths("rect", 100.0, 50.0, NONE).unwrap();
        assert_eq!(r[0].subpaths[0].0, vec![(0.0, 0.0), (100.0, 0.0), (100.0, 50.0), (0.0, 50.0)]);
        assert!(r[0].subpaths[0].1);
        // An ellipse touches the middle of each side.
        let e = &preset_paths("ellipse", 100.0, 50.0, NONE).unwrap()[0].subpaths[0].0;
        let near = |q: (f32, f32)| e.iter().any(|p| (p.0 - q.0).abs() < 0.5 && (p.1 - q.1).abs() < 0.5);
        assert!(near((0.0, 25.0)) && near((50.0, 0.0)) && near((100.0, 25.0)) && near((50.0, 50.0)));
        // A donut has a hole.
        assert_eq!(preset_paths("donut", 100.0, 100.0, NONE).unwrap()[0].subpaths.len(), 2);
        // Adjust values move the geometry: a round rectangle's corners.
        let at = |adj: &[(&str, i64)]| preset_paths("roundRect", 100.0, 100.0, adj).unwrap()[0].subpaths[0].0[0];
        assert!((at(NONE).1 - 16.667).abs() < 0.01);
        assert!((at(&[("adj", 50000)]).1 - 50.0).abs() < 0.01);
        assert_eq!(preset_adjusts("roundRect"), vec![("adj", 16667)]);
        // A cube shades its sides.
        let c = preset_paths("can", 60.0, 100.0, NONE).unwrap();
        assert!(c.iter().any(|p| p.fill == PresetFill::Lighten));
        // Text rectangles and connection sites.
        assert_eq!(preset_text_rect("rect", 10.0, 20.0, NONE), Some([0.0, 0.0, 10.0, 20.0]));
        let sites = preset_sites("rect", 10.0, 20.0, NONE);
        assert!(sites.contains(&((5.0, 0.0), 270.0)) && sites.len() == 4);
        // A bent connector goes from corner to corner.
        let b = &preset_paths("bentConnector3", 100.0, 40.0, NONE).unwrap()[0];
        assert_eq!(b.fill, PresetFill::None);
        assert_eq!(b.subpaths[0].0.first(), Some(&(0.0, 0.0)));
        assert_eq!(b.subpaths[0].0.last(), Some(&(100.0, 40.0)));
    }
}
