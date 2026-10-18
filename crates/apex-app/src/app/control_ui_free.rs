//! File > Document Control is part of ApexDesk Empresa; the free build has
//! no dialog.

use eframe::egui;

use super::App;

pub(super) struct ControlDlg;

impl App {
    pub(super) fn open_control(&mut self) {}

    pub(super) fn control_dialog(&mut self, _ctx: &egui::Context) {}
}
