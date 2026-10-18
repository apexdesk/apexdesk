//! Annotating with the mouse: the tools that add highlights, notes, text
//! boxes, ink, lines, arrows, shapes and stamps, filling in anywhere
//! (text, a check mark, the date), placing a visible signature, picking
//! an annotation to move, recolour or delete it, and the box where a
//! comment's text is written. Each change is one undo step of the file.

use apex_io::pdf::edit::annots::{Annot, Kind, NOTE_SIZE};
use eframe::egui::{self, Color32, CursorIcon, Key, Modifiers, Stroke};

use super::pages::PageMap;
use super::text::char_box;
use super::{PdfView, Request, Tool};
use crate::i18n::tr;
use crate::widgets::{self, DialogResult};

/// Colours and sizes for new annotations.
#[derive(Clone, Debug)]
pub struct Pen {
    /// Highlights.
    pub marker: u32,
    /// Everything else.
    pub color: u32,
    pub width: f32,
    pub font_size: f32,
    /// The stamp's text.
    pub stamp: String,
}

impl Default for Pen {
    fn default() -> Pen {
        Pen { marker: 0xFFE500, color: 0xE03020, width: 1.5, font_size: 12.0, stamp: tr(STAMPS[0]).to_string() }
    }
}

/// Colours offered in the toolstrip.
pub const COLORS: [u32; 8] = [0xFFE500, 0x7CE05A, 0x5AC8FA, 0xFF7EB9, 0xE03020, 0x2060E0, 0x209040, 0x000000];

/// Line widths offered.
pub const WIDTHS: [f32; 4] = [0.75, 1.5, 3.0, 5.0];

/// The standard stamps, by name key (their text is written in the file
/// in the language of the interface).
pub const STAMPS: [&str; 6] = ["pdf.stamp.approved", "pdf.stamp.not_approved", "pdf.stamp.draft", "pdf.stamp.final", "pdf.stamp.confidential", "pdf.stamp.for_comment"];

/// The name comments are signed with: the user's login.
pub fn user_name() -> String {
    ["USER", "USERNAME", "LOGNAME"].iter().find_map(|k| std::env::var(k).ok().filter(|s| !s.is_empty())).unwrap_or_default()
}

/// Today as year-month-day.
pub fn today() -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (y, m, d) = apex_sheet::format::civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// The name of an annotation kind (tools and the comment list).
pub fn kind_name(k: Kind) -> &'static str {
    tr(match k {
        Kind::Highlight => "pdf.tool.highlight",
        Kind::Underline => "pdf.tool.underline",
        Kind::StrikeOut => "pdf.tool.strike",
        Kind::Squiggly => "pdf.tool.squiggly",
        Kind::Note => "pdf.tool.note",
        Kind::FreeText => "pdf.tool.text_box",
        Kind::Ink => "pdf.tool.ink",
        Kind::Line => "pdf.tool.line",
        Kind::Arrow => "pdf.tool.arrow",
        Kind::Square => "pdf.tool.rect",
        Kind::Circle => "pdf.tool.oval",
        Kind::Stamp => "pdf.tool.stamp",
        Kind::Redact => "pdf.tool.redact",
        _ => "pdf.tool.other",
    })
}

pub fn color32(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// An annotation being drawn (points on the page as shown).
pub struct Draft {
    pub page: usize,
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub ink: Vec<[f32; 2]>,
}

/// A picked annotation being dragged, by itself or by one of its
/// handles (see [`handles`]).
pub struct Move {
    pub id: u32,
    pub page: usize,
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub handle: Option<usize>,
}

/// A comment's text being written: for a new annotation (added on OK) or
/// one of the file's.
pub struct EditBox {
    pub annot: Annot,
    pub new: bool,
    pub text: String,
    pub focus: bool,
    /// The box was put down with a click: it takes the size of its text.
    pub fit: bool,
}

/// How near a handle the mouse picks it, in screen pixels.
const HANDLE_PX: f32 = 6.0;
/// The smallest side a box is resized to, in points.
const MIN_SIDE: f32 = 6.0;

/// Annotations resized by their handles: the ones drawn as boxes.
fn resizable(a: &Annot) -> bool {
    matches!(a.kind, Kind::Square | Kind::Circle | Kind::FreeText | Kind::Stamp) && !a.locked
}

/// The handles of a box: its corners from the top left clockwise, then
/// the middles of its top, right, bottom and left sides.
fn handles(r: [f32; 4]) -> [[f32; 2]; 8] {
    let (mx, my) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]], [mx, r[1]], [r[2], my], [mx, r[3]], [r[0], my]]
}

fn handle_cursor(h: usize) -> CursorIcon {
    match h {
        0 | 2 => CursorIcon::ResizeNwSe,
        1 | 3 => CursorIcon::ResizeNeSw,
        4 | 6 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeHorizontal,
    }
}

/// `a` with handle `h` of its box moved by (dx, dy). A text box grows
/// to hold its text.
fn resized(a: &Annot, h: usize, dx: f32, dy: f32) -> Annot {
    let mut r = a.rect;
    let (left, top, right, bottom) = (matches!(h, 0 | 3 | 7), matches!(h, 0 | 1 | 4), matches!(h, 1 | 2 | 5), matches!(h, 2 | 3 | 6));
    if left {
        r[0] = (r[0] + dx).min(r[2] - MIN_SIDE);
    }
    if right {
        r[2] = (r[2] + dx).max(r[0] + MIN_SIDE);
    }
    if top {
        r[1] = (r[1] + dy).min(r[3] - MIN_SIDE);
    }
    if bottom {
        r[3] = (r[3] + dy).max(r[1] + MIN_SIDE);
    }
    let mut out = Annot { rect: r, ..a.clone() };
    fit_text(&mut out, false, f32::MAX);
    out
}

/// Grows a text box to hold its text, keeping its top left corner: as
/// tall as its lines (wrapped to its width), and with `exact` also as
/// wide as its longest line (at most to `right`) and no taller.
pub(super) fn fit_text(a: &mut Annot, exact: bool, right: f32) {
    use apex_io::pdf::edit::appearance::{width, wrap};
    if a.kind != Kind::FreeText {
        return;
    }
    let size = a.font_size.max(1.0) as f64;
    let pad = 2.0 + a.width.max(0.0) as f64;
    if exact {
        let w = a.contents.lines().map(|l| width(l, false, size)).fold(0.0, f64::max) + 2.0 * pad + 1.0;
        a.rect[2] = (a.rect[0] + w as f32).min(right).max(a.rect[0] + MIN_SIDE);
    }
    let lines = wrap(&a.contents, (a.rect[2] - a.rect[0]) as f64 - 2.0 * pad, size, false).len().max(1);
    let h = (2.0 * pad + 1.15 * size * lines as f64) as f32;
    a.rect[3] = if exact { a.rect[1] + h } else { a.rect[3].max(a.rect[1] + h) };
}

fn norm(a: [f32; 2], b: [f32; 2]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}

/// Whether two boxes share more than a sliver (half the smaller one's
/// height and some width).
fn overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    w > 1.0 && h > 0.5 * (a[3] - a[1]).min(b[3] - b[1])
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]
}

/// Boxes around the text of chars `a..=b`, one per line.
pub fn line_boxes(p: &apex_io::pdf::Page, a: usize, b: usize) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::new();
    let mut last: Option<(f32, f32)> = None;
    for c in p.chars.get(a..=b.min(p.chars.len().saturating_sub(1))).unwrap_or(&[]) {
        let r = char_box(c);
        match (last, out.last_mut()) {
            (Some((y, s)), Some(q)) if (c.y - y).abs() < s.max(c.size) * 0.5 => *q = [q[0].min(r[0]), q[1].min(r[1]), q[2].max(r[2]), q[3].max(r[3])],
            _ => {
                if c.ch.is_whitespace() {
                    continue;
                }
                out.push(r);
            }
        }
        last = Some((c.y, c.size));
    }
    out
}

impl PdfView {
    /// Picks a tool; a markup tool with text selected marks it at once, and
    /// the tool in use picked again goes back to selecting (always: its
    /// button is never stuck).
    pub fn set_tool(&mut self, t: Tool) {
        if let Tool::Markup(k) = t
            && self.tool != t
            && self.sel.is_some()
        {
            self.mark_selection(k);
            return;
        }
        self.tool = if self.tool == t && t != Tool::Select { Tool::Select } else { t };
        self.draft = None;
        self.picked = None;
        if self.tool != Tool::PlaceSign {
            self.sig_look = None;
        }
    }

    /// Marks the selected text with a highlight, underline or strike out;
    /// text already marked so loses its marks instead (as a format button
    /// in a document).
    pub fn mark_selection(&mut self, kind: Kind) -> bool {
        let Some(s) = self.sel else { return false };
        let Some(p) = self.text.page(s.page) else { return false };
        let quads = line_boxes(p, s.a.min(s.b), s.a.max(s.b));
        let marked: Vec<u32> = self.annots_of(s.page).iter().filter(|a| a.kind == kind && a.reply_to.is_none() && a.quads.iter().any(|q| quads.iter().any(|r| overlap(*q, *r)))).map(|a| a.id).collect();
        let covered = !marked.is_empty() && {
            let list = self.annots_of(s.page);
            quads.iter().all(|r| list.iter().filter(|a| marked.contains(&a.id)).flat_map(|a| a.quads.iter()).any(|q| overlap(*q, *r)))
        };
        if covered {
            for id in marked {
                self.doc.delete_annot(id);
            }
            self.after_change();
            return true;
        }
        let Some(rect) = quads.iter().copied().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]) else { return false };
        let color = if kind == Kind::Highlight { self.pen.marker } else { self.pen.color };
        let a = Annot { page: s.page, kind, rect, quads, color, author: self.author.clone(), ..Default::default() };
        self.doc.add_annot(&a);
        self.after_change();
        true
    }

    /// The comment under point `pt` of page `i` (the topmost).
    pub(super) fn annot_at(&mut self, i: usize, pt: [f32; 2]) -> Option<Annot> {
        let list = self.annots_of(i);
        list.iter().rev().find(|a| {
            a.kind.is_comment() && a.reply_to.is_none() && !a.hidden && if a.kind.is_markup_text() && !a.quads.is_empty() { a.quads.iter().any(|q| inside(*q, pt)) } else { inside(a.bounds(), pt) }
        }).cloned()
    }

    /// Changes the picked annotation's colour, or the pen's.
    pub fn recolor(&mut self, c: u32) {
        if let Some(id) = self.picked
            && let Some(page) = self.doc.page_of_annot(id)
            && let Some(mut a) = self.doc.annots(page).into_iter().find(|a| a.id == id)
        {
            a.color = c;
            self.doc.update_annot(&a);
            self.after_change();
        } else if self.tool == Tool::Markup(Kind::Highlight) {
            self.pen.marker = c;
        } else {
            self.pen.color = c;
        }
    }

    /// Changes the picked text box's letter size (it grows to hold its
    /// text), or the pen's.
    pub fn set_font_size(&mut self, size: f32) {
        if let Some(id) = self.picked
            && let Some(page) = self.doc.page_of_annot(id)
            && let Some(mut a) = self.doc.annots(page).into_iter().find(|a| a.id == id && a.kind == Kind::FreeText)
        {
            a.font_size = size;
            fit_text(&mut a, false, f32::MAX);
            self.doc.update_annot(&a);
            self.after_change();
        }
        self.pen.font_size = size;
    }

    /// The letter size the size menu shows: the picked text box's, or
    /// the pen's.
    pub fn shown_font_size(&mut self) -> f32 {
        let picked = self.picked.and_then(|id| self.doc.page_of_annot(id).and_then(|p| self.doc.annots(p).into_iter().find(|a| a.id == id && a.kind == Kind::FreeText)));
        picked.map_or(self.pen.font_size, |a| a.font_size)
    }

    /// Deletes the picked annotation (and its replies).
    pub fn delete_picked(&mut self) {
        if let Some(id) = self.picked.take() {
            self.doc.delete_annot(id);
            self.after_change();
        }
    }

    /// Opens the comment box of annotation `id`.
    pub fn edit_comment(&mut self, id: u32) {
        let Some(page) = self.doc.page_of_annot(id) else { return };
        if let Some(a) = self.doc.annots(page).into_iter().find(|a| a.id == id) {
            self.edit = Some(EditBox { text: a.contents.clone(), annot: a, new: false, focus: true, fit: false });
        }
    }

    /// Delete and Escape for annotations and tools; true when used.
    pub(super) fn annot_keys(&mut self, ui: &egui::Ui) -> bool {
        if self.edit.is_some() || self.sign_box.is_some() || (self.picked.is_none() && self.tool == Tool::Select) {
            return false;
        }
        let picked = self.picked.is_some();
        let (del, esc) = ui.input_mut(|i| (picked && (i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)), i.consume_key(Modifiers::NONE, Key::Escape)));
        if del {
            self.delete_picked();
        }
        if esc {
            if self.picked.is_some() {
                self.picked = None;
            } else {
                self.set_tool(Tool::Select);
            }
        }
        del || esc
    }

    /// The mouse on page `i` (at `pt`, pressed at `start`) for
    /// annotations; true when it used it.
    pub(super) fn annot_input(&mut self, ui: &egui::Ui, resp: &egui::Response, i: usize, pt: [f32; 2], start: [f32; 2]) -> bool {
        if !matches!(self.tool, Tool::Hand | Tool::Markup(_)) && self.resize_input(ui, resp, i, pt, start) {
            return true;
        }
        match self.tool {
            Tool::Select => self.pick_input(ui, resp, i, pt, start),
            Tool::Hand | Tool::Markup(_) => false,
            _ => {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                if resp.drag_started() {
                    self.draft = Some(Draft { page: i, from: start, to: pt, ink: vec![start, pt] });
                } else if resp.dragged()
                    && let Some(d) = self.draft.as_mut().filter(|d| d.page == i)
                {
                    d.to = pt;
                    if d.ink.last().is_none_or(|l| (l[0] - pt[0]).abs() + (l[1] - pt[1]).abs() > 0.8) {
                        d.ink.push(pt);
                    }
                }
                if resp.drag_stopped() {
                    if let Some(d) = self.draft.take() {
                        self.finish(d);
                    }
                } else if resp.clicked() {
                    self.finish(Draft { page: i, from: pt, to: pt, ink: vec![pt] });
                }
                true
            }
        }
    }

    /// Resizing the picked box by its handles, with any tool that draws
    /// or picks; true when used.
    fn resize_input(&mut self, ui: &egui::Ui, resp: &egui::Response, i: usize, pt: [f32; 2], start: [f32; 2]) -> bool {
        if let Some(m) = self.moving.as_mut().filter(|m| m.handle.is_some()) {
            if resp.dragged() && m.page == i {
                m.to = pt;
            }
            ui.ctx().set_cursor_icon(handle_cursor(m.handle.unwrap_or(0)));
            if resp.drag_stopped()
                && let Some(m) = self.moving.take()
            {
                let (dx, dy) = (m.to[0] - m.from[0], m.to[1] - m.from[1]);
                if dx.abs() + dy.abs() > 0.5
                    && let Some(a) = self.doc.annots(m.page).into_iter().find(|a| a.id == m.id)
                {
                    self.doc.update_annot(&resized(&a, m.handle.unwrap_or(0), dx, dy));
                    self.after_change();
                }
            }
            return true;
        }
        let Some(a) = self.picked.and_then(|id| self.annots_of(i).iter().find(|a| a.id == id).cloned()).filter(resizable) else { return false };
        let tol = HANDLE_PX / self.zoom.max(0.05);
        let near = |p: [f32; 2]| {
            handles(a.rect).iter().enumerate().map(|(k, h)| (k, (h[0] - p[0]).hypot(h[1] - p[1]))).filter(|(_, d)| *d <= tol).min_by(|x, y| x.1.total_cmp(&y.1)).map(|(k, _)| k)
        };
        if resp.drag_started()
            && let Some(h) = near(start)
        {
            self.moving = Some(Move { id: a.id, page: i, from: start, to: pt, handle: Some(h) });
            ui.ctx().set_cursor_icon(handle_cursor(h));
            return true;
        }
        match near(pt) {
            Some(h) => {
                ui.ctx().set_cursor_icon(handle_cursor(h));
                true
            }
            None => false,
        }
    }

    /// Picking and moving annotations with the select tool.
    fn pick_input(&mut self, ui: &egui::Ui, resp: &egui::Response, i: usize, pt: [f32; 2], start: [f32; 2]) -> bool {
        if let Some(m) = self.moving.as_mut() {
            if resp.dragged() && m.page == i {
                m.to = pt;
            }
            if resp.drag_stopped()
                && let Some(m) = self.moving.take()
            {
                let (dx, dy) = (m.to[0] - m.from[0], m.to[1] - m.from[1]);
                if dx.abs() + dy.abs() > 0.5
                    && let Some(a) = self.doc.annots(m.page).into_iter().find(|a| a.id == m.id)
                {
                    self.doc.update_annot(&a.moved(dx, dy));
                    self.after_change();
                }
            }
            return true;
        }
        let hit = self.annot_at(i, pt);
        let movable = hit.as_ref().filter(|a| self.picked == Some(a.id) && !a.kind.is_markup_text() && !a.locked);
        if movable.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Move);
        }
        if resp.drag_started()
            && let Some(a) = self.annot_at(i, start).filter(|a| self.picked == Some(a.id) && !a.kind.is_markup_text() && !a.locked)
        {
            self.moving = Some(Move { id: a.id, page: i, from: start, to: pt, handle: None });
            return true;
        }
        if resp.secondary_clicked() {
            self.picked = hit.as_ref().map(|a| a.id);
            return false;
        }
        if resp.double_clicked()
            && let Some(a) = &hit
        {
            self.picked = Some(a.id);
            self.edit_comment(a.id);
            return true;
        }
        if resp.clicked() {
            self.picked = hit.as_ref().map(|a| a.id);
            if hit.is_some() {
                self.sel = None;
                return true;
            }
        }
        false
    }

    /// A new annotation from what was drawn with the tool in use.
    fn finish(&mut self, d: Draft) {
        let [pw, ph] = self.sizes.1.get(d.page).copied().unwrap_or([612.0, 792.0]);
        let clamp = |p: [f32; 2]| [p[0].clamp(0.0, pw), p[1].clamp(0.0, ph)];
        let (from, to) = (clamp(d.from), clamp(d.to));
        let r = norm(from, to);
        let big = r[2] - r[0] > 4.0 && r[3] - r[1] > 4.0;
        // A box of this size at the click, kept on the page.
        let at = |w: f32, h: f32, dy: f32| {
            let x = from[0].min(pw - w).max(0.0);
            let y = (from[1] + dy).min(ph - h).max(0.0);
            [x, y, x + w, y + h]
        };
        let base = Annot { page: d.page, color: self.pen.color, width: self.pen.width, font_size: self.pen.font_size, author: self.author.clone(), ..Default::default() };
        let black = |a: Annot| Annot { color: 0x000000, width: 0.0, kind: Kind::FreeText, ..a };
        // A line of text of the pen's size, centred on the click.
        let line_h = 4.0 + 1.15 * self.pen.font_size;
        let add = match self.tool {
            Tool::Draw(Kind::Note) => {
                let a = Annot { kind: Kind::Note, rect: at(NOTE_SIZE, NOTE_SIZE, 0.0), color: self.pen.marker, ..base };
                self.edit = Some(EditBox { annot: a, new: true, text: String::new(), focus: true, fit: false });
                None
            }
            Tool::Draw(Kind::FreeText) => {
                let a = Annot { kind: Kind::FreeText, rect: if big { r } else { at(180.0, 40.0, 0.0) }, ..base };
                self.edit = Some(EditBox { annot: a, new: true, text: String::new(), focus: true, fit: false });
                None
            }
            Tool::FillText => {
                let a = black(Annot { rect: if big { r } else { at(200.0, line_h, -line_h / 2.0) }, ..base });
                self.edit = Some(EditBox { annot: a, new: true, text: String::new(), focus: true, fit: !big });
                None
            }
            Tool::FillDate => {
                let mut a = black(Annot { rect: at(70.0, line_h, -line_h / 2.0), contents: today(), ..base });
                fit_text(&mut a, true, pw);
                Some(a)
            }
            Tool::FillCheck => {
                let (x, y) = (from[0], from[1]);
                let ink = vec![vec![[x - 4.0, y], [x - 1.0, y + 4.0], [x + 5.0, y - 5.0]]];
                Some(Annot { kind: Kind::Ink, rect: [x - 5.0, y - 5.0, x + 5.0, y + 5.0], ink, color: 0x000000, width: 1.5, ..base })
            }
            Tool::Draw(Kind::Ink) if d.ink.len() >= 2 => {
                let ink: Vec<[f32; 2]> = d.ink.iter().map(|p| clamp(*p)).collect();
                let rect = ink.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
                Some(Annot { kind: Kind::Ink, rect, ink: vec![ink], ..base })
            }
            Tool::Draw(k @ (Kind::Line | Kind::Arrow)) if (to[0] - from[0]).abs() + (to[1] - from[1]).abs() > 3.0 => Some(Annot { kind: k, rect: r, line: Some([from[0], from[1], to[0], to[1]]), ..base }),
            Tool::Draw(k @ (Kind::Square | Kind::Circle)) => Some(Annot { kind: k, rect: if big { r } else { at(100.0, 70.0, 0.0) }, ..base }),
            Tool::Draw(Kind::Redact) if big => Some(Annot { kind: Kind::Redact, rect: r, quads: vec![r], color: 0xFF0000, ..base }),
            Tool::Draw(Kind::Stamp) => Some(Annot { kind: Kind::Stamp, rect: if big { r } else { at(160.0, 44.0, -22.0) }, stamp: self.pen.stamp.clone(), ..base }),
            Tool::PlaceSign => {
                if let Some((look, digital)) = self.sig_look.take() {
                    let rect = if big { r } else { at(180.0, 60.0, -30.0) };
                    if let Some(name) = self.doc.add_signature_field(d.page, rect, &look)
                        && digital
                    {
                        self.requests.push(Request::Sign(Some(name)));
                    }
                    self.after_change();
                }
                self.tool = Tool::Select;
                None
            }
            _ => None,
        };
        if let Some(a) = add {
            self.picked = Some(self.doc.add_annot(&a));
            self.after_change();
        }
    }

    /// The picked annotation's frame and what is being drawn.
    pub(super) fn paint_annot_overlay(&mut self, painter: &egui::Painter, i: usize, map: &PageMap) {
        let acc = crate::theme::ACCENT;
        if let Some(id) = self.picked
            && let Some(a) = self.annots_of(i).iter().find(|a| a.id == id)
        {
            let shown = match &self.moving {
                Some(m) if m.id == id => {
                    let (dx, dy) = (m.to[0] - m.from[0], m.to[1] - m.from[1]);
                    match m.handle {
                        Some(h) => resized(a, h, dx, dy),
                        None => a.moved(dx, dy),
                    }
                }
                _ => a.clone(),
            };
            painter.rect_stroke(map.rect_to_screen(shown.bounds()).expand(2.0), 1.0, Stroke::new(1.5, acc), egui::StrokeKind::Outside);
            if resizable(&shown) {
                for h in handles(shown.rect) {
                    let c = map.to_screen(h[0], h[1]);
                    let r = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
                    painter.rect_filled(r, 1.0, Color32::WHITE);
                    painter.rect_stroke(r, 1.0, Stroke::new(1.2, acc), egui::StrokeKind::Inside);
                }
            }
        }
        let Some(d) = self.draft.as_ref().filter(|d| d.page == i) else { return };
        let stroke = Stroke::new((self.pen.width * map.z).max(1.0), color32(self.pen.color));
        match self.tool {
            Tool::Draw(Kind::Ink) => {
                painter.add(egui::Shape::line(d.ink.iter().map(|p| map.to_screen(p[0], p[1])).collect(), stroke));
            }
            Tool::Draw(k @ (Kind::Line | Kind::Arrow)) => {
                let (from, to) = (map.to_screen(d.from[0], d.from[1]), map.to_screen(d.to[0], d.to[1]));
                painter.line_segment([from, to], stroke);
                if k == Kind::Arrow && from.distance(to) > 1.0 {
                    let len = (self.pen.width * 3.0 + 6.0) * map.z;
                    let back = (from - to).normalized();
                    for a in [0.45f32, -0.45] {
                        let (s, c) = a.sin_cos();
                        painter.line_segment([to, to + egui::vec2(back.x * c - back.y * s, back.x * s + back.y * c) * len], stroke);
                    }
                }
            }
            Tool::Draw(Kind::Circle) => {
                let r = map.rect_to_screen(norm(d.from, d.to));
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            _ => {
                painter.rect_stroke(map.rect_to_screen(norm(d.from, d.to)), 0.0, Stroke::new(1.0, acc), egui::StrokeKind::Inside);
            }
        }
    }

    /// The box where a comment's text is written.
    pub(super) fn edit_ui(&mut self, ctx: &egui::Context) {
        let Some(e) = &mut self.edit else { return };
        let title = kind_name(e.annot.kind);
        let res = widgets::dialog(ctx, "pdf_comment", title, 380.0, |ui| {
            if !e.annot.author.is_empty() {
                ui.label(egui::RichText::new(&e.annot.author).weak());
            }
            let r = ui.add(egui::TextEdit::multiline(&mut e.text).desired_width(f32::INFINITY).desired_rows(5));
            if std::mem::take(&mut e.focus) {
                r.request_focus();
            }
        });
        match res {
            DialogResult::Ok => self.commit_edit(),
            DialogResult::Cancel => self.edit = None,
            DialogResult::None => {}
        }
    }

    /// The comment box's OK: the new annotation added, or the text changed.
    pub fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let mut a = e.annot;
        let right = self.sizes.1.get(a.page).map_or(f32::MAX, |s| s[0]);
        if e.new {
            a.contents = e.text;
            fit_text(&mut a, e.fit, right);
            // An empty text box is nothing.
            if a.kind != Kind::FreeText || !a.contents.trim().is_empty() {
                self.picked = Some(self.doc.add_annot(&a));
            }
        } else if a.contents != e.text {
            a.contents = e.text;
            fit_text(&mut a, false, right);
            self.doc.update_annot(&a);
        }
        self.after_change();
    }
}
