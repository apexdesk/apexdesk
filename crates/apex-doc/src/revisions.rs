//! Track changes: insertions and deletions kept as run attributes.
//!
//! Deleted text stays in `Paragraph::text` (so it can be shown struck
//! through) with a `RevKind::Delete` revision on its runs. Paragraph marks
//! aren't tracked; instead, a paragraph whose whole text is inserted counts
//! as an inserted paragraph (rejecting it removes the paragraph) and one
//! whose whole text is deleted as a deleted paragraph (accepting removes it).
//! Formatting changes aren't tracked.

use std::borrow::Cow;
use std::cell::RefCell;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevKind {
    Insert,
    Delete,
}

/// Who changed the text and when (ISO 8601, like "2026-09-25T10:30:00Z").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Revision {
    pub kind: RevKind,
    pub author: Arc<str>,
    pub date: Arc<str>,
}

impl Revision {
    pub fn new(kind: RevKind, author: &str, date: &str) -> Arc<Revision> {
        Arc::new(Revision { kind, author: Arc::from(author), date: Arc::from(date) })
    }
}

/// A run of tracked text: one revision, within one paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub start: Pos,
    pub end: Pos,
    pub rev: Arc<Revision>,
}

/// The current UTC time to the minute, as ISO 8601 (the precision other
/// editors keep; it also lets consecutive typing share one run).
pub fn now_iso() -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:00Z", rem / 3600, rem % 3600 / 60)
}

thread_local! {
    /// The last revisions handed out, so typing reuses one `Arc`.
    static LAST: RefCell<Vec<Arc<Revision>>> = const { RefCell::new(Vec::new()) };
}

fn shared(kind: RevKind, author: &str, date: &str) -> Arc<Revision> {
    LAST.with(|l| {
        let mut l = l.borrow_mut();
        if let Some(r) = l.iter().find(|r| r.kind == kind && &*r.author == author && &*r.date == date) {
            return r.clone();
        }
        let r = Revision::new(kind, author, date);
        l.retain(|x| x.kind != kind);
        l.push(r.clone());
        r
    })
}

impl Attrs {
    pub fn is_deleted(&self) -> bool {
        self.rev.as_ref().is_some_and(|r| r.kind == RevKind::Delete)
    }

    pub fn is_inserted(&self) -> bool {
        self.rev.as_ref().is_some_and(|r| r.kind == RevKind::Insert)
    }
}

impl Paragraph {
    pub fn has_revisions(&self) -> bool {
        self.runs.iter().any(|r| r.attrs.rev.is_some())
    }

    pub fn has_deleted(&self) -> bool {
        self.runs.iter().any(|r| r.len > 0 && r.attrs.is_deleted())
    }

    /// The text of bytes `s..e` without the deleted text.
    pub fn visible(&self, s: usize, e: usize) -> Cow<'_, str> {
        let e = e.min(self.text.len());
        let s = s.min(e);
        if !self.has_deleted() {
            return Cow::Borrowed(&self.text[s..e]);
        }
        let mut out = String::new();
        for (rs, re, a) in self.spans() {
            if !a.is_deleted() && re > s && rs < e {
                out.push_str(&self.text[rs.max(s)..re.min(e)]);
            }
        }
        Cow::Owned(out)
    }

    /// `hay` (the text, maybe lowercased with the same byte offsets) with
    /// deleted text blanked out, so searches skip it.
    pub(crate) fn blank_deleted(&self, mut hay: String) -> String {
        if self.has_deleted() && hay.len() == self.text.len() {
            for (s, e, a) in self.spans() {
                if a.is_deleted() && e > s && hay.is_char_boundary(s) && hay.is_char_boundary(e) {
                    hay.replace_range(s..e, &"\0".repeat(e - s));
                }
            }
        }
        hay
    }

    /// True when the paragraph has text and all of it is tracked as `kind`:
    /// an inserted or deleted paragraph.
    pub fn is_all(&self, kind: RevKind) -> bool {
        !self.text.is_empty() && self.runs.iter().all(|r| r.len == 0 || r.attrs.rev.as_ref().is_some_and(|x| x.kind == kind))
    }

    /// The revision shared by the whole paragraph, if any (see `is_all`).
    pub fn whole_revision(&self) -> Option<&Arc<Revision>> {
        let first = self.runs.iter().find(|r| r.len > 0)?.attrs.rev.as_ref()?;
        self.is_all(first.kind).then_some(first)
    }
}

impl Document {
    /// True if any text is tracked (headers and footers too).
    pub fn has_revisions(&self) -> bool {
        self.paragraphs.iter().any(Paragraph::has_revisions) || self.hf_changes() > 0
    }

    /// Every header and footer story: the document's and each section's.
    pub(crate) fn hf_stories(&self) -> impl Iterator<Item = &crate::headers::Story> {
        self.headers.parts.iter().chain(self.paragraphs.iter().filter_map(|p| p.section.as_deref()).flat_map(|s| s.headers.parts.iter())).flatten()
    }

    pub(crate) fn hf_stories_mut(&mut self) -> impl Iterator<Item = &mut Vec<Paragraph>> {
        self.headers.parts.iter_mut().chain(self.paragraphs.iter_mut().filter_map(|p| p.section.as_deref_mut()).flat_map(|s| s.headers.parts.iter_mut())).flatten().map(Arc::make_mut)
    }

    /// The paragraphs of the footnotes and endnotes (not in the body).
    pub(crate) fn note_paras(&self) -> impl Iterator<Item = &Paragraph> {
        self.paragraphs.iter().flat_map(|p| p.runs.iter().filter_map(|r| r.attrs.note.as_deref())).flat_map(|n| n.paras.iter())
    }

    /// Applies `f` to the paragraphs of every footnote and endnote; the sum
    /// of what it returns (a note is changed where it returns more than 0).
    /// Not an undo step of its own.
    pub(crate) fn edit_notes(&mut self, f: &dyn Fn(&mut Vec<Paragraph>) -> usize) -> usize {
        let mut n = 0;
        for p in &mut self.paragraphs {
            let notes: Vec<(usize, usize, Arc<crate::notes::Note>)> = p.spans().filter_map(|(s, e, a)| a.note.clone().map(|x| (s, e, x))).collect();
            for (s, e, note) in notes {
                let mut m = (*note).clone();
                let k = f(&mut m.paras);
                if k > 0 {
                    n += k;
                    let m = Arc::new(m);
                    p.map_attrs(s, e, &|a| a.note = Some(m.clone()));
                }
            }
        }
        n
    }

    /// Tracked runs in headers, footers and notes (the review pane walks
    /// the body only, but they're saved and accepted all the same).
    pub fn hf_changes(&self) -> usize {
        self.hf_stories().flat_map(|s| s.iter()).chain(self.note_paras()).filter(|p| p.has_revisions()).map(|p| p.spans().filter(|(s, e, a)| e > s && a.rev.is_some()).count()).sum()
    }

    /// Accepts or rejects every change in headers and footers; how many.
    /// Not an undo step of its own (see [`Document::resolve_all`]).
    pub(crate) fn resolve_hf_changes(&mut self, accept: bool) -> usize {
        if self.hf_changes() == 0 {
            return 0;
        }
        self.hf_stories_mut().map(|story| resolve_story(story, accept)).sum::<usize>() + self.edit_notes(&|paras| resolve_story(paras, accept))
    }

    /// Accept All or Reject All: the body and the headers and footers, as
    /// one undo step; how many changes.
    pub fn resolve_all(&mut self, accept: bool, sel: Selection) -> usize {
        let (state, before, steps) = (self.page_state(), self.paragraphs.clone(), self.undo.len());
        let end = self.end();
        let mut n = self.resolve_changes(Pos::new(0, 0), end, accept, sel);
        let hf = self.resolve_hf_changes(accept);
        if hf == 0 {
            return n;
        }
        n += hf;
        // One step putting back the body and the page's headers.
        self.undo.truncate(steps);
        let caret = Selection::caret(self.clamp(sel.cursor));
        let len = self.paragraphs.len();
        self.record(0, before, len, sel, caret, EditKind::Revision);
        if let Some(e) = self.undo.last_mut() {
            e.page = Some(Box::new(state));
        }
        n
    }

    fn rev_now(&self, kind: RevKind) -> Arc<Revision> {
        shared(kind, &self.user, &now_iso())
    }

    /// Marks new text as an insertion while tracking (and drops any
    /// revision it carried otherwise).
    pub(crate) fn stamp(&self, attrs: &mut Attrs) {
        attrs.rev = self.track_changes.then(|| self.rev_now(RevKind::Insert));
    }

    pub(crate) fn stamp_paras(&self, paras: &mut [Paragraph]) {
        let rev = self.track_changes.then(|| self.rev_now(RevKind::Insert));
        for p in paras {
            if rev.is_some() || p.has_revisions() {
                for r in &mut p.runs {
                    r.attrs.rev = rev.clone();
                }
                p.normalize();
            }
        }
    }

    /// Tracked deletion of `a..b`: the text is marked deleted, except the
    /// user's own insertions, which simply go. Returns the range's new ends.
    pub(crate) fn track_delete(&mut self, a: Pos, b: Pos) -> (Pos, Pos) {
        let del = self.rev_now(RevKind::Delete);
        let user = self.user.clone();
        let mut end = b;
        for i in a.para..=b.para {
            let p = &mut self.paragraphs[i];
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { p.len() };
            if s >= e {
                continue;
            }
            let spans: Vec<(usize, usize, Option<Arc<Revision>>)> = p.spans().filter(|(rs, re, _)| re > rs && *re > s && *rs < e).map(|(rs, re, at)| (rs.max(s), re.min(e), at.rev.clone())).collect();
            for (rs, re, rev) in spans.into_iter().rev() {
                match rev {
                    Some(r) if r.kind == RevKind::Delete => {}
                    Some(r) if r.kind == RevKind::Insert && r.author == user => {
                        p.remove(rs, re);
                        if i == b.para {
                            end.offset -= re - rs;
                        }
                    }
                    _ => p.map_attrs(rs, re, &|x| x.rev = Some(del.clone())),
                }
            }
        }
        (a, end)
    }

    /// Delete or Backspace while tracking: the caret goes to the start of
    /// the deleted text (or past it, going forward).
    pub(crate) fn delete_tracked(&mut self, sel: Selection, forward: bool) -> Pos {
        let (a, b) = (sel.start(), sel.end());
        let before = self.paragraphs[a.para..=b.para].to_vec();
        let (s, e) = self.track_delete(a, b);
        let caret = if forward { e } else { s };
        self.record(a.para, before, b.para - a.para + 1, sel, Selection::caret(caret), EditKind::Delete);
        self.clamp(caret)
    }

    /// Every tracked run, in document order (adjacent runs with the same
    /// revision joined).
    pub fn changes(&self) -> Vec<Change> {
        let mut out: Vec<Change> = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if !p.has_revisions() {
                continue;
            }
            for (s, e, a) in p.spans() {
                let Some(rev) = &a.rev else { continue };
                if e == s {
                    continue;
                }
                match out.last_mut() {
                    Some(c) if c.end == Pos::new(i, s) && c.rev == *rev => c.end.offset = e,
                    _ => out.push(Change { start: Pos::new(i, s), end: Pos::new(i, e), rev: rev.clone() }),
                }
            }
        }
        out
    }

    /// The change containing `p` (or just before it).
    pub fn change_at(&self, p: Pos) -> Option<Change> {
        let para = self.paragraphs.get(p.para)?;
        if !para.has_revisions() {
            return None;
        }
        let here: Vec<Change> = self.changes().into_iter().filter(|c| c.start.para == p.para).collect();
        let hit = here.iter().position(|c| c.start <= p && p < c.end).or_else(|| here.iter().position(|c| c.start < p && p <= c.end))?;
        Some(here[hit].clone())
    }

    /// The next (or previous) change from `from`, wrapping around.
    pub fn next_change(&self, from: Pos, forward: bool) -> Option<Change> {
        let all = self.changes();
        if forward {
            all.iter().find(|c| c.start > from).or(all.first()).cloned()
        } else {
            all.iter().rev().find(|c| c.end < from).or(all.last()).cloned()
        }
    }

    /// Accepts or rejects the changes in `a..b` as one undo step: accepted
    /// insertions and rejected deletions become plain text, the rest is
    /// removed, with whole paragraphs when all their text goes. Returns
    /// how many changes were resolved.
    pub fn resolve_changes(&mut self, a: Pos, b: Pos, accept: bool, sel: Selection) -> usize {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        let goes = |k: RevKind| (k == RevKind::Delete) == accept;
        let first = (a.para..=b.para).find(|&i| self.paragraphs[i].has_revisions());
        let Some(first) = first else { return 0 };
        let last = (a.para..=b.para).rev().find(|&i| self.paragraphs[i].has_revisions()).unwrap_or(first);
        let before = self.paragraphs[first..=last].to_vec();
        let n0 = self.paragraphs.len();
        let mut count = 0;
        let mut drop_paras = Vec::new();
        for i in first..=last {
            let p = &mut self.paragraphs[i];
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { p.len() };
            let whole = s == 0 && e >= p.len();
            if whole && p.cell.is_none() && p.whole_revision().is_some_and(|r| goes(r.kind)) {
                count += 1;
                drop_paras.push(i);
                continue;
            }
            let spans: Vec<(usize, usize, RevKind)> = p
                .spans()
                .filter_map(|(rs, re, at)| {
                    let k = at.rev.as_ref()?.kind;
                    match () {
                        _ if re == rs => None,
                        // An empty range resolves the whole change it touches.
                        _ if s == e => (rs <= s && s <= re).then_some((rs, re, k)),
                        _ => (re > s && rs < e).then_some((rs.max(s), re.min(e), k)),
                    }
                })
                .collect();
            for (rs, re, kind) in spans.into_iter().rev() {
                count += 1;
                if goes(kind) {
                    p.remove(rs, re);
                } else {
                    p.map_attrs(rs, re, &|x| x.rev = None);
                }
            }
        }
        // Whole paragraphs, keeping at least one and moving their marks on.
        for &i in drop_paras.iter().rev() {
            if self.paragraphs.len() == 1 {
                let p = &mut self.paragraphs[0];
                let len = p.len();
                p.remove(0, len);
                break;
            }
            let gone = self.paragraphs.remove(i);
            let (to, off) = if i < self.paragraphs.len() { (i, 0) } else { (i - 1, self.paragraphs[i - 1].len()) };
            self.paragraphs[to].marks.extend(gone.marks.into_iter().map(|m| Mark { offset: off, ..m }));
        }
        let after_len = (last + 1 + self.paragraphs.len()).saturating_sub(first + n0);
        let caret = self.clamp(a);
        self.record(first, before, after_len, sel, Selection::caret(caret), EditKind::Revision);
        count
    }

    /// Text between two positions like `text_range`, without deleted text.
    pub fn visible_text_range(&self, a: Pos, b: Pos) -> String {
        let (a, b) = (a.min(b), a.max(b));
        let mut out = String::new();
        for i in a.para..=b.para {
            let p = &self.paragraphs[i];
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { p.len() };
            if i > a.para {
                out.push(self.separator(i));
            }
            push_plain(&mut out, &p.visible(s, e));
        }
        out
    }

    /// The document with every change accepted (or rejected), for the
    /// "No markup" and "Original" views and for comparing.
    pub fn resolved(&self, accept: bool) -> Document {
        let mut d = self.clone();
        let end = d.end();
        let track = std::mem::replace(&mut d.track_changes, false);
        d.resolve_changes(Pos::new(0, 0), end, accept, Selection::default());
        d.resolve_hf_changes(accept);
        d.track_changes = track;
        d.undo.clear();
        d
    }
}

/// Resolves every change of a header or footer story; how many.
fn resolve_story(story: &mut Vec<Paragraph>, accept: bool) -> usize {
    let goes = |k: RevKind| (k == RevKind::Delete) == accept;
    let mut count = 0;
    let mut i = 0;
    while i < story.len() {
        if !story[i].has_revisions() {
            i += 1;
            continue;
        }
        if story.len() > 1 && story[i].cell.is_none() && story[i].whole_revision().is_some_and(|r| goes(r.kind)) {
            story.remove(i);
            count += 1;
            continue;
        }
        let p = &mut story[i];
        let spans: Vec<(usize, usize, RevKind)> = p.spans().filter_map(|(s, e, a)| (e > s).then_some((s, e, a.rev.as_ref()?.kind))).collect();
        for (s, e, kind) in spans.into_iter().rev() {
            count += 1;
            if goes(kind) {
                p.remove(s, e);
            } else {
                p.map_attrs(s, e, &|x| x.rev = None);
            }
        }
        i += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(texts: &[&str]) -> Document {
        let mut d = Document::from_paragraphs(texts.iter().map(|t| Paragraph::new(*t)).collect());
        d.track_changes = true;
        d.user = "Ana".into();
        d
    }

    fn kinds(p: &Paragraph) -> Vec<(&str, Option<RevKind>)> {
        p.spans().filter(|(s, e, _)| e > s).map(|(s, e, a)| (&p.text[s..e], a.rev.as_ref().map(|r| r.kind))).collect()
    }

    #[test]
    fn headers_changes_are_resolved_too() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("body")]);
        let mut p = Paragraph::new("Draft: secret plan");
        p.map_attrs(0, 7, &|a| a.rev = Some(shared(RevKind::Delete, "Ana", "2026-01-01T00:00:00Z")));
        d.headers.parts[1] = Some(Arc::new(vec![p]));
        assert!(d.has_revisions() && d.inspect().revisions == 1);
        assert_eq!(d.resolve_all(true, Selection::default()), 1);
        assert_eq!(d.headers.parts[1].as_ref().unwrap()[0].text, "secret plan");
        assert!(!d.has_revisions());
        d.undo();
        assert_eq!(d.headers.parts[1].as_ref().unwrap()[0].text, "Draft: secret plan");
    }

    #[test]
    fn notes_changes_and_hidden_text_are_found_too() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("body")]);
        let mut p = Paragraph::new("Draft: plan hidden");
        p.map_attrs(0, 7, &|a| a.rev = Some(shared(RevKind::Delete, "Ana", "2026-01-01T00:00:00Z")));
        p.map_attrs(12, 18, &|a| a.set_fx(|f| f.hidden = true));
        d.insert_note_with(Selection::caret(Pos::new(0, 4)), crate::notes::Note { kind: Default::default(), paras: vec![p] });
        let i = d.inspect();
        assert!(d.has_revisions() && i.revisions == 1 && i.hidden == 6, "{i:?}");
        assert_eq!(d.resolve_all(true, Selection::default()), 1);
        assert_eq!(d.notes()[0].1.paras[0].text, "plan hidden");
        d.remove_inspected(crate::Inspect::HiddenText, Selection::default());
        assert_eq!(d.notes()[0].1.paras[0].text, "plan ");
    }

    #[test]
    fn date_format() {
        let d = now_iso();
        assert_eq!(d.len(), 20);
        assert!(d.starts_with("20") && d.ends_with(":00Z") && &d[10..11] == "T");
    }

    #[test]
    fn typing_and_deleting() {
        let mut d = tracked(&["hello world"]);
        let c = d.insert_text(Selection::caret(Pos::new(0, 5)), ",", None);
        let c = d.insert_text(Selection::caret(c), " big", None);
        assert_eq!(kinds(&d.paragraphs[0]), [("hello", None), (", big", Some(RevKind::Insert)), (" world", None)]);
        // Backspace over own insertion removes it; over old text marks it.
        let c = d.backspace(Selection::caret(c), false);
        assert_eq!(d.paragraphs[0].text, "hello, bi world");
        let p = d.backspace(Selection::caret(Pos::new(0, 5)), false);
        assert_eq!(p, Pos::new(0, 4));
        assert_eq!(kinds(&d.paragraphs[0])[1], ("o", Some(RevKind::Delete)));
        // Delete forward goes past the deleted text.
        let p = d.delete_forward(Selection::caret(Pos::new(0, c.offset + 0)), false);
        assert!(p.offset > c.offset);
        assert_eq!(d.word_count(), 2);
        assert_eq!(d.find_all("hello", false).len(), 0);
        assert_eq!(d.find_all("hell", false).len(), 1);
        // Typing over a selection: the old text stays deleted, before the new.
        let mut d = tracked(&["one two"]);
        let c = d.insert_text(Selection { anchor: Pos::new(0, 4), cursor: Pos::new(0, 7) }, "2", None);
        assert_eq!(c, Pos::new(0, 8));
        assert_eq!(kinds(&d.paragraphs[0]), [("one ", None), ("two", Some(RevKind::Delete)), ("2", Some(RevKind::Insert))]);
        assert_eq!(d.visible_text_range(Pos::new(0, 0), d.end()), "one 2");
        d.undo();
        assert_eq!(d.paragraphs[0].text, "one two");
        assert!(!d.has_revisions());
    }

    #[test]
    fn accept_and_reject() {
        let mut d = tracked(&["keep old", "tail"]);
        let c = d.insert_text(Selection { anchor: Pos::new(0, 5), cursor: Pos::new(0, 8) }, "new", None);
        let c = d.insert_text(Selection::caret(c), "\nadded", None);
        d.track_changes = false;
        assert_eq!(d.paragraphs.len(), 3);
        assert!(d.paragraphs[1].is_all(RevKind::Insert));
        assert_eq!(d.changes().len(), 3);
        // Accept the change at a caret, then reject everything else.
        assert_eq!(d.change_at(Pos::new(0, 6)).map(|c| c.rev.kind), Some(RevKind::Delete));
        assert_eq!(d.resolve_changes(Pos::new(0, 6), Pos::new(0, 6), true, Selection::caret(c)), 1);
        assert_eq!(d.paragraphs[0].text, "keep new");
        let end = d.end();
        d.resolve_changes(Pos::new(0, 0), end, false, Selection::default());
        assert_eq!(d.to_plain_text(), "keep \ntail");
        d.undo();
        d.undo();
        assert_eq!(d.changes().len(), 3);
        // Accepting all keeps the inserted paragraph; the resolved copies.
        assert_eq!(d.resolved(true).to_plain_text(), "keep new\nadded\ntail");
        assert_eq!(d.resolved(false).to_plain_text(), "keep old\ntail");
        // A deleted paragraph goes when accepted, with its marks moving on.
        let mut d = tracked(&["a", "gone", "b"]);
        d.add_bookmark("M", Selection::caret(Pos::new(1, 2)));
        d.delete(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 4) });
        assert!(d.paragraphs[1].is_all(RevKind::Delete));
        assert_eq!(d.next_change(Pos::new(0, 0), true).map(|c| c.start), Some(Pos::new(1, 0)));
        let end = d.end();
        d.resolve_changes(Pos::new(0, 0), end, true, Selection::default());
        assert_eq!(d.to_plain_text(), "a\nb");
        assert_eq!(d.bookmark("M").map(|b| b.start), Some(Pos::new(1, 0)));
    }

    #[test]
    fn replace_all_tracked() {
        let mut d = tracked(&["a cat and a cat"]);
        assert_eq!(d.replace_all("cat", "dog", false), 2);
        assert_eq!(d.visible_text_range(Pos::new(0, 0), d.end()), "a dog and a dog");
        assert_eq!(d.resolved(false).to_plain_text(), "a cat and a cat");
    }
}
