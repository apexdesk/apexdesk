//! Sections under a password: their pages' texts are sealed in the file,
//! and out of sight (in the lists, search, links and the book's cards)
//! until the password is given. It is asked again when the notebook opens.

use super::*;
use crate::widgets::FitWindow as _;

pub(super) struct LockDialog {
    section: i64,
    /// Putting a password (true) or giving it.
    set: bool,
    pw: String,
    pw2: String,
    error: Option<&'static str>,
    focus: bool,
}

impl NoteView {
    /// Whether section `id` has a password.
    pub(super) fn has_lock(&self, id: i64) -> bool {
        self.locked.contains(&id)
    }

    /// Reads which sections have a password; those not unlocked are out
    /// of sight.
    pub(super) fn read_locks(&mut self) {
        self.locked = self.store.locks();
        self.nb.sealed = self.locked.iter().copied().filter(|s| !self.store.is_unlocked(*s)).collect();
    }

    pub(super) fn ask_password(&mut self, section: i64, set: bool) {
        self.lock_dialog = Some(LockDialog { section, set, pw: String::new(), pw2: String::new(), error: None, focus: true });
    }

    /// Puts section `id` under `password` (it stays unlocked).
    pub fn set_section_lock(&mut self, id: i64, password: &str) {
        self.flush();
        self.flush_side();
        let r = self.store.set_lock(id, password);
        self.report(r);
        self.read_locks();
    }

    /// Gives section `id`'s password: whether it was right.
    pub fn unlock_section(&mut self, id: i64, password: &str) -> bool {
        if !self.store.unlock(id, password) {
            return false;
        }
        self.read_locks();
        // Their texts, to search them and see their tasks meanwhile.
        let pages: Vec<i64> = self.nb.pages.iter().filter(|p| p.section == id && p.kind == apex_doc::notebook::PageKind::Text).map(|p| p.id).collect();
        for p in pages {
            if let Ok(doc) = self.store.load_page(p) {
                self.nb.texts.insert(p, plain_text(&doc));
                self.nb.tasks.insert(p, apex_doc::notebook::tasks::tasks_in(&doc));
            }
        }
        self.publish_sources();
        if self.section == Some(id) && self.page.is_none() {
            self.show_section(id);
        }
        true
    }

    /// Locks section `id` again: its pages close and go out of sight.
    pub fn lock_section_now(&mut self, id: i64) {
        self.flush();
        self.flush_side();
        if self.side.as_ref().and_then(|s| self.nb.page(s.page)).is_some_and(|p| p.section == id) {
            self.side = None;
        }
        if self.page.and_then(|p| self.nb.page(p)).is_some_and(|p| p.section == id) {
            self.close_page();
        }
        self.store.forget(id);
        self.read_locks();
        let pages: Vec<i64> = self.nb.pages.iter().filter(|p| p.section == id).map(|p| p.id).collect();
        for p in pages {
            self.nb.texts.insert(p, String::new());
            self.nb.tasks.remove(&p);
        }
        self.publish_sources();
    }

    /// Takes the password off section `id` (unlocked): its texts are kept
    /// in the clear again.
    pub fn remove_section_lock(&mut self, id: i64) {
        self.flush();
        self.flush_side();
        if let Err(e) = self.store.remove_lock(id) {
            return self.report(Err(e));
        }
        self.read_locks();
        // Written again, for their plain text and tasks.
        let pages: Vec<(i64, i64)> = self.nb.pages.iter().filter(|p| p.section == id && p.kind == apex_doc::notebook::PageKind::Text).map(|p| (p.id, p.modified)).collect();
        for (p, modified) in pages {
            let Ok(doc) = self.store.load_page(p) else { continue };
            let found = apex_doc::notebook::links::page_links(&doc);
            let r = self.store.save_page(p, &doc, &plain_text(&doc), &found, modified);
            self.report(r);
        }
    }

    /// In the pages list of a section locked: what it is, and a button to
    /// unlock it.
    pub(super) fn sealed_ui(&mut self, ui: &mut egui::Ui, section: i64) {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            crate::icons::paint(ui.painter(), r, crate::icons::Icon::Protect, ui.visuals().weak_text_color());
            ui.add_space(6.0);
            ui.label(RichText::new(tr("nb.lock_sealed")).weak());
            ui.add_space(8.0);
            if ui.button(tr("nb.lock_unlock")).clicked() {
                self.ask_password(section, false);
            }
        });
    }

    pub(super) fn lock_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.lock_dialog.take() else { return };
        let name = self.nb.section(d.section).map(|s| s.name.clone()).unwrap_or_default();
        let mut open = true;
        let mut done = false;
        let title = if d.set { tr("nb.lock_set") } else { tr("nb.lock_unlock") };
        egui::Window::new(title.replace('…', ""))
            .id(Id::new(("note_lock", self.uid)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show_fit(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new(name).strong());
                ui.add_space(6.0);
                let r = ui.add(egui::TextEdit::singleline(&mut d.pw).password(true).hint_text(tr("nb.lock_password")).desired_width(f32::INFINITY));
                if std::mem::take(&mut d.focus) {
                    r.request_focus();
                }
                let mut enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if d.set {
                    let r2 = ui.add(egui::TextEdit::singleline(&mut d.pw2).password(true).hint_text(tr("nb.lock_again")).desired_width(f32::INFINITY));
                    if enter {
                        r2.request_focus();
                        enter = false;
                    }
                    enter |= r2.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("nb.lock_warn")).small().weak());
                }
                if let Some(e) = d.error {
                    ui.colored_label(ui.visuals().error_fg_color, tr(e));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ok = ui.button(tr("dialog.ok")).clicked() || enter;
                    if ui.button(tr("dialog.cancel")).clicked() {
                        done = true;
                    }
                    if !ok {
                        return;
                    }
                    if d.pw.is_empty() {
                        d.error = Some("nb.lock_empty");
                    } else if d.set && d.pw != d.pw2 {
                        d.error = Some("nb.lock_mismatch");
                    } else if d.set {
                        self.set_section_lock(d.section, &d.pw);
                        done = true;
                    } else if self.unlock_section(d.section, &d.pw) {
                        done = true;
                    } else {
                        d.error = Some("nb.lock_wrong");
                        d.pw.clear();
                        d.focus = true;
                    }
                });
            });
        if open && !done {
            self.lock_dialog = Some(d);
        }
    }
}
