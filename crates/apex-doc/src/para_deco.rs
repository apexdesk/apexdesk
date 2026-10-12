//! Paragraph decoration: borders, tab stops and drop caps. All of it is
//! `Copy` so that it lives in `ParaStyle` and follows edits and undo.

/// How a border line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum LineStyle {
    #[default]
    Single,
    Double,
    Dotted,
    Dashed,
    DashDot,
    Thick,
    Wave,
}

impl LineStyle {
    pub const ALL: [LineStyle; 7] = [LineStyle::Single, LineStyle::Double, LineStyle::Dotted, LineStyle::Dashed, LineStyle::DashDot, LineStyle::Thick, LineStyle::Wave];
}

/// A border line of a paragraph or page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub style: LineStyle,
    /// Width in points.
    pub width: f32,
    pub color: u32,
    /// Distance from the text (or from the page edge for page borders
    /// measured from it), in points.
    pub space: f32,
}

impl Line {
    pub const fn new(style: LineStyle, width: f32, color: u32, space: f32) -> Line {
        Line { style, width, color, space }
    }

    /// Room the line takes across its direction: the line itself, both
    /// lines and the gap for a double one.
    pub fn thickness(&self) -> f32 {
        match self.style {
            LineStyle::Double => self.width * 3.0,
            LineStyle::Wave => self.width * 3.0,
            _ => self.width,
        }
        .max(0.0)
    }
}

impl Default for Line {
    fn default() -> Self {
        Line { style: LineStyle::Single, width: 0.5, color: 0, space: 1.0 }
    }
}

/// Paragraph borders. Consecutive paragraphs with the same borders form a
/// group: the top line goes above the first, the bottom line below the
/// last and the `between` line between them.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ParaBorders {
    pub top: Option<Line>,
    pub bottom: Option<Line>,
    pub left: Option<Line>,
    pub right: Option<Line>,
    pub between: Option<Line>,
}

impl ParaBorders {
    pub fn is_empty(&self) -> bool {
        self.top.is_none() && self.bottom.is_none() && self.left.is_none() && self.right.is_none() && self.between.is_none()
    }

    /// The same line on the four sides (a box).
    pub fn boxed(l: Line) -> ParaBorders {
        ParaBorders { top: Some(l), bottom: Some(Line { space: l.space, ..l }), left: Some(Line { space: l.space.max(4.0), ..l }), right: Some(Line { space: l.space.max(4.0), ..l }), between: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TabAlign {
    #[default]
    Left,
    Center,
    Right,
    Decimal,
}

impl TabAlign {
    pub const ALL: [TabAlign; 4] = [TabAlign::Left, TabAlign::Center, TabAlign::Right, TabAlign::Decimal];
}

/// What fills the space a tab jumps over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TabLeader {
    #[default]
    None,
    Dots,
    Dashes,
    Underline,
}

impl TabLeader {
    pub const ALL: [TabLeader; 4] = [TabLeader::None, TabLeader::Dots, TabLeader::Dashes, TabLeader::Underline];
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TabStop {
    /// Position in points from the left edge of the text area (the page
    /// margin, or the cell edge in a table), like other editors.
    pub pos: f32,
    pub align: TabAlign,
    pub leader: TabLeader,
}

/// Distance between the default tab stops, in points (half an inch).
pub const DEFAULT_TAB: f32 = 36.0;
/// Custom tab stops per paragraph; more are dropped when reading files.
pub const MAX_TABS: usize = 16;

/// A paragraph's custom tab stops, sorted by position.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TabStops {
    n: u8,
    stops: [TabStop; MAX_TABS],
}

impl TabStops {
    pub fn from_slice(v: &[TabStop]) -> TabStops {
        let mut t = TabStops::default();
        for s in v {
            t.set(*s);
        }
        t
    }

    pub fn as_slice(&self) -> &[TabStop] {
        &self.stops[..self.n as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn len(&self) -> usize {
        self.n as usize
    }

    /// Adds a stop, replacing one at (almost) the same position. Positions
    /// that aren't finite are ignored, and a full list stays as it is.
    pub fn set(&mut self, s: TabStop) {
        if !s.pos.is_finite() {
            return;
        }
        let mut v: Vec<TabStop> = self.as_slice().iter().copied().filter(|o| (o.pos - s.pos).abs() >= 0.5).collect();
        if v.len() >= MAX_TABS {
            return;
        }
        v.push(s);
        v.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        self.n = v.len() as u8;
        self.stops[..v.len()].copy_from_slice(&v);
    }

    /// Removes the stop at (almost) `pos`.
    pub fn remove(&mut self, pos: f32) {
        let v: Vec<TabStop> = self.as_slice().iter().copied().filter(|o| (o.pos - pos).abs() >= 0.5).collect();
        *self = TabStops::default();
        self.n = v.len() as u8;
        self.stops[..v.len()].copy_from_slice(&v);
    }

    /// The stop a tab starting at `x` (points from the text area's left
    /// edge) goes to: the next custom stop, else the next default stop.
    pub fn next(&self, x: f32) -> TabStop {
        if let Some(s) = self.as_slice().iter().find(|s| s.pos > x + 0.01) {
            return *s;
        }
        // Default stops start after the last custom one.
        let from = self.as_slice().last().map_or(0.0, |s| s.pos).max(x);
        let pos = ((from + 0.01) / DEFAULT_TAB).floor() * DEFAULT_TAB + DEFAULT_TAB;
        TabStop { pos, ..Default::default() }
    }
}

/// A large initial letter spanning several lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropCap {
    /// Lines it spans (2..=10).
    pub lines: u8,
    /// In the margin instead of dropped into the text.
    pub margin: bool,
    /// Characters it takes (usually one letter).
    pub chars: u8,
    /// Distance from the text, in points.
    pub distance: f32,
}

impl Default for DropCap {
    fn default() -> Self {
        DropCap { lines: 3, margin: false, chars: 1, distance: 0.0 }
    }
}

impl DropCap {
    /// Clamped to the values editors support.
    pub fn sane(self) -> DropCap {
        DropCap { lines: self.lines.clamp(1, 10), chars: self.chars.clamp(1, 255), distance: if self.distance.is_finite() { self.distance.clamp(0.0, 200.0) } else { 0.0 }, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_stops() {
        let mut t = TabStops::default();
        t.set(TabStop { pos: 100.0, align: TabAlign::Right, leader: TabLeader::Dots });
        t.set(TabStop { pos: 50.0, ..Default::default() });
        t.set(TabStop { pos: 100.2, align: TabAlign::Center, ..Default::default() });
        t.set(TabStop { pos: f32::NAN, ..Default::default() });
        assert_eq!(t.as_slice().iter().map(|s| s.pos).collect::<Vec<_>>(), [50.0, 100.2]);
        assert_eq!(t.as_slice()[1].align, TabAlign::Center);
        assert_eq!(t.next(0.0).pos, 50.0);
        assert_eq!(t.next(50.0).pos, 100.2);
        // After the custom stops, every half inch.
        assert_eq!(t.next(101.0).pos, 108.0);
        assert_eq!(TabStops::default().next(36.0).pos, 72.0);
        t.remove(50.0);
        assert_eq!(t.len(), 1);
        for i in 0..40 {
            t.set(TabStop { pos: i as f32 * 10.0, ..Default::default() });
        }
        assert_eq!(t.len(), MAX_TABS);
    }
}
