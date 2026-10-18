//! File > Send by E-mail: the file (or a PDF of it) attached to a new
//! message of the system's mail program (see `crate::mail`). The program
//! is called on a thread; if none takes the file, a notice says so and the
//! file is shown in its folder.

use std::path::PathBuf;
use std::sync::mpsc;

use super::{App, Notice, View};
use crate::i18n::{tr, trf};

/// The result of a message being composed, with the file it attaches.
pub(super) type MailResult = (PathBuf, Result<(), String>);

impl App {
    /// Attaches the file of tab `idx` (saved as it is on screen) or, with
    /// `pdf`, a PDF of it.
    pub(super) fn send_by_mail(&mut self, ctx: &eframe::egui::Context, idx: usize, pdf: bool) {
        if !self.share_checked(idx, super::check_ui::Then::Mail { pdf }) {
            return;
        }
        let Some(tab) = self.tabs.get(idx) else { return };
        let name = crate::cli::file_safe(&tab.name());
        let stem = std::path::Path::new(&name).file_stem().map_or_else(|| name.clone(), |s| s.to_string_lossy().into_owned());
        let file = match self.mail_file(idx, pdf) {
            Ok(f) => f,
            Err(e) => {
                self.notify(Notice::error(trf("error.save", &[("error", &e)])));
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let subject = stem.clone();
        let ctx = ctx.clone();
        apex_fs::thread::spawn(move || {
            let res = crate::mail::compose(std::slice::from_ref(&file), &subject);
            let _ = tx.send((file, res));
            ctx.request_repaint();
        });
        self.mail_rx = Some(rx);
    }

    /// Checks on the message being composed.
    pub(super) fn poll_mail(&mut self) {
        let Some(rx) = &self.mail_rx else { return };
        match rx.try_recv() {
            Ok((file, res)) => {
                self.mail_rx = None;
                if res.is_err() {
                    self.notify(Notice::info(tr("mail.no_client")));
                    crate::mail::reveal(&file);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => self.mail_rx = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// The file to attach for tab `idx`: the saved file when it is
    /// unchanged, else a copy as it is on screen (or, with `pdf`, a PDF of
    /// it) in a temporary folder. A database attaches the file itself, or
    /// as a PDF the report of its open object (what printing it gives); a
    /// PDF attaches the PDF as it is edited.
    pub(super) fn mail_file(&mut self, idx: usize, pdf: bool) -> Result<PathBuf, String> {
        let tab = self.tabs.get_mut(idx).ok_or_else(String::new)?;
        let name = crate::cli::file_safe(&tab.name());
        let stem = std::path::Path::new(&name).file_stem().map_or_else(|| name.clone(), |s| s.to_string_lossy().into_owned());
        // A folder of this user only: a shared one in /tmp could be made
        // beforehand by someone else, to read the copy or swap it.
        let dir = apex_io::safe_path::private_temp_dir("apexdesk-mail", idx).map_err(|e| e.to_string())?;
        // A PDF's "PDF" is itself.
        let as_is = !pdf || matches!(tab.view, View::Pdf(_));
        if as_is
            && !tab.view.modified()
            && let Some(p) = &tab.path
        {
            // The saved file, as it is.
            return Ok(p.clone());
        }
        let fmt = tab.format.filter(|f| f.can_save()).unwrap_or(match tab.view.kind() {
            apex_io::Kind::Sheet => apex_io::Format::Xlsx,
            apex_io::Kind::Doc => apex_io::Format::Docx,
            apex_io::Kind::Slides => apex_io::Format::Pptx,
        });
        let ext = match &tab.view {
            _ if pdf => "pdf",
            View::Pdf(_) => "pdf",
            View::Db(v) => v.path.extension().and_then(|e| e.to_str()).unwrap_or("apexdb"),
            View::Note(_) => apex_io::notebook_store::EXT,
            _ => fmt.extension(),
        }
        .to_string();
        let path = dir.join(format!("{stem}.{ext}"));
        // A copy of an encrypted file is encrypted too.
        let pw = tab.password.as_deref();
        match (&mut tab.view, pdf) {
            (View::Doc(v), true) => {
                v.update_for_output();
                apex_fs::write(&path, crate::headless::pdf(apex_tui::Book::Doc(&v.doc), &stem)).map_err(|e| e.to_string())
            }
            (View::Sheet(v), true) => {
                v.commit_edit();
                apex_fs::write(&path, crate::headless::pdf(apex_tui::Book::Sheet(&v.wb), &stem)).map_err(|e| e.to_string())
            }
            (View::Doc(v), false) => {
                v.update_for_output();
                apex_io::save_doc_with(&v.doc, &path, fmt, pw).map_err(|e| e.to_string())
            }
            (View::Sheet(v), false) => {
                v.commit_edit();
                apex_io::save_sheet_with(&v.wb, &path, fmt, pw).map_err(|e| e.to_string())
            }
            (View::Slides(v), true) => {
                v.commit_edit();
                apex_fs::write(&path, crate::headless::slides_pdf(&v.pres.deck, &stem, crate::print::Scope::ActiveSheet)).map_err(|e| e.to_string())
            }
            (View::Slides(v), false) => {
                v.commit_edit();
                apex_io::save_slides_with(&v.pres.deck, &path, fmt, pw).map_err(|e| e.to_string())
            }
            (View::Pdf(v), _) => apex_fs::write(&path, v.bytes()).map_err(|e| e.to_string()),
            (View::Db(v), true) => apex_fs::write(&path, crate::headless::pdf(apex_tui::Book::Doc(&v.print_doc()), &stem)).map_err(|e| e.to_string()),
            (View::Db(v), false) => v.db.backup_to(&path).map_err(|e| e.to_string()),
            (View::Note(v), true) => apex_fs::write(&path, crate::headless::pdf(apex_tui::Book::Doc(&v.print_doc()), &stem)).map_err(|e| e.to_string()),
            (View::Note(v), false) => v.copy_to(&path),
        }?;
        Ok(path)
    }
}
