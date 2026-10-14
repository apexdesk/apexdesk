//! Sign… in the macro editor: signs a workbook's or document's VBA project with the
//! user's code-signing certificate, the V3 way Excel 2019 and later check
//! (older ones see the project as unsigned). The project is written anew
//! for it, and the signature goes in its vbaProjectSignatureV3.bin part.

use std::sync::Arc;

use eframe::egui::{self, RichText};

use super::cert_source;
use crate::app::{App, Notice, View};
use crate::i18n::tr;
use crate::widgets::{self, DialogResult};

pub(super) struct SignMacros {
    tab: u64,
    pick: cert_source::CertPick,
    error: Option<String>,
}

const PART: &str = "vbaProjectSignatureV3.bin";
const CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProjectSignatureV3";
const REL_TYPE: &str = "http://schemas.microsoft.com/office/2020/07/relationships/vbaProjectSignatureV3";

impl App {
    /// The certificate dialog for signing tab `tab`'s macros.
    pub(in crate::app) fn open_sign_macros(&mut self, tab: u64) {
        let has_project = match self.tabs.iter().find(|t| t.id == tab).map(|t| &t.view) {
            Some(View::Sheet(v)) => v.wb.macros.project.is_some(),
            Some(View::Doc(v)) => v.doc.macros.project.is_some(),
            _ => false,
        };
        if !has_project {
            self.notify(Notice::info(tr("macros.sign_empty")));
            return;
        }
        self.sig.macros = Some(SignMacros { tab, pick: cert_source::CertPick::new(&self.settings), error: None });
    }

    pub(super) fn sign_macros_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.sig.macros.take() else { return };
        if !self.tabs.iter().any(|t| t.id == d.tab) {
            return;
        }
        let res = widgets::dialog(ctx, "sign_macros", tr("macros.sign_title"), 560.0, |ui| {
            d.pick.ui(ui);
            ui.add_space(6.0);
            ui.label(RichText::new(tr("macros.sign_note")).weak().small());
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        });
        match res {
            DialogResult::None => self.sig.macros = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let res = d.pick.signer(&mut self.settings).and_then(|id| {
                    self.settings.save();
                    self.sign_macros(d.tab, &*id)
                });
                match res {
                    Ok(()) => self.notify(Notice::info(tr("macros.signed_now"))),
                    Err(e) => {
                        d.error = Some(e);
                        self.sig.macros = Some(d);
                    }
                }
            }
        }
    }

    /// Signs tab `tab`'s project with `id`. Err: a message for the dialog.
    pub(in crate::app) fn sign_macros(&mut self, tab: u64, id: &dyn apex_sign::Signer) -> Result<(), String> {
        if !apex_sign::trust::fit_for_code_signing(id.certificate()) {
            return Err(tr("macros.sign_not_code").to_string());
        }
        self.flush_macros(tab);
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab) else { return Ok(()) };
        if !matches!(self.tabs[idx].view, View::Sheet(_) | View::Doc(_)) {
            return Ok(());
        }
        let Some(bin) = crate::app::macros_ui::macros_of(&mut self.tabs[idx].view).project.clone() else { return Err(tr("macros.sign_empty").to_string()) };
        let project = apex_io::vba::read(&bin).map_err(|e| e.to_string())?;
        if project.protected {
            return Err(tr("macros.locked").to_string());
        }
        let signed = project.write_for_signing().map_err(|e| e.to_string())?;
        let sig = apex_sign::vba::sign(id, &signed).map_err(|e| e.to_string())?;
        // What was signed is what ApexDesk itself would accept.
        if !apex_sign::vba::check(&sig, &signed).is_some_and(|s| s.intact && s.v3 && s.code_signing) {
            return Err(tr("macros.sign_failed").to_string());
        }
        self.hold(idx);
        let m = crate::app::macros_ui::macros_of(&mut self.tabs[idx].view);
        m.set_project(signed);
        m.parts.push(apex_doc::macros::KeptPart { path: PART.into(), content_type: CONTENT_TYPE.into(), rel_type: REL_TYPE.into(), owner: apex_doc::macros::PartOwner::Project, data: Arc::new(sig) });
        m.signed = true;
        crate::app::macros_ui::mark_modified(&mut self.tabs[idx].view, true);
        self.reload_macro_source(tab);
        Ok(())
    }
}
