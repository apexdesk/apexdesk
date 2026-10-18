//! Editing fills as Office's Format Shape > Fill does, shared by shapes,
//! text boxes, slides and page backgrounds: no fill, a solid color or a
//! gradient (the preset gallery of the color's light and dark variations
//! and the theme's presets, its type and angle, and its stops: add, remove,
//! drag, color, place and transparency).

use apex_doc::gradient::{self, GradKind, GradStop, Gradient};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::i18n::tr;
use crate::theme;

/// The theme's six accent colors.
pub fn accents() -> [u32; 6] {
    let t = crate::themes_ui::active();
    [4, 5, 6, 7, 8, 9].map(|s| t.color(s))
}

/// A gradient's swatch: a checkerboard under it when it is see-through.
pub fn swatch(p: &egui::Painter, g: &Gradient, rect: Rect) {
    if !g.opaque() {
        let n = 4.0;
        let (cw, ch) = (rect.width() / n, rect.height() / n);
        for i in 0..n as usize {
            for j in 0..n as usize {
                let c = if (i + j) % 2 == 0 { Color32::from_gray(255) } else { Color32::from_gray(205) };
                p.rect_filled(Rect::from_min_size(pos2(rect.min.x + i as f32 * cw, rect.min.y + j as f32 * ch), vec2(cw, ch)), 0.0, c);
            }
        }
    }
    crate::gradient::paint_rect(p, g, rect);
}

/// A button showing a gradient; clicked.
fn grad_button(ui: &mut egui::Ui, g: &Gradient, size: f32, tip: &str, on: bool) -> bool {
    let (rect, r) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    swatch(ui.painter(), g, rect.shrink(1.0));
    let stroke = if on || r.hovered() { Stroke::new(2.0, ui.visuals().selection.bg_fill) } else { Stroke::new(1.0, Color32::from_gray(160)) };
    ui.painter().rect_stroke(rect, 1.0, stroke, StrokeKind::Inside);
    let r = r.on_hover_text(tip);
    r.clicked()
}

/// The preset gallery for color `base`: no gradient, its light and dark
/// variations and the theme's presets. Some(None): no gradient.
pub fn gallery(ui: &mut egui::Ui, base: u32, current: Option<&Gradient>) -> Option<Option<Gradient>> {
    let mut out = None;
    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
    if ui.button(tr("grad.none")).clicked() {
        out = Some(None);
    }
    for (dark, key) in [(false, "grad.light"), (true, "grad.dark")] {
        ui.label(RichText::new(tr(key)).small().weak());
        ui.horizontal(|ui| {
            for (i, v) in gradient::VARIATIONS.iter().enumerate() {
                let g = gradient::variation(base, dark, i);
                if grad_button(ui, &g, 30.0, &tr(v.0), current == Some(&g)) {
                    out = Some(Some(g));
                }
            }
        });
    }
    ui.label(RichText::new(tr("grad.presets")).small().weak());
    for k in 0..gradient::PRESETS {
        ui.horizontal(|ui| {
            for (i, c) in accents().into_iter().enumerate() {
                let g = gradient::preset(c, k);
                let tip = format!("{} – {} {}", tr(PRESET_KEYS[k]), tr("grad.accent"), i + 1);
                if grad_button(ui, &g, 30.0, &tip, current == Some(&g)) {
                    out = Some(Some(g));
                }
            }
        });
    }
    out
}

const PRESET_KEYS: [&str; gradient::PRESETS] = ["grad.p.light", "grad.p.medium", "grad.p.spot", "grad.p.band", "grad.p.rect"];

/// What the Shape Fill menu picked.
#[derive(Clone, Debug, PartialEq)]
pub enum FillPick {
    /// A color (None: no fill) and a gradient over it.
    Set(Option<u32>, Option<Gradient>),
    /// "More gradients…": the Format Shape dialog.
    More,
}

/// The Shape Fill menu: no fill, the colors, and a Gradient submenu with
/// the variations of the current color and "More gradients…".
pub fn fill_menu(ui: &mut egui::Ui, fill: Option<u32>, grad: Option<&Gradient>, none_key: &'static str) -> Option<FillPick> {
    let mut out = None;
    if let Some(c) = crate::widgets::color_menu(ui, &tr(none_key), fill.filter(|_| grad.is_none())) {
        out = Some(FillPick::Set(c, None));
    }
    ui.separator();
    crate::widgets::menu_button(ui, tr("grad.gradient"), |ui| {
        if let Some(g) = gallery(ui, fill.unwrap_or(apex_doc::SHAPE_FILL), grad) {
            out = Some(match g {
                Some(g) => FillPick::Set(Some(g.main_color()), Some(g)),
                None => FillPick::Set(Some(grad.map_or(fill.unwrap_or(apex_doc::SHAPE_FILL), |g| g.stops[0].color)), None),
            });
            ui.close();
        }
        ui.separator();
        if crate::toolstrip::menu_item(ui, crate::icons::Icon::Fill, &format!("{}…", tr("grad.more"))).clicked() {
            out = Some(FillPick::More);
            ui.close();
        }
    });
    if out.is_some() {
        ui.close();
    }
    out
}

/// Which stop is being edited, kept per editor.
fn selected(ui: &egui::Ui, id: egui::Id) -> usize {
    ui.data(|d| d.get_temp(id.with("stop"))).unwrap_or(0)
}

fn select(ui: &egui::Ui, id: egui::Id, i: usize) {
    ui.data_mut(|d| d.insert_temp(id.with("stop"), i));
}

/// The stops bar: the gradient with a marker under each stop. Click a
/// marker to pick it, drag it to move it, click the bar to add one.
fn stops_bar(ui: &mut egui::Ui, id: egui::Id, g: &mut Gradient) -> bool {
    let width = ui.available_width().clamp(160.0, 300.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), Sense::click_and_drag());
    let bar = Rect::from_min_size(rect.min + vec2(6.0, 0.0), vec2(rect.width() - 12.0, 18.0));
    let lin = Gradient { kind: GradKind::Linear, angle: 0.0, ..g.clone() };
    swatch(ui.painter(), &lin, bar);
    ui.painter().rect_stroke(bar, 0.0, Stroke::new(1.0, Color32::GRAY), StrokeKind::Outside);
    let x_of = |pos: f32| bar.min.x + pos * bar.width();
    let pos_of = |x: f32| ((x - bar.min.x) / bar.width()).clamp(0.0, 1.0);
    let mut sel = selected(ui, id).min(g.stops.len().saturating_sub(1));
    let mut changed = false;
    let marker = |s: &GradStop| Rect::from_center_size(pos2(x_of(s.pos), bar.max.y + 7.0), vec2(10.0, 12.0));
    let hit = |q: Pos2, g: &Gradient| g.stops.iter().enumerate().rev().find(|(_, s)| marker(s).expand(2.0).contains(q)).map(|(i, _)| i);
    if resp.drag_started() {
        // Where the button went down: the drag starts a few pixels later.
        if let Some(i) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).and_then(|q| hit(q, g)) {
            sel = i;
        }
    }
    if resp.dragged() {
        if let Some(q) = resp.interact_pointer_pos() {
            sel = g.move_stop(sel, pos_of(q.x));
            changed = true;
        }
    } else if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            match hit(q, g) {
                Some(i) => sel = i,
                None => {
                    if let Some(i) = g.add_stop(pos_of(q.x)) {
                        sel = i;
                        changed = true;
                    }
                }
            }
        }
    }
    let p = ui.painter();
    for (i, s) in g.stops.iter().enumerate() {
        let r = marker(s).expand(if i == sel { 1.0 } else { 0.0 });
        let tip = pos2(r.center().x, bar.max.y);
        p.add(egui::Shape::convex_polygon(vec![tip, pos2(r.max.x, r.min.y + 4.0), r.right_bottom(), r.left_bottom(), pos2(r.min.x, r.min.y + 4.0)], theme::rgb(s.color), Stroke::new(if i == sel { 2.0 } else { 1.0 }, if i == sel { ui.visuals().selection.bg_fill } else { Color32::DARK_GRAY })));
    }
    select(ui, id, sel);
    changed
}

/// Edits a gradient: type, angle and stops. Whether it changed.
pub fn editor(ui: &mut egui::Ui, id: egui::Id, g: &mut Gradient) -> bool {
    let before = g.clone();
    egui::Grid::new(id.with("form")).num_columns(2).min_col_width(crate::widgets::FORM_LABEL_W).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tr("grad.type"));
        egui::ComboBox::from_id_salt(id.with("kind")).selected_text(tr(g.kind.key())).show_ui(ui, |ui| {
            for k in GradKind::ALL {
                ui.selectable_value(&mut g.kind, k, tr(k.key()));
            }
        });
        ui.end_row();
        if g.kind == GradKind::Linear {
            ui.label(tr("grad.angle"));
            let mut a = g.angle;
            if ui.add(egui::DragValue::new(&mut a).range(0.0..=359.9).speed(1.0).max_decimals(1).suffix("°")).changed() {
                g.angle = a.rem_euclid(360.0);
            }
            ui.end_row();
        }
        ui.label(tr("grad.stops"));
        ui.vertical(|ui| {
            stops_bar(ui, id, g);
            ui.horizontal(|ui| {
                let sel = selected(ui, id).min(g.stops.len() - 1);
                if ui.small_button(tr("grad.add_stop")).on_hover_text(tr("grad.add_stop_tip")).clicked() {
                    let next = g.stops.get(sel + 1).map_or(1.0, |s| s.pos);
                    let at = if sel + 1 < g.stops.len() { (g.stops[sel].pos + next) / 2.0 } else { (g.stops[sel].pos + g.stops[sel.saturating_sub(1)].pos) / 2.0 };
                    if let Some(i) = g.add_stop(at) {
                        select(ui, id, i);
                    }
                }
                if ui.add_enabled(g.stops.len() > 2, egui::Button::new(tr("grad.remove_stop")).small()).clicked() && g.remove_stop(sel) {
                    select(ui, id, sel.saturating_sub(1));
                }
            });
        });
        ui.end_row();
        let sel = selected(ui, id).min(g.stops.len() - 1);
        let s = g.stops[sel];
        ui.label(tr("grad.stop_color"));
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, theme::rgb(s.color));
            crate::widgets::menu_button(ui, tr("shape.pick_color"), |ui| {
                if let Some(Some(c)) = crate::widgets::color_menu(ui, &tr("grad.stop_color"), Some(s.color)) {
                    g.stops[sel].color = c;
                    ui.close();
                }
            });
        });
        ui.end_row();
        ui.label(tr("grad.position"));
        let mut pos = s.pos * 100.0;
        if ui.add(egui::DragValue::new(&mut pos).range(0.0..=100.0).speed(0.5).max_decimals(0).suffix(" %")).changed() {
            let j = g.move_stop(sel, pos / 100.0);
            select(ui, id, j);
        }
        ui.end_row();
        ui.label(tr("grad.transparency"));
        let mut tr_ = (1.0 - s.alpha) * 100.0;
        if ui.add(egui::Slider::new(&mut tr_, 0.0..=100.0).max_decimals(0).suffix(" %")).changed() {
            let sel = selected(ui, id).min(g.stops.len() - 1);
            g.stops[sel].alpha = 1.0 - tr_ / 100.0;
        }
        ui.end_row();
    });
    *g != before
}

/// The kind of fill chosen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    None,
    Solid,
    Gradient,
}

/// Edits a fill: none, a solid color or a gradient (`fill` is the color
/// standing for the gradient). `none_key`: the "no fill" label. Whether it
/// changed.
pub fn fill_editor(ui: &mut egui::Ui, id: egui::Id, fill: &mut Option<u32>, grad: &mut Option<Gradient>, none_key: &'static str) -> bool {
    let (f0, g0) = (*fill, grad.clone());
    let mut kind = match (*fill, grad.is_some()) {
        (None, _) => Kind::None,
        (Some(_), false) => Kind::Solid,
        (Some(_), true) => Kind::Gradient,
    };
    ui.horizontal(|ui| {
        ui.radio_value(&mut kind, Kind::None, tr(none_key));
        ui.radio_value(&mut kind, Kind::Solid, tr("grad.solid"));
        ui.radio_value(&mut kind, Kind::Gradient, tr("grad.gradient"));
    });
    let base = fill.unwrap_or(apex_doc::SHAPE_FILL);
    match kind {
        Kind::None => {
            *fill = None;
            *grad = None;
        }
        Kind::Solid => {
            if grad.is_some() || fill.is_none() {
                *fill = Some(grad.as_ref().map_or(base, |g| g.stops[0].color));
                *grad = None;
            }
            ui.horizontal(|ui| {
                ui.label(tr("shape.fill"));
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, theme::rgb(fill.unwrap_or(base)));
                crate::widgets::menu_button(ui, tr("shape.pick_color"), |ui| {
                    if let Some(Some(c)) = crate::widgets::color_menu(ui, &tr(none_key), *fill) {
                        *fill = Some(c);
                        ui.close();
                    }
                });
            });
        }
        Kind::Gradient => {
            let g = grad.get_or_insert_with(|| gradient::variation(base, false, 1));
            ui.horizontal(|ui| {
                ui.label(tr("grad.presets"));
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 18.0), Sense::hover());
                swatch(ui.painter(), g, r);
                crate::widgets::menu_button(ui, tr("grad.pick"), |ui| {
                    if let Some(Some(n)) = gallery(ui, base, Some(g)) {
                        *g = n;
                        ui.close();
                    }
                });
            });
            editor(ui, id, g);
            *fill = Some(g.main_color());
        }
    }
    *fill != f0 || *grad != g0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_kinds_switch() {
        let ctx = egui::Context::default();
        let g = Gradient::two(0xFF0000, 0x0000FF, 90.0);
        let (mut fill, mut grad) = (Some(g.main_color()), Some(g));
        ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(!fill_editor(ui, egui::Id::new("f"), &mut fill, &mut grad, "shape.no_fill"));
        })
        .textures_delta
        .clear();
        assert!(grad.is_some());
    }
}
