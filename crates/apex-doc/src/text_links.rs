//! Linked text boxes: the text that doesn't fit in a text box goes on in
//! the next box of its chain. The first box holds the text; the others
//! only show what flows into them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{DocShape, Document, EditKind, OBJECT, Pos, Selection, ShapeKind};

/// A text box's place in a chain of linked boxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextLink {
    pub chain: u32,
    /// 0 for the box that holds the text.
    pub seq: u16,
}

impl Document {
    /// The boxes of a chain, in order.
    pub fn text_chain(&self, chain: u32) -> Vec<(Pos, Arc<DocShape>)> {
        let mut v: Vec<(Pos, Arc<DocShape>)> = self.shapes().into_iter().filter(|(_, s)| s.shape.link.is_some_and(|l| l.chain == chain)).collect();
        v.sort_by_key(|(_, s)| s.shape.link.map_or(0, |l| l.seq));
        v
    }

    /// Whether the text flowing out of the box at `from` can go on in the
    /// box at `to`: an empty text box outside any chain, while `from` is
    /// the last box of its chain (or in none).
    pub fn can_link_text(&self, from: Pos, to: Pos) -> bool {
        let (Some((_, a)), Some((_, b))) = (self.shape_at(from), self.shape_at(to)) else { return false };
        if from == to || a.shape.kind != ShapeKind::TextBox || b.shape.kind != ShapeKind::TextBox || b.shape.link.is_some() || b.shape.has_text() {
            return false;
        }
        match a.shape.link {
            Some(l) => self.text_chain(l.chain).last().is_some_and(|(p, _)| *p == from),
            None => true,
        }
    }

    /// Changes the shapes at `changes` as one undo step.
    pub fn change_shapes(&mut self, changes: Vec<(Pos, DocShape)>, sel: Selection) {
        self.change_objects(changes, Vec::new(), sel);
    }

    /// Changes the shapes and pictures at their places as one undo step.
    pub fn change_objects(&mut self, shapes: Vec<(Pos, DocShape)>, images: Vec<(Pos, crate::Image)>, sel: Selection) {
        let paras = || shapes.iter().map(|(p, _)| p.para).chain(images.iter().map(|(p, _)| p.para));
        let (Some(lo), Some(hi)) = (paras().min(), paras().max()) else { return };
        let before = self.paragraphs[lo..=hi].to_vec();
        for (p, s) in shapes {
            let s = Arc::new(s);
            self.paragraphs[p.para].map_attrs(p.offset, p.offset + OBJECT.len_utf8(), &|a| a.shape = Some(s.clone()));
        }
        for (p, i) in images {
            let i = Arc::new(i);
            self.paragraphs[p.para].map_attrs(p.offset, p.offset + OBJECT.len_utf8(), &|a| a.image = Some(i.clone()));
        }
        self.record(lo, before, hi - lo + 1, sel, sel, EditKind::Shape);
    }

    /// Links the box at `to` after the one at `from` (see
    /// [`Document::can_link_text`]). One undo step.
    pub fn link_text_boxes(&mut self, from: Pos, to: Pos) -> bool {
        if !self.can_link_text(from, to) {
            return false;
        }
        let (Some((_, a)), Some((_, b))) = (self.shape_at(from), self.shape_at(to)) else { return false };
        let mut changes = Vec::new();
        let link = match a.shape.link {
            Some(l) => l,
            None => {
                let chain = self.shapes().iter().filter_map(|(_, s)| s.shape.link.map(|l| l.chain)).max().map_or(1, |c| c + 1);
                let l = TextLink { chain, seq: 0 };
                let mut a = (*a).clone();
                a.shape.link = Some(l);
                changes.push((from, a));
                l
            }
        };
        let mut b = (*b).clone();
        b.shape.link = Some(TextLink { chain: link.chain, seq: link.seq + 1 });
        b.shape.text.clear();
        changes.push((to, b));
        let sel = Selection { anchor: from, cursor: from };
        self.change_shapes(changes, sel);
        true
    }

    /// Breaks the chain after the box at `at`: the boxes after it leave
    /// the chain (the text stays in the first box). One undo step.
    pub fn break_text_link(&mut self, at: Pos) -> bool {
        let Some((_, s)) = self.shape_at(at) else { return false };
        let Some(link) = s.shape.link else { return false };
        let chain = self.text_chain(link.chain);
        let mut changes: Vec<(Pos, DocShape)> = chain
            .iter()
            .filter(|(_, x)| x.shape.link.is_some_and(|l| l.seq > link.seq))
            .map(|(p, x)| {
                let mut x = (**x).clone();
                x.shape.link = None;
                (*p, x)
            })
            .collect();
        if changes.is_empty() {
            return false;
        }
        // A box left alone is no longer in a chain.
        if chain.iter().all(|(_, x)| x.shape.link.is_some_and(|l| l.seq >= link.seq)) {
            let mut x = (*s).clone();
            x.shape.link = None;
            changes.push((at, x));
        }
        self.change_shapes(changes, Selection { anchor: at, cursor: at });
        true
    }

    /// The first box of the chain of the box at `at` (itself when not
    /// linked): where its text is edited.
    pub fn text_chain_head(&self, at: Pos) -> Pos {
        match self.shape_at(at).and_then(|(_, s)| s.shape.link) {
            Some(l) => self.text_chain(l.chain).first().map_or(at, |(p, _)| *p),
            None => at,
        }
    }

    /// Links boxes by name: pairs of (box, next box). For readers.
    pub fn link_text_boxes_by_name(&mut self, pairs: &[(String, String)]) {
        if pairs.is_empty() {
            return;
        }
        let shapes = self.shapes();
        let by_name: HashMap<&str, Pos> = shapes.iter().filter(|(_, s)| !s.shape.name.is_empty()).map(|(p, s)| (s.shape.name.as_str(), *p)).collect();
        let next: HashMap<Pos, Pos> = pairs.iter().filter_map(|(a, b)| Some((*by_name.get(a.as_str())?, *by_name.get(b.as_str())?))).collect();
        let targets: std::collections::HashSet<Pos> = next.values().copied().collect();
        let mut links: Vec<(Pos, TextLink)> = Vec::new();
        let mut chain = 0;
        for head in next.keys().filter(|p| !targets.contains(p)) {
            chain += 1;
            let mut at = *head;
            let mut seq = 0u16;
            loop {
                links.push((at, TextLink { chain, seq }));
                match next.get(&at) {
                    Some(n) if seq < 1000 => at = *n,
                    _ => break,
                }
                seq += 1;
            }
        }
        for (p, l) in links {
            self.paragraphs[p.para].map_attrs(p.offset, p.offset + OBJECT.len_utf8(), &|a| {
                if let Some(s) = &a.shape {
                    let mut s = (**s).clone();
                    s.shape.link = Some(l);
                    a.shape = Some(Arc::new(s));
                }
            });
        }
    }

    /// Drops chains of a single box (after reading). For readers.
    pub fn tidy_text_links(&mut self) {
        let mut count: HashMap<u32, usize> = HashMap::new();
        for (_, s) in self.shapes() {
            if let Some(l) = s.shape.link {
                *count.entry(l.chain).or_default() += 1;
            }
        }
        for (p, s) in self.shapes() {
            if s.shape.link.is_some_and(|l| count.get(&l.chain).copied().unwrap_or(0) < 2) {
                self.paragraphs[p.para].map_attrs(p.offset, p.offset + OBJECT.len_utf8(), &|a| {
                    if let Some(s) = &a.shape {
                        let mut s = (**s).clone();
                        s.shape.link = None;
                        a.shape = Some(Arc::new(s));
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Paragraph, Shape};

    fn boxes() -> Document {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b"), Paragraph::new("c")]);
        for (i, name) in ["A", "B", "C"].iter().enumerate() {
            let mut s = Shape::new(ShapeKind::TextBox);
            s.name = name.to_string();
            if i == 0 {
                s.set_plain_text("texto largo");
            }
            d.insert_shape(Selection { anchor: Pos::new(i, 0), cursor: Pos::new(i, 0) }, DocShape::new(s, 100.0, 50.0));
        }
        d
    }

    #[test]
    fn link_and_break() {
        let mut d = boxes();
        let (a, b, c) = (Pos::new(0, 0), Pos::new(1, 0), Pos::new(2, 0));
        assert!(!d.can_link_text(b, a), "a box with text can't take more");
        assert!(d.link_text_boxes(a, b));
        assert!(!d.can_link_text(a, c), "only from the last box");
        assert!(d.link_text_boxes(b, c));
        let chain = d.shape_at(a).unwrap().1.shape.link.unwrap().chain;
        assert_eq!(d.text_chain(chain).iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![a, b, c]);
        assert_eq!(d.text_chain_head(c), a);
        assert!(d.break_text_link(b));
        assert_eq!(d.text_chain(chain).len(), 2);
        d.undo();
        assert_eq!(d.text_chain(chain).len(), 3);
        assert!(d.break_text_link(a));
        assert!(d.shapes().iter().all(|(_, s)| s.shape.link.is_none()));
        let mut d = boxes();
        d.link_text_boxes_by_name(&[("A".into(), "B".into()), ("B".into(), "C".into())]);
        assert_eq!(d.shape_at(c).unwrap().1.shape.link.map(|l| l.seq), Some(2));
    }
}
