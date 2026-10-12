//! Picture effects: the Picture tab's Adjust and Picture Styles groups and
//! the Effects page of the picture properties.

use std::sync::Arc;

use apex_doc::{Image, PicColor, PicFx};
use eframe::egui::{self, Color32};

use super::DocView;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::toolstrip;

/// Page of the Effects tab in the picture properties.
pub(super) const FX_PAGE: usize = 3;

fn color_key(c: PicColor) -> &'static str {
    match c {
        PicColor::Normal => "picfx.color_normal",
        PicColor::Grayscale => "picfx.color_gray",
        PicColor::BlackWhite => "picfx.color_bw",
        PicColor::Washout => "picfx.color_washout",
        PicColor::Sepia => "picfx.color_sepia",
    }
}

fn percent(v: i32) -> String {
    if v > 0 { format!("+{v} %") } else { format!("{v} %") }
}

/// A color swatch button that opens the color menu.
fn color_button(ui: &mut egui::Ui, color: &mut u32) {
    let chip = egui::RichText::new("■").color(crate::theme::rgb(*color));
    crate::widgets::menu_button(ui, chip, |ui| {
        if let Some(Some(c)) = crate::widgets::color_menu(ui, "", Some(*color)) {
            *color = c;
            ui.close();
        }
    });
}

/// The Effects page: corrections, color, transparency and effects.
pub(super) fn fx_page(ui: &mut egui::Ui, img: &mut Image) {
    let mut fx = img.fx.as_deref().cloned().unwrap_or_default();
    crate::widgets::form(ui, "pic-fx", |ui| {
        let mut b = fx.brightness as i32;
        ui.label(tr("picfx.brightness"));
        if ui.add(egui::Slider::new(&mut b, -100..=100).suffix(" %")).changed() {
            fx.brightness = b as i8;
        }
        ui.end_row();
        let mut c = fx.contrast as i32;
        ui.label(tr("picfx.contrast"));
        if ui.add(egui::Slider::new(&mut c, -100..=100).suffix(" %")).changed() {
            fx.contrast = c as i8;
        }
        ui.end_row();
        ui.label(tr("picfx.color"));
        egui::ComboBox::from_id_salt("pic-fx-color").selected_text(tr(color_key(fx.color))).show_ui(ui, |ui| {
            for c in PicColor::ALL {
                ui.selectable_value(&mut fx.color, c, tr(color_key(c)));
            }
        });
        ui.end_row();
        let mut t = fx.transparency as i32;
        ui.label(tr("picfx.transparency"));
        if ui.add(egui::Slider::new(&mut t, 0..=100).suffix(" %")).changed() {
            fx.transparency = t as u8;
        }
        ui.end_row();
    });
    crate::widgets::section(ui, tr("picfx.frame_effects"));
    crate::widgets::form(ui, "pic-fx2", |ui| {
        let mut on = fx.border.is_some();
        if ui.checkbox(&mut on, tr("picfx.border")).changed() {
            fx.border = on.then_some((0x000000, 1.0));
        }
        ui.horizontal(|ui| {
            if let Some((c, w)) = &mut fx.border {
                color_button(ui, c);
                ui.add(egui::DragValue::new(w).range(0.25..=30.0).speed(0.1).max_decimals(2).suffix(" pt"));
            }
        });
        ui.end_row();
        ui.checkbox(&mut fx.shadow, tr("picfx.shadow"));
        ui.label("");
        ui.end_row();
        let mut on = fx.glow.is_some();
        if ui.checkbox(&mut on, tr("picfx.glow")).changed() {
            fx.glow = on.then_some((0x4472C4, 8.0));
        }
        ui.horizontal(|ui| {
            if let Some((c, r)) = &mut fx.glow {
                color_button(ui, c);
                ui.add(egui::DragValue::new(r).range(1.0..=50.0).speed(0.2).max_decimals(1).suffix(" pt"));
            }
        });
        ui.end_row();
        ui.label(tr("picfx.soft_edges"));
        ui.add(egui::DragValue::new(&mut fx.soft_edges).range(0.0..=100.0).speed(0.2).max_decimals(1).suffix(" pt"));
        ui.end_row();
        ui.checkbox(&mut fx.reflection, tr("picfx.reflection"));
        ui.label("");
        ui.end_row();
    });
    ui.add_space(8.0);
    // Off with no effect to reset.
    if ui.add_enabled(!fx.is_default(), egui::Button::new(tr("picfx.reset"))).clicked() {
        fx = PicFx::default();
    }
    let new = (!fx.is_default()).then(|| Arc::new(fx));
    if img.fx != new {
        img.fx = new;
    }
}

impl DocView {
    /// The selected picture's effects.
    fn picture_fx(&self) -> Option<PicFx> {
        self.selected_picture().map(|(_, _, img)| img.fx.as_deref().cloned().unwrap_or_default())
    }

    /// Changes the selected picture's effects (one undo step).
    fn change_picture_fx(&mut self, f: impl FnOnce(&mut PicFx)) {
        let Some((at, _, img)) = self.selected_picture() else { return };
        let mut img = (*img).clone();
        let mut fx = img.fx.as_deref().cloned().unwrap_or_default();
        f(&mut fx);
        let new = (!fx.is_default()).then(|| Arc::new(fx));
        if img.fx != new {
            img.fx = new;
            self.doc.set_image(at, img);
        }
    }

    /// The Adjust group: corrections, color and transparency.
    pub(super) fn picture_adjust_group(&mut self, ui: &mut egui::Ui, a: Color32) {
        let cur = self.picture_fx().unwrap_or_default();
        toolstrip::large_menu(ui, Icon::Brightness, tr("picfx.corrections"), "", a, |ui| {
            ui.set_min_width(260.0);
            for (key, value, is_b) in [("picfx.brightness", cur.brightness, true), ("picfx.contrast", cur.contrast, false)] {
                ui.label(egui::RichText::new(tr(key)).small().weak());
                ui.horizontal(|ui| {
                    for v in [-40, -20, 0, 20, 40] {
                        if ui.selectable_label(value as i32 == v, percent(v)).clicked() {
                            self.change_picture_fx(|f| if is_b { f.brightness = v as i8 } else { f.contrast = v as i8 });
                        }
                    }
                });
            }
            ui.separator();
            if ui.button(format!("{}…", tr("picfx.options"))).clicked() {
                self.open_fx_page();
                ui.close();
            }
        });
        toolstrip::large_menu(ui, Icon::Recolor, tr("picfx.color"), "", a, |ui| {
            for c in PicColor::ALL {
                if ui.selectable_label(cur.color == c, tr(color_key(c))).clicked() {
                    self.change_picture_fx(|f| f.color = c);
                    ui.close();
                }
            }
        });
        toolstrip::large_menu(ui, Icon::Transparency, tr("picfx.transparency"), "", a, |ui| {
            for t in [0u8, 15, 30, 50, 65, 80, 95] {
                if ui.selectable_label(cur.transparency == t, format!("{t} %")).clicked() {
                    self.change_picture_fx(|f| f.transparency = t);
                    ui.close();
                }
            }
        });
    }

    /// The Picture Styles group: the styles gallery, border and effects.
    pub(super) fn picture_styles_group(&mut self, ui: &mut egui::Ui, a: Color32) {
        let cur = self.picture_fx().unwrap_or_default();
        toolstrip::large_menu(ui, Icon::PicStyles, tr("picfx.styles"), "", a, |ui| {
            for (key, st) in PicFx::styles() {
                let on = cur.border == st.border && cur.shadow == st.shadow && cur.glow == st.glow && cur.soft_edges == st.soft_edges && cur.reflection == st.reflection;
                if ui.selectable_label(on, tr(key)).clicked() {
                    // Styles keep the corrections and color.
                    self.change_picture_fx(|f| {
                        (f.border, f.shadow, f.glow, f.soft_edges, f.reflection) = (st.border, st.shadow, st.glow, st.soft_edges, st.reflection);
                    });
                    ui.close();
                }
            }
        });
        toolstrip::column(ui, |ui| {
            toolstrip::small_menu(ui, Some(Icon::Borders), tr("picfx.border"), "", a, |ui| {
                if let Some(c) = crate::widgets::color_menu(ui, tr("picfx.no_border"), cur.border.map(|b| b.0)) {
                    self.change_picture_fx(|f| f.border = c.map(|c| (c, f.border.map_or(1.0, |b| b.1))));
                    ui.close();
                }
                ui.separator();
                ui.label(egui::RichText::new(tr("picfx.weight")).small().weak());
                for w in [0.5f32, 1.0, 2.25, 3.0, 4.5, 6.0] {
                    if ui.selectable_label(cur.border.is_some_and(|b| b.1 == w), format!("{w} pt")).clicked() {
                        self.change_picture_fx(|f| f.border = Some((f.border.map_or(0, |b| b.0), w)));
                        ui.close();
                    }
                }
            });
            toolstrip::small_menu(ui, Some(Icon::PicStyles), tr("picfx.effects"), "", a, |ui| {
                if ui.selectable_label(cur.shadow, tr("picfx.shadow")).clicked() {
                    self.change_picture_fx(|f| f.shadow = !f.shadow);
                    ui.close();
                }
                if ui.selectable_label(cur.reflection, tr("picfx.reflection")).clicked() {
                    self.change_picture_fx(|f| f.reflection = !f.reflection);
                    ui.close();
                }
                crate::widgets::menu_button(ui, tr("picfx.glow"), |ui| {
                    if let Some(c) = crate::widgets::color_menu(ui, tr("picfx.no_glow"), cur.glow.map(|g| g.0)) {
                        self.change_picture_fx(|f| f.glow = c.map(|c| (c, f.glow.map_or(8.0, |g| g.1))));
                        ui.close();
                    }
                });
                crate::widgets::menu_button(ui, tr("picfx.soft_edges"), |ui| {
                    for r in [0.0f32, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0] {
                        let label = if r == 0.0 { tr("picfx.none").to_string() } else { format!("{r} pt") };
                        if ui.selectable_label(cur.soft_edges == r, label).clicked() {
                            self.change_picture_fx(|f| f.soft_edges = r);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if ui.button(format!("{}…", tr("picfx.options"))).clicked() {
                    self.open_fx_page();
                    ui.close();
                }
            });
        });
    }

    fn open_fx_page(&mut self) {
        if let Some((at, _, _)) = self.selected_picture() {
            self.open_picture_props(at, FX_PAGE);
        }
    }
}
