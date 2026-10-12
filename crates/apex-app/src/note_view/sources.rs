//! Sources: pages that describe a book, an article or a web page, with
//! the fields a citation needs. The word processor cites the sources of
//! every notebook open.

use std::sync::Mutex;

use apex_doc::Source;

use super::*;

/// The sources of each notebook open, by its view.
static OPEN: Mutex<Vec<(u64, Vec<Source>)>> = Mutex::new(Vec::new());

/// The sources of all notebooks open, to cite them.
pub fn all() -> Vec<Source> {
    OPEN.lock().map(|o| o.iter().flat_map(|(_, v)| v.iter().cloned()).collect()).unwrap_or_default()
}

impl NoteView {
    /// Tells the word processor what this notebook's sources are.
    pub(super) fn publish_sources(&self) {
        self.publish_cards();
        let mine: Vec<Source> = self
            .nb
            .pages
            .iter()
            .filter(|p| self.nb.is_live(p.id))
            .filter_map(|p| {
                let mut s = p.source.clone()?;
                if s.title.trim().is_empty() {
                    s.title.clone_from(&p.title);
                }
                Some(s)
            })
            .collect();
        if let Ok(mut o) = OPEN.lock() {
            o.retain(|(uid, _)| *uid != self.uid);
            if !mine.is_empty() {
                o.push((self.uid, mine));
            }
        }
    }

    pub(super) fn withdraw_sources(&self) {
        if let Ok(mut o) = OPEN.lock() {
            o.retain(|(uid, _)| *uid != self.uid);
        }
    }

    /// A new source page after the open one.
    pub fn new_source(&mut self) {
        let s = match self.section {
            Some(s) => s,
            None => self.here(),
        };
        let after = self.page.and_then(|p| self.nb.page(p)).map(|p| (p.parent, p.id));
        let mut id = 0;
        let title = tr("nb.untitled_source").to_string();
        self.change(|nb| {
            id = nb.add_page(s, after.and_then(|a| a.0), after.map(|a| a.1), &title, now());
            if let Some(p) = nb.page_mut(id) {
                p.source = Some(Source::default());
            }
        });
        self.close_graph();
        self.open_page(id);
        self.name_new = true;
        self.grid = false;
    }

    /// The open page's source fields, under its title.
    pub(super) fn source_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.page else { return };
        let Some(saved) = self.nb.page(id).and_then(|p| p.source.clone()) else {
            self.source_edit = None;
            return;
        };
        if self.source_edit.as_ref().is_none_or(|e| e.0 != id) {
            self.source_edit = Some((id, saved.clone()));
        }
        let Some((_, mut src)) = self.source_edit.take() else { return };
        ui.add_space(4.0);
        let line = crate::doc_view::source_entry(&src);
        let head = if line.is_empty() { tr("nb.source").to_string() } else { format!("{}: {line}", tr("nb.source")) };
        let mut done = false;
        egui::CollapsingHeader::new(RichText::new(head).color(theme::NOTE_ACCENT)).id_salt(("note_source_open", self.uid)).default_open(true).show(ui, |ui| {
            done = crate::doc_view::source_fields(ui, &format!("note_source_{}", self.uid), &mut src);
            ui.label(RichText::new(tr("nb.source_cite_tip")).weak().small());
        });
        if done && src != saved {
            // A title that followed the source keeps following it.
            let title = self.nb.page(id).map(|p| p.title.clone()).unwrap_or_default();
            let follows = title == saved.title || title == tr("nb.untitled_source");
            let new_title = (follows && !src.title.trim().is_empty()).then(|| src.title.trim().to_string());
            let s = src.clone();
            self.change(|nb| {
                if let Some(p) = nb.page_mut(id) {
                    p.source = Some(s);
                }
            });
            if let Some(t) = new_title {
                self.rename(Naming::Page(id), &t);
                self.title = t;
            }
        }
        self.source_edit = Some((id, src));
    }
}
