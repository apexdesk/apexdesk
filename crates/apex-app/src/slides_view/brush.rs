//! The format painter on slides: the look of the text at the caret (or
//! of the selected object's text), painted on the object clicked next
//! (all its text), or on the word clicked or the text dragged over in the
//! text being typed. The look is resolved (what the placeholder and the
//! master give it too), so it shows the same wherever it goes; a caret
//! or a whole object also carries the paragraph's formatting.

use apex_doc::{Attrs, Document, Selection};
use apex_slides::{ParaProps, TextBody};

use super::{SlidesView, format, objects};

#[derive(Clone)]
pub(super) struct Brush {
    attrs: Attrs,
    /// The paragraph's formatting and level, when it was picked up.
    para: Option<(ParaProps, u8)>,
    /// Stays on after painting (a double click on the button), until
    /// Escape or the button again.
    pub(super) sticky: bool,
}

impl SlidesView {
    /// Picks up the format; nothing when there's no text to take it from.
    pub(super) fn start_brush(&mut self, sticky: bool) {
        let (id, para, off, whole) = match &self.typing {
            Some(t) => {
                let (a, b) = (t.sel.start(), t.sel.end());
                let whole = t.sel.is_empty() || a.para != b.para;
                (t.id, a.para, a.offset, whole)
            }
            None => match self.selection().first() {
                Some(id) => (*id, 0, 0, true),
                None => return,
            },
        };
        let Some(o) = self.cur_objects().iter().find(|o| o.id == id) else { return };
        let Some(ch) = objects::chain(&self.pres.deck, self.target()) else { return };
        let empty = TextBody::plain("");
        let t = o.text().unwrap_or(&empty);
        let pp = ch.para(o, t, para);
        let mut attrs = format::run_attrs(t, para, off);
        let f = ch.run(o, &pp, &attrs);
        attrs.size = Some((f.size * 2.0).round() as u16);
        (attrs.bold, attrs.italic, attrs.underline, attrs.strike) = (f.bold, f.italic, f.underline, f.strike);
        // Theme colors stay theme colors (they follow the target's).
        attrs.font = Some(f.font);
        let level = t.paras.get(para).map_or(0, |p| p.style.level);
        let para = whole.then(|| (ParaProps { run: Default::default(), ..pp }, level));
        self.brush_serial = crate::painter::hold(crate::painter::Paint::Slide { attrs: attrs.clone(), para: para.clone() }, sticky);
        self.brush = Some(Brush { attrs, para, sticky });
    }

    /// Takes the format picked up in another presentation, and lets go of
    /// the painter turned off there (see `painter::sync`).
    pub(super) fn sync_brush(&mut self) {
        use crate::painter::{Paint, Sync};
        match crate::painter::sync(self.brush.is_some(), &mut self.brush_serial) {
            Sync::Keep => {}
            Sync::Drop => self.brush = None,
            Sync::Take(h) => {
                self.brush = None;
                let Paint::Slide { attrs, para } = &*h.paint else { return };
                self.brush = Some(Brush { attrs: attrs.clone(), para: para.clone(), sticky: h.sticky });
            }
        }
    }

    /// Paints the format on the selected text (the word at the caret with
    /// none), or on all the text of the selected objects, as one step.
    pub(super) fn apply_brush(&mut self) {
        let Some(b) = self.brush.clone() else { return };
        let attrs = b.attrs.clone();
        let chars = move |a: &mut Attrs| {
            // What the text is (a link, a field) stays.
            let keep = a.clone();
            *a = attrs.clone();
            (a.link, a.link_tip, a.field, a.rev, a.math, a.image, a.shape) = (keep.link, keep.link_tip, keep.field, keep.rev, keep.math, keep.image, keep.shape);
        };
        let para = move |t: &mut TextBody, i: usize| {
            if let Some((pp, level)) = &b.para
                && i < t.paras.len()
            {
                t.set_props(i, pp.clone());
                t.paras[i].style.level = *level;
            }
        };
        const WHAT: &str = "toolstrip.brush";
        if let Some(t) = &self.typing {
            let (id, cell, mut sel) = (t.id, t.cell, t.sel);
            let Some(text) = self.typing_text().cloned() else { return };
            let mut d = Document::new();
            d.paragraphs = text.paras;
            if sel.is_empty() {
                let (a, b) = d.word_at(sel.cursor);
                sel = Selection { anchor: a, cursor: b };
            }
            if !sel.is_empty() {
                d.format_range(sel, &chars);
            }
            let (a, z) = (sel.start().para, sel.end().para);
            let paras = d.paragraphs;
            self.change_object(WHAT, id, |o| {
                if let Some(t) = super::typing::body_mut(o, cell) {
                    t.paras = paras;
                    (a..=z).for_each(|i| para(t, i));
                }
            });
        } else {
            self.change_selected(WHAT, &|o| {
                o.text_bodies_mut(&mut |t| {
                    t.format_all(&chars);
                    (0..t.paras.len()).for_each(|i| para(t, i));
                    true
                });
            });
        }
        if !b.sticky {
            self.brush = None;
        }
    }

    /// The button: a click turns the painter on (off when on), a double
    /// click on until turned off.
    pub(super) fn brush_button(&mut self, clicked: bool, double: bool) {
        if double {
            self.start_brush(true);
        } else if clicked {
            if self.brush.is_some() {
                self.brush = None;
            } else {
                self.start_brush(false);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn brush_on(&self) -> Option<bool> {
        self.brush.as_ref().map(|b| b.sticky)
    }

    /// Where object `id` showed on the screen last.
    #[cfg(test)]
    pub(crate) fn screen_rect(&self, id: u32) -> Option<eframe::egui::Rect> {
        let ch = objects::chain(&self.pres.deck, self.target())?;
        let o = self.cur_objects().iter().find(|o| o.id == id)?;
        Some(self.canvas_xf?.rect(&ch.frame(o)))
    }
}
