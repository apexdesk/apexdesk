//! Vertical text in text boxes (DrawingML's `vert`, ODF's writing mode):
//! text turned a quarter (`vert`, `vert270`), or East Asian vertical text
//! (`eaVert`) whose columns run down and from right to left, with Chinese,
//! Japanese and Korean characters upright, Latin text on its side and the
//! vertical forms of punctuation. The galleys are laid out as horizontal
//! text as wide as the box is high and drawn turned (see
//! `LayoutJob::vertical` in the vendored epaint), so printing and PDF get
//! them too. `editor` types into such a box with a caret that runs down
//! the columns.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use apex_doc::{Shape, VAnchor};
use eframe::egui::text::{CCursor, LayoutJob};
use eframe::egui::{self, Color32, Galley, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};

/// How the text of a box runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flow {
    Horizontal,
    /// All of it turned by this angle (radians, clockwise).
    Turned(f32),
    /// East Asian vertical text.
    Vertical,
}

pub fn flow(shape: &Shape) -> Flow {
    match shape.vert.as_deref() {
        Some("vert") => Flow::Turned(FRAC_PI_2),
        Some("vert270") => Flow::Turned(-FRAC_PI_2),
        Some("eaVert" | "mongolianVert" | "wordArtVert" | "wordArtVertRtl") => Flow::Vertical,
        _ => Flow::Horizontal,
    }
}

impl Flow {
    /// The angle the galleys are drawn at.
    fn angle(self) -> f32 {
        match self {
            Flow::Horizontal => 0.0,
            Flow::Turned(a) => a,
            Flow::Vertical => FRAC_PI_2,
        }
    }
}

/// Prepares a laid out job for the flow (vertical text: upright Asian
/// characters and vertical punctuation).
pub fn prepare(job: &mut LayoutJob, f: Flow) {
    if f == Flow::Vertical {
        job.vertical = true;
        if let std::borrow::Cow::Owned(t) = apex_doc::to_vertical(&job.text) {
            job.text = t;
        }
    }
}

/// Where each galley (laid out `area.height()` wide) is drawn in `area`
/// for a turned flow: the lines stack from the right edge (clockwise) or
/// the left one (counterclockwise), and `valign` places the block across.
pub fn place(area: Rect, valign: VAnchor, f: Flow, galleys: &[Arc<Galley>]) -> Vec<Pos2> {
    let total: f32 = galleys.iter().map(|g| g.size().y).sum();
    let cw = f.angle() > 0.0;
    // The edge the first line starts at, going across the box.
    let start = match (valign, cw) {
        (VAnchor::Top, true) => area.max.x,
        (VAnchor::Top, false) => area.min.x,
        (VAnchor::Middle, true) => area.center().x + total / 2.0,
        (VAnchor::Middle, false) => area.center().x - total / 2.0,
        (VAnchor::Bottom, true) => area.min.x + total,
        (VAnchor::Bottom, false) => area.max.x - total,
    };
    let mut acc = 0.0;
    galleys
        .iter()
        .map(|g| {
            // Galleys of centered or right aligned text are laid out around x = 0.
            let along = match (g.job.halign, cw) {
                (egui::Align::Center, _) => area.center().y,
                (egui::Align::Max, true) | (egui::Align::Min, false) => area.max.y,
                _ => area.min.y,
            };
            let p = if cw { pos2(start - acc, along) } else { pos2(start + acc, along) };
            acc += g.size().y;
            p
        })
        .collect()
}

/// Paints galleys of a turned flow in `area`, all turned with the shape by
/// `rotation` degrees about `center`.
pub fn paint(p: &Painter, area: Rect, valign: VAnchor, f: Flow, galleys: Vec<Arc<Galley>>, center: Pos2, rotation: f32) {
    let at = place(area, valign, f, &galleys);
    for (g, pos) in galleys.into_iter().zip(at) {
        let pos = if rotation == 0.0 { pos } else { center + crate::shapes::turn(rotation, pos - center) };
        p.add(egui::epaint::TextShape::new(pos, g, Color32::BLACK).with_angle(f.angle() + rotation.to_radians()));
    }
}

// --- the caret in turned text --------------------------------------------------------------

/// A point of a galley drawn at `origin` turned by `angle` (±90°) on the
/// screen, and back.
fn to_screen(origin: Pos2, angle: f32, v: Vec2) -> Pos2 {
    if angle > 0.0 { origin + vec2(-v.y, v.x) } else { origin + vec2(v.y, -v.x) }
}

fn to_galley(origin: Pos2, angle: f32, q: Pos2) -> Vec2 {
    let d = q - origin;
    if angle > 0.0 { vec2(d.y, -d.x) } else { vec2(-d.y, d.x) }
}

/// Caret and selection of the vertical editor (character offsets).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Caret {
    pub cursor: usize,
    pub anchor: usize,
}

impl Caret {
    fn range(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }
}

fn byte_at(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |x| x.0)
}

/// What a key does to the text and caret of turned text laid out in
/// `galley` (arrows follow the screen: down is the next character, left
/// the next line of clockwise text). Returns false for keys it ignores.
pub fn key(text: &mut String, c: &mut Caret, galley: &Galley, clockwise: bool, key: egui::Key, shift: bool, command: bool) -> bool {
    use egui::Key;
    let n = text.chars().count();
    let delete = |text: &mut String, c: &mut Caret, a: usize, b: usize| {
        let (x, y) = (byte_at(text, a), byte_at(text, b));
        text.replace_range(x..y, "");
        *c = Caret { cursor: a, anchor: a };
    };
    let cur = CCursor::new(c.cursor);
    let next_line = |cur: &CCursor| galley.cursor_down_one_row(cur, None).0.index.0;
    let prev_line = |cur: &CCursor| galley.cursor_up_one_row(cur, None).0.index.0;
    let to = match key {
        Key::ArrowDown => c.cursor.saturating_add(1).min(n),
        Key::ArrowUp => c.cursor.saturating_sub(1),
        Key::ArrowLeft if clockwise => next_line(&cur),
        Key::ArrowLeft => prev_line(&cur),
        Key::ArrowRight if clockwise => prev_line(&cur),
        Key::ArrowRight => next_line(&cur),
        Key::Home if command => 0,
        Key::End if command => n,
        Key::Home => galley.cursor_begin_of_row(&cur).index.0,
        Key::End => galley.cursor_end_of_row(&cur).index.0,
        Key::A if command => {
            *c = Caret { cursor: n, anchor: 0 };
            return true;
        }
        Key::Backspace | Key::Delete => {
            let (a, b) = c.range();
            if a != b {
                delete(text, c, a, b);
            } else if key == Key::Backspace && a > 0 {
                delete(text, c, a - 1, a);
            } else if key == Key::Delete && a < n {
                delete(text, c, a, a + 1);
            }
            return true;
        }
        Key::Enter => {
            insert(text, c, "\n");
            return true;
        }
        _ => return false,
    };
    c.cursor = to.min(n);
    if !shift {
        c.anchor = c.cursor;
    }
    true
}

/// Types `s` over the selection.
pub fn insert(text: &mut String, c: &mut Caret, s: &str) {
    let (a, b) = c.range();
    let (x, y) = (byte_at(text, a), byte_at(text, b));
    text.replace_range(x..y, s);
    let at = a + s.chars().count();
    *c = Caret { cursor: at, anchor: at };
}

/// The editor over a box of turned or vertical text drawn in `rect`
/// (screen, unturned), with the text laid out by `layout` (as wide as the
/// text area is high). Returns true when editing is done.
#[allow(clippy::too_many_arguments)]
pub fn editor(ui: &mut egui::Ui, id: egui::Id, area: Rect, shape: &Shape, text: &mut String, fresh: &mut bool, layout: impl Fn(&str) -> Arc<Galley>) -> bool {
    let f = flow(shape);
    let angle = f.angle();
    let cw = angle > 0.0;
    let state_id = id.with("vcaret");
    let mut c: Caret = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
    let resp = ui.interact(area, id.with("varea"), egui::Sense::click_and_drag());
    if *fresh {
        resp.request_focus();
        let n = text.chars().count();
        c = Caret { cursor: n, anchor: n };
        *fresh = false;
    }
    let mut galley = layout(text);
    let origin = place(area, shape.valign, f, std::slice::from_ref(&galley))[0];
    let mut done = false;
    if resp.has_focus() {
        // Keys that move the caret stay with the editor.
        ui.memory_mut(|m| m.set_focus_lock_filter(resp.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            match ev {
                egui::Event::Text(t) | egui::Event::Ime(egui::ImeEvent::Commit(t)) if !t.chars().all(char::is_control) => insert(text, &mut c, &t),
                egui::Event::Paste(t) => insert(text, &mut c, &t.replace("\r\n", "\n")),
                egui::Event::Copy | egui::Event::Cut => {
                    let (a, b) = c.range();
                    if a < b {
                        ui.ctx().copy_text(text[byte_at(text, a)..byte_at(text, b)].to_string());
                        if matches!(ev, egui::Event::Cut) {
                            insert(text, &mut c, "");
                        }
                    }
                }
                egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => done = true,
                egui::Event::Key { key: k, pressed: true, modifiers: m, .. } => {
                    key(text, &mut c, &galley, cw, k, m.shift, m.command);
                }
                _ => {}
            }
            galley = layout(text);
        }
    }
    if let Some(q) = resp.interact_pointer_pos() {
        let at = galley.cursor_from_pos(to_galley(origin, angle, q)).index.0;
        c.cursor = at;
        if resp.clicked() {
            c.anchor = at;
        } else if resp.drag_started() {
            // The selection starts where the button went down.
            let from = ui.input(|i| i.pointer.press_origin()).unwrap_or(q);
            c.anchor = galley.cursor_from_pos(to_galley(origin, angle, from)).index.0;
        }
        resp.request_focus();
    }
    // The text, the selection and the caret, over the box.
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, id.with("vpaint")));
    let (a, b) = c.range();
    if a < b {
        let sel = Color32::from_rgba_unmultiplied(80, 140, 255, 80);
        for ci in a..b {
            let r = galley.pos_from_cursor(CCursor::new(ci));
            let r2 = galley.pos_from_cursor(CCursor::new(ci + 1));
            if (r2.min.y - r.min.y).abs() > 0.5 {
                continue;
            }
            let (p0, p1) = (to_screen(origin, angle, r.min.to_vec2()), to_screen(origin, angle, vec2(r2.min.x, r.max.y)));
            p.rect_filled(Rect::from_two_pos(p0, p1), 0.0, sel);
        }
    }
    p.add(egui::epaint::TextShape::new(origin, galley.clone(), Color32::BLACK).with_angle(angle));
    let r = galley.pos_from_cursor(CCursor::new(c.cursor));
    let (p0, p1) = (to_screen(origin, angle, r.min.to_vec2()), to_screen(origin, angle, vec2(r.min.x, r.max.y)));
    if resp.has_focus() {
        p.line_segment([p0, p1], Stroke::new(1.5, Color32::BLACK));
        let caret = Rect::from_two_pos(p0, p1).expand(1.0);
        ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { purpose: Default::default(), rect: caret, cursor_rect: caret, should_interrupt_composition: false }));
    }
    ui.data_mut(|d| d.insert_temp(state_id, c));
    let clicked_away = ui.input(|i| i.pointer.any_pressed()) && !resp.hovered() && !resp.has_focus();
    done || clicked_away || (resp.lost_focus() && !*fresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn galley(text: &str, width: f32) -> Arc<Galley> {
        let ctx = egui::Context::default();
        let mut g = None;
        ctx.run_ui(Default::default(), |ui| {
            let mut job = LayoutJob::simple(text.into(), egui::FontId::monospace(10.0), Color32::BLACK, width);
            prepare(&mut job, Flow::Vertical);
            g = Some(ui.fonts_mut(|f| f.layout_job(job)));
        }).textures_delta.clear();
        g.unwrap()
    }

    #[test]
    fn caret_runs_down_the_columns() {
        let text = "abcdefghij";
        let g = galley(text, 32.0);
        assert!(g.rows.len() > 1, "wraps into columns");
        let per = g.rows[0].row.glyphs.len();
        let mut t = text.to_string();
        let mut c = Caret { cursor: 1, anchor: 1 };
        key(&mut t, &mut c, &g, true, egui::Key::ArrowDown, false, false);
        assert_eq!(c.cursor, 2);
        // Left is the next column of clockwise text, the same place in it.
        key(&mut t, &mut c, &g, true, egui::Key::ArrowLeft, false, false);
        assert_eq!(c.cursor, 2 + per);
        key(&mut t, &mut c, &g, true, egui::Key::ArrowRight, true, false);
        assert_eq!((c.cursor, c.anchor), (2, 2 + per));
        key(&mut t, &mut c, &g, true, egui::Key::Backspace, false, false);
        assert_eq!(t.chars().count(), 10 - per);
        insert(&mut t, &mut c, "縦");
        assert_eq!(c.cursor, 3);
        assert!(t.starts_with("ab縦"));
    }

    #[test]
    fn vertical_punctuation_and_places() {
        let mut job = LayoutJob::simple("「縦」。".into(), egui::FontId::monospace(10.0), Color32::BLACK, 100.0);
        prepare(&mut job, Flow::Vertical);
        assert!(job.vertical && job.text == "﹁縦﹂︒");
        let g = galley("ab", 50.0);
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 50.0));
        // Clockwise: the first line at the right edge, reading down.
        let at = place(area, VAnchor::Top, Flow::Vertical, &[g.clone(), g.clone()]);
        assert_eq!(at[0], pos2(40.0, 0.0));
        assert!((at[1].x - (40.0 - g.size().y)).abs() < 1e-3);
        let q = to_screen(at[0], FRAC_PI_2, vec2(10.0, 0.0));
        assert_eq!(q, pos2(40.0, 10.0));
        assert_eq!(to_galley(at[0], FRAC_PI_2, q), vec2(10.0, 0.0));
    }
}
