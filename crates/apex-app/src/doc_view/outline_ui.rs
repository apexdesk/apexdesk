//! Outline view: the document as its headings and body text, indented by
//! level, to reorganize it: promote and demote, move up and down, expand
//! and collapse, show levels. The selected row is edited in place.

use std::collections::HashSet;

use super::views::ViewMode;
use super::*;

pub(super) struct OutlineState {
    /// Collapsed headings (paragraph indices, for `count` paragraphs).
    collapsed: HashSet<usize>,
    count: usize,
    /// Deepest heading level shown (0: all levels and body text).
    pub show_level: u8,
    pub first_line: bool,
    pub formatting: bool,
    /// Heights of the rows from the last frame, by first paragraph.
    heights: std::collections::HashMap<usize, f32>,
    /// The paragraph being edited, its text as typed and the revision
    /// of the document it matches.
    edit: Option<(usize, String, u64)>,
    /// Put the edit caret here (character index) on the next frame.
    want_cursor: Option<usize>,
    scroll: bool,
}

impl Default for OutlineState {
    fn default() -> Self {
        OutlineState { collapsed: HashSet::new(), count: 0, show_level: 0, first_line: false, formatting: true, heights: Default::default(), edit: None, want_cursor: None, scroll: true }
    }
}

/// A row: one paragraph, or a whole table.
#[derive(Clone, Copy, Debug)]
struct Row {
    start: usize,
    end: usize,
    /// Heading level (None: body text).
    level: Option<u8>,
    /// Indent steps.
    depth: u8,
    /// A heading with text under it.
    has_body: bool,
    table: bool,
}

/// Text of a row as shown.
fn row_text(p: &Paragraph) -> String {
    p.text.chars().filter(|&c| c != apex_doc::OBJECT).map(|c| if c == LINE_BREAK || c == '\t' { ' ' } else { c }).collect()
}

/// Byte offset of character `ci` of `s`.
fn byte_of(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

impl DocView {
    fn heading_level(&self, i: usize) -> Option<u8> {
        let p = &self.doc.paragraphs[i];
        match p.style.kind {
            ParaKind::Heading(l) if p.cell.is_none() && p.style.toc.is_none() => Some(l.clamp(1, 9)),
            _ => None,
        }
    }

    /// Every row of the outline.
    fn outline_rows(&self) -> Vec<Row> {
        let n = self.doc.paragraphs.len();
        let mut rows = Vec::new();
        let mut depth = 0u8;
        let mut i = 0;
        while i < n {
            if self.doc.in_table(i) {
                let mut e = i + 1;
                while e < n && self.doc.in_table(e) {
                    e += 1;
                }
                rows.push(Row { start: i, end: e, level: None, depth, has_body: false, table: true });
                i = e;
                continue;
            }
            let level = self.heading_level(i);
            if let Some(l) = level {
                depth = l;
            }
            let has_body = level.is_some() && self.doc.section_end(i) > i + 1;
            rows.push(Row { start: i, end: i + 1, level, depth: level.map_or(depth, |l| l - 1), has_body, table: false });
            i += 1;
        }
        rows
    }

    /// The rows shown: not under a collapsed heading, and within the
    /// levels shown.
    fn visible_rows(&self) -> Vec<Row> {
        let st = &self.views.outline;
        let mut out = Vec::new();
        let mut hidden_to = 0;
        for r in self.outline_rows() {
            if r.start < hidden_to {
                continue;
            }
            if st.show_level > 0 && r.level.is_none_or(|l| l > st.show_level) {
                continue;
            }
            if r.level.is_some() && r.has_body && st.collapsed.contains(&r.start) {
                hidden_to = self.doc.section_end(r.start);
            }
            out.push(r);
        }
        out
    }

    /// Paragraphs a row stands for when moved: a collapsed heading takes
    /// its section with it.
    fn row_unit_end(&self, r: &Row) -> usize {
        let collapsed = r.has_body && (self.views.outline.collapsed.contains(&r.start) || self.views.outline.show_level > 0);
        if collapsed { self.doc.section_end(r.start) } else { r.end }
    }

    fn outline_sel(&self) -> (usize, usize) {
        (self.sel.start().para, self.sel.end().para)
    }

    /// Changes the level of the selected paragraphs (tables stay).
    pub(super) fn outline_level(&mut self, f: &dyn Fn(Option<u8>, u8) -> Option<u8>) {
        let (a, b) = self.outline_sel();
        // The level of the heading above, for body text promoted.
        let above = (0..a).rev().find_map(|i| self.heading_level(i)).unwrap_or(1);
        let sel = self.sel;
        let mut i = a;
        while i <= b {
            if self.doc.in_table(i) {
                i += 1;
                continue;
            }
            let mut e = i;
            while e < b && !self.doc.in_table(e + 1) {
                e += 1;
            }
            self.doc.set_outline_levels(i, e, sel, &|l| f(l, above));
            i = e + 1;
        }
        self.views.outline.edit = None;
    }

    pub(super) fn outline_promote(&mut self) {
        self.outline_level(&|l, above| Some(l.map_or(above, |l| l.saturating_sub(1).max(1))));
    }

    pub(super) fn outline_demote(&mut self) {
        self.outline_level(&|l, _| l.map(|l| (l + 1).min(9)));
    }

    /// Moves the selected rows above the row before them, or below the
    /// row after them.
    pub(super) fn outline_move(&mut self, down: bool) {
        let Some((from, end, to)) = self.outline_move_span(down) else { return };
        if let Some(s) = self.doc.move_paras(from, end, to, self.sel) {
            // Collapsed headings keep their state where they went.
            let moved: Vec<usize> = self.views.outline.collapsed.iter().copied().filter(|&c| (from..end).contains(&c)).collect();
            let shift = s.anchor.para as isize - from as isize;
            for c in moved {
                self.views.outline.collapsed.remove(&c);
                self.views.outline.collapsed.insert((c as isize + shift) as usize);
            }
            self.views.outline.count = self.doc.paragraphs.len();
            self.sel = Selection { anchor: s.anchor, cursor: Pos::new(s.anchor.para, 0) };
            self.views.outline.edit = None;
            self.views.outline.scroll = true;
        }
    }

    /// The paragraphs the selected rows stand for, and where they go: None
    /// at the top (or the bottom) already.
    fn outline_move_span(&self, down: bool) -> Option<(usize, usize, usize)> {
        let rows = self.visible_rows();
        let (a, b) = self.outline_sel();
        let first = rows.iter().position(|r| r.end > a)?;
        let last = rows.iter().rposition(|r| r.start <= b).unwrap_or(first).max(first);
        let from = rows[first].start;
        let end = self.row_unit_end(&rows[last]);
        let to = if down { self.row_unit_end(rows.iter().find(|r| r.start >= end)?) } else { rows[first.checked_sub(1)?].start };
        Some((from, end, to))
    }

    /// The headings with text under them that the selection's rows belong
    /// to (body text counts as its heading).
    fn outline_heads(&self) -> Vec<usize> {
        let (a, b) = self.outline_sel();
        let rows = self.outline_rows();
        let mut heads: Vec<usize> = rows
            .iter()
            .filter(|r| r.end > a && r.start <= b)
            .filter_map(|r| if r.level.is_some() { Some(r.start) } else { rows.iter().rev().find(|h| h.start < r.start && h.level.is_some() && self.doc.section_end(h.start) > r.start).map(|h| h.start) })
            .filter(|&h| self.doc.section_end(h) > h + 1)
            .collect();
        heads.dedup();
        heads
    }

    /// Whether Expand (or Collapse) has a heading to act on.
    fn outline_can_expand(&self, expand: bool) -> bool {
        self.outline_heads().iter().any(|h| self.views.outline.collapsed.contains(h) == expand)
    }

    /// Expands (or collapses) the headings of the selection; body text
    /// counts as its heading.
    pub(super) fn outline_expand(&mut self, expand: bool) {
        for h in self.outline_heads() {
            if expand {
                self.views.outline.collapsed.remove(&h);
            } else {
                self.views.outline.collapsed.insert(h);
                if !(h..=h).contains(&self.sel.cursor.para) {
                    self.sel = Selection::caret(Pos::new(h, 0));
                }
            }
        }
    }

    pub(super) fn outline_show_level(&mut self, l: u8) {
        self.views.outline.show_level = l;
        self.views.outline.collapsed.clear();
        self.views.outline.scroll = true;
    }

    /// Keys of the outline.
    fn outline_keys(&mut self, ui: &egui::Ui) {
        if ui.ctx().memory(|m| m.top_modal_layer().is_some()) {
            return;
        }
        let alt_shift = Modifiers::ALT | Modifiers::SHIFT;
        let key = |k: Key, m: Modifiers| ui.input_mut(|i| i.consume_key(m, k));
        if key(Key::Tab, Modifiers::SHIFT) || key(Key::ArrowLeft, alt_shift) {
            self.outline_promote();
        } else if key(Key::Tab, Modifiers::NONE) || key(Key::ArrowRight, alt_shift) {
            self.outline_demote();
        } else if key(Key::ArrowUp, alt_shift) {
            self.outline_move(false);
        } else if key(Key::ArrowDown, alt_shift) {
            self.outline_move(true);
        } else if key(Key::Plus, alt_shift) || key(Key::Equals, alt_shift) {
            self.outline_expand(true);
        } else if key(Key::Minus, alt_shift) {
            self.outline_expand(false);
        } else if key(Key::Z, Modifiers::COMMAND) {
            self.undo();
            self.views.outline.edit = None;
        } else if key(Key::Y, Modifiers::COMMAND) || key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT) {
            self.redo();
            self.views.outline.edit = None;
        }
    }

    /// Up and down go from row to row.
    fn outline_step(&mut self, rows: &[Row], down: bool) {
        let cur = self.sel.cursor.para;
        let k = rows.iter().position(|r| r.end > cur).unwrap_or(0);
        let next = if down { (k + 1).min(rows.len().saturating_sub(1)) } else { k.saturating_sub(1) };
        if let Some(r) = rows.get(next) {
            self.sel = Selection::caret(Pos::new(r.start, 0));
            self.views.outline.edit = None;
            self.views.outline.scroll = true;
        }
    }

    /// Applies the edited text of paragraph `para` (only what changed, so
    /// the formatting of the rest stays).
    fn outline_apply(&mut self, para: usize, new: &str, cursor: usize) {
        let old = row_text(&self.doc.paragraphs[para]);
        if old != self.doc.paragraphs[para].text || old == new {
            return;
        }
        let pre = old.char_indices().zip(new.chars()).take_while(|((_, a), b)| a == b).last().map_or(0, |((i, c), _)| i + c.len_utf8());
        let max_suf = (old.len() - pre).min(new.len() - pre);
        let suf = old.bytes().rev().zip(new.bytes().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
        // Keep the suffix on a character boundary.
        let mut suf = suf;
        while suf > 0 && (!old.is_char_boundary(old.len() - suf) || !new.is_char_boundary(new.len() - suf)) {
            suf -= 1;
        }
        let sel = Selection { anchor: Pos::new(para, pre), cursor: Pos::new(para, old.len() - suf) };
        self.doc.insert_text(sel, &new[pre..new.len() - suf], None);
        let text = &self.doc.paragraphs[para].text;
        self.sel = Selection::caret(Pos::new(para, byte_of(text, cursor)));
    }

    /// The outline in place of the pages.
    pub(super) fn outline_view(&mut self, ui: &mut egui::Ui) {
        if self.views.outline.count != self.doc.paragraphs.len() {
            self.views.outline.collapsed.clear();
            self.views.outline.count = self.doc.paragraphs.len();
        }
        let bg = theme::workspace_bg(ui.visuals());
        ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
        self.outline_keys(ui);
        let rows = self.visible_rows();
        // Up and down move between rows, unless a menu is open.
        if !ui.ctx().memory(|m| m.top_modal_layer().is_some()) && !egui::Popup::is_any_open(ui.ctx()) {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                self.outline_step(&rows, true);
            } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                self.outline_step(&rows, false);
            }
        }
        let cur = self.sel.cursor.para.min(self.doc.paragraphs.len() - 1);
        let (sa, sb) = self.outline_sel();
        let edit_id = Id::new(("outline_edit", self.uid));
        let mut enter = false;
        let mut back_join = false;
        if ui.memory(|m| m.has_focus(edit_id)) {
            let at_start = egui::TextEdit::load_state(ui.ctx(), edit_id).and_then(|s| s.cursor.char_range()).is_some_and(|r| r.primary.index.0 == 0 && r.secondary.index.0 == 0);
            enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            back_join = at_start && cur > 0 && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace));
        }
        if enter {
            let ci = egui::TextEdit::load_state(ui.ctx(), edit_id).and_then(|s| s.cursor.char_range()).map_or(0, |r| r.primary.index.0);
            let text = &self.doc.paragraphs[cur].text;
            let p = self.doc.new_paragraph(Selection::caret(Pos::new(cur, byte_of(text, ci))));
            self.sel = Selection::caret(p);
            self.views.outline.edit = None;
            self.views.outline.want_cursor = Some(0);
            self.views.outline.scroll = true;
        }
        if back_join && !self.doc.in_table(cur - 1) {
            let prev_len = self.doc.paragraphs[cur - 1].text.chars().count();
            let p = self.doc.delete(Selection { anchor: Pos::new(cur - 1, self.doc.paragraphs[cur - 1].text.len()), cursor: Pos::new(cur, 0) });
            self.sel = Selection::caret(p);
            self.views.outline.edit = None;
            self.views.outline.want_cursor = Some(prev_len);
        }
        let rows = if enter || back_join { self.visible_rows() } else { rows };
        let cur = self.sel.cursor.para.min(self.doc.paragraphs.len() - 1);
        let rev = self.doc.revision;
        if self.views.outline.edit.as_ref().is_none_or(|e| e.0 != cur || e.2 != rev) {
            self.views.outline.edit = Some((cur, row_text(&self.doc.paragraphs[cur]), rev));
        }
        let scroll = std::mem::take(&mut self.views.outline.scroll);
        let st_first_line = self.views.outline.first_line;
        let formatting = self.views.outline.formatting;
        let mut clicked: Option<(usize, bool)> = None;
        let mut toggle: Option<usize> = None;
        let mut edited: Option<(usize, String, usize)> = None;
        egui::ScrollArea::vertical().id_salt(("outline_scroll", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
            let width = (ui.available_width() - 80.0).clamp(300.0, 900.0);
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - width) / 2.0).max(12.0));
                let paper = egui::Frame::new().fill(Paper::BG).inner_margin(egui::Margin::symmetric(24, 18)).corner_radius(2.0);
                paper.show(ui, |ui| {
                    ui.set_width(width);
                    ui.vertical(|ui| {

                        ui.spacing_mut().item_spacing.y = 2.0;
                        for r in &rows {
                            let selected = r.end > sa && r.start <= sb;
                            let x0 = r.depth as f32 * 26.0;
                            let est = self.views.outline.heights.get(&r.start).copied().unwrap_or(22.0);
                            let probe = Rect::from_min_size(ui.cursor().min, vec2(width, est));
                            // Text with fields, pictures or tabs is edited in the pages.
                            let editing = !r.table && r.start == cur && selected && sa == sb && row_text(&self.doc.paragraphs[r.start]) == self.doc.paragraphs[r.start].text;
                            if !editing && !selected && !ui.is_rect_visible(probe) {
                                ui.allocate_space(vec2(width, est));
                                continue;
                            }
                            let (size, bold) = match (formatting, r.level) {
                                (true, Some(1)) => (20.0, true),
                                (true, Some(2)) => (17.0, true),
                                (true, Some(3)) => (15.5, true),
                                (true, Some(_)) => (14.5, true),
                                (true, None) => (14.0, false),
                                (false, l) => (14.0, l.is_some()),
                            };
                            let font = fonts::font(Face::Sans, bold, size);
                            let resp = ui.horizontal_top(|ui| {
                                ui.add_space(x0);
                                // The row's symbol: ⊕ a heading with text under
                                // it, ⊖ one without, ▪ body text.
                                let (sr, sresp) = ui.allocate_exact_size(vec2(18.0, size + 4.0), Sense::click());
                                let c = pos2(sr.center().x, sr.min.y + (size + 4.0) / 2.0);
                                let ink = Color32::from_gray(90);
                                if r.level.is_some() {
                                    ui.painter().circle_stroke(c, 6.0, Stroke::new(1.2, ink));
                                    ui.painter().hline(c.x - 3.5..=c.x + 3.5, c.y, Stroke::new(1.4, ink));
                                    if r.has_body {
                                        ui.painter().vline(c.x, c.y - 3.5..=c.y + 3.5, Stroke::new(1.4, ink));
                                    }
                                } else {
                                    ui.painter().rect_filled(Rect::from_center_size(c, vec2(5.0, 5.0)), 0.0, ink);
                                }
                                if sresp.clicked() && r.has_body {
                                    toggle = Some(r.start);
                                }
                                let tw = ui.available_width();
                                if editing {
                                    let buf = &mut self.views.outline.edit.as_mut().expect("edit").1;
                                    let color = if r.level.is_some() && formatting { Color32::from_rgb(0x1F, 0x3A, 0x68) } else { Paper::TEXT };
                                    let te = egui::TextEdit::multiline(buf)
                                        .id(edit_id)
                                        .font(font.clone())
                                        .text_color(color)
                                        .frame(egui::Frame::NONE)
                                        .desired_rows(1)
                                        .desired_width(tw)
                                        .return_key(None)
                                        .background_color(Paper::TEXT_SELECTION.gamma_multiply(0.25));
                                    let out = te.show(ui);
                                    if let Some(ci) = self.views.outline.want_cursor.take() {
                                        let mut s = out.state.clone();
                                        s.cursor.set_char_range(Some(egui::text::CCursorRange::one(CCursor::new(ci))));
                                        s.store(ui.ctx(), edit_id);
                                        out.response.request_focus();
                                    } else if !out.response.has_focus() && ui.memory(|m| m.focused().is_none()) {
                                        out.response.request_focus();
                                    }
                                    if out.response.changed() {
                                        let ci = out.cursor_range.map_or(0, |c| c.primary.index.0);
                                        edited = Some((r.start, buf.clone(), ci));
                                    }
                                    out.response.response.clone()
                                } else {
                                    let text = if r.table { format!("▦ {}", tr("outline.table")) } else { row_text(&self.doc.paragraphs[r.start]) };
                                    let color = if r.table { Color32::from_gray(110) } else if r.level.is_some() && formatting { Color32::from_rgb(0x1F, 0x3A, 0x68) } else { Paper::TEXT };
                                    let mut job = LayoutJob::single_section(text, TextFormat { font_id: font.clone(), color, ..Default::default() });
                                    job.wrap.max_width = tw;
                                    if st_first_line {
                                        job.wrap.max_rows = 1;
                                    }
                                    let g = ui.painter().layout_job(job);
                                    let (tr_, resp) = ui.allocate_exact_size(vec2(tw, g.size().y.max(size + 4.0)), Sense::click());
                                    if selected {
                                        ui.painter().rect_filled(tr_, 2.0, Paper::TEXT_SELECTION);
                                    }
                                    let collapsed = r.has_body && self.views.outline.collapsed.contains(&r.start);
                                    if collapsed {
                                        let y = tr_.min.y + g.size().y + 1.0;
                                        ui.painter().extend(egui::Shape::dashed_line(&[pos2(tr_.min.x, y), pos2(tr_.min.x + g.size().x, y)], Stroke::new(1.0, Color32::from_gray(120)), 3.0, 2.0));
                                    }
                                    ui.painter().galley(tr_.min + vec2(0.0, 2.0), g, color);
                                    resp
                                }
                            });
                            let row_rect = resp.response.rect;
                            self.views.outline.heights.insert(r.start, row_rect.height() + 2.0);
                            if resp.inner.clicked() && !editing {
                                let shift = ui.input(|i| i.modifiers.shift);
                                clicked = Some((r.start, shift));
                            }
                            if scroll && r.start <= cur && cur < r.end {
                                ui.scroll_to_rect(row_rect.expand(30.0), None);
                            }
                        }
                    });
                });
            });
            ui.add_space(40.0);
        });
        if let Some((para, text, ci)) = edited {
            self.outline_apply(para, &text, ci);
            if let Some(e) = &mut self.views.outline.edit {
                e.2 = self.doc.revision;
            }
        }
        if let Some(h) = toggle {
            if !self.views.outline.collapsed.remove(&h) {
                self.views.outline.collapsed.insert(h);
            }
        }
        if let Some((p, shift)) = clicked {
            let pos = Pos::new(p, 0);
            self.sel = if shift { Selection { anchor: self.sel.anchor, cursor: pos } } else { Selection::caret(pos) };
            self.views.outline.edit = None;
            self.views.outline.want_cursor = Some(0);
        }
    }

    /// The Outlining tab.
    pub(super) fn outline_tab(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::toolstrip;
        let a = b.accent();
        b.group("outline.g.tools", Icon::Outline, |ui| {
            toolstrip::column(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("«").on_hover_text(tr("outline.to_h1")).clicked() {
                        self.outline_level(&|_, _| Some(1));
                    }
                    if toolstrip::small(ui, Some(Icon::Outdent), "", &format!("{} (Alt+Mayús+←)", tr("outline.promote")), a, false).clicked() {
                        self.outline_promote();
                    }
                    let cur = self.heading_level(self.sel.cursor.para.min(self.doc.paragraphs.len() - 1));
                    let name = |l: Option<u8>| l.map_or_else(|| tr("outline.body").to_string(), |l| trf("outline.level_n", &[("n", &l)]));
                    let mut pick = None;
                    crate::widgets::combo_box(ui, "outline_level", 130.0, name(cur), |ui| {
                        for l in (1..=9).map(Some).chain([None]) {
                            if ui.selectable_label(cur == l, name(l)).clicked() {
                                pick = Some(l);
                            }
                        }
                    });
                    if let Some(l) = pick {
                        self.outline_level(&|_, _| l);
                    }
                    if toolstrip::small(ui, Some(Icon::Indent), "", &format!("{} (Alt+Mayús+→)", tr("outline.demote")), a, false).clicked() {
                        self.outline_demote();
                    }
                    if ui.button("»").on_hover_text(tr("outline.to_body")).clicked() {
                        self.outline_level(&|_, _| None);
                    }
                });
                // Off with nothing to move past, or nothing to fold.
                ui.horizontal(|ui| {
                    let (up, down) = (self.outline_move_span(false).is_some(), self.outline_move_span(true).is_some());
                    if ui.add_enabled_ui(up, |ui| toolstrip::small(ui, Some(Icon::ChevronUp), "", &format!("{} (Alt+Mayús+↑)", tr("outline.move_up")), a, false)).inner.clicked() {
                        self.outline_move(false);
                    }
                    if ui.add_enabled_ui(down, |ui| toolstrip::small(ui, Some(Icon::ChevronDown), "", &format!("{} (Alt+Mayús+↓)", tr("outline.move_down")), a, false)).inner.clicked() {
                        self.outline_move(true);
                    }
                    if ui.add_enabled(self.outline_can_expand(true), egui::Button::new(" + ")).on_hover_text(tr("outline.expand")).clicked() {
                        self.outline_expand(true);
                    }
                    if ui.add_enabled(self.outline_can_expand(false), egui::Button::new(" − ")).on_hover_text(tr("outline.collapse")).clicked() {
                        self.outline_expand(false);
                    }
                });
            });
            toolstrip::column(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("outline.show_level"));
                    let l = self.views.outline.show_level;
                    let name = |l: u8| if l == 0 { tr("outline.all_levels").to_string() } else { trf("outline.level_n", &[("n", &l)]) };
                    let mut pick = None;
                    crate::widgets::combo_box(ui, "outline_show", 110.0, name(l), |ui| {
                        for k in (1..=9).chain([0]) {
                            if ui.selectable_label(l == k, name(k)).clicked() {
                                pick = Some(k);
                            }
                        }
                    });
                    if let Some(k) = pick {
                        self.outline_show_level(k);
                    }
                });
                if toolstrip::check(ui, self.views.outline.formatting, Icon::Brush, tr("outline.formatting")) {
                    self.views.outline.formatting = !self.views.outline.formatting;
                }
                if toolstrip::check(ui, self.views.outline.first_line, Icon::Pilcrow, tr("outline.first_line")) {
                    self.views.outline.first_line = !self.views.outline.first_line;
                }
            });
        });
        b.group("outline.g.close", Icon::Close, |ui| {
            if toolstrip::large(ui, Icon::Close, tr("outline.close"), "", a, false).clicked() {
                self.set_view(ViewMode::Print);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(texts: &[(&str, Option<u8>)]) -> DocView {
        let paras = texts
            .iter()
            .map(|(t, l)| {
                let mut p = Paragraph::new(*t);
                if let Some(l) = l {
                    p.style.kind = ParaKind::Heading(*l);
                }
                p
            })
            .collect();
        DocView::new(Document::from_paragraphs(paras))
    }

    #[test]
    fn outline_edits() {
        let mut v = view(&[("Uno", Some(1)), ("texto", None), ("Dos", Some(1)), ("más", None), ("Dos.1", Some(2))]);
        assert_eq!(v.visible_rows().len(), 5);
        // Collapsing hides the text under a heading; moving it takes it along.
        v.sel = Selection::caret(Pos::new(0, 0));
        // At the top: nothing above; nothing folded yet to unfold.
        assert!(v.outline_move_span(false).is_none() && v.outline_move_span(true).is_some());
        assert!(!v.outline_can_expand(true) && v.outline_can_expand(false));
        v.outline_expand(false);
        assert!(v.outline_can_expand(true) && !v.outline_can_expand(false));
        assert_eq!(v.visible_rows().iter().map(|r| r.start).collect::<Vec<_>>(), [0, 2, 3, 4]);
        // Down one shown row at a time, like Word.
        v.outline_move(true);
        let t: Vec<&str> = v.doc.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, ["Dos", "Uno", "texto", "más", "Dos.1"]);
        assert!(v.views.outline.collapsed.contains(&1));
        // Up again, above the heading before it, with all that is now
        // under it.
        v.outline_move(false);
        let t: Vec<&str> = v.doc.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, ["Uno", "texto", "más", "Dos.1", "Dos"]);
        // Levels: demote, promote body text to its heading's level.
        v.sel = Selection::caret(Pos::new(0, 0));
        v.outline_demote();
        assert_eq!(v.doc.paragraphs[0].style.kind, ParaKind::Heading(2));
        v.sel = Selection::caret(Pos::new(1, 0));
        v.outline_promote();
        assert_eq!(v.doc.paragraphs[1].style.kind, ParaKind::Heading(2));
        v.outline_show_level(1);
        assert_eq!(v.visible_rows().iter().map(|r| r.start).collect::<Vec<_>>(), [4]);
        // Typing changes only what changed.
        v.outline_apply(4, "texto nuevo", 11);
        assert_eq!(v.doc.paragraphs[4].text, "texto nuevo");
        assert_eq!(v.sel.cursor, Pos::new(4, 11));
    }
}
