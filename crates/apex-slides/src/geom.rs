//! Drawn shapes (`a:custGeom`): their paths, flattened to points in the
//! shape's box, the way `apex_doc::preset_paths` gives a preset's. Guides
//! are not evaluated: coordinates that name one count as 0 (files written
//! by Office and LibreOffice use plain numbers in the paths).

use apex_doc::{PresetFill, PresetPath};

/// A tag of the XML: its local name, attributes and whether it closes.
struct Tag<'a> {
    name: &'a str,
    attrs: &'a str,
    close: bool,
    /// Written `<x/>`.
    empty: bool,
}

impl Tag<'_> {
    fn attr(&self, key: &str) -> Option<&str> {
        let mut rest = self.attrs;
        while let Some(i) = rest.find('=') {
            let k = rest[..i].trim();
            let after = rest[i + 1..].trim_start();
            let q = after.chars().next()?;
            let body = &after[1..];
            let end = body.find(q)?;
            if k == key || k.rsplit(':').next() == Some(key) {
                return Some(&body[..end]);
            }
            rest = &body[end + 1..];
        }
        None
    }

    fn num(&self, key: &str) -> f32 {
        self.attr(key).and_then(|v| v.parse::<f64>().ok()).map_or(0.0, |v| v as f32)
    }
}

/// The tags of `xml`, in order (comments and text skipped).
fn tags(xml: &str) -> Vec<Tag<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let Some(end) = rest.find('>') else { break };
        let body = &rest[..end];
        rest = &rest[end + 1..];
        if body.starts_with('?') || body.starts_with('!') {
            continue;
        }
        let close = body.starts_with('/');
        let empty = body.ends_with('/');
        let body = body.trim_start_matches('/').trim_end_matches('/');
        let (name, attrs) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
        let name = name.rsplit(':').next().unwrap_or(name);
        out.push(Tag { name, attrs, close, empty });
    }
    out
}

/// Points along an arc of the ellipse with radii `rx`, `ry` starting at
/// `from` (at angle `st`, degrees) and turning `sw` degrees.
fn arc(from: (f32, f32), rx: f32, ry: f32, st: f32, sw: f32, out: &mut Vec<(f32, f32)>) -> (f32, f32) {
    let (a0, da) = (st.to_radians(), sw.to_radians());
    let c = (from.0 - rx * a0.cos(), from.1 - ry * a0.sin());
    let n = ((sw.abs() / 10.0).ceil() as usize).clamp(1, 72);
    let mut last = from;
    for k in 1..=n {
        let a = a0 + da * k as f32 / n as f32;
        last = (c.0 + rx * a.cos(), c.1 + ry * a.sin());
        out.push(last);
    }
    last
}

/// A cubic Bézier from `p0`, flattened.
fn cubic(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), out: &mut Vec<(f32, f32)>) {
    const N: usize = 16;
    for k in 1..=N {
        let t = k as f32 / N as f32;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1));
    }
}

/// The paths of a drawn shape's `a:custGeom` in a `w` by `h` box.
pub fn custom_paths(xml: &str, w: f32, h: f32) -> Vec<PresetPath> {
    let mut out = Vec::new();
    let tags = tags(xml);
    let mut i = 0;
    // Points made in all (each arc makes dozens, and every paint makes them).
    let mut made = 0usize;
    while i < tags.len() {
        let t = &tags[i];
        i += 1;
        if t.name != "path" || t.close {
            continue;
        }
        if made > MAX_POINTS {
            break;
        }
        // Path coordinates are in its own w by h box (the shape's if unset).
        let (pw, ph) = (t.num("w"), t.num("h"));
        let (sx, sy) = (if pw > 0.0 { w / pw } else { 1.0 }, if ph > 0.0 { h / ph } else { 1.0 });
        let fill = if t.attr("fill") == Some("none") { PresetFill::None } else { PresetFill::Norm };
        let stroke = !matches!(t.attr("stroke"), Some("0" | "false"));
        let empty = t.empty;
        let mut subs: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
        let mut cur: Vec<(f32, f32)> = Vec::new();
        let mut at = (0.0, 0.0);
        let mut pts: Vec<(f32, f32)> = Vec::new();
        let flush = |cur: &mut Vec<(f32, f32)>, subs: &mut Vec<(Vec<(f32, f32)>, bool)>, closed: bool| {
            if cur.len() > 1 {
                subs.push((std::mem::take(cur), closed));
            } else {
                cur.clear();
            }
        };
        // Points in `subs` so far, counted as they come.
        let (mut in_subs, mut counted) = (0usize, 0usize);
        while i < tags.len() && !empty {
            let k = &tags[i];
            i += 1;
            in_subs += subs[counted..].iter().map(|s| s.0.len()).sum::<usize>();
            counted = subs.len();
            if made + in_subs + cur.len() > MAX_POINTS {
                break;
            }
            match (k.name, k.close) {
                ("path", true) => break,
                ("pt", false) => pts.push((k.num("x") * sx, k.num("y") * sy)),
                ("moveTo" | "lnTo" | "cubicBezTo" | "quadBezTo", false) => pts.clear(),
                ("moveTo", true) => {
                    flush(&mut cur, &mut subs, false);
                    if let Some(&p) = pts.last() {
                        at = p;
                        cur.push(p);
                    }
                }
                ("lnTo", true) => {
                    if let Some(&p) = pts.last() {
                        if cur.is_empty() {
                            cur.push(at);
                        }
                        cur.push(p);
                        at = p;
                    }
                }
                ("cubicBezTo", true) if pts.len() >= 3 => {
                    if cur.is_empty() {
                        cur.push(at);
                    }
                    cubic(at, pts[0], pts[1], pts[2], &mut cur);
                    at = pts[2];
                }
                ("quadBezTo", true) if pts.len() >= 2 => {
                    if cur.is_empty() {
                        cur.push(at);
                    }
                    // Raised to a cubic.
                    let c1 = (at.0 + 2.0 / 3.0 * (pts[0].0 - at.0), at.1 + 2.0 / 3.0 * (pts[0].1 - at.1));
                    let c2 = (pts[1].0 + 2.0 / 3.0 * (pts[0].0 - pts[1].0), pts[1].1 + 2.0 / 3.0 * (pts[0].1 - pts[1].1));
                    cubic(at, c1, c2, pts[1], &mut cur);
                    at = pts[1];
                }
                ("arcTo", false) => {
                    if cur.is_empty() {
                        cur.push(at);
                    }
                    let (rx, ry) = (k.num("wR") * sx, k.num("hR") * sy);
                    at = arc(at, rx, ry, k.num("stAng") / 60000.0, k.num("swAng") / 60000.0, &mut cur);
                }
                ("close", false) => {
                    let start = cur.first().copied();
                    flush(&mut cur, &mut subs, true);
                    if let Some(s) = start {
                        at = s;
                    }
                }
                _ => {}
            }
        }
        flush(&mut cur, &mut subs, false);
        made += subs.iter().map(|s| s.0.len()).sum::<usize>();
        if !subs.is_empty() {
            out.push(PresetPath { subpaths: subs, fill, stroke });
        }
    }
    out
}

/// Points a custom geometry's paths make at most.
pub const MAX_POINTS: usize = 20_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_curves_and_arcs() {
        let xml = r#"<a:custGeom><a:pathLst><a:path w="10" h="10"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="10" y="0"/></a:lnTo><a:lnTo><a:pt x="10" y="10"/></a:lnTo><a:close/></a:path><a:path w="10" h="10" fill="none"><a:moveTo><a:pt x="0" y="5"/></a:moveTo><a:cubicBezTo><a:pt x="3" y="0"/><a:pt x="6" y="0"/><a:pt x="10" y="5"/></a:cubicBezTo><a:arcTo wR="5" hR="5" stAng="0" swAng="5400000"/></a:path></a:pathLst></a:custGeom>"#;
        let p = custom_paths(xml, 100.0, 50.0);
        assert_eq!(p.len(), 2);
        let (tri, closed) = &p[0].subpaths[0];
        assert!(*closed);
        assert_eq!(tri, &vec![(0.0, 0.0), (100.0, 0.0), (100.0, 50.0)]);
        assert_eq!(p[1].fill, PresetFill::None);
        let (curve, closed) = &p[1].subpaths[0];
        assert!(!closed);
        assert_eq!(curve[0], (0.0, 25.0));
        assert!(curve.len() > 16);
        // The arc turns a quarter from the curve's end.
        let end = *curve.last().unwrap();
        assert!((end.0 - 50.0).abs() < 0.5 && (end.1 - 50.0).abs() < 0.5, "{end:?}");
    }
}
