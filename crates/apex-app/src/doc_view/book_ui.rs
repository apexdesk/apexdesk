//! Book mode: View > Book mode marks the document as a book, for good,
//! after a warning; a book has the Book tab (chapters, scenes, counts).
//! Heading 1 is a chapter and Heading 2 a scene.

use apex_doc::book::{Book, SceneStatus};

use super::*;
use crate::toolstrip;
use crate::widgets::DialogResult;

/// How a book is seen.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum BookView {
    #[default]
    Text,
    Cards,
    Table,
}

#[derive(Default)]
pub(super) struct BookState {
    /// The warning before making the document a book is open.
    pub confirm: bool,
    /// Words, chapters and scenes, for the document revision.
    stats: Option<(u64, usize, usize, usize)>,
    pub panel: super::book_panel::Panel,
    /// Anchors of the headings whose sections are shown, when the book is
    /// seen in part.
    pub scope: Option<Vec<String>>,
    /// Where the caret was last seen in what is shown.
    pub scope_at: Option<Pos>,
    /// Go to, while open.
    pub go_to: Option<super::book_goto::GoTo>,
    /// The goals being edited.
    pub goals: Option<super::book_goals::GoalEdit>,
    /// The book's words when the document was opened.
    pub session_start: Option<usize>,
    pub view: BookView,
    pub table: super::book_table::Table,
    /// Writing without distractions; full screen asked for, and whether
    /// it was turned on for it.
    pub quiet: bool,
    pub want_fs: Option<bool>,
    pub owns_fs: bool,
    /// The line written stays half way down; where the caret was centred.
    pub typewriter: bool,
    pub tw_y: Option<f32>,
    pub review: super::book_review::Review,
    /// The cards of the notebooks open, beside the manuscript.
    pub shelf: super::book_notebook::Shelf,
    pub versions: Option<super::book_versions::Versions>,
    pub publish: Option<super::book_publish::PublishDialog>,
    /// A document made from the book to open in a new tab (and export to
    /// PDF).
    pub open: Option<(super::DocToOpen, bool)>,
}

impl BookState {
    /// The state of a document opened as it is: a book shows its panel.
    pub fn new(book: bool) -> Self {
        let mut s = BookState::default();
        s.panel.open = book;
        s
    }
}

/// The color of a scene's status.
pub(super) fn status_color(s: SceneStatus) -> Color32 {
    match s {
        SceneStatus::Idea => Color32::from_rgb(150, 150, 160),
        SceneStatus::Draft => Color32::from_rgb(230, 170, 40),
        SceneStatus::Revision => Color32::from_rgb(70, 140, 230),
        SceneStatus::Done => Color32::from_rgb(70, 180, 90),
    }
}

pub(super) fn status_name(s: SceneStatus) -> &'static str {
    match s {
        SceneStatus::Idea => tr("book.status_idea"),
        SceneStatus::Draft => tr("book.status_draft"),
        SceneStatus::Revision => tr("book.status_revision"),
        SceneStatus::Done => tr("book.status_done"),
    }
}

impl DocView {
    pub(crate) fn is_book(&self) -> bool {
        self.doc.book.is_some()
    }

    /// Makes the document a book (no undo: the warning says so).
    pub(super) fn make_book(&mut self) {
        if self.doc.book.is_none() {
            self.doc.book = Some(Book::default());
            self.doc.modified = true;
            self.book.panel.open = true;
        }
    }

    pub(super) fn book_dialog(&mut self, ctx: &egui::Context) {
        if !self.book.confirm {
            return;
        }
        match crate::widgets::dialog(ctx, "doc_book_mode", tr("book.confirm_title"), 420.0, |ui| {
            ui.label(tr("book.confirm_text"));
            ui.add_space(6.0);
            ui.label(RichText::new(tr("book.confirm_final")).strong());
        }) {
            DialogResult::None => {}
            DialogResult::Ok => {
                self.book.confirm = false;
                self.make_book();
            }
            DialogResult::Cancel => self.book.confirm = false,
        }
    }

    /// The View tab's Book mode button.
    pub(super) fn book_mode_button(&mut self, ui: &mut egui::Ui, a: Color32) {
        let on = self.is_book();
        let tip = if on { tr("book.already") } else { tr("view.book_mode_tip") };
        if toolstrip::large(ui, Icon::Book, tr("view.book_mode"), tip, a, on).clicked() && !on {
            self.book.confirm = true;
        }
    }

    /// Asks to make the document a book (File > Info's entry).
    pub(crate) fn ask_book_mode(&mut self) {
        if !self.is_book() {
            self.book.confirm = true;
        }
    }

    /// Paragraph of the heading of level `level` or higher the caret is
    /// under, if any.
    pub(super) fn heading_above(&self, para: usize, level: u8) -> Option<(usize, u8)> {
        self.doc.headings().into_iter().take_while(|&(i, _)| i <= para).filter(|&(_, l)| l <= level).last()
    }

    /// Words, chapters and scenes of the book.
    pub(super) fn book_stats(&mut self) -> (usize, usize, usize) {
        let rev = self.doc.revision;
        if let Some((r, w, c, s)) = self.book.stats
            && r == rev
        {
            return (w, c, s);
        }
        let h = self.doc.headings();
        let (c, s) = (h.iter().filter(|x| x.1 == 1).count(), h.iter().filter(|x| x.1 == 2).count());
        let w = self.doc.word_count();
        self.book.stats = Some((rev, w, c, s));
        self.record_words(w);
        (w, c, s)
    }

    /// Adds a heading of `level` with `title` before paragraph `at` (after
    /// the paragraph before it) and selects its title, as one undo step.
    pub(super) fn add_heading(&mut self, at: usize, level: u8, title: &str) {
        let last = at.saturating_sub(1).min(self.doc.paragraphs.len() - 1);
        let p = &self.doc.paragraphs[last];
        let reuse = p.text.is_empty() && p.style.kind == ParaKind::Normal && !self.doc.in_table(last);
        let mark = self.doc.undo_mark();
        let para = if reuse {
            last
        } else {
            let end = Pos::new(last, self.doc.paragraphs[last].text.len());
            self.doc.new_paragraph(Selection::caret(end)).para
        };
        let caret = Selection::caret(Pos::new(para, 0));
        self.doc.apply_style(para, para, caret, ParaKind::Heading(level), 0);
        let end = self.doc.insert_text(caret, title, None);
        self.doc.join_undo(mark);
        self.set_sel(Selection { anchor: Pos::new(para, 0), cursor: end });
    }

    /// A new chapter after the one the caret is in.
    pub(super) fn new_chapter(&mut self) {
        let caret = self.sel.cursor.para;
        let at = match self.heading_above(caret, 1) {
            Some((h, _)) => self.doc.section_end(h),
            None => self.doc.headings().into_iter().find(|&(i, l)| i > caret && l == 1).map_or(self.doc.paragraphs.len(), |h| h.0),
        };
        let n = self.doc.headings().iter().filter(|h| h.1 == 1).count() + 1;
        self.add_heading(at, 1, &trf("book.chapter_n", &[("n", &n)]));
    }

    /// A new scene after the one the caret is in, or at the end of its
    /// chapter.
    pub(super) fn new_scene(&mut self) {
        let caret = self.sel.cursor.para;
        let chapter = self.heading_above(caret, 1);
        let at = match self.heading_above(caret, 2) {
            Some((h, _)) => self.doc.section_end(h),
            None => self.doc.paragraphs.len(),
        };
        let from = chapter.map_or(0, |c| c.0);
        let n = self.doc.headings().iter().filter(|&&(i, l)| l == 2 && i >= from && i < at).count() + 1;
        self.add_heading(at, 2, &trf("book.scene_n", &[("n", &n)]));
    }

    pub(super) fn book_tab(&mut self, b: &mut toolstrip::Body) {
        let a = b.accent();
        b.group("book.g_structure", Icon::Chapter, |ui| {
            if toolstrip::large(ui, Icon::Chapter, tr("book.new_chapter"), tr("book.new_chapter_tip"), a, false).clicked() {
                self.new_chapter();
            }
            if toolstrip::large(ui, Icon::Scene, tr("book.new_scene"), tr("book.new_scene_tip"), a, false).clicked() {
                self.new_scene();
            }
            if toolstrip::large(ui, Icon::Search, tr("book.go_to"), tr("book.go_to_tip"), a, self.book.go_to.is_some()).clicked() {
                self.open_go_to();
            }
        });
        let scene = self.book_scene_at(self.sel.cursor.para);
        b.group("book.g_scene", Icon::Scene, |ui| {
            toolstrip::column(ui, |ui| {
                ui.add_enabled_ui(scene.is_some(), |ui| {
                    let cur = scene.and_then(|p| self.doc.scene_info(p)).map(|s| s.status).unwrap_or_default();
                    let mut pick = cur;
                    ui.horizontal(|ui| {
                        ui.label(tr("book.status"));
                        crate::widgets::combo_box(ui, ("book_rib_status", self.uid), 110.0, status_name(cur), |ui| {
                            for st in SceneStatus::ALL {
                                ui.selectable_value(&mut pick, st, status_name(st));
                            }
                        });
                    })
                    .response
                    .on_disabled_hover_text(tr("book.not_in_scene"));
                    if let Some(p) = scene
                        && pick != cur
                    {
                        self.doc.edit_scene(p, self.sel, |s| s.status = pick);
                    }
                    if ui.button(tr("book.edit_scene")).on_hover_text(tr("book.edit_scene_tip")).clicked() {
                        self.book.panel.open = true;
                    }
                });
            });
        });
        b.group("book.g_show", Icon::NavPane, |ui| {
            if toolstrip::large(ui, Icon::NavPane, tr("book.panel"), tr("book.panel_tip"), a, self.book.panel.open).clicked() {
                self.toggle_book_panel();
            }
            let cards = self.book.view == BookView::Cards;
            if toolstrip::large(ui, Icon::Cards, tr("book.cards"), tr("book.cards_tip"), a, cards).clicked() {
                self.book.view = if cards { BookView::Text } else { BookView::Cards };
                self.focus = cards;
            }
            let table = self.book.view == BookView::Table;
            if toolstrip::large(ui, Icon::Table, tr("book.table"), tr("book.table_tip"), a, table).clicked() {
                self.book.view = if table { BookView::Text } else { BookView::Table };
                self.focus = table;
            }
            let caret = self.sel.cursor.para;
            let chapter = self.heading_above(caret, 1).map(|h| h.0);
            toolstrip::column(ui, |ui| {
                for (part, icon, label, tip) in [(scene, Icon::Scene, "book.only_scene", "book.only_scene_tip"), (chapter, Icon::Chapter, "book.only_chapter", "book.only_chapter_tip")] {
                    let shown = part.is_some_and(|p| self.book.scope.as_ref().is_some_and(|s| s.len() == 1 && self.doc.anchor_para(&s[0]) == Some(p)));
                    let r = ui.add_enabled_ui(part.is_some(), |ui| toolstrip::small(ui, Some(icon), tr(label), tr(tip), a, shown)).inner;
                    if r.clicked()
                        && let Some(p) = part
                    {
                        self.show_only(&[p]);
                    }
                }
                if toolstrip::small(ui, Some(Icon::Book), tr("book.whole_book"), tr("book.whole_book_tip"), a, !self.scoped()).clicked() {
                    self.show_whole_book();
                }
            });
        });
        let (w, c, s) = self.book_stats();
        b.group("book.g_write", Icon::Focus, |ui| {
            if toolstrip::large(ui, Icon::Focus, tr("book.quiet"), tr("book.quiet_tip"), a, self.quiet()).clicked() {
                self.start_quiet();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, None, tr("book.typewriter"), tr("book.typewriter_tip"), a, self.book.typewriter).clicked() {
                    self.toggle_typewriter();
                }
            });
        });
        b.group("book.g_revise", Icon::Spelling, |ui| {
            if toolstrip::large(ui, Icon::Spelling, tr("book.review"), tr("book.review_tip"), a, self.book.review.open).clicked() {
                self.toggle_review();
            }
            if toolstrip::large(ui, Icon::Notebook, tr("book.notebook"), tr("book.notebook_tip"), a, self.book.shelf.open).clicked() {
                self.toggle_notebook_pane();
            }
            if toolstrip::large(ui, Icon::Compare, tr("book.versions"), tr("book.versions_tip"), a, self.book.versions.is_some()).clicked() {
                self.open_versions();
            }
        });
        b.group("book.g_book", Icon::Book, |ui| {
            toolstrip::column(ui, |ui| {
                ui.label(trf("doc.words", &[("n", &w)]));
                ui.label(trf("book.chapters", &[("n", &c)]));
                ui.label(trf("book.scenes", &[("n", &s)]));
            });
            if toolstrip::large(ui, Icon::GoalSeek, tr("book.goals"), tr("book.goals_tip"), a, self.book.goals.is_some()).clicked() {
                self.open_goals();
            }
            if toolstrip::large(ui, Icon::Book, tr("book.publish"), tr("book.publish_tip"), a, self.book.publish.is_some()).clicked() {
                self.open_publish();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_book_gets_chapters_and_scenes() {
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("")]));
        assert!(!v.is_book());
        assert!(!v.toolstrip_tabs().iter().any(|t| t.id == "book"));
        v.make_book();
        assert!(v.is_book() && v.doc.modified);
        assert!(v.toolstrip_tabs().iter().any(|t| t.id == "book"));
        // The empty first paragraph becomes the chapter.
        v.new_chapter();
        assert_eq!(v.doc.paragraphs.len(), 1);
        assert_eq!(v.doc.headings(), vec![(0, 1)]);
        v.new_scene();
        v.insert("Texto de la escena.");
        v.new_scene();
        v.new_chapter();
        v.new_scene();
        let heads: Vec<(u8, String)> = v.doc.headings().into_iter().map(|(i, l)| (l, v.doc.paragraphs[i].text.clone())).collect();
        let want = [(1, "book.chapter_n"), (2, "book.scene_n"), (2, "book.scene_n"), (1, "book.chapter_n"), (2, "book.scene_n")];
        assert_eq!(heads.iter().map(|h| h.0).collect::<Vec<_>>(), want.iter().map(|w| w.0).collect::<Vec<_>>());
        assert_eq!(heads[2].1, trf("book.scene_n", &[("n", &2)]));
        assert_eq!(heads[3].1, trf("book.chapter_n", &[("n", &2)]));
        assert_eq!(heads[4].1, trf("book.scene_n", &[("n", &1)]));
        // A new scene in the first chapter goes at its end, before chapter 2.
        v.set_sel(Selection::caret(Pos::new(0, 0)));
        v.new_scene();
        let h = v.doc.headings();
        assert_eq!(h.iter().map(|x| x.1).collect::<Vec<_>>(), vec![1, 2, 2, 2, 1, 2]);
        // Each one undoes at once.
        v.doc.undo();
        assert_eq!(v.doc.headings().len(), 5);
    }
}
