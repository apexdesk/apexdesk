//! Ink in OpenDocument files, which have none: written as unfilled
//! `draw:path` lines LibreOffice shows (one per look of the strokes, in a
//! `draw:g` in text documents), and read back from them, from
//! `draw:polyline` and from freeform lines drawn in LibreOffice.

use std::fmt::Write as _;

use apex_doc::{DocShape, Ink, InkStroke, InkTool, Shape};

use crate::drawingml::Node;
use crate::parse_length_pt;

/// Units of the view box written: 1/100 mm, as LibreOffice uses.
const UNITS_PT: f32 = 2540.0 / 72.0;
/// Points taken along each curve of a path.
const CURVE_STEPS: usize = 8;

/// Whether an element may be ink.
pub(crate) fn is_ink_element(name: &str) -> bool {
    matches!(name, "draw:path" | "draw:polyline")
}

fn nums(s: &str) -> Vec<f32> {
    s.split(|c: char| c.is_whitespace() || c == ',').filter_map(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).collect()
}

/// The lines of an SVG path (`svg:d`), curves as points along them; None
/// when a part is closed (a filled shape, not a line).
fn path_lines(d: &str) -> Option<Vec<Vec<(f32, f32)>>> {
    // Tokens: command letters and numbers.
    let mut toks: Vec<Result<char, f32>> = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            toks.push(Ok(c));
            i += 1;
        } else if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' {
            let start = i;
            i += 1;
            let mut dot = c == '.';
            while i < b.len() {
                let ch = b[i];
                if ch.is_ascii_digit() || (ch == b'.' && !dot) {
                    dot |= ch == b'.';
                    i += 1;
                } else if (ch == b'e' || ch == b'E') && i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == b'-' || b[i + 1] == b'+') {
                    i += 2;
                } else {
                    break;
                }
            }
            toks.push(Err(d[start..i].parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(0.0)));
        } else {
            i += 1;
        }
        if toks.len() > 400_000 {
            break;
        }
    }
    let mut out: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut cmd = 'M';
    let mut k = 0;
    let take = |k: &mut usize, n: usize| -> Option<Vec<f32>> {
        let v: Vec<f32> = toks.get(*k..*k + n)?.iter().map(|t| t.err()).collect::<Option<Vec<f32>>>()?;
        *k += n;
        Some(v)
    };
    while k < toks.len() {
        if let Ok(c) = toks[k] {
            cmd = c;
            k += 1;
            if matches!(cmd, 'Z' | 'z') {
                return None;
            }
            continue;
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        let n = match cmd.to_ascii_uppercase() {
            'M' | 'L' | 'T' => 2,
            'H' | 'V' => 1,
            'C' => 6,
            'S' | 'Q' => 4,
            'A' => 7,
            _ => {
                k += 1;
                continue;
            }
        };
        let Some(v) = take(&mut k, n) else { break };
        let mut ctrl = None;
        match cmd.to_ascii_uppercase() {
            'M' => {
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                (x, y) = (ox + v[0], oy + v[1]);
                cur.push((x, y));
                // Pairs after a move are lines.
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                (x, y) = (ox + v[0], oy + v[1]);
                cur.push((x, y));
            }
            'H' => {
                x = if rel { x + v[0] } else { v[0] };
                cur.push((x, y));
            }
            'V' => {
                y = if rel { y + v[0] } else { v[0] };
                cur.push((x, y));
            }
            'C' | 'S' => {
                let (c1, c2, e) = if n == 6 {
                    ((ox + v[0], oy + v[1]), (ox + v[2], oy + v[3]), (ox + v[4], oy + v[5]))
                } else {
                    let c1 = last_ctrl.map_or((x, y), |c| (2.0 * x - c.0, 2.0 * y - c.1));
                    (c1, (ox + v[0], oy + v[1]), (ox + v[2], oy + v[3]))
                };
                let p0 = (x, y);
                for s in 1..=CURVE_STEPS {
                    let t = s as f32 / CURVE_STEPS as f32;
                    let u = 1.0 - t;
                    let f = |a: f32, b: f32, c: f32, d: f32| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
                    cur.push((f(p0.0, c1.0, c2.0, e.0), f(p0.1, c1.1, c2.1, e.1)));
                }
                ctrl = Some(c2);
                (x, y) = e;
            }
            'Q' | 'T' => {
                let (c, e) = if n == 4 { ((ox + v[0], oy + v[1]), (ox + v[2], oy + v[3])) } else { (last_ctrl.map_or((x, y), |c| (2.0 * x - c.0, 2.0 * y - c.1)), (ox + v[0], oy + v[1])) };
                let p0 = (x, y);
                for s in 1..=CURVE_STEPS {
                    let t = s as f32 / CURVE_STEPS as f32;
                    let u = 1.0 - t;
                    cur.push((u * u * p0.0 + 2.0 * u * t * c.0 + t * t * e.0, u * u * p0.1 + 2.0 * u * t * c.1 + t * t * e.1));
                }
                ctrl = Some(c);
                (x, y) = e;
            }
            // Arcs: to their end.
            _ => {
                (x, y) = (ox + v[5], oy + v[6]);
                cur.push((x, y));
            }
        }
        last_ctrl = ctrl;
        if cur.len() > apex_doc::ink::MAX_POINTS {
            break;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Some(out)
}

/// Ink from a `draw:path` or `draw:polyline` with its box (`w` by `h`
/// points) and its graphic style (`g`); None for filled shapes.
pub(crate) fn read(n: &Node, g: &dyn Fn(&str) -> Option<String>) -> Option<Ink> {
    let lines = match n.local() {
        "polyline" => {
            let v = nums(n.attr("draw:points").unwrap_or_default());
            vec![v.chunks_exact(2).map(|p| (p[0], p[1])).collect::<Vec<_>>()]
        }
        "path" => path_lines(n.attr("svg:d").unwrap_or_default())?,
        _ => return None,
    };
    if g("draw:stroke").as_deref() == Some("none") {
        return None;
    }
    let vb = nums(n.attr("svg:viewBox").unwrap_or_default());
    let (vx, vy, vw, vh) = match vb[..] {
        [x, y, w, h] if w > 0.0 && h > 0.0 => (x, y, w, h),
        _ => {
            // No view box: the points' own extent.
            let all = || lines.iter().flatten();
            let (x0, y0) = all().fold((f32::INFINITY, f32::INFINITY), |a, p| (a.0.min(p.0), a.1.min(p.1)));
            let (x1, y1) = all().fold((f32::NEG_INFINITY, f32::NEG_INFINITY), |a, p| (a.0.max(p.0), a.1.max(p.1)));
            if !x0.is_finite() {
                return None;
            }
            (x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0))
        }
    };
    let color = g("svg:stroke-color").and_then(|c| u32::from_str_radix(c.trim().strip_prefix('#')?, 16).ok()).unwrap_or(0) & 0xFF_FFFF;
    let width = g("svg:stroke-width").and_then(|w| parse_length_pt(&w)).unwrap_or(0.75).max(0.1);
    let alpha = g("svg:stroke-opacity").and_then(|o| {
        let o = o.trim();
        match o.strip_suffix('%') {
            Some(p) => p.trim().parse::<f32>().ok().map(|v| v / 100.0),
            None => o.parse::<f32>().ok(),
        }
    });
    let tool = match alpha.unwrap_or(1.0) {
        a if a < 0.6 => InkTool::Highlighter,
        a if a < 0.95 => InkTool::Pencil,
        _ => InkTool::Pen,
    };
    let strokes = lines
        .into_iter()
        .filter(|l| !l.is_empty())
        .map(|l| InkStroke { points: l.into_iter().map(|(x, y)| ((x - vx) / vw, (y - vy) / vh)).collect(), ..InkStroke::new(tool, color, width) })
        .collect();
    let mut ink = Ink::new(strokes);
    ink.sanitize();
    (!ink.strokes.is_empty()).then_some(ink)
}

/// The graphic properties of a look of strokes.
pub(crate) fn graphic(s: &InkStroke) -> String {
    let mut g = format!(r##" draw:stroke="solid" svg:stroke-color="#{:06x}" svg:stroke-width="{}pt" draw:fill="none" draw:stroke-linejoin="round""##, s.color & 0xFF_FFFF, (s.width * 1000.0).round() / 1000.0);
    g.push_str(if s.tool == InkTool::Highlighter { r#" svg:stroke-linecap="butt""# } else { r#" svg:stroke-linecap="round""# });
    let a = s.tool.alpha();
    if a < 1.0 {
        let _ = write!(g, r#" svg:stroke-opacity="{}%""#, (a * 100.0).round() as i64);
    }
    g
}

/// A line drawing strokes of ink in box `b` (x, y, width, height,
/// points) turned and flipped as `shape` says: the box around the line
/// (LibreOffice fits a line to its box whatever its view box says), its
/// `svg:viewBox` and its `svg:d`.
pub(crate) fn path(strokes: &[&InkStroke], b: [f32; 4], shape: &Shape) -> ([f32; 4], String, String) {
    let (cx, cy) = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
    let (sn, c) = shape.rotation.to_radians().sin_cos();
    let abs: Vec<Vec<(f32, f32)>> = strokes
        .iter()
        .map(|s| {
            s.points
                .iter()
                .map(|&(x, y)| {
                    let x = b[0] + (if shape.flip_h { 1.0 - x } else { x }) * b[2];
                    let y = b[1] + (if shape.flip_v { 1.0 - y } else { y }) * b[3];
                    (cx + (x - cx) * c - (y - cy) * sn, cy + (x - cx) * sn + (y - cy) * c)
                })
                .collect()
        })
        .collect();
    let all = || abs.iter().flatten();
    let (x0, y0) = all().fold((f32::INFINITY, f32::INFINITY), |a, p| (a.0.min(p.0), a.1.min(p.1)));
    let (x1, y1) = all().fold((f32::NEG_INFINITY, f32::NEG_INFINITY), |a, p| (a.0.max(p.0), a.1.max(p.1)));
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return ([b[0], b[1], 0.0, 0.0], "0 0 1 1".into(), String::new());
    }
    let (vw, vh) = (((x1 - x0) * UNITS_PT).round().max(1.0), ((y1 - y0) * UNITS_PT).round().max(1.0));
    let mut d = String::new();
    for line in &abs {
        for (i, &(x, y)) in line.iter().enumerate() {
            let (px, py) = (((x - x0) * UNITS_PT).round() as i64, ((y - y0) * UNITS_PT).round() as i64);
            let _ = write!(d, "{}{px} {py} ", if i == 0 { "M" } else if i == 1 { "L" } else { "" });
            if line.len() == 1 {
                let _ = write!(d, "L{px} {py} ");
            }
        }
    }
    ([x0, y0, x1 - x0, y1 - y0], format!("0 0 {} {}", vw as i64, vh as i64), d.trim_end().to_string())
}

/// One ink shape made of the ink shapes of a group (drawn apart for their
/// looks), in the box around them; None if a member isn't plain ink.
pub(crate) fn merge(members: &[DocShape]) -> Option<DocShape> {
    if members.len() < 2 || !members.iter().all(|m| m.shape.ink.is_some() && m.shape.rotation == 0.0 && !m.shape.flip_h && !m.shape.flip_v) {
        return None;
    }
    let x0 = members.iter().map(|m| m.x).fold(f32::INFINITY, f32::min);
    let y0 = members.iter().map(|m| m.y).fold(f32::INFINITY, f32::min);
    let x1 = members.iter().map(|m| m.x + m.width).fold(f32::NEG_INFINITY, f32::max);
    let y1 = members.iter().map(|m| m.y + m.height).fold(f32::NEG_INFINITY, f32::max);
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let mut strokes = Vec::new();
    for m in members {
        for s in m.shape.ink.as_ref()?.to_abs([m.x, m.y, m.width, m.height]) {
            strokes.push(InkStroke { points: s.points.iter().map(|&(x, y)| ((x - x0) / w, (y - y0) / h)).collect(), ..s });
        }
    }
    let mut ink = Ink::new(strokes);
    ink.sanitize();
    let first = &members[0];
    let mut shape = Shape::ink(ink);
    (shape.name, shape.alt) = (first.shape.name.clone(), first.shape.alt.clone());
    let mut d = DocShape { shape, x: x0, y: y0, width: w, height: h, group: None, h_align: None, v_align: None, ..first.clone() };
    d.z = members.iter().map(|m| m.z).max().unwrap_or(0);
    d.sanitize();
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_paths() {
        let l = path_lines("M 0 0 L 10 0 10 10 m 5 5 h 5 v 5").unwrap();
        assert_eq!(l, vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], vec![(15.0, 15.0), (20.0, 15.0), (20.0, 20.0)]]);
        let c = path_lines("M0 0C0 10 10 10 10 0").unwrap();
        assert_eq!(c[0].len(), 1 + CURVE_STEPS);
        assert_eq!(*c[0].last().unwrap(), (10.0, 0.0));
        assert!(c[0][CURVE_STEPS / 2].1 > 5.0);
        assert!(path_lines("M0 0 L 10 10 Z").is_none(), "closed: a shape");
        assert_eq!(path_lines("M1-2l3,4").unwrap(), vec![vec![(1.0, -2.0), (4.0, 2.0)]]);
        assert!(path_lines("M 1e999 x q").is_some());
    }
}
