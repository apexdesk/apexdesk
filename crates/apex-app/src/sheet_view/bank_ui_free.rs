//! Data > Bank Reconciliation is part of ApexDesk Empresa; the free build
//! has no dialog.

use super::*;

pub(super) struct BankDlg;

impl SheetView {
    pub(super) fn open_bank_rec(&mut self) {}

    pub(super) fn bank_dialog(&mut self, _ctx: &egui::Context, _b: &mut BankDlg) -> widgets::DialogResult {
        widgets::DialogResult::Cancel
    }

    pub(super) fn bank_dialog_ok(&mut self, _b: Box<BankDlg>) {}
}
