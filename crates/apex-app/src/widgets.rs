//! Shared toolbar widgets: split buttons, the color picker, the adaptive
//! toolbar that moves groups that don't fit into a "»" menu, and dialogs.

use std::sync::Mutex;

use eframe::egui::{self, Color32, Id, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme;

// --- split buttons -------------------------------------------------------------

/// What a toolbar button shows.
pub enum Face<'a> {
    Icon(Icon),
    Text(RichText),
    /// Icon with a color bar under it (fill, highlight...).
    IconBar(Icon, Color32),
    /// Text with a color bar under it (text color).
    TextBar(RichText, Color32),
    #[allow(dead_code)]
    Label(&'a str),
}

fn paint_face(ui: &egui::Ui, rect: Rect, face: &Face, enabled: bool) {
    let v = ui.visuals();
    let color = if enabled { v.text_color() } else { v.weak_text_color() };
    match face {
        Face::Icon(i) => icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), *i, color),
        Face::IconBar(i, bar) => {
            icons::paint(ui.painter(), Rect::from_center_size(rect.center() - vec2(0.0, 2.0), vec2(16.0, 16.0)), *i, color);
            ui.painter().rect_filled(Rect::from_min_size(pos2(rect.center().x - 8.0, rect.max.y - 5.0), vec2(16.0, 3.0)), 0.0, *bar);
        }
        Face::Text(t) => {
            let g = egui::WidgetText::from(t.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
            ui.painter().galley(rect.center() - g.size() / 2.0, g, color);
        }
        Face::TextBar(t, bar) => {
            let g = egui::WidgetText::from(t.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
            ui.painter().galley(rect.center() - g.size() / 2.0 - vec2(0.0, 2.0), g, color);
            ui.painter().rect_filled(Rect::from_min_size(pos2(rect.center().x - 8.0, rect.max.y - 5.0), vec2(16.0, 3.0)), 0.0, *bar);
        }
        Face::Label(s) => {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, *s, egui::FontId::proportional(13.0), color);
        }
    }
}

fn face_width(ui: &egui::Ui, face: &Face) -> f32 {
    match face {
        Face::Text(t) => {
            let g = egui::WidgetText::from(t.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
            (g.size().x + 12.0).max(26.0)
        }
        Face::Label(s) => s.chars().count() as f32 * 7.5 + 12.0,
        _ => 26.0,
    }
}

/// Names a widget drawn by hand for screen readers: its kind, its name
/// and, for one that is on or chosen, that it is.
pub fn name(resp: &egui::Response, typ: egui::WidgetType, name: &str, selected: Option<bool>) {
    let enabled = resp.enabled();
    resp.widget_info(|| {
        let mut i = egui::WidgetInfo::labeled(typ, enabled, name);
        i.selected = selected;
        i
    });
}

/// Names a drop-down that has no label of its own (`ComboBox::from_id_salt`)
/// after the label shown beside it, so a screen reader says what it picks.
pub fn combo_name<R>(r: egui::InnerResponse<R>, label: &str) -> egui::InnerResponse<R> {
    name(&r.response, egui::WidgetType::ComboBox, label, None);
    r
}

/// The live region that says what the cursor moved to.
fn live_id() -> egui::Id {
    egui::Id::new("apex_a11y_live")
}

/// Whether a screen reader is listening (AccessKit is on this frame).
pub fn a11y_on(ctx: &egui::Context) -> bool {
    ctx.accesskit_node_builder(live_id(), |_| ()).is_some()
}

/// A canvas drawn by hand (the grid, the page): a group with a name.
pub fn a11y_canvas(ctx: &egui::Context, resp: &egui::Response, name: &str) {
    ctx.accesskit_node_builder(resp.id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_label(name);
    });
}

/// Says `text` through the screen reader when it differs from last
/// frame's: the cell or the text the cursor has moved to. Call it every
/// frame with what is under the cursor.
pub fn announce(ctx: &egui::Context, text: &str) {
    ctx.accesskit_node_builder(live_id(), |n| {
        n.set_role(egui::accesskit::Role::Status);
        n.set_live(egui::accesskit::Live::Polite);
        n.set_label(text);
    });
}

/// A plain toolbar button with any face.
pub fn tool_button(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(face_width(ui, &face), 24.0), Sense::click());
    let enabled = ui.is_enabled();
    let v = ui.visuals();
    if selected {
        ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill.gamma_multiply(0.35));
    } else if resp.hovered() && enabled {
        ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
    }
    paint_face(ui, rect, &face, enabled);
    let resp = tooltip(resp, "", hover);
    if selected {
        name(&resp, egui::WidgetType::Button, split_shortcut(hover).0, Some(true));
    }
    resp
}

/// A button with a main part (returns true when clicked) and an arrow that
/// opens `menu`.
pub fn split_button(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str, menu: impl FnOnce(&mut egui::Ui)) -> bool {
    split_button_dyn(ui, face, selected, hover, &mut once(menu))
}

/// The body of [`split_button`], one for every caller.
fn split_button_dyn(ui: &mut egui::Ui, face: Face, selected: bool, hover: &str, menu: &mut dyn FnMut(&mut egui::Ui)) -> bool {
    let w = face_width(ui, &face);
    let (rect, _) = ui.allocate_exact_size(vec2(w + 12.0, 24.0), Sense::hover());
    let main_rect = Rect::from_min_size(rect.min, vec2(w, 24.0));
    let arrow_rect = Rect::from_min_max(pos2(main_rect.max.x, rect.min.y), rect.max);
    let id = ui.id().with(("split", rect.min.x as i32, rect.min.y as i32));
    let main = ui.interact(main_rect, id.with(0), Sense::click());
    let arrow = ui.interact(arrow_rect, id.with(1), Sense::click());
    let enabled = ui.is_enabled();
    let v = ui.visuals();
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&arrow));
    if selected {
        ui.painter().rect_filled(main_rect, 4.0, v.selection.bg_fill.gamma_multiply(0.35));
    }
    if (main.hovered() || arrow.hovered() || open) && enabled {
        ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, v.widgets.hovered.bg_stroke.color.gamma_multiply(0.6)), egui::StrokeKind::Inside);
        let hot = if arrow.hovered() || open { arrow_rect } else { main_rect };
        ui.painter().rect_filled(hot, 4.0, v.widgets.hovered.weak_bg_fill);
    }
    paint_face(ui, main_rect, &face, enabled);
    let c = arrow_rect.center();
    let col = if enabled { v.text_color() } else { v.weak_text_color() };
    ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(-3.0, -1.5), c + vec2(3.0, -1.5), c + vec2(0.0, 2.0)], col, Stroke::NONE));
    let clicked = tooltip(main, "", hover).clicked();
    let arrow = tooltip(arrow, "", hover);
    name(&arrow, egui::WidgetType::ComboBox, split_shortcut(hover).0, None);
    button_menu(ui, &arrow, egui::PopupCloseBehavior::CloseOnClick, menu);
    clicked
}

/// The menu of a button. Inside another menu (a folded toolstrip group)
/// it opens as a submenu: egui keeps one popup open at a time, so a popup
/// of its own would close the menu holding the button, and itself with it.
pub fn button_menu(ui: &egui::Ui, resp: &egui::Response, close: egui::PopupCloseBehavior, menu: impl FnOnce(&mut egui::Ui)) {
    if egui::containers::menu::is_in_menu(ui) {
        egui::containers::menu::SubMenu::new().show(ui, resp, menu);
    } else {
        egui::Popup::menu(resp).close_behavior(close).show(menu);
    }
}

/// A combo box of the toolstrip. Inside a menu (a folded group) it is a
/// button with a submenu, as in [`button_menu`]: the combo's popup would
/// close the menu holding it.
pub fn combo_box(ui: &mut egui::Ui, id_salt: impl std::hash::Hash + std::fmt::Debug, width: f32, selected: impl Into<egui::WidgetText>, content: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    if !egui::containers::menu::is_in_menu(ui) {
        return egui::ComboBox::from_id_salt(id_salt).width(width).selected_text(selected).show_ui(ui, content).response;
    }
    let label = selected.into().text().to_string();
    let resp = ui.push_id(id_salt, |ui| ui.add(egui::Button::new(format!("{label} ⏷")).min_size(egui::vec2(width, 0.0)))).inner;
    name(&resp, egui::WidgetType::ComboBox, &label, None);
    egui::containers::menu::SubMenu::new().show(ui, &resp, content);
    resp
}

/// The tooltip of a button: its name in bold with the shortcut, and the
/// hover text when it says more than the label.
pub fn tooltip(resp: egui::Response, label: &str, hover: &str) -> egui::Response {
    let (hname, keys) = split_shortcut(hover);
    let title = if label.is_empty() { hname } else { label.trim_end_matches(" ⏷").trim_end_matches('…') };
    let more = (!label.is_empty() && !hname.is_empty() && hname.trim_end_matches('…') != title).then_some(hname);
    if !title.is_empty() {
        name(&resp, egui::WidgetType::Button, title, None);
    }
    if title.is_empty() && more.is_none() {
        return resp;
    }
    resp.on_hover_ui(|ui| {
        ui.set_max_width(320.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            if !keys.is_empty() {
                ui.label(RichText::new(format!("({keys})")).weak());
            }
        });
        if let Some(m) = more {
            ui.label(m);
        }
    })
}

/// "Bold (Ctrl+B)" -> ("Bold", "Ctrl+B").
pub fn split_shortcut(hover: &str) -> (&str, &str) {
    if let Some(i) = hover.rfind(" (")
        && hover.ends_with(')')
    {
        return (&hover[..i], &hover[i + 2..hover.len() - 1]);
    }
    (hover, "")
}

// --- colors -----------------------------------------------------------------------

static RECENT: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static COLORS_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True once after the recent colors changed.
pub fn take_colors_dirty() -> bool {
    COLORS_DIRTY.swap(false, std::sync::atomic::Ordering::Relaxed)
}

pub fn recent_colors() -> Vec<u32> {
    RECENT.lock().map(|v| v.clone()).unwrap_or_default()
}

pub fn set_recent_colors(v: Vec<u32>) {
    if let Ok(mut r) = RECENT.lock() {
        *r = v;
    }
}

fn push_recent(c: u32) {
    if let Ok(mut r) = RECENT.lock() {
        r.retain(|&x| x != c);
        r.insert(0, c);
        r.truncate(10);
    }
    COLORS_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
}

const STANDARD: [u32; 10] = [0xC00000, 0xFF0000, 0xFFC000, 0xFFFF00, 0x92D050, 0x00B050, 0x00B0F0, 0x0070C0, 0x002060, 0x7030A0];

fn swatch(ui: &mut egui::Ui, c: u32, current: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
    name(&resp, egui::WidgetType::ColorButton, &format!("#{c:06X}"), Some(current));
    ui.painter().rect_filled(rect, 2.0, theme::rgb(c));
    let stroke = if resp.hovered() || current { Stroke::new(2.0, theme::ACCENT) } else { Stroke::new(1.0, Color32::from_gray(160)) };
    ui.painter().rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    resp.on_hover_text(format!("#{c:06X}")).clicked()
}

#[derive(Clone, Default)]
struct CustomColor {
    open: bool,
    color: Option<Color32>,
    hex: String,
}

/// The color menu: automatic, theme and standard colors, recent colors and
/// "More colors…" (RGB/HSV and hex). Returns `Some(None)` for automatic.
pub fn color_menu(ui: &mut egui::Ui, auto_label: &str, current: Option<u32>) -> Option<Option<u32>> {
    let mut out = None;
    ui.set_min_width(200.0);
    if ui.button(auto_label).clicked() {
        out = Some(None);
    }
    ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
    // The organization's colors first.
    let brand = crate::brand::get().colors;
    if !brand.is_empty() {
        ui.label(RichText::new(tr("color.brand")).small().weak());
        for row in brand.chunks(10) {
            ui.horizontal(|ui| {
                for &c in row {
                    if swatch(ui, c, current == Some(c)) {
                        out = Some(Some(c));
                    }
                }
            });
        }
    }
    ui.label(RichText::new(tr("color.theme")).small().weak());
    let t = crate::themes_ui::active();
    for (k, row) in t.palette().iter().enumerate() {
        if k == 1 {
            ui.add_space(3.0);
        }
        ui.horizontal(|ui| {
            for tc in row {
                let c = t.resolve(*tc);
                if swatch(ui, c, current == Some(c)) {
                    out = Some(Some(c));
                }
            }
        });
    }
    ui.label(RichText::new(tr("color.standard")).small().weak());
    ui.horizontal(|ui| {
        for c in STANDARD {
            if swatch(ui, c, current == Some(c)) {
                out = Some(Some(c));
            }
        }
    });
    let recent = recent_colors();
    if !recent.is_empty() {
        ui.label(RichText::new(tr("color.recent")).small().weak());
        ui.horizontal(|ui| {
            for c in recent {
                if swatch(ui, c, current == Some(c)) {
                    out = Some(Some(c));
                }
            }
        });
    }
    ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
    let id = ui.id().with("custom_color");
    let mut st: CustomColor = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if ui.button(format!("{}…", tr("color.more"))).clicked() {
        st.open = !st.open;
        let c = current.map_or(Color32::from_rgb(0x44, 0x72, 0xC4), theme::rgb);
        st.color = Some(c);
        st.hex = format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
    }
    if st.open {
        let mut c = st.color.unwrap_or(Color32::GRAY);
        if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
            st.hex = format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
        }
        ui.horizontal(|ui| {
            ui.label("#");
            let r = ui.add(egui::TextEdit::singleline(&mut st.hex).desired_width(70.0).char_limit(7));
            if r.changed() {
                if let Ok(v) = u32::from_str_radix(st.hex.trim_start_matches('#'), 16) {
                    if st.hex.trim_start_matches('#').len() == 6 {
                        c = theme::rgb(v);
                    }
                }
            }
            if ui.button(tr("dialog.ok")).clicked() {
                out = Some(Some(((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32));
            }
        });
        st.color = Some(c);
    }
    if out.is_some() {
        st.open = false;
    }
    ui.data_mut(|d| d.insert_temp(id, st));
    if let Some(Some(c)) = out {
        push_recent(c);
    }
    if out.is_some() {
        ui.close();
    }
    out
}

// --- adaptive toolbar ----------------------------------------------------------------

// --- dialogs ---------------------------------------------------------------------------

/// Result of a dialog frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogResult {
    None,
    Ok,
    Cancel,
}

/// Escape closes the floating window `id` (its `Window::id`, or its title
/// when it has none) when it is the front one and nothing (a menu, a
/// dialog) is over it. Called before showing it.
pub fn esc_closes(ctx: &egui::Context, id: egui::Id, open: &mut bool) {
    let layer = egui::LayerId::new(egui::Order::Middle, id);
    let front = ctx.memory(|m| m.top_modal_layer().is_none() && m.layer_ids().filter(|l| l.order == egui::Order::Middle && m.areas().visible_last_frame(l)).last() == Some(layer));
    if *open && front && !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        *open = false;
    }
}

/// A modal dialog with a title, content and OK/Cancel buttons (Enter and
/// Escape work too).
pub fn dialog(ctx: &egui::Context, id: &str, title: &str, width: f32, content: impl FnOnce(&mut egui::Ui)) -> DialogResult {
    dialog_frame(ctx, id, title, width, false, content)
}

/// A modal dialog that only shows something: one Close button. Returns
/// whether it was closed.
pub fn info_dialog(ctx: &egui::Context, id: &str, title: &str, width: f32, content: impl FnOnce(&mut egui::Ui)) -> bool {
    dialog_frame(ctx, id, title, width, true, content) != DialogResult::None
}

/// How much smaller than the room a scrolled dialog's content gets before
/// it stops scrolling: near the edge it would go back and forth.
const SCROLL_SLACK: f32 = 24.0;

/// Room left between a dialog and the window's edges.
const DIALOG_EDGE: f32 = 8.0;

/// The most a dialog's content may take in this window: the window less
/// the dialog's frame, its title and its buttons. Wider dialogs narrow to
/// it and taller content scrolls, so nothing falls out of a small window.
pub fn dialog_room(ctx: &egui::Context) -> Vec2 {
    let style = ctx.global_style();
    let frame = style.spacing.menu_margin.sum() + Vec2::splat(2.0 * DIALOG_EDGE + 2.0);
    let heading = style.text_styles.get(&egui::TextStyle::Heading).map_or(20.0, |f| f.size) * 1.3;
    let chrome = heading + 8.0 + 10.0 + style.spacing.interact_size.y + 3.0 * style.spacing.item_spacing.y;
    (ctx.content_rect().size() - frame - vec2(0.0, chrome)).max(vec2(160.0, 80.0))
}

/// Windows on a narrow screen (a phone's): see [`FitWindow::show_fit`].
pub trait FitWindow {
    fn show_fit<R>(self, ctx: &egui::Context, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> Option<egui::InnerResponse<Option<R>>>;
}

/// Narrower than this (points), a screen is a phone's.
pub const NARROW_SCREEN: f32 = 600.0;

impl FitWindow for egui::Window<'_> {
    /// Shows the window. On a screen narrower than a tablet's it keeps
    /// within the screen, and once its contents don't fit they scroll.
    /// Elsewhere, as `show`. (egui's window scrolling fills all the room
    /// it is given, so it is only on while needed.)
    #[track_caller]
    fn show_fit<R>(self, ctx: &egui::Context, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> Option<egui::InnerResponse<Option<R>>> {
        if ctx.content_rect().width() >= NARROW_SCREEN {
            return self.show(ctx, add_contents);
        }
        let at = std::panic::Location::caller();
        let key = Id::new(("fit_window", at.file(), at.line(), at.column()));
        let room = dialog_room(ctx);
        let (wide, tall) = ctx.data(|d| d.get_temp::<(bool, bool)>(key)).unwrap_or_default();
        let mut window = self.constrain(true);
        if wide || tall {
            window = window.max_width(room.x).max_height(room.y).scroll([wide, tall]);
        }
        let mut used = None;
        let res = window.show(ctx, |ui| {
            let r = add_contents(ui);
            used = Some(ui.min_rect().size());
            r
        });
        if let Some(used) = used {
            // Scrolling stops once the contents fit with some room to spare.
            let over = |used: f32, room: f32, on: bool| used > room + 0.5 || (on && used > room - SCROLL_SLACK);
            let now = (over(used.x, room.x, wide), over(used.y, room.y, tall));
            if now != (wide, tall) {
                ctx.data_mut(|d| d.insert_temp(key, now));
                ctx.request_discard("window fits the screen");
            }
        }
        res
    }
}

fn dialog_frame(ctx: &egui::Context, id: &str, title: &str, width: f32, info: bool, content: impl FnOnce(&mut egui::Ui)) -> DialogResult {
    let mut res = DialogResult::None;
    // A dialog just opened from a menu (a folded group's) closes it: it
    // would stay open under the dialog.
    let seen_id = Id::new(("dialog", id, "seen"));
    let pass = ctx.cumulative_pass_nr();
    if ctx.data(|d| d.get_temp::<u64>(seen_id)).is_none_or(|last| last + 1 < pass) {
        egui::Popup::close_all(ctx);
    }
    ctx.data_mut(|d| d.insert_temp(seen_id, pass));
    let room = dialog_room(ctx);
    // A dialog that starts or stops scrolling gets a new area: the old one
    // would keep the size it had before.
    let scrolls = [("tall", 1u8), ("wide", 2)].iter().map(|(n, b)| if ctx.data(|d| d.get_temp::<bool>(Id::new(("dialog", id, *n)))).unwrap_or(false) { *b } else { 0 }).sum::<u8>();
    let mut scrolled = None;
    let m = egui::Modal::new(Id::new(("dialog", id)).with(scrolls)).show(ctx, |ui| {
        // Narrowed to the window, rows that don't fit (a form's label and
        // its field) keep their width and the dialog scrolls sideways:
        // wrapped, their labels would break letter by letter.
        ui.set_width(width.min(room.x));
        let head = ui.add(egui::Label::new(RichText::new(title).heading()).wrap()).rect.height();
        ui.add_space(8.0);
        // A title on more lines than one (a phone) leaves less room.
        let line = ui.style().text_styles.get(&egui::TextStyle::Heading).map_or(20.0, |f| f.size) * 1.3;
        let mut room = vec2(room.x, room.y - (head - line).max(0.0));
        // What the title, the buttons and the frame took, measured the frame
        // before, rather than guessed.
        let chrome_id = Id::new(("dialog", id, "chrome"));
        if let Some(chrome) = ui.data(|d| d.get_temp::<f32>(chrome_id)) {
            room.y = room.y.min(ui.ctx().content_rect().height() - 2.0 * DIALOG_EDGE - chrome).max(80.0);
        }
        // Content taller or wider than the window scrolls (known from the
        // frame before: dialogs that fit lay out as always).
        let tall_id = Id::new(("dialog", id, "tall"));
        let wide_id = Id::new(("dialog", id, "wide"));
        let tall = ui.data(|d| d.get_temp::<bool>(tall_id)).unwrap_or(false);
        let wide = ui.data(|d| d.get_temp::<bool>(wide_id)).unwrap_or(false);
        if tall || wide {
            // The bar of a wide one takes room under it.
            let bar = if wide { ui.spacing().scroll.allocated_width() } else { 0.0 };
            let r = egui::ScrollArea::new([wide, tall]).id_salt(("dialog", id, "scroll")).max_height(room.y - bar).max_width(room.x).auto_shrink([!wide, true]).show(ui, content);
            scrolled = Some(r.inner_rect.height());
            if tall && r.content_size.y < room.y - SCROLL_SLACK {
                ui.data_mut(|d| d.insert_temp(tall_id, false));
            }
            if wide && r.content_size.x < room.x - SCROLL_SLACK {
                ui.data_mut(|d| d.insert_temp(wide_id, false));
            }
        } else {
            let top = ui.cursor().min;
            content(ui);
            let used = ui.min_rect().max - top;
            if used.y > room.y {
                ui.data_mut(|d| d.insert_temp(tall_id, true));
                ui.ctx().request_discard("a dialog taller than the window");
            }
            if used.x > room.x + 0.5 {
                ui.data_mut(|d| d.insert_temp(wide_id, true));
                ui.ctx().request_discard("a dialog wider than the window");
            }
        }
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if info {
                if ui.button(RichText::new(tr("dialog.close")).strong()).clicked() {
                    res = DialogResult::Cancel;
                }
                return;
            }
            if ui.button(tr("unsaved.cancel")).clicked() {
                res = DialogResult::Cancel;
            }
            if ui.button(RichText::new(tr("dialog.ok")).strong()).clicked() {
                res = DialogResult::Ok;
            }
        });
    });
    // Centered by its size of the frame before: a new size is shown
    // only once laid out again, so the dialog doesn't jump.
    if let Some(h) = scrolled {
        ctx.data_mut(|d| d.insert_temp(Id::new(("dialog", id, "chrome")), m.response.rect.height() - h));
    }
    let size_id = Id::new(("dialog", id, "size"));
    let size = m.response.rect.size();
    if ctx.data(|d| d.get_temp::<Vec2>(size_id)) != Some(size) {
        ctx.data_mut(|d| d.insert_temp(size_id, size));
        ctx.request_discard("a dialog changed size");
    }
    // Only the dialog on top (another can open over it) takes the keys.
    if res == DialogResult::None && m.is_top_modal {
        // A click on a list that hangs out of the dialog is not a click outside it.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || m.should_close() && !m.any_popup_open && ctx.input(|i| i.pointer.any_click()) {
            res = DialogResult::Cancel;
        } else if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.memory(|m| m.focused().is_some_and(|f| ctx.data(|d| d.get_temp::<bool>(f.with("multiline")).unwrap_or(false)))) {
            res = DialogResult::Ok;
        }
    }
    res
}

/// Width of the label column of a [`form`].
pub const FORM_LABEL_W: f32 = 150.0;

/// Width of the page list of a [`nav_dialog`].
const NAV_W: f32 = 180.0;
/// Its width when it shows only the pages' icons (a small window).
const NAV_ICON_W: f32 = 42.0;

/// A dialog with its pages listed on the left, like File > Options: `pages`
/// are (icon, label key) and `page` the one shown. The page area has a fixed
/// `size` (it scrolls when taller), so the dialog doesn't jump between
/// pages.
pub fn nav_dialog(ctx: &egui::Context, id: &str, title: &str, pages: &[(Icon, &'static str)], page: &mut usize, size: Vec2, content: impl FnOnce(&mut egui::Ui, usize)) -> DialogResult {
    nav_dialog_aside(ctx, id, title, pages, page, size, 0.0, content, |_| {})
}

/// [`nav_dialog`] with a panel `aside_w` wide on the right that doesn't
/// scroll with the page (a preview).
#[allow(clippy::too_many_arguments)]
pub fn nav_dialog_aside(ctx: &egui::Context, id: &str, title: &str, pages: &[(Icon, &'static str)], page: &mut usize, size: Vec2, aside_w: f32, content: impl FnOnce(&mut egui::Ui, usize), aside: impl FnOnce(&mut egui::Ui)) -> DialogResult {
    // In a window too small for it the list shows only its icons, and the
    // page narrows and shortens (it scrolls).
    let room = dialog_room(ctx);
    // On a phone's screen the panel goes: the page needs the room.
    let aside_w = if room.x < NARROW_SCREEN - 100.0 { 0.0 } else { aside_w };
    let aside_room = if aside_w > 0.0 { aside_w + 20.0 } else { 0.0 };
    let nav_w = if NAV_W + 14.0 + size.x + aside_room > room.x { NAV_ICON_W } else { NAV_W };
    let size = vec2(size.x.min(room.x - nav_w - 14.0 - aside_room).max(160.0), size.y.min(room.y));
    let narrow = nav_w == NAV_ICON_W;
    let width = nav_w + 14.0 + size.x + aside_room;
    dialog(ctx, id, title, width, |ui| {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(nav_w);
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, (icon, key)) in pages.iter().enumerate() {
                    if nav_item(ui, *icon, tr(key), *page == i, nav_w).clicked() {
                        *page = i;
                    }
                }
            });
            let x = ui.cursor().min.x + 6.0;
            let top = ui.cursor().min.y;
            ui.painter().vline(x, top..=top + size.y, ui.visuals().widgets.noninteractive.bg_stroke);
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.set_width(size.x);
                ui.set_height(size.y);
                // Narrowed, rows that don't fit scroll sideways (wrapped,
                // a form's labels would break letter by letter).
                egui::ScrollArea::new([narrow, true]).id_salt((id, "page", *page)).auto_shrink([false, false]).max_height(size.y).max_width(size.x).show(ui, |ui| {
                    ui.set_min_width(size.x - 12.0);
                    content(ui, *page)
                });
            });
            if aside_w > 0.0 {
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    ui.set_width(aside_w);
                    aside(ui)
                });
            }
        });
    })
}

/// A page of a [`nav_dialog`]'s list, `w` wide: only its icon (the name
/// in a tooltip) when that is [`NAV_ICON_W`].
fn nav_item(ui: &mut egui::Ui, icon: Icon, label: &str, on: bool, w: f32) -> egui::Response {
    let icon_only = w <= NAV_ICON_W;
    let g = ui.painter().layout(if icon_only { String::new() } else { label.to_string() }, egui::FontId::proportional(13.5), Color32::PLACEHOLDER, w - 48.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, (g.size().y + 12.0).max(30.0)), Sense::click());
    name(&resp, egui::WidgetType::SelectableLabel, label, Some(on));
    let resp = if icon_only { resp.on_hover_text(label) } else { resp };
    let v = ui.visuals();
    if on {
        ui.painter().rect_filled(rect, 5.0, v.selection.bg_fill.gamma_multiply(0.35));
        ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 7.0), vec2(3.0, rect.height() - 14.0)), 1.5, v.selection.bg_fill);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, v.widgets.hovered.weak_bg_fill);
    }
    let color = if on { v.strong_text_color() } else { v.text_color() };
    icons::paint_accent(ui.painter(), Rect::from_min_size(pos2(rect.min.x + 12.0, rect.center().y - 9.0), vec2(18.0, 18.0)), icon, color, theme::ACCENT);
    ui.painter().galley(pos2(rect.min.x + 40.0, rect.center().y - g.size().y / 2.0), g, color);
    resp
}

/// A section title of a dialog page, with a line under it.
pub fn section(ui: &mut egui::Ui, title: &str) {
    if ui.cursor().min.y > ui.min_rect().min.y + 1.0 {
        ui.add_space(8.0);
    }
    ui.label(RichText::new(title).strong());
    let r = ui.available_rect_before_wrap();
    ui.painter().hline(r.min.x..=r.max.x, r.min.y + 2.0, ui.visuals().widgets.noninteractive.bg_stroke);
    ui.add_space(8.0);
}

/// Labels and fields of a dialog page in two aligned columns: call
/// [`form_row`] for each row.
pub fn form<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Grid::new(id).num_columns(2).min_col_width(FORM_LABEL_W).spacing([12.0, 8.0]).show(ui, add).inner
}

/// A row of a [`form`]: `label` and the field(s) `add` draws.
pub fn form_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.label(label);
    let r = ui.horizontal(add).inner;
    ui.end_row();
    r
}

/// A numeric field with a label.
pub fn number_row(ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).range(range).speed(0.5).suffix(suffix).max_decimals(2));
    ui.end_row();
}

/// Small colored square used in menus.
#[allow(dead_code)]
pub fn color_chip(ui: &mut egui::Ui, c: Option<u32>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    let _ = match c {
        Some(c) => ui.painter().rect_filled(rect, 2.0, theme::rgb(c)),
        None => ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside),
    };
}

/// `f` as a `FnMut` that runs it once. Menus take their contents as
/// `&mut dyn FnMut`: egui's menu code is then built once, not again for
/// each caller's closure (which made the program megabytes larger).
pub fn once<'a>(f: impl FnOnce(&mut egui::Ui) + 'a) -> impl FnMut(&mut egui::Ui) + 'a {
    let mut f = Some(f);
    move |ui| {
        if let Some(f) = f.take() {
            f(ui)
        }
    }
}

/// `ui.menu_button`, with egui's menu code built once for all callers.
pub fn menu_button<'a, R>(ui: &mut egui::Ui, atoms: impl egui::IntoAtoms<'a>, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<Option<R>> {
    let mut out = None;
    let mut add = Some(add);
    let r = menu_button_dyn(ui, atoms.into_atoms(), &mut |ui| {
        if let Some(f) = add.take() {
            out = Some(f(ui));
        }
    });
    egui::InnerResponse::new(r.inner.and(out), r.response)
}

fn menu_button_dyn(ui: &mut egui::Ui, atoms: egui::Atoms<'_>, add: &mut dyn FnMut(&mut egui::Ui)) -> egui::InnerResponse<Option<()>> {
    ui.menu_button(atoms, |ui| add(ui))
}
