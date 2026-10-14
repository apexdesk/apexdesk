//! Digital signatures: the message bar and the details of signed files
//! (OOXML, ODF and, from the details, any PDF), keeping the signature when
//! an unchanged file is saved again, the warning before saving changes
//! drops it, and signing PDF exports (PAdES B-B, time-stamped when a
//! time-stamp authority is given, up to B-LTA). Signing the documents
//! themselves is in [`sign_doc`]; extending signatures for the long term
//! in [`extend`]; where the certificate comes from (a file, the system's
//! store, a card or token), in [`cert_source`].

mod cert_source;
mod extend;
mod sign_doc;
mod sign_macros;

pub(super) use cert_source::module_option;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use apex_io::Format;
use apex_sign::{Check, Level, Revocation, Signature, Status, Trust};
use eframe::egui::{self, Color32, RichText};

use super::{App, Notice};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::widgets::{self, DialogResult};

/// A signed file open in a tab: its signatures and its bytes as read, to
/// write them back when it's saved unchanged.
pub(super) struct Signed {
    sigs: Vec<Signature>,
    original: Vec<u8>,
    fmt: Format,
    password: Option<String>,
}

/// The signatures shown in the details dialog.
struct Details {
    file: String,
    sigs: Vec<Signature>,
}

/// A save that would drop the signature: tab, path and format asked for.
struct LosePrompt {
    tab: u64,
    path: PathBuf,
    fmt: Format,
}

/// File > Sign and export as PDF, and File > Sign document (`doc`).
struct SignDlg {
    tab: u64,
    doc: bool,
    pick: cert_source::CertPick,
    reason: String,
    location: String,
    stamp: bool,
    tsa_url: String,
    /// From T up (with `stamp`).
    level: Level,
    error: Option<String>,
    /// A PDF's empty signature field to sign in (a new one if none).
    field: Option<String>,
}

/// What a finished background signing updates.
enum After {
    /// A signed PDF export.
    Export,
    /// Tab `0`'s own file (in format `1`), signed.
    Tab(u64, Format),
    /// A file extended for the long term (a tab's own, or another).
    Extended(Option<(u64, Format)>),
}

/// A signing running in the background (the time stamp may take a while).
struct Signing {
    path: PathBuf,
    job: JoinHandle<Result<Vec<u8>, String>>,
    after: After,
}

#[derive(Default)]
pub(super) struct SigUi {
    files: HashMap<u64, Signed>,
    /// Tabs showing the signature bar.
    bars: HashSet<u64>,
    details: Option<Details>,
    prompt: Option<LosePrompt>,
    sign: Option<SignDlg>,
    running: Option<Signing>,
    /// A document to sign once saved, and one asking to be saved first.
    pending: Option<sign_doc::Pending>,
    save_first: Option<sign_doc::Pending>,
    extend: Option<extend::ExtendDlg>,
    macros: Option<sign_macros::SignMacros>,
}

/// The signatures of the file at `path` (encrypted OOXML files are read
/// with `password`), or None when it isn't signed.
pub(super) fn read_signed(path: &Path, password: Option<&str>, fmt: Format) -> Option<Signed> {
    // A damaged signature doesn't take the app down: it is just not shown.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_signed_unguarded(path, password, fmt))).ok().flatten()
}

fn read_signed_unguarded(path: &Path, password: Option<&str>, fmt: Format) -> Option<Signed> {
    let original = std::fs::read(path).ok()?;
    let sigs = if apex_io::crypto::is_encrypted(&original) {
        let plain = apex_io::crypto::decrypt(&original, password?).ok()?;
        if !apex_sign::is_signed(&plain) {
            return None;
        }
        apex_sign::verify(&plain)
    } else {
        if !apex_sign::is_signed(&original) {
            return None;
        }
        apex_sign::verify(&original)
    };
    // Only signatures of what the file was opened as: a file that is both
    // a PDF and a package mustn't show one's signature over the other.
    let pdf = fmt == apex_io::Format::Pdf;
    let mut sigs = sigs;
    sigs.retain(|s| matches!(s.kind, apex_sign::Kind::Pdf | apex_sign::Kind::PdfTimestamp) == pdf);
    // A PDF shown by scanning it shows objects its signatures may not
    // vouch for.
    if pdf && apex_io::pdf::read_by_repair(&original, password) {
        sigs.iter_mut().for_each(|s| s.changed_after = true);
    }
    if sigs.is_empty() {
        return None;
    }
    Some(Signed { sigs, original, fmt, password: password.map(str::to_string) })
}

/// The worst status of some signatures.
fn overall(sigs: &[Signature]) -> Status {
    sigs.iter().map(Signature::status).min().unwrap_or(Status::Unknown)
}

fn signers(sigs: &[Signature]) -> String {
    let mut names: Vec<String> = Vec::new();
    // Signers, not time-stamp authorities.
    for s in sigs.iter().filter(|s| s.kind != apex_sign::Kind::PdfTimestamp) {
        let n = s.signer();
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    }
    names.join(", ")
}

fn status_color(s: Status, dark: bool) -> Color32 {
    match (s, dark) {
        (Status::Valid, false) => Color32::from_rgb(0x2E, 0x7D, 0x32),
        (Status::Valid, true) => Color32::from_rgb(0x81, 0xC7, 0x84),
        (Status::Invalid, false) => Color32::from_rgb(0xC6, 0x28, 0x28),
        (Status::Invalid, true) => Color32::from_rgb(0xEF, 0x9A, 0x9A),
        (_, false) => Color32::from_rgb(0x9A, 0x6A, 0x00),
        (_, true) => Color32::from_rgb(0xFF, 0xD5, 0x4F),
    }
}

fn status_text(s: Status) -> &'static str {
    match s {
        Status::Valid => tr("sign.s_valid"),
        Status::Untrusted => tr("sign.s_untrusted"),
        Status::ChangedAfter => tr("sign.s_changed"),
        Status::Unknown => tr("sign.s_unknown"),
        Status::Invalid => tr("sign.s_invalid"),
    }
}

/// Year, month, day, hour and minute of Unix time `t`: in local time, or
/// as it is for times recorded without a zone.
fn fields(t: i64, as_is: bool) -> (i64, u32, u32, u32, u32) {
    if as_is || t < 0 {
        let (y, mo, d, h, mi, _) = apex_sign::time::civil(t);
        return (y, mo as u32, d as u32, h as u32, mi as u32);
    }
    crate::common::local_time(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t.min(crate::common::LAST_SECOND) as u64))
}

fn date_time(t: i64, as_is: bool) -> String {
    let (d, h) = crate::common::date_time_of(fields(t, as_is));
    format!("{d} {h}")
}

fn date(t: i64) -> String {
    crate::common::date_time_of(fields(t, false)).0
}

fn trust_text(t: &Trust) -> String {
    match t {
        Trust::Trusted(root) => trf("sign.t_trusted", &[("root", root)]),
        Trust::UnknownIssuer => tr("sign.t_unknown").to_string(),
        Trust::SelfSigned => tr("sign.t_self").to_string(),
        Trust::WrongUse => tr("sign.t_wrong_use").to_string(),
        Trust::Expired => tr("sign.t_expired").to_string(),
        Trust::NotVerified => tr("sign.t_not_verified").to_string(),
        Trust::Revoked(t) => trf("sign.t_revoked", &[("date", &date(*t))]),
    }
}

/// The level remembered in the settings ("t", "lt", "lta").
fn level_setting(s: &str) -> Level {
    Level::parse(s).filter(|l| *l >= Level::T).unwrap_or(Level::T)
}

fn level_key(l: Level) -> &'static str {
    match l {
        Level::B | Level::T => "t",
        Level::Lt => "lt",
        Level::Lta => "lta",
    }
}

fn level_text(l: Level) -> &'static str {
    match l {
        Level::B | Level::T => tr("sign.level_t"),
        Level::Lt => tr("sign.level_lt"),
        Level::Lta => tr("sign.level_lta"),
    }
}

/// A combo of the levels from `from` up.
fn level_combo(ui: &mut egui::Ui, id: &str, level: &mut Level, from: Level) {
    egui::ComboBox::from_id_salt(id).selected_text(level_text(*level)).width(300.0).show_ui(ui, |ui| {
        for l in [Level::T, Level::Lt, Level::Lta].into_iter().filter(|l| *l >= from) {
            ui.selectable_value(level, l, level_text(l));
        }
    });
}

fn row(ui: &mut egui::Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(label).weak());
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

/// One signature's details.
fn signature_ui(ui: &mut egui::Ui, i: usize, s: &Signature) {
    let dark = ui.visuals().dark_mode;
    let status = s.status();
    let title = if s.kind == apex_sign::Kind::PdfTimestamp { tr("sign.doc_timestamp").to_string() } else { s.signer() };
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong());
        ui.label(RichText::new(status_text(status)).color(status_color(status, dark)));
    });
    egui::Grid::new(("signature", i)).num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        if let Some(c) = &s.cert {
            row(ui, tr("sign.signer"), c.subject.full());
            row(ui, tr("sign.issuer"), c.issuer.full());
        }
        if let Some(t) = s.time {
            let when = date_time(t, s.local_time);
            let key = if s.timestamped { "sign.time_stamped" } else { "sign.time_clock" };
            row(ui, tr("sign.time"), trf(key, &[("time", &when)]));
        }
        let content = match s.intact {
            Check::Ok => tr("sign.intact"),
            Check::Failed => tr("sign.altered"),
            Check::Unknown => tr("sign.unchecked"),
        };
        row(ui, tr("sign.content"), content);
        let math = match s.valid {
            Check::Ok => tr("sign.math_ok"),
            Check::Failed => tr("sign.math_bad"),
            Check::Unknown => tr("sign.unchecked"),
        };
        row(ui, tr("sign.math"), math);
        if !s.changed_parts.is_empty() {
            row(ui, "", trf("sign.changed_parts", &[("parts", &s.changed_parts.join(", "))]));
        }
        if s.changed_after {
            let text = if s.unsigned_parts.is_empty() { tr("sign.later_pdf").to_string() } else { trf("sign.unsigned", &[("parts", &s.unsigned_parts.join(", "))]) };
            row(ui, tr("sign.later"), text);
        }
        if let Some(c) = &s.cert {
            row(ui, tr("sign.certificate"), RichText::new(trust_text(&s.trust)).color(status_color(if s.trust.is_trusted() { Status::Valid } else { Status::Untrusted }, dark)));
            row(ui, "", trf("sign.validity", &[("from", &date(c.not_before)), ("to", &date(c.not_after))]));
            if s.chain.len() > 1 {
                let chain: Vec<String> = s.chain.iter().map(|c| c.subject.display()).collect();
                row(ui, tr("sign.chain"), chain.join(" → "));
            }
            row(ui, tr("sign.serial"), RichText::new(c.serial_hex()).monospace());
            row(ui, tr("sign.fingerprint"), RichText::new(c.fingerprint()).monospace().small());
        }
        for (key, v) in [("sign.reason", &s.reason), ("sign.location", &s.location)] {
            if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
                row(ui, tr(key), apex_sign::x509::one_line(v));
            }
        }
        row(ui, tr("sign.kind"), s.format.as_str());
        // Long-term validation.
        if s.kind != apex_sign::Kind::PdfTimestamp {
            row(ui, tr("sign.level"), s.level_name());
            let rev = match s.revocation {
                Revocation::NotChecked => tr("sign.rev_none").to_string(),
                Revocation::Good(t) => trf("sign.rev_good", &[("date", &date(t))]),
                Revocation::Revoked(t) => trf("sign.rev_revoked", &[("date", &date(t))]),
            };
            row(ui, tr("sign.revocation"), rev);
        }
        if let Some(t) = s.verifiable_until {
            let key = if s.level == Level::Lta { "sign.until_renew" } else { "sign.until_date" };
            row(ui, tr("sign.until"), trf(key, &[("date", &date(t))]));
        }
        if let Some(t) = s.archive_time {
            row(ui, tr("sign.archive"), date_time(t, false));
        }
        if let Some(n) = &s.note {
            row(ui, "", RichText::new(n).weak());
        }
    });
}

impl SigUi {
    /// Tab `id` holds a signed file: it shows the bar.
    pub(super) fn note(&mut self, id: u64, s: Signed) {
        self.files.insert(id, s);
        self.bars.insert(id);
    }
}

impl App {
    /// The signature bar of the active tab, under the toolstrip.
    pub(super) fn signature_bar(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.tabs.get(self.active).map(|t| t.id).filter(|id| self.sig.bars.contains(id)) else { return };
        let Some(s) = self.sig.files.get(&id) else {
            self.sig.bars.remove(&id);
            return;
        };
        let status = overall(&s.sigs);
        let names = signers(&s.sigs);
        let text = match status {
            // A time-stamp only says when, not who.
            Status::Valid | Status::Untrusted | Status::ChangedAfter if apex_sign::stamps_only(&s.sigs) => tr("sign.bar_stamp_only").to_string(),
            Status::Valid => trf("sign.bar_valid", &[("names", &names)]),
            Status::Untrusted => trf("sign.bar_untrusted", &[("names", &names)]),
            Status::ChangedAfter => trf("sign.bar_changed", &[("names", &names)]),
            Status::Unknown => tr("sign.bar_unknown").to_string(),
            Status::Invalid => tr("sign.bar_invalid").to_string(),
        };
        let dark = ui.visuals().dark_mode;
        let status = if status == Status::Valid && apex_sign::stamps_only(&s.sigs) { Status::Untrusted } else { status };
        let (fill, edge) = match (status, dark) {
            (Status::Valid, false) => (Color32::from_rgb(0xE6, 0xF4, 0xEA), Color32::from_rgb(0x9C, 0xCF, 0xA8)),
            (Status::Valid, true) => (Color32::from_rgb(0x1C, 0x3A, 0x24), Color32::from_rgb(0x2A, 0x7A, 0x40)),
            (Status::Invalid, false) => (Color32::from_rgb(0xFD, 0xE7, 0xE9), Color32::from_rgb(0xE8, 0xA3, 0xA8)),
            (Status::Invalid, true) => (Color32::from_rgb(0x4A, 0x1C, 0x1F), Color32::from_rgb(0x8A, 0x2A, 0x30)),
            (_, false) => (Color32::from_rgb(0xFF, 0xF4, 0xCE), Color32::from_rgb(0xE8, 0xD0, 0x7A)),
            (_, true) => (Color32::from_rgb(0x4A, 0x40, 0x1C), Color32::from_rgb(0x8A, 0x74, 0x2A)),
        };
        let (mut open, mut close) = (false, false);
        egui::Panel::top("signature_bar").frame(egui::Frame::new().fill(fill).stroke(egui::Stroke::new(1.0, edge)).inner_margin(egui::Margin::symmetric(10, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                icons::paint(ui.painter(), r, Icon::Protect, ui.visuals().text_color());
                ui.label(RichText::new(tr("sign.bar_title")).strong());
                ui.label(text);
                open = ui.button(tr("sign.view")).clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                    let color = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
                    icons::paint(ui.painter(), r.shrink(3.0), Icon::Close, color);
                    close = resp.on_hover_text(tr("dialog.close")).clicked();
                });
            });
        });
        if open {
            self.open_signatures();
        }
        if close {
            self.sig.bars.remove(&id);
        }
    }

    /// File > Signatures: the details of the active tab's signatures.
    pub(super) fn open_signatures(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let sigs = self.sig.files.get(&t.id).map(|s| s.sigs.clone()).unwrap_or_default();
        self.sig.details = Some(Details { file: t.name(), sigs });
    }

    /// Verify a PDF: picks one and shows its signatures.
    fn verify_pdf(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("sign.verify_pdf")).add_filter("PDF", &["pdf"]).pick_file(|app, path| {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read(&path) {
                // A file that trips the checker shows no signatures.
                // Only a PDF's own signatures: a package that is also one
                // vouches for something else.
                Ok(data) => {
                    let mut sigs = std::panic::catch_unwind(|| apex_sign::verify(&data)).unwrap_or_default();
                    sigs.retain(|s| matches!(s.kind, apex_sign::Kind::Pdf | apex_sign::Kind::PdfTimestamp));
                    app.sig.details = Some(Details { file, sigs });
                }
                Err(e) => app.notify(Notice::error(trf("error.open", &[("file", &file), ("error", &e.to_string())]))),
            }
        });
    }

    /// Whether tab `idx` can be saved at `path` in `fmt` without dropping a
    /// signature; if not, asks first (and returns false).
    pub(super) fn signature_fits(&mut self, idx: usize, path: &Path, fmt: Format) -> bool {
        let Some(t) = self.tabs.get(idx) else { return true };
        if !self.sig.files.contains_key(&t.id) || self.signed_original(idx, fmt).is_some() {
            return true;
        }
        self.sig.prompt = Some(LosePrompt { tab: t.id, path: path.to_path_buf(), fmt });
        false
    }

    /// The file's bytes as read, when tab `idx` holds a signed file saved
    /// unchanged in its own format: writing them keeps the signature.
    pub(super) fn signed_original(&self, idx: usize, fmt: Format) -> Option<Vec<u8>> {
        let t = self.tabs.get(idx)?;
        let s = self.sig.files.get(&t.id)?;
        (!t.view.modified() && s.fmt == fmt && s.password == t.password).then(|| s.original.clone())
    }

    /// Who signed the file open in tab `id` (None: it isn't signed).
    pub(super) fn signers(&self, id: u64) -> Option<Vec<String>> {
        let s = self.sig.files.get(&id)?;
        Some(s.sigs.iter().map(|g| g.cert.as_ref().map(|c| c.subject.display()).unwrap_or_default()).collect())
    }

    /// Tab `id` was saved without its signature.
    pub(super) fn signature_dropped(&mut self, id: u64) {
        self.sig.files.remove(&id);
        self.sig.bars.remove(&id);
    }

    /// File > Sign and export as PDF.
    pub(super) fn open_sign_pdf(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let tsa_url = if self.settings.tsa_url.is_empty() { "http://timestamp.digicert.com".to_string() } else { self.settings.tsa_url.clone() };
        let level = level_setting(&self.settings.sign_level);
        self.sig.sign = Some(SignDlg { tab: t.id, doc: false, pick: cert_source::CertPick::new(&self.settings), reason: String::new(), location: String::new(), stamp: !self.settings.tsa_url.is_empty(), tsa_url, level, error: None, field: None });
    }

    /// The signature dialogs and the signing in progress.
    pub(super) fn signature_dialogs(&mut self, ctx: &egui::Context) {
        if !self.sig.files.is_empty() {
            let tabs = &self.tabs;
            self.sig.files.retain(|id, _| tabs.iter().any(|t| t.id == *id));
        }
        self.details_dialog(ctx);
        self.lose_dialog(ctx);
        self.sign_dialog(ctx);
        self.extend_dialog(ctx);
        self.sign_macros_dialog(ctx);
        self.save_first_dialog(ctx);
        self.poll_signing(ctx);
        self.sign_saved_doc();
    }

    fn details_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.sig.details else { return };
        let mut verify = false;
        let title = format!("{} — {}", tr("sign.title"), d.file);
        let closed = widgets::info_dialog(ctx, "signatures", &title, 560.0, |ui| {
            if d.sigs.is_empty() {
                ui.label(tr("sign.none"));
            }
            egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                for (i, s) in d.sigs.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    signature_ui(ui, i, s);
                }
            });
            ui.add_space(6.0);
            verify = ui.button(format!("{}…", tr("sign.verify_pdf"))).clicked();
        });
        if closed || verify {
            self.sig.details = None;
        }
        if verify {
            self.verify_pdf();
        }
    }

    /// The "save without the signature?" question.
    fn lose_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.sig.prompt else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == p.tab) else {
            self.sig.prompt = None;
            return;
        };
        let mut choice = None;
        let m = egui::Modal::new(egui::Id::new("signature_save")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("sign.lose_title"));
            ui.add_space(8.0);
            ui.label(tr("sign.lose"));
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.cancel")).clicked() {
                    choice = Some(false);
                }
                if ui.button(RichText::new(tr("sign.lose_save")).strong()).clicked() {
                    choice = Some(true);
                }
            });
        });
        if choice.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(false);
        }
        let Some(choice) = choice else { return };
        let Some(p) = self.sig.prompt.take() else { return };
        if choice && self.macros_fit(idx, &p.path, p.fmt) {
            self.save_to(idx, p.path, p.fmt);
        }
    }

    fn sign_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.sig.sign.take() else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == d.tab) else { return };
        let title = if d.doc { tr("sign.doc") } else { tr("sign.export") };
        let res = widgets::dialog(ctx, "sign_pdf", title, 560.0, |ui| {
            d.pick.ui(ui);
            ui.add_space(6.0);
            widgets::form(ui, "sign_pdf_form", |ui| {
                widgets::form_row(ui, tr("sign.reason"), |ui| ui.add(egui::TextEdit::singleline(&mut d.reason).desired_width(300.0)));
                if !d.doc {
                    widgets::form_row(ui, tr("sign.location"), |ui| ui.add(egui::TextEdit::singleline(&mut d.location).desired_width(300.0)));
                }
            });
            ui.add_space(6.0);
            if d.doc {
                ui.label(RichText::new(tr("sign.doc_note")).weak().small());
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.stamp, tr("sign.tsa"));
                ui.add_enabled(d.stamp && !crate::policy::locked("TimestampURL"), egui::TextEdit::singleline(&mut d.tsa_url).desired_width(240.0));
            });
            ui.add_enabled_ui(d.stamp && !crate::policy::locked("SignatureLevel"), |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("sign.level_label"));
                    level_combo(ui, "sign_level", &mut d.level, Level::T);
                });
            });
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        });
        match res {
            DialogResult::None => self.sig.sign = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let res = if d.doc { self.start_doc_signing(idx, &mut d) } else { self.start_signing(idx, &mut d) };
                if let Err(e) = res {
                    d.error = Some(e);
                    self.sig.sign = Some(d);
                }
            }
        }
    }

    /// The dialog's signer (a password or PIN is forgotten once used); the
    /// source and the certificate are remembered. Err: a message for the
    /// dialog.
    fn signer(&mut self, d: &mut SignDlg) -> Result<Box<dyn apex_sign::Signer>, String> {
        let res = d.pick.signer(&mut self.settings);
        if res.is_ok() {
            self.settings.save();
        }
        res
    }

    /// Reads the certificate, asks where to save and signs the export in
    /// the background. Err: a message for the dialog.
    fn start_signing(&mut self, idx: usize, d: &mut SignDlg) -> Result<(), String> {
        let id = self.signer(d)?;
        let tsa = self.remember_stamp(d);
        let some = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        let level = if tsa.is_some() { d.level } else { Level::B };
        let opts = apex_sign::SignOptions { reason: some(&d.reason), location: some(&d.location), tsa_url: tsa, level, ..Default::default() };
        self.pdf_path(idx, move |app, idx, path| {
            let ctx = app.ctx.clone();
            let Some(job) = app.print_job(&ctx, idx, crate::print::Scope::ActiveSheet) else { return };
            let all: Vec<usize> = (0..job.len()).collect();
            let pdf = app.job_pdf(&ctx, idx, &job, &all);
            let job = std::thread::spawn(move || apex_sign::sign_pdf(&pdf, id.as_ref(), &opts).map_err(|e| e.to_string()));
            app.sig.running = Some(Signing { path, job, after: After::Export });
            app.notify(Notice::info(tr("sign.signing")));
        });
        Ok(())
    }

    fn poll_signing(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.sig.running else { return };
        if !r.job.is_finished() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let Some(r) = self.sig.running.take() else { return };
        let file = r.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let res = r.job.join().unwrap_or_else(|_| Err("panic".into())).and_then(|bytes| apex_io::write_atomic(&r.path, &bytes).map_err(|e| e.to_string()));
        match (res, r.after) {
            (Ok(()), After::Export) => self.notify(Notice::info(trf("sign.exported", &[("file", &file)]))),
            (Ok(()), After::Tab(tab, fmt)) => {
                self.signed_tab_file(tab, fmt, &r.path);
                self.notify(Notice::info(trf("sign.doc_signed", &[("file", &file)])));
            }
            (Ok(()), After::Extended(tab)) => {
                if let Some((tab, fmt)) = tab {
                    self.signed_tab_file(tab, fmt, &r.path);
                }
                self.notify(Notice::info(trf("sign.extended", &[("file", &file)])));
            }
            (Err(e), After::Export) => self.notify(Notice::error(trf("sign.failed", &[("error", &e)]))),
            (Err(e), _) => self.notify(Notice::error(trf("sign.doc_failed", &[("error", &e)]))),
        }
    }

    /// Tab `tab`'s file at `path` has new signatures.
    fn signed_tab_file(&mut self, tab: u64, fmt: Format, path: &Path) {
        if let Some(idx) = self.tabs.iter().position(|t| t.id == tab) {
            // A PDF tab shows the signed file and adds to it from now on.
            if let super::View::Pdf(v) = &mut self.tabs[idx].view
                && let Ok(data) = std::fs::read(path)
                && let Err(e) = v.reload(data)
            {
                self.notify(Notice::error(e.to_string()));
            }
            self.saved_file(idx, path);
            if let Some(s) = read_signed(path, None, fmt) {
                self.sig.note(tab, s);
            }
        }
    }

    /// The dialog's time-stamp authority (if stamping), remembered with

    /// the level for next time.
    fn remember_stamp(&mut self, d: &SignDlg) -> Option<String> {
        let tsa = d.tsa_url.trim().to_string();
        self.settings.tsa_url = if d.stamp { tsa.clone() } else { String::new() };
        self.settings.sign_level = level_key(d.level).to_string();
        self.settings.save();
        Some(tsa).filter(|t| d.stamp && !t.is_empty())
    }

}
