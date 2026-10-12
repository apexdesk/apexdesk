//! Mailings > Merge to Files: one file per record in a folder (Word,
//! OpenDocument, PDF or an e-mail message), named from a pattern with the
//! record number and its fields, as `apexdesk merge` writes them. The
//! messages (`.eml`, marked unsent) go to the address in a chosen field;
//! mail programs open them as drafts ready to send.

use apex_doc::merge;
use apex_io::Format;
use eframe::egui;

use super::DocView;
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

/// What each record becomes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Out {
    Docx,
    Odt,
    Pdf,
    Eml,
}

impl Out {
    const ALL: [Out; 4] = [Out::Docx, Out::Odt, Out::Pdf, Out::Eml];

    fn ext(self) -> &'static str {
        match self {
            Out::Docx => "docx",
            Out::Odt => "odt",
            Out::Pdf => "pdf",
            Out::Eml => "eml",
        }
    }
}

pub(super) struct FilesDlg {
    out: Out,
    /// File names: {n} is the record number, {Field} a field's value.
    pattern: String,
    /// For messages: the field with the address, and the subject.
    to_field: usize,
    subject: String,
}

/// The field most likely to hold e-mail addresses: by its name, else by
/// the first record's value.
pub(super) fn mail_field(fields: &[String], records: &[Vec<String>]) -> usize {
    let by_name = fields.iter().position(|f| {
        let f = f.to_lowercase();
        ["mail", "correo", "courriel", "posta", "メール", "메일", "邮件", "郵件"].iter().any(|k| f.contains(k))
    });
    by_name.or_else(|| records.first()?.iter().position(|v| v.contains('@'))).unwrap_or(0)
}

/// File names for the records, made unique (ignoring case).
pub(super) fn file_names(pattern: &str, fields: &[String], records: &[Vec<String>]) -> Vec<String> {
    let digits = records.len().to_string().len();
    let mut taken = std::collections::HashSet::new();
    records
        .iter()
        .enumerate()
        .map(|(k, rec)| {
            let mut name = crate::cli::merge_name(pattern, k + 1, digits, fields, rec);
            if name.is_empty() {
                name = format!("{:0digits$}", k + 1);
            }
            let mut unique = name.clone();
            let mut n = 2;
            while !taken.insert(unique.to_lowercase()) {
                unique = format!("{name}-{n}");
                n += 1;
            }
            unique
        })
        .collect()
}

impl DocView {
    pub(super) fn open_merge_files(&mut self) {
        let Some(r) = &self.mail.recipients else { return };
        let pattern = match r.fields.first() {
            Some(f) => format!("{{n}}-{{{f}}}"),
            None => "{n}".into(),
        };
        let to_field = mail_field(&r.fields, &r.records);
        self.mail.files = Some(FilesDlg { out: Out::Docx, pattern, to_field, subject: String::new() });
    }

    pub(super) fn merge_files_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.mail.files.take() else { return };
        let Some(r) = &self.mail.recipients else { return };
        let example = file_names(&d.pattern, &r.fields, &r.records[..r.records.len().min(1)]).pop().unwrap_or_default();
        let res = widgets::dialog(ctx, "merge_files", tr("mail.merge_files"), 420.0, |ui| {
            widgets::form(ui, "merge_files_form", |ui| {
                widgets::form_row(ui, tr("mail.files_format"), |ui| {
                    ui.horizontal(|ui| {
                        for o in Out::ALL {
                            ui.radio_value(&mut d.out, o, o.ext().to_uppercase());
                        }
                    })
                    .response
                });
                widgets::form_row(ui, tr("mail.files_name"), |ui| ui.add(egui::TextEdit::singleline(&mut d.pattern).desired_width(240.0)));
                if d.out == Out::Eml {
                    widgets::form_row(ui, tr("mail.files_to"), |ui| {
                        egui::ComboBox::from_id_salt("merge_files_to")
                            .selected_text(r.fields.get(d.to_field).map_or("", String::as_str))
                            .show_ui(ui, |ui| {
                                for (i, f) in r.fields.iter().enumerate() {
                                    ui.selectable_value(&mut d.to_field, i, f);
                                }
                            })
                            .response
                    });
                    widgets::form_row(ui, tr("mail.files_subject"), |ui| ui.add(egui::TextEdit::singleline(&mut d.subject).desired_width(240.0)));
                }
            });
            let field = format!("{{{}}}", r.fields.first().map_or("", String::as_str));
            ui.label(egui::RichText::new(trf("mail.files_hint", &[("n", &"{n}"), ("field", &field)])).weak().small());
            ui.label(trf("mail.files_example", &[("name", &format!("{example}.{}", d.out.ext()))]));
        });
        match res {
            DialogResult::None => self.mail.files = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                crate::file_dialog::FileDialog::new().set_title(tr("mail.merge_files")).pick_folder(crate::file_dialog::in_view(move |v: &mut DocView, dir: std::path::PathBuf| match v.write_merge_files(&dir, &d) {
                    Ok(n) => v.notices.push(crate::app::Notice::info(trf("mail.files_done", &[("n", &n), ("folder", &dir.display())]))),
                    Err(e) => v.notices.push(crate::app::Notice::error(e)),
                }));
            }
        }
    }

    /// Writes one file per record into `dir`; the number written.
    fn write_merge_files(&self, dir: &std::path::Path, d: &FilesDlg) -> Result<usize, String> {
        let Some(r) = &self.mail.recipients else { return Ok(0) };
        let names = file_names(&d.pattern, &r.fields, &r.records);
        for (k, (rec, name)) in r.records.iter().zip(&names).enumerate() {
            let doc = merge::fill(&self.doc, &r.fields, rec);
            let path = dir.join(format!("{name}.{}", d.out.ext()));
            let res = match d.out {
                Out::Docx => apex_io::save_doc(&doc, &path, Format::Docx).map_err(|e| e.to_string()),
                Out::Odt => apex_io::save_doc(&doc, &path, Format::Odt).map_err(|e| e.to_string()),
                Out::Pdf => crate::common::write_out(&path, crate::headless::pdf(apex_tui::Book::Doc(&doc), name)).map_err(|e| e.to_string()),
                Out::Eml => {
                    let to = rec.get(d.to_field).map_or("", String::as_str);
                    let subject = crate::cli::fill_pattern(&d.subject, k + 1, 1, &r.fields, rec);
                    let m = crate::mail::eml(to, &subject, &apex_io::html::doc_html(&doc), &doc.shared().to_plain_text());
                    crate::common::write_out(&path, m).map_err(|e| e.to_string())
                }
            };
            res.map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(names.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let fields = vec!["Name".to_string(), "City".to_string()];
        let recs = vec![vec!["Ana".to_string(), "Lugo".to_string()], vec!["ana".to_string(), "Vigo".to_string()], vec![String::new(), "x/y".to_string()]];
        assert_eq!(file_names("{Name}", &fields, &recs), ["Ana", "ana-2", "3"]);
        assert_eq!(file_names("{n}-{City}", &fields, &recs), ["1-Lugo", "2-Vigo", "3-x_y"]);
    }

    #[test]
    fn address_field() {
        let recs = vec![vec!["Ana".to_string(), "ana@example.org".to_string()]];
        assert_eq!(mail_field(&["Nombre".into(), "Correo".into()], &recs), 1);
        assert_eq!(mail_field(&["Name".into(), "To".into()], &recs), 1);
        assert_eq!(mail_field(&["Name".into()], &[]), 0);
    }
}
