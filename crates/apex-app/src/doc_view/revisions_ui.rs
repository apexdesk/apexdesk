//! Track changes: how tracked text looks (per author colors, or hidden in
//! the No Markup and Original views), accepting and rejecting, and Compare.
//! Formatting changes aren't tracked.

use std::sync::Arc;

use apex_doc::{Attrs, Document, RevKind, Selection};
use eframe::egui::text::TextFormat;
use eframe::egui::{self, Color32, Key, Modifiers, RichText, Sense, Stroke};

use super::DocView;
use crate::app::Notice;
use crate::i18n::{tr, trf};

/// What the page shows of tracked changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub(super) enum Markup {
    /// Insertions underlined and deletions struck through.
    #[default]
    All,
    /// The text as it would be with every change accepted.
    Final,
    /// The text as it was before the changes.
    Original,
}

/// A document for the app to open in a new tab (the result of Compare).
#[derive(Clone)]
pub struct DocToOpen(pub Arc<Document>);

impl PartialEq for DocToOpen {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for DocToOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DocToOpen")
    }
}

const AUTHOR_COLORS: [u32; 8] = [0xB5082E, 0x2B579A, 0x2E7D32, 0x8E24AA, 0xC05800, 0x00838F, 0x6D4C41, 0xAD1457];

/// A color per author, the same in every document.
pub(super) fn author_color(author: &str) -> Color32 {
    let h = author.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    crate::theme::rgb(AUTHOR_COLORS[(h % AUTHOR_COLORS.len() as u32) as usize])
}

impl DocView {
    /// Tracked text's look in the current view.
    pub(super) fn markup_format(&self, fmt: &mut TextFormat, a: &Attrs) {
        let Some(rev) = &a.rev else { return };
        let hide = match self.markup {
            Markup::All => false,
            Markup::Final => rev.kind == RevKind::Delete,
            Markup::Original => rev.kind == RevKind::Insert,
        };
        if hide {
            // Hidden text keeps its characters (so offsets stay) at no width.
            fmt.font_id.size = 0.05;
            // Lines of hidden text alone (a whole deleted paragraph) fold.
            fmt.line_height = Some(0.05);
            fmt.color = Color32::TRANSPARENT;
            fmt.background = Color32::TRANSPARENT;
            fmt.underline = Stroke::NONE;
            fmt.strikethrough = Stroke::NONE;
            return;
        }
        if self.markup != Markup::All {
            return;
        }
        let c = author_color(&rev.author);
        let w = (fmt.font_id.size / 16.0).max(1.0);
        fmt.color = c;
        match rev.kind {
            RevKind::Insert => fmt.underline = Stroke::new(w, c),
            RevKind::Delete => fmt.strikethrough = Stroke::new(w, c),
        }
    }

    /// A bar in the left margin beside lines with changes.
    pub(super) fn paint_change_bar(&self, p: &egui::Painter, seg: &super::Segment, o: eframe::egui::Pos2) {
        if self.markup != Markup::All || seg.cell || !self.doc.paragraphs[seg.para].has_revisions() {
            return;
        }
        let x = o.x - 10.0 * self.zoom.max(0.5);
        p.vline(x, (o.y + seg.y)..=(o.y + seg.y + seg.gy1 - seg.gy0), Stroke::new(1.5, Color32::from_gray(110)));
    }

    // --- Commands -----------------------------------------------------------------------

    pub(super) fn toggle_tracking(&mut self) {
        self.doc.track_changes = !self.doc.track_changes;
        self.notices.push(Notice::info(tr(if self.doc.track_changes { "rev.tracking_on" } else { "rev.tracking_off" })));
    }

    fn set_markup(&mut self, m: Markup) {
        self.markup = m;
    }

    /// Accepts or rejects the changes in the selection, or the one at the
    /// caret, then moves to the next change.
    fn resolve_here(&mut self, accept: bool) {
        let (a, b) = (self.sel.start(), self.sel.end());
        let n = self.doc.resolve_changes(a, b, accept, self.sel);
        if n == 0 {
            self.notices.push(Notice::info(tr("rev.none_here")));
            return;
        }
        let at = self.doc.clamp(a);
        self.set_sel(Selection::caret(at));
        if let Some(c) = self.doc.next_change(at, true).filter(|c| c.start >= at) {
            self.set_sel(Selection { anchor: c.start, cursor: c.end });
        }
    }

    fn resolve_all(&mut self, accept: bool) {
        let n = self.doc.resolve_all(accept, self.sel);
        if n == 0 {
            self.notices.push(Notice::info(tr("rev.none")));
        } else {
            let s = self.sel;
            self.set_sel(s);
        }
    }

    fn go_to_change(&mut self, forward: bool) {
        let from = if forward { self.sel.end() } else { self.sel.start() };
        match self.doc.next_change(from, forward) {
            Some(c) => self.set_sel(Selection { anchor: c.start, cursor: c.end }),
            None => self.notices.push(Notice::info(tr("rev.none"))),
        }
    }

    /// Review > Compare: the differences between this document (the
    /// original) and a file picked by the user (the revised one), as a new
    /// document with tracked changes.
    fn compare_with_file(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("rev.compare_title")).add_filter(tr("dialog.filter_all"), apex_io::DOC_EXTS).pick_file(crate::file_dialog::in_view(|v: &mut DocView, path: std::path::PathBuf| v.compare_with(&path)));
    }

    /// The differences between this document and the file `path`.
    fn compare_with(&mut self, path: &std::path::Path) {
        match apex_io::open(path) {
            Ok((apex_io::Loaded::Doc(revised), _)) => {
                let d = apex_doc::compare(&self.doc, &revised, &self.doc.user);
                if !d.has_revisions() {
                    self.notices.push(Notice::info(tr("rev.compare_same")));
                }
                self.compared = Some(DocToOpen(Arc::new(d)));
            }
            Ok(_) => self.notices.push(Notice::error(tr("rev.compare_not_doc"))),
            Err(e) => self.notices.push(Notice::error(trf("rev.compare_failed", &[("error", &e)]))),
        }
    }

    pub(super) fn revision_key(&mut self, key: Key, m: Modifiers) -> bool {
        if key == Key::E && m.command && m.shift && !m.alt {
            self.toggle_tracking();
            return true;
        }
        false
    }

    // --- Menus --------------------------------------------------------------------------

    pub(super) fn revision_menu_items(&mut self, ui: &mut egui::Ui) {
        let mut on = self.doc.track_changes;
        if ui.add(egui::Checkbox::new(&mut on, tr("rev.track"))).on_hover_text(format!("Ctrl+Shift+E. {}", tr("rev.formatting_note"))).clicked() {
            self.toggle_tracking();
            ui.close();
        }
        crate::widgets::menu_button(ui, tr("rev.display"), |ui| {
            for (m, key) in [(Markup::All, "rev.markup_all"), (Markup::Final, "rev.markup_none"), (Markup::Original, "rev.markup_original")] {
                if ui.radio(self.markup == m, tr(key)).clicked() {
                    self.set_markup(m);
                    ui.close();
                }
            }
        });
        let any = self.doc.has_revisions();
        ui.add_enabled_ui(any, |ui| {
            crate::widgets::menu_button(ui, tr("rev.accept"), |ui| {
                if ui.button(tr("rev.accept_this")).clicked() {
                    self.resolve_here(true);
                    ui.close();
                }
                if ui.button(tr("rev.accept_all")).clicked() {
                    self.resolve_all(true);
                    ui.close();
                }
            });
            crate::widgets::menu_button(ui, tr("rev.reject"), |ui| {
                if ui.button(tr("rev.reject_this")).clicked() {
                    self.resolve_here(false);
                    ui.close();
                }
                if ui.button(tr("rev.reject_all")).clicked() {
                    self.resolve_all(false);
                    ui.close();
                }
            });
            if ui.button(tr("rev.previous")).clicked() {
                self.go_to_change(false);
                ui.close();
            }
            if ui.button(tr("rev.next")).clicked() {
                self.go_to_change(true);
                ui.close();
            }
        });
        ui.separator();
        if ui.button(format!("{}…", tr("rev.compare"))).on_hover_text(tr("rev.compare_hint")).clicked() {
            ui.close();
            self.compare_with_file();
        }
    }

    /// Context menu entries for the change at the caret or in the selection.
    pub(super) fn revision_context_items(&mut self, ui: &mut egui::Ui) -> bool {
        let here = if self.sel.is_empty() {
            self.doc.change_at(self.sel.cursor).is_some()
        } else {
            let (a, b) = (self.sel.start(), self.sel.end());
            self.doc.changes().iter().any(|c| c.start < b && c.end > a)
        };
        if !here {
            return false;
        }
        if ui.button(tr("rev.accept_change")).clicked() {
            self.resolve_here(true);
            ui.close();
        }
        if ui.button(tr("rev.reject_change")).clicked() {
            self.resolve_here(false);
            ui.close();
        }
        true
    }

    /// "Track Changes: On" in the status bar (a click turns it off).
    pub(super) fn revision_status(&mut self, ui: &mut egui::Ui) {
        if !self.doc.track_changes {
            return;
        }
        ui.separator();
        let r = ui.add(egui::Label::new(RichText::new(tr("rev.status_on")).color(author_color(&self.doc.user))).sense(Sense::click()));
        if r.on_hover_text(format!("{} (Ctrl+Shift+E)", tr("rev.track"))).clicked() {
            self.toggle_tracking();
        }
    }
}
