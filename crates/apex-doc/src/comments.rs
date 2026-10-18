//! Comments: a body in `Document::comments` anchored to a range of the text
//! by a start and an end `Mark` named after the comment, so anchors follow
//! edits and undo like bookmarks do.

use super::*;

/// Mark names of comment anchors start with this (a character that no
/// bookmark name from a file or the user can have).
const PREFIX: &str = "\u{1}c";

pub fn comment_mark(id: u32) -> Arc<str> {
    Arc::from(format!("{PREFIX}{id}").as_str())
}

pub fn is_comment_mark(name: &str) -> bool {
    name.starts_with('\u{1}')
}

/// The comment a mark name anchors.
pub fn comment_id(name: &str) -> Option<u32> {
    name.strip_prefix(PREFIX)?.parse().ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Reply {
    pub author: String,
    /// ISO 8601 ("2026-09-25T10:30:00Z"); may be empty.
    pub date: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Comment {
    pub id: u32,
    pub author: String,
    pub date: String,
    /// Plain text; paragraphs separated by '\n'.
    pub text: String,
    pub resolved: bool,
    pub replies: Vec<Reply>,
}

/// Where a comment is anchored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentSpan {
    pub id: u32,
    pub start: Pos,
    pub end: Pos,
}

impl Document {
    /// Anchors of the comments, in document order.
    pub fn comment_spans(&self) -> Vec<CommentSpan> {
        let mut out: Vec<CommentSpan> = Vec::new();
        let mut index: std::collections::HashMap<u32, usize> = Default::default();
        for (i, p) in self.paragraphs.iter().enumerate() {
            for m in &p.marks {
                let Some(id) = comment_id(&m.name) else { continue };
                let at = Pos::new(i, m.offset);
                let k = *index.entry(id).or_insert_with(|| {
                    out.push(CommentSpan { id, start: at, end: at });
                    out.len() - 1
                });
                let s = &mut out[k];
                if m.end {
                    s.end = at;
                } else {
                    s.start = at;
                }
            }
        }
        for s in &mut out {
            if s.end < s.start {
                std::mem::swap(&mut s.start, &mut s.end);
            }
        }
        out.retain(|s| self.comments.iter().any(|c| c.id == s.id));
        out.sort_by_key(|s| (s.start, s.id));
        out
    }

    pub fn comment(&self, id: u32) -> Option<&Comment> {
        self.comments.iter().find(|c| c.id == id)
    }

    /// For file readers: keeps the comments that have an anchor and the
    /// anchors that have a comment, completing half anchors.
    pub fn set_comments(&mut self, comments: Vec<Comment>) {
        self.comments = comments;
        let ids: std::collections::HashSet<u32> = self.comments.iter().map(|c| c.id).collect();
        let mut seen: std::collections::HashMap<u32, (bool, bool, Pos)> = Default::default();
        for (i, p) in self.paragraphs.iter_mut().enumerate() {
            p.marks.retain(|m| !is_comment_mark(&m.name) || comment_id(&m.name).is_some_and(|id| ids.contains(&id)));
            for m in &mut p.marks {
                if let Some(id) = comment_id(&m.name) {
                    m.offset = floor_boundary(&p.text, m.offset);
                    let e = seen.entry(id).or_insert((false, false, Pos::new(i, m.offset)));
                    if m.end {
                        e.1 = true;
                    } else {
                        e.0 = true;
                    }
                }
            }
        }
        for (id, (start, end, at)) in seen.iter().map(|(k, v)| (*k, *v)) {
            if start != end {
                self.paragraphs[at.para].marks.push(Mark { name: comment_mark(id), offset: at.offset, end: start });
            }
        }
        self.comments.retain(|c| seen.contains_key(&c.id));
    }

    fn next_comment_id(&self) -> u32 {
        self.comments.iter().map(|c| c.id + 1).max().unwrap_or(1)
    }

    /// Records a change of the comment list (and of paragraphs `lo..=hi`,
    /// saved in `before`) as one undo step.
    fn record_comments(&mut self, old: Vec<Comment>, range: Option<(usize, Vec<Paragraph>)>, sel: Selection) {
        let (start, before) = range.unwrap_or((0, Vec::new()));
        let n = before.len();
        self.record(start, before, n, sel, sel, EditKind::Comment);
        if let Some(e) = self.undo.last_mut() {
            e.comments = Some(old);
        }
    }

    /// Adds a comment on the selection (a point comment when it's empty).
    pub fn add_comment(&mut self, sel: Selection, text: &str) -> u32 {
        let (a, b) = (self.clamp(sel.start()), self.clamp(sel.end()));
        let id = self.next_comment_id();
        let old = self.comments.clone();
        let before = self.paragraphs[a.para..=b.para].to_vec();
        let name = comment_mark(id);
        self.paragraphs[a.para].marks.push(Mark { name: name.clone(), offset: a.offset, end: false });
        self.paragraphs[b.para].marks.push(Mark { name, offset: b.offset, end: true });
        self.comments.push(Comment { id, author: self.user.to_string(), date: now_iso(), text: text.to_string(), resolved: false, replies: Vec::new() });
        self.record_comments(old, Some((a.para, before)), sel);
        id
    }

    fn edit_comment(&mut self, id: u32, sel: Selection, f: &dyn Fn(&mut Comment, &str)) -> bool {
        let Some(k) = self.comments.iter().position(|c| c.id == id) else { return false };
        let mut c = self.comments[k].clone();
        f(&mut c, &self.user);
        if c == self.comments[k] {
            return false;
        }
        let old = self.comments.clone();
        self.comments[k] = c;
        self.record_comments(old, None, sel);
        true
    }

    pub fn set_comment_text(&mut self, id: u32, text: &str, sel: Selection) -> bool {
        self.edit_comment(id, sel, &|c, _| c.text = text.to_string())
    }

    pub fn set_comment_resolved(&mut self, id: u32, resolved: bool, sel: Selection) -> bool {
        self.edit_comment(id, sel, &|c, _| c.resolved = resolved)
    }

    pub fn add_reply(&mut self, id: u32, text: &str, sel: Selection) -> bool {
        let date = now_iso();
        self.edit_comment(id, sel, &|c, user| c.replies.push(Reply { author: user.to_string(), date: date.clone(), text: text.to_string() }))
    }

    pub fn set_reply_text(&mut self, id: u32, reply: usize, text: &str, sel: Selection) -> bool {
        self.edit_comment(id, sel, &|c, _| {
            if let Some(r) = c.replies.get_mut(reply) {
                r.text = text.to_string();
            }
        })
    }

    pub fn delete_reply(&mut self, id: u32, reply: usize, sel: Selection) -> bool {
        self.edit_comment(id, sel, &|c, _| {
            if reply < c.replies.len() {
                c.replies.remove(reply);
            }
        })
    }

    /// Deletes comments (and their anchors, not the text) as one step.
    pub fn delete_comments(&mut self, ids: &[u32], sel: Selection) -> bool {
        if !self.comments.iter().any(|c| ids.contains(&c.id)) {
            return false;
        }
        let spans: Vec<CommentSpan> = self.comment_spans().into_iter().filter(|s| ids.contains(&s.id)).collect();
        let old = self.comments.clone();
        let range = match (spans.iter().map(|s| s.start.para).min(), spans.iter().map(|s| s.end.para).max()) {
            (Some(lo), Some(hi)) => {
                let before = self.paragraphs[lo..=hi].to_vec();
                for p in &mut self.paragraphs[lo..=hi] {
                    p.marks.retain(|m| comment_id(&m.name).is_none_or(|id| !ids.contains(&id)));
                }
                Some((lo, before))
            }
            _ => None,
        };
        self.comments.retain(|c| !ids.contains(&c.id));
        self.record_comments(old, range, sel);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(texts: &[&str]) -> Document {
        let mut d = Document::from_paragraphs(texts.iter().map(|t| Paragraph::new(*t)).collect());
        d.user = "Ana".into();
        d
    }

    fn span(d: &Document, id: u32) -> ((usize, usize), (usize, usize)) {
        let s = d.comment_spans().into_iter().find(|s| s.id == id).unwrap();
        ((s.start.para, s.start.offset), (s.end.para, s.end.offset))
    }

    #[test]
    fn comments_follow_edits_and_undo() {
        let mut d = doc(&["hello world", "second"]);
        let sel = Selection { anchor: Pos::new(0, 6), cursor: Pos::new(0, 11) };
        let id = d.add_comment(sel, "Check this");
        let p = d.add_comment(Selection::caret(Pos::new(1, 3)), "");
        assert_eq!(d.comment(id).map(|c| c.author.as_str()), Some("Ana"));
        assert_eq!(d.undo_history()[0], EditKind::Comment);
        // Not bookmarks.
        assert!(d.bookmarks().is_empty());
        d.insert_text(Selection::caret(Pos::new(0, 0)), "Oh, ", None);
        assert_eq!(span(&d, id), ((0, 10), (0, 15)));
        d.new_paragraph(Selection::caret(Pos::new(0, 2)));
        assert_eq!(span(&d, id), ((1, 8), (1, 13)));
        assert_eq!(span(&d, p), ((2, 3), (2, 3)));
        // Text, replies, resolve: each one undo step.
        assert!(d.set_comment_text(id, "Fixed?", Selection::default()));
        assert!(d.add_reply(id, "Yes", Selection::default()));
        assert!(d.set_reply_text(id, 0, "Yes!", Selection::default()));
        assert!(d.set_comment_resolved(id, true, Selection::default()));
        let c = d.comment(id).unwrap();
        assert_eq!((c.text.as_str(), c.replies[0].text.as_str(), c.resolved), ("Fixed?", "Yes!", true));
        d.undo();
        d.undo();
        assert_eq!(d.comment(id).unwrap().replies[0].text, "Yes");
        d.redo();
        assert_eq!(d.comment(id).unwrap().replies[0].text, "Yes!");
        // Deleting removes the anchors; undo brings both back.
        assert!(d.delete_comments(&[id, p], Selection::default()));
        assert!(d.comments.is_empty() && d.comment_spans().is_empty());
        assert!(d.paragraphs.iter().all(|p| p.marks.is_empty()));
        d.undo();
        assert_eq!(d.comment_spans().len(), 2);
        assert_eq!(span(&d, id), ((1, 8), (1, 13)));
        // Undo of the add removes it.
        let mut d = doc(&["x"]);
        d.add_comment(Selection::caret(Pos::new(0, 1)), "a");
        d.undo();
        assert!(d.comments.is_empty() && d.comment_spans().is_empty());
    }

    #[test]
    fn readers_repair_anchors() {
        let mut p = Paragraph::new("abc");
        p.marks.push(Mark { name: comment_mark(3), offset: 1, end: false });
        p.marks.push(Mark { name: comment_mark(9), offset: 2, end: true });
        p.marks.push(Mark { name: "B".into(), offset: 0, end: false });
        let mut d = Document::from_paragraphs(vec![p]);
        d.set_comments(vec![Comment { id: 3, text: "t".into(), ..Default::default() }, Comment { id: 4, ..Default::default() }]);
        // 9 has no body, 4 no anchor, 3 lacked an end.
        assert_eq!(d.comments.len(), 1);
        assert_eq!(span(&d, 3), ((0, 1), (0, 1)));
        assert_eq!(d.bookmarks().len(), 1);
    }
}
