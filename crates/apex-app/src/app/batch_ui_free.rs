//! File > Batch Processing is part of ApexDesk Empresa; the free build has
//! no dialog.

use eframe::egui;

use super::App;

pub(super) struct BatchDlg;

impl App {
    pub(super) fn open_batch(&mut self) {}

    pub(super) fn batch_dialog(&mut self, _ctx: &egui::Context) {}
}
