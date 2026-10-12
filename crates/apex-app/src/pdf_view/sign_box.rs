//! Making a visible signature: the name typed, a signature drawn with the
//! mouse or a picture of one, with the date if wanted. It is then placed
//! on a page (a click, or a box dragged) and, if asked, signed with a
//! certificate through the application's signing dialog.

use apex_io::pdf::edit::annots::Picture;
use apex_io::pdf::edit::forms::SigLook;
use eframe::egui::{self, Color32, Sense, Stroke, vec2};

use super::annotate::today;
use super::{PdfView, Tool};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Type,
    Draw,
    Image,
}

pub struct SignBox {
    pub mode: Mode,
    pub name: String,
    pub date: bool,
    /// Drawn strokes (points in the drawing box).
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub picture: Option<Picture>,
    /// Sign with a certificate once placed.
    pub digital: bool,
    pub error: Option<&'static str>,
}

/// Signature ink: dark blue.
const INK: u32 = 0x14286E;

impl SignBox {
    /// What the signature shows; None when there is nothing to show.
    pub fn look(&self) -> Option<SigLook> {
        let mut lines = Vec::new();
        if !self.name.trim().is_empty() {
            lines.push(self.name.trim().to_string());
        }
        if self.date {
            lines.push(today());
        }
        let ink = if self.mode == Mode::Draw { self.strokes.iter().filter(|s| !s.is_empty()).cloned().collect() } else { Vec::new() };
        let picture = if self.mode == Mode::Image { self.picture.clone() } else { None };
        let mark = !ink.is_empty() || picture.is_some();
        if (self.mode != Mode::Type && !mark) || (self.mode == Mode::Type && self.name.trim().is_empty()) {
            return None;
        }
        Some(SigLook { lines, picture, ink, color: INK })
    }
}

impl PdfView {
    /// Fill & Sign > Signature: the box to make one.
    pub fn open_sign_box(&mut self) {
        self.sign_box = Some(SignBox { mode: Mode::Type, name: self.author.clone(), date: true, strokes: Vec::new(), picture: None, digital: true, error: None });
    }

    pub(super) fn sign_box_ui(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.sign_box else { return };
        let res = widgets::dialog(ctx, "pdf_sign_look", tr("pdf.sign_visible"), 460.0, |ui| {
            ui.horizontal(|ui| {
                for (m, key) in [(Mode::Type, "pdf.sign_type"), (Mode::Draw, "pdf.sign_draw"), (Mode::Image, "pdf.sign_image")] {
                    ui.selectable_value(&mut b.mode, m, tr(key));
                }
            });
            ui.separator();
            widgets::form(ui, "pdf_sign_look_form", |ui| {
                widgets::form_row(ui, tr("pdf.sign_name"), |ui| ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(280.0)));
                ui.end_row();
            });
            match b.mode {
                Mode::Type => {
                    let text = if b.name.trim().is_empty() { " " } else { b.name.trim() };
                    ui.label(egui::RichText::new(text).italics().size(26.0).color(Color32::from_rgb(0x14, 0x28, 0x6E)));
                }
                Mode::Draw => {
                    let (r, resp) = ui.allocate_exact_size(vec2(420.0, 130.0), Sense::drag());
                    let p = ui.painter_at(r);
                    p.rect_filled(r, 4.0, Color32::WHITE);
                    p.line_segment([r.left_bottom() + vec2(20.0, -30.0), r.right_bottom() + vec2(-20.0, -30.0)], Stroke::new(1.0, Color32::LIGHT_GRAY));
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let pt = [pos.x - r.min.x, pos.y - r.min.y];
                        if resp.drag_started() {
                            let o = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                            b.strokes.push(vec![[o.x - r.min.x, o.y - r.min.y], pt]);
                        } else if resp.dragged()
                            && let Some(s) = b.strokes.last_mut()
                        {
                            s.push(pt);
                        }
                    }
                    let stroke = Stroke::new(2.0, Color32::from_rgb(0x14, 0x28, 0x6E));
                    for s in &b.strokes {
                        p.add(egui::Shape::line(s.iter().map(|q| r.min + vec2(q[0], q[1])).collect(), stroke));
                    }
                    if ui.add_enabled(!b.strokes.is_empty(), egui::Button::new(tr("pdf.sign_clear"))).clicked() {
                        b.strokes.clear();
                    }
                }
                Mode::Image => {
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}…", tr("pdf.sign_pick_image"))).clicked() {
                            crate::file_dialog::FileDialog::new().add_filter(tr("pdf.sign_images"), &["png", "jpg", "jpeg", "gif", "bmp", "webp"]).pick_file(crate::file_dialog::in_view(|v: &mut PdfView, path: std::path::PathBuf| {
                                let Some(b) = &mut v.sign_box else { return };
                                match std::fs::read(&path).ok().and_then(|d| crate::images::decode(&d)) {
                                    Some(img) => {
                                        b.picture = Some(Picture { w: img.width, h: img.height, rgba: img.rgba.to_vec() });
                                        b.error = None;
                                    }
                                    None => b.error = Some("pdf.sign_bad_image"),
                                }
                            }));
                        }
                        if let Some(p) = &b.picture {
                            ui.label(trf("pdf.sign_image_size", &[("w", &p.w), ("h", &p.h)]));
                        }
                    });
                }
            }
            ui.add_space(6.0);
            ui.checkbox(&mut b.date, tr("pdf.sign_add_date"));
            ui.checkbox(&mut b.digital, tr("pdf.sign_digital"));
            ui.label(egui::RichText::new(tr("pdf.sign_place_hint")).weak().small());
            if let Some(e) = b.error {
                ui.colored_label(ui.visuals().error_fg_color, tr(e));
            }
        });
        match res {
            DialogResult::Ok => match b.look() {
                Some(look) => {
                    let digital = b.digital;
                    self.sign_box = None;
                    self.tool = Tool::PlaceSign;
                    self.picked = None;
                    self.sig_look = Some((look, digital));
                }
                None => b.error = Some("pdf.sign_empty"),
            },
            DialogResult::Cancel => self.sign_box = None,
            DialogResult::None => {}
        }
    }
}
