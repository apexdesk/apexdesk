//! What `apexdesk convert` does, in the window: File > Convert Files
//! (several files to another format at once), and a workbook's sheet as a
//! Markdown table or plain text, or every sheet as CSV.

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use super::{App, Notice, View};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

pub(super) struct ConvertDlg {
    files: Vec<PathBuf>,
    /// Index into `targets()`.
    to: usize,
    /// None: next to each file.
    folder: Option<PathBuf>,
    all_sheets: bool,
}

/// Every format files can be converted to, sheets' first.
fn targets() -> Vec<&'static str> {
    let mut v: Vec<&str> = crate::cli::SHEET_TARGETS.to_vec();
    for t in crate::cli::DOC_TARGETS.iter().chain(crate::cli::SLIDES_TARGETS) {
        if !v.contains(t) {
            v.push(t);
        }
    }
    v
}

impl App {
    /// File > Convert Files: asks for the files first.
    pub(super) fn open_convert(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("convert.title")).add_filter(tr("dialog.filter_sheets"), apex_io::SHEET_EXTS).add_filter(tr("dialog.filter_docs"), apex_io::DOC_EXTS).add_filter(tr("dialog.filter_slides"), apex_io::SLIDES_EXTS).pick_files(|app, files| {
            let to = targets().iter().position(|t| *t == "pdf").unwrap_or(0);
            app.convert_dlg = Some(ConvertDlg { files, to, folder: None, all_sheets: false });
        });
    }

    pub(super) fn convert_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.convert_dlg.take() else { return };
        let list = targets();
        let res = widgets::dialog(ctx, "convert_files", tr("convert.title"), 420.0, |ui| {
            ui.label(trf("convert.files", &[("n", &d.files.len())]));
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for f in &d.files {
                    ui.label(RichText::new(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).weak());
                }
            });
            ui.add_space(6.0);
            widgets::form(ui, "convert_form", |ui| {
                widgets::form_row(ui, tr("convert.to"), |ui| {
                    egui::ComboBox::from_id_salt("convert_to").selected_text(list[d.to].to_uppercase()).show_ui(ui, |ui| {
                        for (i, t) in list.iter().enumerate() {
                            ui.selectable_value(&mut d.to, i, t.to_uppercase());
                        }
                    })
                    .response
                });
                widgets::form_row(ui, tr("convert.where"), |ui| {
                    ui.horizontal(|ui| {
                        let shown = d.folder.as_ref().map_or_else(|| tr("convert.next_to").to_string(), |f| f.display().to_string());
                        ui.label(shown);
                        if ui.button(format!("{}…", tr("convert.choose"))).clicked() {
                            crate::file_dialog::FileDialog::new().set_title(tr("convert.where")).pick_folder(|app, f| {
                                if let Some(d) = &mut app.convert_dlg {
                                    d.folder = Some(f);
                                }
                            });
                        }
                    })
                    .response
                });
            });
            let text = matches!(list[d.to], "csv" | "tsv" | "md" | "txt");
            ui.add_enabled_ui(text, |ui| ui.checkbox(&mut d.all_sheets, tr("convert.all_sheets")));
        });
        match res {
            DialogResult::None => self.convert_dlg = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let to = list[d.to];
                let mut done = 0;
                for f in &d.files {
                    match crate::cli::convert_file(f, to, d.folder.as_deref(), d.all_sheets) {
                        Ok(()) => done += 1,
                        Err(e) => self.notify(Notice::error(e)),
                    }
                }
                if done > 0 {
                    self.notify(Notice::info(trf("convert.done", &[("n", &done), ("format", &to.to_uppercase())])));
                }
            }
        }
    }

    /// File > Export: the active sheet as a Markdown table ("md") or text
    /// in columns ("txt").
    pub(super) fn export_sheet_text(&mut self, idx: usize, to: &str) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let View::Sheet(_) = &tab.view else { return };
        let stem = tab.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| tab.untitled.clone(), |s| s.to_string_lossy().into_owned());
        let (key, filter) = if to == "md" { ("export.markdown", "Markdown") } else { ("export.text", "Text") };
        let (id, to) = (tab.id, to.to_string());
        crate::file_dialog::FileDialog::new().set_title(tr(key)).set_file_name(format!("{stem}.{to}")).add_filter(filter, &[&to]).save_file(move |app, path| {
            let Some(View::Sheet(v)) = app.tab_view_mut(id) else { return };
            let text = crate::cli::sheet_text(&v.wb, v.sheet_index(), &to);
            if let Err(e) = crate::common::write_out(&path, text) {
                app.notify(Notice::error(trf("error.save", &[("error", &format!("{}: {e}", path.display()))])));
            }
        });
    }

    /// File > Export: every sheet as a CSV file in a folder.
    pub(super) fn export_sheets_csv(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let View::Sheet(_) = &tab.view else { return };
        let stem = tab.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| tab.untitled.clone(), |s| s.to_string_lossy().into_owned());
        let id = tab.id;
        crate::file_dialog::FileDialog::new().set_title(tr("export.all_csv")).pick_folder(move |app, dir| {
            let Some(View::Sheet(v)) = app.tab_view_mut(id) else { return };
            let mut err = None;
            for (i, s) in v.wb.sheets.iter().enumerate() {
                let path = dir.join(format!("{stem}-{}.csv", crate::cli::file_safe(&s.name)));
                if let Err(e) = crate::common::write_out(&path, crate::cli::sheet_text(&v.wb, i, "csv")) {
                    err = Some(trf("error.save", &[("error", &format!("{}: {e}", path.display()))]));
                    break;
                }
            }
            let n = v.wb.sheets.len();
            match err {
                Some(e) => app.notify(Notice::error(e)),
                None => app.notify(Notice::info(trf("convert.done", &[("n", &n), ("format", &"CSV")]))),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn convert_files() {
        let dir = apex_fs::temp_dir().join(format!("apex-convert-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let csv = dir.join("a.csv");
        apex_fs::write(&csv, "Name,Qty\nPen,3\n").unwrap();
        crate::cli::convert_file(&csv, "md", None, false).unwrap();
        let md = apex_fs::read_to_string(dir.join("a.md")).unwrap();
        assert!(md.contains("| Name") && md.contains("Pen"), "{md}");
        let out = dir.join("out");
        crate::cli::convert_file(&csv, "xlsx", Some(&out), false).unwrap();
        assert!(out.join("a.xlsx").exists());
        assert!(crate::cli::convert_file(&csv, "epub", None, false).is_err());
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
