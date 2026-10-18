//! Database files in their own tab ([`crate::db_view`]): making a new one,
//! opening one (Access and dBase files are imported into a new one first),
//! saving (changes are already in the file; Save As writes a copy) and
//! what the tab asks for.

use apex_fs::PathExt;
use std::path::{Path, PathBuf};

use crate::db_view::{self, DbView, Request};
use crate::i18n::{tr, trf};

use super::{App, Notice, View};

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `path` with extension `apexdb` when it has none of the database's.
fn with_db_ext(mut path: PathBuf) -> PathBuf {
    if !db_view::is_db_file(&path) {
        path.as_mut_os_string().push(".apexdb");
    }
    path
}

impl App {
    /// File > New database: asks where, then opens it empty.
    // Android leaves the database tab out.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub(super) fn new_database(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("db.new_database")).add_filter(tr("db.filter_db"), db_view::EXTS).set_file_name(format!("{}.apexdb", tr("db.default_name"))).save_file(|app, path| {
            let path = with_db_ext(path);
            match db_view::create(&path) {
                Ok(v) => {
                    let canon = path.fs_canonicalize().unwrap_or(path);
                    app.push_tab(View::Db(Box::new(v)), Some(canon.clone()), None);
                    app.settings.add_recent(canon);
                    app.settings.save();
                }
                Err(e) => app.notify(Notice::error(trf("error.open", &[("file", &file_name(&path)), ("error", &e.to_string())]))),
            }
        });
    }

    /// File > Open database: SQLite files open; Access and dBase files are
    /// imported into a new database next to them.
    // Android leaves the database tab out.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub(super) fn open_database(&mut self) {
        let exts: Vec<&str> = db_view::EXTS.iter().chain(apex_db::ACCESS_EXTS).chain(apex_db::DBASE_EXTS).copied().collect();
        crate::file_dialog::FileDialog::new().set_title(tr("db.open_database")).add_filter(tr("dialog.filter_databases"), &exts).pick_file(|app, path| app.open_database_file(path));
    }

    /// Opens a database file: SQLite opens, Access and dBase are imported
    /// (asking where the new database goes).
    pub(super) fn open_database_file(&mut self, path: PathBuf) {
        if db_view::is_db_file(&path) {
            self.open_db_tab(&path.fs_canonicalize().unwrap_or(path));
            return;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("db.import_as")).add_filter(tr("db.filter_db"), db_view::EXTS).set_file_name(format!("{stem}.apexdb"));
        if let Some(dir) = path.parent() {
            dlg = dlg.set_directory(dir);
        }
        // A second dialog, once the first has answered.
        dlg.save_file(move |app, dst| app.import_database(&path, &with_db_ext(dst)));
    }

    /// Imports an Access or dBase file into a new database at `dst` and opens it.
    pub(super) fn import_database(&mut self, src: &Path, dst: &Path) {
        match apex_db::import::import_file(src, dst) {
            Ok(rep) => {
                self.notify(Notice::info(trf("db.imported_file", &[("tables", &rep.tables), ("rows", &rep.rows), ("relations", &rep.relations), ("queries", &rep.queries)])));
                if !rep.untranslated.is_empty() {
                    self.notify(Notice::info(trf("db.untranslated_list", &[("names", &rep.untranslated.join(", "))])));
                }
                let l = rep.left_out;
                if l.any() {
                    self.notify(Notice::info(trf("db.left_out", &[("forms", &l.forms), ("reports", &l.reports), ("macros", &l.macros), ("modules", &l.modules)])));
                }
                for w in rep.warnings.iter().take(3) {
                    self.notify(Notice::info(w.clone()));
                }
                self.open_db_tab(&dst.fs_canonicalize().unwrap_or_else(|_| dst.to_path_buf()));
            }
            Err(e) => self.notify(Notice::error(trf("error.open", &[("file", &file_name(src)), ("error", &e.to_string())]))),
        }
    }

    /// Opens a database file in a tab (`canon` is its full path).
    pub(super) fn open_db_tab(&mut self, canon: &Path) {
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(canon)) {
            self.active = i;
            return;
        }
        match DbView::open(canon) {
            Ok(v) => {
                self.push_tab(View::Db(Box::new(v)), Some(canon.to_path_buf()), None);
                self.settings.add_recent(canon.to_path_buf());
                self.settings.save();
            }
            Err(e) => self.notify(Notice::error(trf("error.open", &[("file", &file_name(canon)), ("error", &e.to_string())]))),
        }
    }

    /// Save on a database tab: its changes are in the file already; Save
    /// As writes a copy.
    pub(super) fn save_db(&mut self, idx: usize, save_as: bool) -> bool {
        crate::file_dialog::set_owner(self.tabs[idx].id);
        let View::Db(v) = &mut self.tabs[idx].view else { return false };
        if save_as {
            v.backup();
        } else {
            self.notify(Notice::info(tr("db.always_saved")));
        }
        true
    }

    /// What database tabs asked for this frame.
    pub(super) fn db_requests(&mut self) {
        let mut todo: Vec<Request> = Vec::new();
        for t in &mut self.tabs {
            if let View::Db(v) = &mut t.view {
                todo.append(&mut v.requests);
            }
        }
        for r in todo {
            match r {
                Request::OpenDoc(doc) => self.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(*doc))), None, None),
                Request::OpenWorkbook(wb) => self.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(*wb))), None, None),
                Request::OpenDb(p) => self.open_db_tab(&p),
            }
        }
    }
}
