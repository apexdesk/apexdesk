//! Notebooks (Cuaderno) in their own tab ([`crate::note_view`]): making a
//! new one, opening one, saving (every change is in the file already; Save
//! As writes a copy) and writing what changed in tabs not shown.

use apex_fs::PathExt;
use std::path::{Path, PathBuf};

use apex_io::notebook_store::NoteError;

use crate::i18n::{tr, trf};
use crate::note_view::fragments::Spot;
use crate::note_view::{self, NoteView};

use super::{App, Notice, View};

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `path` with the notebook's extension when it has none.
fn with_note_ext(mut path: PathBuf) -> PathBuf {
    if !note_view::is_note_file(&path) {
        path.as_mut_os_string().push(".");
        path.as_mut_os_string().push(apex_io::notebook_store::EXT);
    }
    path
}

/// What to tell when a notebook can't be opened.
fn open_error(path: &Path, e: &NoteError) -> Notice {
    match e {
        NoteError::Locked => Notice::error(trf("nb.locked", &[("file", &file_name(path))])),
        NoteError::NotNotebook => Notice::error(trf("nb.not_notebook", &[("file", &file_name(path))])),
        NoteError::NeedsPassword => Notice::error(tr("nb.lock_needed")),
        NoteError::Other(e) => Notice::error(trf("error.open", &[("file", &file_name(path)), ("error", e)])),
    }
}

impl App {
    /// File > New notebook: asks where, then opens it with a page.
    pub(super) fn new_notebook(&mut self) {
        let name = format!("{}.{}", tr("nb.default_name"), apex_io::notebook_store::EXT);
        crate::file_dialog::FileDialog::new().set_title(tr("nb.new_notebook")).add_filter(tr("nb.filter"), note_view::EXTS).set_file_name(name).save_file(|app, path| {
            let path = with_note_ext(path);
            match NoteView::create(&path) {
                Ok(v) => {
                    let canon = path.fs_canonicalize().unwrap_or(path);
                    app.push_tab(View::Note(Box::new(v)), Some(canon.clone()), None);
                    app.settings.add_recent(canon);
                    app.settings.save();
                }
                Err(e) => app.notify(open_error(&path, &e)),
            }
        });
    }

    /// Opens a notebook in a tab (`canon` is its full path).
    pub(super) fn open_note_tab(&mut self, canon: &Path) {
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(canon)) {
            self.active = i;
            return;
        }
        match NoteView::open(canon) {
            Ok(v) => {
                self.push_tab(View::Note(Box::new(v)), Some(canon.to_path_buf()), None);
                self.settings.add_recent(canon.to_path_buf());
                self.settings.save();
            }
            Err(e) => self.notify(open_error(canon, &e)),
        }
    }

    /// Save on a notebook tab: its changes are in the file already; Save
    /// As writes a copy.
    pub(super) fn save_note(&mut self, idx: usize, save_as: bool) -> bool {
        crate::file_dialog::set_owner(self.tabs[idx].id);
        if !save_as {
            if let View::Note(v) = &mut self.tabs[idx].view {
                v.flush();
            }
            self.notify(Notice::info(tr("nb.always_saved")));
            return true;
        }
        let stem = self.tabs[idx].path.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let id = self.tabs[idx].id;
        crate::file_dialog::FileDialog::new()
            .set_title(tr("nb.save_copy"))
            .add_filter(tr("nb.filter"), note_view::EXTS)
            .set_file_name(format!("{stem} ({}).{}", tr("nb.copy"), apex_io::notebook_store::EXT))
            .save_file(move |app, path| {
                let path = with_note_ext(path);
                let Some(View::Note(v)) = app.tab_view_mut(id) else { return };
                match v.copy_to(&path) {
                    Ok(()) => app.notify(Notice::info(trf("nb.copied", &[("file", &file_name(&path))]))),
                    Err(e) => app.notify(Notice::error(trf("nb.save_failed", &[("error", &e)]))),
                }
            });
        true
    }

    /// Notebooks in tabs not shown write what was typed in them.
    pub(super) fn flush_notes(&mut self) {
        let active = self.active;
        let mut notices = Vec::new();
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if i != active
                && let View::Note(v) = &mut t.view
            {
                v.flush();
                notices.append(&mut v.notices);
            }
        }
        for n in notices {
            self.notify(n);
        }
    }
}

/// The notebook an idea goes to.
enum Target {
    Tab(usize),
    File(PathBuf),
}

impl App {
    /// The notebook shown, else one open, else the last one used.
    fn capture_target(&self) -> Option<Target> {
        if matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Note(_))) {
            return Some(Target::Tab(self.active));
        }
        if let Some(i) = self.tabs.iter().position(|t| matches!(t.view, View::Note(_))) {
            return Some(Target::Tab(i));
        }
        self.settings.recent.iter().find(|p| note_view::is_note_file(p) && p.fs_exists()).cloned().map(Target::File)
    }

    /// Ctrl+Shift+I: the capture window, with the text selected in the
    /// tab shown and where it comes from.
    pub(super) fn open_capture(&mut self) {
        if self.capture.is_some() {
            return;
        }
        let picked = match self.tabs.get(self.active).map(|t| &t.view) {
            Some(View::Doc(v)) => v.selected_text(),
            Some(View::Pdf(v)) => v.selected_text().unwrap_or_default(),
            _ => String::new(),
        };
        let text = if picked.trim().is_empty() { String::new() } else { format!("{}\n\n— {}", picked.trim(), self.tabs[self.active].name()) };
        self.capture = Some(note_view::inbox::Capture::new(text));
    }

    /// The tab of the notebook `target`, opened aside when it isn't.
    fn target_tab(&mut self, target: Option<Target>) -> Option<usize> {
        match target? {
            Target::Tab(i) => Some(i),
            Target::File(p) => {
                // The tab shown stays shown.
                let shown = (!self.tabs.is_empty()).then_some(self.active);
                self.open_note_tab(&p);
                let i = self.tabs.iter().position(|t| t.path.as_deref() == Some(p.as_path()));
                if let Some(s) = shown {
                    self.active = s;
                }
                i
            }
        }
    }

    /// "Send to notebook" in a document or a PDF: the selection becomes a
    /// fragment under its source.
    fn fragment_requests(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let (text, spot) = match &mut tab.view {
            View::Doc(v) if v.fragment_wanted => {
                v.fragment_wanted = false;
                (v.selected_text(), Spot::Para(v.selection_para()))
            }
            View::Pdf(v) if v.fragment_wanted => {
                v.fragment_wanted = false;
                (v.selected_text().unwrap_or_default(), Spot::Page(v.sel.map_or(0, |s| s.page)))
            }
            _ => return,
        };
        if text.trim().is_empty() {
            return;
        }
        let Some(path) = tab.path.clone() else {
            self.notify(Notice::info(tr("nb.fragment_unsaved")));
            return;
        };
        let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| file_name(&path));
        let target = self.capture_target();
        let Some(i) = self.target_tab(target) else {
            self.notify(Notice::info(tr("nb.capture_nowhere")));
            return;
        };
        let name = self.tabs[i].name();
        if let View::Note(v) = &mut self.tabs[i].view {
            v.add_fragment(&text, &path, spot, &title);
            self.notify(Notice::info(trf("nb.fragment_sent", &[("notebook", &name)])));
        }
    }

    /// A link back to a fragment's original: opens it there.
    fn origin_requests(&mut self) {
        let Some(View::Note(v)) = self.tabs.get_mut(self.active).map(|t| &mut t.view) else { return };
        // An attachment copied out: here when ApexDesk opens it, else
        // with the system's program.
        if let Some(path) = v.open_file.take() {
            if apex_io::Format::from_path(&path).is_some() {
                self.open_path(&path);
            } else if let Err(e) = crate::links::follow(&path.to_string_lossy()) {
                self.notify(Notice::error(e));
            }
            return;
        }
        let Some((path, spot)) = v.open_request.take() else { return };
        // The path is the notebook's to choose: checked before it is
        // touched (looking for a file on a share already connects there).
        let target = path.to_string_lossy().into_owned();
        if let Some(why) = crate::links::in_app_refusal(&target, cfg!(windows)) {
            self.notify(Notice::error(format!("{}\n{target}\n{}", tr("links.blocked"), tr(why))));
            return;
        }
        if !path.fs_exists() {
            self.notify(Notice::info(trf("nb.origin_gone", &[("file", &file_name(&path))])));
            return;
        }
        self.open_path(&path);
        let canon = path.fs_canonicalize().unwrap_or(path);
        let Some(t) = self.tabs.get_mut(self.active).filter(|t| t.path.as_deref() == Some(canon.as_path())) else { return };
        match (&mut t.view, spot) {
            (View::Pdf(v), Spot::Page(p)) => v.go_to(p, 0.0),
            (View::Doc(v), Spot::Para(p)) => v.go_to_para(p),
            _ => {}
        }
    }

    pub(super) fn capture_ui(&mut self, ctx: &eframe::egui::Context) {
        self.fragment_requests();
        self.origin_requests();
        if let Some(View::Note(v)) = self.tabs.get_mut(self.active).map(|t| &mut t.view)
            && std::mem::take(&mut v.capture_wanted)
        {
            self.open_capture();
        }
        let Some(mut c) = self.capture.take() else { return };
        let target = self.capture_target();
        // The notebook, and the section shown when it is open.
        let place = match &target {
            Some(Target::Tab(i)) => {
                let name = self.tabs[*i].name();
                match &self.tabs[*i].view {
                    View::Note(v) => Some(v.capture_place().map_or(name.clone(), |s| format!("{name} › {s}"))),
                    _ => Some(name),
                }
            }
            Some(Target::File(p)) => Some(file_name(p)),
            None => None,
        };
        match c.ui(ctx, place.as_deref()) {
            note_view::inbox::Outcome::Open => self.capture = Some(c),
            note_view::inbox::Outcome::Cancel => {}
            note_view::inbox::Outcome::Keep(text) => {
                if let Some(i) = self.target_tab(target) {
                    let name = self.tabs[i].name();
                    if let View::Note(v) = &mut self.tabs[i].view {
                        v.capture(&text);
                        let place = v.capture_place().map_or(name.clone(), |s| format!("{name} › {s}"));
                        self.notify(Notice::info(trf("nb.captured", &[("place", &place)])));
                    }
                }
            }
        }
    }
}
