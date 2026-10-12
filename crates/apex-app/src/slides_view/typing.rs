//! Typing in a shape's text: the caret and selection, the keys, and the
//! changes, made by `apex_doc`'s editing engine on the text's paragraphs
//! (a text body's paragraphs are a document's). Typing one run of
//! characters is one undo step.

use apex_doc::{Document, LINE_BREAK, Pos, Selection};
use apex_slides::{ObjKind, Object, TextBody};
use eframe::egui::{self, Color32, Key, Modifiers, Painter, Pos2, Stroke, epaint::text::cursor::CCursor, vec2};

use super::SlidesView;
use super::objects::{self, Target, objs_mut};
use super::text::Placed;

/// Undo steps typing joins (per slide and object).
const TYPING_KEY: u64 = 0x7E_17E0;

/// The text being typed.
pub struct Typing {
    /// The object, on the current slide.
    pub id: u32,
    pub sel: Selection,
    /// Where the text showed when last painted (clicks, up and down).
    pub placed: Option<Placed>,
    /// The caret's x kept going up and down (text units).
    h_pos: Option<f32>,
    /// A click to place the caret once the text is laid out (the word
    /// under it for a double click).
    pub pending: Option<(Pos2, bool)>,
    /// The cell (row, column) typed in, in a table.
    pub cell: Option<(usize, usize)>,
}

/// The text typed in object `o`: its own, or that of table cell `cell`.
pub fn body_of(o: &Object, cell: Option<(usize, usize)>) -> Option<&TextBody> {
    match (cell, &o.kind) {
        (Some((r, c)), ObjKind::Table(t)) => Some(&t.rows.get(r)?.cells.get(c)?.text),
        (Some(_), _) => None,
        (None, _) => o.text(),
    }
}

/// The text typed in object `o` to change (made when a shape has none).
pub fn body_mut(o: &mut Object, cell: Option<(usize, usize)>) -> Option<&mut TextBody> {
    match cell {
        Some((r, c)) => match &mut o.kind {
            ObjKind::Table(t) => Some(&mut t.rows.get_mut(r)?.cells.get_mut(c)?.text),
            _ => None,
        },
        None => o.text_or_new(),
    }
}

/// Char index of byte offset `off` in `s`.
fn char_index(s: &str, off: usize) -> usize {
    s.get(..off.min(s.len())).map_or(0, |p| p.chars().count())
}

/// Byte offset of char index `ci` in `s`.
fn byte_offset(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

/// The paragraphs of a text body as a document to edit.
fn doc_of(t: Option<&TextBody>) -> Document {
    let mut d = Document::new();
    d.paragraphs = t.map(|t| t.paras.clone()).unwrap_or_default();
    if d.paragraphs.is_empty() {
        d.paragraphs.push(apex_doc::Paragraph::new(""));
    }
    d
}

impl Placed {
    /// The caret's box (text units, zero wide) for position `p`.
    fn caret_rect(&self, text: &TextBody, p: Pos) -> Option<egui::Rect> {
        let l = self.laid.lines.get(p.para)?;
        let para = text.paras.get(p.para)?;
        let r = l.galley.pos_from_cursor(CCursor::new(char_index(&para.text, p.offset)));
        Some(r.translate(vec2(l.x, l.y)))
    }

    /// The text position under screen point `s`.
    fn pos_at(&self, text: &TextBody, s: Pos2) -> Pos {
        let q = self.local(s);
        let n = self.laid.lines.len().min(text.paras.len());
        if n == 0 {
            return Pos::new(0, 0);
        }
        // The paragraph whose lines are nearest.
        let mut k = 0;
        for (i, l) in self.laid.lines.iter().enumerate().take(n) {
            if q.y >= l.y {
                k = i;
            }
        }
        let l = &self.laid.lines[k];
        let c = l.galley.cursor_from_pos(q - vec2(l.x, l.y));
        Pos::new(k, byte_offset(&text.paras[k].text, c.index.0))
    }
}

impl SlidesView {
    /// Starts typing in object `id` of the current slide, the caret at its end.
    pub fn start_typing(&mut self, id: u32) {
        let Some(o) = self.cur_objects().iter().find(|o| o.id == id) else { return };
        if self.read_only || !can_type(o) {
            return;
        }
        let d = doc_of(o.text());
        let sel = Selection::caret(d.end());
        self.sel = vec![id];
        self.typing = Some(Typing { id, sel, placed: None, h_pos: None, pending: None, cell: None });
    }

    /// Starts typing in cell (`r`, `c`) of table `id`, the caret at its end.
    pub fn start_typing_cell(&mut self, id: u32, r: usize, c: usize) {
        let Some(o) = self.cur_objects().iter().find(|o| o.id == id) else { return };
        let Some(body) = body_of(o, Some((r, c))) else { return };
        if self.read_only {
            return;
        }
        let sel = Selection::caret(doc_of(Some(body)).end());
        self.stop_typing();
        self.sel = vec![id];
        self.typing = Some(Typing { id, sel, placed: None, h_pos: None, pending: None, cell: Some((r, c)) });
    }

    /// Moves typing to the next (`1`) or previous (`-1`) cell of the table,
    /// row after row; false at either end.
    fn next_cell(&mut self, dir: isize) -> bool {
        let Some(t) = &self.typing else { return false };
        let (id, Some((r, c))) = (t.id, t.cell) else { return false };
        let Some(o) = self.cur_objects().iter().find(|o| o.id == id) else { return false };
        let ObjKind::Table(tb) = &o.kind else { return false };
        let cols = tb.cols.len();
        let cells: Vec<(usize, usize)> = (0..tb.rows.len()).flat_map(|r| (0..cols).map(move |c| (r, c))).filter(|&(r, c)| tb.rows[r].cells.get(c).is_some_and(|x| !x.covered())).collect();
        let Some(k) = cells.iter().position(|&x| x == (r, c)) else { return false };
        let Some(&(r, c)) = k.checked_add_signed(dir).and_then(|k| cells.get(k)) else { return false };
        self.start_typing_cell(id, r, c);
        if let Some(t) = &mut self.typing {
            // The whole text of the cell gone to, as in other programs.
            let end = t.sel.cursor;
            t.sel = Selection { anchor: Pos::new(0, 0), cursor: end };
        }
        true
    }

    pub fn stop_typing(&mut self) {
        if self.typing.take().is_some() {
            self.pres.seal();
        }
    }

    /// The text of the object being typed in.
    pub fn typing_text(&self) -> Option<&TextBody> {
        let t = self.typing.as_ref()?;
        body_of(self.cur_objects().iter().find(|o| o.id == t.id)?, t.cell)
    }

    /// The document being typed in: the text's paragraphs.
    fn typing_doc(&self) -> Option<Document> {
        let t = self.typing.as_ref()?;
        let o = self.cur_objects().iter().find(|o| o.id == t.id)?;
        Some(doc_of(body_of(o, t.cell)))
    }

    /// Changes the text being typed with `f` (given the selection, it
    /// returns the new one); `merge`: typing joins the last step.
    fn edit_text(&mut self, what: &'static str, merge: bool, f: impl FnOnce(&mut Document, Selection) -> Selection) {
        if self.read_only {
            return;
        }
        let Some(mut d) = self.typing_doc() else { return };
        let Some(t) = &self.typing else { return };
        let (id, sel, cell) = (t.id, t.sel, t.cell);
        let before = d.paragraphs.clone();
        let sel = f(&mut d, sel);
        if d.paragraphs != before {
            let target = self.target();
            let place = match target {
                Target::Slide(i) => self.pres.slides[i].id as u64,
                Target::Layout(m, l) => 0x8000_0000 | (m as u64) << 16 | l as u64,
                Target::Master(m) => 0x4000_0000 | m as u64,
            };
            let key = TYPING_KEY ^ (place << 24) ^ id as u64;
            let deck = if merge { self.pres.edit_merge(what, key) } else { self.pres.edit(what) };
            if let Some(o) = apex_slides::edit::find_mut(objs_mut(deck, target), id)
                && let Some(t) = body_mut(o, cell)
            {
                t.paras = d.paragraphs;
                o.touch();
            }
            if !merge {
                self.pres.seal();
            }
        }
        if let Some(t) = &mut self.typing {
            t.sel = sel;
            t.h_pos = None;
        }
    }

    /// Types `s` over the selection.
    pub fn type_text(&mut self, s: &str) {
        let single = s.chars().count() == 1 && !s.contains('\n');
        self.edit_text("slides.undo.typing", single, |d, sel| Selection::caret(d.insert_text(sel, s, None)));
    }

    /// Keeps the selected text, copied as `text`, with its look: all of
    /// it on the text itself (what the placeholder and the master give it
    /// too), so it shows the same in another text or presentation, or in
    /// a document. Colors left to the theme stay so: they follow the
    /// place the text goes to.
    fn keep_copied_text(&self, text: &str) {
        let (Some(t), Some(d)) = (&self.typing, self.typing_doc()) else { return };
        let (a, b) = (t.sel.start(), t.sel.end());
        let Some(o) = self.cur_objects().iter().find(|o| o.id == t.id) else { return };
        let Some(ch) = objects::chain(&self.pres.deck, self.target()) else { return };
        let empty = TextBody::plain("");
        let body = body_of(o, t.cell).unwrap_or(&empty);
        let mut paras = d.slice(a, b);
        for (k, p) in paras.iter_mut().enumerate() {
            let pp = ch.para(o, body, a.para + k);
            for r in &mut p.runs {
                let f = ch.run(o, &pp, &r.attrs);
                r.attrs.size = Some((f.size * 2.0).round() as u16);
                (r.attrs.bold, r.attrs.italic, r.attrs.underline, r.attrs.strike) = (f.bold, f.italic, f.underline, f.strike);
                r.attrs.font = Some(f.font);
            }
            // Paragraph properties are numbers in their own text's list.
            p.style = apex_doc::ParaStyle { level: p.style.level, ..Default::default() };
        }
        crate::clip_history::push(crate::clip_history::Entry { text: text.to_string(), rich: crate::clip_history::Rich::Runs(std::sync::Arc::new(paras)) });
    }

    /// Pastes the clipboard's `text` at the caret: with its look when it
    /// was copied on a slide or in a document of ours, else as typed.
    pub fn paste_text(&mut self, text: &str) {
        use crate::clip_history::Rich;
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let paras = match crate::clip_history::find(&text).map(|e| e.rich) {
            Some(Rich::Runs(p)) => p.to_vec(),
            Some(Rich::Doc(p, origin)) => origin.resolved(&p),
            _ => Vec::new(),
        };
        if paras.iter().all(|p| p.text.is_empty()) {
            self.type_text(&text);
            return;
        }
        self.edit_text("slides.undo.paste", false, |d, sel| {
            // The paragraphs take the formatting of the one they go into
            // (its bullets, its alignment), at their own level.
            let here = d.paragraphs[sel.start().para.min(d.paragraphs.len() - 1)].style;
            let paras: Vec<apex_doc::Paragraph> = paras
                .into_iter()
                .map(|mut p| {
                    p.style = apex_doc::ParaStyle { level: p.style.level.max(here.level), ..here };
                    p.cell = None;
                    p
                })
                .collect();
            Selection::caret(d.insert_paragraphs(sel, &paras))
        });
    }

    /// Moves the caret (with `extend`, the selection's end) to `p`.
    fn caret_to(&mut self, p: Pos, extend: bool) {
        if let Some(t) = &mut self.typing {
            t.sel = if extend { Selection { anchor: t.sel.anchor, cursor: p } } else { Selection::caret(p) };
        }
    }

    /// The selected text.
    pub fn typing_selected(&self) -> Option<String> {
        let d = self.typing_doc()?;
        let s = self.typing.as_ref()?.sel;
        (!s.is_empty()).then(|| d.text_range(s.start(), s.end()).replace(LINE_BREAK, "\n"))
    }

    /// Paragraph levels of the selection up (`1`) or down (`-1`).
    pub fn indent_typing(&mut self, d: i8) {
        self.edit_text("slides.undo.indent", false, |doc, sel| {
            for p in &mut doc.paragraphs[sel.start().para..=sel.end().para] {
                p.style.level = (p.style.level as i8 + d).clamp(0, 8) as u8;
            }
            sel
        });
    }

    /// Bold, italic, underline or strike over the selection.
    pub fn toggle_typing(&mut self, get: fn(&apex_doc::Attrs) -> bool, set: fn(&mut apex_doc::Attrs, bool)) {
        self.edit_text("slides.undo.format", false, |doc, sel| {
            doc.toggle(sel, get, set);
            sel
        });
    }

    /// A key typed while typing (the rest go elsewhere): true if used.
    pub fn typing_key(&mut self, key: Key, m: Modifiers) -> bool {
        let Some(d) = self.typing_doc() else { return false };
        let Some(t) = &self.typing else { return false };
        let sel = t.sel;
        let (word, ext) = (m.command || m.alt, m.shift);
        let text = self.typing_text().cloned().unwrap_or_default();
        match key {
            Key::Escape => {
                self.stop_typing();
            }
            Key::Backspace => self.edit_text("slides.undo.typing", sel.is_empty() && !word, |d, s| Selection::caret(d.backspace(s, word))),
            Key::Delete => self.edit_text("slides.undo.typing", false, |d, s| Selection::caret(d.delete_forward(s, word))),
            Key::Enter if m.shift => self.type_text(&LINE_BREAK.to_string()),
            Key::Enter => self.edit_text("slides.undo.new_paragraph", false, |d, s| Selection::caret(d.new_paragraph(s))),
            Key::Tab if self.typing.as_ref().is_some_and(|t| t.cell.is_some()) => {
                // In a table Tab goes from cell to cell.
                self.next_cell(if m.shift { -1 } else { 1 });
            }
            Key::Tab => {
                // At a paragraph's start (or over paragraphs) Tab demotes it.
                if sel.start().offset == 0 || sel.start().para != sel.end().para {
                    self.indent_typing(if m.shift { -1 } else { 1 });
                } else if !m.shift {
                    self.type_text("\t");
                }
            }
            Key::A if m.command => {
                if let Some(t) = &mut self.typing {
                    t.sel = Selection { anchor: Pos::new(0, 0), cursor: d.end() };
                }
            }
            Key::B if m.command => self.toggle_typing(|a| a.bold, |a, v| a.bold = v),
            Key::I if m.command => self.toggle_typing(|a| a.italic, |a, v| a.italic = v),
            Key::U if m.command => self.toggle_typing(|a| a.underline, |a, v| a.underline = v),
            Key::ArrowLeft | Key::ArrowRight => {
                let right = key == Key::ArrowRight;
                let p = if !sel.is_empty() && !ext {
                    if right { sel.end() } else { sel.start() }
                } else if right {
                    if word { d.next_word(sel.cursor) } else { d.next_char(sel.cursor) }
                } else if word {
                    d.prev_word(sel.cursor)
                } else {
                    d.prev_char(sel.cursor)
                };
                self.caret_to(p, ext);
            }
            Key::Home | Key::End => {
                let end = key == Key::End;
                let p = if m.command {
                    if end { d.end() } else { Pos::new(0, 0) }
                } else {
                    self.row_end(&text, sel.cursor, end).unwrap_or_else(|| Pos::new(sel.cursor.para, if end { d.paragraphs[sel.cursor.para].text.len() } else { 0 }))
                };
                self.caret_to(p, ext);
            }
            Key::ArrowUp | Key::ArrowDown => {
                let down = key == Key::ArrowDown;
                let p = self.vertical(&text, sel.cursor, down).unwrap_or_else(|| {
                    let k = if down { (sel.cursor.para + 1).min(d.paragraphs.len() - 1) } else { sel.cursor.para.saturating_sub(1) };
                    d.clamp(Pos::new(k, sel.cursor.offset))
                });
                let h = self.typing.as_ref().and_then(|t| t.h_pos);
                self.caret_to(p, ext);
                if let Some(t) = &mut self.typing {
                    t.h_pos = h.or(t.h_pos);
                }
            }
            _ => return false,
        }
        true
    }

    /// The start or end of the row the caret is on.
    fn row_end(&self, text: &TextBody, p: Pos, end: bool) -> Option<Pos> {
        let placed = self.typing.as_ref()?.placed.as_ref()?;
        let l = placed.laid.lines.get(p.para)?;
        let s = &text.paras.get(p.para)?.text;
        let c = CCursor::new(char_index(s, p.offset));
        let c = if end { l.galley.cursor_end_of_row(&c) } else { l.galley.cursor_begin_of_row(&c) };
        Some(Pos::new(p.para, byte_offset(s, c.index.0)))
    }

    /// The position a row up or down from `p`, at the same x.
    fn vertical(&mut self, text: &TextBody, p: Pos, down: bool) -> Option<Pos> {
        let t = self.typing.as_mut()?;
        let placed = t.placed.as_ref()?;
        let r = placed.caret_rect(text, p)?;
        let x = *t.h_pos.get_or_insert(r.center().x);
        let lines = &placed.laid.lines;
        let l = lines.get(p.para)?;
        // Rows of the paragraph, then the next paragraph's first or the
        // previous one's last row.
        let rows: Vec<egui::Rect> = l.galley.rows.iter().map(|row| row.rect().translate(vec2(l.x, l.y))).collect();
        let at = rows.iter().position(|row| r.center().y >= row.min.y && r.center().y <= row.max.y).unwrap_or(0);
        let (k, y) = if down {
            if at + 1 < rows.len() {
                (p.para, rows[at + 1].center().y)
            } else {
                let n = lines.get(p.para + 1)?;
                (p.para + 1, n.y + n.galley.rows.first()?.rect().center().y)
            }
        } else if at > 0 {
            (p.para, rows[at - 1].center().y)
        } else {
            let k = p.para.checked_sub(1)?;
            let n = &lines[k];
            (k, n.y + n.galley.rows.last()?.rect().center().y)
        };
        let n = &lines[k];
        let c = n.galley.cursor_from_pos(vec2(x - n.x, y - n.y));
        Some(Pos::new(k, byte_offset(&text.paras.get(k)?.text, c.index.0)))
    }

    /// A click (or drag with `extend`) at screen point `s` in the text.
    pub fn typing_click(&mut self, s: Pos2, extend: bool) {
        let text = self.typing_text().cloned().unwrap_or_else(|| TextBody::plain(""));
        let Some(p) = self.typing.as_ref().and_then(|t| t.placed.as_ref()).map(|pl| pl.pos_at(&text, s)) else { return };
        self.caret_to(p, extend);
    }

    /// Selects the word at screen point `s`.
    pub fn typing_word(&mut self, s: Pos2) {
        self.typing_click(s, false);
        let Some(d) = self.typing_doc() else { return };
        if let Some(t) = &mut self.typing {
            let (a, b) = d.word_at(t.sel.cursor);
            t.sel = Selection { anchor: a, cursor: b };
        }
    }

    /// The keys and text typed this frame: used ones are taken out.
    pub(super) fn typing_input(&mut self, ctx: &egui::Context) {
        if self.typing.is_none() {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        let mut used_keys: Vec<(Key, Modifiers)> = Vec::new();
        let mut used_text = false;
        for e in events {
            match e {
                egui::Event::Text(s) if !s.chars().all(char::is_control) => {
                    self.type_text(&s);
                    used_text = true;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(s)) if !s.is_empty() => {
                    self.type_text(&s);
                    used_text = true;
                }
                egui::Event::Copy | egui::Event::Cut => {
                    if let Some(s) = self.typing_selected() {
                        self.keep_copied_text(&s);
                        ctx.copy_text(s);
                        if matches!(e, egui::Event::Cut) {
                            self.edit_text("slides.undo.cut", false, |d, sel| Selection::caret(d.delete(sel)));
                        }
                    }
                    used_text = true;
                }
                egui::Event::Paste(s) => {
                    self.paste_text(&s);
                    used_text = true;
                }
                // Undo and redo stay the view's.
                egui::Event::Key { key, pressed: true, modifiers, .. } if !(modifiers.command && matches!(key, Key::Z | Key::Y)) => {
                    if self.typing.is_some() && self.typing_key(key, modifiers) {
                        used_keys.push((key, modifiers));
                    }
                }
                _ => {}
            }
        }
        if used_text || !used_keys.is_empty() {
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Text(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) | egui::Event::Ime(_) => false,
                    egui::Event::Key { key, modifiers, .. } => !used_keys.iter().any(|(k, m)| k == key && m == modifiers),
                    _ => true,
                })
            });
        }
    }

    /// Paints the selection and the caret over the text being typed.
    pub(super) fn paint_caret(&self, p: &Painter, time: f64) {
        let Some(t) = &self.typing else { return };
        let Some(pl) = &t.placed else { return };
        let text = self.typing_text().cloned().unwrap_or_else(|| TextBody::plain(""));
        let sel = t.sel;
        if !sel.is_empty() {
            let (a, b) = (sel.start(), sel.end());
            for k in a.para..=b.para {
                let (Some(l), Some(para)) = (pl.laid.lines.get(k), text.paras.get(k)) else { continue };
                let from = if k == a.para { char_index(&para.text, a.offset) } else { 0 };
                let to = if k == b.para { char_index(&para.text, b.offset) } else { para.text.chars().count() };
                let mut start = 0;
                for row in &l.galley.rows {
                    let n = row.char_count_including_newline().0;
                    let (s0, s1) = (from.max(start), to.min(start + row.char_count_excluding_newline().0));
                    if s0 < s1 || (s0 == s1 && k != b.para && s1 == start + row.char_count_excluding_newline().0 && from <= s0) {
                        let x0 = l.galley.pos_from_cursor(CCursor::new(s0)).min.x;
                        let x1 = l.galley.pos_from_cursor(CCursor::new(s1)).min.x.max(x0 + 3.0);
                        let rr = row.rect();
                        let q = [vec2(x0, rr.min.y), vec2(x1, rr.min.y), vec2(x1, rr.max.y), vec2(x0, rr.max.y)].map(|v| pl.screen(v + vec2(l.x, l.y)));
                        p.add(egui::Shape::convex_polygon(q.to_vec(), Color32::from_rgba_unmultiplied(80, 140, 230, 90), Stroke::NONE));
                    }
                    start += n;
                }
            }
        }
        // The caret blinks.
        if (time * 2.0) as i64 % 2 == 0
            && let Some(r) = pl.caret_rect(&text, sel.cursor)
        {
            p.line_segment([pl.screen(r.min.to_vec2()), pl.screen(vec2(r.min.x, r.max.y))], Stroke::new(1.5, Color32::BLACK));
        }
    }
}

/// Whether the text of `o` can be typed in (shapes, not lines).
pub fn can_type(o: &Object) -> bool {
    let line = o.sp.geom.as_ref().is_some_and(apex_slides::Geometry::is_line);
    matches!(o.kind, ObjKind::Shape { connector: false, .. }) && !line
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_slides::Frame;

    fn typing_view() -> (SlidesView, u32) {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let id = v.insert_text_box(Frame::new(10.0, 10.0, 200.0, 40.0)).unwrap();
        (v, id)
    }

    fn text(v: &SlidesView) -> String {
        v.typing_text().map(|t| t.text()).unwrap_or_default()
    }

    #[test]
    fn text_is_copied_and_pasted_with_its_look() {
        use crate::clip_history::{Entry, Rich};
        let (mut v, _) = typing_view();
        v.type_text("Hello world");
        v.typing.as_mut().unwrap().sel = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 5) };
        v.toggle_typing(|a| a.bold, |a, on| a.bold = on);
        v.typing.as_mut().unwrap().sel = Selection { anchor: Pos::new(0, 3), cursor: Pos::new(0, 8) };
        v.keep_copied_text("lo wo");

        // In another presentation.
        let (mut w, _) = typing_view();
        w.type_text("x ");
        w.paste_text("lo wo");
        let t = w.typing_text().unwrap();
        assert_eq!(t.text(), "x lo wo");
        let look = |i: usize| t.paras[0].attrs_at(i);
        assert!(!look(1).bold && look(4).bold && !look(7).bold);
        assert!(look(4).size.is_some() && look(4).font.is_some());

        // Text of a document comes with the look its styles gave it.
        let mut d = Document::new();
        d.paragraphs = vec![apex_doc::Paragraph::new("Big title").with_style(apex_doc::ParaStyle { kind: apex_doc::ParaKind::Heading(1), ..Default::default() }), apex_doc::Paragraph::new("body")];
        let size = d.char_format(&d.paragraphs[0].style, &Default::default()).size;
        crate::clip_history::push(Entry { text: "Big title\nbody".into(), rich: Rich::Doc(d.paragraphs.clone().into(), d.origin(&d.paragraphs).into()) });
        w.paste_text("Big title\r\nbody");
        let t = w.typing_text().unwrap();
        assert_eq!(t.text(), "x lo woBig title\nbody");
        assert_eq!(t.paras[0].attrs_at(12).size, Some(size));
        assert_eq!(t.paras[1].style, t.paras[0].style);
        // Anything else is typed.
        w.paste_text("!");
        assert_eq!(w.typing_text().unwrap().text(), "x lo woBig title\nbody!");
    }

    #[test]
    fn typing_joins_one_step() {
        let (mut v, _) = typing_view();
        let steps = v.pres.undo_names().len();
        for c in ["H", "e", "l", "l", "o"] {
            v.type_text(c);
        }
        assert_eq!(text(&v), "Hello");
        assert_eq!(v.pres.undo_names().len(), steps + 1);
        v.typing_key(Key::Enter, Modifiers::NONE);
        v.type_text("W");
        assert_eq!(text(&v), "Hello\nW");
        v.typing_key(Key::Backspace, Modifiers::NONE);
        v.typing_key(Key::Backspace, Modifiers::NONE);
        assert_eq!(text(&v), "Hello");
        // "W" and the deletes are one step, the new paragraph another.
        v.undo();
        v.undo();
        assert_eq!(text(&v), "Hello");
        v.undo();
        assert_eq!(text(&v), "");
    }

    #[test]
    fn keys_move_select_and_format() {
        let (mut v, _) = typing_view();
        v.type_text("one two");
        v.typing_key(Key::ArrowLeft, Modifiers::COMMAND | Modifiers::SHIFT);
        assert_eq!(v.typing_selected().as_deref(), Some("two"));
        v.typing_key(Key::B, Modifiers::COMMAND);
        let t = v.typing_text().unwrap();
        assert!(t.paras[0].runs.iter().any(|r| r.attrs.bold));
        v.typing_key(Key::Home, Modifiers::NONE);
        v.type_text("x");
        assert_eq!(text(&v), "xone two");
        v.typing_key(Key::A, Modifiers::COMMAND);
        v.type_text("new");
        assert_eq!(text(&v), "new");
        // Tab at a paragraph's start demotes it.
        v.typing_key(Key::Home, Modifiers::NONE);
        v.typing_key(Key::Tab, Modifiers::NONE);
        assert_eq!(v.typing_text().unwrap().paras[0].style.level, 1);
        v.typing_key(Key::Tab, Modifiers::SHIFT);
        assert_eq!(v.typing_text().unwrap().paras[0].style.level, 0);
        v.typing_key(Key::Enter, Modifiers::SHIFT);
        assert_eq!(v.typing_text().unwrap().paras.len(), 1);
        v.typing_key(Key::Escape, Modifiers::NONE);
        assert!(v.typing.is_none());
    }

    #[test]
    fn table_cells_are_typed_in_and_tab_goes_through_them() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let id = v.insert_table(2, 2).unwrap();
        v.start_typing_cell(id, 0, 0);
        v.type_text("Name");
        v.typing_key(Key::Tab, Modifiers::NONE);
        assert_eq!(v.typing.as_ref().unwrap().cell, Some((0, 1)));
        v.type_text("Age");
        v.typing_key(Key::Tab, Modifiers::SHIFT);
        assert_eq!(v.typing_selected().as_deref(), Some("Name"));
        let cell = |v: &SlidesView, r: usize, c: usize| match &v.object(id).unwrap().kind {
            ObjKind::Table(t) => t.rows[r].cells[c].text.text(),
            _ => String::new(),
        };
        assert_eq!((cell(&v, 0, 0), cell(&v, 0, 1)), ("Name".to_string(), "Age".to_string()));
        // One undo step takes the typing back.
        v.stop_typing();
        v.undo();
        assert_eq!(cell(&v, 0, 1), "");
        // The table's columns and rows take its new size.
        let f = v.frames_of(&[id])[0].1;
        v.set_frames("move", &[(id, Frame { w: f.w * 2.0, h: f.h * 3.0, ..f })], false);
        let ObjKind::Table(t) = &v.object(id).unwrap().kind else { panic!() };
        assert!((t.cols.iter().sum::<f32>() - f.w * 2.0).abs() < 0.01);
        assert!((t.rows.iter().map(|r| r.height).sum::<f32>() - f.h * 3.0).abs() < 0.01);
    }

    #[test]
    fn placeholders_take_text() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let title = v.pres.slides[0].objects[0].id;
        v.start_typing(title);
        v.type_text("Title");
        assert_eq!(v.pres.slides[0].title(), "Title");
        v.read_only = true;
        v.type_text("x");
        assert_eq!(v.pres.slides[0].title(), "Title");
    }
}
