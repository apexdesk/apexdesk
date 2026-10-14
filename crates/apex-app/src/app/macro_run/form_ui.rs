//! The UserForms a running macro shows: each one a window over the
//! workbook with its controls where the design puts them (in points, as
//! in Excel). What the user does goes back to the macro as an
//! [`Event`] when the form's code handles it; other changes wait and go
//! with the next one, so typing in a box no code watches doesn't stop
//! for the macro at each key.

use std::collections::HashMap;
use std::sync::Arc;

use apex_vba::forms::{Action, Change, Event, Kind, View, handles};
use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Stroke, UiBuilder, Vec2, pos2, vec2};

/// Pixels of a point.
const SCALE: f32 = 4.0 / 3.0;
/// The height of a MultiPage's or TabStrip's tabs, in points.
const TABS: f32 = 18.0;
/// The title bar of a form, in pixels.
const TITLE: f32 = 26.0;

/// What the window keeps between frames while forms are shown.
#[derive(Default)]
pub(super) struct State {
    /// The text of each box as being typed, by form and control.
    texts: HashMap<(u64, usize), String>,
    /// Changes no code watches, sent with the next event.
    pending: Vec<(u64, usize, Change)>,
    /// The control with the focus here.
    focus: Option<(u64, usize)>,
    /// A click that waits for the focus to move first (Exit, then Click).
    queued: Option<(u64, usize)>,
    /// The focus each form's code gave (Show, SetFocus) that the window
    /// has moved to: a new one moves it again.
    applied: HashMap<u64, Option<usize>>,
    /// The pictures of Image controls, as textures take them.
    pictures: HashMap<usize, (Arc<[u8]>, u64)>,
}

impl State {
    /// New views came: the texts are theirs again.
    pub(super) fn reset(&mut self) {
        self.texts.clear();
        self.pending.clear();
    }
}

fn px(pt: f32) -> f32 {
    pt * SCALE
}

fn color(c: Option<u32>, or: Color32) -> Color32 {
    c.map_or(or, |c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8))
}

/// Where control `i`'s rectangle starts inside its form, in points: its
/// own place plus that of the containers it is in.
fn origin(v: &View, i: usize) -> (f32, f32) {
    let (mut x, mut y) = (v.items[i].c.left, v.items[i].c.top);
    let mut at = v.items[i].parent;
    while let Some(p) = at {
        let it = &v.items[p];
        match it.c.kind {
            // A page is the MultiPage's inside, below its tabs.
            Kind::Page => {
                if let Some(mp) = it.parent {
                    let (mx, my) = origin(v, mp);
                    return (x + mx + 2.0, y + my + TABS + 2.0);
                }
            }
            Kind::Frame => {
                x += it.c.left + 2.0;
                y += it.c.top + 8.0;
            }
            _ => {
                x += it.c.left;
                y += it.c.top;
            }
        }
        at = it.parent;
    }
    (x, y)
}

/// The character typed between `old` and `new`, when one was.
fn typed(old: &str, new: &str) -> Option<char> {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    if b.len() != a.len() + 1 {
        return None;
    }
    let k = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len());
    b.get(k).copied()
}

/// Shows the forms (only the last takes input when `live`) and returns
/// what the user did in it that the macro must know now.
pub(super) fn show(ctx: &egui::Context, views: &[Arc<View>], st: &mut State, live: bool) -> Option<Event> {
    let mut out = None;
    let last = views.len().saturating_sub(1);
    // A form shown again takes its focus again.
    st.applied.retain(|f, _| views.iter().any(|v| v.id == *f));
    for (k, v) in views.iter().enumerate() {
        let interactive = live && k == last && v.enabled;
        // A click that waited for the focus: it goes when the focus did.
        if interactive
            && out.is_none()
            && let Some((f, i)) = st.queued.take()
            && f == v.id
            && v.focus == Some(i)
            && v.items.get(i).is_some_and(|it| !it.gone)
        {
            out = Some(Event { form: v.id, changes: take_pending(st, v.id), action: Action::Click(i) });
        }
        let ev = form(ctx, v, st, interactive, k == 0);
        if out.is_none() {
            out = ev;
        }
    }
    out
}

fn take_pending(st: &mut State, form: u64) -> Vec<(usize, Change)> {
    let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut st.pending).into_iter().partition(|(f, _, _)| *f == form);
    st.pending = rest;
    // The last change of a control is the one that counts.
    let mut out: Vec<(usize, Change)> = Vec::new();
    for (_, i, c) in mine {
        out.retain(|(k, x)| !(*k == i && std::mem::discriminant(x) == std::mem::discriminant(&c)));
        out.push((i, c));
    }
    out
}

/// One form.
fn form(ctx: &egui::Context, v: &View, st: &mut State, interactive: bool, first: bool) -> Option<Event> {
    let size = vec2(px(v.width), px(v.height));
    let id = egui::Id::new(("macro_form", v.id));
    let mut area = egui::Modal::default_area(id);
    if v.start_up == 0 {
        area = egui::Area::new(id).kind(egui::UiKind::Modal).sense(Sense::hover()).order(egui::Order::Foreground).interactable(true).fixed_pos(pos2(px(v.left), px(v.top)));
    }
    let modal = egui::Modal::new(id).area(area).frame(egui::Frame::window(&ctx.global_style()).inner_margin(0.0));
    let modal = if first { modal } else { modal.backdrop_color(Color32::TRANSPARENT) };
    let mut out: Option<Event> = None;
    modal.show(ctx, |ui| {
        ui.add_enabled_ui(interactive, |ui| {
            // The title bar with the close button.
            let (bar, _) = ui.allocate_exact_size(vec2(size.x, TITLE), Sense::hover());
            ui.painter().text(bar.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, &v.caption, egui::FontId::proportional(14.0), ui.visuals().strong_text_color());
            let close = Rect::from_min_size(pos2(bar.right() - TITLE, bar.top()), Vec2::splat(TITLE));
            if ui.put(close, egui::Button::new("×").frame(false)).clicked() {
                out = Some(Event { form: v.id, changes: take_pending(st, v.id), action: Action::Close });
            }
            let (client, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect_filled(client, 0.0, color(v.back, ui.visuals().panel_fill));
            let mut ctl = Controls { v, st, out: None, base: client.min, live: interactive };
            for i in 0..v.items.len() {
                if v.showing(i) {
                    ctl.control(ui, i);
                }
            }
            if out.is_none() {
                out = ctl.out.take();
            }
            // Enter presses the Default button, Esc the Cancel one.
            if interactive && out.is_none() {
                let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                let multiline = st.focus.and_then(|(f, i)| (f == v.id).then_some(i)).and_then(|i| v.items.get(i)).is_some_and(|it| it.c.kind == Kind::TextBox && it.c.multiline);
                let pick = if enter && !multiline { v.items.iter().position(|it| it.c.default && !it.gone && it.c.enabled) } else if esc { v.items.iter().position(|it| it.c.cancel && !it.gone && it.c.enabled) } else { None };
                if let Some(b) = pick.filter(|&b| v.showing(b)) {
                    out = Some(click(v, st, b));
                }
            }
        });
    });
    out
}

/// A button pressed: Click, after the focus moves to it when the code
/// watches the focus.
fn click(v: &View, st: &mut State, i: usize) -> Event {
    let from = st.focus.filter(|(f, _)| *f == v.id).map(|(_, k)| k);
    let watched = |k: Option<usize>| k.and_then(|k| v.handles.get(k)).is_some_and(|h| h & handles::FOCUS != 0);
    st.focus = Some((v.id, i));
    if from != Some(i) && (watched(from) || watched(Some(i))) {
        st.queued = Some((v.id, i));
        return Event { form: v.id, changes: take_pending(st, v.id), action: Action::Focus(Some(i)) };
    }
    Event { form: v.id, changes: take_pending(st, v.id), action: Action::Click(i) }
}

struct Controls<'a> {
    v: &'a View,
    st: &'a mut State,
    out: Option<Event>,
    base: egui::Pos2,
    /// The form takes input (the top one, while the macro waits).
    live: bool,
}

impl Controls<'_> {
    fn rect(&self, i: usize) -> Rect {
        let (x, y) = origin(self.v, i);
        let c = &self.v.items[i].c;
        Rect::from_min_size(self.base + vec2(px(x), px(y)), vec2(px(c.width).max(1.0), px(c.height).max(1.0)))
    }

    fn send(&mut self, i: usize, ch: Change, now: bool) {
        let form = self.v.id;
        self.st.pending.push((form, i, ch));
        if now && self.out.is_none() {
            self.out = Some(Event { form, changes: take_pending(self.st, form), action: Action::Changes });
        }
    }

    fn handles(&self, i: usize, what: u8) -> bool {
        self.v.handles.get(i).is_some_and(|h| h & what != 0)
    }

    /// The code put the focus on control `i` (the first one when the form
    /// showed, or SetFocus): the window moves it there once.
    fn want_focus(&mut self, i: usize, r: &egui::Response) {
        let v = self.v;
        if self.live && v.focus == Some(i) && self.st.applied.get(&v.id) != Some(&Some(i)) {
            self.st.applied.insert(v.id, Some(i));
            self.st.focus = Some((v.id, i));
            r.request_focus();
        }
    }

    /// The focus went to control `i` here.
    fn focused(&mut self, i: usize) {
        let form = self.v.id;
        if self.st.focus == Some((form, i)) {
            return;
        }
        let from = self.st.focus.filter(|(f, _)| *f == form).map(|(_, k)| k);
        self.st.focus = Some((form, i));
        let watched = from.is_some_and(|k| self.handles(k, handles::FOCUS)) || self.handles(i, handles::FOCUS);
        if watched && self.out.is_none() {
            self.out = Some(Event { form, changes: take_pending(self.st, form), action: Action::Focus(Some(i)) });
        }
    }

    fn text(&self, i: usize, size: f32, s: &str) -> RichText {
        let c = &self.v.items[i].c;
        let mut t = RichText::new(s).size(px(size).max(6.0));
        if c.bold {
            t = t.strong();
        }
        if c.italic {
            t = t.italics();
        }
        if let Some(f) = c.fore {
            t = t.color(color(Some(f), Color32::BLACK));
        }
        t
    }

    fn control(&mut self, ui: &mut egui::Ui, i: usize) {
        let v = self.v;
        let it = &v.items[i];
        let c = &it.c;
        let rect = self.rect(i);
        let enabled = c.enabled;
        let salt = (v.id, i);
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)).id_salt(salt));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let ui = &mut child;
        if !enabled {
            ui.disable();
        }
        if let Some(b) = c.back
            && !matches!(c.kind, Kind::CommandButton | Kind::Frame | Kind::MultiPage | Kind::Page)
        {
            ui.painter().rect_filled(rect, 0.0, color(Some(b), Color32::WHITE));
        }
        let font = c.font_size;
        match c.kind {
            Kind::Label => {
                // Labels and images take clicks but not the focus: Tab skips them.
                let mut l = egui::Label::new(self.text(i, font, &c.caption));
                l = if c.word_wrap { l.wrap() } else { l.truncate() };
                let r = ui.add(l.sense(Sense::CLICK));
                if r.clicked() && self.handles(i, handles::CLICK) {
                    self.out.get_or_insert(Event { form: v.id, changes: take_pending(self.st, v.id), action: Action::Click(i) });
                }
                tip(r, &c.tip);
            }
            Kind::TextBox => {
                let key = (v.id, i);
                let mut text = self.st.texts.get(&key).cloned().unwrap_or_else(|| c.value.clone());
                let before = text.clone();
                let mut edit = if c.multiline { egui::TextEdit::multiline(&mut text) } else { egui::TextEdit::singleline(&mut text) };
                edit = edit.desired_width(rect.width()).min_size(rect.size()).font(egui::FontId::proportional(px(font).max(6.0))).password(!c.password_char.is_empty()).interactive(enabled && !c.locked);
                if c.max_length > 0 {
                    edit = edit.char_limit(c.max_length as usize);
                }
                if let Some(b) = c.back {
                    edit = edit.background_color(color(Some(b), Color32::WHITE));
                }
                let r = ui.add(edit);
                self.want_focus(i, &r);
                if r.gained_focus() || r.has_focus() {
                    self.focused(i);
                }
                if r.changed() && text != before {
                    let key_ch = typed(&before, &text);
                    self.st.texts.insert(key, text.clone());
                    if let Some(ch) = key_ch.filter(|_| self.handles(i, handles::KEY)) {
                        self.st.pending.push((v.id, i, Change::Text(text)));
                        if self.out.is_none() {
                            self.out = Some(Event { form: v.id, changes: take_pending(self.st, v.id), action: Action::Key(i, ch) });
                        }
                    } else {
                        let now = self.handles(i, handles::CHANGE);
                        self.send(i, Change::Text(text), now);
                    }
                }
                tip(r, &c.tip);
            }
            Kind::CommandButton => {
                let r = ui.add_sized(rect.size(), egui::Button::new(self.text(i, font, &c.caption)));
                self.want_focus(i, &r);
                if r.clicked() && self.out.is_none() && self.handles(i, handles::CLICK) {
                    self.out = Some(click(v, self.st, i));
                } else if r.clicked() {
                    self.focused(i);
                }
                tip(r, &c.tip);
            }
            Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton => {
                let on = c.value == "1" || c.value.eq_ignore_ascii_case("true") || c.value == "-1";
                let pending = self.st.pending.iter().rev().find(|(f, k, ch)| *f == v.id && *k == i && matches!(ch, Change::Checked(_)));
                let mut on = match pending {
                    Some((_, _, Change::Checked(b))) => *b,
                    _ => on,
                };
                let was = on;
                let r = match c.kind {
                    Kind::CheckBox => ui.add(egui::Checkbox::new(&mut on, self.text(i, font, &c.caption))),
                    Kind::OptionButton => {
                        let r = ui.add(egui::RadioButton::new(on, self.text(i, font, &c.caption)));
                        if r.clicked() {
                            on = true;
                        }
                        r
                    }
                    _ => {
                        let r = ui.add_sized(rect.size(), egui::Button::new(self.text(i, font, &c.caption)).selected(on));
                        if r.clicked() {
                            on = !on;
                        }
                        r
                    }
                };
                self.want_focus(i, &r);
                if r.clicked() {
                    self.focused(i);
                }
                if on != was {
                    let now = self.handles(i, handles::CHANGE | handles::CLICK);
                    self.send(i, Change::Checked(on), now);
                    // The others of its group go off with it here too.
                    if on && c.kind == Kind::OptionButton {
                        for k in 0..v.items.len() {
                            let o = &v.items[k];
                            if k != i && !o.gone && o.c.kind == Kind::OptionButton && o.parent == it.parent && o.c.group.eq_ignore_ascii_case(&c.group) {
                                self.st.pending.push((v.id, k, Change::Checked(false)));
                            }
                        }
                    }
                }
                tip(r, &c.tip);
            }
            Kind::ListBox => self.list_box(ui, i, rect),
            Kind::ComboBox => self.combo_box(ui, i, rect),
            Kind::SpinButton => {
                let n: i32 = c.value.parse().unwrap_or(0);
                let vertical = c.height >= c.width;
                let half = if vertical { vec2(rect.width(), rect.height() / 2.0) } else { vec2(rect.width() / 2.0, rect.height()) };
                let (a, b) = if vertical { ("⏶", "⏷") } else { ("⏴", "⏵") };
                let step = c.small_change.max(1);
                let (up, down) = if vertical {
                    (ui.put(Rect::from_min_size(rect.min, half), egui::Button::new(a)), ui.put(Rect::from_min_size(rect.min + vec2(0.0, half.y), half), egui::Button::new(b)))
                } else {
                    (ui.put(Rect::from_min_size(rect.min + vec2(half.x, 0.0), half), egui::Button::new(b)), ui.put(Rect::from_min_size(rect.min, half), egui::Button::new(a)))
                };
                let to = if up.clicked() { Some(n.saturating_add(step)) } else if down.clicked() { Some(n.saturating_sub(step)) } else { None };
                if let Some(to) = to {
                    let (lo, hi) = (c.min.min(c.max), c.min.max(c.max));
                    self.send(i, Change::Number(to.clamp(lo, hi)), true);
                }
            }
            Kind::ScrollBar => {
                let mut n: i32 = c.value.parse().unwrap_or(0);
                let (lo, hi) = (c.min.min(c.max), c.min.max(c.max));
                let mut s = egui::Slider::new(&mut n, lo..=hi).show_value(false).step_by(c.small_change.max(1) as f64);
                if c.height > c.width {
                    s = s.vertical();
                }
                ui.spacing_mut().slider_width = if c.height > c.width { rect.height() } else { rect.width() };
                let r = ui.add(s);
                if r.drag_stopped() || r.changed() && !r.dragged() {
                    let now = self.handles(i, handles::CHANGE);
                    self.send(i, Change::Number(n), now);
                }
            }
            Kind::Frame => {
                let painter = ui.painter();
                let stroke = Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
                let top = rect.top() + px(4.0);
                painter.rect_stroke(Rect::from_min_max(pos2(rect.left(), top), rect.max), 3.0, stroke, egui::StrokeKind::Inside);
                if !c.caption.is_empty() {
                    let g = painter.layout_no_wrap(c.caption.clone(), egui::FontId::proportional(px(font).max(6.0)), ui.visuals().text_color());
                    let at = pos2(rect.left() + 8.0, rect.top() - 1.0);
                    painter.rect_filled(Rect::from_min_size(at - vec2(2.0, 0.0), g.size() + vec2(4.0, 0.0)), 0.0, color(v.back, ui.visuals().panel_fill));
                    painter.galley(at, g, Color32::PLACEHOLDER);
                }
            }
            Kind::MultiPage | Kind::TabStrip => {
                let captions: Vec<(usize, String)> = if c.kind == Kind::MultiPage { v.pages(i).into_iter().enumerate().map(|(k, p)| (k, v.items[p].c.caption.clone())).collect() } else { it.list.iter().enumerate().map(|(k, r)| (k, r.first().cloned().unwrap_or_default())).collect() };
                let stroke = Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
                ui.painter().rect_stroke(Rect::from_min_max(rect.min + vec2(0.0, px(TABS)), rect.max), 0.0, stroke, egui::StrokeKind::Inside);
                let picked = it.list_index.max(0) as usize;
                let pending = self.st.pending.iter().rev().find_map(|(f, k, ch)| match ch {
                    Change::Index(n) if *f == v.id && *k == i => Some(*n as usize),
                    _ => None,
                });
                let picked = pending.unwrap_or(picked);
                ui.horizontal(|ui| {
                    for (k, cap) in &captions {
                        if ui.selectable_label(*k == picked, self.text(i, font, cap)).clicked() && *k != picked {
                            let now = self.handles(i, handles::CHANGE | handles::CLICK);
                            // The page shows from the macro's view.
                            self.send(i, Change::Index(*k as i32), now || c.kind == Kind::MultiPage);
                        }
                    }
                });
            }
            Kind::Page => {}
            Kind::Image => {
                if let Some(p) = &c.picture {
                    let key = Arc::as_ptr(p) as *const u8 as usize;
                    let (bytes, k) = self.st.pictures.entry(key).or_insert_with(|| {
                        let b: Arc<[u8]> = p.as_slice().into();
                        let k = crate::images::key(&b);
                        (b, k)
                    });
                    if let Some(t) = crate::images::texture(ui.ctx(), *k, bytes, false) {
                        ui.painter().image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                }
                let r = ui.allocate_rect(rect, Sense::CLICK);
                if r.clicked() && self.handles(i, handles::CLICK) && self.out.is_none() {
                    self.out = Some(Event { form: v.id, changes: take_pending(self.st, v.id), action: Action::Click(i) });
                }
            }
            Kind::Other => {
                ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
            }
        }
    }

    fn list_box(&mut self, ui: &mut egui::Ui, i: usize, rect: Rect) {
        let v = self.v;
        let it = &v.items[i];
        let c = &it.c;
        let form = v.id;
        let mut index = it.list_index;
        let mut sel = it.selected.clone();
        for (f, k, ch) in &self.st.pending {
            if *f == form && *k == i {
                match ch {
                    Change::Index(n) => index = *n,
                    Change::Selected(s) => sel = s.clone(),
                    _ => {}
                }
            }
        }
        sel.resize(it.list.len(), false);
        let stroke = Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color);
        ui.painter().rect_filled(rect, 0.0, color(c.back, ui.visuals().extreme_bg_color));
        ui.painter().rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        let mut picked = None;
        let mut toggled = None;
        let mut double = None;
        egui::ScrollArea::vertical().id_salt(("list", form, i)).max_height(rect.height()).auto_shrink([false, false]).show_rows(ui, px(c.font_size).max(6.0) + 6.0, it.list.len(), |ui, rows| {
            for r in rows {
                let on = if c.multi_select == 0 { index == r as i32 } else { sel[r] };
                let row = &it.list[r];
                let cols = c.column_count.max(1) as usize;
                let shown = row.iter().take(cols).cloned().collect::<Vec<_>>().join("   ");
                let resp = ui.add_sized(vec2(rect.width() - 4.0, px(c.font_size).max(6.0) + 4.0), egui::Button::selectable(on, self.text(i, c.font_size, &shown)));
                if resp.clicked() {
                    if c.multi_select == 0 { picked = Some(r as i32) } else { toggled = Some(r) }
                }
                if resp.double_clicked() {
                    double = Some(r);
                }
                if resp.gained_focus() {
                    picked.get_or_insert(r as i32);
                }
            }
        });
        if picked.is_some() || toggled.is_some() {
            self.focused(i);
        }
        let now = self.handles(i, handles::CHANGE | handles::CLICK);
        if let Some(r) = picked.filter(|&r| r != index) {
            self.send(i, Change::Index(r), now);
        }
        if let Some(r) = toggled {
            sel[r] = !sel[r];
            self.send(i, Change::Selected(sel), now);
        }
        if double.is_some() && self.handles(i, handles::DBLCLICK) && self.out.is_none() {
            self.out = Some(Event { form, changes: take_pending(self.st, form), action: Action::DblClick(i) });
        }
    }

    fn combo_box(&mut self, ui: &mut egui::Ui, i: usize, rect: Rect) {
        let v = self.v;
        let it = &v.items[i];
        let c = &it.c;
        let key = (v.id, i);
        let mut text = self.st.texts.get(&key).cloned().unwrap_or_else(|| c.value.clone());
        let mut picked: Option<usize> = None;
        let arrow = rect.height().min(22.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let w = (rect.width() - arrow).max(10.0);
            if c.drop_down_list {
                ui.add_sized(vec2(w, rect.height()), egui::Label::new(self.text(i, c.font_size, &text)).truncate());
            } else {
                let before = text.clone();
                let r = ui.add_sized(vec2(w, rect.height()), egui::TextEdit::singleline(&mut text).font(egui::FontId::proportional(px(c.font_size).max(6.0))).interactive(c.enabled && !c.locked));
                self.want_focus(i, &r);
                if r.gained_focus() || r.has_focus() {
                    self.focused(i);
                }
                if r.changed() && text != before {
                    self.st.texts.insert(key, text.clone());
                    let now = self.handles(i, handles::CHANGE);
                    self.send(i, Change::Text(text.clone()), now);
                }
            }
            let menu = ui.add_sized(vec2(arrow, rect.height()), egui::Button::new("⏷"));
            egui::Popup::menu(&menu).show(|ui| {
                ui.set_min_width(rect.width());
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for (r, row) in it.list.iter().enumerate() {
                        let shown = row.iter().take(c.column_count.max(1) as usize).cloned().collect::<Vec<_>>().join("   ");
                        if ui.selectable_label(it.list_index == r as i32, shown).clicked() {
                            picked = Some(r);
                        }
                    }
                });
            });
        });
        if let Some(r) = picked {
            self.focused(i);
            let value = it.list[r].first().cloned().unwrap_or_default();
            self.st.texts.insert(key, value);
            let now = self.handles(i, handles::CHANGE | handles::CLICK);
            self.send(i, Change::Index(r as i32), now);
        }
    }
}

fn tip(r: egui::Response, text: &str) {
    if !text.is_empty() {
        r.on_hover_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_typed_is_found() {
        assert_eq!(typed("ab", "abc"), Some('c'));
        assert_eq!(typed("ac", "abc"), Some('b'));
        assert_eq!(typed("abc", "ab"), None);
        assert_eq!(typed("", "ñ"), Some('ñ'));
    }
}
