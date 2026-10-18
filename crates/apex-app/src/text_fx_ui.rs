//! The artistic text controls shared by documents and presentations: the
//! gallery of text styles and the Text Effects menu (fill, outline,
//! shadow, glow, reflection and bevel of the selected characters).

use std::sync::Arc;

use apex_doc::text_fx::{self, TextGlow, TextOutline, TextReflection, TextShadow};
use apex_doc::{Dash, TextFill, TextFx};
use eframe::egui::{self, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::i18n::tr;

/// The theme's dark text color.
fn dark() -> u32 {
    crate::themes_ui::active().color(0)
}

/// Style `i` of the gallery in the current theme's colors.
pub fn style(i: usize) -> TextFx {
    text_fx::style(i, dark(), &crate::gradient_ui::accents())
}

/// Paints a sample letter with effects `fx` in `rect`.
pub fn sample(p: &egui::Painter, rect: Rect, fx: &TextFx, text: &str) {
    let job = egui::text::LayoutJob::simple_singleline(text.to_string(), egui::FontId::proportional(rect.height() * 0.62), Color32::from_gray(40));
    let g = p.layout_job(job);
    let n = text.chars().count();
    let runs = [crate::text_fx::Run { from: 0, to: n, fx: Arc::new(fx.clone()) }];
    let (g, layers) = crate::text_fx::apply(g, &runs, rect.height() / 40.0, Color32::WHITE);
    let reflect = if fx.reflection.is_some() { rect.height() * 0.12 } else { 0.0 };
    let at = pos2(rect.center().x - g.size().x / 2.0, rect.center().y - g.size().y / 2.0 - reflect);
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    crate::text_fx::paint(&p, at, &layers);
    p.galley(at, g, Color32::BLACK);
}

/// A button showing a style; clicked.
fn style_button(ui: &mut egui::Ui, fx: &TextFx, size: egui::Vec2, tip: &str, on: bool) -> bool {
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_filled(rect, 2.0, Color32::WHITE);
    sample(ui.painter(), rect.shrink(1.0), fx, "A");
    let stroke = if on || r.hovered() { Stroke::new(2.0, ui.visuals().selection.bg_fill) } else { Stroke::new(1.0, Color32::from_gray(200)) };
    ui.painter().rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
    r.on_hover_text(tip).clicked()
}

/// The gallery of text styles: the one picked.
pub fn gallery(ui: &mut egui::Ui, current: Option<&TextFx>) -> Option<TextFx> {
    let mut out = None;
    ui.label(egui::RichText::new(tr("art.styles")).small().weak());
    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
    for row in 0..3 {
        ui.horizontal(|ui| {
            for col in 0..5 {
                let i = row * 5 + col;
                let fx = style(i);
                let tip = format!("{} {}", tr("art.style"), i + 1);
                if style_button(ui, &fx, vec2(44.0, 44.0), &tip, current == Some(&fx)) {
                    out = Some(fx);
                }
            }
        });
    }
    if out.is_some() {
        ui.close();
    }
    out
}

const WEIGHTS: [f32; 8] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.25, 3.0, 4.5];
const DASHES: [(Dash, &str); 4] = [(Dash::Solid, "art.dash.solid"), (Dash::Dot, "art.dash.dot"), (Dash::Dash, "art.dash.dash"), (Dash::DashDot, "art.dash.dash_dot")];

/// The shadows offered: key, and the shadow of the text color `c`.
fn shadows() -> [(&'static str, TextShadow); 5] {
    [
        ("art.shadow.br", TextShadow::new(0, 100, 3.0, 2.0, 45)),
        ("art.shadow.b", TextShadow::new(0, 100, 3.0, 2.5, 90)),
        ("art.shadow.bl", TextShadow::new(0, 100, 3.0, 2.0, 135)),
        ("art.shadow.soft", TextShadow::new(0, 90, 8.0, 3.0, 45)),
        ("art.shadow.hard", TextShadow::new(0x7F7F7F, 255, 0.0, 3.0, 45)),
    ]
}

const GLOWS: [f32; 4] = [5.0, 8.0, 11.0, 18.0];

fn reflections() -> [(&'static str, TextReflection); 3] {
    [("art.refl.tight", TextReflection::new(128, 0.0, 35)), ("art.refl.half", TextReflection::new(128, 1.0, 55)), ("art.refl.full", TextReflection::new(128, 2.0, 90))]
}

/// Buttons in a row showing `fx` changed by each of `items`.
fn effect_row<T: Copy>(ui: &mut egui::Ui, base: &TextFx, items: &[(String, T)], set: &dyn Fn(&mut TextFx, T), cur: &dyn Fn(&TextFx) -> bool) -> Option<TextFx> {
    let mut out = None;
    ui.horizontal(|ui| {
        for (tip, v) in items {
            let mut fx = base.clone();
            set(&mut fx, *v);
            if style_button(ui, &fx, vec2(40.0, 40.0), tip, cur(&fx) && fx == *base) {
                out = Some(fx);
            }
        }
    });
    out
}

/// The Text Effects menu for characters with effects `cur` (None: none),
/// with the styles gallery as a submenu if `styles`: the effects to set,
/// `Some(None)` to clear them.
pub fn menu(ui: &mut egui::Ui, cur: Option<&TextFx>, styles: bool) -> Option<Option<TextFx>> {
    let base = cur.cloned().unwrap_or_default();
    let mut out: Option<Option<TextFx>> = None;
    ui.set_min_width(220.0);
    if styles {
        crate::widgets::menu_button(ui, tr("art.styles"), |ui| {
            if let Some(fx) = gallery(ui, cur) {
                out = Some(Some(fx));
            }
        });
        ui.separator();
    }
    crate::widgets::menu_button(ui, tr("art.fill"), |ui| {
        let solid = match base.fill {
            TextFill::Solid(c, _) => Some(c),
            _ => None,
        };
        if let Some(c) = crate::widgets::color_menu(ui, &tr("art.auto_fill"), solid) {
            out = Some(Some(TextFx { fill: c.map_or(TextFill::Auto, |c| TextFill::Solid(c, 255)), ..base.clone() }));
        }
        if ui.button(tr("art.no_fill")).clicked() {
            out = Some(Some(TextFx { fill: TextFill::None, ..base.clone() }));
            ui.close();
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("grad.gradient"), |ui| {
            let g = match &base.fill {
                TextFill::Gradient(g) => Some(g),
                _ => None,
            };
            if let Some(g) = crate::gradient_ui::gallery(ui, solid.unwrap_or(dark()), g) {
                out = Some(Some(TextFx { fill: g.map_or(TextFill::Auto, TextFill::Gradient), ..base.clone() }));
                ui.close();
            }
        });
    });
    crate::widgets::menu_button(ui, tr("art.outline"), |ui| {
        let o = base.outline;
        if let Some(c) = crate::widgets::color_menu(ui, &tr("art.no_outline"), o.map(|o| o.color)) {
            let outline = c.map(|c| TextOutline { color: c, ..o.unwrap_or_else(|| TextOutline::new(c, 0.75)) });
            out = Some(Some(TextFx { outline, ..base.clone() }));
        }
        ui.separator();
        let color = o.map_or(dark(), |o| o.color);
        crate::widgets::menu_button(ui, tr("art.weight"), |ui| {
            for w in WEIGHTS {
                let on = o.is_some_and(|o| o.width == text_fx::hundredths(w));
                if ui.selectable_label(on, format!("{w} pt")).clicked() {
                    let outline = TextOutline { width: text_fx::hundredths(w), ..o.unwrap_or_else(|| TextOutline::new(color, w)) };
                    out = Some(Some(TextFx { outline: Some(outline), ..base.clone() }));
                    ui.close();
                }
            }
        });
        crate::widgets::menu_button(ui, tr("art.dashes"), |ui| {
            for (d, key) in DASHES {
                if ui.selectable_label(o.is_some_and(|o| o.dash == d), tr(key)).clicked() {
                    let outline = TextOutline { dash: d, ..o.unwrap_or_else(|| TextOutline::new(color, 0.75)) };
                    out = Some(Some(TextFx { outline: Some(outline), ..base.clone() }));
                    ui.close();
                }
            }
        });
    });
    ui.separator();
    crate::widgets::menu_button(ui, tr("art.shadow"), |ui| {
        if ui.button(tr("art.no_shadow")).clicked() {
            out = Some(Some(TextFx { shadow: None, ..base.clone() }));
        }
        let items: Vec<(String, TextShadow)> = shadows().iter().map(|(k, s)| (tr(k).to_string(), *s)).collect();
        if let Some(fx) = effect_row(ui, &base, &items, &|f, s| f.shadow = Some(s), &|_| true) {
            out = Some(Some(fx));
        }
    });
    crate::widgets::menu_button(ui, tr("art.glow"), |ui| {
        if ui.button(tr("art.no_glow")).clicked() {
            out = Some(Some(TextFx { glow: None, ..base.clone() }));
        }
        for r in GLOWS {
            let items: Vec<(String, u32)> = crate::gradient_ui::accents().iter().enumerate().map(|(i, &c)| (format!("{} {} – {r} pt", tr("grad.accent"), i + 1), c)).collect();
            if let Some(fx) = effect_row(ui, &base, &items, &|f, c| f.glow = Some(TextGlow::new(c, 150, r)), &|_| true) {
                out = Some(Some(fx));
            }
        }
    });
    crate::widgets::menu_button(ui, tr("art.reflection"), |ui| {
        if ui.button(tr("art.no_reflection")).clicked() {
            out = Some(Some(TextFx { reflection: None, ..base.clone() }));
        }
        let items: Vec<(String, TextReflection)> = reflections().iter().map(|(k, r)| (tr(k).to_string(), *r)).collect();
        if let Some(fx) = effect_row(ui, &base, &items, &|f, r| f.reflection = Some(r), &|_| true) {
            out = Some(Some(fx));
        }
    });
    let mut bevel = base.bevel;
    if ui.checkbox(&mut bevel, tr("art.bevel")).changed() {
        out = Some(Some(TextFx { bevel, ..base.clone() }));
    }
    ui.separator();
    if crate::toolstrip::menu_item(ui, crate::icons::Icon::ClearFormat, tr("art.clear")).clicked() {
        out = Some(None);
    }
    if out.is_some() {
        ui.close();
    }
    // Effects that leave nothing set clear them.
    out.map(|o| o.filter(|fx| !fx.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_paint() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            for i in 0..text_fx::STYLES {
                let r = Rect::from_min_size(pos2(i as f32 * 50.0, 0.0), vec2(44.0, 44.0));
                sample(ui.painter(), r, &style(i), "A");
            }
        });
        out.textures_delta.clear();
        assert!(out.shapes.len() > text_fx::STYLES * 2);
    }
}
