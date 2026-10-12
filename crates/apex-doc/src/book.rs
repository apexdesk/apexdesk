//! Book mode: a document marked as a book. Its text stays a normal
//! document (Heading 1 a chapter, Heading 2 a scene); what the book adds
//! (state of each scene, goals, daily progress) is kept here and saved in
//! a part of its own that other programs ignore.

use std::sync::Arc;

use crate::{Document, EditKind, ParaKind, Paragraph, Pos, Selection, count_words};

/// Start of the name of the hidden bookmark tying a scene's data to its
/// heading.
pub const ANCHOR: &str = "_Scene";
/// The same for a chapter (to keep track of it while the book is edited).
pub const CHAPTER_ANCHOR: &str = "_Chapter";

/// How far along a scene is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SceneStatus {
    Idea,
    #[default]
    Draft,
    Revision,
    Done,
}

impl SceneStatus {
    pub const ALL: [SceneStatus; 4] = [SceneStatus::Idea, SceneStatus::Draft, SceneStatus::Revision, SceneStatus::Done];

    /// The name in the file.
    pub fn key(self) -> &'static str {
        match self {
            SceneStatus::Idea => "idea",
            SceneStatus::Draft => "draft",
            SceneStatus::Revision => "revision",
            SceneStatus::Done => "done",
        }
    }

    pub fn from_key(s: &str) -> SceneStatus {
        SceneStatus::ALL.into_iter().find(|v| v.key() == s).unwrap_or_default()
    }
}

/// What the book knows of a scene, tied to its heading by a hidden
/// bookmark (`anchor`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneInfo {
    pub anchor: String,
    pub status: SceneStatus,
    pub synopsis: String,
    pub tags: Vec<String>,
    /// Point of view: the character telling the scene.
    pub pov: String,
}

/// Word goals of the book.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Goal {
    /// Words of the whole book (0: none).
    pub words: u32,
    /// Words a day (0: none).
    pub daily: u32,
    /// Deadline, "YYYY-MM-DD" ("" none).
    pub deadline: String,
}

/// Words of the book at the end of a day.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DayWords {
    /// "YYYY-MM-DD".
    pub date: String,
    pub words: u32,
}

/// A document's book data. A document is a book when it has one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Book {
    pub scenes: Vec<SceneInfo>,
    pub goal: Goal,
    pub progress: Vec<DayWords>,
    pub snapshots: Vec<Snapshot>,
}

/// A copy of the text of a scene, a chapter or the whole book, kept to
/// compare with later.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// "YYYY-MM-DD HH:MM".
    pub date: String,
    pub label: String,
    /// The heading's anchor ("": the whole book).
    pub anchor: String,
    /// The heading's text when it was taken.
    pub title: String,
    /// The text, a paragraph each.
    pub paras: Vec<String>,
}

impl Book {
    pub fn scene(&self, anchor: &str) -> Option<&SceneInfo> {
        self.scenes.iter().find(|s| s.anchor == anchor)
    }

    /// The scene's data, made when it has none.
    pub fn scene_mut(&mut self, anchor: &str) -> &mut SceneInfo {
        let i = match self.scenes.iter().position(|s| s.anchor == anchor) {
            Some(i) => i,
            None => {
                self.scenes.push(SceneInfo { anchor: anchor.to_string(), ..Default::default() });
                self.scenes.len() - 1
            }
        };
        &mut self.scenes[i]
    }

    /// Records `words` as the book's words on `date` ("YYYY-MM-DD", today).
    /// The first record also keeps them as where the book started, so the
    /// words of that day count from there. Returns whether it changed.
    pub fn record_day(&mut self, date: &str, words: u32) -> bool {
        if self.progress.is_empty() {
            self.progress.push(DayWords { date: String::new(), words });
        }
        let last = self.progress.last_mut().expect("not empty");
        if last.date == date {
            let changed = last.words != words;
            last.words = words;
            changed
        } else if last.date.as_str() < date {
            self.progress.push(DayWords { date: date.to_string(), words });
            true
        } else {
            false
        }
    }

    /// Words written each recorded day, oldest first: the day's words less
    /// those of the day before (none when fewer).
    pub fn days(&self) -> Vec<(&str, u32)> {
        self.progress.windows(2).filter(|w| !w[1].date.is_empty()).map(|w| (w[1].date.as_str(), w[1].words.saturating_sub(w[0].words))).collect()
    }

    /// Words written on `date`.
    pub fn written_on(&self, date: &str) -> u32 {
        self.days().into_iter().find(|d| d.0 == date).map_or(0, |d| d.1)
    }
}

/// A chapter (level 1) or scene (level 2) of a book: its heading, where
/// it ends and its words (heading included).
#[derive(Clone, Debug, PartialEq)]
pub struct BookNode {
    pub para: usize,
    pub end: usize,
    pub level: u8,
    pub title: String,
    pub words: usize,
}

impl Document {
    /// Chapters and scenes, in order.
    pub fn book_nodes(&self) -> Vec<BookNode> {
        let mut acc = Vec::with_capacity(self.paragraphs.len() + 1);
        acc.push(0usize);
        for p in &self.paragraphs {
            acc.push(acc[acc.len() - 1] + count_words(&p.visible(0, p.len())));
        }
        self.headings()
            .into_iter()
            .filter(|h| h.1 <= 2)
            .map(|(para, level)| {
                let end = self.section_end(para);
                BookNode { para, end, level, title: crate::toc::entry_text(&self.paragraphs[para]), words: acc[end] - acc[para] }
            })
            .collect()
    }

    /// Deletes the section of heading `para` (the heading and its text),
    /// as one undo step. Returns the caret after it.
    pub fn delete_section(&mut self, para: usize, sel: Selection) -> Option<Selection> {
        if para >= self.paragraphs.len() || !matches!(self.paragraphs[para].style.kind, ParaKind::Heading(_)) || self.in_table(para) {
            return None;
        }
        let end = self.section_end(para);
        let new = if end - para == self.paragraphs.len() { vec![Paragraph::new("")] } else { Vec::new() };
        let left = self.paragraphs.len() - (end - para) + new.len();
        let caret = Selection::caret(Pos::new(para.min(left - 1), 0));
        self.replace_paras(para, end, new, sel, caret, EditKind::Outline);
        Some(caret)
    }

    /// The anchor of the scene of heading `para`, if it has one.
    pub fn scene_anchor(&self, para: usize) -> Option<Arc<str>> {
        self.paragraphs.get(para)?.marks.iter().find(|m| !m.end && m.name.starts_with(ANCHOR)).map(|m| m.name.clone())
    }

    /// The data of the scene of heading `para`.
    pub fn scene_info(&self, para: usize) -> Option<&SceneInfo> {
        self.book.as_ref()?.scene(&self.scene_anchor(para)?)
    }

    /// Changes the data of the scene of heading `para` (anchoring it first
    /// if needed), as one undo step.
    pub fn edit_scene(&mut self, para: usize, sel: Selection, f: impl FnOnce(&mut SceneInfo)) {
        if self.book.is_none() || para >= self.paragraphs.len() {
            return;
        }
        let mark = self.undo_mark();
        let anchor = match self.scene_anchor(para) {
            Some(a) => a.to_string(),
            None => {
                let name = self.new_anchor(ANCHOR);
                self.add_bookmark(&name, Selection::caret(Pos::new(para, 0)));
                // `add_bookmark` keeps the selection it is given.
                if let Some(e) = self.undo.last_mut() {
                    e.sel_before = sel;
                    e.sel_after = sel;
                }
                name
            }
        };
        self.edit_parts(|p| {
            if let Some(b) = &mut p.book {
                f(b.scene_mut(&anchor));
            }
        });
        self.join_undo(mark);
    }

    /// An anchor starting with `prefix` no heading and no scene data uses.
    fn new_anchor(&self, prefix: &str) -> String {
        let num = |s: &str| s.strip_prefix(prefix).and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        let in_text = self.paragraphs.iter().flat_map(|p| &p.marks).map(|m| num(&m.name));
        let in_book = self.book.iter().flat_map(|b| &b.scenes).map(|s| num(&s.anchor));
        format!("{prefix}{}", in_text.chain(in_book).max().unwrap_or(0) + 1)
    }

    /// The anchor of heading `para` (a chapter's or a scene's), added if
    /// it has none. Not an undo step: a hidden bookmark changes nothing
    /// shown, and an undo that drops it only loses track of the heading.
    pub fn heading_anchor(&mut self, para: usize) -> Option<String> {
        let ParaKind::Heading(level) = self.paragraphs.get(para)?.style.kind else { return None };
        let prefix = if level == 1 { CHAPTER_ANCHOR } else { ANCHOR };
        if let Some(m) = self.paragraphs[para].marks.iter().find(|m| !m.end && m.name.starts_with(prefix)) {
            return Some(m.name.to_string());
        }
        let name = self.new_anchor(prefix);
        let n: Arc<str> = Arc::from(name.as_str());
        let marks = &mut self.paragraphs[para].marks;
        marks.push(crate::Mark { name: n.clone(), offset: 0, end: false });
        marks.push(crate::Mark { name: n, offset: 0, end: true });
        Some(name)
    }

    /// A copy to lay out with `paragraphs` in place of the document's:
    /// everything but the history and what layout doesn't use.
    pub fn layout_copy(&self, paragraphs: Vec<Paragraph>) -> Document {
        Document {
            paragraphs,
            page: self.page,
            meta: Default::default(),
            default_font: self.default_font.clone(),
            headers: self.headers.clone(),
            odd_even: self.odd_even,
            comments: self.comments.clone(),
            track_changes: self.track_changes,
            auto_hyphenation: self.auto_hyphenation,
            protection: None,
            user: self.user.clone(),
            styles: self.styles.clone(),
            columns: self.columns.clone(),
            deco: self.deco.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: self.revision,
            modified: false,
            notes: self.notes.clone(),
            biblio: self.biblio.clone(),
            kept: Default::default(),
            book: None,
            macros: Default::default(),
            fonts: self.fonts.clone(),
            truncated: self.truncated,
        }
    }

    /// Keeps the text of the section of heading `para` (None: the whole
    /// book) as a snapshot named `label`, as one undo step.
    pub fn take_snapshot(&mut self, para: Option<usize>, date: &str, label: &str) -> bool {
        if self.book.is_none() {
            return false;
        }
        let (range, anchor, title) = match para {
            Some(p) => {
                let Some(anchor) = self.heading_anchor(p) else { return false };
                (p..self.section_end(p), anchor, self.paragraphs[p].text.clone())
            }
            None => (0..self.paragraphs.len(), String::new(), String::new()),
        };
        let paras = self.paragraphs[range].iter().map(|p| p.text.clone()).collect();
        let snap = Snapshot { date: date.to_string(), label: label.to_string(), anchor, title, paras };
        self.edit_parts(|p| {
            if let Some(b) = &mut p.book {
                b.snapshots.push(snap);
            }
        });
        true
    }

    /// Drops snapshot `i`, as one undo step.
    pub fn drop_snapshot(&mut self, i: usize) {
        if self.book.as_ref().is_some_and(|b| i < b.snapshots.len()) {
            self.edit_parts(|p| {
                if let Some(b) = &mut p.book {
                    b.snapshots.remove(i);
                }
            });
        }
    }

    /// The paragraphs snapshot `i` is of now (none when its heading is gone).
    pub fn snapshot_range(&self, i: usize) -> Option<std::ops::Range<usize>> {
        let s = self.book.as_ref()?.snapshots.get(i)?;
        if s.anchor.is_empty() {
            return Some(0..self.paragraphs.len());
        }
        let p = self.anchor_para(&s.anchor)?;
        Some(p..self.section_end(p))
    }

    /// Snapshot `i` (the original) and the text it is of now (the revised
    /// one, with its formatting), to compare.
    pub fn snapshot_pair(&self, i: usize) -> Option<(Document, Document)> {
        let s = self.book.as_ref()?.snapshots.get(i)?;
        let old = Document::from_paragraphs(s.paras.iter().map(|t| Paragraph::new(t)).collect());
        let now = match self.snapshot_range(i) {
            Some(r) => self.paragraphs[r].to_vec(),
            None => vec![Paragraph::new("")],
        };
        Some((old, self.layout_copy(now)))
    }

    /// The heading paragraph anchor `name` is on.
    pub fn anchor_para(&self, name: &str) -> Option<usize> {
        self.paragraphs.iter().position(|p| p.marks.iter().any(|m| !m.end && &*m.name == name))
    }

    /// Scene data whose heading is gone (deleted outside ApexDesk).
    pub fn orphan_scenes(&self) -> Vec<SceneInfo> {
        let Some(b) = &self.book else { return Vec::new() };
        let used: std::collections::HashSet<&str> = self.paragraphs.iter().flat_map(|p| &p.marks).filter(|m| m.name.starts_with(ANCHOR)).map(|m| &*m.name).collect();
        b.scenes.iter().filter(|s| !used.contains(s.anchor.as_str())).cloned().collect()
    }

    /// Gives orphan data `anchor` to the scene of heading `para` (`None`:
    /// deletes it), as one undo step.
    pub fn place_orphan(&mut self, anchor: &str, para: Option<usize>, sel: Selection) {
        let Some(info) = self.book.as_ref().and_then(|b| b.scene(anchor)).cloned() else { return };
        let mark = self.undo_mark();
        if let Some(para) = para {
            self.edit_scene(para, sel, |s| {
                s.status = info.status;
                s.synopsis = info.synopsis.clone();
                s.tags = info.tags.clone();
                s.pov = info.pov.clone();
            });
        }
        self.edit_parts(|p| {
            if let Some(b) = &mut p.book {
                b.scenes.retain(|s| s.anchor != anchor);
            }
        });
        self.join_undo(mark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(text: &str, level: u8) -> Paragraph {
        let mut p = Paragraph::new(text);
        p.style.kind = ParaKind::Heading(level);
        p
    }

    #[test]
    fn chapters_and_scenes_with_their_words() {
        let mut doc = Document::from_paragraphs(vec![
            Paragraph::new("Dedicatoria breve"),
            heading("Uno", 1),
            heading("La llegada", 2),
            Paragraph::new("Ana llega tarde."),
            heading("Detalle", 3),
            heading("La cena", 2),
            Paragraph::new("Cenan."),
            heading("Dos", 1),
            Paragraph::new("Fin."),
        ]);
        let n = doc.book_nodes();
        let got: Vec<(usize, usize, u8, &str, usize)> = n.iter().map(|x| (x.para, x.end, x.level, x.title.as_str(), x.words)).collect();
        assert_eq!(got, vec![(1, 7, 1, "Uno", 10), (2, 5, 2, "La llegada", 6), (5, 7, 2, "La cena", 3), (7, 9, 1, "Dos", 2)]);
        let sel = Selection::caret(Pos::new(0, 0));
        assert_eq!(doc.delete_section(2, sel), Some(Selection::caret(Pos::new(2, 0))));
        assert_eq!(doc.book_nodes().iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Uno", "La cena", "Dos"]);
        assert_eq!(doc.delete_section(0, sel), None);
        doc.undo();
        assert_eq!(doc.paragraphs.len(), 9);
        // The whole document leaves an empty paragraph.
        let mut one = Document::from_paragraphs(vec![heading("Solo", 1), Paragraph::new("x")]);
        one.delete_section(0, sel);
        assert_eq!(one.paragraphs.len(), 1);
        assert!(one.paragraphs[0].text.is_empty());
    }

    #[test]
    fn a_snapshot_keeps_a_scene_to_compare() {
        let mut doc = Document::from_paragraphs(vec![heading("Uno", 1), heading("La llegada", 2), Paragraph::new("Ana llega tarde."), heading("La cena", 2), Paragraph::new("Cenan.")]);
        assert!(!doc.take_snapshot(Some(1), "2026-10-03 10:00", "Primera"));
        doc.book = Some(Book::default());
        assert!(doc.take_snapshot(Some(1), "2026-10-03 10:00", "Primera"));
        assert!(doc.take_snapshot(None, "2026-10-03 10:05", ""));
        let b = doc.book.as_ref().unwrap();
        assert_eq!(b.snapshots[0].paras, ["La llegada", "Ana llega tarde."]);
        assert_eq!(b.snapshots[0].title, "La llegada");
        assert_eq!(b.snapshots[1].paras.len(), 5);
        // Text added before the scene: the snapshot still finds it.
        doc.paragraphs.insert(1, Paragraph::new("Prólogo"));
        doc.paragraphs[3] = Paragraph::new("Ana llega pronto.");
        assert_eq!(doc.snapshot_range(0), Some(2..4));
        let (old, now) = doc.snapshot_pair(0).unwrap();
        assert_eq!(now.paragraphs[0].style.kind, ParaKind::Heading(2));
        assert!(crate::compare(&old, &now, "yo").has_revisions());
        doc.drop_snapshot(0);
        assert_eq!(doc.book.as_ref().unwrap().snapshots.len(), 1);
        doc.undo();
        assert_eq!(doc.book.as_ref().unwrap().snapshots.len(), 2);
    }

    #[test]
    fn the_words_of_each_day_are_recorded() {
        let mut b = Book::default();
        assert!(b.record_day("2026-10-01", 1000));
        assert_eq!(b.written_on("2026-10-01"), 0);
        assert!(b.record_day("2026-10-01", 1400));
        assert!(!b.record_day("2026-10-01", 1400));
        assert!(b.record_day("2026-10-03", 1300));
        assert!(b.record_day("2026-10-03", 2100));
        // A clock gone back changes nothing.
        assert!(!b.record_day("2026-10-02", 5000));
        assert_eq!(b.days(), vec![("2026-10-01", 400), ("2026-10-03", 700)]);
        assert_eq!(b.written_on("2026-10-03"), 700);
        assert_eq!(b.written_on("2026-10-02"), 0);
    }

    #[test]
    fn headings_are_tracked_by_anchors() {
        let mut doc = Document::from_paragraphs(vec![heading("Uno", 1), heading("A", 2), Paragraph::new("a"), heading("Dos", 1)]);
        let c = doc.heading_anchor(3).unwrap();
        let s = doc.heading_anchor(1).unwrap();
        assert_eq!((c.as_str(), s.as_str()), ("_Chapter1", "_Scene1"));
        assert_eq!(doc.heading_anchor(1).unwrap(), s);
        assert_eq!(doc.heading_anchor(2), None);
        assert!(doc.undo.is_empty());
        doc.move_section(3, 0, Selection::caret(Pos::new(0, 0)));
        assert_eq!(doc.anchor_para(&c), Some(0));
        assert_eq!(doc.anchor_para(&s), Some(2));
        // Scene data uses the anchor already there.
        doc.book = Some(Book::default());
        doc.edit_scene(2, Selection::caret(Pos::new(0, 0)), |x| x.pov = "Ana".into());
        assert_eq!(doc.scene_anchor(2).as_deref(), Some(s.as_str()));
    }

    #[test]
    fn scene_data_is_anchored_and_undoes() {
        let mut doc = Document::from_paragraphs(vec![heading("Uno", 1), heading("La llegada", 2), Paragraph::new("a"), heading("La cena", 2)]);
        let sel = Selection::caret(Pos::new(2, 1));
        doc.edit_scene(1, sel, |s| s.status = SceneStatus::Done);
        assert!(doc.scene_info(1).is_none(), "not a book");
        doc.book = Some(Book::default());
        doc.edit_scene(1, sel, |s| s.status = SceneStatus::Done);
        doc.edit_scene(3, sel, |s| s.pov = "Ana".into());
        doc.edit_scene(1, sel, |s| s.synopsis = "Llega".into());
        assert_eq!(doc.scene_anchor(1).as_deref(), Some("_Scene1"));
        assert_eq!(doc.scene_anchor(3).as_deref(), Some("_Scene2"));
        assert_eq!(doc.scene_info(1).unwrap().status, SceneStatus::Done);
        assert_eq!(doc.scene_info(1).unwrap().synopsis, "Llega");
        // Hidden: not in the bookmark list the user sees.
        assert!(doc.bookmarks().iter().all(|b| b.is_hidden()));
        // Renaming the heading keeps it.
        doc.insert_text(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 10) }, "Llegada", None);
        assert_eq!(doc.scene_info(1).unwrap().status, SceneStatus::Done);
        doc.undo();
        doc.undo();
        assert_eq!(doc.scene_info(1).unwrap().synopsis, "");
        doc.undo();
        assert!(doc.scene_anchor(3).is_none());
        assert!(doc.book.as_ref().unwrap().scene("_Scene2").is_none());
        doc.redo();
        assert_eq!(doc.scene_info(3).unwrap().pov, "Ana");
        // A heading deleted elsewhere leaves its data without a scene.
        doc.paragraphs[3].marks.clear();
        let orphans = doc.orphan_scenes();
        assert_eq!(orphans.len(), 1);
        doc.place_orphan(&orphans[0].anchor, Some(1), sel);
        assert_eq!(doc.scene_info(1).unwrap().pov, "Ana");
        assert!(doc.orphan_scenes().is_empty());
        doc.undo();
        assert_eq!(doc.orphan_scenes().len(), 1);
    }

    #[test]
    fn scenes_are_made_on_first_use() {
        let mut b = Book::default();
        assert!(b.scene("_Scene1").is_none());
        b.scene_mut("_Scene1").status = SceneStatus::Done;
        b.scene_mut("_Scene1").pov = "Ana".into();
        assert_eq!(b.scenes.len(), 1);
        assert_eq!(b.scene("_Scene1").unwrap().status, SceneStatus::Done);
        assert_eq!(SceneStatus::from_key("revision"), SceneStatus::Revision);
        assert_eq!(SceneStatus::from_key("??"), SceneStatus::Draft);
    }
}
