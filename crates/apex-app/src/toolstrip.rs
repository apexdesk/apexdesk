//! Office-style toolstrip: a strip of tabs, and under it the active tab's
//! labelled groups of large and small buttons. Groups that don't fit in the
//! window fold, from the right, into a button that opens them in a popup.
//!
//! The toolstrip's buttons are also the commands of the command search
//! (`palette`): drawn off screen with [`collect`], every button says what it
//! is, and [`run`] draws a tab off screen again with one button "clicked".
//! Menus are drawn inline off screen, so their [`menu_item`]s are commands
//! too.
//!
//! The same definitions draw the classic interface ([`Style`]): each tab a
//! menu of the menu bar, or one row of small buttons (the toolbar).

use eframe::egui::{self, Color32, FontId, Id, Rect, Response, RichText, Sense, Stroke, Vec2, pos2, vec2};

use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::widgets::{self, Face};

pub mod keytips;

// --- commands ----------------------------------------------------------------------

/// A button of the toolstrip, as a command of the command search.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Where it is and its name, what [`run`] takes.
    pub key: String,
    pub label: String,
    /// Shortcut, from a hover text like "Bold (Ctrl+B)".
    pub keys: String,
    /// Tab, group and menus leading to it.
    pub place: Vec<String>,
    pub icon: Option<Icon>,
    pub enabled: bool,
    /// Opens a menu rather than running something.
    pub menu: bool,
    /// Shown as on (a pressed toggle, the mode in use).
    pub on: bool,
}

#[derive(Default)]
enum Mode {
    #[default]
    Normal,
    Collect(Vec<Entry>),
    Run(String, bool),
}

/// How the buttons are drawn on screen.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Style {
    #[default]
    Toolstrip,
    /// Rows of a menu: groups are submenus, menu buttons too.
    Menu,
    /// One row of small icon buttons without group titles.
    Compact,
}

#[derive(Default)]
struct State {
    path: Vec<String>,
    mode: Mode,
    style: Style,
    /// A menu button to open on screen (a menu chosen in the search).
    open: Option<String>,
    /// Accent of the toolstrip being drawn (for [`check`]).
    accent: Option<Color32>,
}

thread_local! {
    static STATE: std::cell::RefCell<State> = std::cell::RefCell::default();
}

fn offscreen() -> bool {
    STATE.with_borrow(|s| !matches!(s.mode, Mode::Normal))
}

/// The style buttons are drawn in (off screen, always the toolstrip's).
fn style() -> Style {
    STATE.with_borrow(|s| if matches!(s.mode, Mode::Normal) { s.style } else { Style::Toolstrip })
}

/// Whether buttons are drawn in one compact row.
pub fn compact() -> bool {
    style() == Style::Compact
}

/// `id` for a widget of a tab: the classic interface draws the Home tab
/// twice in a frame (menu and toolbar), so the menu's gets its own.
pub fn own_id(id: Id) -> Id {
    if style() == Style::Menu { id.with("menu") } else { id }
}

/// Draws with `f` in `style`.
pub fn with_style<R>(style: Style, f: impl FnOnce() -> R) -> R {
    let old = STATE.with_borrow_mut(|s| std::mem::replace(&mut s.style, style));
    let r = f();
    STATE.with_borrow_mut(|s| s.style = old);
    r
}

/// Runs `f` with `seg` (a tab or menu name) added to the place of the
/// buttons it draws.
pub fn within<R>(seg: &str, f: impl FnOnce() -> R) -> R {
    let n = STATE.with_borrow_mut(|s| {
        s.path.push(seg.to_string());
        s.path.len() - 1
    });
    let r = f();
    STATE.with_borrow_mut(|s| s.path.truncate(n));
    r
}

fn set_group(base: usize, group: &str) {
    STATE.with_borrow_mut(|s| {
        s.path.truncate(base);
        s.path.push(group.to_string());
    });
}

fn split_keys(hover: &str) -> (&str, &str) {
    widgets::split_shortcut(hover)
}

/// Registers a button; true when it is the one [`run`] clicks, or (on
/// screen) the menu to open.
fn register(ui: &egui::Ui, label: &str, hover: &str, icon: Option<Icon>, menu: bool) -> bool {
    register_on(ui, label, hover, icon, menu, false)
}

/// [`register`] for a control shown as on or off.
fn register_on(ui: &egui::Ui, label: &str, hover: &str, icon: Option<Icon>, menu: bool, on: bool) -> bool {
    let (name, keys) = if label.is_empty() { split_keys(hover) } else { (label.trim_end_matches(" ⏷"), split_keys(hover).1) };
    let enabled = ui.is_enabled();
    STATE.with_borrow_mut(|s| {
        let key = format!("{}/{}", s.path.join("/"), name);
        match &mut s.mode {
            Mode::Normal => {
                if s.style != Style::Menu && !name.is_empty() && enabled && keytips::recording() {
                    keytips::seen(ui, key.clone(), name, menu);
                }
                if s.open.as_deref() == Some(&key) {
                    s.open = None;
                    return true;
                }
                false
            }
            Mode::Collect(v) => {
                if !name.is_empty() && !v.iter().any(|e| e.key == key) {
                    v.push(Entry { key, label: name.to_string(), keys: keys.to_string(), place: s.path.clone(), icon, enabled, menu, on });
                }
                false
            }
            Mode::Run(target, ran) => {
                let hit = *target == key && !*ran;
                *ran |= hit;
                hit
            }
        }
    })
}

/// A child of `ui` that is laid out but neither shown nor hit by the mouse.
pub fn offscreen_ui<R>(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let rect = Rect::from_min_size(ui.max_rect().min, vec2(4000.0, 400.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("toolstrip_offscreen"));
    child.set_clip_rect(Rect::NOTHING);
    f(&mut child)
}

/// Every button `draw` draws (off screen).
pub fn collect(draw: impl FnOnce()) -> Vec<Entry> {
    STATE.with_borrow_mut(|s| s.mode = Mode::Collect(Vec::new()));
    draw();
    STATE.with_borrow_mut(|s| match std::mem::take(&mut s.mode) {
        Mode::Collect(v) => v,
        _ => Vec::new(),
    })
}

/// Whether the commands are being collected (nothing is shown).
pub fn collecting() -> bool {
    STATE.with_borrow(|s| matches!(s.mode, Mode::Collect(_)))
}

/// Draws (off screen) with the button `key` clicked; false if it wasn't
/// there.
pub fn run(key: &str, draw: impl FnOnce()) -> bool {
    STATE.with_borrow_mut(|s| s.mode = Mode::Run(key.to_string(), false));
    draw();
    let hit = STATE.with_borrow_mut(|s| matches!(std::mem::take(&mut s.mode), Mode::Run(_, true)));
    #[cfg(test)]
    test::LAST_RUN.set(Some(hit));
    hit
}

#[cfg(test)]
pub mod test {
    //! What the tests see of the toolstrip's commands.

    use std::cell::Cell;

    thread_local! {
        pub(super) static LAST_RUN: Cell<Option<bool>> = const { Cell::new(None) };
    }

    /// Whether the last [`super::run`] found its button (None: none ran).
    pub fn take_last_run() -> Option<bool> {
        LAST_RUN.take()
    }

    /// Forgets what a drawing cut short (a panic) left behind.
    pub fn reset() {
        super::STATE.with_borrow_mut(|s| *s = Default::default());
        LAST_RUN.set(None);
    }
}

/// Opens the menu button `key` the next time it is drawn on screen.
pub fn open_menu(key: String) {
    STATE.with_borrow_mut(|s| s.open = Some(key));
}

/// Whether a menu from [`open_menu`] is still to open.
pub fn menu_pending() -> bool {
    STATE.with_borrow(|s| s.open.is_some())
}

/// Registers a custom widget as a command; true when it is to run.
pub fn register_only(ui: &egui::Ui, label: &str, icon: Icon) -> bool {
    register(ui, label, "", Some(icon), false)
}

fn force_click(resp: &mut Response) {
    resp.flags.insert(egui::response::Flags::FAKE_PRIMARY_CLICKED);
}

/// Shows `menu` under `resp` on screen; off screen, inline (so its items
/// register).
fn menu_popup(ui: &mut egui::Ui, resp: &Response, label: &str, menu: impl FnOnce(&mut egui::Ui)) {
    menu_popup_dyn(ui, resp, label, &mut widgets::once(menu))
}

/// The body of [`menu_popup`], one for every caller.
fn menu_popup_dyn(ui: &mut egui::Ui, resp: &Response, label: &str, menu: &mut dyn FnMut(&mut egui::Ui)) {
    if offscreen() {
        within(label.trim_end_matches('…'), || menu(ui));
    } else {
        widgets::button_menu(ui, resp, egui::PopupCloseBehavior::CloseOnClickOutside, menu);
    }
}

/// Height of a small button row; three rows fill a group.
pub const ROW_H: f32 = 24.0;
const ROWS_H: f32 = ROW_H * 3.0 + 2.0;
const TITLE_H: f32 = 16.0;
const LARGE_ICON: f32 = 30.0;
/// Width of a folded group's button.
const FOLDED_W: f32 = 64.0;
const COMPACT_FOLDED_W: f32 = 36.0;
/// Width of a folded group that shows only its icon (a window too narrow
/// for the folded groups' names).
const TINY_W: f32 = 42.0;

/// A tab of the toolstrip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TabDef {
    /// Stable name ("home", "insert"...), what views match on.
    pub id: &'static str,
    /// Label key.
    pub label: &'static str,
    /// Shown only for the selected object (chart, table, picture...), in
    /// this color.
    pub context: Option<Color32>,
}

impl TabDef {
    pub const fn new(id: &'static str, label: &'static str) -> TabDef {
        TabDef { id, label, context: None }
    }

    pub const fn contextual(id: &'static str, label: &'static str, color: Color32) -> TabDef {
        TabDef { id, label, context: Some(color) }
    }
}

/// What happened in the tab strip.
#[derive(Default)]
pub struct StripResponse {
    /// The File button was clicked: its rect, to anchor the file menu.
    pub file: Option<Response>,
    /// A tab was double-clicked: fold or unfold the toolstrip.
    pub toggle_fold: bool,
    /// A tab was clicked while the toolstrip is folded.
    pub unfold: bool,
}

/// The strip of tab names, with a colored File button first.
pub fn tab_strip(ui: &mut egui::Ui, tabs: &[TabDef], current: &mut &'static str, accent: Color32, folded: bool) -> StripResponse {
    let mut out = StripResponse::default();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut file = tab_label(ui, tr("menu.file"), false, Some(accent), true);
        let shown = shown_tabs(ui, tabs, current);
        let mut rects = Vec::new();
        let mut ctx_start = true;
        for t in tabs.iter().filter(|t| shown.contains(&t.id)) {
            if t.context.is_some() && ctx_start {
                ui.add_space(6.0);
                ctx_start = false;
            }
            let on = *current == t.id && !folded;
            let r = tab_label(ui, tr(t.label), on, t.context, false);
            rects.push((t.id, tr(t.label), r.rect));
            if r.double_clicked() {
                out.toggle_fold = true;
            } else if r.clicked() {
                *current = t.id;
                if folded {
                    out.unfold = true;
                }
            }
        }
        // The tabs that don't fit, in a menu at the end.
        if shown.len() < tabs.len() {
            let r = tab_label(ui, "»", false, None, false);
            widgets::name(&r, egui::WidgetType::ComboBox, tr("toolstrip.more"), None);
            let r = r.on_hover_text(tr("toolstrip.more"));
            egui::Popup::menu(&r).show(|ui| {
                for t in tabs.iter().filter(|t| !shown.contains(&t.id)) {
                    let mut text = RichText::new(tr(t.label));
                    if let Some(c) = t.context {
                        text = text.color(c);
                    }
                    if ui.button(text).clicked() {
                        *current = t.id;
                        out.unfold |= folded;
                    }
                }
            });
        }
        // A tab or the File menu chosen by its KeyTip.
        let (goto, open_file) = keytips::strip(ui, &rects, file.rect);
        if let Some(id) = goto {
            *current = id;
            out.unfold |= folded;
        }
        if open_file {
            force_click(&mut file);
        }
        out.file = Some(file);
    });
    out
}

/// Width of a tab of the strip showing `text`.
fn tab_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.5), Color32::PLACEHOLDER).size().x + 20.0
}

/// The tabs that fit in the strip's row, in a narrow window: the first
/// ones, and the current one in place of the last that fits. The rest go
/// to the » menu.
fn shown_tabs(ui: &egui::Ui, tabs: &[TabDef], current: &str) -> Vec<&'static str> {
    let gap = ui.spacing().item_spacing.x;
    let widths: Vec<f32> = tabs.iter().map(|t| tab_width(ui, tr(t.label)) + gap + if t.context.is_some() { 6.0 } else { 0.0 }).collect();
    let avail = ui.available_width();
    if widths.iter().sum::<f32>() <= avail {
        return tabs.iter().map(|t| t.id).collect();
    }
    let room = avail - tab_width(ui, "»") - gap;
    let cur = tabs.iter().position(|t| t.id == current);
    let mut shown = Vec::new();
    let mut used = cur.map_or(0.0, |i| widths[i]);
    let mut full = false;
    for (i, t) in tabs.iter().enumerate() {
        if Some(i) == cur {
            shown.push(t.id);
            continue;
        }
        full |= used + widths[i] > room;
        if !full {
            used += widths[i];
            shown.push(t.id);
        }
    }
    shown
}

fn tab_label(ui: &mut egui::Ui, text: &str, on: bool, color: Option<Color32>, filled: bool) -> Response {
    let font = FontId::proportional(13.5);
    let v = ui.visuals().clone();
    let text_color = if filled {
        Color32::WHITE
    } else if on {
        color.unwrap_or(v.strong_text_color())
    } else {
        color.map_or(v.text_color(), |c| c.gamma_multiply(0.9))
    };
    let g = ui.painter().layout_no_wrap(text.to_string(), font, text_color);
    let size = vec2(g.size().x + 20.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    widgets::name(&resp, egui::WidgetType::SelectableLabel, text, Some(on));
    let p = ui.painter();
    if filled {
        let fill = if resp.hovered() { color.unwrap_or(Color32::GRAY).gamma_multiply(0.85) } else { color.unwrap_or(Color32::GRAY) };
        p.rect_filled(rect.shrink2(vec2(0.0, 2.0)), 5.0, fill);
    } else if resp.hovered() && !on {
        p.rect_filled(rect.shrink2(vec2(0.0, 2.0)), 5.0, v.widgets.hovered.weak_bg_fill);
    }
    if let Some(c) = color.filter(|_| !filled) {
        // Contextual tabs: a tinted band behind the name.
        p.rect_filled(Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 3.0)), 1.0, c);
    }
    p.galley(rect.center() - g.size() / 2.0, g, text_color);
    if on {
        let c = color.unwrap_or(v.selection.bg_fill);
        let w = (rect.width() - 16.0).max(10.0);
        p.rect_filled(Rect::from_center_size(pos2(rect.center().x, rect.max.y - 2.0), vec2(w, 3.0)), 1.5, c);
    }
    resp
}

// --- the body -----------------------------------------------------------------------

/// Lays out the groups of one tab. Call [`Body::group`] for each group, in
/// order.
pub struct Body<'u> {
    ui: &'u mut egui::Ui,
    /// Length of the command path before the groups.
    base: usize,
    /// Width of each group unfolded and folded, from earlier frames.
    widths: Vec<[f32; 2]>,
    /// Which groups are folded.
    folded: Vec<bool>,
    /// Which folded groups show only their icon.
    tiny: Vec<bool>,
    next: usize,
    accent: Color32,
}

/// Shows the toolstrip body of a tab: `id` must differ per tab.
pub fn body(ui: &mut egui::Ui, id: Id, accent: Color32, add: impl FnOnce(&mut Body)) {
    STATE.with_borrow_mut(|s| s.accent = Some(accent));
    let st = style();
    if st == Style::Menu {
        let base = STATE.with_borrow(|s| s.path.len());
        let mut b = Body { ui, base, widths: Vec::new(), folded: Vec::new(), tiny: Vec::new(), next: 0, accent };
        add(&mut b);
        return;
    }
    let id = if st == Style::Compact { id.with("compact") } else { id };
    let folded_w = if st == Style::Compact { COMPACT_FOLDED_W } else { FOLDED_W };
    let widths: Vec<[f32; 2]> = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let avail = ui.available_width();
    // Fold groups from the right until the rest fits.
    let sep = 9.0;
    let n = widths.len();
    let folded_of = |i: usize| if widths[i][1] > 0.0 { widths[i][1] } else { folded_w };
    let mut fold_from = n;
    let total = |k: usize| (0..n).map(|i| if i < k { widths[i][0] } else { folded_of(i) } + sep).sum::<f32>();
    while fold_from > 0 && total(fold_from) > avail {
        fold_from -= 1;
    }
    let mut folded: Vec<bool> = (0..n).map(|i| i >= fold_from).collect();
    // The toolbar unfolds again the groups after the first folded one
    // that still fit (a wide group doesn't take the others with it).
    if st == Style::Compact {
        let used = |f: &[bool]| (0..n).map(|i| if f[i] { folded_of(i) } else { widths[i][0] } + sep).sum::<f32>();
        for i in fold_from + 1..n {
            folded[i] = false;
            if used(&folded) > avail {
                folded[i] = true;
            }
        }
    }
    // Still too wide with every group folded: the last ones lose their names.
    let mut tiny = vec![false; n];
    if st == Style::Toolstrip && fold_from == 0 {
        let mut used = total(0);
        for i in (0..n).rev() {
            if used <= avail {
                break;
            }
            used -= folded_of(i) - TINY_W;
            tiny[i] = true;
        }
    }
    if offscreen() {
        folded.clear();
    }
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
        let base = STATE.with_borrow(|s| s.path.len());
        let mut b = Body { ui, base, widths, folded, tiny, next: 0, accent };
        add(&mut b);
        let Body { widths, next, .. } = b;
        let mut widths = widths;
        widths.truncate(next);
        if !offscreen() {
            ui.data_mut(|d| d.insert_temp(id, widths));
        }
    });
}

impl Body<'_> {
    pub fn accent(&self) -> Color32 {
        self.accent
    }

    /// A group titled `title` (a label key); `icon` shows when it is folded.
    pub fn group(&mut self, title: &str, icon: Icon, mut content: impl FnMut(&mut egui::Ui)) {
        self.group_dyn(title, icon, &mut content)
    }

    /// The body of [`Self::group`], one for every caller.
    fn group_dyn(&mut self, title: &str, icon: Icon, content: &mut dyn FnMut(&mut egui::Ui)) {
        let i = self.next;
        self.next += 1;
        if self.widths.len() <= i {
            self.widths.resize(i + 1, [0.0; 2]);
        }
        let ui = &mut *self.ui;
        let title = tr_or(title);
        let st = style();
        if st == Style::Menu {
            set_group(self.base, title);
            let accent = self.accent;
            sub_menu(ui, Some(icon), title, accent, |ui| content(ui));
            return;
        }
        let compact = st == Style::Compact;
        if i > 0 {
            let x = ui.cursor().min.x + 3.0;
            let top = ui.cursor().min.y + if compact { 3.0 } else { 4.0 };
            let h = if compact { ROW_H - 6.0 } else { ROWS_H + TITLE_H - 6.0 };
            ui.painter().vline(x, top..=top + h, Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color));
            ui.add_space(7.0);
        }
        set_group(self.base, title);
        if compact {
            self.widths[i][1] = COMPACT_FOLDED_W;
            if self.folded.get(i).copied().unwrap_or(false) && self.widths[i][0] > 0.0 {
                // Folded: an icon that opens the group as on the toolstrip.
                let (rect, _) = ui.allocate_exact_size(vec2(COMPACT_FOLDED_W, ROW_H), Sense::hover());
                let mut resp = ui.interact(rect, ui.id().with(("cgroup", i)), Sense::click());
                widgets::name(&resp, egui::WidgetType::ComboBox, title, None);
                if keytips::group(ui, title, rect) {
                    force_click(&mut resp);
                }
                let shown = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
                hover_fill(ui, rect, false, resp.hovered() || shown);
                paint_small(ui, rect, Some(icon), self.accent, ui.painter().layout_no_wrap("⏷".into(), FontId::proportional(10.0), text_color(ui)));
                let resp = resp.on_hover_text(title);
                egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                    with_style(Style::Toolstrip, || {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                            ui.set_min_height(ROWS_H);
                            content(ui)
                        });
                    })
                });
                return;
            }
            let r = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
                    ui.set_min_height(ROW_H);
                    content(ui)
                })
                .response;
            self.widths[i][0] = r.rect.width();
            return;
        }
        self.widths[i][1] = large_width(ui, title, true);
        // Groups not measured yet show unfolded.
        if self.folded.get(i).copied().unwrap_or(false) && self.widths[i][0] > 0.0 {
            // A menu inside chosen in the search opens the group first.
            let prefix = STATE.with_borrow(|s| format!("{}/", s.path.join("/")));
            let open = STATE.with_borrow(|s| s.open.as_ref().is_some_and(|k| k.starts_with(&prefix)));
            let (rect, gs) = if self.tiny.get(i).copied().unwrap_or(false) {
                let (rect, _) = ui.allocate_exact_size(vec2(TINY_W, ROWS_H), Sense::hover());
                (rect, vec![ui.painter().layout_no_wrap("⏷".into(), FontId::proportional(12.0), text_color(ui))])
            } else {
                large_rect(ui, title, true)
            };
            let mut resp = ui.interact(rect, ui.id().with(("group", i)), Sense::click());
            widgets::name(&resp, egui::WidgetType::ComboBox, title, None);
            if open && !egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp)) {
                force_click(&mut resp);
            }
            if keytips::group(ui, title, rect) {
                force_click(&mut resp);
            }
            let shown = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
            hover_fill(ui, rect, false, resp.hovered() || shown);
            paint_large(ui, rect, icon, self.accent, &gs);
            let resp = resp.on_hover_text(title);
            egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                    content(ui)
                });
            });
            return;
        }
        let r = ui
            .vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                let top = ui
                    .horizontal_top(|ui| {
                        ui.set_min_height(ROWS_H);
                        content(ui);
                    })
                    .response;
                // Titles line up along the bottom: a group is three rows high.
                if cfg!(debug_assertions) && top.rect.height() > ROWS_H + 0.5 && !offscreen() {
                    ui.ctx().debug_painter().debug_rect(top.rect, Color32::RED, format!("{title}: more than three rows ({:.0})", top.rect.height()));
                }
                // A title wider than the buttons (short CJK labels) widens the group.
                let tg = ui.painter().layout_no_wrap(title.to_string(), FontId::proportional(11.0), ui.visuals().weak_text_color());
                let (tr, _) = ui.allocate_exact_size(vec2(top.rect.width().max(tg.size().x + 6.0).max(20.0), TITLE_H), Sense::hover());
                ui.painter().galley(tr.center() - tg.size() / 2.0, tg, ui.visuals().weak_text_color());
            })
            .response;
        self.widths[i][0] = r.rect.width();
    }
}

fn tr_or(key: &str) -> &str {
    crate::i18n::lookup(key).unwrap_or(key)
}

// --- buttons -----------------------------------------------------------------------

fn tip(resp: Response, label: &str, hover: &str) -> Response {
    widgets::tooltip(resp, label, hover)
}

/// Name and shortcut of a button, for a menu row.
fn names(label: &str, hover: &str) -> (String, String) {
    let (name, keys) = split_keys(hover);
    let name = if label.is_empty() { name } else { label.trim_end_matches(" ⏷") };
    (name.to_string(), keys.to_string())
}

/// The hover text of a compact button: its label when it has none.
fn compact_hover(label: &str, hover: &str) -> String {
    if hover.is_empty() { label.trim_end_matches(" ⏷").to_string() } else { hover.to_string() }
}

fn menu_job(ui: &egui::Ui, label: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::TextStyle::Button.resolve(ui.style());
    job.append(label, 24.0, egui::TextFormat { font_id: font, color: text_color(ui), ..Default::default() });
    job
}

fn menu_icon(ui: &egui::Ui, rect: Rect, icon: Option<Icon>, accent: Color32) {
    let Some(icon) = icon else { return };
    let r = Rect::from_min_size(pos2(rect.min.x + 5.0, rect.center().y - 8.0), vec2(16.0, 16.0));
    let color = text_color(ui);
    icons::paint_accent(ui.painter(), r, icon, color, if ui.is_enabled() { accent } else { color });
}

/// A row of a menu: icon, label and shortcut.
fn menu_row(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, keys: &str, selected: bool) -> Response {
    let resp = ui.add(egui::Button::new(menu_job(ui, label)).shortcut_text(keys).selected(selected).min_size(vec2(200.0, 22.0)));
    menu_icon(ui, resp.rect, icon, crate::theme::ACCENT);
    let hover = if keys.is_empty() { String::new() } else { format!("{label} ({keys})") };
    tip(resp, label, &hover)
}

/// A row of a menu that opens `content` beside it.
fn sub_menu(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, accent: Color32, content: impl FnOnce(&mut egui::Ui)) -> Response {
    sub_menu_dyn(ui, icon, label, accent, &mut widgets::once(content))
}

/// The body of [`sub_menu`], one for every caller.
fn sub_menu_dyn(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, accent: Color32, content: &mut dyn FnMut(&mut egui::Ui)) -> Response {
    let button = egui::Button::new(menu_job(ui, label.trim_end_matches('…'))).right_text(egui::containers::menu::SubMenuButton::RIGHT_ARROW).min_size(vec2(200.0, 22.0));
    let (resp, _) = egui::containers::menu::SubMenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(200.0);
        content(ui)
    });
    menu_icon(ui, resp.rect, icon, accent);
    tip(resp, label, "")
}

/// A split button as a submenu: its command first, then its menu.
fn split_menu(ui: &mut egui::Ui, icon: Option<Icon>, name: &str, keys: &str, accent: Color32, menu: impl FnOnce(&mut egui::Ui)) -> bool {
    split_menu_dyn(ui, icon, name, keys, accent, &mut widgets::once(menu))
}

/// The body of [`split_menu`], one for every caller.
fn split_menu_dyn(ui: &mut egui::Ui, icon: Option<Icon>, name: &str, keys: &str, accent: Color32, menu: &mut dyn FnMut(&mut egui::Ui)) -> bool {
    let mut clicked = false;
    sub_menu(ui, icon, name, accent, |ui| {
        clicked = menu_row(ui, icon, name, keys, false).clicked();
        ui.separator();
        menu(ui);
    });
    clicked
}

/// A compact button that opens `menu`.
fn compact_menu(ui: &mut egui::Ui, face: Face, hover: &str, forced: bool, menu: impl FnOnce(&mut egui::Ui)) -> Response {
    compact_menu_dyn(ui, face, hover, forced, &mut widgets::once(menu))
}

/// The body of [`compact_menu`], one for every caller.
fn compact_menu_dyn(ui: &mut egui::Ui, face: Face, hover: &str, forced: bool, menu: &mut dyn FnMut(&mut egui::Ui)) -> Response {
    let mut resp = widgets::tool_button(ui, face, false, hover);
    if forced {
        force_click(&mut resp);
    }
    widgets::button_menu(ui, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| with_style(Style::Toolstrip, || menu(ui)));
    resp
}

fn small_face(icon: Option<Icon>, label: &str) -> Face<'static> {
    match icon {
        Some(i) => Face::Icon(i),
        None => Face::Text(RichText::new(label).size(12.5)),
    }
}

fn hover_fill(ui: &egui::Ui, rect: Rect, selected: bool, hot: bool) {
    let v = ui.visuals();
    if selected {
        ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill.gamma_multiply(0.3));
    }
    if hot && ui.is_enabled() {
        ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
    }
}

fn text_color(ui: &egui::Ui) -> Color32 {
    if ui.is_enabled() { ui.visuals().text_color() } else { ui.visuals().weak_text_color() }
}

/// Splits a label into at most two lines for a large button: at the space
/// closest to the middle, or between two CJK characters (written without
/// spaces).
fn two_lines(label: &str) -> (String, Option<String>) {
    let label = label.trim_end_matches('…');
    // CJK characters are about twice as wide as Latin letters.
    let units = |s: &str| s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum::<usize>();
    let spaces: Vec<usize> = label.match_indices(' ').map(|(i, _)| i).collect();
    if units(label) <= 9 {
        return (label.to_string(), None);
    }
    if spaces.is_empty() {
        return match cjk_break(label) {
            Some(at) => (label[..at].to_string(), Some(label[at..].to_string())),
            None => (label.to_string(), None),
        };
    }
    let mid = label.len() / 2;
    let at = *spaces.iter().min_by_key(|&&i| i.abs_diff(mid)).unwrap();
    (label[..at].to_string(), Some(label[at + 1..].to_string()))
}

fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6)
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Narrow,
    Han,
    Hiragana,
    Katakana,
    Hangul,
    Punct,
}

fn kind(c: char) -> Kind {
    match c as u32 {
        0x3041..=0x309F => Kind::Hiragana,
        0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Kind::Katakana,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7A3 => Kind::Hangul,
        0x3000..=0x303F | 0xFF00..=0xFF65 => Kind::Punct,
        _ if is_wide(c) => Kind::Han,
        _ => Kind::Narrow,
    }
}

/// Where to break a label written without spaces, near the middle and
/// between "words": where Japanese changes script (not inside a kana word,
/// nor between a kanji and its kana ending), after a particle, between
/// CJK and Latin text; between two Han characters only in long labels.
/// None when there is no good place.
fn cjk_break(label: &str) -> Option<usize> {
    use Kind::*;
    let cs: Vec<(usize, char)> = label.char_indices().collect();
    let w = |c: char| if is_wide(c) { 2.0 } else { 1.0 };
    let total: f32 = cs.iter().map(|&(_, c)| w(c)).sum();
    let mut before = 0.0;
    let mut best: Option<(f32, usize, bool)> = None;
    for k in 1..cs.len() {
        before += w(cs[k - 1].1);
        let (prev, next) = (cs[k - 1].1, cs[k].1);
        let particle = "のとをにでへやが".contains(prev);
        let ok = match (kind(prev), kind(next)) {
            (Narrow, Narrow) | (Hangul, Hangul) | (Katakana, Katakana) | (Han, Hiragana) | (Katakana, Hiragana) | (_, Punct) => false,
            (Hiragana, Hiragana) => particle,
            (Punct, _) => !"「『（【〔".contains(prev),
            _ => !"/)]:,.;!?".contains(next) && !"([".contains(prev),
        };
        if !ok {
            continue;
        }
        let han = kind(prev) == Han && kind(next) == Han;
        let cost = (before - total / 2.0).abs() - if particle { 1.5 } else { 0.0 } + if han { 2.0 } else { 0.0 };
        if best.is_none_or(|(c, _, _)| cost < c) {
            best = Some((cost, cs[k].0, han));
        }
    }
    // Short runs of Han characters are often one word.
    best.filter(|&(_, _, han)| !han || total > 12.0).map(|(_, at, _)| at)
}

/// The lines of a large button's label, laid out.
fn large_lines(ui: &egui::Ui, label: &str, arrow: bool) -> Vec<std::sync::Arc<egui::Galley>> {
    let font = FontId::proportional(12.0);
    let color = text_color(ui);
    let (a, b) = two_lines(label);
    let mut lines = vec![a];
    lines.extend(b);
    if arrow {
        let last = lines.len() - 1;
        lines[last].push_str(" ⏷");
    }
    lines.into_iter().map(|l| ui.painter().layout_no_wrap(l, font.clone(), color)).collect()
}

fn large_size(gs: &[std::sync::Arc<egui::Galley>]) -> f32 {
    (gs.iter().map(|g| g.size().x).fold(LARGE_ICON, f32::max) + 10.0).max(46.0)
}

/// Width of a large button labelled `label`.
fn large_width(ui: &egui::Ui, label: &str, arrow: bool) -> f32 {
    large_size(&large_lines(ui, label, arrow))
}

fn large_rect(ui: &mut egui::Ui, label: &str, arrow: bool) -> (Rect, Vec<std::sync::Arc<egui::Galley>>) {
    let gs = large_lines(ui, label, arrow);
    let (rect, _) = ui.allocate_exact_size(vec2(large_size(&gs), ROWS_H), Sense::hover());
    (rect, gs)
}

fn paint_large(ui: &egui::Ui, rect: Rect, icon: Icon, accent: Color32, gs: &[std::sync::Arc<egui::Galley>]) {
    let color = text_color(ui);
    let accent = if ui.is_enabled() { accent } else { color };
    let ir = Rect::from_center_size(pos2(rect.center().x, rect.min.y + 6.0 + LARGE_ICON / 2.0), Vec2::splat(LARGE_ICON));
    icons::paint_accent(ui.painter(), ir, icon, color, accent);
    let mut y = ir.max.y + 5.0;
    for g in gs {
        ui.painter().galley(pos2(rect.center().x - g.size().x / 2.0, y), g.clone(), color);
        y += g.size().y - 1.0;
    }
}

/// A large button: icon over a one or two line label.
pub fn large(ui: &mut egui::Ui, icon: Icon, label: &str, hover: &str, accent: Color32, selected: bool) -> Response {
    match style() {
        Style::Menu => {
            let (n, k) = names(label, hover);
            let mut resp = menu_row(ui, Some(icon), &n, &k, selected);
            if register_on(ui, label, hover, Some(icon), false, selected) {
                force_click(&mut resp);
            }
            return resp;
        }
        Style::Compact => return small_compact(ui, Some(icon), label, hover, selected),
        Style::Toolstrip => {}
    }
    let (rect, gs) = large_rect(ui, label, false);
    let mut resp = ui.interact(rect, ui.id().with(("large", label)), Sense::click());
    if register_on(ui, label, hover, Some(icon), false, selected) {
        force_click(&mut resp);
    }
    keytips::place(rect);
    hover_fill(ui, rect, selected, resp.hovered());
    paint_large(ui, rect, icon, accent, &gs);
    let resp = tip(resp, label, hover);
    if selected {
        widgets::name(&resp, egui::WidgetType::Button, label, Some(true));
    }
    resp
}

/// A large button that opens `menu`.
pub fn large_menu(ui: &mut egui::Ui, icon: Icon, label: &str, hover: &str, accent: Color32, menu: impl FnOnce(&mut egui::Ui)) -> Response {
    large_menu_dyn(ui, icon, label, hover, accent, &mut widgets::once(menu))
}

/// The body of [`large_menu`], one for every caller.
fn large_menu_dyn(ui: &mut egui::Ui, icon: Icon, label: &str, hover: &str, accent: Color32, menu: &mut dyn FnMut(&mut egui::Ui)) -> Response {
    match style() {
        Style::Menu => {
            register(ui, label, hover, Some(icon), true);
            return sub_menu(ui, Some(icon), &names(label, hover).0, accent, menu);
        }
        Style::Compact => {
            let forced = register(ui, label, hover, Some(icon), true);
            return compact_menu(ui, Face::Icon(icon), &compact_hover(label, hover), forced, menu);
        }
        Style::Toolstrip => {}
    }
    let (rect, gs) = large_rect(ui, label, true);
    let mut resp = ui.interact(rect, ui.id().with(("large_menu", label)), Sense::click());
    if register(ui, label, hover, Some(icon), true) {
        force_click(&mut resp);
    }
    keytips::place(rect);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    hover_fill(ui, rect, false, resp.hovered() || open);
    paint_large(ui, rect, icon, accent, &gs);
    let resp = tip(resp, label, hover);
    menu_popup(ui, &resp, label, menu);
    resp
}

/// A large button in two parts: the icon runs the command (returns true),
/// the label opens `menu`.
pub fn large_split(ui: &mut egui::Ui, icon: Icon, label: &str, hover: &str, accent: Color32, menu: impl FnOnce(&mut egui::Ui)) -> bool {
    large_split_dyn(ui, icon, label, hover, accent, &mut widgets::once(menu))
}

/// The body of [`large_split`], one for every caller.
fn large_split_dyn(ui: &mut egui::Ui, icon: Icon, label: &str, hover: &str, accent: Color32, menu: &mut dyn FnMut(&mut egui::Ui)) -> bool {
    match style() {
        Style::Menu => {
            let forced = register(ui, label, hover, Some(icon), false);
            let (n, k) = names(label, hover);
            return split_menu(ui, Some(icon), &n, &k, accent, menu) || forced;
        }
        Style::Compact => {
            let forced = register(ui, label, hover, Some(icon), false);
            return widgets::split_button(ui, Face::Icon(icon), false, &compact_hover(label, hover), |ui| with_style(Style::Toolstrip, || menu(ui))) || forced;
        }
        Style::Toolstrip => {}
    }
    let (rect, gs) = large_rect(ui, label, true);
    let split = rect.min.y + LARGE_ICON + 9.0;
    let top = Rect::from_min_max(rect.min, pos2(rect.max.x, split));
    let bottom = Rect::from_min_max(pos2(rect.min.x, split), rect.max);
    let id = ui.id().with(("large_split", label));
    let mut main = ui.interact(top, id.with(0), Sense::click());
    if register(ui, label, hover, Some(icon), false) {
        force_click(&mut main);
    }
    keytips::place(rect);
    let arrow = ui.interact(bottom, id.with(1), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&arrow));
    if (main.hovered() || arrow.hovered() || open) && ui.is_enabled() {
        let v = ui.visuals();
        ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, v.widgets.hovered.bg_stroke.color.gamma_multiply(0.5)), egui::StrokeKind::Inside);
        ui.painter().rect_filled(if main.hovered() { top } else { bottom }, 4.0, v.widgets.hovered.weak_bg_fill);
    }
    paint_large(ui, rect, icon, accent, &gs);
    let clicked = tip(main, label, hover).clicked();
    let arrow = tip(arrow, label, hover);
    widgets::name(&arrow, egui::WidgetType::ComboBox, label, None);
    menu_popup(ui, &arrow, label, menu);
    clicked
}

/// A column of up to three small rows.
pub fn column<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if style() == Style::Compact {
        return ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
            add(ui)
        })
        .inner;
    }
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 1.0);
        add(ui)
    })
    .inner
}

/// A row of small (usually icon-only) buttons inside a column.
pub fn row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if style() == Style::Menu {
        return ui.vertical(add).inner;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
        ui.set_min_height(ROW_H);
        add(ui)
    })
    .inner
}

/// Widest label of a small button: a longer one ends in "…" (its
/// tooltip has it whole), so one long translation doesn't widen a group.
const SMALL_LABEL_W: f32 = 250.0;

fn small_rect(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, arrow: bool) -> (Rect, std::sync::Arc<egui::Galley>) {
    let font = FontId::proportional(12.5);
    let mut text = label.to_string();
    if ui.painter().layout_no_wrap(text.clone(), font.clone(), text_color(ui)).size().x > SMALL_LABEL_W {
        let mut job = egui::text::LayoutJob::single_section(text, egui::TextFormat::simple(font.clone(), text_color(ui)));
        job.wrap = egui::text::TextWrapping { max_width: SMALL_LABEL_W, max_rows: 1, break_anywhere: true, overflow_character: Some('…'), ..Default::default() };
        text = ui.painter().layout_job(job).rows.iter().flat_map(|r| r.glyphs.iter().map(|g| g.chr)).collect();
    }
    if arrow {
        text.push_str(" ⏷");
    }
    let g = ui.painter().layout_no_wrap(text, font, text_color(ui));
    let w = g.size().x + if icon.is_some() { 30.0 } else { 12.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
    (rect, g)
}

fn paint_small(ui: &egui::Ui, rect: Rect, icon: Option<Icon>, accent: Color32, g: std::sync::Arc<egui::Galley>) {
    let color = text_color(ui);
    let mut x = rect.min.x + 6.0;
    if let Some(i) = icon {
        let accent = if ui.is_enabled() { accent } else { color };
        icons::paint_accent(ui.painter(), Rect::from_min_size(pos2(x, rect.center().y - 8.0), vec2(16.0, 16.0)), i, color, accent);
        x += 22.0;
    }
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
}

/// The command `add` draws, off unless `on`: while off, resting the
/// pointer on it says `why`.
pub fn enabled(ui: &mut egui::Ui, on: bool, why: &str, add: impl FnOnce(&mut egui::Ui) -> Response) -> Response {
    ui.add_enabled_ui(on, add).inner.on_disabled_hover_text(why)
}

/// A small button: icon and label in one row.
pub fn small(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, hover: &str, accent: Color32, selected: bool) -> Response {
    match style() {
        Style::Menu => {
            let (n, k) = names(label, hover);
            let mut resp = menu_row(ui, icon, &n, &k, selected);
            if register_on(ui, label, hover, icon, false, selected) {
                force_click(&mut resp);
            }
            return resp;
        }
        Style::Compact => return small_compact(ui, icon, label, hover, selected),
        Style::Toolstrip => {}
    }
    let (rect, g) = small_rect(ui, icon, label, false);
    let mut resp = ui.interact(rect, ui.id().with(("small", label, hover)), Sense::click());
    if register_on(ui, label, hover, icon, false, selected) {
        force_click(&mut resp);
    }
    keytips::place(rect);
    hover_fill(ui, rect, selected, resp.hovered());
    paint_small(ui, rect, icon, accent, g);
    let resp = tip(resp, label, hover);
    if selected {
        widgets::name(&resp, egui::WidgetType::Button, if label.is_empty() { split_keys(hover).0 } else { label }, Some(true));
    }
    resp
}

/// A compact button for a large or small one.
fn small_compact(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, hover: &str, selected: bool) -> Response {
    let forced = register_on(ui, label, hover, icon, false, selected);
    let mut resp = widgets::tool_button(ui, small_face(icon, label), selected, &compact_hover(label, hover));
    if forced {
        force_click(&mut resp);
    }
    resp
}

/// A small button that opens `menu`.
pub fn small_menu(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, hover: &str, accent: Color32, menu: impl FnOnce(&mut egui::Ui)) -> Response {
    small_menu_dyn(ui, icon, label, hover, accent, &mut widgets::once(menu))
}

/// The body of [`small_menu`], one for every caller.
fn small_menu_dyn(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, hover: &str, accent: Color32, menu: &mut dyn FnMut(&mut egui::Ui)) -> Response {
    match style() {
        Style::Menu => {
            register(ui, label, hover, icon, true);
            return sub_menu(ui, icon, &names(label, hover).0, accent, menu);
        }
        Style::Compact => {
            let forced = register(ui, label, hover, icon, true);
            let face = if icon.is_some() { small_face(icon, label) } else { small_face(None, &format!("{label} ⏷")) };
            return compact_menu(ui, face, &compact_hover(label, hover), forced, menu);
        }
        Style::Toolstrip => {}
    }
    let (rect, g) = small_rect(ui, icon, label, true);
    let mut resp = ui.interact(rect, ui.id().with(("small_menu", label, hover)), Sense::click());
    if register(ui, label, hover, icon, true) {
        force_click(&mut resp);
    }
    keytips::place(rect);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    hover_fill(ui, rect, false, resp.hovered() || open);
    paint_small(ui, rect, icon, accent, g);
    let resp = tip(resp, label, hover);
    menu_popup(ui, &resp, if label.is_empty() { split_keys(hover).0 } else { label }, menu);
    resp
}

/// A small button of a menu (the File menu) that opens `menu` beside it,
/// as the pointer comes to it or with a click: a popup of its own would
/// close the menu it is in.
pub fn small_sub_menu(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, accent: Color32, menu: impl FnOnce(&mut egui::Ui)) -> Response {
    small_sub_menu_dyn(ui, icon, label, accent, &mut widgets::once(menu))
}

/// The body of [`small_sub_menu`], one for every caller.
fn small_sub_menu_dyn(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, accent: Color32, menu: &mut dyn FnMut(&mut egui::Ui)) -> Response {
    if style() != Style::Toolstrip {
        return small_menu(ui, icon, label, "", accent, menu);
    }
    let (rect, g) = small_rect(ui, icon, label, false);
    // The whole row, so the submenu opens beside the menu.
    let rect = Rect::from_min_size(rect.min, vec2(rect.width().max(ui.available_width()), rect.height()));
    let mut resp = ui.interact(rect, ui.id().with(("small_sub_menu", label)), Sense::click());
    if register(ui, label, "", icon, true) {
        force_click(&mut resp);
    }
    keytips::place(rect);
    hover_fill(ui, rect, false, resp.hovered());
    paint_small(ui, rect, icon, accent, g);
    let arrow = egui::containers::menu::SubMenuButton::RIGHT_ARROW;
    ui.painter().text(pos2(rect.max.x - 6.0, rect.center().y), egui::Align2::RIGHT_CENTER, arrow, FontId::proportional(12.5), text_color(ui));
    // Named as egui names its submenu buttons.
    widgets::name(&resp, egui::WidgetType::Button, &format!("{label} {arrow}"), None);
    if offscreen() {
        within(label.trim_end_matches('…'), || menu(ui));
    } else {
        egui::containers::menu::SubMenu::new().show(ui, &resp, |ui| {
            ui.set_min_width(200.0);
            menu(ui)
        });
    }
    resp
}

/// A small toggle (ruler, gridlines...): a small button shown pressed
/// while `on`, like the other toggles. Whether it was clicked.
pub fn check(ui: &mut egui::Ui, on: bool, icon: Icon, label: &str) -> bool {
    small(ui, Some(icon), label, "", accent(), on).clicked()
}

/// A number of the toolstrip (a duration...): icon and label as on a
/// small button, then the value. Whether it changed.
pub fn number(ui: &mut egui::Ui, icon: Icon, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
        ui.set_min_height(ROW_H);
        let shown = if style() == Style::Compact { "" } else { label };
        let (rect, g) = small_rect(ui, Some(icon), shown, false);
        paint_small(ui, rect, Some(icon), accent(), g);
        let r = number_field(ui, value, range, suffix);
        widgets::name(&r, egui::WidgetType::DragValue, label, None);
        r.on_hover_text(label).changed()
    })
    .inner
}

/// The value box of [`number`] alone (after a toggle, say).
pub fn number_field(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str) -> Response {
    let drag = egui::DragValue::new(value).range(range).speed(0.05).max_decimals(2).suffix(suffix);
    // As high as a row's button, so three rows still fill a group.
    ui.scope(|ui| {
        let s = ui.spacing_mut();
        s.interact_size.y = ROW_H - 4.0;
        s.button_padding.y = 1.0;
        ui.add_sized(vec2(56.0, ROW_H - 4.0), drag)
    })
    .inner
}

/// The accent of the toolstrip being drawn, for buttons drawn by code
/// shared between apps.
pub fn accent() -> Color32 {
    STATE.with_borrow(|s| s.accent).unwrap_or(crate::theme::ACCENT)
}

/// A menu entry with an icon; a command of the search too.
pub fn menu_item(ui: &mut egui::Ui, icon: Icon, label: &str) -> Response {
    menu_item_keys(ui, icon, label, "")
}

/// [`menu_item`] with a shortcut shown on the right.
pub fn menu_item_keys(ui: &mut egui::Ui, icon: Icon, label: &str, keys: &str) -> Response {
    menu_item_on(ui, icon, label, keys, false)
}

/// [`menu_item_keys`] for one of several modes: the one in use (`on`)
/// shows chosen.
pub fn menu_item_on(ui: &mut egui::Ui, icon: Icon, label: &str, keys: &str, on: bool) -> Response {
    let hover = if keys.is_empty() { String::new() } else { format!("{label} ({keys})") };
    let forced = register_on(ui, label, &hover, Some(icon), false, on);
    let mut resp = menu_row(ui, Some(icon), label, keys, on);
    if forced {
        force_click(&mut resp);
    }
    resp
}

// --- compact buttons (`widgets` faces), as commands too -----------------------------

fn face_icon(face: &Face) -> Option<Icon> {
    match face {
        Face::Icon(i) | Face::IconBar(i, _) => Some(*i),
        _ => None,
    }
}

/// [`widgets::tool_button`] as a command.
pub fn tool(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str) -> Response {
    let forced = register_on(ui, "", hover, face_icon(&face), false, selected);
    let mut resp = if style() == Style::Menu {
        let (n, k) = split_keys(hover);
        menu_row(ui, face_icon(&face), n, k, selected)
    } else {
        widgets::tool_button(ui, face, selected, hover)
    };
    if forced {
        force_click(&mut resp);
    }
    resp
}

/// A compact button that opens `menu`.
pub fn tool_menu(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str, menu: impl FnOnce(&mut egui::Ui)) -> Response {
    tool_menu_dyn(ui, face, selected, hover, &mut widgets::once(menu))
}

/// The body of [`tool_menu`], one for every caller.
fn tool_menu_dyn(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str, menu: &mut dyn FnMut(&mut egui::Ui)) -> Response {
    let forced = register(ui, "", hover, face_icon(&face), true);
    if style() == Style::Menu {
        return sub_menu(ui, face_icon(&face), split_keys(hover).0, crate::theme::ACCENT, menu);
    }
    let mut resp = widgets::tool_button(ui, face, selected, hover);
    if forced {
        force_click(&mut resp);
    }
    menu_popup(ui, &resp, split_keys(hover).0, menu);
    resp
}

/// [`widgets::split_button`] as a command.
pub fn split(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str, menu: impl FnOnce(&mut egui::Ui)) -> bool {
    split_dyn(ui, face, selected, hover, &mut widgets::once(menu))
}

/// The body of [`split`], one for every caller.
fn split_dyn(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str, menu: &mut dyn FnMut(&mut egui::Ui)) -> bool {
    let forced = register_on(ui, "", hover, face_icon(&face), false, selected);
    if style() == Style::Menu {
        let (n, k) = split_keys(hover);
        return split_menu(ui, face_icon(&face), n, k, crate::theme::ACCENT, menu) || forced;
    }
    if offscreen() {
        widgets::split_button(ui, face, selected, hover, |_| {});
        within(split_keys(hover).0, || menu(ui));
        return forced;
    }
    widgets::split_button(ui, face, selected, hover, menu) || forced
}

#[cfg(test)]
mod label_tests {
    use super::two_lines;

    #[test]
    fn splits_large_labels() {
        assert_eq!(two_lines("Picture in Cell"), ("Picture".into(), Some("in Cell".into())));
        assert_eq!(two_lines("Chart"), ("Chart".into(), None));
        assert_eq!(two_lines("セルの上に配置"), ("セルの".into(), Some("上に配置".into())));
        assert_eq!(two_lines("新しいメモ"), ("新しい".into(), Some("メモ".into())));
        assert_eq!(two_lines("データ分析"), ("データ".into(), Some("分析".into())));
        assert_eq!(two_lines("フィルター"), ("フィルター".into(), None));
        assert_eq!(two_lines("再計算実行"), ("再計算実行".into(), None));
        assert_eq!(two_lines("数据透视表"), ("数据透视表".into(), None));
        assert_eq!(two_lines("插入单元格内的图片"), ("插入单元".into(), Some("格内的图片".into())));
        assert_eq!(two_lines("表格"), ("表格".into(), None));
        assert_eq!(two_lines("오프라인"), ("오프라인".into(), None));
        assert_eq!(two_lines("머리글/바닥글"), ("머리글/".into(), Some("바닥글".into())));
    }
}
