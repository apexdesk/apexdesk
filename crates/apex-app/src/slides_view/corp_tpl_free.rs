//! Design > Company Template is part of ApexDesk Empresa; the free build
//! has no dialog.

use eframe::egui;

use super::SlidesView;

pub struct CorpDlg;

impl SlidesView {
    pub fn open_company_template(&mut self) {}

    pub(super) fn company_template_dialog(&mut self, _ctx: &egui::Context) {}
}
