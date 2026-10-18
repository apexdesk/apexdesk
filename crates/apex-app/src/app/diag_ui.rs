//! File > Diagnostics: where the error log is and the crash reports
//! saved on this computer, each of which the user may read, mail (their
//! mail program opens with it; nothing is sent by itself) or delete. It
//! also opens by itself once after a crash.

use eframe::egui::{self, RichText};

use super::App;
use crate::diag;
use crate::i18n::tr;
use crate::widgets;

impl App {
    pub(super) fn diag_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_diag {
            return;
        }
        let crashed = self.diag_crashed;
        let to = self.settings.support_email.clone();
        let reports = diag::crashes();
        let closed = widgets::info_dialog(ctx, "diagnostics", tr("diag.title"), 560.0, |ui| {
            if crashed {
                ui.label(RichText::new(tr("diag.crashed")).strong());
                ui.add_space(4.0);
            }
            ui.label(RichText::new(tr("diag.intro")).weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(tr("diag.open_log")).clicked()
                    && let Some(p) = diag::log_path()
                {
                    crate::common::open_external(&p.to_string_lossy());
                }
                if ui.button(tr("diag.open_folder")).clicked()
                    && let Some(d) = diag::dir()
                {
                    let _ = apex_fs::create_dir_all(&d);
                    crate::common::open_external(&d.to_string_lossy());
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new(tr("diag.reports")).strong());
            if reports.is_empty() {
                ui.label(RichText::new(tr("diag.no_reports")).weak());
                return;
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                egui::Grid::new("diag_grid").num_columns(4).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                    for p in reports.iter().take(10) {
                        let name = p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.label(&name);
                        if ui.button(tr("diag.view")).clicked() {
                            crate::common::open_external(&p.to_string_lossy());
                        }
                        if ui.button(tr("diag.send")).on_hover_text(tr("diag.send_hint")).clicked() {
                            let text = apex_fs::read_to_string(p).unwrap_or_default();
                            crate::common::open_external(&diag::mail_link(&to, &name, &text));
                        }
                        if ui.button(tr("diag.delete")).clicked() {
                            let _ = apex_fs::remove_file(p);
                        }
                        ui.end_row();
                    }
                });
            });
        });
        if closed {
            self.show_diag = false;
            self.diag_crashed = false;
        }
    }
}
