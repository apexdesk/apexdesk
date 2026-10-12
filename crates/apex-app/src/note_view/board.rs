//! A canvas page: cards of pages, notes, pictures and groups placed
//! anywhere, joined by arrows. An arrow between two page cards links
//! those pages, with what it means.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use apex_doc::notebook::Rel;
use apex_doc::notebook::canvas::{Canvas, ItemKind, NOTE_COLORS};
use eframe::egui::{Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};

use super::graph::rel_color;
use super::*;

/// Changes of a canvas undone at most.
const STEPS: usize = 100;
const NOTE_SIZE: Vec2 = vec2(200.0, 130.0);
const CARD_SIZE: Vec2 = vec2(240.0, 150.0);
/// Height of a group's title bar, in canvas units.
const GROUP_BAR: f32 = 30.0;
const MIN_SIZE: f32 = 40.0;
/// Widest a picture comes in.
const PICTURE_W: f32 = 320.0;
/// The dot to drag an arrow from, in screen points.
const DOT: f32 = 6.0;
/// A marker on a map, in screen points (the same at any zoom).
const PIN: Vec2 = vec2(22.0, 32.0);
/// Widest a map comes in, in canvas units.
const MAP_W: f32 = 2400.0;

/// What a drag on the canvas does.
enum Drag {
    Pan,
    Move,
    Resize(i64),
    Connect { from: i64, to: Pos2 },
    Select { from: Pos2, to: Pos2 },
}

pub(super) struct Board {
    pub page: i64,
    pub canvas: Canvas,
    /// The canvas point at the middle of the view, and its scale.
    center: Vec2,
    zoom: f32,
    /// Items and arrows picked.
    pub selected: Vec<i64>,
    drag: Option<Drag>,
    /// Where the pointer was last frame of a drag.
    last: Pos2,
    /// The note, group or arrow whose words are being typed.
    editing: Option<(i64, String)>,
    focus_edit: bool,
    undo: Vec<Canvas>,
    redo: Vec<Canvas>,
    /// What new arrows mean.
    pub rel: Rel,
    /// The canvas when a drag started.
    before: Option<Canvas>,
    /// A picture being read from the clipboard.
    incoming: Option<Receiver<Option<Vec<u8>>>>,
    /// Where the next thing added goes.
    spot: Vec2,
    /// Everything in view on the next frame.
    fit: bool,
    /// Changed since written.
    pub dirty: bool,
    /// What the context menu is about.
    menu: Option<i64>,
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn rect_of(r: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(r[0], r[1]), vec2(r[2], r[3]))
}

/// A map marker in `r` (its point at the bottom middle), with its page's
/// title beside it.
fn paint_pin(painter: &egui::Painter, r: Rect, color: Color32, title: &str, picked: bool, v: &egui::Visuals) {
    let tip = r.center_bottom();
    let rad = r.width() / 2.0;
    let c = pos2(tip.x, r.min.y + rad);
    let side = rad * 0.87;
    painter.add(egui::Shape::convex_polygon(vec![c + vec2(-side, rad * 0.5), c + vec2(side, rad * 0.5), tip], color, Stroke::NONE));
    painter.circle(c, rad, color, Stroke::new(1.0, Color32::from_black_alpha(90)));
    painter.circle_filled(c, rad * 0.38, Color32::WHITE);
    if picked {
        painter.circle_stroke(c, rad + 3.0, Stroke::new(2.0, theme::NOTE_ACCENT));
    }
    let g = painter.layout_no_wrap(title.to_string(), egui::FontId::proportional(13.0), v.strong_text_color());
    let at = pos2(c.x + rad + 5.0, c.y - g.size().y / 2.0);
    painter.rect_filled(Rect::from_min_size(at, g.size()).expand2(vec2(4.0, 1.0)), 4.0, v.extreme_bg_color.gamma_multiply(0.85));
    painter.galley(at, g, v.strong_text_color());
}

/// Where the line from the middle of `r` towards `to` leaves it.
fn edge(r: Rect, to: Pos2) -> Pos2 {
    let c = r.center();
    let d = to - c;
    if d.length() < 1.0 {
        return c;
    }
    let tx = if d.x.abs() > 1e-3 { r.width() / 2.0 / d.x.abs() } else { f32::MAX };
    let ty = if d.y.abs() > 1e-3 { r.height() / 2.0 / d.y.abs() } else { f32::MAX };
    c + d * tx.min(ty).min(1.0)
}

/// Distance from `p` to the segment `a`–`b`.
fn to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t - p).length()
}

impl Board {
    pub fn new(page: i64, canvas: Canvas) -> Board {
        Board {
            page,
            canvas,
            center: Vec2::ZERO,
            zoom: 1.0,
            selected: Vec::new(),
            drag: None,
            last: Pos2::ZERO,
            editing: None,
            focus_edit: false,
            undo: Vec::new(),
            redo: Vec::new(),
            rel: Rel::Link,
            before: None,
            incoming: None,
            spot: Vec2::ZERO,
            fit: true,
            dirty: false,
            menu: None,
        }
    }

    /// One undo step from `before` to now, if anything changed.
    fn step(&mut self, before: Canvas) {
        if before != self.canvas {
            self.undo.push(before);
            if self.undo.len() > STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.dirty = true;
        }
    }

    /// Changes the canvas as one undo step.
    pub fn edit(&mut self, f: impl FnOnce(&mut Canvas)) {
        let before = self.canvas.clone();
        f(&mut self.canvas);
        self.step(before);
    }

    pub fn undo(&mut self) {
        if let Some(c) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.canvas, c));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        if let Some(c) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.canvas, c));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.selected.retain(|&s| self.canvas.item(s).is_some() || self.canvas.arrows.iter().any(|a| a.id == s));
        self.editing = None;
        self.dirty = true;
    }

    /// A rect of `size` centred at `at`.
    fn around(at: Vec2, size: Vec2) -> [f32; 4] {
        [at.x - size.x / 2.0, at.y - size.y / 2.0, size.x, size.y]
    }

    /// `at`, or a step down and right of it while another item is centred
    /// there: things added one after another don't hide each other.
    fn free(&self, mut at: Vec2) -> Vec2 {
        for _ in 0..50 {
            let taken = self.canvas.items.iter().filter(|i| !matches!(i.kind, ItemKind::Map(_))).any(|i| (vec2(i.rect[0] + i.rect[2] / 2.0, i.rect[1] + i.rect[3] / 2.0) - at).length() < 12.0);
            if !taken {
                break;
            }
            at += vec2(28.0, 28.0);
        }
        at
    }

    pub fn add_note(&mut self, at: Vec2, text: &str) -> i64 {
        let at = self.free(at);
        let mut id = 0;
        self.edit(|c| id = c.add(ItemKind::Note(text.to_string()), Self::around(at, NOTE_SIZE), NOTE_COLORS[0]));
        self.selected = vec![id];
        id
    }

    pub fn add_card(&mut self, page: i64, at: Vec2) -> i64 {
        let at = self.free(at);
        let mut id = 0;
        self.edit(|c| id = c.add(ItemKind::Page(page), Self::around(at, CARD_SIZE), 0xFFFFFF));
        self.selected = vec![id];
        id
    }

    pub fn add_picture(&mut self, bytes: Vec<u8>, at: Vec2) -> i64 {
        let (w, h) = apex_io::pixel_size(&bytes).map_or((PICTURE_W, PICTURE_W * 0.75), |(w, h)| (w.max(1) as f32, h.max(1) as f32));
        let s = (PICTURE_W / w).min(1.0);
        let at = self.free(at);
        let mut id = 0;
        self.edit(|c| id = c.add(ItemKind::Picture(Arc::from(bytes)), Self::around(at, vec2(w * s, h * s)), 0xFFFFFF));
        self.selected = vec![id];
        id
    }

    /// A marker of `page` whose point is `at`.
    pub fn add_pin(&mut self, page: i64, at: Vec2) -> i64 {
        let at = self.free(at);
        let mut id = 0;
        self.edit(|c| id = c.add(ItemKind::Pin(page), [at.x, at.y, 0.0, 0.0], 0));
        self.selected = vec![id];
        id
    }

    /// `bytes` as the map (in the place of the one it had, or around what
    /// is in view).
    pub fn set_map(&mut self, bytes: Vec<u8>) {
        let (w, h) = apex_io::pixel_size(&bytes).map_or((MAP_W, MAP_W * 0.75), |(w, h)| (w.max(1) as f32, h.max(1) as f32));
        let s = (MAP_W / w).min(1.0);
        let rect = match self.canvas.map() {
            Some(m) => [m.rect[0], m.rect[1], m.rect[2], m.rect[2] * h / w],
            None => Self::around(self.center, vec2(w * s, h * s)),
        };
        self.edit(|c| {
            c.set_map(Arc::from(bytes), rect);
        });
        self.fit = true;
    }

    pub fn remove_map(&mut self) {
        let Some(id) = self.canvas.map().map(|m| m.id) else { return };
        self.edit(|c| c.remove(&[id]));
    }

    /// A group around what is picked (or an empty one at the spot).
    pub fn add_group(&mut self, title: &str) -> i64 {
        let picked: Vec<Rect> = self.selected.iter().filter_map(|&s| self.canvas.item(s)).filter(|i| !matches!(i.kind, ItemKind::Group(_))).map(|i| rect_of(i.rect)).collect();
        let r = match picked.into_iter().reduce(|a, b| a.union(b)) {
            Some(r) => Rect::from_min_max(r.min - vec2(20.0, 20.0 + GROUP_BAR), r.max + vec2(20.0, 20.0)),
            None => rect_of(Self::around(self.spot, vec2(420.0, 300.0))),
        };
        let mut id = 0;
        self.edit(|c| id = c.add(ItemKind::Group(title.to_string()), [r.min.x, r.min.y, r.width(), r.height()], NOTE_COLORS[2]));
        self.selected = vec![id];
        id
    }

    pub fn delete_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let ids = std::mem::take(&mut self.selected);
        self.edit(|c| c.remove(&ids));
    }

    /// The item ids picked with what the picked groups hold.
    fn moving(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.selected.iter().copied().filter(|&s| self.canvas.item(s).is_some()).collect();
        for s in v.clone() {
            for i in self.canvas.inside(s) {
                if !v.contains(&i) {
                    v.push(i);
                }
            }
        }
        v
    }

    /// The words of a note, group or arrow.
    fn words(&self, id: i64) -> Option<String> {
        match self.canvas.item(id).map(|i| &i.kind) {
            Some(ItemKind::Note(t) | ItemKind::Group(t)) => Some(t.clone()),
            Some(_) => None,
            None => self.canvas.arrows.iter().find(|a| a.id == id).map(|a| a.label.clone()),
        }
    }

    fn set_words(&mut self, id: i64, text: String) {
        self.edit(|c| {
            if let Some(i) = c.item_mut(id) {
                match &mut i.kind {
                    ItemKind::Note(t) | ItemKind::Group(t) => *t = text,
                    _ => {}
                }
            } else if let Some(a) = c.arrows.iter_mut().find(|a| a.id == id) {
                a.label = text;
            }
        });
    }

    /// Keeps the words being typed.
    fn finish_editing(&mut self) {
        if let Some((id, text)) = self.editing.take()
            && self.words(id).is_some_and(|t| t != text)
        {
            self.set_words(id, text);
        }
    }

    /// Put aside: nothing picked or being typed.
    pub(super) fn rest(&mut self) {
        self.finish_editing();
        self.selected.clear();
        self.drag = None;
    }

    fn start_editing(&mut self, id: i64) {
        self.finish_editing();
        if let Some(t) = self.words(id) {
            self.editing = Some((id, t));
            self.focus_edit = true;
        }
    }

    /// Everything in view in `rect`.
    fn fit_to(&mut self, rect: Rect) {
        let Some(all) = self.canvas.items.iter().map(|i| rect_of(i.rect)).reduce(|a, b| a.union(b)) else {
            self.center = Vec2::ZERO;
            self.zoom = 1.0;
            return;
        };
        self.center = all.center().to_vec2();
        self.zoom = ((rect.width() - 80.0) / all.width()).min((rect.height() - 80.0) / all.height()).clamp(0.2, 1.0);
    }
}

impl NoteView {
    pub fn board_open(&self) -> bool {
        self.board.is_some()
    }

    /// A new canvas page after the open one.
    pub fn new_canvas(&mut self) {
        let s = match self.section {
            Some(s) => s,
            None => self.here(),
        };
        let after = self.page.and_then(|p| self.nb.page(p)).map(|p| (p.parent, p.id));
        let mut id = 0;
        let title = tr("nb.untitled_canvas").to_string();
        self.change(|nb| {
            id = nb.add_page(s, after.and_then(|a| a.0), after.map(|a| a.1), &title, now());
            if let Some(p) = nb.page_mut(id) {
                p.kind = apex_doc::notebook::PageKind::Canvas;
            }
        });
        self.close_graph();
        self.open_page(id);
        self.name_new = true;
        self.grid = false;
    }

    /// Writes the canvas open if it changed.
    pub(super) fn write_board(&mut self) {
        let Some(mut b) = self.board.take() else { return };
        self.save_board(&mut b);
        self.board = Some(b);
    }

    /// Writes canvas `b` if it changed.
    pub(super) fn save_board(&mut self, b: &mut Board) {
        if !b.dirty {
            return;
        }
        b.dirty = false;
        let t = now();
        let r = self.store.save_canvas(b.page, &b.canvas, t);
        self.nb.canvas_links.insert(b.page, b.canvas.page_links(b.page));
        self.nb.texts.insert(b.page, b.canvas.text());
        if let Some(p) = self.nb.page_mut(b.page) {
            p.modified = t;
        }
        self.report(r);
    }

    /// A picture file put on the canvas open.
    pub fn add_picture_file(&mut self, path: &Path) {
        let Some(b) = &mut self.board else { return };
        match std::fs::read(path) {
            Ok(bytes) => {
                b.add_picture(bytes, b.spot);
                self.write_board();
            }
            Err(e) => self.notices.push(Notice::error(e.to_string())),
        }
    }

    fn pick_picture(&self) {
        crate::file_dialog::FileDialog::new()
            .set_title(tr("nb.add_picture"))
            .add_filter(tr("pic.files"), &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
            .pick_file(crate::file_dialog::in_view(|v: &mut NoteView, path: std::path::PathBuf| v.add_picture_file(&path)));
    }

    fn pick_map(&self) {
        crate::file_dialog::FileDialog::new()
            .set_title(tr("nb.map_set"))
            .add_filter(tr("pic.files"), &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
            .pick_file(crate::file_dialog::in_view(|v: &mut NoteView, path: std::path::PathBuf| v.set_map_file(&path)));
    }

    /// A picture file as the map of the canvas open.
    pub fn set_map_file(&mut self, path: &Path) {
        let Some(b) = &mut self.board else { return };
        match std::fs::read(path) {
            Ok(bytes) => {
                b.set_map(bytes);
                self.write_board();
            }
            Err(e) => self.notices.push(Notice::error(e.to_string())),
        }
    }

    /// A marker of `page` at a point of the canvas open, for the finder.
    pub(super) fn put_pin(&mut self, page: i64, x: f32, y: f32) {
        if let Some(b) = &mut self.board {
            b.add_pin(page, vec2(x, y));
            self.write_board();
        }
    }

    /// A marker's color: its page's type, or else its section.
    fn pin_color(&self, page: i64) -> Color32 {
        let p = self.nb.page(page);
        match p.and_then(|p| p.ptype).and_then(|t| self.nb.page_type(t)) {
            Some(t) => rgb(t.color),
            None => p.and_then(|p| self.nb.section(p.section)).map_or(Color32::GRAY, |s| section_color(s.color)),
        }
    }

    /// The finder for a page card at the spot.
    fn pick_card(&mut self) {
        let at = self.board.as_ref().map_or(Vec2::ZERO, |b| b.spot);
        self.finder = Some(links::Finder::new(links::Want::Card(at.x, at.y)));
    }

    /// A page card for the finder's choice.
    pub(super) fn put_card(&mut self, page: i64, x: f32, y: f32) {
        if let Some(b) = &mut self.board {
            b.add_card(page, vec2(x, y));
            self.write_board();
        }
    }

    pub(super) fn board_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut b) = self.board.take() else { return };
        let mut open = None;
        let mut want_card = false;
        let mut want_picture = false;
        let mut want_pin = false;
        let mut want_map = false;
        // The bar.
        egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin { left: 24, right: 24, top: 2, bottom: 6 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("+ {}", tr("nb.add_note"))).clicked() {
                    let id = b.add_note(b.spot, "");
                    b.start_editing(id);
                }
                if ui.button(format!("+ {}…", tr("nb.page"))).clicked() {
                    want_card = true;
                }
                if ui.button(format!("+ {}…", tr("nb.picture"))).clicked() {
                    want_picture = true;
                }
                if ui.button(format!("+ {}", tr("nb.group"))).on_hover_text(tr("nb.group_tip")).clicked() {
                    let id = b.add_group(&tr("nb.group"));
                    b.start_editing(id);
                }
                // Markers belong to a map.
                if b.canvas.map().is_some() && ui.button(format!("+ {}…", tr("nb.pin"))).on_hover_text(tr("nb.pin_tip")).clicked() {
                    b.spot = b.center;
                    want_pin = true;
                }
                ui.separator();
                ui.label(RichText::new(tr("nb.arrows")).weak());
                egui::ComboBox::from_id_salt(("board_rel", self.uid)).selected_text(rel_name(b.rel)).show_ui(ui, |ui| {
                    for rel in Rel::ALL {
                        ui.selectable_value(&mut b.rel, rel, rel_name(rel));
                    }
                });
                ui.separator();
                if ui.add_enabled(!b.undo.is_empty(), egui::Button::new(tr("edit.undo"))).clicked() {
                    b.undo();
                }
                if ui.add_enabled(!b.redo.is_empty(), egui::Button::new(tr("edit.redo"))).clicked() {
                    b.redo();
                }
                if ui.button(tr("nb.graph_fit")).clicked() {
                    b.fit = true;
                }
                ui.label(RichText::new(tr("nb.board_hint")).weak().small());
            });
        });
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if b.fit {
            b.fit_to(rect);
            b.fit = false;
        }
        let mid = rect.center();
        let to_screen = |p: Pos2, b: &Board| mid + (p.to_vec2() - b.center) * b.zoom;
        let to_canvas = |p: Pos2, b: &Board| b.center + (p - mid) / b.zoom;
        let screen_rect = |r: [f32; 4], b: &Board| Rect::from_min_max(to_screen(pos2(r[0], r[1]), b), to_screen(pos2(r[0] + r[2], r[1] + r[3]), b));
        // A marker is the same size at any zoom, above its point.
        let item_rect = |i: &apex_doc::notebook::canvas::Item, b: &Board| match i.kind {
            ItemKind::Pin(_) => {
                let tip = to_screen(pos2(i.rect[0], i.rect[1]), b);
                Rect::from_min_max(tip - vec2(PIN.x / 2.0, PIN.y), tip + vec2(PIN.x / 2.0, 0.0))
            }
            _ => screen_rect(i.rect, b),
        };
        let hover = resp.hover_pos();
        // A page dragged from the list: a card where it is let go, or a
        // marker on a map.
        let pointer = ui.ctx().pointer_latest_pos().filter(|p| rect.contains(*p));
        let page_in = match self.drag {
            Some(Naming::Page(p)) if p != b.page && pointer.is_some() => Some(p),
            _ => None,
        };
        if let (Some(p), Some(at)) = (page_in, pointer)
            && ui.input(|i| i.pointer.any_released())
        {
            self.drag = None;
            let at = to_canvas(at, &b);
            if b.canvas.map().is_some() {
                b.add_pin(p, at);
            } else {
                b.add_card(p, at);
            }
        }
        if let Some(h) = hover {
            b.spot = to_canvas(h, &b);
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let at = to_canvas(h, &b);
                b.zoom = (b.zoom * (scroll / 300.0).exp()).clamp(0.2, 3.0);
                b.center = at - (h - mid) / b.zoom;
            }
        } else if !rect.contains(ui.input(|i| i.pointer.hover_pos()).unwrap_or(Pos2::ZERO)) {
            b.spot = b.center;
        }
        // What is under the pointer: an item, its dot, its corner, an arrow.
        let item_at = |p: Pos2, b: &Board| {
            let pin = b.canvas.items.iter().rev().find(|i| matches!(i.kind, ItemKind::Pin(_)) && item_rect(i, b).contains(p));
            let c = to_canvas(p, b);
            pin.map(|i| i.id).or_else(|| b.canvas.item_at(c.x, c.y))
        };
        let dot_of = |r: Rect| pos2(r.max.x + DOT + 2.0, r.center().y);
        let dot_at = |p: Pos2, b: &Board| b.canvas.items.iter().rev().filter(|i| !matches!(i.kind, ItemKind::Map(_) | ItemKind::Pin(_))).find(|i| (dot_of(item_rect(i, b)) - p).length() <= DOT + 3.0).map(|i| i.id);
        let corner_at = |p: Pos2, b: &Board| b.selected.iter().copied().find(|&s| b.canvas.item(s).is_some_and(|i| !matches!(i.kind, ItemKind::Pin(_)) && (item_rect(i, b).max - p).length() <= 9.0));
        let arrow_ends = |a: &apex_doc::notebook::canvas::Arrow, b: &Board| {
            let (f, t) = (item_rect(b.canvas.item(a.from)?, b), item_rect(b.canvas.item(a.to)?, b));
            Some((edge(f, t.center()), edge(t, f.center())))
        };
        let arrow_at = |p: Pos2, b: &Board| b.canvas.arrows.iter().rev().find(|a| arrow_ends(a, b).is_some_and(|(x, y)| to_segment(p, x, y) <= 6.0)).map(|a| a.id);
        let hovered = hover.and_then(|h| item_at(h, &b));
        let shift = ui.input(|i| i.modifiers.shift);
        // Dragging.
        // Where the press was, not where the drag got going.
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        {
            b.finish_editing();
            b.last = p;
            b.before = Some(b.canvas.clone());
            b.drag = Some(if let Some(from) = dot_at(p, &b) {
                Drag::Connect { from, to: p }
            } else if let Some(id) = corner_at(p, &b) {
                Drag::Resize(id)
            } else if let Some(id) = item_at(p, &b) {
                if !b.selected.contains(&id) {
                    if !shift {
                        b.selected.clear();
                    }
                    b.selected.push(id);
                }
                Drag::Move
            } else if shift {
                Drag::Select { from: p, to: p }
            } else {
                b.selected.clear();
                Drag::Pan
            });
        }
        if resp.dragged()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let d = (at - b.last) / b.zoom;
            b.last = at;
            let p = Some(at);
            match &mut b.drag {
                Some(Drag::Pan) => b.center -= d,
                Some(Drag::Move) => {
                    let ids = b.moving();
                    for id in ids {
                        if let Some(i) = b.canvas.item_mut(id) {
                            i.rect[0] += d.x;
                            i.rect[1] += d.y;
                        }
                    }
                }
                Some(Drag::Resize(id)) => {
                    let id = *id;
                    if let Some(i) = b.canvas.item_mut(id) {
                        // Pictures keep their shape.
                        let ratio = matches!(i.kind, ItemKind::Picture(_)).then(|| i.rect[3] / i.rect[2].max(1.0));
                        i.rect[2] = (i.rect[2] + d.x).max(MIN_SIZE);
                        i.rect[3] = match ratio {
                            Some(r) => i.rect[2] * r,
                            None => (i.rect[3] + d.y).max(MIN_SIZE),
                        };
                    }
                }
                Some(Drag::Connect { to, .. } | Drag::Select { to, .. }) => {
                    if let Some(p) = p {
                        *to = p;
                    }
                }
                None => {}
            }
        }
        if resp.drag_stopped() {
            match b.drag.take() {
                Some(Drag::Connect { from, to }) => {
                    if let Some(target) = item_at(to, &b) {
                        let rel = b.rel;
                        b.canvas.connect(from, target, rel);
                    }
                }
                Some(Drag::Select { from, to }) => {
                    let area = Rect::from_two_pos(from, to);
                    let found: Vec<i64> = b.canvas.items.iter().filter(|i| !matches!(i.kind, ItemKind::Map(_)) && area.contains_rect(item_rect(i, &b))).map(|i| i.id).collect();
                    for f in found {
                        if !b.selected.contains(&f) {
                            b.selected.push(f);
                        }
                    }
                }
                _ => {}
            }
            if let Some(before) = b.before.take() {
                b.step(before);
            }
        }
        // Clicks.
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            b.finish_editing();
            let hit = item_at(p, &b).or_else(|| arrow_at(p, &b));
            match hit {
                Some(id) if shift => {
                    if let Some(at) = b.selected.iter().position(|&s| s == id) {
                        b.selected.remove(at);
                    } else {
                        b.selected.push(id);
                    }
                }
                Some(id) => b.selected = vec![id],
                None => b.selected.clear(),
            }
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            match item_at(p, &b).or_else(|| arrow_at(p, &b)) {
                Some(id) => match b.canvas.item(id).map(|i| &i.kind) {
                    Some(ItemKind::Page(page) | ItemKind::Pin(page)) => open = Some(*page),
                    _ => b.start_editing(id),
                },
                None => {
                    let at = to_canvas(p, &b);
                    let id = b.add_note(at, "");
                    b.start_editing(id);
                }
            }
        }
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            b.menu = item_at(p, &b).or_else(|| arrow_at(p, &b));
            if let Some(m) = b.menu
                && !b.selected.contains(&m)
            {
                b.selected = vec![m];
            }
        }
        // Keys, when nothing else takes them.
        let free = ui.memory(|m| m.focused().is_none()) && b.editing.is_none() && self.finder.is_none();
        if free && hover.is_some() {
            let (del, undo, redo) = ui.input_mut(|i| {
                let del = i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace);
                let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
                let redo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
                (del, undo, redo)
            });
            if del {
                b.delete_selected();
            }
            if undo {
                b.undo();
            }
            if redo {
                b.redo();
            }
            // Pasted: words become a note, a picture a picture.
            let (pasted, ctrl_v) = ui.input(|i| {
                let text = i.events.iter().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None });
                let v = i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::V, pressed: false, modifiers: m, .. } if m.command && !m.shift && !m.alt));
                (text, v)
            });
            if let Some(t) = pasted.filter(|t| !t.trim().is_empty()) {
                b.add_note(b.spot, t.trim());
            } else if ctrl_v && b.incoming.is_none() {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = ui.ctx().clone();
                if std::thread::Builder::new()
                    .name("apex-clipboard".into())
                    .spawn(move || {
                        let _ = tx.send(crate::images::read_clipboard_image());
                        ctx.request_repaint();
                    })
                    .is_ok()
                {
                    b.incoming = Some(rx);
                }
            }
        }
        if let Some(rx) = &b.incoming {
            match rx.try_recv() {
                Ok(got) => {
                    b.incoming = None;
                    if let Some(bytes) = got {
                        b.add_picture(bytes, b.spot);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => b.incoming = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        // Drawing.
        let painter = ui.painter_at(rect);
        let v = ui.visuals().clone();
        painter.rect_filled(rect, 0.0, v.extreme_bg_color);
        // A dotted grid, to see it move.
        let gap = 40.0 * b.zoom;
        if gap >= 12.0 {
            let origin = to_screen(Pos2::ZERO, &b);
            let x0 = rect.min.x + (origin.x - rect.min.x).rem_euclid(gap);
            let y0 = rect.min.y + (origin.y - rect.min.y).rem_euclid(gap);
            let dot = v.weak_text_color().gamma_multiply(0.35);
            let mut y = y0;
            while y < rect.max.y {
                let mut x = x0;
                while x < rect.max.x {
                    painter.circle_filled(pos2(x, y), 1.0, dot);
                    x += gap;
                }
                y += gap;
            }
        }
        let accent = theme::NOTE_ACCENT;
        let font = |s: f32| egui::FontId::proportional(s * b.zoom);
        let dark = Color32::from_gray(30);
        // The map, groups behind, then arrows, then the rest.
        if let Some(m) = b.canvas.map()
            && let ItemKind::Map(bytes) = &m.kind
        {
            let tex = crate::images::texture(ui.ctx(), crate::images::key(bytes), bytes, false);
            crate::images::paint(&painter, tex.as_ref(), screen_rect(m.rect, &b), 0.0, [0.0; 4]);
        }
        for i in b.canvas.items.iter().filter(|i| matches!(i.kind, ItemKind::Group(_))) {
            let r = item_rect(i, &b);
            let c = rgb(i.color);
            painter.rect(r, 8.0 * b.zoom, c.gamma_multiply(0.35), Stroke::new(1.5, c), StrokeKind::Inside);
            if let ItemKind::Group(t) = &i.kind
                && b.editing.as_ref().is_none_or(|e| e.0 != i.id)
            {
                let g = painter.layout(t.clone(), font(15.0), v.text_color(), (r.width() - 16.0).max(10.0));
                painter.with_clip_rect(r).galley(r.min + vec2(10.0, 7.0) * b.zoom, g, v.text_color());
            }
        }
        for a in &b.canvas.arrows {
            let Some((from, to)) = arrow_ends(a, &b) else { continue };
            let picked = b.selected.contains(&a.id);
            let c = rel_color(a.rel);
            let w = if picked { 3.5 } else if a.rel == Rel::Link { 1.5 } else { 2.2 };
            let dir = (to - from).normalized();
            painter.line_segment([from, to - dir * 6.0], Stroke::new(w, c));
            let side = vec2(-dir.y, dir.x) * 5.0;
            painter.add(egui::Shape::convex_polygon(vec![to, to - dir * 11.0 + side, to - dir * 11.0 - side], c, Stroke::NONE));
            let label = if a.label.is_empty() && a.rel != Rel::Link { rel_name(a.rel) } else { a.label.clone() };
            if !label.is_empty() && b.editing.as_ref().is_none_or(|e| e.0 != a.id) {
                let g = painter.layout_no_wrap(label, egui::FontId::proportional(12.0), c);
                let at = from + (to - from) / 2.0 - g.size() / 2.0;
                painter.rect_filled(Rect::from_min_size(at, g.size()).expand(3.0), 4.0, v.extreme_bg_color);
                painter.galley(at, g, c);
            }
        }
        for i in &b.canvas.items {
            let r = item_rect(i, &b);
            if !r.intersects(rect) {
                continue;
            }
            let pad = 10.0 * b.zoom;
            match &i.kind {
                ItemKind::Group(_) | ItemKind::Map(_) => {}
                ItemKind::Pin(p) => {
                    let title = if self.nb.is_live(*p) { self.title_of(*p) } else { tr("nb.link_gone").to_string() };
                    paint_pin(&painter, r, self.pin_color(*p), &title, b.selected.contains(&i.id), &v);
                    continue;
                }
                ItemKind::Note(t) => {
                    painter.rect(r, 6.0 * b.zoom, rgb(i.color), Stroke::new(1.0, Color32::from_black_alpha(40)), StrokeKind::Inside);
                    if b.editing.as_ref().is_none_or(|e| e.0 != i.id) {
                        let g = painter.layout(t.clone(), font(14.0), dark, (r.width() - 2.0 * pad).max(10.0));
                        painter.with_clip_rect(r.shrink(pad / 2.0)).galley(r.min + Vec2::splat(pad), g, dark);
                    }
                }
                ItemKind::Page(p) => {
                    let gone = !self.nb.is_live(*p);
                    let band = self.nb.page(*p).and_then(|p| self.nb.section(p.section)).map_or(Color32::GRAY, |s| section_color(s.color));
                    painter.rect(r, 6.0 * b.zoom, v.window_fill, Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
                    painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 6.0 * b.zoom)), egui::CornerRadius { nw: (6.0 * b.zoom) as u8, ne: (6.0 * b.zoom) as u8, sw: 0, se: 0 }, band);
                    let clip = painter.with_clip_rect(r.shrink(pad / 2.0));
                    let title = if gone { tr("nb.link_gone").to_string() } else { self.title_of(*p) };
                    let g = painter.layout(title, egui::FontId::proportional(15.0 * b.zoom), v.strong_text_color(), (r.width() - 2.0 * pad).max(10.0));
                    let th = g.size().y;
                    clip.galley(r.min + vec2(pad, pad + 4.0 * b.zoom), g, v.text_color());
                    let snippet: String = self.nb.texts.get(p).map(|t| t.chars().take(400).collect()).unwrap_or_default();
                    let g = painter.layout(snippet, font(12.0), v.weak_text_color(), (r.width() - 2.0 * pad).max(10.0));
                    clip.galley(r.min + vec2(pad, pad + 10.0 * b.zoom + th), g, v.weak_text_color());
                }
                ItemKind::Picture(bytes) => {
                    let tex = crate::images::texture(ui.ctx(), crate::images::key(bytes), bytes, false);
                    crate::images::paint(&painter, tex.as_ref(), r, 0.0, [0.0; 4]);
                }
            }
            if b.selected.contains(&i.id) {
                painter.rect_stroke(r.expand(2.0), 6.0 * b.zoom, Stroke::new(2.0, accent), StrokeKind::Outside);
                painter.rect_filled(Rect::from_center_size(r.max, Vec2::splat(8.0)), 1.0, accent);
            }
            if hovered == Some(i.id) || matches!(b.drag, Some(Drag::Connect { from, .. }) if from == i.id) {
                painter.circle(dot_of(r), DOT, v.window_fill, Stroke::new(2.0, accent));
            }
        }
        match &b.drag {
            Some(Drag::Connect { from, to }) => {
                if let Some(f) = b.canvas.item(*from) {
                    let fr = item_rect(f, &b);
                    painter.line_segment([edge(fr, *to), *to], Stroke::new(2.0, rel_color(b.rel)));
                }
                if let Some(t) = item_at(*to, &b).and_then(|t| b.canvas.item(t)) {
                    painter.rect_stroke(item_rect(t, &b).expand(3.0), 6.0, Stroke::new(2.0, rel_color(b.rel)), StrokeKind::Outside);
                }
            }
            Some(Drag::Select { from, to }) => {
                let area = Rect::from_two_pos(*from, *to);
                painter.rect(area, 0.0, accent.gamma_multiply(0.12), Stroke::new(1.0, accent), StrokeKind::Inside);
            }
            _ => {}
        }
        if page_in.is_some() && self.drag.is_some() {
            painter.rect_stroke(rect.shrink(2.0), 4.0, Stroke::new(2.0, accent), StrokeKind::Inside);
        }
        if b.canvas.items.is_empty() {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, tr("nb.board_empty"), egui::FontId::proportional(14.0), v.weak_text_color());
        }
        if let Some(h) = hover {
            if dot_at(h, &b).is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            } else if corner_at(h, &b).is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            } else if hovered.is_some_and(|h| matches!(b.canvas.item(h).map(|i| &i.kind), Some(ItemKind::Page(_) | ItemKind::Pin(_)))) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }

        // Typing the words of a note, a group or an arrow, in place.
        if let Some((id, mut text)) = b.editing.take() {
            let place = match b.canvas.item(id) {
                Some(i) if matches!(i.kind, ItemKind::Group(_)) => {
                    let r = item_rect(i, &b);
                    Some((Rect::from_min_size(r.min + vec2(6.0, 4.0), vec2((r.width() - 12.0).max(40.0), 26.0)), false, v.text_color()))
                }
                Some(i) => Some((item_rect(i, &b).shrink(6.0), true, dark)),
                None => b.canvas.arrows.iter().find(|a| a.id == id).and_then(|a| arrow_ends(a, &b)).map(|(f, t)| (Rect::from_center_size(f + (t - f) / 2.0, vec2(160.0, 24.0)), false, v.text_color())),
            };
            if let Some((r, multi, color)) = place {
                let edit = if multi { egui::TextEdit::multiline(&mut text) } else { egui::TextEdit::singleline(&mut text) };
                let te = ui.put(r, edit.frame(egui::Frame::NONE).text_color(color).font(egui::FontId::proportional(14.0 * b.zoom.min(1.5))).desired_width(r.width()));
                if b.focus_edit {
                    te.request_focus();
                    b.focus_edit = false;
                }
                let done = te.lost_focus() || (!multi && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if done {
                    if b.words(id).is_some_and(|t| t != text) {
                        b.set_words(id, text);
                    }
                } else {
                    b.editing = Some((id, text));
                }
            }
        }

        // The context menu.
        let mut delete = false;
        resp.context_menu(|ui| {
            let Some(m) = b.menu else {
                if ui.button(tr("nb.add_note")).clicked() {
                    let id = b.add_note(b.spot, "");
                    b.start_editing(id);
                    ui.close();
                }
                if ui.button(format!("{}…", tr("nb.page"))).clicked() {
                    want_card = true;
                    ui.close();
                }
                if ui.button(format!("{}…", tr("nb.picture"))).clicked() {
                    want_picture = true;
                    ui.close();
                }
                ui.separator();
                let has_map = b.canvas.map().is_some();
                if has_map && ui.button(format!("{}…", tr("nb.pin_here"))).clicked() {
                    want_pin = true;
                    ui.close();
                }
                if ui.button(format!("{}…", tr(if has_map { "nb.map_change" } else { "nb.map_set" }))).clicked() {
                    want_map = true;
                    ui.close();
                }
                if has_map && ui.button(tr("nb.map_remove")).clicked() {
                    b.remove_map();
                    ui.close();
                }
                return;
            };
            if let Some(i) = b.canvas.item(m).cloned() {
                match i.kind {
                    ItemKind::Page(p) => {
                        if self.nb.is_live(p) && ui.button(tr("nb.open_page")).clicked() {
                            open = Some(p);
                            ui.close();
                        }
                    }
                    ItemKind::Note(_) | ItemKind::Group(_) => {
                        if ui.button(tr("nb.edit_text")).clicked() {
                            b.start_editing(m);
                            ui.close();
                        }
                        ui.horizontal(|ui| {
                            ui.label(tr("nb.color"));
                            for c in NOTE_COLORS {
                                let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                                ui.painter().rect(r, 4.0, rgb(c), Stroke::new(if c == i.color { 2.0 } else { 1.0 }, if c == i.color { accent } else { Color32::from_black_alpha(60) }), StrokeKind::Inside);
                                if resp.clicked() {
                                    b.edit(|cv| {
                                        if let Some(it) = cv.item_mut(m) {
                                            it.color = c;
                                        }
                                    });
                                    ui.close();
                                }
                            }
                        });
                    }
                    ItemKind::Pin(p) => {
                        if self.nb.is_live(p) && ui.button(tr("nb.open_page")).clicked() {
                            open = Some(p);
                            ui.close();
                        }
                    }
                    ItemKind::Picture(_) | ItemKind::Map(_) => {}
                }
                if b.selected.len() > 1 && ui.button(tr("nb.group")).clicked() {
                    let id = b.add_group(&tr("nb.group"));
                    b.start_editing(id);
                    ui.close();
                }
            } else if let Some(a) = b.canvas.arrows.iter().find(|a| a.id == m).cloned() {
                crate::widgets::menu_button(ui, tr("nb.relation"), |ui| {
                    for rel in Rel::ALL {
                        if ui.radio(a.rel == rel, rel_name(rel)).clicked() {
                            b.edit(|cv| {
                                if let Some(x) = cv.arrows.iter_mut().find(|x| x.id == m) {
                                    x.rel = rel;
                                }
                            });
                            ui.close();
                        }
                    }
                });
                if ui.button(tr("nb.arrow_label")).clicked() {
                    b.start_editing(m);
                    ui.close();
                }
                if ui.button(tr("nb.arrow_reverse")).clicked() {
                    b.edit(|cv| {
                        let taken = cv.arrows.iter().any(|x| x.from == a.to && x.to == a.from);
                        if let Some(x) = cv.arrows.iter_mut().find(|x| x.id == m)
                            && !taken
                        {
                            std::mem::swap(&mut x.from, &mut x.to);
                        }
                    });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(tr("nb.remove_from_board")).clicked() {
                delete = true;
                ui.close();
            }
        });
        if delete {
            b.delete_selected();
        }

        let dirty = b.dirty;
        self.board = Some(b);
        if dirty {
            self.write_board();
        }
        if want_card {
            self.pick_card();
        }
        if want_picture {
            self.pick_picture();
        }
        if want_pin {
            let at = self.board.as_ref().map_or(Vec2::ZERO, |b| b.spot);
            self.finder = Some(links::Finder::new(links::Want::Pin(at.x, at.y)));
        }
        if want_map {
            self.pick_map();
        }
        if let Some(p) = open {
            self.open_page(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_undoes_its_changes_and_groups_what_is_picked() {
        let mut b = Board::new(1, Canvas::default());
        let n = b.add_note(vec2(0.0, 0.0), "uno");
        let p = b.add_card(7, vec2(400.0, 0.0));
        b.edit(|c| {
            c.connect(n, p, Rel::Supports);
        });
        assert_eq!(b.canvas.arrows.len(), 1);
        b.selected = vec![n, p];
        let g = b.add_group("Grupo");
        assert_eq!(b.canvas.inside(g).len(), 2);
        // Groups move what they hold.
        b.selected = vec![g];
        assert_eq!(b.moving().len(), 3);
        b.undo();
        assert!(b.canvas.item(g).is_none() && b.selected.is_empty());
        b.selected = vec![n];
        b.delete_selected();
        assert!(b.canvas.arrows.is_empty());
        b.undo();
        assert_eq!(b.canvas.arrows.len(), 1);
        b.redo();
        assert!(b.canvas.item(n).is_none());
        // An arrow's line leaves its card at the edge.
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 50.0));
        assert_eq!(edge(r, pos2(500.0, 25.0)), pos2(100.0, 25.0));
    }

    #[test]
    fn markers_put_on_the_same_spot_stand_apart() {
        let mut b = Board::new(1, Canvas::default());
        let one = b.add_pin(7, vec2(50.0, 50.0));
        let two = b.add_pin(8, vec2(50.0, 50.0));
        let at = |id| b.canvas.item(id).map(|i| vec2(i.rect[0], i.rect[1])).unwrap_or_default();
        assert!((at(one) - at(two)).length() > 20.0);
    }
}
