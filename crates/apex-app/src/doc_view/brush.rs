//! The format painter: the formatting at the caret (or of the selected
//! text), painted on the word clicked or the text dragged over next. As
//! in Word, a caret or whole paragraphs pick up their paragraph's
//! formatting and style too; text alone carries the look its paragraph
//! style gives it (as the style's linked character style).

use super::*;

#[derive(Clone)]
pub(super) struct Brush {
    attrs: Attrs,
    /// The source paragraph's style id, for text painted into a
    /// paragraph of another style.
    style: Option<Arc<str>>,
    /// The source paragraph's formatting, when it was picked up.
    para: Option<ParaStyle>,
    /// Stays on after painting (a double click on the button), until
    /// Escape or the button again.
    pub(super) sticky: bool,
}

impl DocView {
    pub(super) fn start_brush(&mut self, sticky: bool) {
        let attrs = self.attrs_at_caret();
        let (a, b) = (self.sel.start(), self.sel.end());
        let src = self.doc.paragraphs[a.para].style;
        let whole = self.sel.is_empty() || a.para != b.para || (a.offset == 0 && b.offset >= self.doc.paragraphs[b.para].len());
        let id = self.doc.styles.para_style_id(src.kind, src.custom);
        let style = self.doc.styles.para_style_of(&id).is_some().then_some(id);
        let mut sample = Paragraph::new("").with_style(src);
        sample.push("x", attrs.clone());
        let origin = Arc::new(self.doc.origin(std::slice::from_ref(&sample)));
        self.brush = Some(Brush { attrs, style, para: whole.then_some(src), sticky });
        self.brush_serial = crate::painter::hold(crate::painter::Paint::Doc { sample, origin, whole }, sticky);
    }

    /// Takes the format picked up in another document, and lets go of the
    /// painter turned off there (see `painter::sync`).
    pub(super) fn sync_brush(&mut self) {
        use crate::painter::{Paint, Sync};
        match crate::painter::sync(self.brush.is_some(), &mut self.brush_serial) {
            Sync::Keep => {}
            Sync::Drop => self.brush = None,
            Sync::Take(h) => {
                self.brush = None;
                let Paint::Doc { sample, origin, whole } = &*h.paint else { return };
                // As its text would come pasted: with the styles it needs,
                // and the look its own document's styles gave it.
                let Some(p) = self.doc.adopt(std::slice::from_ref(sample), origin).into_iter().next() else { return };
                let id = self.doc.styles.para_style_id(p.style.kind, p.style.custom);
                let style = self.doc.styles.para_style_of(&id).is_some().then_some(id);
                self.brush = Some(Brush { attrs: p.attrs_at(0), style, para: whole.then_some(p.style), sticky: h.sticky });
            }
        }
    }

    /// Applies the format painter to the selection (or the word at the
    /// caret; with no word, to its paragraph), as one undo step.
    pub(super) fn apply_brush(&mut self) {
        let Some(b) = self.brush.clone() else { return };
        let sel = self.sel_or_word();
        let mut attrs = b.attrs.clone();
        if b.para.is_none() && attrs.char_style.is_none() && let Some(id) = &b.style {
            let other = (sel.start().para..=sel.end().para).any(|i| {
                let st = &self.doc.paragraphs[i].style;
                self.doc.styles.para_style_id(st.kind, st.custom) != *id
            });
            if other {
                attrs.char_style = Some(id.clone());
            }
        }
        let chars = move |_: ParaKind, a: &mut Attrs| {
            // What the text is (a link, an object, a tracked change) stays.
            let keep = a.clone();
            *a = attrs.clone();
            (a.link, a.link_tip, a.image, a.note, a.field, a.rev, a.math, a.shape, a.control) = (keep.link, keep.link_tip, keep.image, keep.note, keep.field, keep.rev, keep.math, keep.shape, keep.control);
        };
        match b.para {
            Some(src) => {
                let para = move |s: &mut ParaStyle| paint_para(s, src);
                if sel.is_empty() {
                    self.para_style(para);
                } else {
                    self.doc.format_with(sel, &chars, Some(&para));
                }
            }
            None if !sel.is_empty() => self.doc.format_with(sel, &chars, None),
            None => {}
        }
        if !b.sticky {
            self.brush = None;
        }
    }

    /// Escape turns the painter off, also with the focus out of the page
    /// (on the button that turned it on).
    pub(super) fn brush_escape(&mut self, ui: &egui::Ui) {
        if self.brush.is_some() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.brush = None;
        }
    }
}

/// The paragraph formatting and style of `src` on `s`, which keeps what
/// belongs to its place: breaks, numbering restarts, TOC marks, drop cap.
fn paint_para(s: &mut ParaStyle, src: ParaStyle) {
    let keep = *s;
    *s = src;
    (s.page_break_before, s.column_break_before, s.restart, s.start_at, s.toc, s.drop_cap, s.no_line_numbers) = (keep.page_break_before, keep.column_break_before, keep.restart, keep.start_at, keep.toc, keep.drop_cap, keep.no_line_numbers);
}
