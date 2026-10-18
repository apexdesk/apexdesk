//! A page's history: its earlier texts, kept as it's written, to look at
//! and to bring back.

use apex_io::notebook_store::Version;

use super::*;
use crate::widgets::FitWindow as _;

pub(super) struct HistoryWindow {
    page: i64,
    versions: Vec<Version>,
    /// The version shown (None: the page as it is).
    sel: Option<i64>,
    /// Its text.
    preview: String,
}

impl NoteView {
    /// Shows page `id`'s history.
    pub fn open_history(&mut self, id: i64) {
        self.flush();
        let versions = match self.store.versions(id) {
            Ok(v) => v,
            Err(e) => return self.report(Err(e)),
        };
        let mut w = HistoryWindow { page: id, versions, sel: None, preview: String::new() };
        let first = w.versions.first().map(|v| v.id);
        self.show_version(&mut w, first);
        self.close_windows();
        self.history = Some(w);
    }

    fn show_version(&mut self, w: &mut HistoryWindow, sel: Option<i64>) {
        w.sel = sel;
        let doc = match sel {
            Some(v) => self.store.load_version(v),
            None => self.store.load_page(w.page),
        };
        w.preview = match doc {
            Ok(d) => plain_text(&d),
            Err(e) => e.to_string(),
        };
    }

    /// Puts version `version` back as page `page`'s text; the text it had
    /// is kept as a version.
    pub fn restore_version(&mut self, page: i64, version: i64) {
        self.flush();
        let doc = match self.store.keep_version(page).and_then(|()| self.store.load_version(version)) {
            Ok(d) => d,
            Err(e) => return self.report(Err(e)),
        };
        let text = plain_text(&doc);
        let found = apex_doc::notebook::links::page_links(&doc);
        let t = now();
        let r = self.store.save_page(page, &doc, &text, &found, t);
        self.report(r);
        self.nb.texts.insert(page, text);
        self.nb.links.insert(page, found);
        self.nb.tasks.insert(page, apex_doc::notebook::tasks::tasks_in(&doc));
        if let Some(p) = self.nb.page_mut(page) {
            p.modified = t;
        }
        if self.page == Some(page) {
            // Opened again with the text brought back.
            self.page = None;
            self.editor = None;
            self.open_page(page);
        }
        self.notices.push(Notice::info(tr("nb.version_restored")));
    }

    pub(super) fn history_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.history.take() else { return };
        let Some(title) = self.nb.page(w.page).map(|p| p.title.clone()) else { return };
        let mut open = true;
        let mut pick = None;
        let mut restore = false;
        crate::widgets::esc_closes(ctx, Id::new(("note_history", self.uid)), &mut open);
        egui::Window::new(trf("nb.history_of", &[("page", &title)]))
            .id(Id::new(("note_history", self.uid)))
            .open(&mut open)
            .collapsible(false)
            .default_size(vec2(680.0, 420.0))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show_fit(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(170.0);
                        egui::ScrollArea::vertical().id_salt("versions").max_height(360.0).show(ui, |ui| {
                            if ui.selectable_label(w.sel.is_none(), RichText::new(tr("nb.version_now")).strong()).clicked() {
                                pick = Some(None);
                            }
                            for v in &w.versions {
                                if ui.selectable_label(w.sel == Some(v.id), date_text(v.at)).clicked() {
                                    pick = Some(Some(v.id));
                                }
                            }
                            if w.versions.is_empty() {
                                ui.add_space(6.0);
                                ui.label(RichText::new(tr("nb.versions_none")).weak().small());
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(4.0).inner_margin(10).show(ui, |ui| {
                            egui::ScrollArea::vertical().id_salt("version_text").max_height(320.0).auto_shrink([false, false]).show(ui, |ui| {
                                ui.label(&w.preview);
                            });
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.add_enabled(w.sel.is_some(), egui::Button::new(tr("nb.version_restore"))).on_hover_text(tr("nb.version_restore_tip")).clicked() {
                                restore = true;
                            }
                        });
                    });
                });
            });
        if let Some(sel) = pick {
            self.show_version(&mut w, sel);
        }
        if restore && let Some(v) = w.sel {
            self.restore_version(w.page, v);
            match self.store.versions(w.page) {
                Ok(list) => w.versions = list,
                Err(e) => self.report(Err(e)),
            }
            self.show_version(&mut w, None);
        }
        if open {
            self.history = Some(w);
        }
    }
}
