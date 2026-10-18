//! A notebook (Cuaderno) in its own tab: its sections, the pages of the
//! one open, and the page open, written with the document editor. Every
//! change goes to the file as it is made (a moment after typing stops).

use std::path::Path;

use apex_doc::notebook::{Notebook, Structure, Trashed};
use apex_doc::{Document, Paragraph};
use apex_io::notebook_store::{NoteError, NoteStore};
use eframe::egui::{self, Color32, Id, RichText, Sense, vec2};

use crate::app::Notice;
use crate::common::{self, Shell};
use crate::doc_view::DocView;
use crate::i18n::{tr, trf};
use crate::theme;

pub mod attachments;
mod board;
mod cards;
pub mod claims;
mod export;
mod graph;
mod history;
mod import;
pub mod fragments;
pub mod inbox;
pub mod sources;
pub mod shelf;
mod queries;
mod table;
mod timeline;
mod tasks;
mod types;
mod links;
mod locks;
mod panels;
mod split;
mod toolstrip;

/// File extensions of notebooks.
pub const EXTS: &[&str] = &[apex_io::notebook_store::EXT];

pub fn is_note_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(apex_io::notebook_store::EXT))
}

/// Seconds after the last keystroke before a page is written.
const SAVE_AFTER: f64 = 1.0;
/// Structure changes undo keeps.
const UNDO_STEPS: usize = 100;

/// Now, in seconds since 1970.
pub fn now() -> i64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// "YYYY-MM-DD HH:MM" of `secs` in local time.
pub fn date_text(secs: i64) -> String {
    let t = web_time::UNIX_EPOCH + web_time::Duration::from_secs(secs.clamp(0, common::LAST_SECOND) as u64);
    let (y, m, d, h, mi) = common::local_time(t);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// The plain text of a page, to search it.
pub fn plain_text(doc: &Document) -> String {
    let mut s = String::new();
    for p in &doc.paragraphs {
        if !s.is_empty() {
            s.push('\n');
        }
        s.extend(p.text.chars().filter(|&c| c != apex_doc::OBJECT));
    }
    s
}

/// What is being renamed in place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Naming {
    Section(i64),
    Page(i64),
}

pub struct NoteView {
    store: NoteStore,
    pub nb: Notebook,
    /// The section shown and the page open.
    section: Option<i64>,
    page: Option<i64>,
    /// The open page's editor.
    pub editor: Option<Box<DocView>>,
    /// The editor's revision last written, and when it changed since.
    saved_rev: u64,
    changed_at: Option<f64>,
    undo: Vec<Structure>,
    redo: Vec<Structure>,
    /// Something was deleted for good: undo no longer goes back to how
    /// the notebook was opened.
    pub purged: bool,
    pub notices: Vec<Notice>,
    /// The open page's title as typed.
    title: String,
    naming: Option<(Naming, String)>,
    /// Text searched for in the whole notebook ("" none).
    search: String,
    pub trash_open: bool,
    /// A page or section being dragged.
    drag: Option<Naming>,
    uid: u64,
    pub read_only: bool,
    pub macros: apex_doc::Macros,
    /// Pages shown as cards; `grid`: the cards show, not the open page.
    pub cards: bool,
    grid: bool,
    /// The finder `[[` and Ctrl+P open.
    finder: Option<links::Finder>,
    /// What is related shows at the foot of the page.
    related_open: bool,
    /// Canvases as their cards show them, with when they were read.
    previews: std::collections::HashMap<i64, (i64, std::sync::Arc<apex_doc::notebook::canvas::Canvas>)>,
    /// The first picture of each text page its card shows, with when it
    /// was read.
    pictures: std::collections::HashMap<i64, (i64, Option<std::sync::Arc<[u8]>>)>,
    /// The open page's other names as typed ("a, b").
    aliases: String,
    /// "Other name" was clicked: the aliases' box shows, focused.
    naming_aliases: bool,
    /// A page was just made: its title is selected, to name it.
    name_new: bool,
    /// The editor's revision whose links are in `nb.links`.
    links_rev: u64,
    /// The graph, when it shows instead of the page.
    graph: Option<Box<graph::Graph>>,
    /// The open page's canvas, when it is one (it has no editor).
    board: Option<Box<board::Board>>,
    /// The tasks panel, while shown.
    tasks: Option<tasks::TaskPanel>,
    /// The gallery of attachments, while shown.
    gallery: Option<attachments::Gallery>,
    /// An attachment copied out, for the app to open.
    pub open_file: Option<std::path::PathBuf>,
    /// The capture window was asked for here (the app shows it).
    pub capture_wanted: bool,
    /// A link back to a fragment's original, for the app to open.
    pub open_request: Option<(std::path::PathBuf, fragments::Spot)>,
    /// The open source page's fields as typed.
    source_edit: Option<(i64, apex_doc::Source)>,
    /// The page types window, while open.
    types_win: Option<types::TypesWindow>,
    /// The table of pages, while shown.
    table: Option<table::TablePanel>,
    /// The timeline, while shown.
    timeline: Option<timeline::Timeline>,
    /// A page's history, while open.
    history: Option<history::HistoryWindow>,
    /// The sections with a password, and the password window.
    locked: std::collections::HashSet<i64>,
    lock_dialog: Option<locks::LockDialog>,
    /// The page shown beside the open one.
    side: Option<split::Side>,
    /// It is to the left of the open one.
    side_left: bool,
}

impl NoteView {
    /// A new notebook at `path` with a section and an empty page.
    pub fn create(path: &Path) -> Result<NoteView, NoteError> {
        let mut store = NoteStore::create(path)?;
        let mut nb = Notebook::default();
        let s = nb.add_section(&tr("nb.first_section"), now());
        nb.add_page(s, None, None, &tr("nb.untitled"), now());
        store.save_structure(&nb)?;
        Ok(NoteView::with(store, nb))
    }

    pub fn open(path: &Path) -> Result<NoteView, NoteError> {
        let mut store = NoteStore::open(path)?;
        let mut nb = store.load()?;
        // What has been in the trash long enough goes.
        if !nb.purge(now(), None).is_empty() || nb.sections.iter().any(|s| s.deleted.is_some_and(|d| now() - d > apex_doc::notebook::TRASH_DAYS * 86_400)) {
            store.save_structure(&nb)?;
        }
        Ok(NoteView::with(store, nb))
    }

    fn with(store: NoteStore, nb: Notebook) -> NoteView {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut v = NoteView {
            store,
            nb,
            section: None,
            page: None,
            editor: None,
            saved_rev: 0,
            changed_at: None,
            undo: Vec::new(),
            redo: Vec::new(),
            purged: false,
            notices: Vec::new(),
            title: String::new(),
            naming: None,
            search: String::new(),
            trash_open: false,
            drag: None,
            uid: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            read_only: false,
            cards: false,
            grid: false,
            finder: None,
            related_open: true,
            previews: Default::default(),
            pictures: Default::default(),
            aliases: String::new(),
            naming_aliases: false,
            name_new: false,
            links_rev: u64::MAX,
            graph: None,
            board: None,
            tasks: None,
            gallery: None,
            open_file: None,
            capture_wanted: false,
            open_request: None,
            source_edit: None,
            types_win: None,
            table: None,
            timeline: None,
            history: None,
            locked: Default::default(),
            lock_dialog: None,
            side: None,
            side_left: false,
            macros: apex_doc::Macros::default(),
        };
        v.cards = v.store.setting("cards").as_deref() == Some("1");
        v.related_open = v.store.setting("related").as_deref() != Some("0");
        v.read_locks();
        let first = v.nb.live_sections().first().map(|s| s.id);
        if let Some(s) = first {
            v.show_section(s);
        }
        v.publish_sources();
        v
    }

    /// Its changes are always in the file.
    pub fn is_modified(&self) -> bool {
        false
    }

    #[cfg(test)]
    pub fn section(&self) -> Option<i64> {
        self.section
    }

    #[cfg(test)]
    pub fn page(&self) -> Option<i64> {
        self.page
    }

    fn report(&mut self, r: Result<(), NoteError>) {
        if let Err(e) = r {
            self.notices.push(Notice::error(trf("nb.save_failed", &[("error", &e)])));
        }
    }

    /// Writes the structure after a change.
    fn save_structure(&mut self) {
        let r = self.store.save_structure(&self.nb);
        self.report(r);
        self.publish_sources();
    }

    /// Changes the structure as one undo step, and writes it.
    fn change(&mut self, f: impl FnOnce(&mut Notebook)) {
        let before = self.nb.structure();
        f(&mut self.nb);
        // Nothing goes into a section locked: it would be kept unsealed.
        let into_sealed = self.nb.pages.iter().find(|p| self.nb.sealed.contains(&p.section) && before.pages.iter().find(|b| b.id == p.id).is_none_or(|b| b.section != p.section)).map(|p| p.section);
        if let Some(s) = into_sealed {
            self.nb.set_structure(before);
            self.ask_password(s, false);
            return;
        }
        if self.nb.structure() != before {
            self.undo.push(before);
            if self.undo.len() > UNDO_STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.save_structure();
            self.fix_selection();
        }
    }

    #[cfg(test)]
    pub fn can_undo(&self) -> bool {
        self.undo.last().is_some()
    }

    /// Undoes the last change of sections and pages.
    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            // The page shown is written while it is still there: the
            // change may take it out of the file.
            self.flush();
            self.redo.push(self.nb.structure());
            self.nb.set_structure(s);
            self.save_structure();
            self.fix_selection();
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            // The page shown is written while it is still there: the
            // change may take it out of the file.
            self.flush();
            self.undo.push(self.nb.structure());
            self.nb.set_structure(s);
            self.save_structure();
            self.fix_selection();
        }
    }

    /// After a change: the section and page shown still there.
    fn fix_selection(&mut self) {
        if self.side.as_ref().is_some_and(|s| !self.nb.is_live(s.page)) {
            self.side = None;
        }
        if self.page.is_some_and(|p| !self.nb.is_live(p)) {
            self.close_page();
        }
        if let Some(p) = self.page.and_then(|p| self.nb.page(p)) {
            self.section = Some(p.section);
            self.title.clone_from(&p.title);
            self.aliases = p.aliases.join(", ");
        }
        if self.section.is_none_or(|s| self.nb.section(s).is_none_or(|s| s.deleted.is_some())) {
            self.section = self.nb.live_sections().first().map(|s| s.id);
        }
        if self.page.is_none()
            && let Some(s) = self.section
            && let Some(p) = self.nb.tree(s).first().map(|(_, p)| p.id)
        {
            self.open_page(p);
        }
    }

    pub fn show_section(&mut self, id: i64) {
        self.section = Some(id);
        self.grid = self.cards;
        if self.page.and_then(|p| self.nb.page(p)).is_none_or(|p| p.section != id) {
            match self.nb.tree(id).first().map(|(_, p)| p.id) {
                Some(p) => self.open_page(p),
                None => self.close_page(),
            }
        }
    }

    /// A text page is open (files dropped on it are kept with it).
    pub fn page_editing(&self) -> bool {
        self.editor.is_some() && self.graph.is_none() && self.tasks.is_none() && self.gallery.is_none() && self.timeline.is_none()
    }

    /// Writes the open page if it changed.
    pub fn flush(&mut self) {
        self.write_board();
        let (Some(id), Some(ed)) = (self.page, self.editor.as_ref()) else { return };
        if ed.doc.revision == self.saved_rev {
            return;
        }
        let text = plain_text(&ed.doc);
        let found = apex_doc::notebook::links::page_links(&ed.doc);
        let tasks = apex_doc::notebook::tasks::tasks_in(&ed.doc);
        let t = now();
        let r = self.store.save_page(id, &ed.doc, &text, &found, t);
        self.saved_rev = ed.doc.revision;
        self.changed_at = None;
        self.nb.texts.insert(id, text);
        self.nb.links.insert(id, found);
        self.nb.tasks.insert(id, tasks);
        if let Some(p) = self.nb.page_mut(id) {
            p.modified = t;
        }
        self.report(r);
        self.publish_cards();
    }

    fn close_page(&mut self) {
        // A page an undo took out has nowhere to be written.
        if self.page.is_some_and(|p| self.nb.page(p).is_some()) {
            self.flush();
        }
        self.page = None;
        self.editor = None;
        self.board = None;
        self.title.clear();
    }

    pub fn open_page(&mut self, id: i64) {
        if self.page == Some(id) {
            return;
        }
        if self.side.as_ref().is_some_and(|s| s.page == id) {
            return self.swap_sides();
        }
        self.close_page();
        if self.nb.page(id).is_some_and(|p| p.kind == apex_doc::notebook::PageKind::Canvas) {
            let canvas = self.store.load_canvas(id).unwrap_or_else(|e| {
                self.notices.push(Notice::error(trf("nb.save_failed", &[("error", &e)])));
                Default::default()
            });
            self.board = Some(Box::new(board::Board::new(id, canvas)));
        } else {
            let doc = match self.store.load_page(id) {
                Ok(d) => self.with_embeds(id, d),
                Err(e) => {
                    self.notices.push(Notice::error(trf("nb.save_failed", &[("error", &e)])));
                    Document::from_paragraphs(vec![Paragraph::new("")])
                }
            };
            let ed = DocView::embedded(doc);
            self.saved_rev = ed.doc.revision;
            self.editor = Some(Box::new(ed));
        }
        self.page = Some(id);
        if let Some(p) = self.nb.page(id) {
            self.section = Some(p.section);
            self.title.clone_from(&p.title);
            self.aliases = p.aliases.join(", ");
            self.naming_aliases = false;
        }
    }

    /// Page `id`'s text with the pages shown in it as they are now.
    fn with_embeds(&self, id: i64, mut doc: Document) -> Document {
        apex_doc::notebook::embed::refresh(&mut doc, &mut |p| {
            let title = self.nb.page(p).filter(|_| p != id && self.nb.is_live(p))?.title.clone();
            Some((title, self.store.load_page(p).ok()?))
        });
        doc
    }

    pub fn new_section(&mut self) {
        let mut id = 0;
        let name = tr("nb.new_section").to_string();
        self.change(|nb| id = nb.add_section(&name, now()));
        let mut page = 0;
        self.change(|nb| page = nb.add_page(id, None, None, &tr("nb.untitled"), now()));
        // One step for both.
        if self.undo.len() >= 2 {
            self.undo.pop();
        }
        self.show_section(id);
        self.open_page(page);
        self.naming = Some((Naming::Section(id), name));
    }

    /// The section new pages from elsewhere go to: the one shown, else
    /// the first one that is open.
    pub(super) fn here_section(&self) -> Option<i64> {
        let open = |s: &i64| self.nb.section(*s).is_some_and(|x| x.deleted.is_none()) && !self.nb.sealed.contains(s);
        self.section.filter(open).or_else(|| self.nb.live_sections().iter().map(|s| s.id).find(open))
    }

    /// As [`here_section`](Self::here_section), made when there is none.
    pub(super) fn here(&mut self) -> i64 {
        if let Some(s) = self.here_section() {
            return s;
        }
        let mut id = 0;
        self.change(|nb| id = nb.add_section(&tr("nb.first_section"), now()));
        id
    }

    /// A new page after the open one (`sub`: under it).
    pub fn new_page(&mut self, sub: bool) {
        let s = match self.section {
            Some(s) => s,
            None => self.here(),
        };
        self.close_graph();
        let (parent, after) = match (self.page.and_then(|p| self.nb.page(p)), sub) {
            (Some(p), true) => (Some(p.id), None),
            (Some(p), false) => (p.parent, Some(p.id)),
            (None, _) => (None, None),
        };
        let mut id = 0;
        self.change(|nb| id = nb.add_page(s, parent, after, &tr("nb.untitled"), now()));
        self.open_page(id);
        self.name_new = true;
        self.grid = false;
    }

    pub fn delete_page(&mut self, id: i64) {
        self.flush();
        self.change(|nb| nb.delete_page(id, now()));
        self.notices.push(Notice::info(tr("nb.to_trash")));
    }

    pub fn delete_section(&mut self, id: i64) {
        self.flush();
        self.change(|nb| nb.delete_section(id, now()));
        self.notices.push(Notice::info(tr("nb.to_trash")));
    }

    pub fn restore(&mut self, t: Trashed) {
        self.change(|nb| nb.restore(t));
        match t {
            Trashed::Page(p) => {
                self.open_page(p);
                self.grid = false;
            }
            Trashed::Section(s) => self.show_section(s),
        }
    }

    /// Removes `t` from the trash for good (not undone).
    pub fn purge(&mut self, t: Trashed) {
        self.nb.purge(now(), Some(t));
        self.purged = true;
        self.undo.clear();
        self.redo.clear();
        self.save_structure();
    }

    fn rename(&mut self, what: Naming, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let name = name.to_string();
        let old = match what {
            Naming::Page(id) => self.nb.page(id).map(|p| p.title.clone()),
            Naming::Section(_) => None,
        };
        let new = name.clone();
        self.change(|nb| match what {
            Naming::Section(id) => {
                if let Some(s) = nb.section_mut(id) {
                    s.name = name;
                }
            }
            Naming::Page(id) => {
                if let Some(p) = nb.page_mut(id) {
                    p.title = name;
                }
            }
        });
        if let (Naming::Page(id), Some(old)) = (what, old)
            && old != new
        {
            self.retitle_links(id, &old, &new);
        }
    }

    /// Sets the open page's other names as typed.
    fn commit_aliases(&mut self) {
        let Some(id) = self.page else { return };
        let list: Vec<String> = self.aliases.split(',').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
        self.change(|nb| {
            if let Some(p) = nb.page_mut(id) {
                p.aliases = list;
            }
        });
    }

    /// Sets the open page's title as typed.
    fn commit_title(&mut self) {
        let Some(id) = self.page else { return };
        let t = self.title.trim().to_string();
        if t.is_empty() || self.nb.page(id).is_some_and(|p| p.title == t) {
            return;
        }
        self.rename(Naming::Page(id), &t);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let time = ui.input(|i| i.time);
        if ui.input_mut(|i| i.modifiers.matches_exact(egui::Modifiers::COMMAND) && i.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
            self.flush();
            self.open_go_to();
        }
        egui::Panel::left(Id::new(("note_sections", self.uid))).resizable(true).default_size(170.0).min_size(34.0).show(ui, |ui| self.sections_ui(ui));
        egui::Panel::left(Id::new(("note_pages", self.uid))).resizable(true).default_size(230.0).min_size(150.0).show(ui, |ui| self.pages_ui(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ui.visuals().extreme_bg_color)).show(ui, |ui| {
            if self.table.is_some() {
                self.table_ui(ui);
            } else if self.timeline.is_some() {
                self.timeline_ui(ui);
            } else if self.tasks.is_some() {
                self.tasks_ui(ui);
            } else if self.gallery.is_some() {
                self.gallery_ui(ui);
            } else if self.graph.is_some() {
                self.graph_ui(ui);
            } else if self.cards && self.grid {
                self.cards_ui(ui);
            } else {
                self.page_ui(ui, shell);
            }
        });
        self.trash_ui(ui.ctx());
        self.editor_requests(ui);
        self.finder_ui(ui.ctx());
        self.types_window(ui.ctx());
        self.history_window(ui.ctx());
        self.lock_window(ui.ctx());
        // A moment after typing stops, the page goes to the file.
        if let Some(ed) = &self.editor {
            if ed.doc.revision != self.saved_rev {
                let at = *self.changed_at.get_or_insert(time);
                if time - at >= SAVE_AFTER {
                    self.flush();
                } else {
                    ui.ctx().request_repaint_after(web_time::Duration::from_secs_f64(SAVE_AFTER));
                }
            }
        }
        if let Some(ed) = &mut self.editor {
            self.notices.append(&mut ed.notices);
        }
        self.side_autosave(ui, time);
    }

    fn page_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let Some(id) = self.page else {
            // A locked section says so in its list: nothing to create here.
            if !self.section.is_some_and(|s| self.nb.sealed.contains(&s)) {
                ui.centered_and_justified(|ui| ui.label(RichText::new(tr("nb.no_page")).weak()));
            }
            return;
        };
        let (created, modified) = self.nb.page(id).map_or((0, 0), |p| (p.created, p.modified));
        egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin { left: 24, right: 24, top: 14, bottom: 8 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let r = ui.add(egui::TextEdit::singleline(&mut self.title).id(Id::new(("note_title", self.uid))).font(egui::FontId::proportional(24.0)).frame(egui::Frame::NONE).hint_text(tr("nb.untitled")).desired_width(f32::INFINITY));
            // Asked until it has it: the editor takes focus as it opens.
            if self.name_new && r.has_focus() {
                self.name_new = false;
            } else if self.name_new {
                let mut st = egui::text_edit::TextEditState::load(ui.ctx(), r.id).unwrap_or_default();
                let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(self.title.chars().count()));
                st.cursor.set_char_range(Some(all));
                st.store(ui.ctx(), r.id);
                r.request_focus();
            }
            if r.lost_focus() {
                self.commit_title();
            }
            // Enter goes on to the text.
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                // Taken here, or the text would get it too.
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                match &mut self.editor {
                    Some(ed) => ed.focus_text(),
                    None => ui.memory_mut(|m| m.request_focus(Id::NULL)),
                }
            }
            let dates = if modified > created { trf("nb.dates", &[("created", &date_text(created)), ("modified", &date_text(modified))]) } else { date_text(created) };
            ui.horizontal(|ui| {
                ui.label(RichText::new(dates).weak().small());
                ui.add_space(12.0);
                // Other names only when it has them, or one is being given.
                if self.aliases.is_empty() && !self.naming_aliases {
                    if ui.add(egui::Label::new(RichText::new(tr("nb.add_alias")).weak().small()).sense(Sense::click())).on_hover_text(tr("nb.aliases_hint")).clicked() {
                        self.naming_aliases = true;
                    }
                    return;
                }
                ui.label(RichText::new(tr("nb.aliases")).weak().small());
                let r = ui.add(egui::TextEdit::singleline(&mut self.aliases).font(egui::TextStyle::Small).frame(egui::Frame::NONE).hint_text(tr("nb.aliases_hint")).desired_width(f32::INFINITY));
                if r.lost_focus() {
                    self.naming_aliases = false;
                    self.commit_aliases();
                } else if self.naming_aliases && !r.has_focus() {
                    r.request_focus();
                }
            });
            self.source_ui(ui);
            self.claim_ui(ui);
            self.fields_ui(ui);
        });
        let open = self.related_open && !self.nothing_related(id);
        egui::Panel::bottom(Id::new(("note_related_panel", self.uid)))
            .resizable(open)
            .default_size(200.0)
            .min_size(if open { 120.0 } else { 0.0 })
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin { left: 24, right: 24, top: 6, bottom: 6 }))
            .show(ui, |ui| self.related_ui(ui));
        if self.side.is_some() && (self.editor.is_some() || self.board.is_some()) {
            self.split_ui(ui, shell);
        } else if self.board.is_some() {
            self.board_ui(ui);
        } else if let Some(ed) = &mut self.editor {
            ed.ui(ui, shell);
        }
    }

    /// The open page, to print it.
    pub fn print_doc(&self) -> Document {
        self.editor.as_ref().map_or_else(|| Document::from_paragraphs(vec![Paragraph::new("")]), |ed| ed.doc.clone())
    }

    /// A copy of the notebook at `path`, with the open page as it is.
    pub fn copy_to(&mut self, path: &Path) -> Result<(), String> {
        self.flush();
        self.store.copy_to(path).map_err(|e| e.to_string())
    }

    pub fn status(&self, ui: &mut egui::Ui) {
        let live = self.nb.live_set();
        let pages = self.nb.pages.iter().filter(|p| live.contains(&p.id)).count();
        ui.label(trf("nb.pages", &[("n", &pages)]));
        if let Some(ed) = &self.editor {
            ui.separator();
            let words = plain_text(&ed.doc).split_whitespace().count();
            ui.label(trf("doc.words", &[("n", &words)]));
        }
    }
}

/// A small notebook for the window checks: two sections, text pages
/// with links and a subpage, a canvas with notes, a page type with a
/// field, written to `path`.
#[cfg(test)]
pub fn qa_sample(path: &Path) {
    let mut v = NoteView::create(path).expect("sample notebook");
    let first = v.page().expect("a first page");
    let mut ty = 0;
    v.change(|nb| {
        ty = nb.add_type("Lugar");
        if let Some(t) = nb.page_type_mut(ty) {
            t.fields = vec![apex_doc::notebook::Field::new("Habitantes", apex_doc::notebook::FieldKind::Number)];
        }
    });
    let typed = |v: &mut NoteView, text: &str| {
        if let Some(ed) = v.editor.as_mut() {
            ed.doc.insert_text(apex_doc::Selection::default(), text, None);
        }
        v.flush();
    };
    typed(&mut v, "Ana llega al puerto de noche. #trama La lonja abre al amanecer.");
    v.new_page(false);
    typed(&mut v, "El faro sigue apagado.");
    let place = v.page();
    v.new_page(true);
    typed(&mut v, "Una subpágina con una tarea.");
    v.change(|nb| {
        if let Some(p) = place {
            nb.set_field(p, "Habitantes", "1200");
            if let Some(pg) = nb.page_mut(p) {
                pg.ptype = Some(ty);
            }
        }
    });
    v.new_canvas();
    if let Some(b) = v.board.as_mut() {
        b.add_note(eframe::egui::Vec2::ZERO, "Idea");
        b.add_note(eframe::egui::Vec2::new(260.0, 40.0), "Otra idea");
    }
    v.write_board();
    v.new_section();
    typed(&mut v, "Otra sección.");
    v.open_page(first);
}

impl Drop for NoteView {
    fn drop(&mut self) {
        self.withdraw_sources();
        self.withdraw_cards();
        self.flush();
        self.flush_side();
    }
}

/// What a link says, in the user's language.
pub fn rel_name(rel: apex_doc::notebook::Rel) -> String {
    use apex_doc::notebook::Rel;
    tr(match rel {
        Rel::Link => "nb.rel.link",
        Rel::Supports => "nb.rel.supports",
        Rel::Contradicts => "nb.rel.contradicts",
        Rel::Expands => "nb.rel.expands",
        Rel::Example => "nb.rel.example",
        Rel::Question => "nb.rel.question",
        Rel::Causes => "nb.rel.causes",
    })
    .to_string()
}

/// The color of a section, for egui.
pub fn section_color(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[cfg(test)]
mod tests;
