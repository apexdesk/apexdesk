//! Footnotes and endnotes on the page: their layout at the bottom of pages
//! (above the footer, below a separator line) and at the end of the
//! document, pagination with notes that continue on the next page, and the
//! notes pane where their text is edited.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use apex_doc::{Note, NoteKind, Run};

use super::*;

/// Space above the first footnote of a page, holding the separator line.
const SEP_PT: f32 = 12.0;

/// A laid out note.
pub(super) struct NoteBox {
    /// Its mark in the body.
    pub pos: Pos,
    pub kind: NoteKind,
    /// The paragraphs as laid out (the first starts with the note's number).
    pub paras: Vec<ParaLayout>,
    pub shown: Vec<Paragraph>,
    /// Rows: paragraph, galley top and bottom (bottoms include the space to
    /// the next row).
    lines: Vec<Line>,
}

/// A row of a note: paragraph, galley top and bottom, and the paragraph's
/// space above it (first rows) and below it (last rows).
#[derive(Clone, Copy)]
struct Line {
    k: usize,
    a: f32,
    e: f32,
    above: f32,
    below: f32,
}

impl Line {
    fn h(&self) -> f32 {
        self.above + (self.e - self.a) + self.below
    }
}


/// A slice of a note placed on a page.
#[derive(Clone, Copy)]
pub(super) struct Piece {
    pub note: usize,
    /// Paragraph of the note and galley range.
    pub k: usize,
    pub gy0: f32,
    pub gy1: f32,
    /// Top: from the top of the footnote area for footnotes, of the content
    /// area for endnotes.
    pub y: f32,
}

#[derive(Default, Clone)]
pub(super) struct PageNotes {
    /// Height of the footnote area (separator included); 0 without footnotes.
    pub foot_h: f32,
    /// The area starts with the rest of a note from the page before (its
    /// separator spans the whole width, like other editors).
    pub cont: bool,
    pub foot: Vec<Piece>,
    pub end: Vec<Piece>,
    /// Top of the endnotes separator when the endnotes start on this page.
    pub end_top: Option<f32>,
}

/// Notes of a layout.
#[derive(Default)]
pub(super) struct NoteLayout {
    pub boxes: Vec<NoteBox>,
    pub pages: Vec<PageNotes>,
    /// Height of the footnote separator.
    pub sep: f32,
    /// Laid out notes by content hash, reused by the next layout.
    cache: HashMap<u64, Vec<ParaLayout>>,
}

/// Places footnotes while the body is paginated.
pub(super) struct FootPager {
    nl: NoteLayout,
    /// Footnote marks of body paragraphs (not in tables): char index, box.
    marks: HashMap<usize, Vec<(usize, usize)>>,
    /// Footnotes of paragraphs in table cells, by paragraph, until their
    /// row is placed.
    cell_marks: HashMap<usize, Vec<usize>>,
    content_h: f32,
    sep: f32,
    /// Rests of notes that didn't fit: box and first line.
    carry: VecDeque<(usize, usize)>,
}

/// The paragraphs of a note as shown: its number, superscript, before the text.
fn shown_note(n: &Note, label: &str) -> Vec<Paragraph> {
    let mut out: Vec<Paragraph> = n.paras.iter().map(|p| Paragraph { cell: None, ..p.clone() }).collect();
    if out.is_empty() {
        out.push(Paragraph::new(""));
    }
    for p in &mut out {
        p.style.toc = None;
        // Word files set the space their note style gives; ours have none.
        p.style.space_before = Some(p.style.space_before.unwrap_or(0.0));
        p.style.space_after = Some(p.style.space_after.unwrap_or(0.0));
        p.style.page_break_before = false;
    }
    let first = &mut out[0];
    let base = first.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
    let mark = Attrs { superscript: true, subscript: false, link: None, link_tip: None, image: None, note: None, field: None, ..base.clone() };
    let prefix = format!("{label}");
    let mut runs = vec![Run { len: prefix.len(), attrs: mark }, Run { len: 1, attrs: Attrs { image: None, note: None, field: None, link: None, link_tip: None, ..base } }];
    runs.extend(first.runs.iter().filter(|r| r.len > 0).cloned());
    first.text = format!("{prefix} {}", first.text);
    first.runs = runs;
    out
}

impl DocView {
    /// Lays out the notes of the document and returns the pager that places
    /// them; `values` has the labels of the marks.
    pub(super) fn foot_pager(&self, ctx: &egui::Context, z: f32, content: Vec2, values: &HashMap<Pos, String>, old: &mut NoteLayout, geometry: u64) -> FootPager {
        let pt = PT * z;
        let mut nl = NoteLayout::default();
        let mut marks: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        let mut cell_marks: HashMap<usize, Vec<usize>> = HashMap::new();
        for (pos, n) in self.doc.notes() {
            let label = values.get(&pos).cloned().unwrap_or_default();
            let shown = shown_note(&n, &label);
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (n.as_ref(), &label, geometry).hash(&mut h);
            let key = h.finish();
            let paras = match old.cache.remove(&key) {
                Some(p) => p,
                None => shown.iter().map(|p| self.layout_para(ctx, p, z, content.x, None)).collect(),
            };
            let mut lines = Vec::new();
            for (k, pl) in paras.iter().enumerate() {
                let rows = &pl.galley.rows;
                let n = rows.len().max(1);
                for ri in 0..n {
                    let (a, e) = match rows.get(ri) {
                        Some(row) => (row.pos.y, rows.get(ri + 1).map_or(pl.galley.size().y, |n| n.pos.y)),
                        None => (0.0, pl.galley.size().y.max(1.0)),
                    };
                    let above = if ri == 0 { pl.space_before } else { 0.0 };
                    let below = if ri + 1 == n { pl.space_after } else { 0.0 };
                    lines.push(Line { k, a, e, above, below });
                }
            }
            let _ = pt;
            nl.cache.insert(key, paras.clone());
            let i = nl.boxes.len();
            if n.kind == NoteKind::Foot && self.doc.paragraphs.get(pos.para).is_some_and(|p| p.cell.is_none()) {
                let ci = char_count(&self.doc.paragraphs[pos.para].text, pos.offset);
                marks.entry(pos.para).or_default().push((ci, i));
            } else if n.kind == NoteKind::Foot && self.doc.paragraphs.get(pos.para).is_some() {
                cell_marks.entry(pos.para).or_default().push(i);
            }
            nl.boxes.push(NoteBox { pos, kind: n.kind, paras, shown, lines });
        }
        let sep = self.note_separator(ctx, z);
        nl.sep = sep;
        FootPager { nl, marks, cell_marks, content_h: content.y, sep, carry: VecDeque::new() }
    }

    /// Height of the separator above footnotes: the paragraph that holds
    /// its line in Word files (see `NoteSeparator`), else 12 pt.
    fn note_separator(&self, ctx: &egui::Context, z: f32) -> f32 {
        let Some(s) = self.doc.notes.separator else { return SEP_PT * PT * z };
        let st = ParaStyle::default();
        let a = Attrs { size: Some(s.size), ..Default::default() };
        let fmt = self.text_format(&st, &a, z);
        let _ = ctx;
        let factor = fonts::line_factor_for(self.doc.default_font_name().as_deref(), &fmt.font_id.family);
        s.height(factor) * PT * z
    }
}

impl FootPager {
    fn page(&mut self, page: usize) -> &mut PageNotes {
        if self.nl.pages.len() <= page {
            let first_new = self.nl.pages.len();
            self.nl.pages.resize(page + 1, PageNotes::default());
            // Rests of notes go first on the next page.
            if !self.carry.is_empty() && first_new <= page {
                self.place_carry(first_new.max(page));
            }
        }
        &mut self.nl.pages[page]
    }

    fn place_carry(&mut self, page: usize) {
        // Up to half the page, so the body goes on.
        let max = self.content_h * 0.5;
        let mut used = self.sep;
        let mut pieces = Vec::new();
        while let Some((b, from)) = self.carry.pop_front() {
            let nb = &self.nl.boxes[b];
            let mut li = from;
            while li < nb.lines.len() {
                let l = nb.lines[li];
                if used + l.h() > max && !pieces.is_empty() {
                    break;
                }
                pieces.push(Piece { note: b, k: l.k, gy0: l.a, gy1: l.e, y: used + l.above });
                used += l.h();
                li += 1;
            }
            if li < nb.lines.len() {
                self.carry.push_front((b, li));
                break;
            }
        }
        let pn = &mut self.nl.pages[page];
        pn.cont = true;
        pn.foot_h = used;
        pn.foot = merge(pieces);
    }

    /// Height of the footnote area of `page`.
    pub fn foot_h(&mut self, page: usize) -> f32 {
        if self.marks.is_empty() && self.carry.is_empty() && self.nl.pages.len() <= page {
            return 0.0;
        }
        self.page(page).foot_h
    }

    /// Height the footnotes marked in table cell paragraphs `paras` need
    /// on `page`.
    pub fn cell_need(&mut self, paras: std::ops::Range<usize>, page: usize) -> f32 {
        if self.cell_marks.is_empty() {
            return 0.0;
        }
        let h: f32 = paras.filter_map(|p| self.cell_marks.get(&p)).flatten().map(|&b| self.nl.boxes[b].lines.iter().map(Line::h).sum::<f32>()).sum();
        if h == 0.0 {
            return 0.0;
        }
        h + if self.foot_h(page) > 0.0 { 0.0 } else { self.sep }
    }

    /// Places the footnotes marked in table cell paragraphs `paras` on
    /// `page`, whole.
    pub fn place_cells(&mut self, paras: std::ops::Range<usize>, page: usize) {
        if self.cell_marks.is_empty() {
            return;
        }
        let mut boxes: Vec<usize> = paras.filter_map(|p| self.cell_marks.remove(&p)).flatten().collect();
        boxes.sort_unstable();
        for b in boxes {
            self.place_whole(b, page);
        }
    }

    fn place_whole(&mut self, b: usize, pg: usize) {
        let sep = self.sep;
        let pn = self.page(pg);
        let mut used = if pn.foot_h > 0.0 { pn.foot_h } else { sep };
        let new: Vec<Piece> = self.nl.boxes[b]
            .lines
            .iter()
            .map(|l| {
                let p = Piece { note: b, k: l.k, gy0: l.a, gy1: l.e, y: used + l.above };
                used += l.h();
                p
            })
            .collect();
        let pn = &mut self.nl.pages[pg];
        pn.foot.extend(new);
        pn.foot = merge(std::mem::take(&mut pn.foot));
        pn.foot_h = used;
    }

    /// Whether body paragraphs in `range` have footnote marks.
    pub fn has_marks(&self, range: std::ops::Range<usize>) -> bool {
        self.marks.keys().any(|p| range.contains(p))
    }

    /// Lowest body y on `page`.
    pub fn limit(&mut self, page: usize) -> f32 {
        let h = self.content_h;
        h - self.page(page).foot_h
    }

    fn row_notes(&self, para: usize, pl: &ParaLayout, ri: usize) -> Vec<usize> {
        let Some(m) = self.marks.get(&para) else { return Vec::new() };
        let mut start = 0usize;
        for row in &pl.galley.rows[..ri.min(pl.galley.rows.len())] {
            start += row.row.glyphs.len() + usize::from(row.ends_with_newline);
        }
        let Some(row) = pl.galley.rows.get(ri) else { return Vec::new() };
        let end = start + row.row.glyphs.len() + usize::from(row.ends_with_newline);
        let last = ri + 1 == pl.galley.rows.len();
        m.iter().filter(|(ci, _)| *ci >= start && (*ci < end || last)).map(|m| m.1).collect()
    }

    /// Extra height that the footnotes of row `ri` of paragraph `para` need
    /// on `page`.
    pub fn need(&mut self, para: usize, pl: &ParaLayout, ri: usize, page: usize) -> f32 {
        if self.marks.is_empty() {
            return 0.0;
        }
        let notes = self.row_notes(para, pl, ri);
        if notes.is_empty() {
            return 0.0;
        }
        let sep = if self.page(page).foot_h > 0.0 { 0.0 } else { self.sep };
        // The first line of each note has to fit with its mark; the rest may
        // go on to the next page.
        sep + notes.iter().map(|&b| self.nl.boxes[b].lines.first().map_or(0.0, Line::h)).sum::<f32>()
    }

    /// Places the footnotes of row `ri` of paragraph `para` on `page`, whose
    /// body now ends at `body_y`; what doesn't fit continues on the next page.
    pub fn place(&mut self, para: usize, pl: &ParaLayout, ri: usize, page: usize, body_y: f32) {
        if self.marks.is_empty() {
            return;
        }
        let notes = self.row_notes(para, pl, ri);
        if notes.is_empty() {
            return;
        }
        let content_h = self.content_h;
        let sep = self.sep;
        let pn = self.page(page);
        if pn.foot_h == 0.0 {
            pn.foot_h = sep;
        }
        let mut used = pn.foot_h;
        let carrying = !self.carry.is_empty();
        let mut rest = Vec::new();
        let mut pieces = Vec::new();
        for b in notes {
            // Notes after one that was cut wait for the next page too.
            if !rest.is_empty() || carrying {
                rest.push((b, 0));
                continue;
            }
            let nb = &self.nl.boxes[b];
            let mut li = 0;
            while li < nb.lines.len() {
                let l = nb.lines[li];
                if body_y + used + l.h() > content_h + 0.5 && li > 0 {
                    break;
                }
                // At least the first line stays with the mark.
                pieces.push(Piece { note: b, k: l.k, gy0: l.a, gy1: l.e, y: used + l.above });
                used += l.h();
                li += 1;
            }
            if li < nb.lines.len() {
                rest.push((b, li));
            }
        }
        let pn = &mut self.nl.pages[page];
        pn.foot_h = used;
        pn.foot.extend(pieces);
        pn.foot = merge(std::mem::take(&mut pn.foot));
        self.carry.extend(rest);
    }

    /// Ends the pagination of the body, which ended on `page` at `y`: places
    /// the rest of notes, the endnotes and the notes of table cells.
    /// Returns the number of pages.
    pub fn finish(mut self, mut page: usize, y: f32, segs: &[Segment], paras: &[ParaLayout], doc: &Document) -> (usize, NoteLayout) {
        self.page(page);
        while !self.carry.is_empty() {
            page += 1;
            self.page(page);
        }
        // Endnotes after the text.
        let mut y = y;
        let mut first = true;
        for b in 0..self.nl.boxes.len() {
            if self.nl.boxes[b].kind != NoteKind::End {
                continue;
            }
            for li in 0..self.nl.boxes[b].lines.len() {
                let Line { k, a, e, above, below } = self.nl.boxes[b].lines[li];
                let head = if first { self.sep * 1.5 } else if li == 0 { self.sep * 0.3 } else { 0.0 };
                let limit = self.limit(page);
                if y + head + above + (e - a) > limit && y > 0.0 {
                    page += 1;
                    y = 0.0;
                    self.page(page);
                }
                if first {
                    self.nl.pages[page].end_top = Some(y);
                    first = false;
                }
                y += head + above;
                self.nl.pages[page].end.push(Piece { note: b, k, gy0: a, gy1: e, y });
                y += e - a + below;
            }
        }
        // Footnotes in table cells not placed with their row (tables in
        // text boxes and the like) go on the page of their mark.
        let mut rest: Vec<usize> = std::mem::take(&mut self.cell_marks).into_values().flatten().collect();
        rest.sort_unstable();
        for b in rest {
            let nb = &self.nl.boxes[b];
            let Some(pl) = paras.get(nb.pos.para) else { continue };
            let text = &doc.paragraphs[nb.pos.para].text;
            let r = pl.galley.pos_from_cursor(CCursor::new(char_count(text, nb.pos.offset)));
            let pg = segs.iter().filter(|s| s.para == nb.pos.para).rev().find(|s| r.min.y >= s.gy0 - 0.5).map_or(page, |s| s.page);
            self.place_whole(b, pg);
        }
        (page + 1, self.nl)
    }
}

/// Joins consecutive rows of the same note paragraph into one piece.
fn merge(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for p in pieces {
        match out.last_mut() {
            Some(l) if l.note == p.note && l.k == p.k && (l.gy1 - p.gy0).abs() < 0.01 && (l.y + (l.gy1 - l.gy0) - p.y).abs() < 0.01 => l.gy1 = p.gy1,
            _ => out.push(p),
        }
    }
    out
}

impl DocView {
    /// Where piece `pc` of a note is drawn on the page whose content area
    /// starts at `o`: galley position and clip rect.
    fn piece_geometry(l: &Layout, nb: &NoteBox, pc: &Piece, top: f32, o: Pos2) -> (Pos2, Rect) {
        let pl = &nb.paras[pc.k];
        let y = top + pc.y;
        let galley_pos = pos2(o.x + pl.indent + pl.align_off, y - pc.gy0);
        (galley_pos, Rect::from_min_max(pos2(o.x - 20.0, y), pos2(o.x + l.content.x + 20.0, y + pc.gy1 - pc.gy0)))
    }

    /// The footnotes and endnotes of `page`, whose content area starts at `o`.
    pub(super) fn paint_notes(&self, p: &egui::Painter, l: &Layout, page: usize, o: Pos2, z: f32) {
        let nl = &l.refs.notes;
        let Some(pn) = nl.pages.get(page) else { return };
        let stroke = Stroke::new((0.75 * z).max(0.5), Paper::TEXT);
        let none = HashMap::new();
        let draw = |pieces: &[Piece], top: f32| {
            for pc in pieces {
                let Some(nb) = nl.boxes.get(pc.note) else { continue };
                let Some(pl) = nb.paras.get(pc.k) else { continue };
                let (galley_pos, clip) = Self::piece_geometry(l, nb, pc, top, o);
                let cp = p.with_clip_rect(clip.intersect(p.clip_rect()));
                crate::text_fx::paint(&cp, galley_pos, &pl.fx);
                cp.galley(galley_pos, pl.galley.clone(), Paper::TEXT);
                self.paint_objects(&cp, &nb.shown[pc.k], None, pl, galley_pos, (pc.gy0, pc.gy1), &none, z);
            }
        };
        if !pn.foot.is_empty() {
            let top = o.y + l.content.y - pn.foot_h;
            let w = if pn.cont { l.content.x } else { l.content.x / 3.0 };
            p.hline(o.x..=o.x + w, top + nl.sep * 0.45, stroke);
            draw(&pn.foot, top);
        }
        if let Some(t) = pn.end_top {
            p.hline(o.x..=o.x + l.content.x / 3.0, o.y + t + SEP_PT * PT * z * 0.6, stroke);
        }
        if !pn.end.is_empty() {
            draw(&pn.end, o.y);
        }
    }

    /// The note whose text is at a canvas point.
    pub(super) fn note_at_point(&self, local: Pos2) -> Option<Pos> {
        let l = &self.layout;
        let nl = &l.refs.notes;
        if nl.boxes.is_empty() || l.pages == 0 {
            return None;
        }
        let (page, local) = self.flow_point(local);
        let pn = nl.pages.get(page)?;
        let o = self.content_origin(page);
        let top = o.y + l.content.y - pn.foot_h;
        for (pieces, t) in [(&pn.foot, top), (&pn.end, o.y)] {
            for pc in pieces {
                let nb = &nl.boxes[pc.note];
                let (_, r) = Self::piece_geometry(l, nb, pc, t, o);
                if r.contains(local) {
                    return Some(nb.pos);
                }
            }
        }
        None
    }

    /// Double click on a note mark: its text in the notes pane.
    pub(super) fn note_double_click(&mut self, local: Pos2) -> bool {
        let pos = self.pos_at(local);
        let hit = self.doc.note_at(pos).or_else(|| self.doc.note_at(self.doc.prev_char(pos)));
        match hit {
            Some((s, _)) => {
                self.toggle_notes_pane(Some(s.start()));
                true
            }
            None => false,
        }
    }

    /// Inserts a footnote or endnote at the caret and opens its text.
    pub(super) fn insert_note(&mut self, kind: NoteKind) {
        self.clear_block();
        let at = self.sel.end();
        let end = self.doc.insert_note(self.sel, kind);
        self.sel = Selection::caret(end);
        self.scroll_to_caret = true;
        let mark = Pos::new(end.para, end.offset - apex_doc::OBJECT.len_utf8());
        let _ = at;
        self.refs.pane = Some(NotesPane { focus: Some(mark), ..Default::default() });
    }

    /// Moves the caret to the next note mark (from the start again at the end).
    pub(super) fn next_note(&mut self) {
        let notes = self.doc.notes();
        let c = self.sel.end();
        if let Some((p, _)) = notes.iter().find(|(p, _)| *p >= c && !(*p == self.sel.start() && !self.sel.is_empty())).or(notes.first()) {
            let p = *p;
            let e = Pos::new(p.para, p.offset + apex_doc::OBJECT.len_utf8());
            self.set_sel(Selection { anchor: p, cursor: e });
        } else {
            self.notices.push(Notice::info(tr("fnote.none")));
        }
    }

    /// Back from a note's text to its mark in the text, selected (as Next
    /// Footnote does), so it shows where the caret already was.
    fn go_to_mark(&mut self, p: Pos) {
        self.set_sel(Selection { anchor: p, cursor: Pos::new(p.para, p.offset + apex_doc::OBJECT.len_utf8()) });
        self.focus = true;
    }

    /// Shows or hides the notes pane; `at` shows it with that note's text.
    pub(super) fn toggle_notes_pane(&mut self, at: Option<Pos>) {
        if at.is_some() {
            self.refs.pane = Some(NotesPane { focus: at, ..Default::default() });
        } else if self.refs.pane.take().is_none() {
            self.refs.pane = Some(NotesPane::default());
        }
    }

    /// The notes pane: every note's text, edited in place.
    pub(super) fn notes_pane(&mut self, ui: &mut egui::Ui) {
        let Some(mut pane) = self.refs.pane.take() else { return };
        if pane.rev != self.doc.revision || pane.items.is_empty() && !pane.loaded {
            pane.items = self.doc.notes().into_iter().map(|(p, n)| (p, n.kind, n.text())).collect();
            pane.rev = self.doc.revision;
            pane.loaded = true;
        }
        let mut close = false;
        let mut goto = None;
        let mut edit: Option<(Pos, String)> = None;
        egui::Panel::bottom(Id::new(("doc_notes", self.uid)))
            .resizable(true)
            .default_size(180.0)
            .min_size(80.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(tr("fnote.pane"));
                    ui.label(RichText::new(tr("fnote.pane_hint")).weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("dialog.close")).clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if pane.items.is_empty() {
                        ui.label(RichText::new(tr("fnote.none")).weak());
                    }
                    let labels = &self.layout.refs.values;
                    for (i, (pos, kind, text)) in pane.items.iter_mut().enumerate() {
                        ui.horizontal_top(|ui| {
                            let label = labels.get(pos).cloned().unwrap_or_default();
                            let kind_s = if *kind == NoteKind::Foot { tr("fnote.footnote") } else { tr("fnote.endnote") };
                            let r = ui.add_sized([56.0, 20.0], egui::Button::new(RichText::new(&label).strong()).frame(false)).on_hover_text(format!("{kind_s} {label}: {}", tr("fnote.go_to_mark")));
                            if r.clicked() {
                                goto = Some(*pos);
                            }
                            let id = Id::new(("note_text", self.uid, i));
                            let drag = crate::edit_drag::begin(ui, id);
                            let out = egui::TextEdit::multiline(text).id(id).desired_rows(1).desired_width(ui.available_width()).interactive(!drag.grabbing()).show(ui);
                            let dropped = drag.finish(ui, &out, text);
                            let resp = out.response;
                            if pane.focus == Some(*pos) {
                                resp.request_focus();
                                resp.scroll_to_me(Some(egui::Align::Center));
                            }
                            if resp.changed() || dropped {
                                edit = Some((*pos, text.clone()));
                            }
                        });
                    }
                });
            });
        pane.focus = None;
        if let Some((pos, text)) = edit
            && let Some((_, n)) = self.doc.note_at(pos)
        {
            let mut n = (*n).clone();
            n.set_text(&text);
            self.doc.set_note(pos, n);
            // The pane's text is already current.
            pane.rev = self.doc.revision;
        }
        if let Some(p) = goto {
            self.go_to_mark(p);
        }
        if !close {
            self.refs.pane = Some(pane);
        } else {
            self.focus = true;
        }
    }
}

/// The notes pane.
#[derive(Default)]
pub(super) struct NotesPane {
    /// Note whose text gets the focus.
    pub focus: Option<Pos>,
    items: Vec<(Pos, NoteKind, String)>,
    rev: u64,
    loaded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number in the notes pane goes to the note's mark, selected: a
    /// note just inserted has the caret after its mark already.
    #[test]
    fn note_number_selects_mark() {
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Text")]));
        v.sel = Selection::caret(Pos::new(0, 4));
        v.insert_note(NoteKind::Foot);
        let caret = v.sel;
        let mark = v.refs.pane.as_ref().and_then(|p| p.focus).expect("the note's text");
        v.go_to_mark(mark);
        assert_ne!(v.sel, caret);
        assert_eq!((v.sel.start(), v.sel.end()), (mark, caret.cursor));
    }
}
