//! Dragging selected text inside a text field (text boxes, notes): it
//! moves where it is dropped, Ctrl (Option on macOS) copies it, Esc
//! cancels. As in the document body (`doc_view/text_drag.rs`).
//!
//! ```ignore
//! let drag = edit_drag::begin(ui, id);
//! let out = edit.interactive(!drag.grabbing()).show(ui);
//! if drag.finish(ui, &out, &mut text) { /* text changed */ }
//! ```
//!
//! While the selection is being dragged the field doesn't take the mouse
//! (it would select instead); it keeps its focus and selection.

use std::sync::Arc;

use eframe::egui::{self, Id, Pos2, Stroke, Ui, Vec2};
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditOutput;

/// How far the pointer moves before a press in the selection drags it.
const DRAG_START: f32 = 4.0;

#[derive(Clone)]
struct Drag {
    from: Pos2,
    /// The selection, in characters.
    start: usize,
    end: usize,
    /// The pointer moved far enough: a drag, not a click.
    moving: bool,
    /// Where the text would go, in characters.
    to: Option<usize>,
}

/// The field's text as last drawn, to find characters under the pointer.
#[derive(Clone)]
struct Drawn {
    galley: Arc<egui::Galley>,
    origin: Pos2,
}

impl Drawn {
    fn char_at(&self, p: Pos2) -> usize {
        self.galley.cursor_from_pos(p - self.origin).index.0
    }
}

pub struct EditDrag {
    id: Id,
    drag: Option<Drag>,
}

fn drag_id(id: Id) -> Id {
    id.with("edit_drag")
}

/// Call before showing the field `id`: a press in its selection starts a
/// drag.
pub fn begin(ui: &Ui, id: Id) -> EditDrag {
    let ctx = ui.ctx();
    let mut drag = ctx.data(|d| d.get_temp::<Drag>(drag_id(id)));
    if drag.is_none()
        && ctx.memory(|m| m.has_focus(id))
        && let Some(at) = ui.input(|i| i.pointer.primary_pressed().then(|| i.pointer.interact_pos()).flatten())
        && let Some(drawn) = ctx.data(|d| d.get_temp::<Drawn>(drag_id(id).with("drawn")))
        && drawn.galley.rect.translate(drawn.origin.to_vec2()).contains(at)
        && let Some(sel) = egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range())
    {
        let r = sel.as_sorted_char_range();
        let (start, end) = (r.start.0, r.end.0);
        let i = drawn.char_at(at);
        if start < end && i >= start && i < end {
            drag = Some(Drag { from: at, start, end, moving: false, to: None });
        }
    }
    EditDrag { id, drag }
}

impl EditDrag {
    /// Whether the selection is being dragged: the field then mustn't be
    /// interactive.
    pub fn grabbing(&self) -> bool {
        self.drag.is_some()
    }

    /// Call after showing the field, with its text. Returns whether the
    /// text changed (dropped).
    pub fn finish(self, ui: &Ui, out: &TextEditOutput, text: &mut String) -> bool {
        let ctx = ui.ctx();
        let key = drag_id(self.id);
        let drawn = Drawn { galley: out.galley.clone(), origin: out.galley_pos - Vec2::new(out.galley.rect.left(), 0.0) };
        ctx.data_mut(|d| d.insert_temp(key.with("drawn"), drawn.clone()));
        let Some(mut d) = self.drag else { return false };
        let (down, at, copy) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), crate::doc_view::text_drag::copy_modifier(&i.modifiers)));
        let select = |a: usize, b: usize| {
            let mut state = out.state.clone();
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
            state.store(ctx, self.id);
        };
        // Esc drops nothing: the selection stays as it was.
        if d.moving && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            ctx.data_mut(|m| m.remove::<Drag>(key));
            ctx.request_repaint();
            return false;
        }
        if down {
            if let Some(at) = at {
                d.moving |= (at - d.from).length() > DRAG_START;
                if d.moving {
                    ctx.set_cursor_icon(if copy { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
                    let to = drawn.char_at(at);
                    d.to = Some(to);
                    if to <= d.start || to >= d.end {
                        let r = drawn.galley.pos_from_cursor(CCursor::new(to)).translate(drawn.origin.to_vec2());
                        ui.painter().vline(r.min.x.round() + 0.5, r.y_range(), Stroke::new(2.0, ui.visuals().text_color()));
                    }
                    ctx.request_repaint();
                }
            }
            ctx.data_mut(|m| m.insert_temp(key, d));
            return false;
        }
        ctx.data_mut(|m| m.remove::<Drag>(key));
        match d.to.filter(|_| d.moving).and_then(|to| moved(text, d.start, d.end, to, copy)) {
            Some((t, a, b)) => {
                *text = t;
                select(a, b);
                true
            }
            None => {
                if !d.moving {
                    // A click in the selection places the caret.
                    let i = drawn.char_at(d.from);
                    select(i, i);
                }
                false
            }
        }
    }
}

/// `text` with characters `start..end` moved (or copied) to `to`, and
/// where they are now. A word moved between words keeps one space on each
/// side. None when it would stay as it is.
fn moved(text: &str, start: usize, end: usize, to: usize, copy: bool) -> Option<(String, usize, usize)> {
    if (to > start && to < end) || (!copy && (to == start || to == end)) {
        return None;
    }
    let mut chars: Vec<char> = text.chars().collect();
    if end > chars.len() || to > chars.len() {
        return None;
    }
    let piece: Vec<char> = chars[start..end].to_vec();
    let words = !piece.contains(&'\n') && piece.first().is_some_and(|c| !c.is_whitespace()) && piece.last().is_some_and(|c| !c.is_whitespace());
    let mut to = to;
    if !copy {
        // The cut, with the space that would be left doubled (or before
        // punctuation).
        let (mut a, mut b) = (start, end);
        if words {
            match (a.checked_sub(1).map(|i| chars[i]), chars.get(b).copied()) {
                (Some(' ') | None, Some(' ')) => b += 1,
                (Some(' '), None | Some('.' | ',' | ';' | ':' | '!' | '?' | ')')) => a -= 1,
                _ => {}
            }
        }
        chars.drain(a..b);
        if to >= b {
            to -= b - a;
        }
    }
    let mut ins = piece;
    let mut at = to;
    if words {
        if chars.get(to).is_some_and(|c| c.is_alphanumeric()) {
            ins.push(' ');
        }
        if to > 0 && chars[to - 1].is_alphanumeric() {
            ins.insert(0, ' ');
            at += 1;
        }
    }
    let n = end - start;
    chars.splice(to..to, ins);
    Some((chars.into_iter().collect(), at, at + n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(r: Option<(String, usize, usize)>) -> (String, String) {
        let (t, a, b) = r.unwrap();
        let s: String = t.chars().skip(a).take(b - a).collect();
        (t, s)
    }

    #[test]
    fn moves_a_word_with_its_spaces() {
        assert_eq!(at(moved("one two three", 4, 7, 13, false)), ("one three two".into(), "two".into()));
        assert_eq!(at(moved("one two three.", 8, 13, 0, false)), ("three one two.".into(), "three".into()));
        assert_eq!(at(moved("one two", 0, 3, 7, true)), ("one two one".into(), "one".into()));
        assert_eq!(at(moved("añó bé\ncé", 0, 3, 9, false)), ("bé\ncé añó".into(), "añó".into()));
        // Into itself, or where it is: nothing.
        assert!(moved("one two", 0, 3, 2, false).is_none());
        assert!(moved("one two", 0, 3, 3, false).is_none());
    }

    /// One frame with a field showing `text`, after `events`.
    fn frame(ctx: &egui::Context, text: &mut String, events: Vec<egui::Event>) {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))), events, ..Default::default() };
        ctx.run_ui(input, |ui| {
            let id = Id::new("field");
            let drag = begin(ui, id);
            let out = egui::TextEdit::multiline(text).id(id).interactive(!drag.grabbing()).show(ui);
            drag.finish(ui, &out, text);
        })
        .textures_delta
        .clear();
    }

    #[test]
    fn drags_in_a_field() {
        let ctx = egui::Context::default();
        let id = Id::new("field");
        let mut text = String::from("one two three");
        frame(&ctx, &mut text, vec![]);
        ctx.memory_mut(|m| m.request_focus(id));
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(4), CCursor::new(7))));
        state.store(&ctx, id);
        frame(&ctx, &mut text, vec![]);
        let drawn = ctx.data(|d| d.get_temp::<Drawn>(drag_id(id).with("drawn"))).unwrap();
        let pos = |i: usize| drawn.galley.pos_from_cursor(CCursor::new(i)).translate(drawn.origin.to_vec2()).left_center();
        let button = |at: Pos2, pressed: bool| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let (from, to) = (pos(5) + Vec2::new(2.0, 0.0), pos(13));
        frame(&ctx, &mut text, vec![egui::Event::PointerMoved(from), button(from, true)]);
        frame(&ctx, &mut text, vec![egui::Event::PointerMoved(to)]);
        frame(&ctx, &mut text, vec![button(to, false)]);
        assert_eq!(text, "one three two");
        let sel = egui::TextEdit::load_state(&ctx, id).and_then(|s| s.cursor.char_range()).unwrap().as_sorted_char_range();
        assert_eq!((sel.start.0, sel.end.0), (10, 13));
    }
}
