//! The signing circuit is part of ApexDesk Empresa; the free build has no
//! dialog.

use eframe::egui;

use super::PdfView;

pub struct CircuitBox;

impl PdfView {
    pub fn open_circuit(&mut self) {}

    pub(super) fn circuit_ui(&mut self, _ctx: &egui::Context) {}
}
