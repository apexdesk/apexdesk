//! What a file tab does besides showing its document: renaming it in
//! place (its file on disk too) and taking it to a window of its own,
//! from its menu or dragged out of the tab bar.

use apex_fs::PathExt;
use std::path::Path;

use apex_io::Format;
use eframe::egui::{self, Key, Modifiers, text::CCursor, text::CCursorRange};

use super::{App, Notice, View};
use crate::i18n::{tr, trf};

/// How far below the tab bar a dragged tab goes to a new window.
pub(super) const TEAR_OFF: f32 = 60.0;

fn field_id() -> egui::Id {
    egui::Id::new("tab_rename")
}

/// The name field of a tab being renamed: Some(Some(name)) when Enter
/// confirms it, Some(None) when cancelled (Escape, or a click elsewhere).
pub(super) fn rename_field(ui: &mut egui::Ui, text: &mut String) -> Option<Option<String>> {
    let id = field_id();
    let width = ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.clone(), egui::TextStyle::Body.resolve(ui.style()), egui::Color32::WHITE).size().x);
    let resp = ui.add(egui::TextEdit::singleline(text).id(id).desired_width((width + 24.0).clamp(80.0, 320.0)).margin(egui::Margin::symmetric(2, 0)));
    let start = id.with("start");
    if ui.ctx().data_mut(|d| d.remove_temp::<bool>(start)).is_some() {
        resp.request_focus();
        return None;
    }
    if resp.lost_focus() {
        // The key is used up here, not by the document under the bar.
        let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        return Some(enter.then(|| text.clone()));
    }
    (!resp.has_focus()).then_some(None)
}

/// A tab dragged out of the bar, under the pointer at `pos`.
pub(super) fn paint_torn_tab(ui: &egui::Ui, pos: egui::Pos2, name: &str) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("torn_tab")));
    let color = ui.visuals().strong_text_color();
    let galley = painter.layout_no_wrap(name.to_string(), egui::TextStyle::Body.resolve(ui.style()), color);
    let rect = egui::Rect::from_min_size(pos + egui::vec2(12.0, 8.0), galley.size() + egui::vec2(16.0, 8.0));
    painter.rect(rect, 6.0, ui.visuals().extreme_bg_color, egui::Stroke::new(1.0, ui.visuals().selection.stroke.color), egui::StrokeKind::Inside);
    painter.galley(rect.min + egui::vec2(8.0, 4.0), galley, color);
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
}

/// Whether `name` can't be a file name.
fn bad_name(name: &str) -> bool {
    let reserved: &[char] = if cfg!(windows) { &['/', '\\', ':', '*', '?', '"', '<', '>', '|'] } else { &['/', '\\'] };
    name == "." || name == ".." || name.contains(reserved) || name.chars().any(char::is_control)
}

impl App {
    /// Whether tab `i` can be renamed (a database or notebook keeps its
    /// file open; an embedded object has no name of its own).
    pub(super) fn can_rename(&self, i: usize) -> bool {
        self.tabs.get(i).is_some_and(|t| t.embed.is_none() && !matches!(t.view, View::Db(_) | View::Note(_)))
    }

    /// Starts renaming tab `i`: its name, the part before the extension selected.
    pub(super) fn start_rename(&mut self, ctx: &egui::Context, i: usize) {
        if !self.can_rename(i) {
            return;
        }
        let t = &self.tabs[i];
        let (text, stem) = match &t.path {
            Some(p) => {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let stem = p.file_stem().map_or(0, |s| s.to_string_lossy().chars().count());
                (name, stem)
            }
            None => (t.untitled.clone(), t.untitled.chars().count()),
        };
        let id = field_id();
        let mut state = egui::text_edit::TextEditState::default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(stem))));
        state.store(ctx, id);
        ctx.data_mut(|d| d.insert_temp(id.with("start"), true));
        self.active = i;
        self.tab_rename = Some((t.id, text));
    }

    /// Renames tab `i` to `name`: an unsaved one gets the name Save
    /// offers; a saved one's file is renamed in its folder (keeping its
    /// extension when left out).
    pub(super) fn rename_tab(&mut self, i: usize, name: &str) {
        let name = name.trim();
        let Some(t) = self.tabs.get(i) else { return };
        if name.is_empty() || name == t.name() {
            return;
        }
        if bad_name(name) {
            self.notify(Notice::error(tr("tab.rename_bad")));
            return;
        }
        let group = t.group();
        let Some(old) = t.path.clone() else {
            // An extension of its kind is left to Save, which adds it.
            let p = Path::new(name);
            let stem = Format::from_path(p).filter(|f| f.kind() == t.view.kind()).and_then(|_| p.file_stem()).map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned());
            let id = t.id;
            for t in self.tabs.iter_mut().filter(|x| x.id == id || group.is_some() && x.group() == group) {
                t.untitled.clone_from(&stem);
            }
            self.copy_renamed(i);
            return;
        };
        let file = match old.extension().map(|e| e.to_string_lossy().into_owned()) {
            Some(ext) if !name.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) => format!("{name}.{ext}"),
            _ => name.to_string(),
        };
        let new = old.with_file_name(&file);
        if new == old {
            return;
        }
        // Another file of that name (but the same one in other letter case,
        // on a disk that doesn't tell them apart).
        let same = new.fs_canonicalize().ok().is_some_and(|c| old.fs_canonicalize().ok() == Some(c));
        if new.fs_exists() && !same {
            self.notify(Notice::error(trf("tab.rename_exists", &[("file", &file)])));
            return;
        }
        if self.locked_by_other(&old) {
            let cur = old.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.notify(Notice::error(trf("tab.rename_in_use", &[("file", &cur)])));
            return;
        }
        if let Err(e) = apex_fs::rename(&old, &new) {
            self.notify(Notice::error(trf("tab.rename_failed", &[("error", &e)])));
            return;
        }
        for t in self.tabs.iter_mut().filter(|t| t.path.as_deref() == Some(old.as_path())) {
            t.path = Some(new.clone());
            if let View::Doc(v) = &mut t.view {
                v.source_saved_as(&new);
            }
        }
        self.renamed_file(&old, &new);
        self.copy_renamed(i);
        let canon = new.fs_canonicalize().unwrap_or_else(|_| new.clone());
        for p in self.settings.recent.iter_mut().filter(|p| **p == old) {
            p.clone_from(&canon);
        }
        self.settings.save();
    }

    /// Whether tab `i` can go to a window of its own: an embedded object
    /// goes back to its container, and the windows of a workbook stay
    /// together.
    pub(super) fn can_move_out(&self, i: usize) -> bool {
        let Some(t) = self.tabs.get(i) else { return false };
        if self.edits_object(i) || self.has_other_window(i) {
            return false;
        }
        match t.view {
            View::Db(_) | View::Note(_) => t.path.is_some(),
            _ => true,
        }
    }

    /// Takes tab `idx` to a window of its own: closes it here and opens it
    /// in a new ApexDesk (which doesn't hand it back to this one). A tab
    /// with changes, or never saved, goes as a copy of what it holds now.
    pub(super) fn open_in_new_window(&mut self, idx: usize) {
        if !self.can_move_out(idx) {
            return;
        }
        let t = &self.tabs[idx];
        let path = t.path.clone();
        // A database or notebook writes its changes to its file at once.
        let copied = (path.is_none() || self.tab_modified(idx)) && !matches!(t.view, View::Db(_) | View::Note(_));
        let handoff = if copied {
            let Some(info) = self.hand_off(idx) else {
                self.notify(Notice::error(tr("tab.new_window_failed")));
                return;
            };
            Some(info)
        } else {
            None
        };
        // Closed first, its lock with it: the new window opens the file as
        // its own (and a notebook is open in one place only).
        self.remove_tab(idx);
        self.sync_locks();
        let spawned = std::env::current_exe().and_then(|exe| {
            let mut cmd = std::process::Command::new(exe);
            cmd.env(crate::instance::NEW_WINDOW, "1").env_remove(super::recovery::HANDOFF);
            match (&handoff, &path) {
                (Some(info), _) => cmd.env(super::recovery::HANDOFF, info),
                (None, Some(p)) => cmd.arg(p),
                (None, None) => &mut cmd,
            };
            cmd.spawn()
        });
        if let Err(e) = spawned {
            self.notify(Notice::error(format!("{}: {e}", tr("tab.new_window"))));
            match (handoff, path) {
                (Some(info), _) => self.adopt_handoff(&info),
                (None, Some(p)) => self.open_path(&p),
                (None, None) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use apex_fs::PathExt;
    use web_time::Instant;

    use eframe::egui;

    use super::super::{App, View};
    use crate::doc_view::DocView;

    fn app() -> App {
        let ctx = egui::Context::default();
        App::blank(&ctx, crate::settings::Settings::default(), None, Instant::now())
    }

    /// An unsaved tab takes the name Save will offer, without the
    /// extension of its kind.
    #[test]
    fn renaming_an_unsaved_tab_names_it() {
        let mut app = app();
        app.push_tab(View::Doc(Box::new(DocView::new(apex_doc::Document::new()))), None, None);
        app.rename_tab(0, " Informe.docx ");
        assert_eq!(app.tabs[0].untitled, "Informe");
        app.rename_tab(0, "a/b");
        assert_eq!(app.tabs[0].untitled, "Informe");
    }

    /// A tab with changes goes to a new window as a copy, which opens
    /// there as the same changed, unsaved tab and is then removed.
    #[test]
    fn a_handed_off_tab_keeps_its_changes() {
        let mut app = app();
        let mut doc = apex_doc::Document::new();
        doc.modified = true;
        app.push_tab(View::Doc(Box::new(DocView::new(doc))), None, None);
        app.tabs[0].untitled = "Notas".into();
        let info = app.hand_off(0).expect("copy written");
        app.remove_tab(0);
        let mut other = self::app();
        other.adopt_handoff(&info);
        assert_eq!(other.tabs.len(), 1);
        assert_eq!(other.tabs[0].untitled, "Notas");
        assert!(other.tabs[0].path.is_none() && other.tab_modified(0));
        assert!(!info.fs_exists());
    }

    /// A saved tab renames its file, keeping the extension, and won't
    /// take the name of another file.
    #[test]
    fn renaming_a_saved_tab_renames_its_file() {
        let dir = apex_fs::temp_dir().join(format!("apexdesk-rename-{}", apex_fs::process_id()));
        let _ = apex_fs::create_dir_all(&dir);
        let dir = dir.fs_canonicalize().unwrap();
        let old = dir.join("a.docx");
        apex_fs::write(&old, b"x").unwrap();
        apex_fs::write(dir.join("taken.docx"), b"y").unwrap();
        let mut app = app();
        app.settings.recent = vec![old.clone()];
        app.push_tab(View::Doc(Box::new(DocView::new(apex_doc::Document::new()))), Some(old.clone()), Some(apex_io::Format::Docx));
        app.rename_tab(0, "b");
        let new = dir.join("b.docx");
        assert!(new.fs_exists() && !old.fs_exists());
        assert_eq!(app.tabs[0].path.as_deref(), Some(new.as_path()));
        assert_eq!(app.settings.recent, [new.clone()]);
        app.rename_tab(0, "taken.docx");
        assert!(new.fs_exists());
        assert_eq!(app.tabs[0].path.as_deref(), Some(new.as_path()));
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
