//! Handwriting (ink): strokes drawn with a pen, a pencil or a highlighter.
//! A shape holds them (see [`crate::Shape::ink`]), so documents and sheets
//! anchor, move, resize, stack and undo ink like any other shape. Points
//! are kept in the shape's box (0..1 each way) and follow it when resized.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::{DocShape, Document, EditKind, OBJECT, Pos, Selection, Shape, ShapeKind};

/// Strokes one ink object keeps at most, and points per stroke (hostile
/// files, endless scribbling).
pub const MAX_STROKES: usize = 5_000;
pub const MAX_POINTS: usize = 20_000;

/// What a stroke was drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum InkTool {
    #[default]
    Pen,
    /// A textured, slightly see-through line.
    Pencil,
    /// A wide see-through line under which the text shows.
    Highlighter,
}

impl InkTool {
    pub const ALL: [InkTool; 3] = [InkTool::Pen, InkTool::Pencil, InkTool::Highlighter];

    /// Opacity the stroke is painted with.
    pub fn alpha(self) -> f32 {
        match self {
            InkTool::Pen => 1.0,
            InkTool::Pencil => 0.85,
            InkTool::Highlighter => 0.4,
        }
    }

    /// Default width in points.
    pub fn default_width(self) -> f32 {
        match self {
            InkTool::Pen => 1.5,
            InkTool::Pencil => 1.0,
            InkTool::Highlighter => 10.0,
        }
    }

    /// Default color.
    pub fn default_color(self) -> u32 {
        match self {
            InkTool::Pen => 0x000000,
            InkTool::Pencil => 0x404040,
            InkTool::Highlighter => 0xFFFF00,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            InkTool::Pen => "ink.pen",
            InkTool::Pencil => "ink.pencil",
            InkTool::Highlighter => "ink.highlighter",
        }
    }
}

/// One stroke.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct InkStroke {
    pub tool: InkTool,
    pub color: u32,
    /// Width in points (at full pressure).
    pub width: f32,
    /// Points: in the ink's box (0..1) when held by an [`Ink`], absolute
    /// when passed to [`Ink::from_abs`].
    pub points: Vec<(f32, f32)>,
    /// Pen pressure of each point, 0..1; empty when the device gave none.
    pub pressure: Vec<f32>,
}

impl InkStroke {
    pub fn new(tool: InkTool, color: u32, width: f32) -> InkStroke {
        InkStroke { tool, color, width, points: Vec::new(), pressure: Vec::new() }
    }

    /// Width at point `i` (pressure makes it thinner, down to a third).
    pub fn width_at(&self, i: usize) -> f32 {
        match self.pressure.get(i) {
            Some(p) if self.pressure.len() == self.points.len() => self.width * (0.35 + 0.65 * p.clamp(0.0, 1.0)),
            _ => self.width,
        }
    }

    pub fn has_pressure(&self) -> bool {
        !self.pressure.is_empty() && self.pressure.len() == self.points.len()
    }

    fn hash_into<H: Hasher>(&self, h: &mut H) {
        (self.tool, self.color, self.width.to_bits()).hash(h);
        for (x, y) in &self.points {
            (x.to_bits(), y.to_bits()).hash(h);
        }
        for p in &self.pressure {
            p.to_bits().hash(h);
        }
    }
}

/// Ink as read from a file (InkML), written back as it was while the
/// strokes are the ones read from it: what ApexDesk doesn't understand of
/// it (timestamps, other channels, annotations…) is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct InkSource {
    pub xml: String,
    /// [`Ink::signature`] of the strokes read from it.
    pub signature: u64,
}

/// The strokes of one ink object.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Ink {
    pub strokes: Vec<InkStroke>,
    pub source: Option<Arc<InkSource>>,
}

impl Ink {
    pub fn new(strokes: Vec<InkStroke>) -> Ink {
        Ink { strokes, source: None }
    }

    /// A hash of the strokes.
    pub fn signature(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.strokes.len().hash(&mut h);
        for s in &self.strokes {
            s.hash_into(&mut h);
        }
        h.finish()
    }

    /// The InkML read from a file, while the strokes are still the ones
    /// read.
    pub fn kept_source(&self) -> Option<&str> {
        self.source.as_ref().filter(|s| s.signature == self.signature()).map(|s| s.xml.as_str())
    }

    /// Ink made of strokes with absolute points (any unit; `unit` is how
    /// many of them make a point, to leave room for the stroke width) and
    /// the box around them: x, y, width, height. None without points.
    pub fn from_abs(strokes: Vec<InkStroke>, unit: f32) -> Option<(Ink, [f32; 4])> {
        let pts = || strokes.iter().flat_map(|s| s.points.iter());
        let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in pts() {
            if x.is_finite() && y.is_finite() {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        if !x0.is_finite() || !y0.is_finite() {
            return None;
        }
        let unit = if unit.is_finite() && unit > 0.0 { unit } else { 1.0 };
        let pad = strokes.iter().map(|s| s.width).filter(|w| w.is_finite()).fold(0.0f32, f32::max).min(200.0) / 2.0 * unit + unit;
        let (x, y) = (x0 - pad, y0 - pad);
        let (w, h) = (x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad);
        let strokes = strokes
            .into_iter()
            .map(|s| InkStroke { points: s.points.iter().map(|&(px, py)| ((px - x) / w, (py - y) / h)).collect(), ..s })
            .filter(|s| !s.points.is_empty())
            .collect();
        let mut ink = Ink::new(strokes);
        ink.sanitize();
        Some((ink, [x, y, w, h]))
    }

    /// The strokes with absolute points in box `b` (x, y, width, height).
    pub fn to_abs(&self, b: [f32; 4]) -> Vec<InkStroke> {
        self.strokes.iter().map(|s| InkStroke { points: s.points.iter().map(|&(px, py)| (b[0] + px * b[2], b[1] + py * b[3])).collect(), ..s.clone() }).collect()
    }

    /// This ink (in box `b`) with one more stroke (absolute points), and
    /// the box around them all.
    pub fn with_stroke(&self, b: [f32; 4], s: InkStroke, unit: f32) -> Option<(Ink, [f32; 4])> {
        let mut all = self.to_abs(b);
        all.push(s);
        Ink::from_abs(all, unit)
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        self.strokes.truncate(MAX_STROKES);
        let unit = |v: f32| if v.is_finite() { v.clamp(-1.0, 2.0) } else { 0.0 };
        for s in &mut self.strokes {
            s.points.truncate(MAX_POINTS);
            for p in &mut s.points {
                *p = (unit(p.0), unit(p.1));
            }
            s.width = if s.width.is_finite() { s.width.clamp(0.1, 200.0) } else { 1.0 };
            s.color &= 0xFF_FFFF;
            if s.pressure.len() != s.points.len() {
                s.pressure.clear();
            }
            for p in &mut s.pressure {
                *p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 1.0 };
            }
        }
        self.strokes.retain(|s| !s.points.is_empty());
    }

    pub(crate) fn hash_into<H: Hasher>(&self, h: &mut H) {
        self.signature().hash(h);
        self.source.as_ref().map(|s| s.signature).hash(h);
    }
}

impl Shape {
    /// A shape holding ink: no fill, no outline, no text.
    pub fn ink(ink: Ink) -> Shape {
        Shape { fill: None, line: None, name: "Ink".into(), ink: Some(Arc::new(ink)), ..Shape::new(ShapeKind::Rect) }
    }

    pub fn is_ink(&self) -> bool {
        self.ink.is_some()
    }
}

impl Document {
    /// Changes the shapes at the given places (None: removes it) and adds
    /// shapes at the start of paragraphs (`add`), as one undo step: the
    /// eraser changes several ink objects at once, the lasso moves strokes
    /// into an object of their own. Returns where the caret goes: the
    /// first shape added selected, if any.
    pub fn change_ink(&mut self, changes: Vec<(Pos, Option<DocShape>)>, add: Vec<(usize, DocShape)>, sel: Selection) -> Selection {
        let changes: Vec<(Pos, Option<DocShape>)> = changes.into_iter().filter(|(p, _)| self.shape_at(*p).is_some()).collect();
        let add: Vec<(usize, DocShape)> = add.into_iter().filter(|(p, _)| *p < self.paragraphs.len()).collect();
        if changes.is_empty() && add.is_empty() {
            return sel;
        }
        let paras = || changes.iter().map(|(p, _)| p.para).chain(add.iter().map(|(p, _)| *p));
        let lo = paras().min().unwrap_or(0);
        let hi = paras().max().unwrap_or(0);
        let before = self.paragraphs[lo..=hi].to_vec();
        let mut gone: Vec<Pos> = Vec::new();
        for (p, s) in changes {
            match s {
                Some(s) => {
                    let s = Arc::new(s);
                    self.paragraphs[p.para].map_attrs(p.offset, p.offset + OBJECT.len_utf8(), &|a| a.shape = Some(s.clone()));
                }
                None => gone.push(p),
            }
        }
        // Last first, so the earlier offsets hold.
        gone.sort();
        gone.dedup();
        for p in gone.iter().rev() {
            self.paragraphs[p.para].remove(p.offset, p.offset + OBJECT.len_utf8());
        }
        let mut after = match gone.first() {
            Some(p) => Selection::caret(*p),
            None => sel,
        };
        for (i, (para, shape)) in add.into_iter().enumerate().rev() {
            let mut attrs = self.typing_attrs(Pos::new(para, 0));
            attrs.clear_object();
            (attrs.link, attrs.link_tip) = (None, None);
            attrs.shape = Some(Arc::new(shape));
            self.paragraphs[para].insert_str(0, &OBJECT.to_string(), attrs);
            if i == 0 {
                after = Selection { anchor: Pos::new(para, 0), cursor: Pos::new(para, OBJECT.len_utf8()) };
            }
        }
        self.record(lo, before, hi - lo + 1, sel, after, EditKind::Shape);
        after
    }
}

// --- smoothing -------------------------------------------------------------------------

/// A stroke as drawn (absolute points, `tol`: how far a point may move,
/// in the points' unit) made smooth and light: repeated points dropped,
/// the jitter of the hand averaged out and the points a straight line
/// would draw anyway left out.
pub fn smooth(points: &[(f32, f32)], pressure: &[f32], tol: f32) -> (Vec<(f32, f32)>, Vec<f32>) {
    let with_p = pressure.len() == points.len();
    let mut pts: Vec<(f32, f32, f32)> = Vec::with_capacity(points.len());
    for (i, &(x, y)) in points.iter().enumerate() {
        if !(x.is_finite() && y.is_finite()) {
            continue;
        }
        let p = if with_p { pressure[i] } else { 1.0 };
        if let Some(l) = pts.last() {
            if (l.0 - x).hypot(l.1 - y) < tol * 0.5 {
                continue;
            }
        }
        pts.push((x, y, p));
    }
    if pts.len() > 2 {
        // Two passes of a 1-2-1 average; the ends stay put.
        for _ in 0..2 {
            let prev = pts.clone();
            for i in 1..pts.len() - 1 {
                let (a, b, c) = (prev[i - 1], prev[i], prev[i + 1]);
                pts[i] = ((a.0 + 2.0 * b.0 + c.0) / 4.0, (a.1 + 2.0 * b.1 + c.1) / 4.0, (a.2 + 2.0 * b.2 + c.2) / 4.0);
            }
        }
    }
    let mut keep = vec![false; pts.len()];
    if !pts.is_empty() {
        keep[0] = true;
        let n = pts.len() - 1;
        keep[n] = true;
        simplify(&pts, 0, n, tol * 0.25, &mut keep);
    }
    let kept: Vec<(f32, f32, f32)> = pts.into_iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| p).take(MAX_POINTS).collect();
    let out = kept.iter().map(|p| (p.0, p.1)).collect();
    let pressure = if with_p { kept.iter().map(|p| p.2.clamp(0.0, 1.0)).collect() } else { Vec::new() };
    (out, pressure)
}

/// Ramer-Douglas-Peucker between `a` and `b` (kept); pressure changes
/// count as distance too.
fn simplify(pts: &[(f32, f32, f32)], a: usize, b: usize, tol: f32, keep: &mut [bool]) {
    let mut stack = vec![(a, b)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (p, q) = (pts[a], pts[b]);
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let len = dx.hypot(dy);
        let mut far = (a, 0.0f32);
        for (i, r) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = if len > 0.0 { ((r.0 - p.0) * dy - (r.1 - p.1) * dx).abs() / len } else { (r.0 - p.0).hypot(r.1 - p.1) };
            let t = (i - a) as f32 / (b - a) as f32;
            let dp = (r.2 - (p.2 + (q.2 - p.2) * t)).abs() * 10.0 * tol;
            let d = d.max(dp);
            if d > far.1 {
                far = (i, d);
            }
        }
        if far.1 > tol {
            keep[far.0] = true;
            stack.push((a, far.0));
            stack.push((far.0, b));
        }
    }
}

/// Distance from `q` to the segment `a`-`b`.
pub fn seg_dist(q: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let l2 = abx * abx + aby * aby;
    let t = if l2 > 0.0 { (((q.0 - a.0) * abx + (q.1 - a.1) * aby) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (a.0 + abx * t - q.0).hypot(a.1 + aby * t - q.1)
}

/// Whether `q` is inside the polygon `poly` (even-odd).
pub fn inside(poly: &[(f32, f32)], q: (f32, f32)) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.1 > q.1) != (b.1 > q.1) && q.0 < (b.0 - a.0) * (q.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(pts: &[(f32, f32)]) -> InkStroke {
        InkStroke { points: pts.to_vec(), ..InkStroke::new(InkTool::Pen, 0xFF0000, 2.0) }
    }

    #[test]
    fn boxes_and_back() {
        let (ink, b) = Ink::from_abs(vec![stroke(&[(10.0, 20.0), (30.0, 60.0)]), stroke(&[(20.0, 40.0)])], 1.0).unwrap();
        // Room for half the width and a point.
        assert_eq!(b, [8.0, 18.0, 24.0, 44.0]);
        let abs = ink.to_abs(b);
        assert!((abs[0].points[1].0 - 30.0).abs() < 1e-4 && (abs[0].points[1].1 - 60.0).abs() < 1e-4);
        // Resized: the strokes follow.
        let abs = ink.to_abs([0.0, 0.0, 48.0, 88.0]);
        assert!((abs[1].points[0].0 - 24.0).abs() < 1e-4);
        let (more, b2) = ink.with_stroke(b, stroke(&[(100.0, 20.0)]), 1.0).unwrap();
        assert_eq!(more.strokes.len(), 3);
        assert_eq!(b2[2], 94.0);
        assert!(Ink::from_abs(vec![stroke(&[])], 1.0).is_none());
    }

    #[test]
    fn kept_source_only_while_unchanged() {
        let mut ink = Ink::new(vec![stroke(&[(0.1, 0.1), (0.9, 0.9)])]);
        ink.source = Some(Arc::new(InkSource { xml: "<ink/>".into(), signature: ink.signature() }));
        assert_eq!(ink.kept_source(), Some("<ink/>"));
        ink.strokes[0].color = 0x00FF00;
        assert_eq!(ink.kept_source(), None);
    }

    #[test]
    fn smoothing() {
        // A jittery straight line becomes a few points.
        let pts: Vec<(f32, f32)> = (0..100).map(|i| (i as f32, if i % 2 == 0 { 0.1 } else { -0.1 })).collect();
        let (s, p) = smooth(&pts, &[], 1.0);
        assert!(s.len() < 10, "{}", s.len());
        assert!(p.is_empty());
        assert_eq!(s[0], (0.0, 0.1));
        assert_eq!(*s.last().unwrap(), (99.0, -0.1));
        // A corner survives; pressure stays with its points.
        let pts = [(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (10.0, 5.0), (10.0, 10.0)];
        let (s, p) = smooth(&pts, &[0.2, 0.4, 0.6, 0.8, 1.0], 0.5);
        assert!(s.len() >= 3 && p.len() == s.len());
        assert!(s.iter().any(|q| q.0 > 8.0 && q.1 < 2.0));
        let (s, _) = smooth(&[(1.0, 1.0), (f32::NAN, 0.0)], &[], 1.0);
        assert_eq!(s, vec![(1.0, 1.0)]);
    }

    #[test]
    fn erasing_several_objects_is_one_step() {
        use crate::Paragraph;
        let mut d = Document::from_paragraphs(vec![Paragraph::new("One"), Paragraph::new("Two")]);
        let ink = |n: usize| DocShape::new(Shape::ink(Ink::new((0..n).map(|_| stroke(&[(0.2, 0.2), (0.8, 0.8)])).collect())), 50.0, 50.0);
        d.insert_shape(Selection::caret(Pos::new(0, 0)), ink(2));
        d.insert_shape(Selection::caret(Pos::new(0, 0)), ink(1));
        d.insert_shape(Selection::caret(Pos::new(1, 1)), ink(3));
        let all = d.shapes();
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|(_, s)| s.shape.is_ink() && !s.shape.can_have_text()));
        let mut fewer = (*all[1].1).clone();
        Arc::make_mut(fewer.shape.ink.as_mut().unwrap()).strokes.pop();
        let sel = d.change_ink(vec![(all[0].0, None), (all[1].0, Some(fewer)), (all[2].0, None)], Vec::new(), Selection::caret(Pos::new(0, 0)));
        assert_eq!(sel, Selection::caret(Pos::new(0, 0)));
        assert_eq!(d.shapes().len(), 1);
        assert_eq!(d.paragraphs[1].text, "Two");
        assert_eq!(d.shapes()[0].1.shape.ink.as_ref().unwrap().strokes.len(), 1);
        d.undo();
        assert_eq!(d.shapes().len(), 3);
        assert_eq!(d.shapes()[1].1.shape.ink.as_ref().unwrap().strokes.len(), 2);
        // Strokes taken out of one object into a new one: one step too.
        let all = d.shapes();
        let mut fewer = (*all[2].1).clone();
        Arc::make_mut(fewer.shape.ink.as_mut().unwrap()).strokes.truncate(1);
        let sel = d.change_ink(vec![(all[2].0, Some(fewer))], vec![(1, ink(2))], Selection::caret(Pos::new(0, 0)));
        assert_eq!(sel, Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, OBJECT.len_utf8()) });
        assert_eq!(d.shapes().len(), 4);
        assert_eq!(d.shape_at(Pos::new(1, 0)).unwrap().1.shape.ink.as_ref().unwrap().strokes.len(), 2);
        d.undo();
        assert_eq!(d.shapes().len(), 3);
    }

    #[test]
    fn hostile_values() {
        let mut ink = Ink::new(vec![InkStroke { width: f32::NAN, color: 0xFF00_0000, points: vec![(f32::INFINITY, 0.5)], pressure: vec![0.5, 0.5], ..Default::default() }]);
        ink.sanitize();
        let s = &ink.strokes[0];
        assert!(s.width == 1.0 && s.color == 0 && s.points[0] == (0.0, 0.5) && s.pressure.is_empty());
        assert!(inside(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], (5.0, 5.0)));
        assert!(!inside(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], (1.0, 9.0)));
        assert_eq!(seg_dist((5.0, 3.0), (0.0, 0.0), (10.0, 0.0)), 3.0);
    }
}
