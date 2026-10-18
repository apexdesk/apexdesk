//! File > Check Before Sharing is part of ApexDesk Empresa; in the free
//! build files are shared and saved without a check.

use std::path::{Path, PathBuf};

use apex_io::Format;
use eframe::egui;

use super::App;

/// What comes after the check when the user goes on.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Then {
    Nothing,
    ExportPdf { dialog: bool },
    Mail { pdf: bool },
    Save { path: PathBuf, fmt: Format },
}

pub(super) struct CheckDlg;

impl App {
    pub(super) fn open_check(&mut self) {}

    pub(super) fn share_checked(&mut self, _idx: usize, _then: Then) -> bool {
        true
    }

    pub(super) fn props_fit(&mut self, _idx: usize, _path: &Path, _fmt: Format) -> bool {
        true
    }

    pub(super) fn check_dialog(&mut self, _ctx: &egui::Context) {}
}
