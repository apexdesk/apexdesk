//! The Font dialog: typeface, style, size, color, underline, effects and
//! (advanced) spacing, scale and position.

use std::sync::Arc;

use apex_doc::{Emphasis, UnderlineStyle};
use eframe::egui::{self, Id, RichText};

use super::DocView;
use crate::common;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme;
use crate::widgets::{self, DialogResult};

/// What the dialog shows and edits.
#[derive(Clone, PartialEq)]
struct FontVals {
    font: Arc<str>,
    /// Font of Chinese, Japanese and Korean characters.
    asian_font: Arc<str>,
    bold: bool,
    italic: bool,
    /// Points.
    size: f32,
    color: Option<u32>,
    /// `None` is no underline.
    underline: Option<UnderlineStyle>,
    underline_color: Option<u32>,
    /// East Asian emphasis mark.
    emphasis: Option<Emphasis>,
    strike: bool,
    double_strike: bool,
    superscript: bool,
    subscript: bool,
    small_caps: bool,
    all_caps: bool,
    hidden: bool,
    /// Percent.
    scale: u16,
    /// Points (negative condenses).
    spacing: f32,
    /// Points (negative lowers).
    position: f32,
}

pub struct FontDialog {
    orig: FontVals,
    cur: FontVals,
    tab: usize,
}

fn underline_label(u: Option<UnderlineStyle>) -> &'static str {
    match u {
        None => tr("font.ul_none"),
        Some(UnderlineStyle::Single) => tr("font.ul_single"),
        Some(UnderlineStyle::Words) => tr("font.ul_words"),
        Some(UnderlineStyle::Double) => tr("font.ul_double"),
        Some(UnderlineStyle::Thick) => tr("font.ul_thick"),
        Some(UnderlineStyle::Dotted) => tr("font.ul_dotted"),
        Some(UnderlineStyle::Dashed) => tr("font.ul_dashed"),
        Some(UnderlineStyle::DotDash) => tr("font.ul_dot_dash"),
        Some(UnderlineStyle::Wave) => tr("font.ul_wave"),
    }
}

impl DocView {
    pub(super) fn open_font_dialog(&mut self) {
        let a = self.attrs_at_caret();
        let st = self.cur_para().style;
        let cf = self.doc.char_format(&st, &a);
        let fx = cf.fx.clone();
        let font = cf.font.clone().or(self.doc.default_font_name()).unwrap_or_else(|| Arc::from(tr("font.default")));
        let asian_font = cf.asian.font.clone().filter(|f| !f.starts_with('+')).unwrap_or_else(|| Arc::from(tr("font.default")));
        let v = FontVals {
            font,
            asian_font,
            bold: cf.bold,
            italic: cf.italic,
            size: cf.size as f32 / 2.0,
            color: cf.color,
            underline: cf.underline.then_some(fx.underline),
            underline_color: fx.underline_color,
            emphasis: cf.asian.emphasis,
            strike: a.strike,
            double_strike: fx.double_strike,
            superscript: a.superscript,
            subscript: a.subscript,
            small_caps: fx.small_caps,
            all_caps: fx.all_caps,
            hidden: fx.hidden,
            scale: if fx.scale == 0 { 100 } else { fx.scale },
            spacing: fx.spacing as f32 / 20.0,
            position: fx.position as f32 / 2.0,
        };
        self.font_dialog = Some(FontDialog { orig: v.clone(), cur: v, tab: 0 });
    }

    pub(super) fn font_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.font_dialog else { return };
        let pages = [(Icon::GrowFont, "font.tab_font"), (Icon::Columns, "font.tab_advanced")];
        let mut tab = d.tab;
        let uid = self.uid;
        let res = widgets::nav_dialog(ctx, "doc-font", tr("font.title"), &pages, &mut tab, egui::vec2(520.0, 460.0), |ui, page| {
            let v = &mut d.cur;
            if page == 0 {
                font_page(ui, v, uid);
            } else {
                advanced_page(ui, v);
            }
            ui.add_space(10.0);
            preview(ui, v);
        });
        d.tab = tab;
        match res {
            DialogResult::None => {}
            DialogResult::Cancel => {
                self.font_dialog = None;
                self.focus = true;
            }
            DialogResult::Ok => {
                if let Some(d) = self.font_dialog.take() {
                    self.apply_font(d.orig, d.cur);
                }
                self.focus = true;
            }
        }
    }

    /// Applies the fields the user changed (the others keep each run's own
    /// value when the selection mixes them) as one step.
    fn apply_font(&mut self, o: FontVals, c: FontVals) {
        if o == c {
            return;
        }
        let default = tr("font.default");
        self.format(move |x| {
            if c.font != o.font {
                x.font = (!c.font.eq_ignore_ascii_case(default)).then(|| c.font.clone());
            }
            if c.asian_font != o.asian_font {
                let f = (!c.asian_font.eq_ignore_ascii_case(default)).then(|| c.asian_font.clone());
                x.set_asian(|a| a.font = f);
            }
            if c.emphasis != o.emphasis {
                x.set_asian(|a| a.emphasis = c.emphasis);
            }
            if c.bold != o.bold {
                x.bold = c.bold;
            }
            if c.italic != o.italic {
                x.italic = c.italic;
            }
            if c.size != o.size {
                x.size = Some((c.size * 2.0).round().clamp(2.0, 3276.0) as u16);
            }
            if c.color != o.color {
                x.color = c.color;
            }
            if c.underline.is_some() != o.underline.is_some() {
                x.underline = c.underline.is_some();
            }
            if c.strike != o.strike {
                x.strike = c.strike;
            }
            if (c.superscript, c.subscript) != (o.superscript, o.subscript) {
                (x.superscript, x.subscript) = (c.superscript, c.subscript);
            }
            x.set_fx(|f| {
                if c.underline != o.underline {
                    if let Some(u) = c.underline {
                        f.underline = u;
                    }
                }
                if c.underline_color != o.underline_color {
                    f.underline_color = c.underline_color;
                }
                if c.double_strike != o.double_strike {
                    f.double_strike = c.double_strike;
                }
                if c.small_caps != o.small_caps {
                    f.small_caps = c.small_caps;
                }
                if c.all_caps != o.all_caps {
                    f.all_caps = c.all_caps;
                }
                if c.hidden != o.hidden {
                    f.hidden = c.hidden;
                }
                if c.scale != o.scale {
                    f.scale = if c.scale == 100 { 0 } else { c.scale };
                }
                if c.spacing != o.spacing {
                    f.spacing = (c.spacing * 20.0).round() as i16;
                }
                if c.position != o.position {
                    f.position = (c.position * 2.0).round() as i16;
                }
            });
        });
    }
}

fn emphasis_label(e: Option<Emphasis>) -> &'static str {
    match e {
        None => tr("font.em_none"),
        Some(Emphasis::Dot) => tr("font.em_dot"),
        Some(Emphasis::Comma) => tr("font.em_comma"),
        Some(Emphasis::Circle) => tr("font.em_circle"),
        Some(Emphasis::Disc) => tr("font.em_disc"),
        Some(Emphasis::UnderDot) => tr("font.em_under_dot"),
    }
}

fn color_button(ui: &mut egui::Ui, id: &str, c: &mut Option<u32>, auto: &str) {
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, c.map_or(crate::theme::Paper::TEXT, theme::rgb));
            ui.painter().rect_stroke(r, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(150)), egui::StrokeKind::Inside);
            let label = match c {
                Some(v) => format!("#{:06X}", *v & 0xFF_FFFF),
                None => auto.to_string(),
            };
            crate::widgets::menu_button(ui, label, |ui| {
                if let Some(n) = widgets::color_menu(ui, auto, *c) {
                    *c = n;
                    ui.close();
                }
            });
        });
    });
}

fn font_page(ui: &mut egui::Ui, v: &mut FontVals, uid: u64) {
    egui::Grid::new("font_main").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tr("font.font"));
        if let Some(f) = common::themed_font_picker(ui, Id::new(("font_dlg_face", uid)), &v.font, 200.0) {
            v.font = Arc::from(f.as_str());
        }
        ui.end_row();
        ui.label(tr("font.asian_font"));
        if let Some(f) = common::font_picker(ui, Id::new(("font_dlg_asian", uid)), &v.asian_font, 200.0) {
            v.asian_font = Arc::from(f.as_str());
        }
        ui.end_row();
        ui.label(tr("font.style"));
        let styles = [tr("font.regular"), tr("font.italic"), tr("font.bold"), tr("font.bold_italic")];
        let cur = usize::from(v.italic) + 2 * usize::from(v.bold);
        widgets::combo_name(egui::ComboBox::from_id_salt("font_style").width(200.0).selected_text(styles[cur]).show_ui(ui, |ui| {
            for (i, s) in styles.iter().enumerate() {
                if ui.selectable_label(cur == i, *s).clicked() {
                    (v.italic, v.bold) = (i % 2 == 1, i >= 2);
                }
            }
        }), tr("font.style"));
        ui.end_row();
        ui.label(tr("font.size"));
        ui.add(egui::DragValue::new(&mut v.size).range(1.0..=1638.0).speed(0.5).suffix(" pt").max_decimals(1));
        ui.end_row();
        ui.label(tr("font.color"));
        color_button(ui, "font_color", &mut v.color, tr("color.automatic"));
        ui.end_row();
        ui.label(tr("font.underline"));
        widgets::combo_name(egui::ComboBox::from_id_salt("font_ul").width(200.0).selected_text(underline_label(v.underline)).show_ui(ui, |ui| {
            ui.selectable_value(&mut v.underline, None, underline_label(None));
            for u in UnderlineStyle::ALL {
                ui.selectable_value(&mut v.underline, Some(u), underline_label(Some(u)));
            }
        }), tr("font.underline"));
        ui.end_row();
        ui.label(tr("font.underline_color"));
        ui.add_enabled_ui(v.underline.is_some(), |ui| color_button(ui, "font_ul_color", &mut v.underline_color, tr("color.automatic")));
        ui.end_row();
        ui.label(tr("font.emphasis"));
        widgets::combo_name(egui::ComboBox::from_id_salt("font_em").width(200.0).selected_text(emphasis_label(v.emphasis)).show_ui(ui, |ui| {
            for e in [None, Some(Emphasis::Dot), Some(Emphasis::Comma), Some(Emphasis::Circle), Some(Emphasis::Disc), Some(Emphasis::UnderDot)] {
                ui.selectable_value(&mut v.emphasis, e, emphasis_label(e));
            }
        }), tr("font.emphasis"));
        ui.end_row();
    });
    ui.add_space(8.0);
    ui.strong(tr("font.effects"));
    egui::Grid::new("font_fx").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
        if ui.checkbox(&mut v.strike, tr("font.strike")).changed() && v.strike {
            v.double_strike = false;
        }
        if ui.checkbox(&mut v.small_caps, tr("font.small_caps")).changed() && v.small_caps {
            v.all_caps = false;
        }
        ui.end_row();
        if ui.checkbox(&mut v.double_strike, tr("font.double_strike")).changed() && v.double_strike {
            v.strike = false;
        }
        if ui.checkbox(&mut v.all_caps, tr("font.all_caps")).changed() && v.all_caps {
            v.small_caps = false;
        }
        ui.end_row();
        if ui.checkbox(&mut v.superscript, tr("format.superscript")).changed() && v.superscript {
            v.subscript = false;
        }
        ui.checkbox(&mut v.hidden, tr("font.hidden"));
        ui.end_row();
        if ui.checkbox(&mut v.subscript, tr("format.subscript")).changed() && v.subscript {
            v.superscript = false;
        }
        ui.end_row();
    });
}

fn advanced_page(ui: &mut egui::Ui, v: &mut FontVals) {
    egui::Grid::new("font_adv").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tr("font.scale"));
        widgets::combo_name(egui::ComboBox::from_id_salt("font_scale").width(120.0).selected_text(format!("{}%", v.scale)).show_ui(ui, |ui| {
            for s in [33u16, 50, 66, 80, 90, 100, 150, 200] {
                ui.selectable_value(&mut v.scale, s, format!("{s}%"));
            }
        }), tr("font.scale"));
        ui.add(egui::DragValue::new(&mut v.scale).range(1..=600).suffix("%"));
        ui.end_row();
        ui.label(tr("font.spacing"));
        ui.add(egui::DragValue::new(&mut v.spacing).range(-50.0..=150.0).speed(0.1).suffix(" pt").max_decimals(2));
        ui.label(RichText::new(tr("font.spacing_hint")).weak());
        ui.end_row();
        ui.label(tr("font.position"));
        ui.add(egui::DragValue::new(&mut v.position).range(-100.0..=100.0).speed(0.5).suffix(" pt").max_decimals(1));
        ui.label(RichText::new(tr("font.position_hint")).weak());
        ui.end_row();
    });
}

/// A sample of the text as it will look (with the effects egui can show).
fn preview(ui: &mut egui::Ui, v: &FontVals) {
    ui.strong(tr("font.preview"));
    let size = v.size.clamp(6.0, 40.0);
    let (font_id, synth_italic) = crate::fonts::font_for(Some(&v.font), v.bold, v.italic, size);
    let sample = tr("font.sample");
    let color = v.color.map_or(crate::theme::Paper::TEXT, theme::rgb);
    let stroke = egui::Stroke::new(1.0, v.underline_color.map_or(color, theme::rgb));
    let fmt = egui::TextFormat {
        font_id,
        color,
        italics: synth_italic,
        underline: if v.underline.is_some() { stroke } else { egui::Stroke::NONE },
        strikethrough: if v.strike || v.double_strike { egui::Stroke::new(1.0, color) } else { egui::Stroke::NONE },
        extra_letter_spacing: v.spacing + (v.scale as f32 / 100.0 - 1.0) * size * 0.5,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    let parts = if v.small_caps && !v.all_caps { super::char_fx::small_caps_parts(sample) } else { vec![(sample, false)] };
    for (part, lower) in parts {
        let mut f = fmt.clone();
        if lower {
            f.font_id.size *= 0.8;
            f.valign = egui::Align::BOTTOM;
        }
        let text = if v.all_caps || v.small_caps { super::char_fx::caps(part) } else { part.to_string() };
        job.append(&text, 0.0, f);
    }
    egui::Frame::new().fill(crate::theme::Paper::BG).stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(180))).inner_margin(10.0).show(ui, |ui| {
        ui.set_min_size(egui::vec2(ui.available_width(), 50.0));
        ui.centered_and_justified(|ui| ui.label(job));
    });
}
