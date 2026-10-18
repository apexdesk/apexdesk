//! File > Tools > PDF Dossier: several files made one PDF with a cover, a
//! table of contents linking to each part, separator pages, bookmarks and
//! page numbers. Files that aren't PDFs are converted first; the work runs
//! on a thread and the dossier opens when done.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::atomic::{AtomicUsize, Ordering};

use apex_io::pdf::edit::dossier::{self, Dossier, Part};
use eframe::egui::{self, RichText};

use super::{App, Notice};
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::widgets::{self, DialogResult, Face};

pub(crate) fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// The file as PDF: a PDF as it is, anything else converted as `apexdesk
/// convert --to pdf` does, in `scratch` (emptied after).
pub(crate) fn pdf_of(path: &Path, scratch: &Path) -> Result<Vec<u8>, String> {
    if is_pdf(path) {
        return apex_fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    }
    let _ = apex_fs::remove_dir_all(scratch);
    let res = crate::cli::convert_file(path, "pdf", Some(scratch), false).and_then(|()| {
        let made = apex_fs::read_dir(scratch).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| is_pdf(p));
        let made = made.ok_or_else(|| format!("{}: no PDF made", path.display()))?;
        apex_fs::read(&made).map_err(|e| format!("{}: {e}", made.display()))
    });
    let _ = apex_fs::remove_dir_all(scratch);
    res
}

pub(super) struct DossierDlg {
    /// Each file and its title in the contents.
    files: Vec<(PathBuf, String)>,
    sel: Option<usize>,
    opts: Dossier,
    /// Where it is being made, until done: Ok(path) or the errors.
    job: Option<mpsc::Receiver<Result<(PathBuf, Vec<String>), String>>>,
    need: bool,
}

fn title_of(p: &std::path::Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default()
}

fn pick_files(then: impl FnOnce(&mut App, Vec<PathBuf>) + 'static) {
    crate::file_dialog::FileDialog::new()
        .set_title(tr("dossier.title"))
        .add_filter("PDF", &["pdf"])
        .add_filter(tr("dialog.filter_docs"), apex_io::DOC_EXTS)
        .add_filter(tr("dialog.filter_sheets"), apex_io::SHEET_EXTS)
        .add_filter(tr("dialog.filter_slides"), apex_io::SLIDES_EXTS)
        .pick_files(then);
}

/// Makes the dossier of `files` at `out`; the parts left out, as messages.
pub(crate) fn make(files: &[(PathBuf, String)], opts: &Dossier, out: &std::path::Path) -> Result<Vec<String>, String> {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let scratch = apex_io::safe_path::private_temp_dir("apexdesk-dossier", RUNS.fetch_add(1, Ordering::Relaxed)).map_err(|e| e.to_string())?;
    let mut errors = Vec::new();
    let mut parts = Vec::new();
    for (i, (f, title)) in files.iter().enumerate() {
        match pdf_of(f, &scratch.join(i.to_string())) {
            Ok(data) => parts.push(Part { title: if title.trim().is_empty() { title_of(f) } else { title.trim().to_string() }, data }),
            Err(e) => errors.push(e),
        }
    }
    let _ = apex_fs::remove_dir_all(&scratch);
    let (bytes, failed) = dossier::build(&parts, opts);
    for (i, e) in failed {
        errors.push(format!("{}: {e}", parts[i].title));
    }
    crate::common::write_out(out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(errors)
}

impl App {
    pub(super) fn open_dossier(&mut self) {
        pick_files(|app, files| {
            let title = files.first().and_then(|f| f.parent()).and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            app.dossier_dlg = Some(DossierDlg {
                files: files.into_iter().map(|f| (f.clone(), title_of(&f))).collect(),
                sel: None,
                opts: Dossier { title, ..Dossier::default() },
                job: None,
                need: false,
            });
        });
    }

    pub(super) fn dossier_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dossier_dlg.take() else { return };
        if let Some(rx) = &d.job
            && let Ok(res) = rx.try_recv()
        {
            match res {
                Ok((path, errors)) => {
                    if !errors.is_empty() {
                        self.notify(Notice::error(format!("{}\n{}", trf("batch.failed", &[("n", &errors.len())]), errors.join("\n"))));
                    }
                    self.open_path(&path);
                }
                Err(e) => self.notify(Notice::error(e)),
            }
            return;
        }
        let busy = d.job.is_some();
        let res = widgets::dialog(ctx, "dossier", tr("dossier.title"), 480.0, |ui| {
            ui.add_enabled_ui(!busy, |ui| form(ui, &mut d));
            if busy {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("dossier.working"));
                });
            }
            if d.need {
                ui.colored_label(ui.visuals().error_fg_color, tr("dossier.need"));
            }
        });
        match res {
            DialogResult::None => self.dossier_dlg = Some(d),
            DialogResult::Cancel | DialogResult::Ok if busy => self.dossier_dlg = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                if d.files.is_empty() {
                    d.need = true;
                    self.dossier_dlg = Some(d);
                    return;
                }
                let name = if d.opts.title.trim().is_empty() { tr("dossier.title").to_string() } else { crate::cli::file_safe(d.opts.title.trim()) };
                let dir = d.files[0].0.parent().map(PathBuf::from);
                self.dossier_dlg = Some(d);
                let mut fd = crate::file_dialog::FileDialog::new().set_title(tr("dossier.title")).add_filter("PDF", &["pdf"]).set_file_name(format!("{name}.pdf"));
                if let Some(dir) = dir {
                    fd = fd.set_directory(dir);
                }
                fd.save_file(|app, path| {
                    let path = if path.extension().is_none() { path.with_extension("pdf") } else { path };
                    let Some(d) = &mut app.dossier_dlg else { return };
                    let (tx, rx) = mpsc::channel();
                    let (files, opts, ctx) = (d.files.clone(), d.opts.clone(), app.ctx.clone());
                    apex_fs::thread::spawn(move || {
                        let _ = tx.send(make(&files, &opts, &path).map(|e| (path, e)));
                        ctx.request_repaint();
                    });
                    d.job = Some(rx);
                });
            }
        }
    }
}

fn form(ui: &mut egui::Ui, d: &mut DossierDlg) {
    widgets::form(ui, "dossier_form", |ui| {
        widgets::form_row(ui, tr("dossier.doc_title"), |ui| ui.add(egui::TextEdit::singleline(&mut d.opts.title).desired_width(300.0)));
        widgets::form_row(ui, tr("dossier.subtitle"), |ui| ui.add(egui::TextEdit::singleline(&mut d.opts.subtitle).desired_width(300.0).hint_text(tr("dossier.subtitle_hint"))));
    });
    widgets::section(ui, tr("dossier.parts"));
    ui.label(RichText::new(tr("dossier.parts_hint")).weak());
    egui::ScrollArea::vertical().id_salt("dossier_files").max_height(180.0).show(ui, |ui| {
        for (i, (f, title)) in d.files.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                if ui.selectable_label(d.sel == Some(i), format!("{}.", i + 1)).on_hover_text(f.display().to_string()).clicked() {
                    d.sel = Some(i);
                }
                if ui.add(egui::TextEdit::singleline(title).desired_width(260.0)).on_hover_text(f.display().to_string()).gained_focus() {
                    d.sel = Some(i);
                }
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                ui.label(RichText::new(name).weak());
            });
        }
    });
    ui.horizontal(|ui| {
        if ui.button(format!("{}…", tr("batch.add"))).clicked() {
            pick_files(|app, files| {
                if let Some(d) = &mut app.dossier_dlg {
                    for f in files {
                        if !d.files.iter().any(|(p, _)| *p == f) {
                            let t = title_of(&f);
                            d.files.push((f, t));
                        }
                    }
                }
            });
        }
        let sel = d.sel.filter(|&i| i < d.files.len());
        if ui.add_enabled(sel.is_some(), egui::Button::new(tr("batch.remove"))).clicked()
            && let Some(i) = sel
        {
            d.files.remove(i);
            d.sel = (!d.files.is_empty()).then(|| i.min(d.files.len() - 1));
        }
        let up = sel.filter(|&i| i > 0);
        if ui.add_enabled_ui(up.is_some(), |ui| widgets::tool_button(ui, Face::Icon(Icon::ChevronUp), false, tr("sort.move_up"))).inner.clicked()
            && let Some(i) = up
        {
            d.files.swap(i, i - 1);
            d.sel = Some(i - 1);
        }
        let down = sel.filter(|&i| i + 1 < d.files.len());
        if ui.add_enabled_ui(down.is_some(), |ui| widgets::tool_button(ui, Face::Icon(Icon::ChevronDown), false, tr("sort.move_down"))).inner.clicked()
            && let Some(i) = down
        {
            d.files.swap(i, i + 1);
            d.sel = Some(i + 1);
        }
    });
    widgets::section(ui, tr("dossier.include"));
    ui.checkbox(&mut d.opts.cover, tr("dossier.cover"));
    ui.checkbox(&mut d.opts.index, tr("dossier.index"));
    ui.checkbox(&mut d.opts.separators, tr("dossier.separators"));
    ui.checkbox(&mut d.opts.numbers, tr("dossier.numbers"));
    ui.label(RichText::new(tr("batch.to_pdf")).weak());
}
