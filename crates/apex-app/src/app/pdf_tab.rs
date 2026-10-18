//! PDF files in their own tab ([`crate::pdf_view`]): opening them, saving
//! them (as an update added to the file when it can, which keeps earlier
//! signatures valid) and what the tab asks for.

use apex_fs::PathExt;
use std::path::{Path, PathBuf};

use crate::i18n::{tr, trf};
use crate::pdf_view::{PdfView, Request};

use super::{App, Notice, View, locks};

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    /// File > Open PDF: asks for the file.
    pub(super) fn open_pdf_view(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("pdf.open_title")).add_filter("PDF", &["pdf"]).pick_file(|app, path| app.open_path(&path));
    }

    /// Opens a PDF in a PDF tab (`canon` is its full path).
    pub(super) fn open_pdf_tab(&mut self, canon: &Path, password: Option<String>, mode: locks::OpenMode) {
        let data = match apex_fs::read(canon) {
            Ok(d) => d,
            Err(e) => {
                self.notify(Notice::error(trf("error.open", &[("file", &file_name(canon)), ("error", &e.to_string())])));
                return;
            }
        };
        match PdfView::open(data, password.clone()) {
            Ok(v) => {
                let signed = super::signatures_ui::read_signed(canon, password.as_deref(), apex_io::Format::Pdf);
                if mode == locks::OpenMode::Copy {
                    self.push_tab(View::Pdf(Box::new(v)), None, None);
                    self.opened_copy(canon);
                } else {
                    self.push_tab(View::Pdf(Box::new(v)), Some(canon.to_path_buf()), Some(apex_io::Format::Pdf));
                    if let Some(t) = self.tabs.last_mut() {
                        t.password = password;
                    }
                    self.opened(canon, mode);
                }
                if self.should_protect(canon) {
                    self.protect_last();
                }
                if let (Some(s), Some(id)) = (signed, self.tabs.last().map(|t| t.id)) {
                    self.sig.note(id, s);
                }
                if !crate::pdf_view::engine::available() && !crate::pdf_view::warned_simple() {
                    self.notify(Notice::info(tr("pdf.no_pdfium")));
                }
                self.settings.add_recent(canon.to_path_buf());
                self.settings.save();
            }
            Err(e) if e.needs_password() || e.wrong_password() => {
                self.keep_open_mode(canon, mode);
                self.ask_password(canon.to_path_buf(), None, e.wrong_password());
            }
            Err(e) => self.notify(Notice::error(trf("error.open", &[("file", &file_name(canon)), ("error", &e)]))),
        }
    }

    /// Saves PDF tab `idx` (asking where for a new file or `save_as`);
    /// `then` learns whether it was.
    pub(super) fn save_pdf(&mut self, idx: usize, save_as: bool, then: impl FnOnce(&mut App, bool) + 'static) {
        let Some(tab) = self.tabs.get(idx) else { return then(self, false) };
        let read_only = tab.view.read_only();
        match (&tab.path, save_as || read_only) {
            (Some(p), false) => {
                let p = p.clone();
                let ok = self.save_pdf_to(idx, p);
                then(self, ok);
            }
            _ => self.ask_pdf_path(idx, move |app, target| {
                let ok = target.is_some_and(|(idx, path)| app.save_pdf_to(idx, path));
                then(app, ok);
            }),
        }
    }

    /// Writes PDF tab `idx` at `path`.
    fn save_pdf_to(&mut self, idx: usize, path: PathBuf) -> bool {
        if !self.may_overwrite(idx, &path) {
            return false;
        }
        // A downloaded file stays marked as downloaded in its copies.
        let downloaded = self.tabs[idx].downloaded();
        let View::Pdf(v) = &mut self.tabs[idx].view else { return false };
        // The file read with the changes added (another file gets the
        // same), which is what the next save adds to.
        match v.doc.save_to(|b| apex_io::write_atomic(&path, b)) {
            Ok(()) => {
                if downloaded {
                    apex_io::safe_path::mark_untrusted(&path);
                }
                let t = &mut self.tabs[idx];
                t.path = Some(path.clone());
                t.format = Some(apex_io::Format::Pdf);
                self.share_path(idx);
                self.saved_file(idx, &path);
                self.settings.add_recent(path);
                self.settings.save();
                self.notify(Notice::info(tr("notice.saved")));
                true
            }
            Err(e) => {
                self.notify(Notice::error(trf("error.save", &[("error", &e.to_string())])));
                false
            }
        }
    }

    /// Asks where to save a PDF tab; `then` gets where the tab is now and
    /// the path (None: cancelled, or the tab has closed).
    fn ask_pdf_path(&self, idx: usize, then: impl FnOnce(&mut App, Option<(usize, PathBuf)>) + 'static) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let stem = match &tab.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tab.untitled.clone(),
        };
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("dialog.save_title")).set_file_name(format!("{stem}.pdf")).add_filter("PDF", &["pdf"]);
        if let Some(dir) = tab.path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let id = tab.id;
        dlg.save_file_or_cancel(move |app, path| {
            let target = path.and_then(|mut path| {
                if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("pdf")) {
                    path.as_mut_os_string().push(".pdf");
                }
                Some((app.tab_index(id)?, path))
            });
            then(app, target);
        });
    }

    /// File > Export as PDF on a PDF tab: a copy of the file as it is now.
    pub(super) fn export_pdf_copy(&mut self, idx: usize) {
        self.ask_pdf_path(idx, |app, target| {
            let Some((idx, path)) = target else { return };
            let downloaded = app.tabs[idx].downloaded();
            let View::Pdf(v) = &mut app.tabs[idx].view else { return };
            let bytes = v.doc.to_bytes();
            match crate::common::write_out(&path, bytes) {
                Ok(()) => {
                    if downloaded {
                        apex_io::safe_path::mark_untrusted(&path);
                    }
                    app.notify(Notice::info(trf("print.exported", &[("file", &file_name(&path))])));
                }
                Err(e) => app.notify(Notice::error(trf("error.save", &[("error", &e.to_string())]))),
            }
        });
    }

    /// What PDF tabs asked for this frame.
    pub(super) fn pdf_requests(&mut self) {
        let mut todo: Vec<(usize, Request)> = Vec::new();
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if let View::Pdf(v) = &mut t.view {
                todo.extend(v.requests.drain(..).map(|r| (i, r)));
            }
        }
        for (idx, r) in todo {
            match r {
                Request::Convert => self.convert_pdf(idx),
                Request::Printed(Ok(printer)) => self.notify(Notice::info(trf("print.sent", &[("printer", &printer)]))),
                Request::Printed(Err(e)) => self.notify(Notice::error(trf("print.failed", &[("error", &e)]))),
                Request::Sign(field) => {
                    self.active = idx;
                    match self.sign_doc_blocked() {
                        Some(why) => self.notify(Notice::error(why)),
                        None => self.open_sign_field(field),
                    }
                }
                Request::Signatures => {
                    self.active = idx;
                    self.open_signatures();
                }
                Request::Extract(pages) => self.extract_pdf_pages(idx, pages),
                Request::Split(ranges) => self.split_pdf(idx, ranges),
                Request::InsertFile(at) => self.insert_pdf_files(idx, at),
                Request::Compare => self.compare_pdf(idx),
                Request::ExportImages(pages, dpi) => self.export_pdf_images(idx, pages, dpi),
                Request::SaveAs => {
                    self.save_pdf(idx, true, |_, _| {});
                }
            }
        }
    }

    /// The name a PDF tab's file goes by, without extension.
    fn pdf_stem(&self, idx: usize) -> String {
        let Some(tab) = self.tabs.get(idx) else { return String::new() };
        match &tab.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tab.untitled.clone(),
        }
    }

    /// Saves pages of tab `idx` as a new file, where the user says.
    fn extract_pdf_pages(&mut self, idx: usize, pages: Vec<usize>) {
        let stem = self.pdf_stem(idx);
        let first = pages.first().map_or(1, |p| p + 1);
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("pdf.extract_pages")).set_file_name(format!("{stem}_{first}.pdf")).add_filter("PDF", &["pdf"]);
        if let Some(dir) = self.tabs[idx].path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let id = self.tabs[idx].id;
        dlg.save_file(move |app, mut path| {
            if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("pdf")) {
                path.as_mut_os_string().push(".pdf");
            }
            let Some(View::Pdf(v)) = app.tab_view_mut(id) else { return };
            match crate::common::write_out(&path, v.doc.extract(&pages)) {
                Ok(()) => app.notify(Notice::info(trf("print.exported", &[("file", &file_name(&path))]))),
                Err(e) => app.notify(Notice::error(trf("error.save", &[("error", &e.to_string())]))),
            }
        });
    }

    /// Writes each range of pages of tab `idx` as a file in a folder the
    /// user picks (numbered, and named by the bookmark it starts at).
    fn split_pdf(&mut self, idx: usize, ranges: Vec<(String, usize, usize)>) {
        let stem = self.pdf_stem(idx);
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("pdf.split_folder"));
        if let Some(dir) = self.tabs[idx].path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let id = self.tabs[idx].id;
        dlg.pick_folder(move |app, dir| {
            let Some(View::Pdf(v)) = app.tab_view_mut(id) else { return };
            let files = v.doc.split(&ranges.iter().map(|r| (r.1, r.2)).collect::<Vec<_>>());
            let mut written = 0;
            for (k, (bytes, r)) in files.iter().zip(&ranges).enumerate() {
                let label: String = r.0.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(60).collect::<String>().trim().to_string();
                let base = if label.is_empty() { format!("{stem}_{}", k + 1) } else { format!("{stem}_{} {label}", k + 1) };
                let path = split_target(&dir, &base);
                if let Err(e) = crate::common::write_out(&path, bytes) {
                    app.notify(Notice::error(trf("error.save", &[("error", &e.to_string())])));
                    return;
                }
                written += 1;
            }
            app.notify(Notice::info(trf("pdf.split_done", &[("n", &written)])));
        });
    }

    /// Adds the pages of PDF files the user picks to tab `idx`.
    fn insert_pdf_files(&mut self, idx: usize, at: usize) {
        let id = self.tabs[idx].id;
        crate::file_dialog::FileDialog::new().set_title(tr("pdf.insert_file")).add_filter("PDF", &["pdf"]).pick_files(move |app, paths| {
            let mut files = Vec::new();
            for p in &paths {
                match apex_fs::read(p) {
                    Ok(d) => files.push((file_name(p), d)),
                    Err(e) => {
                        app.notify(Notice::error(trf("error.open", &[("file", &file_name(p)), ("error", &e.to_string())])));
                        return;
                    }
                }
            }
            let Some(View::Pdf(v)) = app.tab_view_mut(id) else { return };
            match v.insert_files(files, at) {
                Ok(n) => app.notify(Notice::info(trf("pdf.inserted", &[("n", &n)]))),
                Err(e) => app.notify(Notice::error(trf("pdf.insert_failed", &[("error", &e)]))),
            }
        });
    }

    /// Compares tab `idx` with another version the user picks.
    fn compare_pdf(&mut self, idx: usize) {
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("pdf.compare_pick")).add_filter("PDF", &["pdf"]);
        if let Some(dir) = self.tabs[idx].path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let id = self.tabs[idx].id;
        dlg.pick_file(move |app, path| {
            let data = match apex_fs::read(&path) {
                Ok(d) => d,
                Err(e) => {
                    app.notify(Notice::error(trf("error.open", &[("file", &file_name(&path)), ("error", &e.to_string())])));
                    return;
                }
            };
            let Some(idx) = app.tab_index(id) else { return };
            let pw = app.tabs[idx].password.clone();
            let View::Pdf(v) = &mut app.tabs[idx].view else { return };
            // The other version may share the password of this one.
            if let Err(e) = v.start_compare(file_name(&path), &data, pw.as_deref()) {
                app.notify(Notice::error(trf("error.open", &[("file", &file_name(&path)), ("error", &e)])));
            }
        });
    }

    /// Saves pages of tab `idx` as PNG pictures in a folder the user picks.
    fn export_pdf_images(&mut self, idx: usize, pages: Vec<usize>, dpi: u32) {
        let stem = self.pdf_stem(idx);
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("pdf.export_images"));
        if let Some(dir) = self.tabs[idx].path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let id = self.tabs[idx].id;
        dlg.pick_folder(move |app, dir| {
            let Some(View::Pdf(v)) = app.tab_view_mut(id) else { return };
            match v.export_images(&pages, dpi, |k| dir.join(format!("{stem}_{}.png", k + 1))) {
                Ok(n) => app.notify(Notice::info(trf("pdf.exported_images", &[("n", &n)]))),
                Err(e) => app.notify(Notice::error(trf("error.save", &[("error", &e)]))),
            }
        });
    }

    /// Opens the PDF of tab `idx` as an editable document.
    fn convert_pdf(&mut self, idx: usize) {
        let Some(t) = self.tabs.get_mut(idx) else { return };
        let pw = t.password.clone();
        let View::Pdf(v) = &mut t.view else { return };
        let bytes = v.bytes();
        match apex_io::pdf::read(&bytes, pw.as_deref()) {
            Ok(doc) => {
                self.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(doc))), None, None);
                self.notify(Notice::info(tr("notice.pdf_opened")));
            }
            Err(e) if e.pdf_scanned() => {
                let name = self.tabs[idx].name();
                self.notify(Notice::error(trf("error.pdf_scanned", &[("file", &name)])));
            }
            Err(e) => {
                let name = self.tabs[idx].name();
                self.notify(Notice::error(trf("error.open", &[("file", &name), ("error", &e)])));
            }
        }
    }
}

/// `dir/base.pdf`, or with a number added when that file is there.
fn split_target(dir: &Path, base: &str) -> PathBuf {
    let mut p = dir.join(format!("{base}.pdf"));
    let mut k = 2;
    while p.fs_exists() && k < 1000 {
        p = dir.join(format!("{base} ({k}).pdf"));
        k += 1;
    }
    p
}
