//! Dragging selected text: it moves where it is dropped, Ctrl (Option on
//! macOS) copies it. A word moved between words keeps one space on each
//! side, as in Word. A press in the selection that doesn't move places
//! the caret, as any click; Esc while dragging leaves it all as it was.

use super::*;

/// How far the pointer moves before a press in the selection drags it.
const DRAG_START: f32 = 4.0;

pub(super) struct TextDrag {
    /// Where the button went down, on the canvas.
    from: Pos2,
    /// The pointer moved far enough: a drag, not a click.
    moving: bool,
    /// Where the text would go.
    to: Option<Pos>,
}

impl DocView {
    /// Whether a press at `pos` grabs the selection to drag it.
    pub(super) fn grabs_selection(&self, pos: Pos) -> bool {
        !self.sel.is_empty() && self.doc.block_selection(self.sel).is_none() && pos >= self.sel.start() && pos < self.sel.end()
    }

    pub(super) fn start_text_drag(&mut self, from: Pos2) {
        self.text_drag = Some(TextDrag { from, moving: false, to: None });
    }

    /// Follows the pointer while the button is down; drops the text when
    /// it goes up. Returns whether a drag is going on (it takes the mouse).
    pub(super) fn text_drag_mouse(&mut self, ui: &egui::Ui, origin: Pos2) -> bool {
        let Some(d) = &mut self.text_drag else { return false };
        let (down, at, copy) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), copy_modifier(&i.modifiers)));
        let Some(at) = at else { return true };
        // Esc drops nothing: the selection stays as it was.
        if d.moving && ui.input_mut(|i| i.consume_key(Modifiers::NONE, egui::Key::Escape)) {
            self.text_drag = None;
            ui.ctx().request_repaint();
            return true;
        }
        let local = at - origin.to_vec2();
        if down {
            d.moving |= (local - d.from).length() > DRAG_START;
            if d.moving {
                ui.ctx().set_cursor_icon(if copy { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
                let to = self.pos_at(local);
                if let Some(d) = &mut self.text_drag {
                    d.to = Some(to);
                }
                // Past the visible area: scroll.
                let clip = ui.clip_rect();
                if at.y > clip.max.y || at.y < clip.min.y {
                    ui.scroll_to_rect(Rect::from_center_size(at, vec2(1.0, 1.0)), None);
                }
                ui.ctx().request_repaint();
            }
            return true;
        }
        let Some(d) = self.text_drag.take() else { return true };
        match d.to.filter(|_| d.moving) {
            Some(to) => self.drop_text(to, copy),
            None => {
                let pos = self.pos_at(d.from);
                self.caret(pos, false);
                self.desired_x = None;
            }
        }
        true
    }

    /// The caret where dragged text would land.
    pub(super) fn paint_text_drop(&self, p: &egui::Painter, off: Vec2) {
        let Some(to) = self.text_drag.as_ref().filter(|d| d.moving).and_then(|d| d.to) else { return };
        if to > self.sel.start() && to < self.sel.end() {
            return;
        }
        let r = self.caret_rect(to).translate(off);
        let stroke = Stroke::new(2.0, Paper::TEXT);
        if r.width() > r.height() {
            p.hline(r.x_range(), r.min.y.round() + 0.5, stroke);
        } else {
            p.vline(r.min.x.round() + 0.5, r.y_range(), stroke);
        }
    }

    /// Moves (or copies) the selection to `to`, as one undo step, and
    /// selects it there.
    pub(super) fn drop_text(&mut self, to: Pos, copy: bool) {
        let mark = self.doc.undo_mark();
        let Some(sel) = move_text(&mut self.doc, self.sel, to, copy) else { return };
        self.sel = sel;
        self.doc.join_undo(mark);
        self.desired_x = None;
        self.scroll_to_caret = true;
    }
}

/// Moves (or copies) the text selected by `sel` in `doc` to `to`; returns
/// where it is then, or None when it would stay as it is. Also for the
/// text of slides.
pub(crate) fn move_text(doc: &mut Document, sel: Selection, to: Pos, copy: bool) -> Option<Selection> {
    let (a, b) = (sel.start(), sel.end());
    if (to > a && to < b) || (!copy && (to == a || to == b)) {
        return None;
    }
    let mut paras = doc.slice(a, b);
    if paras.iter().any(|p| p.has_deleted()) {
        paras = Document::from_paragraphs(paras).resolved(true).paragraphs;
    }
    let text = doc.visible_text_range(a, b);
    // Smart spaces: only for words within one paragraph.
    let words = a.para == b.para && !text.is_empty() && text.trim() == text;
    let (start, end) = if to >= b {
        // Inserted after the selection, which stays where it was.
        let (s, e) = insert_dragged(doc, to, &paras, words);
        if copy {
            (s, e)
        } else {
            let n = doc.paragraphs.len();
            let lens: Vec<usize> = [s.para, e.para].iter().map(|&i| doc.paragraphs[i].text.len()).collect();
            cut_dragged(doc, a, b, words);
            // What follows the cut keeps its distance to the end of
            // its paragraph and of the document.
            let m = doc.paragraphs.len();
            let shift = |p: Pos, len: usize| {
                let para = p.para + m - n;
                Pos::new(para, doc.paragraphs[para].text.len() - (len - p.offset))
            };
            (shift(s, lens[0]), shift(e, lens[1]))
        }
    } else {
        // Before it: the cut goes first and leaves `to` where it is.
        if !copy {
            cut_dragged(doc, a, b, words);
        }
        insert_dragged(doc, to, &paras, words)
    };
    Some(Selection { anchor: start, cursor: end })
}

/// Inserts dragged paragraphs at `to`, with a space between them and a
/// word they touch; returns where they start and end.
fn insert_dragged(doc: &mut Document, to: Pos, paras: &[Paragraph], words: bool) -> (Pos, Pos) {
    let mut e = doc.insert_paragraphs(Selection::caret(to), paras);
    let mut s = to;
    if !words {
        return (s, e);
    }
    let first = paras.first().and_then(|p| p.text.chars().next());
    let last = paras.last().and_then(|p| p.text.chars().next_back());
    if last.is_some_and(is_word) && char_after(doc, e).is_some_and(is_word) {
        doc.insert_text(Selection::caret(e), " ", None);
    }
    if first.is_some_and(is_word) && char_before(doc, s).is_some_and(is_word) {
        doc.insert_text(Selection::caret(s), " ", None);
        s.offset += 1;
        if e.para == s.para {
            e.offset += 1;
        }
    }
    (s, e)
}

/// Deletes the dragged text from `a`..`b` and, for words, the space that
/// would be left doubled (or before punctuation).
fn cut_dragged(doc: &mut Document, mut a: Pos, mut b: Pos, words: bool) {
    if words {
        match (char_before(doc, a), char_after(doc, b)) {
            (Some(' ') | None, Some(' ')) => b.offset += 1,
            (Some(' '), None | Some('.' | ',' | ';' | ':' | '!' | '?' | ')')) => a.offset -= 1,
            _ => {}
        }
    }
    doc.delete(Selection { anchor: a, cursor: b });
}

fn char_before(doc: &Document, p: Pos) -> Option<char> {
    doc.paragraphs.get(p.para)?.text.get(..p.offset)?.chars().next_back()
}

fn char_after(doc: &Document, p: Pos) -> Option<char> {
    doc.paragraphs.get(p.para)?.text.get(p.offset..)?.chars().next()
}

/// The key held to copy rather than move: Ctrl, Option on macOS.
pub(crate) fn copy_modifier(m: &Modifiers) -> bool {
    if cfg!(target_os = "macos") { m.alt } else { m.ctrl || m.command }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text: &str) -> DocView {
        DocView::new(Document::from_paragraphs(text.split('\n').map(Paragraph::new).collect()))
    }

    #[test]
    fn moves_a_word_with_its_spaces() {
        let mut v = view("one two three");
        v.set_sel(Selection { anchor: Pos::new(0, 4), cursor: Pos::new(0, 7) });
        v.drop_text(Pos::new(0, 13), false);
        assert_eq!(v.doc.paragraphs[0].text, "one three two");
        assert_eq!(v.doc.visible_text_range(v.sel.start(), v.sel.end()), "two");
        v.undo();
        assert_eq!(v.doc.paragraphs[0].text, "one two three");
        // Backwards, and before a full stop.
        let mut v = view("one two three.");
        v.set_sel(Selection { anchor: Pos::new(0, 8), cursor: Pos::new(0, 13) });
        v.drop_text(Pos::new(0, 0), false);
        assert_eq!(v.doc.paragraphs[0].text, "three one two.");
    }

    #[test]
    fn copies_across_paragraphs() {
        let mut v = view("alpha beta\ngamma");
        v.set_sel(Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 5) });
        v.drop_text(Pos::new(1, 5), true);
        assert_eq!(v.doc.paragraphs[0].text, "alpha beta");
        assert_eq!(v.doc.paragraphs[1].text, "gamma alpha");
        // Moved to the next paragraph.
        let mut v = view("alpha beta\ngamma");
        v.set_sel(Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 5) });
        v.drop_text(Pos::new(1, 0), false);
        assert_eq!(v.doc.paragraphs[0].text, "beta");
        assert_eq!(v.doc.paragraphs[1].text, "alpha gamma");
        assert_eq!(v.doc.visible_text_range(v.sel.start(), v.sel.end()), "alpha");
        // Into itself: nothing.
        let mut v = view("alpha beta");
        v.set_sel(Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 10) });
        v.drop_text(Pos::new(0, 3), false);
        assert_eq!(v.doc.paragraphs[0].text, "alpha beta");
    }
}
