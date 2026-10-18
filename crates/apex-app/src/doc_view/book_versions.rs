//! Versions of a book: snapshots of the text of a scene, a chapter or the
//! whole book, kept in the book, to compare later with the text as it is
//! (in a new tab, the changes since as tracked changes).

use super::*;

/// What a new snapshot is of.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    Scene,
    Chapter,
    Book,
}

/// The versions dialog, while open.
pub(super) struct Versions {
    part: Part,
    label: String,
}

/// Now, "YYYY-MM-DD HH:MM" in local time.
fn now() -> String {
    let (y, m, d, h, mi) = common::local_now();
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

impl DocView {
    pub(super) fn open_versions(&mut self) {
        let part = if self.book_scene_at(self.sel.cursor.para).is_some() {
            Part::Scene
        } else if self.heading_above(self.sel.cursor.para, 1).is_some() {
            Part::Chapter
        } else {
            Part::Book
        };
        self.book.versions = Some(Versions { part, label: String::new() });
    }

    /// The heading a snapshot of `part` is of (None: the whole book), or
    /// None when the caret is in no such part.
    fn part_heading(&self, part: Part) -> Option<Option<usize>> {
        let caret = self.sel.cursor.para;
        match part {
            Part::Scene => self.book_scene_at(caret).map(Some),
            Part::Chapter => self.heading_above(caret, 1).map(|h| Some(h.0)),
            Part::Book => Some(None),
        }
    }

    pub(super) fn take_snapshot(&mut self, part: Part, label: &str) -> bool {
        let Some(para) = self.part_heading(part) else { return false };
        self.doc.take_snapshot(para, &now(), label.trim())
    }

    /// Opens snapshot `i` compared with the text as it is, in a new tab.
    pub(super) fn compare_snapshot(&mut self, i: usize) {
        let Some((old, now)) = self.doc.snapshot_pair(i) else { return };
        let d = apex_doc::compare(&old, &now, &self.doc.user);
        if !d.has_revisions() {
            self.notices.push(Notice::info(tr("book.v_same")));
            return;
        }
        self.compared = Some(DocToOpen(Arc::new(d)));
    }

    pub(super) fn versions_dialog(&mut self, ctx: &egui::Context) {
        let Some(v) = self.book.versions.as_mut() else { return };
        let mut part = v.part;
        let mut label = std::mem::take(&mut v.label);
        let can = [Part::Scene, Part::Chapter, Part::Book].map(|p| self.part_heading(p).is_some());
        let snaps: Vec<(String, String, String, Option<usize>)> = self
            .doc
            .book
            .iter()
            .flat_map(|b| b.snapshots.iter().enumerate())
            .map(|(i, s)| {
                let what = if s.anchor.is_empty() { tr("book.whole_book").to_string() } else { s.title.clone() };
                let words: usize = s.paras.iter().map(|p| apex_doc::count_words(p)).sum();
                (s.date.clone(), s.label.clone(), format!("{what} · {}", trf("doc.words", &[("n", &words)])), self.doc.snapshot_range(i).map(|_| i))
            })
            .collect();
        let (mut take, mut compare, mut drop) = (false, None, None);
        let closed = crate::widgets::info_dialog(ctx, "doc_book_versions", tr("book.versions_title"), 460.0, |ui| {
            ui.label(RichText::new(tr("book.v_take")).strong());
            ui.horizontal(|ui| {
                for (i, (p, key)) in [(Part::Scene, "book.v_scene"), (Part::Chapter, "book.v_chapter"), (Part::Book, "book.whole_book")].into_iter().enumerate() {
                    ui.add_enabled_ui(can[i], |ui| ui.radio_value(&mut part, p, tr(key)));
                }
            });
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut label).hint_text(tr("book.v_label_hint")).desired_width(300.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let ok = can[[Part::Scene, Part::Chapter, Part::Book].iter().position(|p| *p == part).unwrap_or(2)];
                if ui.add_enabled(ok, egui::Button::new(tr("book.v_save"))).clicked() || (enter && ok) {
                    take = true;
                }
            });
            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new(tr("book.v_list")).strong());
            if snaps.is_empty() {
                ui.label(RichText::new(tr("book.v_none")).weak());
            }
            egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                for (i, (date, name, what, there)) in snaps.iter().enumerate().rev() {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            let title = if name.is_empty() { date.clone() } else { format!("{name} — {date}") };
                            ui.label(RichText::new(title).strong());
                            ui.label(RichText::new(what).weak().small());
                            if there.is_none() {
                                ui.label(RichText::new(tr("book.v_gone")).weak().small().italics());
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(tr("book.v_delete")).clicked() {
                                drop = Some(i);
                            }
                            if ui.button(tr("book.v_compare")).on_hover_text(tr("book.v_compare_tip")).clicked() {
                                compare = Some(i);
                            }
                        });
                    });
                    ui.separator();
                }
            });
        });
        if let Some(v) = self.book.versions.as_mut() {
            v.part = part;
            v.label = label;
        }
        if take {
            let (part, label) = self.book.versions.as_ref().map(|v| (v.part, v.label.clone())).unwrap_or((Part::Book, String::new()));
            if self.take_snapshot(part, &label)
                && let Some(v) = self.book.versions.as_mut()
            {
                v.label.clear();
            }
        }
        if let Some(i) = compare {
            self.compare_snapshot(i);
            self.book.versions = None;
        }
        if let Some(i) = drop {
            self.doc.drop_snapshot(i);
        }
        if closed {
            self.book.versions = None;
            self.focus = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(t: &str, level: u8) -> Paragraph {
        let mut p = Paragraph::new(t);
        p.style.kind = apex_doc::ParaKind::Heading(level);
        p
    }

    #[test]
    fn a_snapshot_of_the_scene_compares_with_the_text_now() {
        let mut v = DocView::new(Document::from_paragraphs(vec![heading("Uno", 1), heading("Llega", 2), Paragraph::new("Ana llega tarde.")]));
        v.make_book();
        v.sel = Selection::caret(Pos::new(2, 3));
        v.open_versions();
        assert!(v.book.versions.as_ref().is_some_and(|x| x.part == Part::Scene));
        assert!(v.take_snapshot(Part::Scene, " Primera "));
        let s = &v.doc.book.as_ref().unwrap().snapshots[0];
        assert_eq!((s.label.as_str(), s.title.as_str(), s.paras.len()), ("Primera", "Llega", 2));
        // Nothing changed: nothing to open.
        v.compare_snapshot(0);
        assert!(v.compared.is_none());
        v.doc.paragraphs[2] = Paragraph::new("Ana llega pronto.");
        v.compare_snapshot(0);
        assert!(v.compared.as_ref().is_some_and(|d| d.0.has_revisions()));
        // Before any heading only the whole book can be kept.
        v.sel = Selection::caret(Pos::new(0, 0));
        assert!(!v.take_snapshot(Part::Scene, ""));
        assert!(v.take_snapshot(Part::Book, ""));
    }
}
