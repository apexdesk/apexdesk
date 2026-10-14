//! ApexDesk's own window frame on the desktop. The system's title bar goes
//! and the row of tabs takes its place: its empty part moves the window (a
//! double click maximizes or restores it), the buttons at its right
//! minimize, maximize and close, and the edges resize the window. The
//! corners are rounded while the window isn't maximized: by the system on
//! macOS and Windows 11, painted by ApexDesk elsewhere. Never on Android;
//! File > Options > "Use the system's title bar" turns it off.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, ResizeDirection, Sense, Stroke, ViewportCommand, vec2};

use crate::i18n::tr;

/// The window has ApexDesk's frame.
static ON: AtomicBool = AtomicBool::new(false);
/// ApexDesk paints the rounded corners (the window is transparent).
static ROUND: AtomicBool = AtomicBool::new(false);

/// The radius of the window's corners.
pub const RADIUS: u8 = 8;
/// How far in from the window's edge the pointer resizes it.
const EDGE: f32 = 5.0;
/// The size of each window button.
const BUTTON: egui::Vec2 = egui::Vec2::new(40.0, 28.0);

/// Whether the window has ApexDesk's frame.
pub fn on() -> bool {
    ON.load(Relaxed)
}

/// The window as it opens. `own`: without the system's title bar.
/// `round`: transparent, for the rounded corners ApexDesk paints where the
/// system doesn't round them (scripted runs go without: their virtual
/// screens have no transparency).
pub fn viewport(b: egui::ViewportBuilder, own: bool, round: bool) -> egui::ViewportBuilder {
    let own = own && cfg!(not(target_os = "android"));
    ON.store(own, Relaxed);
    if own { frameless(b, round) } else { b }
}

/// macOS keeps its window (rounded, with a shadow and resizable edges)
/// with the title bar and its buttons hidden.
#[cfg(target_os = "macos")]
fn frameless(b: egui::ViewportBuilder, _round: bool) -> egui::ViewportBuilder {
    b.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false).with_titlebar_buttons_shown(false)
}

/// Windows 11 rounds the window itself (see `native`).
#[cfg(windows)]
fn frameless(b: egui::ViewportBuilder, _round: bool) -> egui::ViewportBuilder {
    b.with_decorations(false)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn frameless(b: egui::ViewportBuilder, round: bool) -> egui::ViewportBuilder {
    ROUND.store(round, Relaxed);
    b.with_decorations(false).with_transparent(round)
}

/// Once the window exists: Windows 11 rounds its corners (Windows 10
/// leaves them square).
#[cfg(windows)]
pub fn native(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !on() || DONE.swap(true, Relaxed) {
        return;
    }
    if let Ok(h) = frame.window_handle()
        && let RawWindowHandle::Win32(h) = h.as_raw()
    {
        let pref = DWMWCP_ROUND;
        // SAFETY: the handle is this window's, and the value is the
        // attribute's own type and size.
        unsafe {
            let _ = DwmSetWindowAttribute(HWND(h.hwnd.get() as *mut _), DWMWA_WINDOW_CORNER_PREFERENCE, (&raw const pref).cast(), size_of_val(&pref) as u32);
        }
    }
}

#[cfg(not(windows))]
pub fn native(_frame: &eframe::Frame) {}

/// Maximized or full screen: no edges to resize and no rounded corners.
fn filling(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

/// The radius the panels at the window's corners round them with.
pub fn corners(ctx: &egui::Context) -> u8 {
    if ROUND.load(Relaxed) && !filling(ctx) { RADIUS } else { 0 }
}

/// What the window shows where nothing is painted: nothing, around the
/// rounded corners.
pub fn clear_color() -> Option<[f32; 4]> {
    ROUND.load(Relaxed).then_some([0.0; 4])
}

/// Makes the empty part of `rect` (what no widget added after this takes)
/// move the window; a double click maximizes or restores it.
pub fn title_area(ui: &egui::Ui, rect: Rect) {
    if !on() {
        return;
    }
    let r = ui.interact(rect, egui::Id::new("window_title"), Sense::click_and_drag());
    if r.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if r.double_clicked() {
        let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
}

/// The width the window buttons take.
pub fn buttons_width(ctx: &egui::Context) -> f32 {
    if on() && !full_screen(ctx) { 3.0 * BUTTON.x + 6.0 } else { 0.0 }
}

fn full_screen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

#[derive(Clone, Copy, PartialEq)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Close, maximize (or restore) and minimize, from the right, in a
/// right-to-left `ui`. Full screen has none (F11 leaves it).
pub fn buttons(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if !on() || full_screen(&ctx) {
        return;
    }
    ui.spacing_mut().item_spacing.x = 0.0;
    if button(ui, Glyph::Close, tr("window.close")).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let (glyph, tip) = if max { (Glyph::Restore, tr("window.restore")) } else { (Glyph::Maximize, tr("window.maximize")) };
    if button(ui, glyph, tip).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    if button(ui, Glyph::Minimize, tr("window.minimize")).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    ui.add_space(6.0);
}

fn button(ui: &mut egui::Ui, glyph: Glyph, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(BUTTON, Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, tip, None);
    let v = ui.visuals();
    let close = glyph == Glyph::Close;
    let down = resp.is_pointer_button_down_on();
    let bg = match (close, down, resp.hovered()) {
        (true, true, _) => Color32::from_rgb(0xC4, 0x2B, 0x1C),
        (true, false, true) => Color32::from_rgb(0xE8, 0x11, 0x23),
        (false, true, _) => v.widgets.active.weak_bg_fill,
        (false, false, true) => v.widgets.hovered.weak_bg_fill,
        _ => Color32::TRANSPARENT,
    };
    let p = ui.painter();
    p.rect_filled(rect, 4.0, bg);
    let ink = if close && (down || resp.hovered()) { Color32::WHITE } else { v.text_color() };
    let s = Stroke::new(1.0, ink);
    // On pixel centres: one-pixel lines stay sharp.
    let c = Pos2::new(p.round_to_pixel_center(rect.center().x), p.round_to_pixel_center(rect.center().y));
    match glyph {
        Glyph::Minimize => {
            p.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], s);
        }
        Glyph::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, vec2(10.0, 10.0)), 1.0, s, egui::StrokeKind::Middle);
        }
        Glyph::Restore => {
            let back = Rect::from_min_size(c + vec2(-3.0, -5.0), vec2(8.0, 8.0));
            let front = Rect::from_min_size(c + vec2(-5.0, -3.0), vec2(8.0, 8.0));
            p.line_segment([back.left_top(), back.right_top()], s);
            p.line_segment([back.right_top(), back.right_bottom()], s);
            p.rect_stroke(front, 1.0, s, egui::StrokeKind::Middle);
        }
        Glyph::Close => {
            p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], s);
            p.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], s);
        }
    }
    resp.on_hover_text(tip)
}

/// The window's edges resize it (macOS's own window does that).
fn edges(ctx: &egui::Context) -> bool {
    on() && cfg!(not(target_os = "macos")) && !filling(ctx)
}

/// The edge or corner of the window at `p`, if any.
fn edge_at(ctx: &egui::Context, p: Pos2) -> Option<ResizeDirection> {
    let r = ctx.viewport_rect();
    // The corners take a little more, as they're harder to hit.
    let near = |d: f32, m: f32| d < m;
    let (l, rt, t, b) = (p.x - r.left(), r.right() - p.x, p.y - r.top(), r.bottom() - p.y);
    let corner = EDGE * 2.5;
    Some(match () {
        _ if near(l, corner) && near(t, corner) && (near(l, EDGE) || near(t, EDGE)) => ResizeDirection::NorthWest,
        _ if near(rt, corner) && near(t, corner) && (near(rt, EDGE) || near(t, EDGE)) => ResizeDirection::NorthEast,
        _ if near(l, corner) && near(b, corner) && (near(l, EDGE) || near(b, EDGE)) => ResizeDirection::SouthWest,
        _ if near(rt, corner) && near(b, corner) && (near(rt, EDGE) || near(b, EDGE)) => ResizeDirection::SouthEast,
        _ if near(l, EDGE) => ResizeDirection::West,
        _ if near(rt, EDGE) => ResizeDirection::East,
        _ if near(t, EDGE) => ResizeDirection::North,
        _ if near(b, EDGE) => ResizeDirection::South,
        _ => return None,
    })
}

/// Before egui sees the input: a press on an edge resizes the window, and
/// nothing under it gets the press.
pub fn input_hook(ctx: &egui::Context, raw: &mut egui::RawInput) {
    if !edges(ctx) {
        return;
    }
    let mut start = None;
    raw.events.retain(|e| match e {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } => match edge_at(ctx, *pos) {
            Some(d) => {
                start = Some(d);
                false
            }
            None => true,
        },
        _ => true,
    });
    if let Some(d) = start {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
    }
}

/// After the frame's UI: the resize pointer on the edges, and the line
/// around the window.
pub fn finish(ctx: &egui::Context) {
    if !on() || cfg!(target_os = "macos") || filling(ctx) {
        return;
    }
    if let Some(d) = ctx.input(|i| i.pointer.hover_pos()).and_then(|p| edge_at(ctx, p)) {
        ctx.set_cursor_icon(match d {
            ResizeDirection::North => CursorIcon::ResizeNorth,
            ResizeDirection::South => CursorIcon::ResizeSouth,
            ResizeDirection::East => CursorIcon::ResizeEast,
            ResizeDirection::West => CursorIcon::ResizeWest,
            ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
            ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
            ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
            ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
        });
    }
    // Without the system's frame the window needs a line of its own to
    // stand out from what is behind it.
    let dark = ctx.global_style().visuals.dark_mode;
    let line = if dark { Color32::from_white_alpha(40) } else { Color32::from_black_alpha(60) };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window_line")));
    painter.rect_stroke(ctx.viewport_rect(), corners(ctx), Stroke::new(1.0, line), egui::StrokeKind::Inside);
}
