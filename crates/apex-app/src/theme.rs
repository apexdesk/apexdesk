//! Colors and visual style shared by the views.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};

pub const ACCENT: Color32 = Color32::from_rgb(0x21, 0x73, 0xE6);
pub const SHEET_ACCENT: Color32 = Color32::from_rgb(0x1E, 0x8E, 0x4E);
pub const DOC_ACCENT: Color32 = Color32::from_rgb(0x2B, 0x5C, 0xC7);
pub const SLIDES_ACCENT: Color32 = Color32::from_rgb(0xC4, 0x3E, 0x1C);
pub const PDF_ACCENT: Color32 = Color32::from_rgb(0xB3, 0x1B, 0x1B);
pub const DB_ACCENT: Color32 = Color32::from_rgb(0xA4, 0x37, 0x3A);
pub const NOTE_ACCENT: Color32 = Color32::from_rgb(0x7A, 0x4F, 0xB8);

/// Colors of the "paper" surfaces (grid, pages), which stay light like real
/// paper; only the chrome follows the dark/light theme.
pub struct Paper;

impl Paper {
    pub const BG: Color32 = Color32::WHITE;
    pub const TEXT: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
    pub const GRID: Color32 = Color32::from_rgb(0xE1, 0xE3, 0xE6);
    pub const SELECTION: Color32 = Color32::from_rgba_premultiplied(0x10, 0x3A, 0x70, 0x30);
    pub const TEXT_SELECTION: Color32 = Color32::from_rgb(0xB4, 0xD5, 0xFE);
}

pub fn header_bg(v: &Visuals) -> Color32 {
    if v.dark_mode { Color32::from_gray(38) } else { Color32::from_gray(243) }
}

pub fn header_active_bg(v: &Visuals) -> Color32 {
    if v.dark_mode { Color32::from_rgb(40, 62, 52) } else { Color32::from_rgb(0xD3, 0xEB, 0xDD) }
}

pub fn workspace_bg(v: &Visuals) -> Color32 {
    if v.dark_mode { Color32::from_gray(30) } else { Color32::from_rgb(0xE8, 0xEA, 0xED) }
}

/// Configures both themes; `ctx.set_theme` switches between them. With
/// `contrast` the interface text is a little brighter on dark and darker
/// on light; with `sharp` glyphs sit on whole pixels instead of quarter
/// pixels, sharper on low-dpi screens at the cost of a little evenness in
/// the spacing (egui rebuilds its fonts when this changes).
pub fn setup(ctx: &egui::Context, contrast: bool, sharp: bool) {
    for dark in [false, true] {
        let mut v = if dark { Visuals::dark() } else { Visuals::light() };
        let r = CornerRadius::same(4);
        v.widgets.noninteractive.corner_radius = r;
        v.widgets.inactive.corner_radius = r;
        v.widgets.hovered.corner_radius = r;
        v.widgets.active.corner_radius = r;
        v.widgets.open.corner_radius = r;
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(6);
        if !dark {
            v.panel_fill = Color32::from_rgb(0xF7, 0xF8, 0xFA);
        }
        v.text_options.subpixel_binning = !sharp;
        if contrast {
            // egui's labels and buttons: 140 and 180 on dark, 80 and 60 on light.
            let (text, button) = if dark { (175, 210) } else { (45, 30) };
            v.widgets.noninteractive.fg_stroke.color = Color32::from_gray(text);
            v.widgets.inactive.fg_stroke.color = Color32::from_gray(button);
        }
        ctx.set_visuals_of(if dark { egui::Theme::Dark } else { egui::Theme::Light }, v);
    }
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(6.0, 3.0);
        s.animation_time = 0.08;
        // Tooltips once the pointer has rested on something a while.
        s.interaction.tooltip_delay = TOOLTIP_DELAY;
        s.interaction.tooltip_grace_time = 0.6;
    });
}

/// Seconds before a tooltip shows.
pub const TOOLTIP_DELAY: f32 = 1.0;

pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

// --- dark workspace ------------------------------------------------------------------

thread_local! {
    /// Pages, cells and slides are drawn dark too (View > Dark Workspace,
    /// with the dark theme). Set by the app every frame.
    /// (the option, whether it applies now: the theme is dark)
    static DARK_CANVAS: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((false, false)) };
}

/// The Dark Workspace option and whether the interface is dark now.
pub fn set_dark_canvas(option: bool, dark_theme: bool) {
    DARK_CANVAS.with(|d| d.set((option, dark_theme)));
}

/// The Dark Workspace option is on (it shows only with the dark theme).
pub fn dark_canvas_option() -> bool {
    DARK_CANVAS.with(|d| d.get().0)
}

/// Whether the document's surfaces are drawn dark on screen now.
pub fn dark_canvas() -> bool {
    DARK_CANVAS.with(|d| d.get() == (true, true))
}

/// A document color for the dark workspace: its lightness turned around
/// (white paper goes dark gray, black text light gray) and its hue and
/// saturation kept, so a red stays red.
pub fn dark_color(c: Color32) -> Color32 {
    if c == Color32::PLACEHOLDER || c.a() == 0 {
        return c;
    }
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6) };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    // The ends stay short of black and white, as dark themes do.
    let l2 = 0.12 + 0.76 * (1.0 - l);
    let c2 = (1.0 - (2.0 * l2 - 1.0).abs()) * s.min(1.0);
    let x = c2 * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match h as u32 {
        0 => (c2, x, 0.0),
        1 => (x, c2, 0.0),
        2 => (0.0, c2, x),
        3 => (0.0, x, c2),
        4 => (x, 0.0, c2),
        _ => (c2, 0.0, x),
    };
    let m = l2 - c2 / 2.0;
    let q = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(q(r1), q(g1), q(b1), a)
}

fn dark_stroke(s: &mut Stroke) {
    s.color = dark_color(s.color);
}

fn dark_path_stroke(s: &mut egui::epaint::PathStroke) {
    if let egui::epaint::ColorMode::Solid(c) = &mut s.color {
        *c = dark_color(*c);
    }
}

/// Recolors a shape for the dark workspace; pictures (textured shapes)
/// keep their colors.
pub fn darken_shape(s: &mut egui::Shape) {
    use egui::Shape;
    match s {
        Shape::Vec(v) => v.iter_mut().for_each(darken_shape),
        Shape::Circle(c) => {
            c.fill = dark_color(c.fill);
            dark_stroke(&mut c.stroke);
        }
        Shape::Ellipse(e) => {
            e.fill = dark_color(e.fill);
            dark_stroke(&mut e.stroke);
        }
        Shape::LineSegment { stroke, .. } => dark_stroke(stroke),
        Shape::Path(p) => {
            p.fill = dark_color(p.fill);
            dark_path_stroke(&mut p.stroke);
        }
        Shape::Rect(r) => {
            if r.brush.is_none() {
                r.fill = dark_color(r.fill);
            }
            dark_stroke(&mut r.stroke);
        }
        Shape::Text(t) => {
            t.fallback_color = dark_color(t.fallback_color);
            t.override_text_color = t.override_text_color.map(dark_color);
            dark_stroke(&mut t.underline);
            let g = std::sync::Arc::make_mut(&mut t.galley);
            for row in &mut g.rows {
                let row = std::sync::Arc::make_mut(&mut row.row);
                for v in &mut row.visuals.mesh.vertices {
                    v.color = dark_color(v.color);
                }
            }
        }
        Shape::Mesh(m) => {
            if m.texture_id == egui::TextureId::default() {
                for v in &mut std::sync::Arc::make_mut(m).vertices {
                    v.color = dark_color(v.color);
                }
            }
        }
        Shape::QuadraticBezier(b) => {
            b.fill = dark_color(b.fill);
            dark_path_stroke(&mut b.stroke);
        }
        Shape::CubicBezier(b) => {
            b.fill = dark_color(b.fill);
            dark_path_stroke(&mut b.stroke);
        }
        Shape::Noop | Shape::Callback(_) => {}
    }
}

/// Where the next shape painted on `p`'s layer goes: pass it to
/// [`darken_since`] once the document's surface is painted.
pub fn canvas_mark(p: &egui::Painter) -> usize {
    p.ctx().graphics(|g| g.get(p.layer_id()).map_or(0, |l| l.next_idx().0))
}

/// Recolors what was painted on `p`'s layer since `mark` for the dark
/// workspace, when it is on (only the screen: printing and PDF export
/// don't paint through here).
pub fn darken_since(p: &egui::Painter, mark: usize) {
    if !dark_canvas() {
        return;
    }
    p.ctx().graphics_mut(|g| {
        let list = g.entry(p.layer_id());
        let end = list.next_idx().0;
        for i in mark..end {
            list.mutate_shape(egui::layers::ShapeIdx(i), |c| darken_shape(&mut c.shape));
        }
    });
}

/// Paints with `f` and recolors it for the dark workspace when it is on.
pub fn dark_paint<R>(p: &egui::Painter, f: impl FnOnce() -> R) -> R {
    let mark = canvas_mark(p);
    let out = f();
    darken_since(p, mark);
    out
}

#[cfg(test)]
mod dark_tests {
    use super::*;

    #[test]
    fn lightness_turns_hue_stays() {
        let w = dark_color(Color32::WHITE);
        let b = dark_color(Color32::BLACK);
        assert!(w.r() < 40 && w.r() == w.g() && w.g() == w.b(), "{w:?}");
        assert!(b.r() > 210, "{b:?}");
        // A light yellow fill turns dark yellow; a dark blue text light blue.
        let y = dark_color(Color32::from_rgb(0xFF, 0xF2, 0xCC));
        assert!(y.r() > y.b() && y.g() > y.b() && y.r() < 0x80, "{y:?}");
        let blue = dark_color(Color32::from_rgb(0x1F, 0x3A, 0x93));
        assert!(blue.b() > blue.r() && blue.b() > 0xA0, "{blue:?}");
        // Pure red keeps its lightness.
        let r = dark_color(Color32::from_rgb(255, 0, 0));
        assert!(r.r() > 200 && r.g() < 30, "{r:?}");
        assert_eq!(dark_color(Color32::TRANSPARENT), Color32::TRANSPARENT);
    }
}

