//! The revision pane of a book: words repeated close together, filler
//! words, how much of each scene is dialogue and names written in more
//! than one way. A click goes to the words in the text.

use super::*;
use apex_doc::book_review::{Hit, Repeat, fillers_of};

/// Words between a word and the same word again that make a repeat.
const WINDOW: usize = 40;
/// Shortest word that counts as repeated.
const MIN_LEN: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Tab {
    #[default]
    Repeats,
    Fillers,
    Dialogue,
    Names,
}

/// What is looked at: the whole book, or the chapter or scene of the caret.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Range {
    #[default]
    Book,
    Chapter,
    Scene,
}

/// What was found, for a revision of the text.
#[derive(Default)]
struct Found {
    rev: u64,
    paras: (usize, usize),
    repeats: Vec<Repeat>,
    /// The repeats by word, most repeated first: where it was written again.
    repeated: Vec<(String, Vec<Hit>)>,
    fillers: Vec<(String, Vec<Hit>)>,
    /// Scene heading, title and share of dialogue.
    dialogue: Vec<(usize, String, f32)>,
    names: Vec<Vec<(String, Vec<Hit>)>>,
}

#[derive(Default)]
pub(super) struct Review {
    pub open: bool,
    tab: Tab,
    range: Range,
    found: Option<Found>,
    /// When it was last looked at (seconds), so typing doesn't redo it
    /// at every key.
    at: f64,
    /// The next place shown for each word clicked.
    next: std::collections::HashMap<String, usize>,
}

impl DocView {
    /// The paragraphs looked at.
    fn review_paras(&self, r: Range) -> (usize, usize) {
        let caret = self.sel.cursor.para;
        let level = match r {
            Range::Book => return (0, self.doc.paragraphs.len()),
            Range::Chapter => 1,
            Range::Scene => 2,
        };
        self.doc.headings().into_iter().take_while(|&(i, _)| i <= caret).filter(|&(_, l)| l <= level).last().filter(|h| h.1 == level).map_or((0, self.doc.paragraphs.len()), |(h, _)| (h, self.doc.section_end(h)))
    }

    pub(super) fn review_lang(&self) -> String {
        let l = self.doc.language().map(|l| l.to_string()).unwrap_or_else(crate::spell::language);
        l.split(['-', '_']).next().unwrap_or("").to_lowercase()
    }

    fn find_review(&self, paras: (usize, usize)) -> Found {
        let lang = self.review_lang();
        let r = paras.0..paras.1;
        let dialogue = self.doc.book_nodes().into_iter().filter(|n| n.level == 2 && n.para >= paras.0 && n.para < paras.1).map(|n| (n.para, n.title.clone(), self.doc.dialogue_share(n.para..n.end))).collect();
        let repeats = self.doc.repeats(r.clone(), WINDOW, MIN_LEN, &lang);
        let mut repeated: Vec<(String, Vec<Hit>)> = Vec::new();
        for x in &repeats {
            let key = crate::palette::fold(&x.word);
            match repeated.iter_mut().find(|g| crate::palette::fold(&g.0) == key) {
                Some(g) => g.1.push(x.again),
                None => repeated.push((x.word.to_lowercase(), vec![x.again])),
            }
        }
        repeated.sort_by_key(|g| std::cmp::Reverse(g.1.len()));
        Found {
            rev: self.doc.revision,
            paras,
            repeated,
            repeats,
            fillers: self.doc.fillers(r.clone(), fillers_of(&lang)),
            dialogue,
            names: self.doc.name_variants(r),
        }
    }

    /// Selects a hit in the text (the whole book shown if it was hidden).
    fn show_hit(&mut self, h: Hit) {
        let Some(p) = self.doc.paragraphs.get(h.para) else { return };
        let fit = |i: usize| {
            let mut i = i.min(p.text.len());
            while !p.text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let (s, e) = (fit(h.start), fit(h.end));
        if self.scope_hidden().is_some_and(|x| x.get(h.para).copied().unwrap_or(false)) {
            self.show_whole_book();
        }
        self.book.view = super::book_ui::BookView::Text;
        self.sel = Selection { anchor: Pos::new(h.para, s), cursor: Pos::new(h.para, e) };
        self.scroll_to_caret = true;
    }

    pub(super) fn toggle_review(&mut self) {
        self.book.review.open = !self.book.review.open;
        self.book.review.found = None;
    }

    pub(super) fn review_pane(&mut self, ui: &mut egui::Ui) {
        if !self.is_book() || !self.book.review.open {
            return;
        }
        let mut rv = std::mem::take(&mut self.book.review);
        let now = ui.input(|i| i.time);
        let paras = self.review_paras(rv.range);
        let stale = rv.found.as_ref().is_none_or(|f| f.paras != paras || f.rev != self.doc.revision);
        if stale && (rv.found.is_none() || now - rv.at > 1.5) {
            rv.found = Some(self.find_review(paras));
            rv.at = now;
        } else if stale {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(1600));
        }
        let mut go: Option<Hit> = None;
        let mut scene: Option<usize> = None;
        let frame = egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 6));
        egui::Panel::right(Id::new(("book_review", self.uid))).resizable(true).default_size(290.0).frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("book.review")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").on_hover_text(tr("book.close")).clicked() {
                        rv.open = false;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(tr("book.review_in"));
                for (r, k) in [(Range::Book, "book.tab"), (Range::Chapter, "book.col_chapter"), (Range::Scene, "book.col_scene")] {
                    ui.selectable_value(&mut rv.range, r, tr(k));
                }
            });
            ui.horizontal_wrapped(|ui| {
                for (t, k) in [(Tab::Repeats, "book.r_repeats"), (Tab::Fillers, "book.r_fillers"), (Tab::Dialogue, "book.r_dialogue"), (Tab::Names, "book.r_names")] {
                    ui.selectable_value(&mut rv.tab, t, tr(k));
                }
            });
            ui.separator();
            let Some(f) = rv.found.as_ref() else { return };
            let title = |p: usize| self.doc.book_nodes().into_iter().filter(|n| n.para <= p).last().map(|n| n.title).unwrap_or_default();
            egui::ScrollArea::vertical().id_salt("book_review_list").auto_shrink([false, false]).show(ui, |ui| match rv.tab {
                Tab::Repeats => {
                    ui.label(RichText::new(trf("book.r_repeats_n", &[("n", &f.repeats.len()), ("w", &WINDOW)])).weak().small());
                    if f.repeats.is_empty() {
                        ui.label(tr("book.r_none"));
                    }
                    for (w, hits) in &f.repeated {
                        let first = hits.first().map_or(String::new(), |h| title(h.para));
                        let t = format!("{w}  ×{} · {first}", hits.len());
                        if ui.selectable_label(false, t).on_hover_text(tr("book.r_next")).clicked() {
                            let k = rv.next.entry(format!("r:{w}")).or_default();
                            go = hits.get(*k % hits.len()).copied();
                            *k += 1;
                        }
                    }
                }
                Tab::Fillers => {
                    if fillers_of(&self.review_lang()).is_empty() {
                        ui.label(RichText::new(tr("book.r_no_list")).weak());
                    } else if f.fillers.is_empty() {
                        ui.label(tr("book.r_none"));
                    }
                    for (w, hits) in &f.fillers {
                        ui.horizontal(|ui| {
                            if ui.selectable_label(false, format!("{w}  ×{}", hits.len())).on_hover_text(tr("book.r_next")).clicked() {
                                let k = rv.next.entry(w.clone()).or_default();
                                go = hits.get(*k % hits.len()).copied();
                                *k += 1;
                            }
                        });
                    }
                }
                Tab::Dialogue => {
                    if f.dialogue.is_empty() {
                        ui.label(tr("book.r_none"));
                    }
                    for (p, t, share) in &f.dialogue {
                        let r = ui.add(egui::ProgressBar::new(*share).text(format!("{t} · {:.0} %", share * 100.0)).desired_height(18.0));
                        if r.interact(Sense::click()).on_hover_text(tr("book.r_open_scene")).clicked() {
                            scene = Some(*p);
                        }
                    }
                }
                Tab::Names => {
                    ui.label(RichText::new(tr("book.r_names_tip")).weak().small());
                    if f.names.is_empty() {
                        ui.label(tr("book.r_none"));
                    }
                    for g in &f.names {
                        ui.horizontal_wrapped(|ui| {
                            for (w, hits) in g {
                                if ui.selectable_label(false, format!("{w} ({})", hits.len())).on_hover_text(tr("book.r_next")).clicked() {
                                    let k = rv.next.entry(w.clone()).or_default();
                                    go = hits.get(*k % hits.len()).copied();
                                    *k += 1;
                                }
                            }
                        });
                        ui.separator();
                    }
                }
            });
        });
        self.book.review = rv;
        if let Some(h) = go {
            self.show_hit(h);
        }
        if let Some(p) = scene {
            self.go_to_heading(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_review_looks_at_the_range_and_goes_to_hits() {
        let h = |t: &str, l: u8| {
            let mut p = Paragraph::new(t);
            p.style.kind = ParaKind::Heading(l);
            p
        };
        let doc = Document::from_paragraphs(vec![h("Uno", 1), h("A", 2), Paragraph::new("La puerta y la puerta."), h("Dos", 1), h("B", 2), Paragraph::new("—Hola —dijo.")]);
        let mut v = DocView::new(doc);
        v.make_book();
        v.sel = Selection::caret(Pos::new(5, 0));
        assert_eq!(v.review_paras(Range::Chapter), (3, 6));
        assert_eq!(v.review_paras(Range::Scene), (4, 6));
        let f = v.find_review(v.review_paras(Range::Book));
        assert_eq!(f.repeats.len(), 1);
        assert_eq!(f.repeated, vec![("puerta".to_string(), vec![f.repeats[0].again])]);
        assert_eq!(f.dialogue.iter().map(|d| (d.1.as_str(), d.2 > 0.9)).collect::<Vec<_>>(), vec![("A", false), ("B", true)]);
        v.show_only(&[4]);
        v.show_hit(f.repeats[0].again);
        assert!(!v.scoped());
        assert_eq!(&v.doc.paragraphs[2].text[v.sel.anchor.offset..v.sel.cursor.offset], "puerta");
    }
}
