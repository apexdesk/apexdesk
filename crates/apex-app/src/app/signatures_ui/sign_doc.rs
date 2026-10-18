//! File > Sign document: signs the saved docx, xlsx, odt or ods file of a
//! tab with a certificate file, keeping the signatures it already has.
//! Signing covers the file on disk, so a tab with unsaved changes is saved
//! first (after asking); the signed file then shows in the signature bar.
//! With a time-stamp authority the signature can go up to XAdES-A; the
//! signing then runs in the background.

use eframe::egui::{self, RichText};

use apex_io::Format;

use super::{After, SignDlg, Signing, cert_source, level_setting};
use crate::app::{App, Notice};
use crate::i18n::tr;

/// Formats whose files can be signed (their macro and template kinds too).
pub(super) fn signable(fmt: Format) -> bool {
    matches!(fmt, Format::Docx | Format::Xlsx | Format::Odt | Format::Ods | Format::Pdf)
}

/// Signs a PDF file. One never signed whose cross-references are
/// compressed (which the signing can't add to) is written out whole
/// first.
fn sign_pdf_file(data: Vec<u8>, id: &dyn apex_sign::Signer, opts: &apex_sign::SignOptions) -> Result<Vec<u8>, String> {
    match apex_sign::sign_pdf(&data, id, opts) {
        Err(e) if !apex_sign::is_signed(&data) && !apex_sign::pdf_can_add_to(&data) => {
            let whole = apex_io::pdf::edit::PdfDoc::open(data, None).map_err(|_| e.to_string())?.to_bytes_whole();
            apex_sign::sign_pdf(&whole, id, opts).map_err(|e| e.to_string())
        }
        r => r.map_err(|e| e.to_string()),
    }
}

/// A signing waiting for its tab to be saved.
pub(super) struct Pending {
    tab: u64,
    id: Box<dyn apex_sign::Signer>,
    reason: Option<String>,
    tsa: Option<String>,
    level: apex_sign::Level,
    field: Option<String>,
}

impl App {
    /// Why the active tab can't be signed, if it can't.
    pub(in crate::app) fn sign_doc_blocked(&self) -> Option<&'static str> {
        let t = self.tabs.get(self.active)?;
        if t.password.is_some() {
            return Some(tr("sign.doc_encrypted"));
        }
        if t.embed.is_some() || t.format.is_some_and(|f| !signable(f)) {
            return Some(tr("sign.doc_unsupported"));
        }
        None
    }

    /// File > Sign document: the certificate dialog.
    pub(in crate::app) fn open_sign_doc(&mut self) {
        self.open_sign_field(None);
    }

    /// Signing a PDF tab in its empty signature field `field` (a new
    /// invisible one if None), or any other signable tab.
    pub(in crate::app) fn open_sign_field(&mut self, field: Option<String>) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let tsa_url = if self.settings.tsa_url.is_empty() { "http://timestamp.digicert.com".to_string() } else { self.settings.tsa_url.clone() };
        let level = level_setting(&self.settings.sign_level);
        self.sig.sign = Some(SignDlg { tab: t.id, doc: true, pick: cert_source::CertPick::new(&self.settings), reason: String::new(), location: String::new(), stamp: !self.settings.tsa_url.is_empty(), tsa_url, level, error: None, field });
    }

    /// Reads the certificate and signs the tab's file, once saved. Err: a
    /// message for the dialog.
    pub(super) fn start_doc_signing(&mut self, idx: usize, d: &mut SignDlg) -> Result<(), String> {
        let id = self.signer(d)?;
        let tsa = self.remember_stamp(d);
        let level = if tsa.is_some() { d.level } else { apex_sign::Level::B };
        let Some(t) = self.tabs.get(idx) else { return Ok(()) };
        let reason = Some(d.reason.trim().to_string()).filter(|r| !r.is_empty());
        let p = Pending { tab: t.id, id, reason, tsa, level, field: d.field.clone() };
        // What other programs would show otherwise is saved as shown here.
        let otherwise = t.view.shown_otherwise() || (t.format != Some(Format::Pdf) && t.path.as_ref().and_then(|p| apex_fs::read(p).ok()).is_some_and(|d| apex_io::fidelity::shown_otherwise(&d)));
        if t.view.modified() || otherwise || t.path.is_none() || t.view.read_only() || t.format.is_none_or(|f| !signable(f)) {
            self.sig.save_first = Some(p);
        } else {
            self.sig.pending = Some(p);
        }
        Ok(())
    }

    /// "Save before signing?"
    pub(super) fn save_first_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.sig.save_first else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == p.tab) else {
            self.sig.save_first = None;
            return;
        };
        let mut choice = None;
        let m = egui::Modal::new(egui::Id::new("sign_save_first")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("sign.save_first_title"));
            ui.add_space(8.0);
            ui.label(tr("sign.save_first"));
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.cancel")).clicked() {
                    choice = Some(false);
                }
                if ui.button(RichText::new(tr("sign.save_and_sign")).strong()).clicked() {
                    choice = Some(true);
                }
            });
        });
        if choice.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(false);
        }
        let Some(choice) = choice else { return };
        let p = self.sig.save_first.take();
        if choice {
            self.sig.pending = p;
            // A save cancelled, or one that failed, signs nothing (a "drop
            // the signature?" question keeps it waiting for the answer).
            self.save_then(idx, false, |app, saved| {
                if !saved && app.sig.prompt.is_none() {
                    app.sig.pending = None;
                }
            });
        }
    }

    /// Signs the pending tab's file once it's saved.
    pub(super) fn sign_saved_doc(&mut self) {
        let Some(p) = &self.sig.pending else { return };
        // Not while the save asks where.
        if self.sig.prompt.is_some() || crate::file_dialog::busy() {
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == p.tab) else {
            self.sig.pending = None;
            return;
        };
        let Some(p) = self.sig.pending.take() else { return };
        let t = &self.tabs[idx];
        let (Some(path), Some(fmt)) = (t.path.clone(), t.format) else { return };
        if t.view.modified() {
            return;
        }
        // A PDF read by repairing it: other readers may repair it another
        // way and show something else. One never signed is written out
        // whole as shown, and signed so; a signed one can't be.
        let repaired = matches!(&t.view, crate::app::View::Pdf(v) if v.doc.repaired() && apex_sign::is_signed(v.doc.base()));
        if !signable(fmt) || t.password.is_some() || repaired {
            self.notify(Notice::error(tr("sign.doc_unsupported")));
            return;
        }
        if self.sig.running.is_some() {
            self.notify(Notice::info(tr("sign.busy")));
            return;
        }
        let opts = apex_sign::SignOptions { reason: p.reason, tsa_url: p.tsa, level: p.level, field: p.field, ..Default::default() };
        let id = p.id;
        // What is shown is signed, not what the file on disk may have
        // become since: a PDF's bytes as read; another file only as it was
        // when opened or saved here, and with nothing other programs show
        // otherwise.
        let data = match &t.view {
            crate::app::View::Pdf(v) if v.doc.repaired() => Ok(v.doc.to_bytes_whole()),
            crate::app::View::Pdf(v) => Ok(v.doc.base().to_vec()),
            view => {
                let before = self.unchanged_on_disk(&path);
                match apex_fs::read(&path) {
                    Ok(data) if before && self.unchanged_on_disk(&path) => {
                        if view.shown_otherwise() || apex_io::fidelity::shown_otherwise(&data) { Err("sign.doc_shown_otherwise") } else { Ok(data) }
                    }
                    _ => Err("sign.doc_changed"),
                }
            }
        };
        let data = match data {
            Ok(d) => d,
            Err(key) => {
                self.notify(Notice::error(tr(key)));
                return;
            }
        };
        let job = apex_fs::thread::spawn(move || {
            if fmt == Format::Pdf {
                return sign_pdf_file(data, id.as_ref(), &opts);
            }
            apex_sign::sign_package(&data, id.as_ref(), &opts).map_err(|e| e.to_string())
        });
        self.sig.running = Some(Signing { path, job, after: After::Tab(p.tab, fmt) });
        self.notify(Notice::info(tr("sign.signing_doc")));
    }

}
