//! Editing East Asian layout: phonetic guides over a selection, and the
//! ids of combined characters and fitted text.

use std::sync::Arc;

use crate::{Document, Pos, Ruby, Selection};

impl Document {
    /// The text around `at` under one phonetic guide, and the guide.
    pub fn ruby_at(&self, at: Pos) -> Option<(Selection, Arc<Ruby>)> {
        let p = self.paragraphs.get(at.para)?;
        let spans: Vec<_> = p.spans().collect();
        let i = spans.iter().position(|&(s, e, a)| (s <= at.offset && at.offset < e || e == at.offset && at.offset > s) && a.asian.as_ref().is_some_and(|x| x.ruby.is_some()))?;
        let ruby = spans[i].2.asian.as_ref()?.ruby.clone()?;
        let same = |k: usize| spans[k].2.asian.as_ref().is_some_and(|x| x.ruby.as_ref() == Some(&ruby));
        let (mut from, mut to) = (i, i);
        while from > 0 && same(from - 1) {
            from -= 1;
        }
        while to + 1 < spans.len() && same(to + 1) {
            to += 1;
        }
        Some((Selection { anchor: Pos::new(at.para, spans[from].0), cursor: Pos::new(at.para, spans[to].1) }, ruby))
    }

    /// Puts the guide `ruby` over the text of `sel` (`None` takes the
    /// guides off it) as one undo step.
    pub fn set_ruby(&mut self, sel: Selection, ruby: Option<Ruby>) {
        let ruby = ruby.map(|mut r| {
            // Tells it apart from a neighbouring guide with the same text.
            r.seq = self.paragraphs[sel.start().para..=sel.end().para]
                .iter()
                .flat_map(|p| p.runs.iter())
                .filter_map(|r| r.attrs.asian.as_ref()?.ruby.as_ref().map(|r| r.seq))
                .max()
                .unwrap_or(0)
                + 1;
            Arc::new(r)
        });
        self.format_range(sel, &move |a| {
            if ruby.is_some() || a.asian.as_ref().is_some_and(|x| x.ruby.is_some()) {
                a.set_asian(|x| x.ruby = ruby.clone());
            }
        });
    }

    /// An id for new combined characters or fitted text, unused in the
    /// document.
    pub fn next_layout_id(&self) -> i32 {
        let mut max = 0;
        for p in &self.paragraphs {
            for r in &p.runs {
                if let Some(x) = r.attrs.asian.as_deref() {
                    max = max.max(x.combine.as_ref().and_then(|c| c.id).unwrap_or(0)).max(x.fit_text.map_or(0, |f| f.1));
                }
            }
        }
        max + 1
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn guides_are_set_found_and_removed() {
        let mut d = Document::default();
        d.paragraphs = vec![Paragraph::new("今日は漢字です")];
        let sel = Selection { anchor: Pos::new(0, 9), cursor: Pos::new(0, 15) };
        d.set_ruby(sel, Some(Ruby { text: "かんじ".into(), ..Default::default() }));
        let (found, r) = d.ruby_at(Pos::new(0, 12)).unwrap();
        assert_eq!((found.start().offset, found.end().offset), (9, 15));
        assert_eq!(&*r.text, "かんじ");
        assert_eq!(r.seq, 1);
        // Right after the base still finds it; before it doesn't.
        assert!(d.ruby_at(Pos::new(0, 15)).is_some());
        assert!(d.ruby_at(Pos::new(0, 6)).is_none());
        d.set_ruby(found, None);
        assert!(d.ruby_at(Pos::new(0, 12)).is_none());
        assert!(d.paragraphs[0].runs.iter().all(|r| r.attrs.asian.is_none()));
        d.undo();
        assert!(d.ruby_at(Pos::new(0, 12)).is_some());
    }

    #[test]
    fn layout_ids_are_fresh() {
        let mut d = Document::default();
        d.paragraphs = vec![Paragraph::new("東京大阪")];
        assert_eq!(d.next_layout_id(), 1);
        let sel = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 6) };
        d.format_range(sel, &|a| a.set_asian(|x| x.fit_text = Some((1260, 4))));
        assert_eq!(d.next_layout_id(), 5);
    }
}
