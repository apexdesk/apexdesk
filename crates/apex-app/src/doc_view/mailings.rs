//! The Mailings menu: mail merge with data from a spreadsheet or CSV file,
//! a database or a Get Data query (see `mail_db`; merge fields are «Name»
//! text), and envelopes and labels.

use apex_doc::merge::{self, ENVELOPES, LABELS};
use apex_io::Loaded;
use eframe::egui;

use super::DocView;
use crate::common::AppCmd;
use crate::i18n::{tr, trf};

/// The data source: field names (the first row) and records.
pub(super) struct Recipients {
    file: String,
    pub(super) fields: Vec<String>,
    pub(super) records: Vec<Vec<String>>,
}

impl Recipients {
    pub(super) fn new(file: String, fields: Vec<String>, records: Vec<Vec<String>>) -> Recipients {
        Recipients { file, fields, records }
    }
}

enum Dialog {
    Envelopes { size: usize, ret: String, to: String, merge: bool },
    Labels { sheet: usize, text: String, merge: bool, full_page: bool },
}

#[derive(Default)]
pub(super) struct MailState {
    pub(super) recipients: Option<Recipients>,
    dialog: Option<Dialog>,
    /// Merge to Files.
    pub(super) files: Option<super::mail_files::FilesDlg>,
    /// A document for the app to open (and export as PDF).
    pub(super) out: Option<AppCmd>,
    /// Recipients from a database or a query: the dialog, and the reading.
    pub(super) db: Option<super::mail_db::DbDlg>,
    pub(super) job: Option<super::mail_db::Job>,
    /// Fill In Form.
    pub(super) form: Option<super::assemble_ui::FormDlg>,
}

/// Reads the first sheet of a spreadsheet or CSV file.
fn load_recipients(path: &std::path::Path) -> Result<Recipients, String> {
    let (loaded, _) = apex_io::open(path).map_err(|e| e.to_string())?;
    let Loaded::Sheet(wb) = loaded else { return Err(tr("mail.not_sheet").to_string()) };
    let (fields, records) = apex_io::records::records(&wb, 0, |n| trf("mail.column", &[("n", &n)])).ok_or_else(|| tr("mail.no_data").to_string())?;
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Recipients { file, fields, records })
}

/// A template with every field on its own line ("«Name»\n«Street»...").
fn address_block(r: &Recipients) -> String {
    r.fields.iter().take(6).map(|f| merge::field_text(f)).collect::<Vec<_>>().join("\n")
}

/// Fills the «fields» of a plain text for each record.
fn fill_text(text: &str, r: &Recipients) -> Vec<String> {
    r.records
        .iter()
        .map(|rec| {
            let mut t = text.to_string();
            for (i, f) in r.fields.iter().enumerate() {
                t = t.replace(&merge::field_text(f), rec.get(i).map_or("", String::as_str));
            }
            // Blank lines left by empty fields go away.
            t.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
        })
        .collect()
}

impl DocView {
    /// The Mailings tab of the toolstrip.
    pub(super) fn mailings_tab(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::icons::Icon;
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.create", Icon::Envelope, |ui| {
            if toolstrip::large(ui, Icon::Envelope, tr("mail.envelopes"), "", a, false).clicked() {
                let merge = self.mail.recipients.is_some();
                let to = match &self.mail.recipients {
                    Some(r) => address_block(r),
                    None if !self.sel.is_empty() => self.doc.text_range(self.sel.start(), self.sel.end()),
                    None => String::new(),
                };
                self.mail.dialog = Some(Dialog::Envelopes { size: 0, ret: String::new(), to, merge });
            }
            if toolstrip::large(ui, Icon::Labels, tr("mail.labels"), "", a, false).clicked() {
                let merge = self.mail.recipients.is_some();
                let text = match &self.mail.recipients {
                    Some(r) => address_block(r),
                    None if !self.sel.is_empty() => self.doc.text_range(self.sel.start(), self.sel.end()),
                    None => String::new(),
                };
                self.mail.dialog = Some(Dialog::Labels { sheet: 0, text, merge, full_page: !merge });
            }
        });
        b.group("toolstrip.g.start_merge", Icon::MailMerge, |ui| {
            let from_file = toolstrip::large_split(ui, Icon::MailMerge, tr("mail.select"), "", a, |ui| {
                if toolstrip::menu_item(ui, Icon::Sheet, &format!("{}…", tr("mail.from_file"))).clicked() {
                    self.pick_recipients();
                    ui.close();
                }
                if toolstrip::menu_item(ui, Icon::Table, &format!("{}…", tr("mail.from_db"))).clicked() {
                    self.open_mail_db(false);
                    ui.close();
                }
                if toolstrip::menu_item(ui, Icon::Pivot, &format!("{}…", tr("mail.from_query"))).clicked() {
                    self.open_mail_db(true);
                    ui.close();
                }
            });
            if from_file {
                self.pick_recipients();
            }
            let fields: Vec<String> = self.mail.recipients.as_ref().map(|r| r.fields.clone()).unwrap_or_default();
            ui.add_enabled_ui(!fields.is_empty(), |ui| {
                toolstrip::large_menu(ui, Icon::Field, tr("mail.insert_field"), "", a, |ui| {
                    for f in &fields {
                        if ui.button(f).clicked() {
                            self.insert(&merge::field_text(f));
                            ui.close();
                        }
                    }
                })
                .on_disabled_hover_text(tr("mail.no_recipients"));
            });
        });
        b.group("toolstrip.g.finish", Icon::Accept, |ui| {
            let can = self.mail.recipients.as_ref().is_some_and(|r| !r.records.is_empty());
            ui.add_enabled_ui(can, |ui| {
                let why = tr("mail.no_recipients");
                if toolstrip::large(ui, Icon::Doc, tr("mail.merge_doc"), "", a, false).on_disabled_hover_text(why).clicked() {
                    self.merge_out(false);
                }
                if toolstrip::large(ui, Icon::Pdf, tr("mail.merge_pdf"), "", a, false).on_disabled_hover_text(why).clicked() {
                    self.merge_out(true);
                }
                if toolstrip::large(ui, Icon::Folder, &format!("{}…", tr("mail.merge_files")), tr("mail.merge_files_tip"), a, false).on_disabled_hover_text(why).clicked() {
                    self.open_merge_files();
                }
            });
            if let Some(r) = &self.mail.recipients {
                toolstrip::column(ui, |ui| {
                    ui.weak(trf("mail.source", &[("n", &r.records.len()), ("file", &r.file)]));
                });
            }
        });
        self.assemble_group(b);
    }

    fn pick_recipients(&mut self) {
        let exts = ["xlsx", "xlsm", "ods", "csv", "tsv"];
        crate::file_dialog::FileDialog::new().set_title(tr("mail.select")).add_filter(tr("dialog.filter_sheets"), &exts).pick_file(crate::file_dialog::in_view(|v: &mut crate::doc_view::DocView, path: std::path::PathBuf| v.load_recipients_from(&path)));
    }

    pub(crate) fn load_recipients_from(&mut self, path: &std::path::Path) {
        match load_recipients(path) {
            Ok(r) => {
                self.notices.push(crate::app::Notice::info(trf("mail.loaded", &[("n", &r.records.len()), ("fields", &r.fields.len())])));
                self.mail.recipients = Some(r);
            }
            Err(e) => self.notices.push(crate::app::Notice::error(trf("error.open", &[("file", &path.display()), ("error", &e)]))),
        }
    }

    /// Merges the document with every record into a new document.
    pub(crate) fn merge_out(&mut self, pdf: bool) {
        let Some(r) = &self.mail.recipients else { return };
        let used = merge::fields_used(&self.doc);
        if let Some(missing) = used.iter().find(|u| !r.fields.contains(u)) {
            self.notices.push(crate::app::Notice::info(trf("mail.unknown_field", &[("field", missing)])));
        }
        let out = merge::merge(&self.doc, &r.fields, &r.records);
        self.mail.out = Some(AppCmd::OpenDoc(crate::doc_view::DocToOpen(std::sync::Arc::new(out)), pdf));
    }

    pub(super) fn mail_dialogs(&mut self, ctx: &egui::Context) {
        self.mail_db_dialog(ctx);
        self.fill_form_dialog(ctx);
        let Some(mut d) = self.mail.dialog.take() else { return };
        let fields: Vec<String> = self.mail.recipients.as_ref().map(|r| r.fields.clone()).unwrap_or_default();
        let has_data = !fields.is_empty();
        let pages = [(crate::icons::Icon::Envelope, "mail.envelopes"), (crate::icons::Icon::Labels, "mail.labels")];
        let mut page = usize::from(matches!(d, Dialog::Labels { .. }));
        let res = crate::widgets::nav_dialog(ctx, "mail_dialog", tr("mail.envelopes_labels"), &pages, &mut page, egui::vec2(440.0, 380.0), |ui, page| {
            // Switching pages keeps the address typed so far.
            match (&d, page) {
                (Dialog::Envelopes { to, merge, .. }, 1) => d = Dialog::Labels { sheet: 0, text: to.clone(), merge: *merge, full_page: !*merge },
                (Dialog::Labels { text, merge, .. }, 0) => d = Dialog::Envelopes { size: 0, ret: String::new(), to: text.clone(), merge: *merge },
                _ => {}
            }
            let field_menu = |ui: &mut egui::Ui, text: &mut String| {
                ui.add_enabled_ui(has_data, |ui| {
                    crate::widgets::menu_button(ui, tr("mail.insert_field"), |ui| {
                        for f in &fields {
                            if ui.button(f).clicked() {
                                if !text.is_empty() && !text.ends_with(['\n', ' ']) {
                                    text.push(' ');
                                }
                                text.push_str(&merge::field_text(f));
                                ui.close();
                            }
                        }
                    });
                });
            };
            match &mut d {
                Dialog::Envelopes { size, ret, to, merge } => {
                    crate::widgets::section(ui, tr("mail.delivery").trim_end_matches(':'));
                    ui.add(egui::TextEdit::multiline(to).desired_rows(4).desired_width(f32::INFINITY));
                    field_menu(ui, to);
                    crate::widgets::section(ui, tr("mail.return").trim_end_matches(':'));
                    ui.add(egui::TextEdit::multiline(ret).desired_rows(3).desired_width(f32::INFINITY));
                    crate::widgets::section(ui, tr("mail.envelope"));
                    crate::widgets::form(ui, "env_form", |ui| {
                        crate::widgets::form_row(ui, tr("mail.size").trim_end_matches(':'), |ui| {
                            egui::ComboBox::from_id_salt("env_size").width(220.0).selected_text(ENVELOPES[*size].name).show_ui(ui, |ui| {
                                for (i, e) in ENVELOPES.iter().enumerate() {
                                    ui.selectable_value(size, i, e.name);
                                }
                            });
                        });
                    });
                    crate::widgets::section(ui, tr("mail.options"));
                    ui.add_enabled(has_data, egui::Checkbox::new(merge, tr("mail.one_per_record")));
                }
                Dialog::Labels { sheet, text, merge, full_page } => {
                    crate::widgets::section(ui, tr("mail.label_text").trim_end_matches(':'));
                    ui.add(egui::TextEdit::multiline(text).desired_rows(4).desired_width(f32::INFINITY));
                    field_menu(ui, text);
                    crate::widgets::section(ui, tr("mail.label"));
                    crate::widgets::form(ui, "label_form", |ui| {
                        crate::widgets::form_row(ui, tr("mail.product").trim_end_matches(':'), |ui| {
                            egui::ComboBox::from_id_salt("label_sheet").width(250.0).selected_text(LABELS[*sheet].name).show_ui(ui, |ui| {
                                for (i, l) in LABELS.iter().enumerate() {
                                    ui.selectable_value(sheet, i, l.name);
                                }
                            });
                        });
                    });
                    crate::widgets::section(ui, tr("mail.options"));
                    ui.add_enabled(has_data, egui::Checkbox::new(merge, tr("mail.one_per_record")));
                    ui.add_enabled(!*merge || !has_data, egui::Checkbox::new(full_page, tr("mail.full_page")));
                }
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tr("mail.result_hint")).small().weak());
        });
        let done = res == crate::widgets::DialogResult::Ok;
        let (open, cancel) = (true, res == crate::widgets::DialogResult::Cancel);
        if done {
            let r = self.mail.recipients.as_ref();
            let out = match &d {
                Dialog::Envelopes { size, ret, to, merge } => {
                    let list = match r {
                        Some(r) if *merge => fill_text(to, r),
                        _ => vec![to.clone()],
                    };
                    merge::envelopes(&ENVELOPES[*size], ret, &list)
                }
                Dialog::Labels { sheet, text, merge, full_page } => {
                    let list = match r {
                        Some(r) if *merge => fill_text(text, r),
                        _ if *full_page => vec![text.clone(); LABELS[*sheet].cols * LABELS[*sheet].rows],
                        _ => vec![text.clone()],
                    };
                    merge::labels(&LABELS[*sheet], &list)
                }
            };
            let out = crate::doc_view::DocToOpen(std::sync::Arc::new(out));
            self.mail.out = Some(if matches!(d, Dialog::Labels { .. }) { AppCmd::OpenLabels(out) } else { AppCmd::OpenDoc(out, false) });
        } else if open && !cancel {
            self.mail.dialog = Some(d);
        }
    }

    pub(super) fn mail_dialog_open(&self) -> bool {
        self.mail.dialog.is_some() || self.mail.db.is_some()

    }
}
