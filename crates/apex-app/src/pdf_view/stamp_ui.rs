//! Numbering pages: Bates numbers ("foliado"), "page n of total" and
//! diagonal watermarks, stamped into the pages.

use apex_io::pdf::edit::organize::parse_ranges;
use apex_io::pdf::edit::stamp::{Place, Stamp};
use eframe::egui;

use super::PdfView;
use crate::i18n::tr;
use crate::widgets::{self, DialogResult};

/// What the dialog starts from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Bates,
    PageNumbers,
    Watermark,
}

impl Preset {
    fn key(self) -> &'static str {
        match self {
            Preset::Bates => "pdf.stamp_bates",
            Preset::PageNumbers => "pdf.stamp_page_numbers",
            Preset::Watermark => "pdf.stamp_watermark",
        }
    }

    pub fn stamp(self) -> Stamp {
        match self {
            Preset::Bates => Stamp::default(),
            Preset::PageNumbers => Stamp { text: "{n} / {total}".into(), place: Place::BottomCenter, digits: 0, ..Stamp::default() },
            Preset::Watermark => Stamp { text: tr("pdf.stamp_watermark_text").to_string(), place: Place::Diagonal, color: 0xC00000, digits: 0, ..Stamp::default() },
        }
    }
}

fn place_name(p: Place) -> &'static str {
    tr(match p {
        Place::TopLeft => "pdf.stamp_top_left",
        Place::TopCenter => "pdf.stamp_top_center",
        Place::TopRight => "pdf.stamp_top_right",
        Place::BottomLeft => "pdf.stamp_bottom_left",
        Place::BottomCenter => "pdf.stamp_bottom_center",
        Place::BottomRight => "pdf.stamp_bottom_right",
        Place::Diagonal => "pdf.stamp_diagonal",
    })
}

pub struct StampBox {
    pub preset: Preset,
    pub stamp: Stamp,
    pub first: u64,
    /// Which pages (all when empty).
    pub ranges: String,
    pub bad: bool,
}

impl PdfView {
    pub fn open_stamp(&mut self) {
        let ranges = match self.chosen_pages().as_slice() {
            [] | [_] => String::new(),
            p => p.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(", "),
        };
        let stamp = self.last_stamp.clone().unwrap_or_else(|| Preset::Bates.stamp());
        let preset = if stamp.place == Place::Diagonal {
            Preset::Watermark
        } else if stamp.text.contains("{total}") {
            Preset::PageNumbers
        } else {
            Preset::Bates
        };
        self.stamp_box = Some(StampBox { preset, stamp, first: 1, ranges, bad: false });
    }

    /// Stamps the pages the box asks for; false when its pages are wrong.
    pub fn apply_stamp(&mut self, b: &StampBox) -> bool {
        let n = self.page_count();
        let which: Vec<usize> = if b.ranges.trim().is_empty() {
            Vec::new()
        } else {
            match parse_ranges(&b.ranges, n) {
                Some(r) => r.iter().flat_map(|&(a, z)| a..=z).collect(),
                None => return false,
            }
        };
        self.doc.stamp_pages(&which, &b.stamp, b.first, None);
        self.last_stamp = Some(b.stamp.clone());
        let sel = self.page_sel.clone();
        self.pages_changed(sel);
        true
    }

    pub(super) fn stamp_ui(&mut self, ctx: &egui::Context) {
        let Some(mut b) = self.stamp_box.take() else { return };
        let res = widgets::dialog(ctx, "pdf_stamp", tr("pdf.stamp"), 380.0, |ui| {
            ui.horizontal(|ui| {
                for p in [Preset::Bates, Preset::PageNumbers, Preset::Watermark] {
                    if ui.selectable_label(b.preset == p, tr(p.key())).clicked() && b.preset != p {
                        b.preset = p;
                        b.stamp = p.stamp();
                    }
                }
            });
            ui.add_space(4.0);
            egui::Grid::new("pdf_stamp_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(tr("pdf.stamp_text"));
                ui.add(egui::TextEdit::singleline(&mut b.stamp.text).desired_width(220.0)).on_hover_text(tr("pdf.stamp_text_tip"));
                ui.end_row();
                if b.stamp.place != Place::Diagonal {
                    ui.label(tr("pdf.stamp_first"));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut b.first).range(0..=u64::MAX / 2));
                        ui.label(tr("pdf.stamp_digits"));
                        ui.add(egui::DragValue::new(&mut b.stamp.digits).range(0..=12));
                    });
                    ui.end_row();
                }
                ui.label(tr("pdf.stamp_place"));
                egui::ComboBox::from_id_salt("pdf_stamp_place").selected_text(place_name(b.stamp.place)).show_ui(ui, |ui| {
                    for p in Place::ALL {
                        ui.selectable_value(&mut b.stamp.place, p, place_name(p));
                    }
                });
                ui.end_row();
                if b.stamp.place != Place::Diagonal {
                    ui.label(tr("pdf.stamp_size"));
                    ui.add(egui::DragValue::new(&mut b.stamp.size).range(4.0..=72.0).suffix(" pt"));
                    ui.end_row();
                }
                ui.label(tr("pdf.color"));
                let mut rgb = [(b.stamp.color >> 16) as u8, (b.stamp.color >> 8) as u8, b.stamp.color as u8];
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    b.stamp.color = u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
                }
                ui.end_row();
                ui.label(tr("print.pages"));
                if ui.add(egui::TextEdit::singleline(&mut b.ranges).hint_text(tr("pdf.print_all_pages")).desired_width(220.0)).changed() {
                    b.bad = false;
                }
                ui.end_row();
            });
            ui.add_space(4.0);
            let sample = b.stamp.text_for(b.first, b.first + self.page_count().saturating_sub(1) as u64);
            ui.label(egui::RichText::new(format!("{}: {sample}", tr("pdf.stamp_sample"))).weak());
            if b.bad {
                ui.colored_label(ui.visuals().error_fg_color, tr("pdf.split_bad"));
            }
        });
        match res {
            DialogResult::Ok => {
                if !self.apply_stamp(&b) {
                    b.bad = true;
                    self.stamp_box = Some(b);
                }
            }
            DialogResult::Cancel => {}
            DialogResult::None => self.stamp_box = Some(b),
        }
    }
}
