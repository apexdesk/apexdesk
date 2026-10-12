//! Where the signing certificate comes from, in both signing dialogs: a
//! certificate file (.p12, .pfx), the system's store (on Linux, the NSS
//! databases of Firefox, Thunderbird and Chromium) or a smart card or
//! token through its PKCS #11 module. The stores and tokens are listed in
//! the background; certificates that can't sign documents (expired, a
//! CA's, another key usage) are hidden unless asked for. Only the source,
//! the certificate (its file, or its SHA-1) and the module are remembered:
//! never a password or a PIN.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use apex_sign::Signer;
use apex_sign::signer::{self, CertInfo};
use apex_sign::store::{KeyCert, KeyError};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::file_dialog::Slot;
use crate::i18n::{tr, trf};
use crate::settings::Settings;
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Source {
    File,
    Store,
    Token,
}

impl Source {
    fn key(self) -> &'static str {
        match self {
            Source::File => "file",
            Source::Store => "store",
            Source::Token => "token",
        }
    }

    fn parse(s: &str) -> Source {
        match s {
            "store" => Source::Store,
            "token" => Source::Token,
            _ => Source::File,
        }
    }
}

type Listing = Result<Vec<KeyCert>, String>;

/// The certificate part of a signing dialog.
pub(super) struct CertPick {
    source: Source,
    file: String,
    /// The file's password or the card's PIN, typed for this signing only.
    secret: String,
    module: String,
    certs: Option<Listing>,
    loading: Option<JoinHandle<Listing>>,
    /// The chosen certificate's SHA-1.
    selected: String,
    show_all: bool,
    /// Browse's answers: the certificate file, the PKCS #11 module.
    file_pick: Slot<PathBuf>,
    module_pick: Slot<String>,
}

impl CertPick {
    pub(super) fn new(st: &Settings) -> CertPick {
        let source = Source::parse(&st.sign_source);
        let mut p = CertPick { source, file: st.sign_cert.clone(), secret: String::new(), module: st.pkcs11_module.clone(), certs: None, loading: None, selected: st.sign_key.clone(), show_all: false, file_pick: Slot::default(), module_pick: Slot::default() };
        if p.module.is_empty() {
            p.module = apex_sign::pkcs11::suggestions().first().map(|m| m.to_string_lossy().into_owned()).unwrap_or_default();
        }
        p.reload();
        p
    }

    /// Lists the store's or the token's certificates again, in the
    /// background.
    fn reload(&mut self) {
        self.certs = None;
        self.loading = match self.source {
            Source::File => None,
            Source::Store => Some(std::thread::spawn(|| apex_sign::store::list().map_err(|e| e.to_string()))),
            Source::Token if self.module.trim().is_empty() => {
                self.certs = Some(Err(tr("sign.no_module").to_string()));
                None
            }
            Source::Token => {
                let module = self.module.trim().to_string();
                Some(std::thread::spawn(move || apex_sign::pkcs11::list(Path::new(&module)).map_err(|e| e.to_string())))
            }
        };
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if self.loading.as_ref().is_some_and(|j| j.is_finished())
            && let Some(job) = self.loading.take()
        {
            self.certs = Some(job.join().unwrap_or_else(|_| Err("panic".into())));
            // The last certificate, else the first that can sign.
            if self.chosen().is_none() {
                let now = apex_sign::time::now();
                if let Some(Ok(list)) = &self.certs
                    && let Some(k) = list.iter().find(|k| signer::can_sign(&k.cert, now))
                {
                    self.selected = k.cert.sha1();
                }
            }
        } else if self.loading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The chosen certificate in the list, if still there.
    fn chosen(&self) -> Option<&KeyCert> {
        self.certs.as_ref()?.as_ref().ok()?.iter().find(|k| k.cert.sha1() == self.selected)
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        if let Some(p) = self.file_pick.take() {
            self.file = p.to_string_lossy().into_owned();
        }
        if let Some(p) = self.module_pick.take() {
            self.module = p;
            self.reload();
        }
        ui.horizontal(|ui| {
            ui.label(tr("sign.source"));
            let before = self.source;
            ui.radio_value(&mut self.source, Source::File, tr("sign.src_file"));
            ui.radio_value(&mut self.source, Source::Store, tr("sign.src_store"));
            ui.radio_value(&mut self.source, Source::Token, tr("sign.src_token"));
            if self.source != before {
                self.secret.clear();
                self.reload();
            }
        });
        ui.add_space(4.0);
        match self.source {
            Source::File => {
                widgets::form(ui, "sign_cert_file", |ui| {
                    widgets::form_row(ui, tr("sign.cert_file"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.file).desired_width(300.0));
                        if ui.button(format!("{}…", tr("sign.browse"))).clicked() {
                            let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("sign.cert_file")).add_filter(tr("sign.filter"), &["p12", "pfx"]);
                            if let Some(dir) = Path::new(&self.file).parent().filter(|p| p.is_dir()) {
                                dlg = dlg.set_directory(dir);
                            }
                            dlg.pick_file(self.file_pick.fill());
                        }
                    });
                    widgets::form_row(ui, tr("sign.password"), |ui| ui.add(egui::TextEdit::singleline(&mut self.secret).password(true).desired_width(300.0)));
                });
                ui.label(RichText::new(tr("sign.password_note")).weak().small());
            }
            Source::Store | Source::Token => self.list_ui(ui),
        }
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) {
        if self.source == Source::Token {
            ui.horizontal(|ui| {
                ui.label(tr("sign.module"));
                let r = ui.add(egui::TextEdit::singleline(&mut self.module).desired_width(280.0));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.reload();
                }
                if ui.button(format!("{}…", tr("sign.browse"))).clicked() {
                    pick_module(&self.module, self.module_pick.fill());
                }
            });
        } else if cfg!(not(any(windows, target_os = "macos"))) {
            ui.label(RichText::new(tr("sign.store_linux")).weak().small());
        }
        let now = apex_sign::time::now();
        let mut refresh = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("sign_certs").max_height(180.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                match &self.certs {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tr("sign.searching"));
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    Some(Ok(list)) => {
                        let shown: Vec<&KeyCert> = list.iter().filter(|k| self.show_all || signer::can_sign(&k.cert, now)).collect();
                        if shown.is_empty() {
                            ui.label(RichText::new(if list.is_empty() { tr("sign.no_certs") } else { tr("sign.no_fit_certs") }).weak());
                        }
                        for k in shown {
                            let info = CertInfo::of(&k.cert);
                            let on = info.sha1 == self.selected;
                            if ui.add(egui::Button::selectable(on, cert_text(ui, k, &info, now, on)).min_size(egui::vec2(ui.available_width(), 0.0))).clicked() {
                                self.selected = info.sha1.clone();
                            }
                        }
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_all, tr("sign.show_all"));
            refresh = ui.button(tr("sign.refresh")).clicked();
        });
        if refresh {
            self.reload();
        }
        if self.chosen().is_some_and(|k| k.needs_pin) {
            widgets::form(ui, "sign_pin", |ui| {
                widgets::form_row(ui, tr("sign.pin"), |ui| ui.add(egui::TextEdit::singleline(&mut self.secret).password(true).desired_width(160.0)));
            });
            ui.label(RichText::new(tr("sign.pin_note")).weak().small());
        } else if self.source == Source::Store && cfg!(any(windows, target_os = "macos")) {
            ui.label(RichText::new(tr("sign.system_asks")).weak().small());
        }
    }

    /// The signer the dialog chose, reading the file or opening the key
    /// (the password or PIN is then forgotten), and what to remember of
    /// it. Err: a message for the dialog.
    pub(super) fn signer(&mut self, st: &mut Settings) -> Result<Box<dyn Signer>, String> {
        let secret = std::mem::take(&mut self.secret);
        let signer: Box<dyn Signer> = match self.source {
            Source::File => {
                let cert = self.file.trim().to_string();
                if cert.is_empty() {
                    return Err(tr("sign.no_cert").to_string());
                }
                let data = std::fs::read(&cert).map_err(|e| trf("sign.bad_cert", &[("error", &e.to_string())]))?;
                let id = apex_sign::p12::read(&data, &secret).map_err(|e| match e {
                    apex_sign::P12Error::WrongPassword => tr("sign.wrong_password").to_string(),
                    apex_sign::P12Error::NoKey => tr("sign.no_key").to_string(),
                    e => trf("sign.bad_cert", &[("error", &e.to_string())]),
                })?;
                st.sign_cert = cert;
                Box::new(id)
            }
            Source::Store | Source::Token => {
                let Some(k) = self.chosen() else { return Err(tr("sign.pick_cert").to_string()) };
                let pin = (k.needs_pin && !secret.is_empty()).then_some(secret.as_str());
                let s = k.signer(pin).map_err(|e| match e {
                    KeyError::NeedPin => tr("sign.need_pin").to_string(),
                    KeyError::WrongPin => tr("sign.wrong_pin").to_string(),
                    KeyError::PinLocked => tr("sign.pin_locked").to_string(),
                    KeyError::NotFound => tr("sign.key_gone").to_string(),
                    KeyError::Other(e) => trf("sign.key_failed", &[("error", &e)]),
                })?;
                st.sign_key = self.selected.clone();
                if self.source == Source::Token {
                    st.pkcs11_module = self.module.trim().to_string();
                }
                s
            }
        };
        st.sign_source = self.source.key().to_string();
        Ok(signer)
    }
}

/// A certificate in the list: subject, then issuer, validity, key usage
/// and where it is (in the selection's color when chosen).
fn cert_text(ui: &egui::Ui, k: &KeyCert, info: &CertInfo, now: i64, on: bool) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let v = ui.visuals();
    let (strong, weak) = if on { (v.selection.stroke.color, v.selection.stroke.color) } else { (v.strong_text_color(), v.weak_text_color()) };
    job.append(&info.subject, 0.0, egui::TextFormat::simple(body, strong));
    let mut facts = vec![format!("{}: {}", tr("sign.issuer"), info.issuer), trf("sign.validity", &[("from", &super::date(info.not_before)), ("to", &super::date(info.not_after))])];
    if !k.cert.valid_at(now) {
        facts.push(tr("sign.expired").to_string());
    }
    if !info.usage.is_empty() {
        let usage: Vec<&str> = info.usage.iter().map(|u| tr(usage_key(u))).collect();
        facts.push(usage.join(", "));
    }
    let place = [k.place.as_str(), k.label.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
    if !place.is_empty() {
        facts.push(place);
    }
    job.append(&format!("\n{}", facts.join("  ·  ")), 0.0, egui::TextFormat::simple(small, weak));
    job
}

fn usage_key(u: &str) -> &'static str {
    match u {
        "digital_signature" => "sign.ku_digital_signature",
        "non_repudiation" => "sign.ku_non_repudiation",
        "key_encipherment" => "sign.ku_key_encipherment",
        "data_encipherment" => "sign.ku_data_encipherment",
        "key_agreement" => "sign.ku_key_agreement",
        "cert_sign" => "sign.ku_cert_sign",
        _ => "sign.ku_crl_sign",
    }
}

/// Asks for a PKCS #11 module file; `then` gets its path.
pub(super) fn pick_module(current: &str, then: impl FnOnce(&mut App, String) + 'static) {
    let ext: &[&str] = if cfg!(windows) { &["dll"] } else if cfg!(target_os = "macos") { &["dylib", "so"] } else { &["so"] };
    let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("sign.module")).add_filter(tr("sign.module_filter"), ext);
    if let Some(dir) = Path::new(current).parent().filter(|p| p.is_dir()) {
        dlg = dlg.set_directory(dir);
    }
    dlg.pick_file(move |app, p| then(app, p.to_string_lossy().into_owned()));
}

/// Options: the PKCS #11 module, with the ones found installed (`set`
/// takes the one Browse picks).
pub(in crate::app) fn module_option(ui: &mut egui::Ui, module: &mut String, set: impl FnOnce(&mut App, String) + 'static) {
    widgets::section(ui, tr("sign.module_section"));
    ui.label(RichText::new(tr("sign.module_hint")).weak());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(module).desired_width(320.0));
        if ui.button(format!("{}…", tr("sign.browse"))).clicked() {
            pick_module(module, set);
        }
    });
    let found = apex_sign::pkcs11::suggestions();
    if !found.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("sign.module_found")).weak());
            for m in found {
                let text = m.to_string_lossy().into_owned();
                let name = m.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if ui.small_button(name).on_hover_text(&text).clicked() {
                    *module = text;
                }
            }
        });
    }
}
