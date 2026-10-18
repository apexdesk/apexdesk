//! Text encodings of CSV and text files: File > Open with Encoding (the
//! encoding detected, another one to pick, and a preview of the first
//! lines in it) and File > Save with Encoding. A text file remembers the
//! encoding it was read in, and is saved in it again.

use apex_fs::PathExt;
use std::path::PathBuf;

use apex_io::{Encoding, Format};
use eframe::egui::{self, RichText};

use super::{App, Notice, View};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

/// Bytes of the file the preview shows (and detection looks at).
const HEAD: u64 = 64 << 10;

/// Lines in the preview.
const LINES: usize = 14;

pub(super) enum EncodingDlg {
    Open { path: PathBuf, fmt: Format, head: Vec<u8>, enc: Encoding, detected: Encoding, delim: Option<u8> },
    Save { tab: u64, enc: Encoding },
}

/// Delimiters offered for CSV (None: detect).
const DELIMS: [(Option<u8>, &str); 5] = [(None, "getdata.delim.auto"), (Some(b','), "getdata.delim.comma"), (Some(b';'), "getdata.delim.semicolon"), (Some(b'\t'), "getdata.delim.tab"), (Some(b'|'), "getdata.delim.pipe")];

/// An encoding's name in the list, "(detected)" after the one detected.
fn enc_label(e: Encoding, detected: Option<Encoding>) -> String {
    let name = match e {
        Encoding::Utf8Bom => tr("enc.utf8_bom").to_string(),
        Encoding::Cp1252 => tr("enc.cp1252").to_string(),
        Encoding::Cp850 => tr("enc.cp850").to_string(),
        e => e.name().to_string(),
    };
    if detected == Some(e) { trf("enc.detected", &[("name", &name)]) } else { name }
}

fn enc_combo(ui: &mut egui::Ui, id: &str, enc: &mut Encoding, detected: Option<Encoding>) {
    egui::ComboBox::from_id_salt(id).width(240.0).selected_text(enc_label(*enc, detected)).show_ui(ui, |ui| {
        for e in Encoding::ALL {
            ui.selectable_value(enc, e, enc_label(e, detected));
        }
    });
}

/// The first lines of `head` in `enc`.
pub(crate) fn preview(head: &[u8], enc: Encoding) -> String {
    let text = enc.decode(head);
    text.lines().take(LINES).map(|l| l.chars().take(200).collect::<String>()).collect::<Vec<_>>().join("\n")
}

impl App {
    /// File > Open with Encoding: asks for a text file first.
    pub(super) fn open_with_encoding(&mut self) {
        let exts = ["csv", "tsv", "txt", "text", "log", "md", "markdown"];
        crate::file_dialog::FileDialog::new().set_title(tr("enc.open")).add_filter(tr("enc.filter"), &exts).pick_file(|app, path| app.open_encoding_dialog(path));
    }

    /// Shows the text file `path` in the encoding it seems to have.
    fn open_encoding_dialog(&mut self, path: std::path::PathBuf) {
        let fmt = Format::from_path(&path).filter(|f| apex_io::is_text_format(*f)).unwrap_or(Format::Text);
        let head = {
            use std::io::Read;
            let mut head = Vec::new();
            match apex_fs::File::open(&path).and_then(|f| f.take(HEAD).read_to_end(&mut head)) {
                Ok(_) => head,
                Err(e) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.notify(Notice::error(trf("error.open", &[("file", &name), ("error", &e)])));
                    return;
                }
            }
        };
        let detected = apex_io::encoding::detect(&head);
        self.encoding_dlg = Some(EncodingDlg::Open { path, fmt, head, enc: detected, detected, delim: None });
    }

    /// File > Save with Encoding: the active tab's encoding, then Save As.
    pub(super) fn save_with_encoding(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let enc = t.encoding.unwrap_or(Encoding::Utf8);
        self.encoding_dlg = Some(EncodingDlg::Save { tab: t.id, enc });
    }

    pub(super) fn encoding_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.encoding_dlg.take() else { return };
        let title = match &d {
            EncodingDlg::Open { .. } => tr("enc.open"),
            EncodingDlg::Save { .. } => tr("enc.save"),
        };
        let res = widgets::dialog(ctx, "text_encoding", title, 560.0, |ui| match &mut d {
            EncodingDlg::Open { path, fmt, head, enc, detected, delim } => {
                ui.label(RichText::new(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).strong());
                ui.add_space(4.0);
                widgets::form(ui, "enc_open_form", |ui| {
                    widgets::form_row(ui, tr("enc.encoding"), |ui| enc_combo(ui, "enc_open", enc, Some(*detected)));
                    if matches!(fmt, Format::Csv | Format::Tsv) {
                        widgets::form_row(ui, tr("getdata.delimiter"), |ui| {
                            let shown = DELIMS.iter().find(|(x, _)| *x == *delim).map_or("", |(_, k)| tr(k));
                            egui::ComboBox::from_id_salt("enc_delim").selected_text(shown).show_ui(ui, |ui| {
                                for (x, k) in DELIMS {
                                    ui.selectable_value(delim, x, tr(k));
                                }
                            });
                        });
                    }
                });
                ui.add_space(6.0);
                ui.label(RichText::new(tr("enc.preview")).small().weak());
                let mut text = preview(head, *enc);
                egui::ScrollArea::both().max_height(220.0).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).interactive(false));
                });
            }
            EncodingDlg::Save { enc, .. } => {
                ui.label(RichText::new(tr("enc.save_note")).weak().small());
                ui.add_space(6.0);
                widgets::form(ui, "enc_save_form", |ui| {
                    widgets::form_row(ui, tr("enc.encoding"), |ui| enc_combo(ui, "enc_save", enc, None));
                });
            }
        });
        match res {
            DialogResult::None => self.encoding_dlg = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => match d {
                EncodingDlg::Open { path, fmt, enc, delim, .. } => self.open_text_as(path, fmt, enc, delim),
                EncodingDlg::Save { tab, enc } => {
                    let Some(idx) = self.tabs.iter().position(|t| t.id == tab) else { return };
                    self.tabs[idx].encoding = Some(enc);
                    self.save(idx, true);
                }
            },
        }
    }

    /// Opens a text file read in `enc` (in the tab that shows it, if it
    /// has no changes).
    fn open_text_as(&mut self, path: PathBuf, fmt: Format, enc: Encoding, delim: Option<u8>) {
        let canon = path.fs_canonicalize().unwrap_or(path);
        let name = canon.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let open = self.tabs.iter().position(|t| t.path.as_deref() == Some(canon.as_path()));
        if let Some(i) = open.filter(|&i| self.tabs[i].view.modified()) {
            self.active = i;
            self.notify(Notice::error(tr("enc.reopen_modified")));
            return;
        }
        match apex_io::open_text(&canon, fmt, Some(enc), delim) {
            Ok((loaded, enc)) => {
                let truncated = matches!(&loaded, apex_io::Loaded::Doc(d) if d.truncated);
                let view = super::locks::view_of(loaded);
                match open {
                    Some(i) => {
                        self.tabs[i].view = view;
                        self.tabs[i].encoding = Some(enc);
                        self.active = i;
                    }
                    None => {
                        self.push_tab(view, Some(canon.clone()), Some(fmt));
                        if let Some(t) = self.tabs.last_mut() {
                            t.encoding = Some(enc);
                        }
                        self.opened(&canon, super::locks::OpenMode::Normal);
                    }
                }
                self.note_source(self.active, &canon, true);
                self.settings.add_recent(canon.clone());
                self.settings.save();
                if truncated {
                    self.opened_in_part(&canon);
                } else {
                    self.notify(Notice::info(trf("enc.opened", &[("file", &name), ("name", &enc_label(enc, None))])));
                }
            }
            Err(e) => self.notify(Notice::error(trf("error.open", &[("file", &name), ("error", &e)]))),
        }
    }

    /// Saves tab `idx` as text in its encoding; the characters it has no
    /// room for are reported.
    pub(super) fn save_text_encoded(&mut self, idx: usize, path: &std::path::Path, fmt: Format, enc: Encoding) -> Result<(), String> {
        let tab = &mut self.tabs[idx];
        let lost = match &mut tab.view {
            View::Sheet(v) => {
                v.commit_edit();
                let n = apex_io::save_text(apex_io::TextSource::Sheet(&v.wb), path, fmt, enc).map_err(|e| e.to_string())?;
                v.wb.modified = false;
                n
            }
            View::Doc(v) => {
                let n = if let Some(text) = v.md_source_for(fmt) {
                    apex_io::save_text(apex_io::TextSource::Raw(&text), path, fmt, enc).map_err(|e| e.to_string())?
                } else {
                    v.end_hf_edit();
                    v.update_for_output();
                    apex_io::save_text(apex_io::TextSource::Doc(&v.doc), path, fmt, enc).map_err(|e| e.to_string())?
                };
                v.doc.modified = false;
                v.md_saved(fmt);
                n
            }
            View::Slides(_) => return Err("not a text format".into()),
            View::Pdf(_) | View::Db(_) | View::Note(_) => return Err("not a text format".into()),
        };
        if lost > 0 {
            self.notify(Notice::info(trf("enc.lost", &[("n", &lost), ("name", &enc_label(enc, None))])));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_in_each_encoding() {
        let (bytes, _) = Encoding::Cp850.encode("Nombre;Año\nJosé;1984\n");
        assert_eq!(apex_io::encoding::detect(&bytes), Encoding::Cp850);
        assert_eq!(preview(&bytes, Encoding::Cp850), "Nombre;Año\nJosé;1984");
        assert_ne!(preview(&bytes, Encoding::Cp1252), "Nombre;Año\nJosé;1984");
        let many: String = (0..40).map(|i| format!("line {i}\n")).collect();
        assert_eq!(preview(many.as_bytes(), Encoding::Utf8).lines().count(), LINES);
    }
}
