//! The pen and the laser pointer during a show. Pen marks are drawn over
//! the slide in slide points, kept per slide while the show runs, and at
//! its end may become freeform shapes on their slides. Keys as in
//! PowerPoint: Ctrl+P pen, Ctrl+L laser, Ctrl+E eraser, Ctrl+A arrow,
//! E erases the slide's marks.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_slides::{Fill, Frame, Geometry, Line, ObjKind, Object, SColor};
use eframe::egui::{self, Color32, Key, Modifiers, Painter, Pos2, Rect, Stroke};

use super::render::Xf;

/// What the pointer does on the show's screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pointer {
    #[default]
    Arrow,
    Laser,
    Pen,
    Eraser,
}

/// A pen mark: its points (slide points), color and width (points).
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub points: Vec<(f32, f32)>,
    pub color: u32,
    pub width: f32,
}

/// Pen colors offered, the first the default (red, as PowerPoint's).
pub const COLORS: [u32; 5] = [0xE81123, 0x0078D7, 0x107C10, 0xFFB900, 0x000000];

/// The pointer's state and the marks made, by deck slide index.
#[derive(Clone, Debug, Default)]
pub struct Ink {
    pub pointer: Pointer,
    pub color: u32,
    pub marks: HashMap<usize, Vec<Mark>>,
    /// A mark being drawn.
    drawing: bool,
    /// Where the laser points (slide points).
    pub laser: Option<(f32, f32)>,
}

impl Ink {
    pub fn new() -> Ink {
        Ink { color: COLORS[0], ..Ink::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.marks.values().all(Vec::is_empty)
    }

    /// Keys of the pointer; whether one was taken.
    pub fn key(&mut self, key: Key, m: Modifiers, slide: Option<usize>) -> bool {
        let ctrl = m.ctrl || m.command;
        match key {
            Key::P if ctrl => self.pointer = Pointer::Pen,
            Key::L if ctrl => self.pointer = Pointer::Laser,
            Key::E if ctrl => self.pointer = Pointer::Eraser,
            Key::A if ctrl => self.pointer = Pointer::Arrow,
            Key::E if !ctrl && !m.alt => {
                if let Some(s) = slide {
                    self.marks.remove(&s);
                }
            }
            _ => return false,
        }
        if self.pointer != Pointer::Laser {
            self.laser = None;
        }
        true
    }

    /// The pointer on slide `slide` shown with `xf`; whether it used the
    /// press (so it does not advance the show).
    pub fn input(&mut self, resp: &egui::Response, xf: Xf, slide: usize) -> bool {
        let at = resp.hover_pos().map(|q| xf.back(q));
        match self.pointer {
            Pointer::Arrow => false,
            Pointer::Laser => {
                self.laser = at;
                false
            }
            Pointer::Pen => {
                if resp.drag_started() || (resp.is_pointer_button_down_on() && !self.drawing) {
                    if let Some(p) = at {
                        self.marks.entry(slide).or_default().push(Mark { points: vec![p], color: self.color, width: 2.5 });
                        self.drawing = true;
                    }
                }
                if self.drawing {
                    if let (Some(p), Some(m)) = (at, self.marks.get_mut(&slide).and_then(|v| v.last_mut())) {
                        if m.points.last().is_none_or(|l| (l.0 - p.0).hypot(l.1 - p.1) > 0.5) {
                            m.points.push(p);
                        }
                    }
                    if !resp.is_pointer_button_down_on() {
                        self.drawing = false;
                    }
                }
                true
            }
            Pointer::Eraser => {
                if resp.is_pointer_button_down_on() || resp.clicked() {
                    if let Some(p) = at {
                        self.erase_at(slide, p, 6.0);
                    }
                }
                true
            }
        }
    }

    /// Removes the marks of `slide` passing within `r` points of `p`.
    pub fn erase_at(&mut self, slide: usize, p: (f32, f32), r: f32) {
        if let Some(v) = self.marks.get_mut(&slide) {
            v.retain(|m| !m.points.iter().any(|q| (q.0 - p.0).hypot(q.1 - p.1) <= r + m.width / 2.0));
        }
    }

    /// Paints the marks of `slide` and the laser dot.
    pub fn paint(&self, p: &Painter, xf: Xf, slide: usize) {
        for m in self.marks.get(&slide).into_iter().flatten() {
            let pts: Vec<Pos2> = m.points.iter().map(|&(x, y)| xf.pos(x, y)).collect();
            let c = rgb(m.color);
            let w = (m.width * xf.sx).max(1.0);
            if pts.len() == 1 {
                p.circle_filled(pts[0], w / 2.0, c);
            } else {
                p.add(egui::Shape::line(pts, Stroke::new(w, c)));
            }
        }
        if let (Pointer::Laser, Some((x, y))) = (self.pointer, self.laser) {
            let c = xf.pos(x, y);
            p.circle_filled(c, 9.0, Color32::from_rgba_unmultiplied(255, 0, 0, 60));
            p.circle_filled(c, 5.0, Color32::from_rgb(255, 30, 30));
        }
    }
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A mark as a freeform shape (points in EMUs inside its frame).
pub fn mark_object(id: u32, m: &Mark) -> Option<Object> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &m.points {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    if m.points.is_empty() {
        return None;
    }
    let (w, h) = ((x1 - x0).max(0.5), (y1 - y0).max(0.5));
    let emu = |v: f32| (v * 12700.0).round() as i64;
    let mut body = String::new();
    for (i, &(x, y)) in m.points.iter().enumerate() {
        let tag = if i == 0 { "moveTo" } else { "lnTo" };
        let _ = write!(body, r#"<a:{tag}><a:pt x="{}" y="{}"/></a:{tag}>"#, emu(x - x0), emu(y - y0));
    }
    if m.points.len() == 1 {
        let _ = write!(body, r#"<a:lnTo><a:pt x="{}" y="0"/></a:lnTo>"#, emu(0.5));
    }
    let xml = format!(r#"<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst><a:path w="{}" h="{}" fill="none">{body}</a:path></a:pathLst></a:custGeom>"#, emu(w), emu(h));
    let mut o = Object::new(id, format!("Ink {}", id.saturating_sub(1)), ObjKind::Shape { text: None, text_box: false, connector: false });
    o.sp.frame = Some(Frame::new(x0, y0, w, h));
    o.sp.geom = Some(Geometry::Custom(Arc::from(xml)));
    o.sp.fill = Some(Fill::None);
    o.sp.line = Some(Line::solid(SColor::rgb(m.color), m.width));
    Some(o)
}

/// The show's pointer bar in the corner of `screen`: arrow, laser, pen
/// (with its colors) and eraser. Shown while the mouse is near it (and
/// then returns true).
pub fn bar(ui: &mut egui::Ui, screen: Rect, ink: &mut Ink) -> bool {
    let near = ui.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|p| p.x < screen.min.x + 360.0 && p.y > screen.max.y - 70.0);
    if !near {
        return false;
    }
    let tools = [
        (Pointer::Arrow, crate::i18n::tr("slides.pointer_arrow")),
        (Pointer::Laser, crate::i18n::tr("slides.pointer_laser")),
        (Pointer::Pen, crate::i18n::tr("slides.pointer_pen")),
        (Pointer::Eraser, crate::i18n::tr("slides.pointer_eraser")),
    ];
    let mut x = screen.min.x + 12.0;
    let y = screen.max.y - 40.0;
    for (tool, name) in tools {
        let w = 16.0 + name.chars().count() as f32 * 7.0;
        let r = Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, 26.0));
        let b = egui::Button::selectable(ink.pointer == tool, egui::RichText::new(name).size(12.0));
        if ui.put(r, b).clicked() {
            ink.pointer = tool;
            ink.laser = None;
        }
        x += w + 4.0;
    }
    if ink.pointer == Pointer::Pen {
        for c in COLORS {
            let r = Rect::from_min_size(egui::pos2(x + 4.0, y + 4.0), egui::vec2(18.0, 18.0));
            let resp = ui.interact(r, ui.id().with(("ink_color", c)), egui::Sense::click());
            ui.painter().rect_filled(r, 3.0, rgb(c));
            if ink.color == c {
                ui.painter().rect_stroke(r.expand(2.0), 3.0, Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Outside);
            }
            crate::widgets::name(&resp, egui::WidgetType::Button, &format!("#{c:06X}"), Some(ink.color == c));
            if resp.clicked() {
                ink.color = c;
            }
            x += 24.0;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_pick_the_pointer_and_erase() {
        let mut ink = Ink::new();
        assert!(ink.key(Key::P, Modifiers::CTRL, Some(0)));
        assert_eq!(ink.pointer, Pointer::Pen);
        ink.marks.insert(0, vec![Mark { points: vec![(1.0, 1.0)], color: 0, width: 2.0 }]);
        assert!(!ink.is_empty());
        assert!(ink.key(Key::E, Modifiers::NONE, Some(0)));
        assert!(ink.is_empty());
        assert!(ink.key(Key::L, Modifiers::CTRL, None));
        assert_eq!(ink.pointer, Pointer::Laser);
        assert!(!ink.key(Key::N, Modifiers::NONE, None));
    }

    #[test]
    fn eraser_takes_marks_it_touches() {
        let mut ink = Ink::new();
        ink.marks.insert(2, vec![Mark { points: vec![(0.0, 0.0), (100.0, 0.0)], color: 0, width: 2.0 }, Mark { points: vec![(0.0, 50.0)], color: 0, width: 2.0 }]);
        ink.erase_at(2, (100.0, 3.0), 4.0);
        assert_eq!(ink.marks[&2].len(), 1);
        assert_eq!(ink.marks[&2][0].points, vec![(0.0, 50.0)]);
    }

    #[test]
    fn marks_become_freeforms() {
        let m = Mark { points: vec![(10.0, 20.0), (30.0, 25.0), (20.0, 40.0)], color: 0xE81123, width: 2.5 };
        let o = mark_object(5, &m).unwrap();
        assert_eq!(o.sp.frame, Some(Frame::new(10.0, 20.0, 20.0, 20.0)));
        let Some(Geometry::Custom(xml)) = &o.sp.geom else { panic!() };
        assert!(xml.contains(r#"<a:moveTo><a:pt x="0" y="0"/></a:moveTo>"#) && xml.contains(r#"<a:pt x="254000" y="63500"/>"#), "{xml}");
        let paths = apex_slides::geom::custom_paths(xml, 20.0, 20.0);
        assert_eq!(paths.len(), 1);
    }
}
