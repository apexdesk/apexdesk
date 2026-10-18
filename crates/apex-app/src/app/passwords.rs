//! Password-encrypted files: the password asked for on opening one, and
//! the password a file is saved with (File > Protect with Password); and
//! files whose author recommends opening them read-only.

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use super::recovery::Found;
use super::{App, View};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

/// Asking for the password of an encrypted file being opened (or of the
/// recovered copy of one).
pub(super) struct Prompt {
    path: PathBuf,
    restore: Option<Found>,
    text: String,
    wrong: bool,
    focus: bool,
    /// Enter was pressed in the field.
    submit: bool,
}

/// Setting or removing the password of a tab's file.
pub(super) struct EncryptDlg {
    tab: u64,
    pw: String,
    confirm: String,
    mismatch: bool,
}

impl App {
    /// Asks for the password of `path` (again, after a `wrong` one).
    pub(super) fn ask_password(&mut self, path: PathBuf, restore: Option<Found>, wrong: bool) {
        self.password_prompt = Some(Prompt { path, restore, text: String::new(), wrong, focus: true, submit: false });
    }

    pub(super) fn open_encrypt_dialog(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let pw = t.password.clone().unwrap_or_default();
        self.encrypt_dlg = Some(EncryptDlg { tab: t.id, confirm: pw.clone(), pw, mismatch: false });
    }

    pub(super) fn password_dialogs(&mut self, ctx: &egui::Context) {
        self.read_only_dialog(ctx);
        if let Some(mut p) = self.password_prompt.take() {
            let name = p.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = p.restore.as_ref().map_or(name, |f| f.name.clone());
            let res = widgets::dialog(ctx, "file_password", tr("password.title"), 360.0, |ui| {
                ui.label(trf("password.enter", &[("file", &name)]));
                let r = ui.add(egui::TextEdit::singleline(&mut p.text).password(true).desired_width(f32::INFINITY));
                if std::mem::take(&mut p.focus) {
                    r.request_focus();
                }
                if p.wrong {
                    ui.label(RichText::new(tr("password.wrong")).color(ui.visuals().error_fg_color));
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    p.submit = true;
                }
            });
            match res {
                DialogResult::Cancel => {}
                DialogResult::None if !p.submit => self.password_prompt = Some(p),
                _ => {
                    let pw = std::mem::take(&mut p.text);
                    match p.restore.take() {
                        Some(f) => {
                            if self.restore_with(&f, Some(&pw)) {
                                f.discard();
                            }
                        }
                        None => self.open_path_with(&p.path, Some(pw)),
                    }
                }
            }
        }
        if let Some(mut d) = self.encrypt_dlg.take() {
            let fmt = self.tabs.iter().find(|t| t.id == d.tab).and_then(|t| t.format);
            let res = widgets::dialog(ctx, "encrypt", tr("password.encrypt_title"), 380.0, |ui| {
                ui.label(tr("password.encrypt_intro"));
                ui.add_space(4.0);
                egui::Grid::new("encrypt_grid").num_columns(2).show(ui, |ui| {
                    ui.label(tr("password.password"));
                    ui.add(egui::TextEdit::singleline(&mut d.pw).password(true).desired_width(200.0));
                    ui.end_row();
                    ui.label(tr("password.confirm"));
                    ui.add(egui::TextEdit::singleline(&mut d.confirm).password(true).desired_width(200.0));
                    ui.end_row();
                });
                if d.mismatch {
                    ui.label(RichText::new(tr("password.mismatch")).color(ui.visuals().error_fg_color));
                }
                ui.add_space(4.0);
                ui.label(RichText::new(tr("password.warning")).weak().small());
                if fmt.is_some_and(|f| !apex_io::can_encrypt(f)) {
                    ui.label(RichText::new(tr("password.only_formats")).weak().small());
                }
            });
            match res {
                DialogResult::None => self.encrypt_dlg = Some(d),
                DialogResult::Cancel => {}
                DialogResult::Ok if d.pw != d.confirm => {
                    d.mismatch = true;
                    self.encrypt_dlg = Some(d);
                }
                DialogResult::Ok => {
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == d.tab) {
                        let pw = (!d.pw.is_empty()).then_some(d.pw);
                        if t.password != pw {
                            t.password = pw;
                            // Saving writes the file with the new password.
                            match &mut t.view {
                                View::Sheet(v) => v.wb.modified = true,
                                View::Doc(v) => v.doc.modified = true,
                                View::Slides(v) => v.pres.modified = true,
                                View::Pdf(_) | View::Db(_) | View::Note(_) => {}
                            }
                        }
                    }
                }
            }
        }
    }

    /// Asks whether a file opened read-only, as its author recommends,
    /// should be edited after all (Office's "Always Open Read-Only").
    fn read_only_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.read_only_ask else { return };
        if !self.tabs.iter().any(|t| t.path.as_deref() == Some(path.as_path())) {
            self.read_only_ask = None;
            return;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut choice = None;
        egui::Modal::new(egui::Id::new("read_only_recommended")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(tr("password.read_only_title"));
            ui.add_space(6.0);
            ui.label(trf("password.read_only_body", &[("file", &name)]));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(tr("password.read_only_keep")).strong()).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    choice = Some(false);
                }
                if ui.button(tr("password.read_only_edit")).clicked() {
                    choice = Some(true);
                }
            });
        });
        let Some(edit) = choice else { return };
        if let Some(p) = self.read_only_ask.take()
            && edit
        {
            self.edit_after_all(&p);
        }
    }
}
