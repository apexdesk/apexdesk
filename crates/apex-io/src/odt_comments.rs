//! Comments of odt documents: `office:annotation` where the range starts
//! and `office:annotation-end` where it ends (a point comment has no end).
//! Replies and resolved threads use LibreOffice's `loext:parent-name` and
//! `loext:resolved`.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_doc::{Comment, Document, Mark, Paragraph, Reply, comment_id, comment_mark};

use crate::xml::{Ev, Reader, Tag, esc};

struct Raw {
    name: Option<String>,
    parent: Option<String>,
    comment: Comment,
}

/// Annotations met while reading the body.
#[derive(Default)]
pub(crate) struct Reading {
    raw: Vec<Raw>,
    /// Annotations read that aren't replies.
    roots: u32,
    /// Our ids by annotation name (not for replies).
    ids: HashMap<String, u32>,
    /// Those whose end was met (one end each).
    ended: std::collections::HashSet<u32>,
}

/// The text of the annotation's paragraphs, and its author and date.
fn read_body(r: &mut Reader, c: &mut Comment) {
    let mut depth = 1;
    let mut field = 0u8; // 1 creator, 2 date, 3 paragraph text
    let mut paras = 0;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return,
            Ev::Start(t) | Ev::Empty(t) => {
                let start = matches!(ev, Ev::Start(_));
                match t.name {
                    "dc:creator" if start => field = 1,
                    "dc:date" if start => field = 2,
                    "text:p" | "text:h" => {
                        if paras > 0 {
                            c.text.push('\n');
                        }
                        paras += 1;
                        field = 3;
                    }
                    "text:s" => c.text.push_str(&" ".repeat(t.attr("text:c").and_then(|v| v.parse().ok()).unwrap_or(1usize).min(100))),
                    "text:tab" => c.text.push('\t'),
                    "text:line-break" => c.text.push('\n'),
                    _ => {}
                }
                if start {
                    depth += 1;
                }
            }
            Ev::Text(x) => match field {
                1 => c.author.push_str(x.trim()),
                2 => c.date.push_str(x.trim()),
                3 => c.text.push_str(&x),
                _ => {}
            },
            Ev::End(n) => {
                if matches!(n, "dc:creator" | "dc:date") {
                    field = 0;
                }
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
    }
}

impl Reading {
    /// `office:annotation` (start of the range) or `office:annotation-end`
    /// at the end of `p`'s text so far.
    pub fn element(&mut self, r: &mut Reader, t: &Tag, is_start: bool, p: &mut Paragraph) {
        let name = t.attr("office:name").map(|v| v.into_owned());
        if t.name == "office:annotation-end" {
            if let Some(&id) = name.as_ref().and_then(|n| self.ids.get(n))
                && self.ended.insert(id)
            {
                p.marks.push(Mark { name: comment_mark(id), offset: p.text.len(), end: true });
            }
            if is_start {
                r.skip();
            }
            return;
        }
        let parent = t.attr("loext:parent-name").map(|v| v.into_owned());
        let mut c = Comment { resolved: t.attr("loext:resolved").as_deref() == Some("true"), ..Default::default() };
        if is_start {
            read_body(r, &mut c);
        }
        if parent.is_none() {
            self.roots += 1;
            c.id = self.roots;
            let offset = p.text.len();
            p.marks.push(Mark { name: comment_mark(c.id), offset, end: false });
            match &name {
                Some(n) => {
                    self.ids.insert(n.clone(), c.id);
                }
                // No name, no end: a point comment.
                None => p.marks.push(Mark { name: comment_mark(c.id), offset, end: true }),
            }
        }
        self.raw.push(Raw { name, parent, comment: c });
    }

    /// The comments with their replies folded in.
    pub fn finish(self) -> Vec<Comment> {
        let mut out: Vec<Comment> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut replies = Vec::new();
        for r in self.raw {
            match r.parent {
                Some(p) => replies.push((p, r.name, r.comment)),
                None => {
                    if let Some(n) = r.name {
                        by_name.insert(n, out.len());
                    }
                    out.push(r.comment);
                }
            }
        }
        // Replies to replies belong to the same thread.
        replies.sort_by(|a, b| a.2.date.cmp(&b.2.date));
        let mut pending = replies;
        for _ in 0..8 {
            let mut left = Vec::new();
            for (parent, name, c) in pending {
                match by_name.get(&parent).copied() {
                    Some(k) => {
                        if let Some(n) = name {
                            by_name.insert(n, k);
                        }
                        out[k].replies.push(Reply { author: c.author, date: c.date, text: c.text });
                    }
                    None => left.push((parent, name, c)),
                }
            }
            if left.is_empty() {
                break;
            }
            pending = left;
        }
        out
    }
}

/// ODF dates carry no zone in LibreOffice's files.
fn odf_date(d: &str) -> &str {
    d.strip_suffix('Z').unwrap_or(d)
}

fn name(id: u32, reply: Option<usize>) -> String {
    match reply {
        None => format!("__Annotation__{id}"),
        Some(k) => format!("__Annotation__{id}_{k}"),
    }
}

fn annotation(out: &mut String, id: u32, reply: Option<usize>, author: &str, date: &str, text: &str, resolved: bool) {
    let _ = write!(out, r#"<office:annotation office:name="{}""#, name(id, reply));
    if reply.is_some() {
        let _ = write!(out, r#" loext:parent-name="{}""#, name(id, None));
    }
    let _ = write!(out, r#" loext:resolved="{resolved}"><dc:creator>{}</dc:creator>"#, esc(author));
    if !date.is_empty() {
        let _ = write!(out, "<dc:date>{}</dc:date>", esc(odf_date(date)));
    }
    for line in text.split('\n') {
        out.push_str("<text:p>");
        crate::odt::write_text(out, line);
        out.push_str("</text:p>");
    }
    out.push_str("</office:annotation>");
}

/// The XML written at the start and at the end of each comment's range.
pub(crate) struct Writing {
    xml: HashMap<u32, (String, String)>,
}

impl Writing {
    pub fn new(doc: &Document) -> Writing {
        let mut xml = HashMap::new();
        for c in &doc.comments {
            let mut start = String::new();
            let mut end = String::new();
            annotation(&mut start, c.id, None, &c.author, &c.date, &c.text, c.resolved);
            let _ = write!(end, r#"<office:annotation-end office:name="{}"/>"#, name(c.id, None));
            for (k, r) in c.replies.iter().enumerate() {
                annotation(&mut start, c.id, Some(k + 1), &r.author, &r.date, &r.text, c.resolved);
                let _ = write!(end, r#"<office:annotation-end office:name="{}"/>"#, name(c.id, Some(k + 1)));
            }
            xml.insert(c.id, (start, end));
        }
        Writing { xml }
    }

    /// The XML of a comment mark (empty for other marks or unknown ids).
    pub fn mark(&self, mark_name: &str, end: bool) -> &str {
        match comment_id(mark_name).and_then(|id| self.xml.get(&id)) {
            Some((s, e)) => if end { e } else { s },
            None => "",
        }
    }
}
