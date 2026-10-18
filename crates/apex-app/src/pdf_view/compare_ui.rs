//! Comparing the open PDF with another version of it: the words of both
//! files are diffed, and what was added, removed or changed is listed in a
//! pane at the right and marked on the pages of the open file.

use apex_doc::DiffOp;
use apex_io::pdf::Page;
use eframe::egui::{self, Color32};

use super::{PdfView, Selection};
use crate::app::Notice;
use crate::i18n::{tr, trf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
}

/// A run of words that differs. Its place is on the open file's pages:
/// the added or changed words, or the word a removal comes before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub page: usize,
    pub a: usize,
    pub b: usize,
    pub old: String,
    pub new: String,
}

pub struct Comparison {
    /// The other file's name.
    pub other: String,
    pub changes: Vec<Change>,
    /// The file's revision the places are of.
    rev: u64,
    pub cur: usize,
}

/// A word: page, first and last char, text.
struct Word {
    page: usize,
    a: usize,
    b: usize,
    text: String,
}

fn words(pages: &[Page]) -> Vec<Word> {
    let mut out = Vec::new();
    for (pi, p) in pages.iter().enumerate() {
        let mut cur: Option<Word> = None;
        for (i, c) in p.chars.iter().enumerate() {
            if c.hidden || c.ch.is_whitespace() {
                out.extend(cur.take());
                continue;
            }
            match &mut cur {
                Some(w) => {
                    w.b = i;
                    w.text.push(c.ch);
                }
                None => cur = Some(Word { page: pi, a: i, b: i, text: c.ch.to_string() }),
            }
        }
        out.extend(cur.take());
    }
    out
}

/// The changes from `old` to `new` (places on `new`'s pages).
pub fn compare(old: &[Page], new: &[Page]) -> Vec<Change> {
    let (ow, nw) = (words(old), words(new));
    let ot: Vec<&str> = ow.iter().map(|w| w.text.as_str()).collect();
    let nt: Vec<&str> = nw.iter().map(|w| w.text.as_str()).collect();
    let ops = apex_doc::diff(&ot, &nt, 16_000_000);
    let mut out = Vec::new();
    let (mut dels, mut ins): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    // The new word after the last one kept, where a removal is marked.
    let mut next_new = 0;
    let flush = |dels: &mut Vec<usize>, ins: &mut Vec<usize>, next_new: usize, out: &mut Vec<Change>| {
        if dels.is_empty() && ins.is_empty() {
            return;
        }
        let join = |ws: &[Word], ix: &[usize]| ix.iter().map(|&i| ws[i].text.as_str()).collect::<Vec<_>>().join(" ");
        let (old, new) = (join(&ow, dels), join(&nw, ins));
        let kind = match (dels.is_empty(), ins.is_empty()) {
            (false, false) => ChangeKind::Changed,
            (true, _) => ChangeKind::Added,
            _ => ChangeKind::Removed,
        };
        // Changed runs that cross pages are marked on the first page.
        let (page, a, b) = match ins.first() {
            Some(&f) => {
                let last = ins.iter().rev().find(|&&i| nw[i].page == nw[f].page).copied().unwrap_or(f);
                (nw[f].page, nw[f].a, nw[last].b)
            }
            None => match nw.get(next_new).or(nw.last()) {
                Some(w) => (w.page, w.a, w.a),
                None => (0, 0, 0),
            },
        };
        out.push(Change { kind, page, a, b, old, new });
        dels.clear();
        ins.clear();
    };
    for op in ops {
        match op {
            DiffOp::Same(_, j) => {
                next_new = j;
                flush(&mut dels, &mut ins, next_new, &mut out);
                next_new = j + 1;
            }
            DiffOp::Del(i) => dels.push(i),
            DiffOp::Ins(j) => ins.push(j),
        }
    }
    flush(&mut dels, &mut ins, next_new, &mut out);
    out
}

fn color(k: ChangeKind) -> Color32 {
    match k {
        ChangeKind::Added => Color32::from_rgba_unmultiplied(40, 180, 60, 90),
        ChangeKind::Removed => Color32::from_rgb(220, 40, 40),
        ChangeKind::Changed => Color32::from_rgba_unmultiplied(255, 150, 0, 100),
    }
}

impl PdfView {
    /// Compares the open file (the newer version) with `other`.
    pub fn start_compare(&mut self, name: String, other: &[u8], password: Option<&str>) -> Result<usize, apex_io::Error> {
        let old = apex_io::pdf::text_pages(other, password)?;
        let bytes = self.bytes();
        let new = apex_io::pdf::text_pages(&bytes, self.password.as_deref())?;
        let changes = compare(&old, &new);
        let n = changes.len();
        if n == 0 {
            self.notices.push(Notice::info(tr("pdf.compare_same")));
            self.compare = None;
        } else {
            self.compare = Some(Comparison { other: name, changes, rev: self.doc.revision, cur: 0 });
            self.show_change(0);
        }
        Ok(n)
    }

    fn show_change(&mut self, k: usize) {
        let Some(c) = self.compare.as_mut() else { return };
        let Some(ch) = c.changes.get(k) else { return };
        c.cur = k;
        let page = ch.page;
        let top = self.text.page(page).and_then(|p| p.chars.get(ch.a)).map_or(0.0, |c| (c.y - c.size * 2.0).max(0.0));
        self.go_to(page, top);
    }

    /// Marks the changes on page `i`.
    pub(super) fn paint_changes(&self, painter: &egui::Painter, map: &super::pages::PageMap, i: usize) {
        let Some(c) = self.compare.as_ref().filter(|c| c.rev == self.doc.revision) else { return };
        for (k, ch) in c.changes.iter().enumerate().filter(|(_, ch)| ch.page == i) {
            if ch.kind == ChangeKind::Removed {
                let Some(ch0) = self.text.page(i).and_then(|p| p.chars.get(ch.a)) else { continue };
                let r = map.rect_to_screen(super::text::char_box(ch0));
                painter.line_segment([r.left_top(), r.left_bottom()], egui::Stroke::new(if k == c.cur { 3.0 } else { 2.0 }, color(ch.kind)));
            } else {
                self.paint_chars(painter, map, Selection { page: i, a: ch.a, b: ch.b }, color(ch.kind));
                if k == c.cur {
                    let Some(p) = self.text.page(i) else { continue };
                    let boxes = p.chars.get(ch.a..=ch.b.min(p.chars.len().saturating_sub(1))).unwrap_or(&[]).iter().map(|c| map.rect_to_screen(super::text::char_box(c)));
                    if let Some(r) = boxes.reduce(|x, y| x.union(y)) {
                        painter.rect_stroke(r.expand(2.0), 2.0, egui::Stroke::new(1.5, Color32::from_rgb(200, 90, 0)), egui::StrokeKind::Outside);
                    }
                }
            }
        }
    }

    /// The list of changes at the right.
    pub(super) fn compare_pane(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.compare.as_ref() else { return };
        let (n, cur) = (c.changes.len(), c.cur);
        let mut go = None;
        let mut close = false;
        ui.horizontal(|ui| {
            ui.strong(trf("pdf.compare_title", &[("file", &c.other)]));
        });
        ui.label(trf("pdf.compare_count", &[("n", &n)]));
        if c.rev != self.doc.revision {
            ui.colored_label(ui.visuals().warn_fg_color, tr("pdf.compare_stale"));
        }
        ui.horizontal(|ui| {
            if ui.button("◀").on_hover_text(tr("pdf.compare_prev")).clicked() {
                go = Some((cur + n - 1) % n);
            }
            if ui.button("▶").on_hover_text(tr("pdf.compare_next")).clicked() {
                go = Some((cur + 1) % n);
            }
            if ui.button(tr("pdf.compare_close")).clicked() {
                close = true;
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (k, ch) in c.changes.iter().enumerate() {
                let what = match ch.kind {
                    ChangeKind::Added => trf("pdf.compare_added", &[("text", &short(&ch.new))]),
                    ChangeKind::Removed => trf("pdf.compare_removed", &[("text", &short(&ch.old))]),
                    ChangeKind::Changed => trf("pdf.compare_changed", &[("old", &short(&ch.old)), ("new", &short(&ch.new))]),
                };
                let head = trf("pdf.page_n", &[("n", &(ch.page + 1))]);
                let text = egui::RichText::new(format!("{head} · {what}")).color(color(ch.kind).to_opaque());
                if ui.selectable_label(k == cur, text).clicked() {
                    go = Some(k);
                }
            }
        });
        if let Some(k) = go {
            self.show_change(k);
        }
        if close {
            self.compare = None;
        }
    }
}

/// At most 80 characters of `s`.
fn short(s: &str) -> String {
    if s.chars().count() > 80 { format!("{}…", s.chars().take(80).collect::<String>()) } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(s: &str) -> Page {
        let chars = s.chars().enumerate().map(|(i, ch)| apex_io::pdf::Char { ch, x: i as f32 * 6.0, y: 100.0, w: 6.0, size: 10.0, font: 0, color: 0, upright: true, hidden: false }).collect();
        Page { width: 600.0, height: 800.0, chars, ..Default::default() }
    }

    #[test]
    fn words_added_removed_and_changed() {
        let old = [page("the quick brown fox jumps"), page("over the dog")];
        let new = [page("the quick red fox jumps"), page("over the lazy dog")];
        let ch = compare(&old, &new);
        assert_eq!(ch.len(), 2, "{ch:?}");
        assert_eq!((ch[0].kind, ch[0].old.as_str(), ch[0].new.as_str(), ch[0].page, ch[0].a, ch[0].b), (ChangeKind::Changed, "brown", "red", 0, 10, 12));
        assert_eq!((ch[1].kind, ch[1].new.as_str(), ch[1].page), (ChangeKind::Added, "lazy", 1));
        let ch = compare(&new, &old);
        assert_eq!((ch[1].kind, ch[1].old.as_str(), ch[1].page, ch[1].a), (ChangeKind::Removed, "lazy", 1, 9));
        assert!(compare(&old, &old).is_empty());
    }
}
