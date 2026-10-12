//! Comments in the margin: a balloon beside the page for each comment with
//! its replies, the highlight of the commented text, and the commands of
//! the Review menu. The author name is a setting shared by all documents.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use apex_doc::{CommentSpan, Pos, Selection};
use eframe::egui::{self, Color32, Galley, Id, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};

use super::{DocView, ParaLayout, Segment, char_count};
use crate::i18n::tr;

// --- Author name ------------------------------------------------------------------------

static USER: Mutex<String> = Mutex::new(String::new());
static USER_CHANGED: AtomicBool = AtomicBool::new(false);
static USER_GEN: AtomicU64 = AtomicU64::new(0);

/// The author name of new comments and changes: the setting, or the
/// login name.
pub fn user_name() -> String {
    let set = USER.lock().map(|u| u.trim().to_string()).unwrap_or_default();
    if !set.is_empty() {
        return set;
    }
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| tr("comment.default_author").to_string())
}

/// The setting as saved (empty uses the login name).
pub fn user_setting() -> String {
    USER.lock().map(|u| u.clone()).unwrap_or_default()
}

/// Sets the author name; `changed` asks the app to save it.
pub fn set_user_name(name: &str, changed: bool) {
    if let Ok(mut u) = USER.lock() {
        *u = name.trim().to_string();
    }
    USER_GEN.fetch_add(1, Ordering::Relaxed);
    if changed {
        USER_CHANGED.store(true, Ordering::Relaxed);
    }
}

/// The name changed since the last call (the app saves it).
pub fn take_user_changed() -> bool {
    USER_CHANGED.swap(false, Ordering::Relaxed)
}

// --- State ------------------------------------------------------------------------------

const MARGIN: f32 = 290.0;
const BALLOON_W: f32 = 250.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Comment,
    Reply(usize),
    NewReply,
}

struct Editing {
    id: u32,
    target: Target,
    text: String,
    focus: bool,
}

pub(super) struct CommentState {
    pub show: bool,
    spans: Vec<CommentSpan>,
    spans_rev: u64,
    active: Option<u32>,
    last_sel: Selection,
    editing: Option<Editing>,
    /// Balloon heights from the last frame, for stacking them.
    heights: HashMap<u32, f32>,
    pub user_dialog: Option<String>,
    /// Painted balloons by comment: document revision and font generation,
    /// dark mode, layout.
    painted: HashMap<u32, ((u64, u64), bool, Arc<Painted>)>,
    /// The author name generation last copied into the document.
    user_gen: u64,
}

impl Default for CommentState {
    fn default() -> Self {
        CommentState { show: true, spans: Vec::new(), spans_rev: u64::MAX, active: None, last_sel: Selection::default(), editing: None, heights: HashMap::new(), user_dialog: None, painted: HashMap::new(), user_gen: u64::MAX }
    }
}

fn span_contains(s: &CommentSpan, p: Pos) -> bool {
    s.start <= p && p <= s.end
}

fn short_date(d: &str) -> String {
    // "2024-05-01T10:00:00Z" -> "2024-05-01 10:00"
    let d = d.trim_end_matches('Z');
    match d.split_once('T') {
        Some((day, time)) => format!("{day} {}", time.get(..5).unwrap_or(time)),
        None => d.to_string(),
    }
}

impl DocView {
    /// Width added beside the pages for the balloons.
    pub(super) fn comment_margin(&self) -> f32 {
        if self.comments.show && !self.doc.comments.is_empty() { MARGIN } else { 0.0 }
    }

    /// Keeps the comment ranges and the active comment current.
    pub(super) fn refresh_comments(&mut self) {
        let generation = USER_GEN.load(Ordering::Relaxed);
        if self.comments.user_gen != generation {
            self.comments.user_gen = generation;
            self.doc.user = user_name().into();
        }
        let c = &mut self.comments;
        if c.spans_rev != self.doc.revision {
            c.spans = self.doc.comment_spans();
            c.spans_rev = self.doc.revision;
            c.painted.retain(|id, _| c.spans.iter().any(|s| s.id == *id));
            if c.active.is_some_and(|id| !c.spans.iter().any(|s| s.id == id)) {
                c.active = None;
            }
            if c.editing.as_ref().is_some_and(|e| !c.spans.iter().any(|s| s.id == e.id)) {
                c.editing = None;
            }
        }
        if c.last_sel != self.sel {
            c.last_sel = self.sel;
            // The innermost comment around the caret becomes the active one.
            let p = self.sel.cursor;
            if let Some(s) = c.spans.iter().filter(|s| span_contains(s, p)).min_by_key(|s| (s.end.para - s.start.para, s.end.offset.wrapping_sub(s.start.offset))) {
                c.active = Some(s.id);
            }
        }
    }

    /// The comment the commands act on: the active one, or the one at the
    /// caret.
    fn current_comment(&self) -> Option<u32> {
        let p = self.sel.cursor;
        self.comments.active.filter(|id| self.doc.comment(*id).is_some()).or_else(|| self.comments.spans.iter().find(|s| span_contains(s, p)).map(|s| s.id))
    }

    /// Highlight of the commented text in a segment.
    pub(super) fn paint_comment_ranges(&self, p: &egui::Painter, seg: &Segment, pl: &ParaLayout, galley_pos: Pos2) {
        if !self.comments.show || self.comments.spans.is_empty() {
            return;
        }
        let text = &self.doc.paragraphs[seg.para].text;
        for s in &self.comments.spans {
            if s.start.para > seg.para || s.end.para < seg.para {
                continue;
            }
            let resolved = self.doc.comment(s.id).is_some_and(|c| c.resolved);
            let active = self.comments.active == Some(s.id);
            let color = match (resolved, active) {
                (true, false) => Color32::from_rgba_unmultiplied(0, 0, 0, 18),
                (true, true) => Color32::from_rgba_unmultiplied(0, 0, 0, 36),
                (false, false) => Color32::from_rgba_unmultiplied(255, 196, 64, 70),
                (false, true) => Color32::from_rgba_unmultiplied(255, 160, 0, 120),
            };
            let from = if s.start.para == seg.para { char_count(text, s.start.offset) } else { 0 };
            let to = if s.end.para == seg.para { char_count(text, s.end.offset) } else { usize::MAX };
            self.paint_range(p, seg, pl, from, to, galley_pos, color);
        }
    }

    // --- Commands -----------------------------------------------------------------------

    pub(super) fn new_comment(&mut self) {
        let sel = self.sel_or_word();
        let id = self.doc.add_comment(sel, "");
        if id == 0 {
            return;
        }
        self.comments.show = true;
        self.comments.active = Some(id);
        self.comments.editing = Some(Editing { id, target: Target::Comment, text: String::new(), focus: true });
    }

    fn reply_to(&mut self, id: u32) {
        self.comments.active = Some(id);
        self.comments.editing = Some(Editing { id, target: Target::NewReply, text: String::new(), focus: true });
    }

    fn toggle_resolved(&mut self, id: u32) {
        let on = self.doc.comment(id).is_some_and(|c| !c.resolved);
        self.commit_edit();
        self.doc.set_comment_resolved(id, on, self.sel);
    }

    fn delete_comment(&mut self, id: u32) {
        self.comments.editing = None;
        self.doc.delete_comments(&[id], self.sel);
        self.refocus = true;
    }

    fn delete_all_comments(&mut self) {
        self.comments.editing = None;
        let ids: Vec<u32> = self.doc.comments.iter().map(|c| c.id).collect();
        self.doc.delete_comments(&ids, self.sel);
        self.refocus = true;
    }

    fn go_to_comment(&mut self, forward: bool) {
        let p = self.sel.cursor;
        let spans = &self.comments.spans;
        let next = if forward { spans.iter().find(|s| s.start > p).or(spans.first()) } else { spans.iter().rev().find(|s| s.start < p).or(spans.last()) };
        let Some(s) = next.cloned() else {
            self.notices.push(crate::app::Notice::info(tr("comment.none")));
            return;
        };
        self.comments.show = true;
        self.set_sel(Selection::caret(s.start));
        self.comments.last_sel = self.sel;
        self.comments.active = Some(s.id);
    }

    /// Saves the text being edited in a balloon.
    fn commit_edit(&mut self) {
        let Some(e) = self.comments.editing.take() else { return };
        let Some(c) = self.doc.comment(e.id) else { return };
        let text = e.text.trim_end().to_string();
        match e.target {
            Target::Comment if c.text != text => {
                self.doc.set_comment_text(e.id, &text, self.sel);
            }
            Target::Reply(k) if c.replies.get(k).is_some_and(|r| r.text != text) => {
                if text.is_empty() {
                    self.doc.delete_reply(e.id, k, self.sel);
                } else {
                    self.doc.set_reply_text(e.id, k, &text, self.sel);
                }
            }
            Target::NewReply if !text.is_empty() => {
                self.doc.add_reply(e.id, &text, self.sel);
            }
            _ => {}
        }
    }

    pub(super) fn comment_key(&mut self, key: Key, m: Modifiers) -> bool {
        if key == Key::M && m.command && m.alt && !m.shift {
            self.new_comment();
            return true;
        }
        false
    }

    // --- Menus --------------------------------------------------------------------------

    pub(super) fn comment_menu_items(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(tr("comment.new")).shortcut_text("Ctrl+Alt+M")).clicked() {
            self.new_comment();
            ui.close();
        }
        let cur = self.current_comment();
        let any = !self.doc.comments.is_empty();
        ui.add_enabled_ui(cur.is_some(), |ui| {
            let resolved = cur.and_then(|id| self.doc.comment(id)).is_some_and(|c| c.resolved);
            if ui.button(tr("comment.reply")).clicked() {
                if let Some(id) = cur {
                    self.reply_to(id);
                }
                ui.close();
            }
            if ui.button(tr(if resolved { "comment.reopen" } else { "comment.resolve" })).clicked() {
                if let Some(id) = cur {
                    self.toggle_resolved(id);
                }
                ui.close();
            }
            if ui.button(tr("comment.delete")).clicked() {
                if let Some(id) = cur {
                    self.delete_comment(id);
                }
                ui.close();
            }
        });
        ui.add_enabled_ui(any, |ui| {
            if ui.button(tr("comment.delete_all")).clicked() {
                self.delete_all_comments();
                ui.close();
            }
            if ui.button(tr("comment.previous")).clicked() {
                self.go_to_comment(false);
                ui.close();
            }
            if ui.button(tr("comment.next")).clicked() {
                self.go_to_comment(true);
                ui.close();
            }
        });
        if ui.checkbox(&mut self.comments.show, tr("comment.show")).clicked() {
            ui.close();
        }
    }

    pub(super) fn user_name_item(&mut self, ui: &mut egui::Ui) {
        let label = format!("{}…", tr("comment.user_name"));
        // The name in use, so it shows what the setting does.
        let tip = format!("{} {}", tr("comment.user_name_hint"), user_name());
        if crate::toolstrip::large(ui, crate::icons::Icon::Pencil, &label, &tip, crate::toolstrip::accent(), false).clicked() {
            self.comments.user_dialog = Some(user_name());
        }
    }

    /// Context menu entries for the comment at the caret.
    pub(super) fn comment_context_items(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(tr("comment.new")).shortcut_text("Ctrl+Alt+M")).clicked() {
            self.new_comment();
            ui.close();
        }
        let p = self.sel.cursor;
        let Some(id) = self.comments.spans.iter().find(|s| span_contains(s, p)).map(|s| s.id) else { return };
        self.balloon_menu(ui, id);
    }

    fn balloon_menu(&mut self, ui: &mut egui::Ui, id: u32) {
        let resolved = self.doc.comment(id).is_some_and(|c| c.resolved);
        if ui.button(tr("comment.reply")).clicked() {
            self.reply_to(id);
            ui.close();
        }
        if ui.button(tr(if resolved { "comment.reopen" } else { "comment.resolve" })).clicked() {
            self.toggle_resolved(id);
            ui.close();
        }
        if ui.button(tr("comment.delete")).clicked() {
            self.delete_comment(id);
            ui.close();
        }
    }

    pub(super) fn user_name_dialog(&mut self, ctx: &egui::Context) {
        let Some(name) = &mut self.comments.user_dialog else { return };
        let mut open = true;
        let mut done = None;
        egui::Window::new(tr("comment.user_name")).id(Id::new(("user_name", self.uid))).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(tr("comment.user_name_hint"));
            let r = ui.add(egui::TextEdit::singleline(name).desired_width(260.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                done = Some(true);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(tr("dialog.ok")).clicked() {
                    done = Some(true);
                }
                if ui.button(tr("dialog.cancel")).clicked() {
                    done = Some(false);
                }
            });
        });
        if done.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            done = Some(false);
        }
        if done == Some(true) {
            set_user_name(name, true);
        }
        if !open || done.is_some() {
            self.comments.user_dialog = None;
            self.refocus = true;
        }
    }

    // --- Balloons -----------------------------------------------------------------------

    /// The balloons in the margin right of the pages, with their connector
    /// lines. `origin` is the canvas origin on screen.
    pub(super) fn comment_balloons(&mut self, ui: &mut egui::Ui, origin: Pos2, viewport: egui::Rect) {
        if self.comment_margin() == 0.0 || self.comments.spans.is_empty() {
            return;
        }
        let off = origin.to_vec2();
        let page_right = self.page_x + self.layout.row_w.max(self.layout.page.x);
        let x0 = page_right + 18.0;
        let visible = viewport.expand2(vec2(0.0, 400.0));
        // Stack the balloons from their anchors down.
        let mut placed = Vec::with_capacity(self.comments.spans.len());
        let mut bottom = f32::MIN;
        for s in &self.comments.spans {
            let anchor = self.caret_rect(self.doc.clamp(s.start));
            let h = self.comments.heights.get(&s.id).copied().unwrap_or(64.0);
            let y = anchor.min.y.max(bottom + 8.0);
            bottom = y + h;
            placed.push((s.id, anchor, y, h));
        }
        let accent = Color32::from_rgb(0xE0, 0x8A, 0x00);
        let faint = Color32::from_rgba_unmultiplied(0xE0, 0x8A, 0x00, 90);
        let grey = Color32::from_gray(160);
        let mut actions: Vec<(u32, Action)> = Vec::new();
        for (id, anchor, y, h) in placed {
            let rect = Rect::from_min_size(pos2(x0, y), vec2(BALLOON_W, h));
            if !rect.intersects(visible) && !anchor.intersects(visible) {
                continue;
            }
            let Some(c) = self.doc.comment(id).cloned() else { continue };
            let active = self.comments.active == Some(id);
            // Connector: along the anchor's line to the page edge, then to
            // the balloon.
            let line = if active { Stroke::new(1.5, accent) } else { Stroke::new(1.0, if c.resolved { grey } else { faint }) };
            let a = pos2(anchor.min.x, anchor.max.y) + off;
            let edge = pos2(page_right, anchor.max.y) + off;
            let p = ui.painter();
            p.line_segment([a, edge], line);
            p.line_segment([edge, pos2(x0, y + 12.0) + off], line);
            let screen = rect.translate(off);
            // The balloon's background takes clicks meant for it.
            let bg = ui.interact(screen, Id::new(("balloon", self.uid, id)), Sense::click());
            if bg.clicked() {
                actions.push((id, Action::Activate));
            }
            bg.context_menu(|ui| {
                let resolved = c.resolved;
                if ui.button(tr("comment.reply")).clicked() {
                    actions.push((id, Action::Reply));
                    ui.close();
                }
                if ui.button(tr(if resolved { "comment.reopen" } else { "comment.resolve" })).clicked() {
                    actions.push((id, Action::Resolve));
                    ui.close();
                }
                if ui.button(tr("comment.delete")).clicked() {
                    actions.push((id, Action::Delete));
                    ui.close();
                }
            });
            let fill = if c.resolved { ui.visuals().faint_bg_color } else { ui.visuals().window_fill };
            let stroke = if active { Stroke::new(1.5, accent) } else { ui.visuals().widgets.noninteractive.bg_stroke };
            let text_color = if c.resolved { ui.visuals().weak_text_color() } else { ui.visuals().text_color() };
            // Balloons that aren't being worked on are painted from a cache
            // (cheaper than widgets on every frame).
            if !active && self.comments.editing.as_ref().is_none_or(|e| e.id != id) {
                let dark = ui.visuals().dark_mode;
                let rev = (self.doc.revision, crate::fonts::generation());
                let cached = self.comments.painted.get(&id).filter(|(r, d, _)| *r == rev && *d == dark).map(|(_, _, p)| p.clone());
                let painted = cached.unwrap_or_else(|| {
                    let p = std::sync::Arc::new(paint_layout(ui, &c, text_color));
                    self.comments.painted.insert(id, (rev, dark, p.clone()));
                    p
                });
                let frame = Rect::from_min_size(screen.min, vec2(BALLOON_W, painted.height));
                let p = ui.painter();
                p.rect(frame, 4.0, fill, stroke, egui::StrokeKind::Inside);
                for (at, g, _) in &painted.items {
                    p.galley(frame.min + at.to_vec2(), g.clone(), text_color);
                }
                if bg.clicked()
                    && let Some(pos) = bg.interact_pointer_pos()
                    && let Some((_, _, Some(t))) = painted.items.iter().find(|(at, g, t)| t.is_some() && Rect::from_min_size(frame.min + at.to_vec2(), g.size()).contains(pos))
                {
                    actions.push((id, Action::Edit(*t)));
                }
                if (painted.height - h).abs() > 0.5 {
                    self.comments.heights.insert(id, painted.height);
                    ui.ctx().request_repaint();
                }
                continue;
            }
            let editing = self.comments.editing.as_mut().filter(|e| e.id == id);
            let mut ed = editing.map(|e| (e.target, std::mem::take(&mut e.text), std::mem::replace(&mut e.focus, false)));
            let builder = egui::UiBuilder::new().max_rect(Rect::from_min_size(screen.min, vec2(BALLOON_W, 10_000.0))).id_salt(("balloon_ui", id));
            let r = ui.scope_builder(builder, |ui| {
                egui::Frame::new().fill(fill).stroke(stroke).corner_radius(4.0).inner_margin(8.0).show(ui, |ui| {
                    ui.set_width(BALLOON_W - 16.0);
                    ui.set_max_width(BALLOON_W - 16.0);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    let head = |ui: &mut egui::Ui, author: &str, date: &str| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(author).strong().color(super::revisions_ui::author_color(author)));
                            if !date.is_empty() {
                                ui.label(RichText::new(short_date(date)).small().weak());
                            }
                        });
                    };
                    head(ui, &c.author, &c.date);
                    if c.resolved {
                        ui.label(RichText::new(tr("comment.resolved")).small().italics().weak());
                    }
                    body(ui, id, Target::Comment, &c.text, text_color, &mut ed, &mut actions);
                    for (k, reply) in c.replies.iter().enumerate() {
                        ui.add_space(4.0);
                        ui.indent(("reply", k), |ui| {
                            head(ui, &reply.author, &reply.date);
                            body(ui, id, Target::Reply(k), &reply.text, text_color, &mut ed, &mut actions);
                        });
                    }
                    if let Some((Target::NewReply, text, focus)) = &mut ed {
                        ui.add_space(4.0);
                        edit_box(ui, id, Target::NewReply, text, focus, &mut actions);
                    }
                    if active && ed.is_none() {
                        ui.add_space(2.0);
                        ui.horizontal_wrapped(|ui| {
                            if ui.small_button(tr("comment.reply")).clicked() {
                                actions.push((id, Action::Reply));
                            }
                            if ui.small_button(tr(if c.resolved { "comment.reopen" } else { "comment.resolve" })).clicked() {
                                actions.push((id, Action::Resolve));
                            }
                            if ui.small_button(tr("comment.delete")).clicked() {
                                actions.push((id, Action::Delete));
                            }
                        });
                    }
                });
            });
            let height = r.response.rect.height();
            if (height - h).abs() > 0.5 {
                self.comments.heights.insert(id, height);
                ui.ctx().request_repaint();
            }
            if let (Some(e), Some((_, text, _))) = (self.comments.editing.as_mut().filter(|e| e.id == id), ed) {
                e.text = text;
            }
        }
        for (id, a) in actions {
            match a {
                Action::Activate => {
                    self.comments.active = Some(id);
                }
                Action::Edit(target) => {
                    self.commit_edit();
                    let text = self.doc.comment(id).map(|c| match target {
                        Target::Comment => c.text.clone(),
                        Target::Reply(k) => c.replies.get(k).map(|r| r.text.clone()).unwrap_or_default(),
                        Target::NewReply => String::new(),
                    });
                    if let Some(text) = text {
                        self.comments.active = Some(id);
                        self.comments.editing = Some(Editing { id, target, text, focus: true });
                    }
                }
                Action::Commit => {
                    self.commit_edit();
                    self.focus = true;
                }
                Action::Cancel => {
                    self.comments.editing = None;
                    self.focus = true;
                }
                Action::Reply => {
                    self.commit_edit();
                    self.reply_to(id);
                }
                Action::Resolve => self.toggle_resolved(id),
                Action::Delete => self.delete_comment(id),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Activate,
    Edit(Target),
    Commit,
    Cancel,
    Reply,
    Resolve,
    Delete,
}

type Edit = Option<(Target, String, bool)>;

/// A balloon laid out for painting: galleys at offsets from its corner
/// (the texts that edit on click have their target).
pub(super) struct Painted {
    items: Vec<(Pos2, Arc<Galley>, Option<Target>)>,
    height: f32,
}

fn paint_layout(ui: &egui::Ui, c: &apex_doc::Comment, text_color: Color32) -> Painted {
    let body_font = egui::TextStyle::Body.resolve(ui.style());
    let small_font = egui::TextStyle::Small.resolve(ui.style());
    let weak = ui.visuals().weak_text_color();
    let p = ui.painter();
    let pad = 8.0;
    let full = BALLOON_W - 2.0 * pad;
    let mut items = Vec::new();
    let mut y = pad;
    let head = |items: &mut Vec<(Pos2, Arc<Galley>, Option<Target>)>, y: &mut f32, x: f32, author: &str, date: &str| {
        let color = super::revisions_ui::author_color(author);
        let a = p.layout(author.to_string(), body_font.clone(), color, full - (x - pad));
        let aw = a.size().x;
        let ah = a.size().y;
        items.push((pos2(x, *y), a, None));
        if !date.is_empty() {
            let d = p.layout_no_wrap(short_date(date), small_font.clone(), weak);
            let dh = d.size().y;
            items.push((pos2(x + aw + 6.0, *y + (ah - dh) / 2.0), d, None));
        }
        *y += ah + 3.0;
    };
    head(&mut items, &mut y, pad, &c.author, &c.date);
    if c.resolved {
        let g = p.layout_no_wrap(tr("comment.resolved").to_string(), small_font.clone(), weak);
        y += g.size().y + 3.0;
        items.push((pos2(pad, y - g.size().y - 3.0), g, None));
    }
    let text = |t: &str, width: f32| {
        if t.is_empty() {
            let mut job = egui::text::LayoutJob::simple(tr("comment.placeholder").to_string(), body_font.clone(), weak, width);
            if let Some(s) = job.sections.first_mut() {
                s.format.italics = true;
            }
            p.layout_job(job)
        } else {
            p.layout(t.to_string(), body_font.clone(), text_color, width)
        }
    };
    let g = text(&c.text, full);
    let gh = g.size().y;
    items.push((pos2(pad, y), g, Some(Target::Comment)));
    y += gh + 3.0;
    let indent = pad + 18.0;
    for (k, r) in c.replies.iter().enumerate() {
        y += 4.0;
        head(&mut items, &mut y, indent, &r.author, &r.date);
        let g = text(&r.text, full - 18.0);
        let gh = g.size().y;
        items.push((pos2(indent, y), g, Some(Target::Reply(k))));
        y += gh + 3.0;
    }
    Painted { items, height: y - 3.0 + pad }
}

/// A comment's or reply's text: a label that edits on click, or the box
/// being edited.
fn body(ui: &mut egui::Ui, id: u32, target: Target, text: &str, color: Color32, ed: &mut Edit, actions: &mut Vec<(u32, Action)>) {
    if let Some((t, buf, focus)) = ed
        && *t == target
    {
        edit_box(ui, id, target, buf, focus, actions);
        return;
    }
    let shown = if text.is_empty() { RichText::new(tr("comment.placeholder")).italics().weak() } else { RichText::new(text).color(color) };
    let r = ui.add(egui::Label::new(shown).wrap().sense(Sense::click()));
    if r.on_hover_cursor(egui::CursorIcon::Text).clicked() {
        actions.push((id, Action::Edit(target)));
    }
}

fn edit_box(ui: &mut egui::Ui, id: u32, target: Target, text: &mut String, focus: &mut bool, actions: &mut Vec<(u32, Action)>) {
    let hint = if target == Target::NewReply { tr("comment.reply_hint") } else { tr("comment.placeholder") };
    let r = ui.add(egui::TextEdit::multiline(text).desired_rows(2).desired_width(f32::INFINITY).hint_text(hint).id(Id::new(("comment_edit", id, format!("{target:?}")))));
    if std::mem::take(focus) {
        r.request_focus();
    }
    let (esc, ctrl_enter) = ui.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter) && i.modifiers.command));
    if r.has_focus() && ctrl_enter {
        actions.push((id, Action::Commit));
    } else if r.lost_focus() {
        actions.push((id, if esc { Action::Cancel } else { Action::Commit }));
    }
    // Clicking elsewhere also saves; Esc cancels.
    if ui.small_button(tr(if target == Target::NewReply { "comment.post" } else { "dialog.ok" })).clicked() {
        actions.push((id, Action::Commit));
    }
}
