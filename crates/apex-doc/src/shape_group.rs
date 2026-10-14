//! Grouped shapes: floating shapes that move, line up and are saved as
//! one object. The members stay separate shapes sharing a group id, at
//! the start of one paragraph and placed from the same frame.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{DocShape, Document, EditKind, OBJECT, Pos, Selection};

/// A group id for readers, unique in the process; see
/// [`Document::tidy_shape_groups`].
pub fn fresh_group_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1 << 20);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Document {
    /// The shapes of the group of the shape at `at` (just that shape when
    /// it isn't grouped), in document order.
    pub fn shape_group(&self, at: Pos) -> Vec<(Pos, Arc<DocShape>)> {
        let Some((_, s)) = self.shape_at(at) else { return Vec::new() };
        match s.group {
            Some(g) => self.shapes().into_iter().filter(|(_, x)| x.group == Some(g)).collect(),
            None => vec![(at, s)],
        }
    }

    /// Groups `members` (with their new look, placed from paragraph `to`):
    /// they move to the start of paragraph `to`, in order, sharing a new
    /// group id. One undo step. Returns where they are now.
    pub fn group_shapes(&mut self, to: usize, members: Vec<(Pos, DocShape)>) -> Vec<Pos> {
        let mut seen = std::collections::HashSet::new();
        if members.len() < 2 || to >= self.paragraphs.len() || members.iter().any(|(p, _)| self.shape_at(*p).is_none() || !seen.insert(*p)) {
            return Vec::new();
        }
        let id = self.shapes().iter().filter_map(|(_, s)| s.group).max().map_or(1, |g| g + 1);
        let lo = members.iter().map(|(p, _)| p.para).min().unwrap_or(to).min(to);
        let hi = members.iter().map(|(p, _)| p.para).max().unwrap_or(to).max(to);
        let before = self.paragraphs[lo..=hi].to_vec();
        let items: Vec<crate::Attrs> = members
            .iter()
            .map(|(p, s)| {
                let mut a = self.paragraphs[p.para].attrs_at(p.offset + OBJECT.len_utf8());
                a.shape = Some(Arc::new(DocShape { group: Some(id), ..s.clone() }));
                a
            })
            .collect();
        let mut from: Vec<Pos> = members.iter().map(|(p, _)| *p).collect();
        from.sort_by(|a, b| b.cmp(a));
        for p in &from {
            self.paragraphs[p.para].remove(p.offset, p.offset + OBJECT.len_utf8());
        }
        for a in items.iter().rev() {
            self.paragraphs[to].insert_str(0, &OBJECT.to_string(), a.clone());
        }
        let at = Pos::new(to, 0);
        let after = Selection { anchor: at, cursor: Pos::new(to, OBJECT.len_utf8()) };
        self.record(lo, before, hi - lo + 1, Selection::caret(members[0].0), after, EditKind::Shape);
        (0..items.len()).map(|i| Pos::new(to, i * OBJECT.len_utf8())).collect()
    }

    /// Puts `made` in place of the shapes at `gone` (Merge Shapes): they go
    /// to the start of paragraph `to`, in order. One undo step. Returns
    /// where they are.
    pub fn replace_shapes(&mut self, to: usize, gone: &[Pos], made: Vec<DocShape>) -> Vec<Pos> {
        let mut seen = std::collections::HashSet::new();
        if gone.is_empty() || made.is_empty() || to >= self.paragraphs.len() || gone.iter().any(|p| self.shape_at(*p).is_none() || !seen.insert(*p)) {
            return Vec::new();
        }
        let lo = gone.iter().map(|p| p.para).min().unwrap_or(to).min(to);
        let hi = gone.iter().map(|p| p.para).max().unwrap_or(to).max(to);
        let before = self.paragraphs[lo..=hi].to_vec();
        let mut attrs = self.paragraphs[gone[0].para].attrs_at(gone[0].offset + OBJECT.len_utf8());
        let mut from = gone.to_vec();
        from.sort_by(|a, b| b.cmp(a));
        for p in &from {
            self.paragraphs[p.para].remove(p.offset, p.offset + OBJECT.len_utf8());
        }
        for s in made.iter().rev() {
            attrs.shape = Some(Arc::new(s.clone()));
            self.paragraphs[to].insert_str(0, &OBJECT.to_string(), attrs.clone());
        }
        let at = Pos::new(to, 0);
        let after = Selection { anchor: at, cursor: Pos::new(to, OBJECT.len_utf8()) };
        self.record(lo, before, hi - lo + 1, Selection::caret(gone[0]), after, EditKind::Shape);
        (0..made.len()).map(|i| Pos::new(to, i * OBJECT.len_utf8())).collect()
    }

    /// Ungroups the group of the shape at `at`. One undo step.
    pub fn ungroup_shapes(&mut self, at: Pos) -> bool {
        let members = self.shape_group(at);
        if members.len() < 2 {
            return false;
        }
        let changes = members.into_iter().map(|(p, s)| (p, DocShape { group: None, ..(*s).clone() })).collect();
        self.change_shapes(changes, Selection::caret(at));
        true
    }

    /// Numbers groups from 1 and drops groups of one shape (after
    /// reading). For readers.
    pub fn tidy_shape_groups(&mut self) {
        let shapes = self.shapes();
        let mut count: HashMap<u32, usize> = HashMap::new();
        for (_, s) in &shapes {
            if let Some(g) = s.group {
                *count.entry(g).or_default() += 1;
            }
        }
        let mut ids: HashMap<u32, u32> = HashMap::new();
        for (p, s) in shapes {
            let Some(g) = s.group else { continue };
            let new = if count[&g] < 2 {
                None
            } else {
                let n = ids.len() as u32 + 1;
                Some(*ids.entry(g).or_insert(n))
            };
            if new != s.group {
                self.paragraphs[p.para].map_attrs(p.offset, p.offset + OBJECT.len_utf8(), &|a| {
                    if let Some(s) = &a.shape {
                        a.shape = Some(Arc::new(DocShape { group: new, ..(**s).clone() }));
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DocShape, Document, Paragraph, Pos, Selection, Shape, ShapeKind, ShapeWrap};

    #[test]
    fn group_and_ungroup() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("uno"), Paragraph::new("dos")]);
        for i in 0..2 {
            let mut s = DocShape::new(Shape::new(ShapeKind::Rect), 50.0, 20.0);
            s.wrap = ShapeWrap::InFront;
            d.insert_shape(Selection::caret(Pos::new(i, 0)), s);
        }
        let members: Vec<_> = d.shapes().into_iter().map(|(p, s)| (p, (*s).clone())).collect();
        let at = d.group_shapes(0, members);
        assert_eq!(at, vec![Pos::new(0, 0), Pos::new(0, 3)]);
        assert_eq!(d.paragraphs[1].text, "dos");
        assert_eq!(d.shape_group(Pos::new(0, 3)).len(), 2);
        assert!(d.ungroup_shapes(Pos::new(0, 0)));
        assert_eq!(d.shape_group(Pos::new(0, 3)).len(), 1);
        d.undo();
        d.undo();
        assert_eq!(d.paragraphs[1].text.chars().count(), 4);
    }
}
