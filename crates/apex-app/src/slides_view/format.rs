//! Formatting text and shapes: the characters selected while typing (or
//! all the text of the selected objects), their paragraphs, and the
//! shapes' fill and outline.

use apex_doc::{Align, Attrs, Selection};
use apex_slides::text::Bullet;
use apex_slides::{Fill, ObjKind, SColor, TextBody};

use super::SlidesView;
use super::objects;

/// Font sizes grow and shrink through these, as in Office.
const SIZES: [f32; 22] = [8.0, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 40.0, 44.0, 48.0, 54.0, 60.0, 66.0, 72.0, 80.0];

/// The next size up or down from `s`.
pub fn step_size(s: f32, up: bool) -> f32 {
    if up {
        SIZES.iter().copied().find(|&x| x > s + 0.01).unwrap_or(s + 10.0)
    } else {
        SIZES.iter().rev().copied().find(|&x| x < s - 0.01).unwrap_or((s - 1.0).max(1.0))
    }
}

/// The attributes of the run at offset `off` of paragraph `para`.
pub(super) fn run_attrs(t: &TextBody, para: usize, off: usize) -> Attrs {
    t.paras
        .get(para)
        .and_then(|p| {
            let mut at = 0;
            p.runs.iter().find(|r| {
                at += r.len;
                at > off || at == p.text.len()
            })
        })
        .map(|r| r.attrs.clone())
        .unwrap_or_default()
}

/// What the text format commands show: the first selected characters'
/// attributes, their size (points) and paragraph alignment and bullet.
#[derive(Clone, Default)]
pub struct Current {
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<u32>,
    pub font: String,
    pub align: Align,
    pub bullet: bool,
    /// Artistic text effects.
    pub fx: Option<std::sync::Arc<apex_doc::TextFx>>,
}

impl SlidesView {
    /// The format at the caret, or of the first selected object's text.
    pub fn current_format(&self) -> Current {
        let (id, para, off) = match &self.typing {
            Some(t) => (t.id, t.sel.start().para, t.sel.start().offset),
            None => match self.selection().first() {
                Some(id) => (*id, 0, 0),
                None => return Current { size: 18.0, ..Default::default() },
            },
        };
        let Some(o) = self.cur_objects().iter().find(|o| o.id == id) else { return Current::default() };
        let Some(ch) = objects::chain(&self.pres.deck, self.target()) else { return Current::default() };
        let empty = TextBody::plain("");
        let t = o.text().unwrap_or(&empty);
        let pp = ch.para(o, t, para);
        let attrs = run_attrs(t, para, off);
        let f = ch.run(o, &pp, &attrs);
        Current {
            size: f.size,
            bold: f.bold,
            italic: f.italic,
            underline: f.underline,
            strike: f.strike,
            color: attrs.color,
            font: f.font.to_string(),
            align: pp.align.unwrap_or(Align::Left),
            bullet: matches!(pp.bullet, Some(Bullet::Char(_) | Bullet::Auto(..))),
            fx: attrs.text_fx.clone(),
        }
    }

    /// Formats the selected characters (the word at the caret with none)
    /// or all the text of the selected objects.
    pub fn format_text(&mut self, what: &'static str, f: &dyn Fn(&mut Attrs)) {
        if let Some(t) = &self.typing {
            let (id, cell) = (t.id, t.cell);
            let mut sel = t.sel;
            let Some(text) = self.typing_text().cloned() else { return };
            let mut d = apex_doc::Document::new();
            d.paragraphs = text.paras;
            if sel.is_empty() {
                let (a, b) = d.word_at(sel.cursor);
                sel = Selection { anchor: a, cursor: b };
            }
            if sel.is_empty() {
                return;
            }
            d.format_range(sel, f);
            let paras = d.paragraphs;
            self.change_object(what, id, |o| {
                if let Some(t) = super::typing::body_mut(o, cell) {
                    t.paras = paras;
                }
            });
            return;
        }
        self.change_selected(what, &|o| {
            o.text_bodies_mut(&mut |t| {
                t.format_all(f);
                true
            });
        });
    }

    /// Changes the selected paragraphs (all of the selected objects'
    /// text when not typing) with `f`, given the body and the index.
    pub fn change_paras(&mut self, what: &'static str, f: &dyn Fn(&mut TextBody, usize)) {
        if let Some(t) = &self.typing {
            let (id, cell, a, b) = (t.id, t.cell, t.sel.start().para, t.sel.end().para);
            self.change_object(what, id, |o| {
                if let Some(t) = super::typing::body_mut(o, cell) {
                    for i in a..=b.min(t.paras.len().saturating_sub(1)) {
                        f(t, i);
                    }
                }
            });
            return;
        }
        self.change_selected(what, &|o| {
            o.text_bodies_mut(&mut |t| {
                for i in 0..t.paras.len() {
                    f(t, i);
                }
                true
            });
        });
    }

    pub fn toggle_bold(&mut self) {
        let on = !self.current_format().bold;
        self.format_text("slides.undo.format", &move |a| a.bold = on);
    }

    pub fn toggle_italic(&mut self) {
        let on = !self.current_format().italic;
        self.format_text("slides.undo.format", &move |a| a.italic = on);
    }

    pub fn toggle_underline(&mut self) {
        let on = !self.current_format().underline;
        self.format_text("slides.undo.format", &move |a| a.underline = on);
    }

    pub fn toggle_strike(&mut self) {
        let on = !self.current_format().strike;
        self.format_text("slides.undo.format", &move |a| a.strike = on);
    }

    /// Sets the size (points).
    pub fn set_font_size(&mut self, pt: f32) {
        let hp = (pt * 2.0).round().clamp(2.0, 8000.0) as u16;
        self.format_text("slides.undo.font_size", &move |a| a.size = Some(hp));
    }

    /// The next size up or down from the current one.
    pub fn grow_font(&mut self, up: bool) {
        let s = self.current_format().size;
        self.set_font_size(step_size(s, up));
    }

    pub fn set_font(&mut self, name: Option<std::sync::Arc<str>>) {
        self.format_text("slides.undo.font", &move |a| a.font = name.clone());
    }

    pub fn set_text_color(&mut self, c: Option<u32>) {
        self.format_text("slides.undo.format", &move |a| a.color = c);
    }

    /// Sets (None: clears) the artistic text of the selected text.
    pub fn set_text_fx(&mut self, fx: Option<apex_doc::TextFx>) {
        let fx = fx.map(std::sync::Arc::new);
        self.format_text("slides.undo.format", &move |a| a.text_fx = fx.clone());
    }

    /// A text box in the middle of the slide with sample text in style
    /// `fx`, or the style on the selected text.
    pub fn text_style_picked(&mut self, fx: apex_doc::TextFx) {
        if self.typing.is_some() || !self.selection().is_empty() {
            return self.set_text_fx(Some(fx));
        }
        let frame = self.centered(self.pres.width * 0.6, 80.0);
        let mut o = objects::new_text_box(0, frame);
        if let Some(t) = o.text_or_new() {
            *t = TextBody::plain(crate::i18n::tr("art.sample"));
            t.body.wrap = Some(true);
            t.body.autofit = Some(apex_slides::text::Autofit::Resize);
            let fx = std::sync::Arc::new(fx);
            t.format_all(&move |a| {
                a.size = Some(72);
                a.text_fx = Some(fx.clone());
            });
            t.edit_props(0, |p| p.align = Some(Align::Center));
        }
        self.insert_object("slides.undo.insert_text_box", o);
    }

    pub fn set_align(&mut self, al: Align) {
        self.change_paras("slides.undo.align_text", &move |t, i| t.edit_props(i, |p| p.align = Some(al)));
    }

    /// Bullets on or off.
    pub fn toggle_bullets(&mut self) {
        let on = !self.current_format().bullet;
        self.change_paras("slides.undo.bullets", &move |t, i| t.edit_props(i, |p| p.bullet = Some(if on { Bullet::Char('•') } else { Bullet::None })));
    }

    /// Numbering on.
    pub fn numbering(&mut self) {
        self.change_paras("slides.undo.bullets", &|t, i| t.edit_props(i, |p| p.bullet = Some(Bullet::Auto(std::sync::Arc::from("arabicPeriod"), 1))));
    }

    /// Paragraph levels up (`1`) or down (`-1`).
    pub fn indent(&mut self, d: i8) {
        self.change_paras("slides.undo.indent", &move |t, i| {
            let p = &mut t.paras[i].style;
            p.level = (p.level as i8 + d).clamp(0, 8) as u8;
        });
    }

    /// The selected shapes' fill: `None` their style's, else a color or
    /// none (`Some(None)`).
    pub fn set_fill(&mut self, c: Option<Option<u32>>) {
        self.change_selected("slides.undo.fill", &move |o| {
            if matches!(o.kind, ObjKind::Shape { .. } | ObjKind::Picture(_)) {
                o.sp.fill = c.map(|c| c.map_or(Fill::None, |c| Fill::Solid(SColor::rgb(c))));
            }
        });
    }

    /// The selected shapes' outline color (as `set_fill`).
    pub fn set_line_color(&mut self, c: Option<Option<u32>>) {
        self.change_selected("slides.undo.line", &move |o| {
            let mut line = o.sp.line.clone().unwrap_or_default();
            line.fill = c.map(|c| c.map_or(Fill::None, |c| Fill::Solid(SColor::rgb(c))));
            if line.fill.is_some() && line.width.is_none() && c.is_some_and(|c| c.is_some()) {
                line.width = Some(1.0);
            }
            o.sp.line = Some(line);
        });
    }

    /// The selected shapes' outline width, points.
    pub fn set_outline_width(&mut self, w: f32) {
        self.change_selected("slides.undo.line", &move |o| {
            let mut line = o.sp.line.clone().unwrap_or_default();
            line.width = Some(w);
            o.sp.line = Some(line);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_slides::Frame;

    #[test]
    fn sizes_step() {
        assert_eq!(step_size(18.0, true), 20.0);
        assert_eq!(step_size(18.0, false), 16.0);
        assert_eq!(step_size(80.0, true), 90.0);
        assert_eq!(step_size(8.0, false), 7.0);
    }

    #[test]
    fn formats_objects_and_typed_text() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let id = v.insert_text_box(Frame::new(0.0, 0.0, 200.0, 40.0)).unwrap();
        v.type_text("one two");
        // The word at the caret.
        v.toggle_bold();
        let t = v.typing_text().unwrap().clone();
        assert!(t.paras[0].runs.iter().any(|r| r.attrs.bold) && t.paras[0].runs.iter().any(|r| !r.attrs.bold));
        v.stop_typing();
        v.sel = vec![id];
        v.set_font_size(30.0);
        assert_eq!(v.current_format().size, 30.0);
        v.grow_font(true);
        assert_eq!(v.current_format().size, 32.0);
        v.set_align(Align::Center);
        assert_eq!(v.current_format().align, Align::Center);
        v.toggle_bullets();
        assert!(v.current_format().bullet);
        v.indent(1);
        assert_eq!(v.selected_object().unwrap().text().unwrap().paras[0].style.level, 1);
        v.set_text_color(Some(0xFF0000));
        assert_eq!(v.current_format().color, Some(0xFF0000));
        v.set_fill(Some(Some(0x00FF00)));
        v.set_line_color(Some(None));
        let o = v.selected_object().unwrap();
        assert_eq!(o.sp.fill, Some(Fill::Solid(SColor::rgb(0x00FF00))));
        assert_eq!(o.sp.line.as_ref().unwrap().fill, Some(Fill::None));
    }
}
