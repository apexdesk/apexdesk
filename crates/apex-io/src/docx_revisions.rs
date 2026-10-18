//! Tracked changes of docx documents: runs inside `w:ins` and `w:del`
//! (deleted text in `w:delText`), and moves read as deletions and
//! insertions. A paragraph whose whole text is inserted or deleted also
//! gets its paragraph mark tracked, so Word joins or removes it the same
//! way we do.

use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Paragraph, RevKind, Revision};

use crate::xml::{Tag, esc};

/// Open `w:ins` / `w:del` elements while reading.
#[derive(Default)]
pub(crate) struct Tracks {
    stack: Vec<Arc<Revision>>,
}

impl Tracks {
    pub fn push(&mut self, kind: RevKind, t: &Tag) {
        let author = t.attr("w:author").unwrap_or_default();
        let date = t.attr("w:date").unwrap_or_default();
        self.stack.push(Revision::new(kind, &author, &date));
    }

    pub fn pop(&mut self) {
        self.stack.pop();
    }

    /// The innermost change: a deletion of an insertion is a deletion.
    pub fn current(&self) -> Option<Arc<Revision>> {
        self.stack.last().cloned()
    }
}

/// Revision ids while writing.
#[derive(Default)]
pub(crate) struct Ids(u32);

fn open_tag(rev: &Revision, ids: &mut Ids, empty: bool) -> String {
    ids.0 += 1;
    let tag = if rev.kind == RevKind::Insert { "w:ins" } else { "w:del" };
    let mut s = format!(r#"<{tag} w:id="{}" w:author="{}""#, ids.0, esc(&rev.author));
    if !rev.date.is_empty() {
        let _ = write!(s, r#" w:date="{}""#, esc(&rev.date));
    }
    s.push_str(if empty { "/>" } else { ">" });
    s
}

/// Wraps the runs written to `body` since `start` in the change `rev`;
/// deleted text becomes `w:delText`.
pub(crate) fn wrap(body: &mut String, start: usize, rev: Option<&Arc<Revision>>, ids: &mut Ids) {
    let Some(rev) = rev else { return };
    if start >= body.len() {
        return;
    }
    let tail = body.split_off(start);
    let open = open_tag(rev, ids, false);
    let close = if rev.kind == RevKind::Insert { "</w:ins>" } else { "</w:del>" };
    let mut tail = tail.replace("<w:r>", &format!("{open}<w:r>")).replace("</w:r>", &format!("</w:r>{close}"));
    if rev.kind == RevKind::Delete {
        tail = tail.replace("<w:t ", "<w:delText ").replace("<w:t>", "<w:delText>").replace("</w:t>", "</w:delText>");
    }
    body.push_str(&tail);
}

/// `w:rPr` for the paragraph mark of an inserted or deleted paragraph
/// (goes last in `w:pPr`), or nothing.
pub(crate) fn para_mark(p: &Paragraph, ids: &mut Ids) -> String {
    match p.whole_revision() {
        Some(r) if p.cell.is_none() => format!("<w:rPr>{}</w:rPr>", open_tag(r, ids, true)),
        _ => String::new(),
    }
}
