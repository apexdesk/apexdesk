//! File > Extend signatures for the long term: adds to the signatures of
//! the active tab's file (or of a file picked, such as a signed PDF) the
//! chains and revocation answers that verifying them years later needs
//! (PAdES B-LT, XAdES-XL) and an archive time-stamp (B-LTA, XAdES-A); run
//! again, it renews the archive time-stamp. The answers are downloaded, so
//! it runs in the background.

use std::path::PathBuf;

use apex_sign::Level;
use eframe::egui::{self, RichText};

use super::{After, Signing, level_combo, level_key, level_setting};
use crate::app::{App, Notice};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

pub(in crate::app) struct ExtendDlg {
    path: PathBuf,
    /// The tab showing the file, with its format.
    tab: Option<(u64, apex_io::Format)>,
    level: Level,
    tsa_url: String,
    error: Option<String>,
}

impl App {
    /// File > Extend signatures: the active tab's signed file, else one to
    /// pick.
    pub(in crate::app) fn open_extend_signatures(&mut self) {
        let own = self.tabs.get(self.active).and_then(|t| {
            let s = self.sig.files.get(&t.id)?;
            Some((t.path.clone()?, t.id, s.fmt, t.view.modified() || t.password.is_some()))
        });
        match own {
            Some((_, _, _, true)) => self.notify(Notice::info(tr("sign.extend_unsaved"))),
            Some((path, id, fmt, false)) => self.show_extend(path, Some((id, fmt))),
            None => {
                crate::file_dialog::FileDialog::new().set_title(tr("sign.extend_pick")).add_filter(tr("sign.signed_files"), &["pdf", "docx", "docm", "xlsx", "xlsm", "odt", "ods"]).pick_file(|app, path| app.show_extend(path, None));
            }
        }
    }

    /// The Extend signatures dialog for `path` (open in `tab`, if it is).
    fn show_extend(&mut self, path: std::path::PathBuf, tab: Option<(u64, apex_io::Format)>) {
        let tsa_url = if self.settings.tsa_url.is_empty() { "http://timestamp.digicert.com".to_string() } else { self.settings.tsa_url.clone() };
        let level = level_setting(&self.settings.sign_level).max(Level::Lt);
        self.sig.extend = Some(ExtendDlg { path, tab, level, tsa_url, error: None });
    }

    pub(super) fn extend_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.sig.extend.take() else { return };
        let file = d.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let res = widgets::dialog(ctx, "extend_signatures", tr("sign.extend"), 520.0, |ui| {
            ui.label(RichText::new(&file).strong());
            ui.add_space(4.0);
            ui.label(RichText::new(tr("sign.extend_note")).weak().small());
            ui.add_space(6.0);
            widgets::form(ui, "extend_form", |ui| {
                widgets::form_row(ui, tr("sign.level_label"), |ui| level_combo(ui, "extend_level", &mut d.level, Level::Lt));
                widgets::form_row(ui, tr("sign.tsa"), |ui| ui.add_enabled(!crate::policy::locked("TimestampURL"), egui::TextEdit::singleline(&mut d.tsa_url).desired_width(300.0)));
            });
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        });
        match res {
            DialogResult::None => self.sig.extend = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let tsa = d.tsa_url.trim().to_string();
                if tsa.is_empty() {
                    d.error = Some(tr("sign.tsa_needed").to_string());
                    self.sig.extend = Some(d);
                    return;
                }
                if self.sig.running.is_some() {
                    d.error = Some(tr("sign.busy").to_string());
                    self.sig.extend = Some(d);
                    return;
                }
                self.settings.tsa_url = tsa.clone();
                self.settings.sign_level = level_key(d.level).to_string();
                self.settings.save();
                let (from, level) = (d.path.clone(), d.level);
                let job = apex_fs::thread::spawn(move || {
                    let data = apex_fs::read(&from).map_err(|e| e.to_string())?;
                    apex_sign::extend(&data, level, Some(&tsa)).map_err(|e| e.to_string())
                });
                let after = After::Extended(d.tab);
                self.sig.running = Some(Signing { path: d.path, job, after });
                self.notify(Notice::info(trf("sign.extending", &[("file", &file)])));
            }
        }
    }
}
