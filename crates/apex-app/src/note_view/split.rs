//! Two pages at once: a second page beside the open one. Clicking into
//! it makes it the open page (the header, the toolstrip and the related
//! pages are its), and the other one goes beside; neither moves.

use super::*;

/// The page beside the open one.
pub(super) struct Side {
    pub page: i64,
    pub pane: Pane,
}

/// What the page beside is: text in its editor, or a canvas.
pub(super) enum Pane {
    Text { editor: Box<DocView>, saved_rev: u64, changed_at: Option<f64> },
    Canvas(Box<board::Board>),
}

impl Side {
    /// Its editor, when it is a text page.
    pub fn editor_mut(&mut self) -> Option<&mut DocView> {
        match &mut self.pane {
            Pane::Text { editor, .. } => Some(editor),
            Pane::Canvas(_) => None,
        }
    }
}

impl NoteView {
    /// Shows page `id` beside the open one.
    pub fn open_beside(&mut self, id: i64) {
        if self.page == Some(id) || self.side.as_ref().is_some_and(|s| s.page == id) {
            return;
        }
        let Some(kind) = self.nb.page(id).map(|p| p.kind) else { return };
        self.flush_side();
        let pane = if kind == apex_doc::notebook::PageKind::Canvas {
            match self.store.load_canvas(id) {
                Ok(c) => Pane::Canvas(Box::new(board::Board::new(id, c))),
                Err(e) => return self.report(Err(e)),
            }
        } else {
            let doc = match self.store.load_page(id) {
                Ok(d) => self.with_embeds(id, d),
                Err(e) => return self.report(Err(e)),
            };
            let ed = DocView::embedded(doc);
            Pane::Text { saved_rev: ed.doc.revision, editor: Box::new(ed), changed_at: None }
        };
        self.side = Some(Side { page: id, pane });
        self.close_graph();
        self.grid = false;
    }

    pub fn close_side(&mut self) {
        self.flush_side();
        self.side = None;
    }

    /// Writes the page beside, when it changed.
    pub(super) fn flush_side(&mut self) {
        let Some(mut s) = self.side.take() else { return };
        match &mut s.pane {
            Pane::Canvas(b) => self.save_board(b),
            Pane::Text { editor, saved_rev, changed_at } => {
                if editor.doc.revision != *saved_rev {
                    let (id, doc) = (s.page, &editor.doc);
                    let text = plain_text(doc);
                    let found = apex_doc::notebook::links::page_links(doc);
                    let tasks = apex_doc::notebook::tasks::tasks_in(doc);
                    let t = now();
                    let r = self.store.save_page(id, doc, &text, &found, t);
                    *saved_rev = editor.doc.revision;
                    *changed_at = None;
                    self.nb.texts.insert(id, text);
                    self.nb.links.insert(id, found);
                    self.nb.tasks.insert(id, tasks);
                    if let Some(p) = self.nb.page_mut(id) {
                        p.modified = t;
                    }
                    self.report(r);
                    self.publish_cards();
                }
            }
        }
        self.side = Some(s);
    }

    /// The page beside becomes the open one, and the open one goes
    /// beside.
    pub(super) fn swap_sides(&mut self) {
        if self.side.is_none() || (self.editor.is_none() && self.board.is_none()) {
            return;
        }
        self.commit_title();
        self.commit_aliases();
        self.flush();
        self.flush_side();
        let (Some(s), Some(page)) = (&mut self.side, self.page) else { return };
        let main = match (self.editor.take(), self.board.take()) {
            (Some(editor), _) => Pane::Text { saved_rev: editor.doc.revision, editor, changed_at: None },
            (None, Some(mut b)) => {
                b.rest();
                Pane::Canvas(b)
            }
            (None, None) => return,
        };
        match std::mem::replace(&mut s.pane, main) {
            Pane::Text { editor, .. } => {
                self.saved_rev = editor.doc.revision;
                self.editor = Some(editor);
            }
            Pane::Canvas(b) => self.board = Some(b),
        }
        self.changed_at = None;
        self.page = Some(s.page);
        s.page = page;
        self.links_rev = 0;
        self.side_left = !self.side_left;
        if let Some(p) = self.page.and_then(|p| self.nb.page(p)) {
            self.section = Some(p.section);
            self.title.clone_from(&p.title);
            self.aliases = p.aliases.join(", ");
        }
    }

    /// The page beside, written a moment after typing stops.
    pub(super) fn side_autosave(&mut self, ui: &egui::Ui, time: f64) {
        let Some(Side { pane: Pane::Text { editor, saved_rev, changed_at }, .. }) = &mut self.side else { return };
        self.notices.append(&mut editor.notices);
        if editor.doc.revision == *saved_rev {
            return;
        }
        let at = *changed_at.get_or_insert(time);
        if time - at >= SAVE_AFTER {
            self.flush_side();
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(SAVE_AFTER));
        }
    }

    /// The bar over a pane: its page's title, and a close button. Whether
    /// that was clicked, and where the bar is.
    fn pane_bar(&self, ui: &mut egui::Ui, page: i64, active: bool) -> (bool, egui::Rect) {
        let title = self.nb.page(page).map_or_else(String::new, |p| if p.title.trim().is_empty() { tr("nb.untitled").to_string() } else { p.title.clone() });
        let mut close = false;
        let r = egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let text = RichText::new(title).small();
                ui.label(if active { text.strong() } else { text.weak() });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.add(egui::Button::new(RichText::new("×").small()).frame(false)).on_hover_text(tr("nb.split_close")).clicked();
                });
            });
        });
        if active {
            let rect = r.response.rect;
            ui.painter().hline(rect.x_range(), rect.max.y - 1.0, egui::Stroke::new(2.0, theme::NOTE_ACCENT));
        }
        (close, r.response.rect)
    }

    /// The open page and the one beside, side by side.
    pub(super) fn split_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let full = ui.available_rect_before_wrap();
        let gap = 6.0;
        let half = ((full.width() - gap) / 2.0).max(0.0);
        let left = egui::Rect::from_min_size(full.min, vec2(half, full.height()));
        let right = egui::Rect::from_min_max(egui::pos2(left.max.x + gap, full.min.y), full.max);
        // A click in the page beside makes it the open one before anything
        // takes the click: only the open page answers the pointer and keys.
        let side_rect = if self.side_left { left } else { right };
        // Not on its bar: its close button is for it, not for the page
        // that would take its place.
        let bar_id = egui::Id::new(("side_bar", self.uid));
        let bar = ui.ctx().data(|d| d.get_temp::<egui::Rect>(bar_id)).unwrap_or(egui::Rect::NOTHING);
        if ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| side_rect.contains(p) && !bar.contains(p))) {
            self.swap_sides();
        }
        let (main_rect, side_rect) = if self.side_left { (right, left) } else { (left, right) };
        ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(left.max.x, full.min.y), egui::pos2(right.min.x, full.max.y)), 0.0, ui.visuals().panel_fill);
        let (Some(page), Some(side_page)) = (self.page, self.side.as_ref().map(|s| s.page)) else { return };
        let mut close_main = false;
        let mut close_side = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(main_rect), |ui| {
            close_main = self.pane_bar(ui, page, true).0;
            if let Some(ed) = &mut self.editor {
                ed.ui(ui, shell);
            } else {
                self.board_ui(ui);
            }
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(side_rect), |ui| {
            let (close, bar) = self.pane_bar(ui, side_page, false);
            close_side = close;
            ui.ctx().data_mut(|d| d.insert_temp(bar_id, bar));
            match self.side.take() {
                Some(Side { page, pane: Pane::Text { mut editor, saved_rev, changed_at } }) => {
                    editor.ui(ui, shell);
                    self.side = Some(Side { page, pane: Pane::Text { editor, saved_rev, changed_at } });
                }
                // Drawn as the open canvas for a moment; it gets no
                // clicks (they swap it in first), so it only shows.
                Some(Side { page, pane: Pane::Canvas(b) }) => {
                    let main = self.board.replace(b);
                    self.board_ui(ui);
                    let b = std::mem::replace(&mut self.board, main);
                    self.side = b.map(|b| Side { page, pane: Pane::Canvas(b) });
                }
                None => {}
            }
        });
        ui.allocate_rect(full, Sense::hover());
        if let Some(ed) = self.side.as_mut().and_then(Side::editor_mut) {
            // A page link followed beside opens there.
            ed.link_prompt = None;
            let follow = ed.note_follow.take();
            let hover = ed.hover_link.clone();
            // The keyboard stays with the open page.
            let id = ed.canvas_id();
            if ui.ctx().memory(|m| m.has_focus(id)) {
                match &self.editor {
                    Some(main) => ui.ctx().memory_mut(|m| m.request_focus(main.canvas_id())),
                    None => ui.ctx().memory_mut(|m| m.surrender_focus(id)),
                }
            }
            if let Some((id, _)) = follow.as_deref().and_then(apex_doc::notebook::links::parse) {
                self.open_beside(id);
            } else if let Some(t) = hover
                && self.finder.is_none()
                && let Some((id, rel)) = apex_doc::notebook::links::parse(&t)
            {
                self.preview(ui.ctx(), id, rel);
            }
        }
        if close_side {
            self.close_side();
        } else if close_main {
            self.swap_sides();
            self.close_side();
        }
    }
}
