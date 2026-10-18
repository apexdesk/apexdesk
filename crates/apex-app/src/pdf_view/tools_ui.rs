//! The PDF tab's Tools toolstrip tab: comparing with another version,
//! exporting pages as pictures, printing, the document's properties and
//! how it opens, and saving it as PDF/A for archiving.

use apex_io::pdf::edit::organize::parse_ranges;
use apex_io::pdf::edit::props::{Info, LAYOUTS, MODES, View};
use eframe::egui;

use super::{PdfView, Request};
use crate::app::Notice;
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::toolstrip::{self, Body};
use crate::widgets::{self, DialogResult};

/// Export as pictures: which pages and how sharp.
pub struct ExportBox {
    pub ranges: String,
    pub dpi: u32,
    pub bad: bool,
}

/// The properties dialog's copy of the properties.
pub struct PropsBox {
    pub info: Info,
    pub view: View,
    was: (Info, View),
}

fn layout_key(s: &str) -> &'static str {
    match s {
        "SinglePage" => "pdf.layout_single",
        "OneColumn" => "pdf.layout_continuous",
        "TwoColumnLeft" => "pdf.open_two_column",
        "TwoColumnRight" => "pdf.open_two_column_right",
        "TwoPageLeft" => "pdf.layout_two",
        "TwoPageRight" => "pdf.open_two_page_right",
        _ => "pdf.open_default",
    }
}

fn mode_key(s: &str) -> &'static str {
    match s {
        "UseNone" => "pdf.open_page_only",
        "UseOutlines" => "pdf.bookmarks",
        "UseThumbs" => "pdf.thumbnails",
        "FullScreen" => "pdf.open_full_screen",
        "UseAttachments" => "pdf.open_attachments",
        _ => "pdf.open_default",
    }
}

impl PdfView {
    pub(super) fn tools_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("pdf.g.compare", Icon::Compare, |ui| {
            if toolstrip::large(ui, Icon::Compare, tr("pdf.compare"), tr("pdf.compare_tip"), acc, self.compare.is_some()).clicked() {
                self.requests.push(Request::Compare);
            }
        });
        b.group("pdf.g.export", Icon::Picture, |ui| {
            if toolstrip::large(ui, Icon::Picture, tr("pdf.export_images"), tr("pdf.export_images_tip"), acc, false).clicked() {
                self.open_export();
            }
            if toolstrip::large(ui, Icon::Print, tr("print.print"), &format!("{} (Ctrl+P)", tr("pdf.print_tip")), acc, false).clicked() {
                self.open_print();
            }
        });
        b.group("pdf.g.document", Icon::Field, |ui| {
            if toolstrip::large(ui, Icon::Field, tr("pdf.properties"), tr("pdf.properties_tip"), acc, false).clicked() {
                self.open_props();
            }
            if toolstrip::large(ui, Icon::Save, tr("pdf.save_pdfa"), tr("pdf.save_pdfa_tip"), acc, false).clicked() {
                self.make_pdfa();
            }
        });
    }

    pub fn open_export(&mut self) {
        if !super::engine::available() {
            self.notices.push(Notice::error(tr("pdf.export_failed")));
            return;
        }
        let ranges = match self.chosen_pages().as_slice() {
            [] => String::new(),
            p => p.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(", "),
        };
        self.export_box = Some(ExportBox { ranges, dpi: 150, bad: false });
    }

    /// The pages the export box asks for (all when its field is empty).
    pub fn export_pages(&self, b: &ExportBox) -> Option<Vec<usize>> {
        let n = self.page_count();
        if b.ranges.trim().is_empty() {
            return Some((0..n).collect());
        }
        let r = parse_ranges(&b.ranges, n)?;
        let mut out: Vec<usize> = r.iter().flat_map(|&(a, z)| a..=z).collect();
        out.dedup();
        Some(out)
    }

    pub(super) fn export_ui(&mut self, ctx: &egui::Context) {
        let Some(mut b) = self.export_box.take() else { return };
        let res = widgets::dialog(ctx, "pdf_export_images", tr("pdf.export_images"), 340.0, |ui| {
            ui.label(tr("print.pages"));
            if ui.add(egui::TextEdit::singleline(&mut b.ranges).hint_text(tr("pdf.print_all_pages")).desired_width(220.0)).changed() {
                b.bad = false;
            }
            ui.horizontal(|ui| {
                ui.label(tr("pdf.export_dpi"));
                ui.add(egui::DragValue::new(&mut b.dpi).range(36..=600));
            });
            if b.bad {
                ui.colored_label(ui.visuals().error_fg_color, tr("pdf.split_bad"));
            }
        });
        match res {
            DialogResult::Ok => match self.export_pages(&b) {
                Some(p) if !p.is_empty() => self.requests.push(Request::ExportImages(p, b.dpi)),
                _ => {
                    b.bad = true;
                    self.export_box = Some(b);
                }
            },
            DialogResult::Cancel => {}
            DialogResult::None => self.export_box = Some(b),
        }
    }

    /// Draws `pages` at `dpi` and writes each as a PNG file at `path(k)`;
    /// returns how many were written.
    pub fn export_images(&mut self, pages: &[usize], dpi: u32, path: impl Fn(usize) -> std::path::PathBuf) -> Result<usize, String> {
        let bytes = self.bytes();
        let all = self.doc.pages();
        let mut n = 0;
        for &k in pages {
            let Some(&pn) = all.get(k) else { continue };
            let g = self.doc.geom(pn);
            let width = ((g.width * dpi as f64 / 72.0).round() as u32).clamp(16, 12_000);
            let Some(d) = super::engine::render_now(bytes.clone(), self.password.clone(), k, width) else { return Err(tr("pdf.export_failed").to_string()) };
            let png = encode_png(&d.rgba, d.size[0] as u32, d.size[1] as u32, dpi).map_err(|e| e.to_string())?;
            crate::common::write_out(path(k), png).map_err(|e| e.to_string())?;
            n += 1;
        }
        Ok(n)
    }

    /// Opens the properties dialog.
    pub fn open_props(&mut self) {
        let (info, view) = (self.doc.info(), self.doc.view());
        self.props_box = Some(PropsBox { info: info.clone(), view: view.clone(), was: (info, view) });
    }

    /// Changes the properties and how the file opens, as one step.
    pub fn apply_props(&mut self, info: &Info, view: &View) {
        let (old_info, old_view) = (self.doc.info(), self.doc.view());
        self.doc.begin("pdf.undo.properties");
        if *info != old_info {
            self.doc.set_info(info);
        }
        if *view != old_view {
            self.doc.set_view(view);
        }
        self.doc.end();
        self.after_change();
    }

    pub(super) fn props_ui(&mut self, ctx: &egui::Context) {
        let Some(mut b) = self.props_box.take() else { return };
        let claim = self.doc.pdfa_claim();
        let pages = self.page_count();
        let res = widgets::dialog(ctx, "pdf_props", tr("pdf.properties"), 440.0, |ui| {
            egui::Grid::new("pdf_props_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                for (key, v) in [("props.f_title", &mut b.info.title), ("props.f_author", &mut b.info.author), ("props.f_subject", &mut b.info.subject), ("props.f_keywords", &mut b.info.keywords)] {
                    ui.label(tr(key));
                    ui.add(egui::TextEdit::singleline(v).desired_width(280.0));
                    ui.end_row();
                }
                let shown = [
                    ("pdf.prop_creator", b.info.creator.clone()),
                    ("pdf.prop_producer", b.info.producer.clone()),
                    ("props.f_created", apex_io::pdf::edit::show_date(&b.info.created)),
                    ("pdf.prop_modified", apex_io::pdf::edit::show_date(&b.info.modified)),
                    ("pdf.prop_pages", pages.to_string()),
                    ("pdf.prop_pdfa", claim.as_ref().map_or_else(|| tr("pdf.prop_no").to_string(), |(p, c)| format!("PDF/A-{p}{}", c.to_lowercase()))),
                ];
                for (key, v) in shown {
                    ui.label(tr(key));
                    ui.label(egui::RichText::new(v).weak());
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.strong(tr("pdf.initial_view"));
            egui::Grid::new("pdf_view_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(tr("pdf.open_layout"));
                egui::ComboBox::from_id_salt("pdf_open_layout").selected_text(tr(layout_key(&b.view.layout))).show_ui(ui, |ui| {
                    ui.selectable_value(&mut b.view.layout, String::new(), tr("pdf.open_default"));
                    for l in LAYOUTS {
                        ui.selectable_value(&mut b.view.layout, l.to_string(), tr(layout_key(l)));
                    }
                });
                ui.end_row();
                ui.label(tr("pdf.open_pane"));
                egui::ComboBox::from_id_salt("pdf_open_mode").selected_text(tr(mode_key(&b.view.mode))).show_ui(ui, |ui| {
                    ui.selectable_value(&mut b.view.mode, String::new(), tr("pdf.open_default"));
                    for m in MODES {
                        ui.selectable_value(&mut b.view.mode, m.to_string(), tr(mode_key(m)));
                    }
                });
                ui.end_row();
            });
            ui.checkbox(&mut b.view.fit_window, tr("pdf.open_fit_window"));
            ui.checkbox(&mut b.view.center_window, tr("pdf.open_center_window"));
            ui.checkbox(&mut b.view.show_title, tr("pdf.open_show_title"));
        });
        match res {
            DialogResult::Ok => {
                if (b.info.clone(), b.view.clone()) != b.was {
                    self.apply_props(&b.info, &b.view);
                }
            }
            DialogResult::Cancel => {}
            DialogResult::None => self.props_box = Some(b),
        }
    }

    /// Makes the file PDF/A-2b (an undo step) and asks where to save it.
    pub fn make_pdfa(&mut self) {
        match self.doc.make_pdfa(&apex_pdf::srgb_icc()) {
            Ok(rep) => {
                self.after_change();
                if rep.unembedded_fonts.is_empty() {
                    self.notices.push(Notice::info(tr("pdf.pdfa_done")));
                } else {
                    self.notices.push(Notice::error(trf("pdf.pdfa_fonts", &[("fonts", &rep.unembedded_fonts.join(", "))])));
                }
                self.requests.push(Request::SaveAs);
            }
            Err(_) => self.notices.push(Notice::error(tr("pdf.pdfa_encrypted"))),
        }
    }
}

/// RGBA pixels as a PNG file that says its resolution.
pub fn encode_png(rgba: &[u8], w: u32, h: u32, dpi: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let ppm = (dpi as f64 / 0.0254).round() as u32;
        e.set_pixel_dims(Some(png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter }));
        let mut wr = e.write_header()?;
        wr.write_image_data(rgba)?;
    }
    Ok(out)
}
