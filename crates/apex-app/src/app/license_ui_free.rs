//! The free build has no business license: no badge, no License dialog.

use eframe::egui;

use super::App;

pub(super) struct LicenseState;

impl LicenseState {
    pub(super) fn new() -> LicenseState {
        LicenseState
    }

    pub(super) fn badge(&mut self, _ui: &mut egui::Ui) {}
}

impl App {
    pub(super) fn license_about(&mut self, _ui: &mut egui::Ui) {}

    pub(super) fn license_button(&mut self, _ui: &mut egui::Ui) {}

    pub(super) fn license_dialog_ui(&mut self, _ctx: &egui::Context) {}
}
