//! Shapes and text boxes: their geometry kind, fill, outline and text,
//! shared by documents and sheets, and the document's floating shapes
//! (anchored to a paragraph by an `OBJECT` character, like pictures).

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::{Align, Attrs, Document, EditKind, Image, OBJECT, Pos, Selection};

/// The geometry of a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ShapeKind {
    #[default]
    Rect,
    RoundRect,
    Ellipse,
    Triangle,
    RightTriangle,
    Diamond,
    Pentagon,
    Hexagon,
    RightArrow,
    LeftArrow,
    UpArrow,
    DownArrow,
    /// A straight line from the top-left to the bottom-right corner of its
    /// box (flips choose the other diagonal).
    Line,
    Star5,
    /// A rectangle with a pointer (a speech callout).
    Callout,
    /// A rectangle for text.
    TextBox,
}

impl ShapeKind {
    /// Every kind, in gallery order.
    pub const ALL: [ShapeKind; 16] = [
        ShapeKind::TextBox,
        ShapeKind::Line,
        ShapeKind::Rect,
        ShapeKind::RoundRect,
        ShapeKind::Ellipse,
        ShapeKind::Triangle,
        ShapeKind::RightTriangle,
        ShapeKind::Diamond,
        ShapeKind::Pentagon,
        ShapeKind::Hexagon,
        ShapeKind::RightArrow,
        ShapeKind::LeftArrow,
        ShapeKind::UpArrow,
        ShapeKind::DownArrow,
        ShapeKind::Star5,
        ShapeKind::Callout,
    ];

    pub fn is_line(self) -> bool {
        self == ShapeKind::Line
    }

    /// DrawingML preset geometry name.
    pub fn preset(self) -> &'static str {
        match self {
            ShapeKind::Rect | ShapeKind::TextBox => "rect",
            ShapeKind::RoundRect => "roundRect",
            ShapeKind::Ellipse => "ellipse",
            ShapeKind::Triangle => "triangle",
            ShapeKind::RightTriangle => "rtTriangle",
            ShapeKind::Diamond => "diamond",
            ShapeKind::Pentagon => "pentagon",
            ShapeKind::Hexagon => "hexagon",
            ShapeKind::RightArrow => "rightArrow",
            ShapeKind::LeftArrow => "leftArrow",
            ShapeKind::UpArrow => "upArrow",
            ShapeKind::DownArrow => "downArrow",
            ShapeKind::Line => "line",
            ShapeKind::Star5 => "star5",
            ShapeKind::Callout => "wedgeRectCallout",
        }
    }

    /// The kind of a DrawingML preset (unknown ones draw as the closest
    /// kind we have, else as a rectangle).
    pub fn from_preset(name: &str) -> (ShapeKind, bool) {
        let k = match name {
            "rect" => ShapeKind::Rect,
            "roundRect" => ShapeKind::RoundRect,
            "ellipse" => ShapeKind::Ellipse,
            "triangle" => ShapeKind::Triangle,
            "rtTriangle" => ShapeKind::RightTriangle,
            "diamond" => ShapeKind::Diamond,
            "pentagon" => ShapeKind::Pentagon,
            "hexagon" => ShapeKind::Hexagon,
            "rightArrow" => ShapeKind::RightArrow,
            "leftArrow" => ShapeKind::LeftArrow,
            "upArrow" => ShapeKind::UpArrow,
            "downArrow" => ShapeKind::DownArrow,
            "line" | "straightConnector1" => ShapeKind::Line,
            "star5" => ShapeKind::Star5,
            "wedgeRectCallout" => ShapeKind::Callout,
            _ => {
                let near = match name {
                    "flowChartAlternateProcess" | "snip1Rect" | "round1Rect" | "round2SameRect" => ShapeKind::RoundRect,
                    "flowChartConnector" | "donut" | "pie" | "chord" | "arc" => ShapeKind::Ellipse,
                    "flowChartDecision" => ShapeKind::Diamond,
                    "flowChartExtract" | "flowChartMerge" => ShapeKind::Triangle,
                    "octagon" | "heptagon" | "decagon" | "dodecagon" | "flowChartPreparation" => ShapeKind::Hexagon,
                    "bentConnector2" | "bentConnector3" | "curvedConnector3" => ShapeKind::Line,
                    "star4" | "star6" | "star7" | "star8" | "star10" | "star12" | "star16" | "star24" | "star32" => ShapeKind::Star5,
                    "wedgeRoundRectCallout" | "wedgeEllipseCallout" | "cloudCallout" => ShapeKind::Callout,
                    "notchedRightArrow" | "stripedRightArrow" | "homePlate" | "chevron" => ShapeKind::RightArrow,
                    _ => ShapeKind::Rect,
                };
                return (near, false);
            }
        };
        (k, true)
    }

    /// Key of the kind's name in the UI translations.
    pub fn key(self) -> &'static str {
        match self {
            ShapeKind::Rect => "shape.k.rect",
            ShapeKind::RoundRect => "shape.k.round_rect",
            ShapeKind::Ellipse => "shape.k.ellipse",
            ShapeKind::Triangle => "shape.k.triangle",
            ShapeKind::RightTriangle => "shape.k.right_triangle",
            ShapeKind::Diamond => "shape.k.diamond",
            ShapeKind::Pentagon => "shape.k.pentagon",
            ShapeKind::Hexagon => "shape.k.hexagon",
            ShapeKind::RightArrow => "shape.k.right_arrow",
            ShapeKind::LeftArrow => "shape.k.left_arrow",
            ShapeKind::UpArrow => "shape.k.up_arrow",
            ShapeKind::DownArrow => "shape.k.down_arrow",
            ShapeKind::Line => "shape.k.line",
            ShapeKind::Star5 => "shape.k.star",
            ShapeKind::Callout => "shape.k.callout",
            ShapeKind::TextBox => "shape.k.text_box",
        }
    }
}

/// Dash pattern of an outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Dash {
    #[default]
    Solid,
    Dot,
    Dash,
    DashDot,
    LongDash,
}

impl Dash {
    pub const ALL: [Dash; 5] = [Dash::Solid, Dash::Dot, Dash::Dash, Dash::DashDot, Dash::LongDash];

    /// DrawingML `prstDash` value.
    pub fn preset(self) -> &'static str {
        match self {
            Dash::Solid => "solid",
            Dash::Dot => "sysDot",
            Dash::Dash => "dash",
            Dash::DashDot => "dashDot",
            Dash::LongDash => "lgDash",
        }
    }

    pub fn from_preset(v: &str) -> Dash {
        match v {
            "dot" | "sysDot" => Dash::Dot,
            "dash" | "sysDash" => Dash::Dash,
            "dashDot" | "sysDashDot" | "lgDashDot" | "lgDashDotDot" | "sysDashDotDot" => Dash::DashDot,
            "lgDash" => Dash::LongDash,
            _ => Dash::Solid,
        }
    }

    /// Dash and gap lengths in line widths (empty: solid).
    pub fn pattern(self) -> &'static [f32] {
        match self {
            Dash::Solid => &[],
            Dash::Dot => &[1.0, 1.0],
            Dash::Dash => &[4.0, 3.0],
            Dash::DashDot => &[4.0, 3.0, 1.0, 3.0],
            Dash::LongDash => &[8.0, 3.0],
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Dash::Solid => "shape.dash_solid",
            Dash::Dot => "shape.dash_dot",
            Dash::Dash => "shape.dash_dash",
            Dash::DashDot => "shape.dash_dash_dot",
            Dash::LongDash => "shape.dash_long",
        }
    }
}

/// Where the text of a shape sits vertically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VAnchor {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// A run of text in a shape.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ShapeRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Points; `None` is the default size.
    pub size: Option<f32>,
    /// `None`: automatic (dark on light fills, white on dark ones).
    pub color: Option<u32>,
    pub font: Option<Arc<str>>,
    /// Artistic effects: fill, outline, shadow, glow, reflection, bevel.
    pub fx: Option<Arc<crate::TextFx>>,
}

impl ShapeRun {
    pub fn new(text: impl Into<String>) -> ShapeRun {
        ShapeRun { text: text.into(), ..Default::default() }
    }

    /// The same formatting with other text.
    pub fn with_text(&self, text: impl Into<String>) -> ShapeRun {
        ShapeRun { text: text.into(), ..self.clone() }
    }
}

/// A paragraph of text in a shape.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ShapePara {
    pub align: Align,
    pub runs: Vec<ShapeRun>,
}

impl ShapePara {
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Formatting of the first run (for text typed into it).
    pub fn first_run(&self) -> ShapeRun {
        self.runs.first().map(|r| r.with_text("")).unwrap_or_default()
    }
}

// --- geometry ---------------------------------------------------------------------

fn ellipse_pts(cx: f32, cy: f32, rx: f32, ry: f32, from: f32, to: f32, n: usize) -> impl Iterator<Item = (f32, f32)> {
    (0..=n).map(move |i| {
        let a = (from + (to - from) * i as f32 / n as f32).to_radians();
        (cx + rx * a.cos(), cy + ry * a.sin())
    })
}

/// The outline of a shape of kind `kind` in a `w` by `h` box with its
/// top-left corner at the origin (lines: their two ends).
pub fn shape_outline(kind: ShapeKind, w: f32, h: f32) -> Vec<(f32, f32)> {
    let ss = w.min(h);
    let p = |x: f32, y: f32| (x, y);
    match kind {
        ShapeKind::Rect | ShapeKind::TextBox => vec![p(0.0, 0.0), p(w, 0.0), p(w, h), p(0.0, h)],
        ShapeKind::RoundRect => {
            let r = ss * 0.16667;
            let mut v = Vec::with_capacity(28);
            v.extend(ellipse_pts(w - r, r, r, r, -90.0, 0.0, 6));
            v.extend(ellipse_pts(w - r, h - r, r, r, 0.0, 90.0, 6));
            v.extend(ellipse_pts(r, h - r, r, r, 90.0, 180.0, 6));
            v.extend(ellipse_pts(r, r, r, r, 180.0, 270.0, 6));
            v
        }
        ShapeKind::Ellipse => ellipse_pts(w / 2.0, h / 2.0, w / 2.0, h / 2.0, 0.0, 360.0, 64).take(64).collect(),
        ShapeKind::Triangle => vec![p(w / 2.0, 0.0), p(w, h), p(0.0, h)],
        ShapeKind::RightTriangle => vec![p(0.0, 0.0), p(w, h), p(0.0, h)],
        ShapeKind::Diamond => vec![p(w / 2.0, 0.0), p(w, h / 2.0), p(w / 2.0, h), p(0.0, h / 2.0)],
        ShapeKind::Pentagon => {
            // A regular pentagon stretched to the box.
            let (x0, y0, y1) = (-(72f32.to_radians().sin()), -1.0, 36f32.to_radians().cos());
            (0..5)
                .map(|i| {
                    let a = (-90.0 + 72.0 * i as f32).to_radians();
                    p((a.cos() - x0) / (-2.0 * x0) * w, (a.sin() - y0) / (y1 - y0) * h)
                })
                .collect()
        }
        ShapeKind::Hexagon => {
            let x1 = ss * 0.25;
            vec![p(0.0, h / 2.0), p(x1, 0.0), p(w - x1, 0.0), p(w, h / 2.0), p(w - x1, h), p(x1, h)]
        }
        ShapeKind::RightArrow | ShapeKind::LeftArrow => {
            let (y1, y2, xh) = (h * 0.25, h * 0.75, (w - ss * 0.5).max(0.0));
            let v = vec![p(0.0, y1), p(xh, y1), p(xh, 0.0), p(w, h / 2.0), p(xh, h), p(xh, y2), p(0.0, y2)];
            if kind == ShapeKind::LeftArrow { v.into_iter().map(|q| p(w - q.0, q.1)).collect() } else { v }
        }
        ShapeKind::DownArrow | ShapeKind::UpArrow => {
            let (x1, x2, yh) = (w * 0.25, w * 0.75, (h - ss * 0.5).max(0.0));
            let v = vec![p(x1, 0.0), p(x2, 0.0), p(x2, yh), p(w, yh), p(w / 2.0, h), p(0.0, yh), p(x1, yh)];
            if kind == ShapeKind::UpArrow { v.into_iter().map(|q| p(q.0, h - q.1)).collect() } else { v }
        }
        ShapeKind::Line => vec![p(0.0, 0.0), p(w, h)],
        ShapeKind::Star5 => {
            // Points on a circle whose top touches the top edge and whose
            // lower points touch the bottom edge.
            let (rx, ry) = (w / (2.0 * 72f32.to_radians().sin()), h / (1.0 + 36f32.to_radians().cos()));
            let (cx, cy) = (w / 2.0, ry);
            (0..10)
                .map(|i| {
                    let a = (-90.0 + 36.0 * i as f32).to_radians();
                    let k = if i % 2 == 0 { 1.0 } else { 0.382 };
                    p(cx + rx * k * a.cos(), cy + ry * k * a.sin())
                })
                .collect()
        }
        ShapeKind::Callout => callout(w, h, -0.20833, 0.625),
    }
}

/// DrawingML's `wedgeRectCallout`, with the tip at (`ax`, `ay`) of the
/// size from the center.
fn callout(w: f32, h: f32, ax: f32, ay: f32) -> Vec<(f32, f32)> {
    let (dx, dy) = (w * ax, h * ay);
    let (xp, yp) = (w / 2.0 + dx, h / 2.0 + dy);
    let dq = if w > 0.0 { dx * h / w } else { 0.0 };
    let horizontal = dy.abs() - dq.abs() < 0.0;
    let (xg1, xg2) = if dx > 0.0 { (7.0, 10.0) } else { (2.0, 5.0) };
    let (yg1, yg2) = if dy > 0.0 { (7.0, 10.0) } else { (2.0, 5.0) };
    let (x1, x2, y1, y2) = (w * xg1 / 12.0, w * xg2 / 12.0, h * yg1 / 12.0, h * yg2 / 12.0);
    let tip = (xp, yp);
    let mut v = vec![(0.0, 0.0)];
    if !horizontal && dy < 0.0 {
        v.extend([(x1, 0.0), tip, (x2, 0.0)]);
    }
    v.push((w, 0.0));
    if horizontal && dx > 0.0 {
        v.extend([(w, y1), tip, (w, y2)]);
    }
    v.push((w, h));
    if !horizontal && dy > 0.0 {
        v.extend([(x2, h), tip, (x1, h)]);
    }
    v.push((0.0, h));
    if horizontal && dx < 0.0 {
        v.extend([(0.0, y2), tip, (0.0, y1)]);
    }
    v
}

/// Default text size in shapes, points.
pub const SHAPE_TEXT_PT: f32 = 11.0;
/// Office's default shape colors: fill and outline.
pub const SHAPE_FILL: u32 = 0x4472C4;
pub const SHAPE_LINE: u32 = 0x2F528F;

/// A shape: what it looks like, in its own box (the containers keep its
/// place and size).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    /// A DrawingML preset we draw as `kind`, kept to write it back.
    pub preset: Option<Arc<str>>,
    /// Adjust values of the geometry (DrawingML `avLst`), by name.
    pub adj: Vec<(Arc<str>, i64)>,
    pub fill: Option<u32>,
    /// A gradient drawn in place of the fill color (which then stands for
    /// it: the automatic text color, formats without gradients).
    pub gradient: Option<crate::Gradient>,
    pub line: Option<u32>,
    /// Outline width in points.
    pub line_width: f32,
    pub dash: Dash,
    /// Arrowheads at the start and the end of lines.
    pub arrow_start: bool,
    pub arrow_end: bool,
    /// Clockwise, in degrees, about the center.
    pub rotation: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub text: Vec<ShapePara>,
    pub valign: VAnchor,
    pub name: String,
    pub alt: String,
    /// Hidden (Selection Pane): not shown nor printed, kept in files.
    pub hidden: bool,
    /// A picture filling the shape: a document's picture placed in front
    /// of or behind the text (its crop is kept; its size and rotation are
    /// the shape's).
    pub picture: Option<Arc<crate::Image>>,
    /// Its place in a chain of linked text boxes.
    pub link: Option<crate::TextLink>,
    /// Handwriting drawn in the shape's box (see [`crate::ink`]).
    pub ink: Option<Arc<crate::Ink>>,
    /// Vertical text in the box: DrawingML's `vert` ("eaVert", "vert",
    /// "vert270"...), kept for files.
    pub vert: Option<Arc<str>>,
    /// A drawn geometry (`<a:custGeom>...</a:custGeom>`, as written in
    /// DrawingML) in place of the kind's: see [`crate::custom_paths`].
    pub custom: Option<Arc<str>>,
}

impl Default for Shape {
    fn default() -> Self {
        Shape::new(ShapeKind::Rect)
    }
}

impl Shape {
    /// A shape with Office's default look for its kind.
    pub fn new(kind: ShapeKind) -> Shape {
        let (fill, line, width) = match kind {
            ShapeKind::TextBox => (Some(0xFFFFFF), Some(0x000000), 0.75),
            ShapeKind::Line => (None, Some(0x4472C4), 1.0),
            _ => (Some(SHAPE_FILL), Some(SHAPE_LINE), 1.0),
        };
        Shape {
            kind,
            preset: None,
            adj: Vec::new(),
            fill,
            gradient: None,
            line,
            line_width: width,
            dash: Dash::Solid,
            arrow_start: false,
            arrow_end: false,
            rotation: 0.0,
            flip_h: false,
            flip_v: false,
            text: Vec::new(),
            valign: if kind == ShapeKind::TextBox { VAnchor::Top } else { VAnchor::Middle },
            name: String::new(),
            alt: String::new(),
            hidden: false,
            picture: None,
            link: None,
            ink: None,
            vert: None,
            custom: None,
        }
    }

    /// The preset whose standard geometry the shape is drawn with: one we
    /// have no kind for, or any with adjust values (None: draw `kind`).
    pub fn geometry(&self) -> Option<&str> {
        match &self.preset {
            Some(p) => crate::is_preset(p).then_some(&**p),
            None if !self.adj.is_empty() && !matches!(self.kind, ShapeKind::Line | ShapeKind::TextBox) => Some(self.kind.preset()),
            None => None,
        }
    }

    /// A frame for a picture: no fill, no outline.
    pub fn picture(img: crate::Image) -> Shape {
        Shape { fill: None, line: None, rotation: img.rotation, name: img.name.clone(), alt: img.alt.clone(), hidden: img.hidden, picture: Some(Arc::new(img)), ..Shape::new(ShapeKind::Rect) }
    }

    pub fn is_text_box(&self) -> bool {
        self.kind == ShapeKind::TextBox
    }

    /// Holds text (lines and ink don't).
    pub fn can_have_text(&self) -> bool {
        !self.kind.is_line() && self.ink.is_none()
    }

    pub fn has_text(&self) -> bool {
        self.text.iter().any(|p| p.runs.iter().any(|r| !r.text.is_empty()))
    }

    /// The text, paragraphs separated by '\n'.
    pub fn plain_text(&self) -> String {
        self.text.iter().map(ShapePara::text).collect::<Vec<_>>().join("\n")
    }

    /// Replaces the text with `s` ('\n' between paragraphs). Paragraphs
    /// whose text didn't change keep their runs; changed ones take the
    /// formatting of their first run (or of the last paragraph's).
    pub fn set_plain_text(&mut self, s: &str) {
        if s == self.plain_text() {
            return;
        }
        let old = std::mem::take(&mut self.text);
        let fallback = old.last().cloned().unwrap_or_else(|| ShapePara { align: self.text_align(), runs: Vec::new() });
        if s.is_empty() {
            return;
        }
        for (i, line) in s.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            match old.get(i) {
                Some(p) if p.text() == line => self.text.push(p.clone()),
                Some(p) => self.text.push(ShapePara { align: p.align, runs: vec![p.first_run().with_text(line)] }),
                None => self.text.push(ShapePara { align: fallback.align, runs: vec![fallback.first_run().with_text(line)] }),
            }
        }
    }

    /// Applies `f` to every run (and to a run to come when there's no text).
    pub fn format_text(&mut self, f: &dyn Fn(&mut ShapeRun)) {
        if self.text.is_empty() {
            self.text.push(ShapePara { align: if self.is_text_box() { Align::Left } else { Align::Center }, runs: vec![ShapeRun::default()] });
        }
        for p in &mut self.text {
            if p.runs.is_empty() {
                p.runs.push(ShapeRun::default());
            }
            for r in &mut p.runs {
                f(r);
            }
        }
    }

    /// Text color when the runs don't set one: dark on light fills, white
    /// on dark ones (Office's defaults: text boxes black, shapes white).
    pub fn auto_text_color(&self) -> u32 {
        match self.fill.map(|c| self.gradient.as_ref().map_or(c, |g| g.main_color())) {
            Some(c) if !self.is_text_box() || c != 0xFFFFFF => {
                let (r, g, b) = ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);
                let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                if lum < 150.0 { 0xFFFFFF } else { 0x000000 }
            }
            _ => 0x000000,
        }
    }

    /// Formatting of the first run of the text.
    pub fn text_format(&self) -> ShapeRun {
        self.text.first().map(ShapePara::first_run).unwrap_or_default()
    }

    /// Alignment of the first paragraph.
    pub fn text_align(&self) -> Align {
        self.text.first().map_or(if self.is_text_box() { Align::Left } else { Align::Center }, |p| p.align)
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        let r = if self.rotation.is_finite() { self.rotation.rem_euclid(360.0) + 0.0 } else { 0.0 };
        self.rotation = if r >= 360.0 { 0.0 } else { r };
        self.line_width = if self.line_width.is_finite() { self.line_width.clamp(0.0, 100.0) } else { 1.0 };
        self.text.truncate(10_000);
        self.adj.truncate(16);
        if self.custom.as_ref().is_some_and(|c| c.len() > (1 << 22)) {
            self.custom = None;
        }
        if let Some(g) = &mut self.gradient {
            g.sanitize();
        }
        if self.fill.is_none() || self.kind.is_line() {
            self.gradient = None;
        }
        for p in &mut self.text {
            for r in &mut p.runs {
                if r.size.is_some_and(|s| !(s.is_finite() && (1.0..=400.0).contains(&s))) {
                    r.size = None;
                }
            }
        }
        if !self.can_have_text() || self.picture.is_some() {
            self.text.clear();
        }
        if let Some(ink) = &mut self.ink {
            Arc::make_mut(ink).sanitize();
        }
    }

    fn hash_into<H: Hasher>(&self, h: &mut H) {
        (self.kind, &self.preset, self.fill, self.line, self.dash, self.arrow_start, self.arrow_end, self.flip_h, self.flip_v, self.valign).hash(h);
        (self.line_width.to_bits(), self.rotation.to_bits(), &self.adj).hash(h);
        (&self.name, &self.alt, self.link, self.hidden, &self.gradient, &self.custom).hash(h);
        self.picture.as_ref().map(|i| i.data_key()).hash(h);
        if let Some(i) = &self.picture {
            i.crop.map(f32::to_bits).hash(h);
        }
        if let Some(ink) = &self.ink {
            ink.hash_into(h);
        }
        for p in &self.text {
            p.align.hash(h);
            for r in &p.runs {
                (&r.text, r.bold, r.italic, r.underline, r.size.map(f32::to_bits), r.color, &r.font).hash(h);
            }
        }
    }
}

// --- documents --------------------------------------------------------------------

/// How text flows around a shape in a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ShapeWrap {
    /// In the line, like a big character.
    Inline,
    /// Text flows beside it (approximated inside its paragraph).
    Square,
    /// On lines of its own.
    TopBottom,
    /// Over the text, which doesn't move.
    #[default]
    InFront,
    /// Under the text.
    Behind,
}

impl ShapeWrap {
    pub const ALL: [ShapeWrap; 5] = [ShapeWrap::Inline, ShapeWrap::Square, ShapeWrap::TopBottom, ShapeWrap::Behind, ShapeWrap::InFront];

    /// Placed freely on the page, out of the text flow.
    pub fn floats(self) -> bool {
        matches!(self, ShapeWrap::InFront | ShapeWrap::Behind)
    }

    pub fn key(self) -> &'static str {
        match self {
            ShapeWrap::Inline => "pic.wrap_inline",
            ShapeWrap::Square => "pic.wrap_square",
            ShapeWrap::TopBottom => "pic.wrap_top_bottom",
            ShapeWrap::InFront => "shape.wrap_front",
            ShapeWrap::Behind => "shape.wrap_behind",
        }
    }
}

/// What a shape's horizontal offset is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum HRel {
    /// The left margin (the text column).
    #[default]
    Margin,
    Page,
}

/// What a shape's vertical offset is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VRel {
    /// The top of the paragraph it is anchored to.
    #[default]
    Paragraph,
    Margin,
    Page,
}

/// A shape in a document, anchored to a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct DocShape {
    pub shape: Shape,
    /// Size in points.
    pub width: f32,
    pub height: f32,
    pub wrap: ShapeWrap,
    /// Offset of the top-left corner (before rotation) in points.
    pub x: f32,
    pub y: f32,
    pub h_rel: HRel,
    pub v_rel: VRel,
    /// Aligned in its horizontal frame instead of `x`.
    pub h_align: Option<Align>,
    /// Stacking order among the floating shapes (higher is in front).
    pub z: i32,
    /// Floating shapes grouped together share a group id (see
    /// [`Document::shape_group`]).
    pub group: Option<u32>,
}

impl DocShape {
    pub fn new(shape: Shape, width: f32, height: f32) -> DocShape {
        let mut s = DocShape { shape, width, height, wrap: ShapeWrap::InFront, x: 0.0, y: 0.0, h_rel: HRel::Margin, v_rel: VRel::Paragraph, h_align: None, z: 0, group: None };
        s.sanitize();
        s
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        let sane = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let min = if self.shape.kind.is_line() { 0.0 } else { 1.0 };
        self.width = sane(self.width, min, 20_000.0, 72.0);
        self.height = sane(self.height, min, 20_000.0, 72.0);
        self.x = sane(self.x, -20_000.0, 20_000.0, 0.0);
        self.y = sane(self.y, -20_000.0, 20_000.0, 0.0);
        self.shape.sanitize();
    }
}

impl Eq for DocShape {}

impl Hash for DocShape {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.shape.hash_into(h);
        for v in [self.width, self.height, self.x, self.y] {
            v.to_bits().hash(h);
        }
        (self.wrap, self.h_rel, self.v_rel, self.h_align, self.z, self.group).hash(h);
    }
}

impl Document {
    /// Inserts a shape anchored at the start of the selection (replacing
    /// it). Returns the shape's extent.
    pub fn insert_shape(&mut self, sel: Selection, shape: DocShape) -> Selection {
        let a = sel.start();
        let mut attrs = self.typing_attrs(a);
        attrs.clear_object();
        (attrs.link, attrs.link_tip) = (None, None);
        attrs.shape = Some(Arc::new(shape));
        let end = self.insert_text(sel, &OBJECT.to_string(), Some(attrs));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Shape;
        }
        Selection { anchor: Pos::new(end.para, end.offset - OBJECT.len_utf8()), cursor: end }
    }

    /// The shape at `p` (the character right after it) and its extent.
    pub fn shape_at(&self, p: Pos) -> Option<(Selection, Arc<DocShape>)> {
        let para = self.paragraphs.get(p.para)?;
        if !para.text.get(p.offset..)?.starts_with(OBJECT) {
            return None;
        }
        let s = para.attrs_at(p.offset + OBJECT.len_utf8()).shape?;
        Some((Selection { anchor: p, cursor: Pos::new(p.para, p.offset + OBJECT.len_utf8()) }, s))
    }

    /// Changes the shape at `p` (one undo step).
    pub fn set_shape(&mut self, p: Pos, shape: DocShape) {
        let Some((sel, old)) = self.shape_at(p) else { return };
        if *old == shape {
            return;
        }
        let shape = Arc::new(shape);
        self.format_range(sel, &|a| a.shape = Some(shape.clone()));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Shape;
        }
    }

    /// Moves the shape at `from` to the start of paragraph `to` (Word
    /// anchors a dropped shape to the paragraph under it) and changes it,
    /// as one undo step. Returns where it is now.
    pub fn move_shape(&mut self, from: Pos, to: usize, shape: DocShape) -> Pos {
        let Some((sel, _)) = self.shape_at(from) else { return from };
        if to == from.para || to >= self.paragraphs.len() {
            self.set_shape(from, shape);
            return from;
        }
        let (lo, hi) = (from.para.min(to), from.para.max(to));
        let before = self.paragraphs[lo..=hi].to_vec();
        let mut attrs = self.paragraphs[from.para].attrs_at(from.offset + OBJECT.len_utf8());
        attrs.shape = Some(Arc::new(shape));
        self.paragraphs[from.para].remove(sel.start().offset, sel.end().offset);
        self.paragraphs[to].insert_str(0, &OBJECT.to_string(), attrs);
        let at = Pos::new(to, 0);
        let after = Selection { anchor: at, cursor: Pos::new(to, OBJECT.len_utf8()) };
        self.record(lo, before, hi - lo + 1, sel, after, EditKind::Shape);
        at
    }

    /// Puts the picture at `p` in front of or behind the text (a floating
    /// shape holding it), at `offset` from the margin and its paragraph
    /// when given. One undo step.
    pub fn float_image(&mut self, p: Pos, behind: bool, offset: Option<(f32, f32)>) -> bool {
        let Some((sel, img)) = self.image_at(p) else { return false };
        let z = self.shapes().iter().filter(|(_, s)| s.wrap.floats()).map(|(_, s)| s.z).max().map_or(0, |z| z + 1);
        let (w, h) = (img.width, img.height);
        let h_align = if img.wrap == crate::Wrap::Inline { None } else { img.align };
        let x = if img.wrap == crate::Wrap::Inline { 0.0 } else { img.offset_x };
        let mut ds = DocShape::new(Shape::picture(Image { wrap: crate::Wrap::Inline, offset_x: 0.0, align: None, ..(*img).clone() }), w, h);
        (ds.wrap, ds.x, ds.h_align, ds.z) = (if behind { ShapeWrap::Behind } else { ShapeWrap::InFront }, x, h_align, z);
        if let Some((x, y)) = offset {
            (ds.x, ds.y, ds.h_align) = (x, y, None);
            ds.sanitize();
        }
        let ds = Arc::new(ds);
        self.format_range(sel, &|a| {
            a.image = None;
            a.shape = Some(ds.clone());
        });
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Image;
        }
        true
    }

    /// Puts the floating picture at `p` back in the text flow with `wrap`.
    pub fn unfloat_image(&mut self, p: Pos, wrap: crate::Wrap) -> bool {
        let Some((sel, ds)) = self.shape_at(p) else { return false };
        let Some(pic) = &ds.shape.picture else { return false };
        let aligned = wrap != crate::Wrap::Inline;
        let img = Arc::new(Image {
            width: ds.width,
            height: ds.height,
            rotation: ds.shape.rotation,
            wrap,
            offset_x: if aligned && ds.h_rel == HRel::Margin { ds.x.max(0.0) } else { 0.0 },
            align: if aligned { ds.h_align } else { None },
            name: ds.shape.name.clone(),
            alt: ds.shape.alt.clone(),
            hidden: ds.shape.hidden,
            ..(**pic).clone()
        });
        self.format_range(sel, &|a| {
            a.shape = None;
            a.image = Some(img.clone());
        });
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Image;
        }
        true
    }

    /// Every shape: its position and data.
    pub fn shapes(&self) -> Vec<(Pos, Arc<DocShape>)> {
        let mut out = Vec::new();
        for (i, para) in self.paragraphs.iter().enumerate() {
            for (s, e, a) in para.spans() {
                if let Some(sh) = &a.shape {
                    for (o, _) in para.text[s..e].match_indices(OBJECT) {
                        out.push((Pos::new(i, s + o), sh.clone()));
                    }
                }
            }
        }
        out
    }
}

/// Shapes of a paragraph: byte offset and data.
pub fn shapes_of(p: &crate::Paragraph) -> impl Iterator<Item = (usize, &Arc<DocShape>)> {
    p.spans().filter_map(|(s, e, a)| a.shape.as_ref().map(|sh| (s, e, sh))).flat_map(move |(s, e, sh)| p.text[s..e].match_indices(OBJECT).map(move |(o, _)| (s + o, sh)))
}

/// Whether a paragraph anchors shapes.
pub fn has_shapes(p: &crate::Paragraph) -> bool {
    p.runs.iter().any(|r| r.attrs.shape.is_some())
}

/// Attributes for text typed next to a shape.
pub fn without_shape(a: &Attrs) -> Attrs {
    Attrs { shape: None, ..a.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paragraph;

    fn doc() -> Document {
        Document::from_paragraphs(vec![Paragraph::new("First"), Paragraph::new("Second"), Paragraph::new("Third")])
    }

    #[test]
    fn text_editing_keeps_formatting() {
        let mut s = Shape::new(ShapeKind::Rect);
        assert_eq!(s.plain_text(), "");
        s.format_text(&|r| r.bold = true);
        s.set_plain_text("One\nTwo");
        assert_eq!(s.text.len(), 2);
        assert!(s.text.iter().all(|p| p.runs[0].bold && p.align == Align::Center));
        s.text[0].runs.push(ShapeRun { text: " more".into(), italic: true, ..Default::default() });
        s.set_plain_text("One more\nTwo changed\nThree");
        assert_eq!(s.text[0].runs.len(), 2, "unchanged paragraph keeps its runs");
        assert_eq!(s.text[1].text(), "Two changed");
        assert!(s.text[2].runs[0].bold);
        s.set_plain_text("");
        assert!(!s.has_text());
        let mut line = Shape::new(ShapeKind::Line);
        line.set_plain_text("x");
        line.rotation = -30.0;
        line.line_width = f32::NAN;
        line.sanitize();
        assert!(line.text.is_empty() && line.rotation == 330.0 && line.line_width == 1.0);
    }

    #[test]
    fn presets() {
        for k in ShapeKind::ALL {
            let (back, exact) = ShapeKind::from_preset(k.preset());
            assert!(exact);
            assert!(back == k || k == ShapeKind::TextBox);
        }
        assert_eq!(ShapeKind::from_preset("cloud"), (ShapeKind::Rect, false));
        assert_eq!(ShapeKind::from_preset("octagon").0, ShapeKind::Hexagon);
        for d in Dash::ALL {
            assert_eq!(Dash::from_preset(d.preset()), d);
        }
    }

    #[test]
    fn document_shapes_and_undo() {
        let mut d = doc();
        let sel = d.insert_shape(Selection::caret(Pos::new(1, 3)), DocShape::new(Shape::new(ShapeKind::Ellipse), 100.0, 50.0));
        assert_eq!(sel.start(), Pos::new(1, 3));
        assert_eq!(d.shapes().len(), 1);
        // Typing after it doesn't make shapes.
        let end = d.insert_text(Selection::caret(sel.end()), "x", None);
        assert!(d.paragraphs[1].attrs_at(end.offset).shape.is_none());
        let (_, s) = d.shape_at(Pos::new(1, 3)).unwrap();
        d.set_shape(Pos::new(1, 3), DocShape { width: 30.0, ..(*s).clone() });
        assert_eq!(d.shapes()[0].1.width, 30.0);
        // Moved to another paragraph in one step.
        let at = d.move_shape(Pos::new(1, 3), 2, DocShape { y: 12.0, ..(*s).clone() });
        assert_eq!(at, Pos::new(2, 0));
        assert_eq!(d.paragraphs[1].text, "Secxond");
        assert_eq!(d.shape_at(at).unwrap().1.y, 12.0);
        d.undo();
        assert_eq!(d.shapes()[0].0, Pos::new(1, 3));
        assert_eq!(d.shapes()[0].1.width, 30.0);
        d.undo();
        assert_eq!(d.shapes()[0].1.width, 100.0);
        d.redo();
        d.redo();
        assert_eq!(d.shapes()[0].0, Pos::new(2, 0));
        // Deleting its character removes it.
        let (sel, _) = d.shape_at(Pos::new(2, 0)).unwrap();
        d.delete(sel);
        assert!(d.shapes().is_empty());
        d.undo();
        assert_eq!(d.shapes().len(), 1);
        assert_eq!(d.redo_history()[0], EditKind::Delete);
    }

    #[test]
    fn floating_pictures() {
        let mut d = doc();
        let img = Image { crop: [0.1, 0.0, 0.0, 0.0], rotation: 15.0, alt: "logo".into(), ..Image::new(Arc::from(&b"png"[..]), 50.0, 40.0) };
        let s = d.insert_image(Selection::caret(Pos::new(0, 0)), img);
        let at = s.start();
        assert!(d.float_image(at, true, None));
        let (_, ds) = d.shape_at(at).unwrap();
        assert_eq!((ds.wrap, ds.width, ds.height, ds.shape.rotation), (ShapeWrap::Behind, 50.0, 40.0, 15.0));
        assert_eq!(ds.shape.picture.as_ref().unwrap().crop[0], 0.1);
        assert!(d.image_at(at).is_none());
        let mut moved = (*ds).clone();
        (moved.x, moved.width) = (30.0, 100.0);
        d.set_shape(at, moved);
        assert!(d.unfloat_image(at, crate::Wrap::Square));
        let (_, img) = d.image_at(at).unwrap();
        assert_eq!((img.width, img.offset_x, img.wrap, img.alt.as_str()), (100.0, 30.0, crate::Wrap::Square, "logo"));
        d.undo();
        d.undo();
        d.undo();
        assert_eq!(d.image_at(at).unwrap().1.width, 50.0);
    }

    #[test]
    fn hostile_values() {
        let mut s = DocShape::new(Shape::new(ShapeKind::Rect), f32::NAN, -5.0);
        s.x = f32::INFINITY;
        s.sanitize();
        assert!(s.width == 72.0 && s.height == 1.0 && s.x == 0.0);
        let l = DocShape::new(Shape::new(ShapeKind::Line), 100.0, 0.0);
        assert_eq!(l.height, 0.0);
    }
}
