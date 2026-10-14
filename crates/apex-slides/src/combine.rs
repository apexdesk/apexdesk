//! Merge Shapes (PowerPoint's Shape Format > Merge Shapes): union,
//! combine, fragment, intersect and subtract of the shapes' outlines. The
//! outlines are flattened (curves as points) and the result is a drawn
//! shape (`a:custGeom`) of straight segments.

use std::fmt::Write;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;

use crate::object::{Frame, Geometry};

/// How the shapes are merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// All of them as one.
    Union,
    /// All of them as one, without where they overlap.
    Combine,
    /// Every piece the outlines cut, each a shape.
    Fragment,
    /// Only where all of them overlap.
    Intersect,
    /// The first one without the others.
    Subtract,
}

/// Closed contours (holes among them) in slide points.
pub type Outline = Vec<Vec<[f64; 2]>>;

/// Area under which a piece is a sliver of rounding, not a shape (square
/// points).
const SLIVER: f64 = 0.01;

/// The outline of a shape with geometry `g` in frame `f`: what its fill
/// covers, turned and flipped as it shows. Lines have none.
pub fn outline(g: &Geometry, f: &Frame) -> Outline {
    let (w, h) = (f.w.max(0.0), f.h.max(0.0));
    let paths = match g {
        Geometry::Preset(..) if g.is_line() => return Vec::new(),
        Geometry::Preset(name, adj) => apex_doc::preset_paths(name, w, h, adj).unwrap_or_default(),
        Geometry::Custom(xml) => crate::geom::custom_paths(xml, w, h),
    };
    let paths: Vec<_> = if paths.is_empty() && matches!(g, Geometry::Preset(..)) {
        vec![vec![(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]]
    } else {
        paths.into_iter().filter(|p| p.fill != apex_doc::PresetFill::None).flat_map(|p| p.subpaths.into_iter().map(|s| s.0)).collect()
    };
    let (cx, cy) = (f64::from(f.x + w / 2.0), f64::from(f.y + h / 2.0));
    let (sin, cos) = f64::from(f.rot).to_radians().sin_cos();
    let place = |(x, y): (f32, f32)| {
        let x = if f.flip_h { w - x } else { x };
        let y = if f.flip_v { h - y } else { y };
        let (dx, dy) = (f64::from(x - w / 2.0), f64::from(y - h / 2.0));
        [cx + dx * cos - dy * sin, cy + dx * sin + dy * cos]
    };
    let contours: Outline = paths.into_iter().filter(|s| s.len() > 2).map(|s| s.into_iter().map(place).collect()).collect();
    // Overlapping subpaths and their direction made into plain contours.
    flat(contours.simplify_shape(FillRule::EvenOdd))
}

/// Contours drawn with the non-zero rule (a font's glyphs) as an outline.
pub fn filled(contours: Outline) -> Outline {
    flat(contours.simplify_shape(FillRule::NonZero))
}

/// The outlines `how` makes of `shapes` (in the order they were selected):
/// one, or for Fragment every piece. Empty when nothing is left.
pub fn merge(shapes: &[Outline], how: Merge) -> Vec<Outline> {
    let Some((first, rest)) = shapes.split_first() else { return Vec::new() };
    let fold = |rule| rest.iter().fold(first.clone(), |acc, s| flat(acc.overlay(s, rule, FillRule::EvenOdd)));
    let one = match how {
        Merge::Union => fold(OverlayRule::Union),
        Merge::Combine => fold(OverlayRule::Xor),
        Merge::Intersect => fold(OverlayRule::Intersect),
        Merge::Subtract => fold(OverlayRule::Difference),
        Merge::Fragment => {
            let mut pieces = vec![first.clone()];
            let mut seen = first.clone();
            for s in rest {
                let mut next = Vec::new();
                for p in &pieces {
                    next.extend(p.overlay(s, OverlayRule::Intersect, FillRule::EvenOdd).into_iter().map(|x| x.into_iter().collect()));
                    next.extend(p.overlay(s, OverlayRule::Difference, FillRule::EvenOdd).into_iter().map(|x| x.into_iter().collect()));
                }
                next.extend(s.overlay(&seen, OverlayRule::Difference, FillRule::EvenOdd).into_iter().map(|x| x.into_iter().collect()));
                seen = flat(seen.overlay(s, OverlayRule::Union, FillRule::EvenOdd));
                pieces = next;
            }
            pieces.retain(|p| area(p) > SLIVER);
            return pieces;
        }
    };
    if area(&one) > SLIVER { vec![one] } else { Vec::new() }
}

/// The shapes of an overlay as one outline.
fn flat(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Outline {
    shapes.into_iter().flatten().collect()
}

/// The area an outline covers (holes run the other way and take away).
fn area(o: &Outline) -> f64 {
    o.iter()
        .map(|c| {
            let n = c.len();
            (0..n).map(|i| c[i][0] * c[(i + 1) % n][1] - c[(i + 1) % n][0] * c[i][1]).sum::<f64>() / 2.0
        })
        .sum::<f64>()
        .abs()
}

/// An outline as a drawn shape: its frame (unturned) and geometry. None
/// when it is empty.
pub fn to_geometry(o: &Outline) -> Option<(Frame, Geometry)> {
    let pts = o.iter().flatten();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
    }
    if x0 > x1 {
        return None;
    }
    let emu = |v: f64| (v * 12700.0).round() as i64;
    let mut body = String::new();
    for c in o.iter().filter(|c| c.len() > 2) {
        for (i, p) in c.iter().enumerate() {
            let tag = if i == 0 { "moveTo" } else { "lnTo" };
            let _ = write!(body, r#"<a:{tag}><a:pt x="{}" y="{}"/></a:{tag}>"#, emu(p[0] - x0), emu(p[1] - y0));
        }
        body.push_str("<a:close/>");
    }
    let (w, h) = ((x1 - x0).max(0.01), (y1 - y0).max(0.01));
    let xml = format!(r#"<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst><a:path w="{}" h="{}">{body}</a:path></a:pathLst></a:custGeom>"#, emu(w), emu(h));
    Some((Frame::new(x0 as f32, y0 as f32, w as f32, h as f32), Geometry::Custom(xml.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f32, y: f32, s: f32) -> Outline {
        outline(&Geometry::preset("rect"), &Frame::new(x, y, s, s))
    }

    #[test]
    fn the_five_merges() {
        let (a, b) = (square(0.0, 0.0, 10.0), square(5.0, 5.0, 10.0));
        let ab = [a.clone(), b.clone()];
        let areas = |v: Vec<Outline>| v.iter().map(|o| area(o).round() as i64).collect::<Vec<_>>();
        assert_eq!(areas(merge(&ab, Merge::Union)), [175]);
        assert_eq!(areas(merge(&ab, Merge::Combine)), [150]);
        assert_eq!(areas(merge(&ab, Merge::Intersect)), [25]);
        assert_eq!(areas(merge(&ab, Merge::Subtract)), [75]);
        let mut parts = areas(merge(&ab, Merge::Fragment));
        parts.sort();
        assert_eq!(parts, [25, 75, 75]);
        // Apart: nothing in common.
        assert!(merge(&[a, square(50.0, 50.0, 1.0)], Merge::Intersect).is_empty());
    }

    #[test]
    fn turned_shapes_and_holes() {
        // A square turned 45° about its centre reaches past its box.
        let o = outline(&Geometry::preset("rect"), &Frame { rot: 45.0, ..Frame::new(0.0, 0.0, 10.0, 10.0) });
        let xs = o.iter().flatten().map(|p| p[0]);
        assert!(xs.fold(f64::MAX, f64::min) < -2.0);
        // A frame with a hole: subtracting the hole changes nothing.
        let hole = square(4.0, 4.0, 2.0);
        let ring = merge(&[square(0.0, 0.0, 10.0), hole.clone()], Merge::Subtract);
        assert_eq!(area(&ring[0]).round(), 96.0);
        assert_eq!(area(&merge(&[ring[0].clone(), hole], Merge::Subtract)[0]).round(), 96.0);
    }

    #[test]
    fn written_back_as_a_drawn_shape() {
        let o = merge(&[square(10.0, 20.0, 10.0), square(15.0, 25.0, 10.0)], Merge::Union).remove(0);
        let (f, g) = to_geometry(&o).unwrap();
        assert_eq!((f.x, f.y, f.w, f.h), (10.0, 20.0, 15.0, 15.0));
        let back = outline(&g, &f);
        assert_eq!(area(&back).round(), 175.0);
    }
}
