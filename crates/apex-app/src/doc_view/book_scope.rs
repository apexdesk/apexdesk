//! A book seen in part: one scene, one chapter or several scenes
//! together, edited in place. The rest of the book is laid out empty and
//! the caret stays in what is shown; saving and everything else still see
//! the whole document. A bar above the page shows where it is and goes
//! back to the whole book.

use super::*;

impl DocView {
    /// Paragraph ranges shown, when the book is seen in part.
    pub(super) fn scope_ranges(&self) -> Option<Vec<(usize, usize)>> {
        let anchors = self.book.scope.as_ref()?;
        let mut r: Vec<(usize, usize)> = anchors.iter().filter_map(|a| self.doc.anchor_para(a)).map(|p| (p, self.doc.section_end(p))).collect();
        r.sort_unstable();
        let mut out: Vec<(usize, usize)> = Vec::with_capacity(r.len());
        for (a, b) in r {
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// Which paragraphs are left out, when the book is seen in part.
    pub(super) fn scope_hidden(&self) -> Option<Vec<bool>> {
        let r = self.scope_ranges()?;
        let mut hidden = vec![true; self.doc.paragraphs.len()];
        for (a, b) in r {
            let n = hidden.len();
            hidden[a.min(n)..b.min(n)].fill(false);
        }
        Some(hidden)
    }

    pub(super) fn scoped(&self) -> bool {
        self.book.scope.is_some()
    }

    /// Shows only the sections of headings `paras`.
    pub(super) fn show_only(&mut self, paras: &[usize]) {
        let anchors: Vec<String> = paras.iter().filter_map(|&p| self.doc.heading_anchor(p)).collect();
        let Some(&first) = paras.iter().min() else { return };
        if anchors.is_empty() {
            return;
        }
        self.book.scope = Some(anchors);
        self.book.view = super::book_ui::BookView::Text;
        self.layout.key.0 = u64::MAX;
        self.book.scope_at = None;
        self.go_to_para(first);
    }

    pub(super) fn show_whole_book(&mut self) {
        if self.book.scope.take().is_some() {
            self.layout.key.0 = u64::MAX;
            self.scroll_to_caret = true;
        }
    }

    /// Keeps the caret and the selection in what is shown: a caret moved
    /// out goes on to the next part shown (or back to the one before), and
    /// a selection stays within one part.
    pub(super) fn clamp_scope(&mut self) {
        if self.book.scope.is_none() {
            return;
        }
        let Some(r) = self.scope_ranges() else {
            self.show_whole_book();
            return;
        };
        let len = |doc: &Document, p: usize| doc.paragraphs.get(p).map_or(0, |x| x.text.len());
        let part = |p: Pos| r.iter().position(|&(a, b)| p.para >= a && p.para < b);
        let c = self.sel.cursor;
        let forward = self.book.scope_at.is_none_or(|q| c >= q);
        let cursor = if part(c).is_some() {
            c
        } else if forward && let Some(&(a, _)) = r.iter().find(|x| x.0 > c.para) {
            Pos::new(a, 0)
        } else if let Some(&(_, b)) = r.iter().rev().find(|x| x.1 <= c.para) {
            Pos::new(b - 1, len(&self.doc, b - 1))
        } else {
            Pos::new(r[0].0, 0)
        };
        let k = part(cursor).unwrap_or(0);
        let (ra, rb) = r[k];
        let a = self.sel.anchor;
        let anchor = if part(a) == Some(k) {
            a
        } else if a < cursor {
            Pos::new(ra, 0)
        } else {
            Pos::new(rb - 1, len(&self.doc, rb - 1))
        };
        if anchor != self.sel.anchor || cursor != self.sel.cursor {
            self.sel = Selection { anchor, cursor };
            self.scroll_to_caret = true;
        }
        self.book.scope_at = Some(cursor);
    }

    /// The document to lay out: the rest of the book left empty.
    pub(super) fn scope_layout_doc(&self, hidden: &[bool]) -> Document {
        let paras = self.doc.paragraphs.iter().zip(hidden).map(|(p, &h)| if h { Paragraph::new("") } else { p.clone() }).collect();
        self.doc.layout_copy(paras)
    }

    /// Where the part shown is: "Book › Chapter › Scene", or how many
    /// parts are shown together.
    fn scope_path(&self) -> String {
        let anchors = self.book.scope.as_deref().unwrap_or_default();
        let sep = " › ";
        if anchors.len() > 1 {
            return format!("{}{sep}{}", tr("book.tab"), trf("book.together_n", &[("n", &anchors.len())]));
        }
        let Some(p) = anchors.first().and_then(|a| self.doc.anchor_para(a)) else { return tr("book.tab").to_string() };
        let title = |i: usize| apex_doc::Paragraph::visible(&self.doc.paragraphs[i], 0, self.doc.paragraphs[i].len()).trim().to_string();
        let mut path = vec![tr("book.tab").to_string()];
        if self.doc.paragraphs[p].style.kind == ParaKind::Heading(2)
            && let Some((c, _)) = self.doc.headings().into_iter().take_while(|&(i, _)| i < p).filter(|&(_, l)| l == 1).last()
        {
            path.push(title(c));
        }
        path.push(title(p));
        path.join(sep)
    }

    /// The bar above the page while the book is seen in part.
    pub(super) fn scope_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.scope_path()).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("book.whole_book")).clicked() {
                    self.show_whole_book();
                    self.focus = true;
                }
            });
        });
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
    fn a_scene_is_seen_alone_and_the_caret_stays_in_it() {
        let doc = Document::from_paragraphs(vec![heading("Uno", 1), heading("A", 2), Paragraph::new("a"), heading("B", 2), Paragraph::new("bb"), heading("Dos", 1), heading("C", 2), Paragraph::new("c")]);
        let mut v = DocView::new(doc);
        v.make_book();
        v.show_only(&[3]);
        assert_eq!(v.scope_ranges(), Some(vec![(3, 5)]));
        assert_eq!(v.scope_path(), format!("{} › Uno › B", tr("book.tab")));
        let h = v.scope_hidden().unwrap();
        assert_eq!(h.iter().filter(|x| !**x).count(), 2);
        v.clamp_scope();
        assert_eq!(v.sel.cursor, Pos::new(3, 0));
        // Down past the end comes back to it; select all selects it.
        v.sel = Selection::caret(Pos::new(5, 0));
        v.clamp_scope();
        assert_eq!(v.sel.cursor, Pos::new(4, 2));
        v.sel = Selection { anchor: Pos::new(0, 0), cursor: v.doc.end() };
        v.clamp_scope();
        assert_eq!((v.sel.anchor, v.sel.cursor), (Pos::new(3, 0), Pos::new(4, 2)));
        // Typing adds to it; it follows its heading when it moves.
        v.sel = Selection::caret(Pos::new(4, 2));
        v.insert("\nmás");
        assert_eq!(v.scope_ranges(), Some(vec![(3, 6)]));
        let s = v.doc.move_section(3, 0, v.sel).unwrap();
        v.set_sel(s);
        assert_eq!(v.scope_ranges(), Some(vec![(0, 3)]));
        // Two scenes together: the caret goes from one to the other.
        v.show_whole_book();
        let n = v.doc.book_nodes();
        let (a, c) = (n.iter().find(|x| x.title == "A").unwrap().para, n.iter().find(|x| x.title == "C").unwrap().para);
        v.show_only(&[a, c]);
        assert_eq!(v.scope_ranges().unwrap().len(), 2);
        assert_eq!(v.scope_path(), format!("{} › {}", tr("book.tab"), trf("book.together_n", &[("n", &2)])));
        v.sel = Selection::caret(Pos::new(a + 2, 0));
        v.clamp_scope();
        assert_eq!(v.sel.cursor, Pos::new(c, 0));
        // A selection from one to the other stays in one.
        v.sel = Selection { anchor: Pos::new(a, 0), cursor: Pos::new(c + 1, 1) };
        v.clamp_scope();
        assert_eq!(v.sel.anchor, Pos::new(c, 0));
        // Its headings gone, the whole book shows again.
        v.doc.paragraphs.iter_mut().for_each(|p| p.marks.clear());
        v.clamp_scope();
        assert!(!v.scoped());
    }
}
