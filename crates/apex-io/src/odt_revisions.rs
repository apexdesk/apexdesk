//! Tracked changes of odt documents. `text:tracked-changes` lists the
//! changed regions: insertions (the text is in the body between
//! `text:change-start` and `text:change-end`, possibly across paragraphs)
//! and deletions (the text is kept in the region, as paragraphs, and the
//! body has a `text:change` where it was). A deletion of n paragraphs puts
//! n-1 paragraph breaks back when rejected, which is how whole deleted
//! paragraphs round-trip.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Attrs, InCell, Mark, Paragraph, RevKind, Revision, Run};

use crate::xml::{Ev, Reader, Tag, esc};

struct Region {
    rev: Option<Arc<Revision>>,
    deleted: Vec<Paragraph>,
}

/// Reading state: the regions, and the change marks of the paragraph being
/// read.
#[derive(Default)]
pub(crate) struct Reading {
    regions: HashMap<String, Region>,
    /// Insertions still open at the end of the previous paragraph.
    open: Vec<String>,
    /// (offset, id, 0 start / 1 end / 2 deletion point)
    marks: Vec<(usize, String, u8)>,
    /// Deletions already put back: a region is one change, however many
    /// `text:change` point at it (a broken file repeating one thousands
    /// of times would copy its text each time).
    placed: std::collections::HashSet<String>,
}

/// Sets the revision of bytes `s..e` of `p`.
fn retag(p: &mut Paragraph, s: usize, e: usize, rev: &Option<Arc<Revision>>) {
    if s >= e {
        return;
    }
    let mut runs: Vec<Run> = Vec::with_capacity(p.runs.len() + 2);
    for (rs, re, a) in p.spans() {
        for (x, y, inside) in [(rs, re.min(s), false), (rs.max(s), re.min(e), true), (rs.max(e), re, false)] {
            if x < y {
                let attrs = if inside { Attrs { rev: rev.clone(), ..a.clone() } } else { a.clone() };
                match runs.last_mut() {
                    Some(l) if l.attrs == attrs => l.len += y - x,
                    _ => runs.push(Run { len: y - x, attrs }),
                }
            }
        }
    }
    if !runs.is_empty() {
        p.runs = runs;
    }
}

fn blank(style_of: &Paragraph) -> Paragraph {
    let mut p = Paragraph::new("");
    p.style = style_of.style;
    p.cell = style_of.cell.as_ref().map(InCell::bare);
    p
}

/// Splits `p` at byte `o` (marks at `o` go right, except ends).
fn split(p: Paragraph, o: usize) -> (Paragraph, Paragraph) {
    let (mut a, mut b) = (blank(&p), blank(&p));
    a.cell.clone_from(&p.cell);
    for (s, e, at) in p.spans() {
        if s < o {
            a.push(&p.text[s..e.min(o)], at.clone());
        }
        if e > o {
            b.push(&p.text[s.max(o)..e], at.clone());
        }
    }
    for m in p.marks {
        if m.offset < o || (m.offset == o && m.end) {
            a.marks.push(m);
        } else {
            b.marks.push(Mark { offset: m.offset - o, ..m });
        }
    }
    (a, b)
}

fn concat(mut a: Paragraph, b: Paragraph) -> Paragraph {
    let shift = a.text.len();
    for (s, e, at) in b.spans() {
        a.push(&b.text[s..e], at.clone());
    }
    a.marks.extend(b.marks.into_iter().map(|m| Mark { offset: m.offset + shift, ..m }));
    a
}

impl Reading {
    /// `text:tracked-changes`; `para` reads a paragraph of a deletion.
    pub fn read_regions(&mut self, r: &mut Reader, para: &mut dyn FnMut(&mut Reader, &Tag, bool) -> Vec<Paragraph>) {
        let mut depth = 1;
        let mut id = String::new();
        let mut kind = None;
        let (mut author, mut date) = (String::new(), String::new());
        let mut field = 0u8;
        let mut deleted = Vec::new();
        loop {
            let ev = r.next();
            match ev {
                Ev::Eof => return,
                Ev::Start(t) | Ev::Empty(t) => {
                    let start = matches!(ev, Ev::Start(_));
                    match t.name {
                        "text:changed-region" => {
                            id = t.attr("text:id").or_else(|| t.attr("xml:id")).unwrap_or_default().into_owned();
                            (kind, author, date) = (None, String::new(), String::new());
                            deleted.clear();
                        }
                        "text:insertion" => kind = Some(RevKind::Insert),
                        "text:deletion" => kind = Some(RevKind::Delete),
                        "dc:creator" if start => field = 1,
                        "dc:date" if start => field = 2,
                        "text:p" | "text:h" if kind == Some(RevKind::Delete) => {
                            deleted.extend(para(r, &t, !start));
                            continue;
                        }
                        _ => {}
                    }
                    if start {
                        depth += 1;
                    }
                }
                Ev::Text(x) => match field {
                    1 => author.push_str(x.trim()),
                    2 => date.push_str(x.trim()),
                    _ => {}
                },
                Ev::End(n) => {
                    match n {
                        "dc:creator" | "dc:date" => field = 0,
                        "text:changed-region" if !id.is_empty() => {
                            let rev = kind.map(|k| Revision::new(k, &author, &date));
                            for p in &mut deleted {
                                let len = p.text.len();
                                retag(p, 0, len, &rev);
                            }
                            self.regions.insert(std::mem::take(&mut id), Region { rev, deleted: std::mem::take(&mut deleted) });
                        }
                        _ => {}
                    }
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
            }
        }
    }

    /// `text:change-start`, `text:change-end` or `text:change` at `offset`.
    pub fn element(&mut self, t: &Tag, offset: usize) {
        let Some(id) = t.attr("text:change-id") else { return };
        let what = match t.name {
            "text:change-start" => 0,
            "text:change-end" => 1,
            _ => 2,
        };
        self.marks.push((offset, id.into_owned(), what));
    }

    /// Applies the paragraph's changes: insertions get their revision and
    /// deleted text goes back in, splitting the paragraph where the
    /// deletion had paragraph breaks.
    pub fn finish(&mut self, mut p: Paragraph) -> Vec<Paragraph> {
        if self.marks.is_empty() && self.open.is_empty() {
            return vec![p];
        }
        let mut marks = std::mem::take(&mut self.marks);
        marks.retain(|(_, id, what)| *what != 2 || self.placed.insert(id.clone()));
        let mut open: Vec<(String, usize)> = self.open.drain(..).map(|id| (id, 0)).collect();
        for (off, id, what) in &marks {
            match what {
                // A change left open is looked over in every paragraph
                // after: each once, and a few of them.
                0 if open.len() < 256 && !open.iter().any(|(o, _)| o == id) => open.push((id.clone(), *off)),
                0 => {}
                1 => {
                    if let Some(k) = open.iter().position(|(o, _)| o == id) {
                        let (_, s) = open.remove(k);
                        if let Some(r) = self.regions.get(id) {
                            retag(&mut p, s, *off, &r.rev);
                        }
                    }
                }
                _ => {}
            }
        }
        let len = p.text.len();
        for (id, s) in open {
            if let Some(r) = self.regions.get(&id) {
                retag(&mut p, s, len, &r.rev);
            }
            self.open.push(id);
        }
        // Deletions, from the last one back.
        let mut out = vec![p];
        for (off, id, what) in marks.iter().rev() {
            if *what != 2 {
                continue;
            }
            let Some(r) = self.regions.get(id).filter(|r| !r.deleted.is_empty()) else { continue };
            let first = out.remove(0);
            let o = (*off).min(first.text.len());
            let (head, tail) = split(first, o);
            let n = r.deleted.len();
            let bare = tail.cell.clone();
            let mut pieces = Vec::with_capacity(n + 1);
            let mut lead = concat(head, r.deleted[0].clone());
            if o == 0 && n > 1 {
                // A deleted paragraph of its own before this one.
                lead.style = r.deleted[0].style;
            }
            if n > 1 {
                pieces.push(lead);
                pieces.extend(r.deleted[1..n - 1].iter().map(|d| Paragraph { cell: bare.clone(), ..d.clone() }));
                let last = Paragraph { style: tail.style, cell: bare, ..r.deleted[n - 1].clone() };
                pieces.push(concat(last, tail));
            } else {
                pieces.push(concat(lead, tail));
            }
            pieces.append(&mut out);
            out = pieces;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Writing state: the regions so far and the change open in the text.
#[derive(Default)]
pub(crate) struct Writing {
    regions: String,
    next: u32,
    ins: Option<(Arc<Revision>, String)>,
    /// An open deletion: its revision, id and content so far.
    del: Option<(Arc<Revision>, String, String)>,
    /// Written right after the next paragraph opens.
    pending: String,
}

fn odf_date(d: &str) -> &str {
    d.strip_suffix('Z').unwrap_or(d)
}

impl Writing {
    fn new_id(&mut self) -> String {
        self.next += 1;
        format!("ct{}", self.next)
    }

    fn write_region(&mut self, id: &str, rev: &Revision, content: &str) {
        let tag = if rev.kind == RevKind::Insert { "text:insertion" } else { "text:deletion" };
        let _ = write!(self.regions, r#"<text:changed-region xml:id="{id}" text:id="{id}"><{tag}><office:change-info><dc:creator>{}</dc:creator>"#, esc(&rev.author));
        if !rev.date.is_empty() {
            let _ = write!(self.regions, "<dc:date>{}</dc:date>", esc(odf_date(&rev.date)));
        }
        let _ = write!(self.regions, "</office:change-info>{content}</{tag}></text:changed-region>");
    }

    fn close_del(&mut self) {
        if let Some((rev, id, content)) = self.del.take() {
            self.write_region(&id, &rev, &format!("<text:p>{content}</text:p>"));
        }
    }

    fn close_ins(&mut self, body: &mut String) {
        if let Some((_, id)) = self.ins.take() {
            let _ = write!(body, r#"<text:change-end text:change-id="{id}"/>"#);
        }
    }

    /// Right after a paragraph's start tag.
    pub fn start_para(&mut self, body: &mut String) {
        body.push_str(&std::mem::take(&mut self.pending));
    }

    /// Before the text of a span with revision `rev`: opens and closes
    /// changes. Returns the buffer for deleted text when the span is
    /// deleted (its text goes into the region, not the body).
    pub fn span(&mut self, body: &mut String, rev: Option<&Arc<Revision>>) -> Option<&mut String> {
        let kind = rev.map(|r| r.kind);
        if self.ins.as_ref().is_some_and(|(r, _)| Some(r) != rev) {
            self.close_ins(body);
        }
        if self.del.as_ref().is_some_and(|(r, _, _)| Some(r) != rev) {
            self.close_del();
        }
        match (kind, rev) {
            (Some(RevKind::Insert), Some(r)) if self.ins.is_none() => {
                let id = self.new_id();
                self.write_region(&id, r, "");
                let _ = write!(body, r#"<text:change-start text:change-id="{id}"/>"#);
                self.ins = Some((r.clone(), id));
                None
            }
            (Some(RevKind::Delete), Some(r)) => {
                if self.del.is_none() {
                    let id = self.new_id();
                    let _ = write!(body, r#"<text:change text:change-id="{id}"/>"#);
                    self.del = Some((r.clone(), id, String::new()));
                }
                self.del.as_mut().map(|d| &mut d.2)
            }
            _ => None,
        }
    }

    /// At the end of a paragraph's text. An insertion that covers the whole
    /// paragraph also covers its break when `into_next` (it ends at the
    /// start of the next paragraph).
    pub fn end_para(&mut self, body: &mut String, into_next: bool) {
        self.close_del();
        if into_next {
            if let Some((_, id)) = self.ins.take() {
                let _ = write!(self.pending, r#"<text:change-end text:change-id="{id}"/>"#);
            }
        } else {
            self.close_ins(body);
        }
    }

    /// A whole deleted paragraph, already written as `para_xml`: it goes
    /// into a region with its break, marked at the next paragraph's start.
    pub fn deleted_paragraph(&mut self, rev: &Revision, para_xml: &str) {
        let id = self.new_id();
        self.write_region(&id, rev, &format!("{para_xml}<text:p/>"));
        let _ = write!(self.pending, r#"<text:change text:change-id="{id}"/>"#);
    }

    /// The `text:tracked-changes` element (empty when there's nothing).
    pub fn finish(mut self, body: &mut String, track: bool) -> String {
        self.close_del();
        self.close_ins(body);
        if self.regions.is_empty() && !track {
            return String::new();
        }
        format!(r#"<text:tracked-changes text:track-changes="{track}">{}</text:tracked-changes>"#, self.regions)
    }
}
