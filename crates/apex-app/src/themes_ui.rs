//! Color and font themes shared by spreadsheets and documents: the theme
//! of the active file (color pickers and font lists use it) and the
//! Themes, Colors and Fonts galleries.

use std::sync::Mutex;

use apex_doc::theme::{self, ColorScheme, FontScheme, Theme};
use eframe::egui::{self, Color32, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::fonts;
use crate::i18n::tr;

static ACTIVE: Mutex<Option<Theme>> = Mutex::new(None);

/// Makes `t` the theme of the pickers (each view calls it every frame).
pub fn set_active(t: &Theme) {
    let mut g = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref() != Some(t) {
        *g = Some(t.clone());
    }
}

/// The theme of the active file.
pub fn active() -> Theme {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Selected item frame and hover highlight.
fn item_frame(ui: &egui::Ui, rect: Rect, resp: &egui::Response, selected: bool) {
    let v = ui.visuals();
    if selected {
        ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill.gamma_multiply(0.35));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
    }
}

/// A theme card: "Aa" in its fonts over its colors, and its name.
fn theme_card(ui: &mut egui::Ui, t: &Theme, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(96.0, 92.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &t.name, Some(selected));
    item_frame(ui, rect, &resp, selected);
    let card = Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(80.0, 58.0));
    let p = ui.painter();
    p.rect_filled(card, 2.0, rgb(t.color(theme::slot::LT1)));
    p.rect_stroke(card, 2.0, Stroke::new(1.0, Color32::from_gray(170)), egui::StrokeKind::Inside);
    p.text(card.min + vec2(8.0, 6.0), egui::Align2::LEFT_TOP, "Aa", fonts::preview_font(&t.fonts.major, 20.0), rgb(t.color(theme::slot::DK1)));
    let w = (card.width() - 12.0) / 6.0;
    for i in 0..6 {
        let x = card.min.x + 6.0 + i as f32 * w;
        p.rect_filled(Rect::from_min_size(pos2(x, card.max.y - 18.0), vec2(w - 2.0, 12.0)), 0.0, rgb(t.color(4 + i as u8)));
    }
    p.text(pos2(rect.center().x, card.max.y + 6.0), egui::Align2::CENTER_TOP, &*t.name, egui::FontId::proportional(12.0), ui.visuals().text_color());
    resp.on_hover_text(format!("{}: {} / {}", t.name, t.fonts.major, t.fonts.minor))
}

/// A color scheme row: its eight main colors and its name.
fn colors_row(ui: &mut egui::Ui, c: &ColorScheme, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(260.0, 22.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &c.name, Some(selected));
    item_frame(ui, rect, &resp, selected);
    let p = ui.painter();
    for (i, slot) in [theme::slot::DK1, theme::slot::LT1, theme::slot::DK2, theme::slot::LT2, 4, 5, 6, 7, 8, 9].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.min.x + 6.0 + i as f32 * 13.0, rect.min.y + 4.0), vec2(12.0, 14.0));
        p.rect_filled(r, 0.0, rgb(c.colors[slot as usize]));
        p.rect_stroke(r, 0.0, Stroke::new(0.5, Color32::from_gray(150)), egui::StrokeKind::Inside);
    }
    p.text(pos2(rect.min.x + 144.0, rect.center().y), egui::Align2::LEFT_CENTER, &*c.name, egui::FontId::proportional(13.0), ui.visuals().text_color());
    resp
}

/// A font scheme row: "Aa", the scheme name and its two fonts, each drawn
/// in itself.
fn fonts_row(ui: &mut egui::Ui, f: &FontScheme, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(260.0, 52.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &f.name, Some(selected));
    item_frame(ui, rect, &resp, selected);
    let p = ui.painter();
    let text = ui.visuals().text_color();
    let bx = Rect::from_min_size(rect.min + vec2(6.0, 8.0), vec2(36.0, 36.0));
    p.rect_stroke(bx, 2.0, Stroke::new(1.0, Color32::from_gray(160)), egui::StrokeKind::Inside);
    p.text(bx.center(), egui::Align2::CENTER_CENTER, "Aa", fonts::preview_font(&f.major, 18.0), text);
    let x = rect.min.x + 50.0;
    p.text(pos2(x, rect.min.y + 3.0), egui::Align2::LEFT_TOP, &*f.name, egui::FontId::proportional(11.0), ui.visuals().weak_text_color());
    p.text(pos2(x, rect.min.y + 17.0), egui::Align2::LEFT_TOP, &*f.major, fonts::preview_font(&f.major, 15.0), text);
    p.text(pos2(x, rect.min.y + 35.0), egui::Align2::LEFT_TOP, &*f.minor, fonts::preview_font(&f.minor, 12.0), text);
    resp
}

/// The Themes, Colors and Fonts submenus (Page Layout in spreadsheets,
/// Design in documents). Returns the theme picked.
pub fn theme_menus(ui: &mut egui::Ui, cur: &Theme) -> Option<Theme> {
    let mut out = None;
    crate::widgets::menu_button(ui, tr("theme.themes"), |ui| {
        ui.label(RichText::new(tr("theme.builtin")).small().weak());
        let themes = theme::themes();
        egui::Grid::new("theme_cards").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            for (i, t) in themes.iter().enumerate() {
                if theme_card(ui, t, t == cur).clicked() {
                    out = Some(t.clone());
                    ui.close();
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
        ui.separator();
        if ui.button(tr("theme.reset")).clicked() {
            out = Some(Theme::default());
            ui.close();
        }
    });
    crate::widgets::menu_button(ui, tr("theme.colors"), |ui| {
        ui.label(RichText::new(tr("theme.builtin")).small().weak());
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for c in theme::color_schemes() {
                if colors_row(ui, &c, c.colors == cur.colors.colors).clicked() {
                    out = Some(Theme { colors: c, ..cur.clone() });
                    ui.close();
                }
            }
        });
    });
    crate::widgets::menu_button(ui, tr("theme.fonts"), |ui| {
        ui.label(RichText::new(tr("theme.builtin")).small().weak());
        egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
            for f in theme::font_schemes() {
                if fonts_row(ui, &f, f.major == cur.fonts.major && f.minor == cur.fonts.minor).clicked() {
                    out = Some(Theme { fonts: f, ..cur.clone() });
                    ui.close();
                }
            }
        });
    });
    out
}

/// The theme font entries at the top of a font list: returns the token
/// picked. `filter` (lowercase) hides the entries that don't match.
pub fn theme_font_rows(ui: &mut egui::Ui, current: &str, filter: &str) -> Option<&'static str> {
    let t = active();
    let mut out = None;
    let rows: Vec<_> = [(theme::MAJOR_TOKEN, &t.fonts.major, "theme.headings"), (theme::MINOR_TOKEN, &t.fonts.minor, "theme.body")]
        .into_iter()
        .filter(|(_, name, key)| filter.is_empty() || name.to_lowercase().contains(filter) || tr(key).to_lowercase().contains(filter))
        .collect();
    if rows.is_empty() {
        return None;
    }
    ui.label(RichText::new(tr("theme.theme_fonts")).small().weak());
    for (token, name, key) in rows {
        let text = RichText::new(format!("{name} ({})", tr(key))).font(fonts::preview_font(name, 15.0));
        let on = current.eq_ignore_ascii_case(name) || current == token;
        if ui.add(egui::Button::selectable(on, text).min_size(vec2(ui.available_width(), 0.0))).clicked() {
            out = Some(token);
        }
    }
    ui.separator();
    out
}
