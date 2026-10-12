//! Comments of docx documents: `word/comments.xml` holds the bodies,
//! `word/commentsExtended.xml` (Word 2013+) marks replies (`paraIdParent`)
//! and resolved threads (`done`). Replies are comments of their own in the
//! file, anchored where their parent is.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_doc::{Comment, Document, Mark, Reply, comment_id};

use crate::xml::{Ev, HEADER, Reader, esc, local};
use crate::zip::ZipReader;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";

/// Our comment id for a docx comment id (docx ids start at 0).
pub(crate) fn our_id(docx_id: &str) -> Option<u32> {
    docx_id.trim().parse::<u32>().ok().and_then(|n| n.checked_add(1))
}

/// A comment as read from comments.xml.
struct Raw {
    id: u32,
    author: String,
    date: String,
    text: String,
    /// w14:paraId of its last paragraph (what commentsExtended refers to).
    para_id: Option<String>,
}

fn read_bodies(xml: &str) -> Vec<Raw> {
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    let mut cur: Option<Raw> = None;
    let mut paras = 0;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "comment" => {
                cur = t.attr("w:id").and_then(|i| our_id(&i)).map(|id| Raw {
                    id,
                    author: t.attr("w:author").unwrap_or_default().into_owned(),
                    date: t.attr("w:date").unwrap_or_default().into_owned(),
                    text: String::new(),
                    para_id: None,
                });
                paras = 0;
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "p" => {
                if let Some(c) = &mut cur {
                    if paras > 0 {
                        c.text.push('\n');
                    }
                    paras += 1;
                    if let Some(p) = t.attr("w14:paraId") {
                        c.para_id = Some(p.to_ascii_uppercase());
                    }
                }
            }
            Ev::Start(t) if t.local() == "t" => {
                let s = r.text();
                if let Some(c) = &mut cur {
                    c.text.push_str(&s);
                }
            }
            Ev::Empty(t) if matches!(t.local(), "tab" | "br" | "cr") => {
                if let Some(c) = &mut cur {
                    c.text.push(if t.local() == "tab" { '\t' } else { '\n' });
                }
            }
            Ev::End(n) if local(n) == "comment" => {
                if let Some(c) = cur.take() {
                    out.push(c);
                }
            }
            _ => {}
        }
    }
    out
}

/// commentsExtended: paraId -> (parent paraId, done).
fn read_extended(xml: &str) -> HashMap<String, (Option<String>, bool)> {
    let mut out = HashMap::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "commentEx" => {
                if let Some(p) = t.attr("w15:paraId") {
                    let parent = t.attr("w15:paraIdParent").map(|v| v.to_ascii_uppercase());
                    let done = t.attr("w15:done").is_some_and(|v| v == "1" || v == "true");
                    out.insert(p.to_ascii_uppercase(), (parent, done));
                }
            }
            _ => {}
        }
    }
    out
}

/// The part named in the document relationships, or the usual name.
fn part(z: &ZipReader, rels: &HashMap<String, String>, suffix: &str) -> Option<String> {
    let target = rels.values().find(|t| t.to_ascii_lowercase().ends_with(suffix)).map(|t| {
        let t = t.trim_start_matches('/');
        if t.starts_with("word/") { t.to_string() } else { format!("word/{t}") }
    });
    let name = target.filter(|t| z.contains(t)).unwrap_or_else(|| format!("word/{suffix}"));
    z.read_string(&name).ok()
}

/// Comments with their replies folded in.
pub(crate) fn read(z: &ZipReader, rels: &HashMap<String, String>) -> Vec<Comment> {
    let Some(xml) = part(z, rels, "comments.xml") else { return Vec::new() };
    let raw = read_bodies(&xml);
    let ext = part(z, rels, "commentsextended.xml").map(|x| read_extended(&x)).unwrap_or_default();
    let mut out: Vec<Comment> = Vec::new();
    // paraId -> index in `out` of the thread it belongs to.
    let mut threads: HashMap<String, usize> = HashMap::new();
    for c in raw {
        let info = c.para_id.as_ref().and_then(|p| ext.get(p));
        let parent = info.and_then(|(p, _)| p.as_ref()).and_then(|p| threads.get(p).copied());
        match parent {
            Some(k) => {
                out[k].replies.push(Reply { author: c.author, date: c.date, text: c.text });
                if let Some(p) = c.para_id {
                    threads.insert(p, k);
                }
            }
            None => {
                if let Some(p) = c.para_id {
                    threads.insert(p, out.len());
                }
                let resolved = info.is_some_and(|(_, d)| *d);
                out.push(Comment { id: c.id, author: c.author, date: c.date, text: c.text, resolved, replies: Vec::new() });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Writing

/// Docx ids of each comment and its replies, in document order.
pub(crate) struct Ids {
    ids: HashMap<u32, (u32, usize)>,
}

impl Ids {
    pub fn new(doc: &Document) -> Ids {
        let mut ids = HashMap::new();
        let mut next = 0;
        for s in doc.comment_spans() {
            if let Some(c) = doc.comment(s.id) {
                ids.insert(c.id, (next, c.replies.len()));
                next += 1 + c.replies.len() as u32;
            }
        }
        Ids { ids }
    }

    /// Writes the start or end of a comment's range (and the reference run
    /// after the end); false for marks that aren't comments.
    pub fn mark(&self, body: &mut String, m: &Mark) -> bool {
        let Some(id) = comment_id(&m.name) else { return false };
        let Some(&(first, replies)) = self.ids.get(&id) else { return true };
        for k in first..=first + replies as u32 {
            if m.end {
                let _ = write!(body, r#"<w:commentRangeEnd w:id="{k}"/><w:r><w:commentReference w:id="{k}"/></w:r>"#);
            } else {
                let _ = write!(body, r#"<w:commentRangeStart w:id="{k}"/>"#);
            }
        }
        true
    }
}

fn para_id(n: usize) -> String {
    format!("{:08X}", 0x1000_0000 + n)
}

fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).take(3).collect()
}

/// The comment parts: comments.xml and commentsExtended.xml, or `None`
/// without comments.
pub(crate) fn parts(doc: &Document) -> Option<(String, String)> {
    let ids = Ids::new(doc);
    let mut order: Vec<(u32, &Comment)> = doc.comments.iter().filter_map(|c| ids.ids.get(&c.id).map(|&(k, _)| (k, c))).collect();
    if order.is_empty() {
        return None;
    }
    order.sort_by_key(|(k, _)| *k);
    let mut xml = format!(r#"{HEADER}<w:comments xmlns:w="{W}" xmlns:w14="{W14}">"#);
    let mut ext = format!(r#"{HEADER}<w15:commentsEx xmlns:w15="{W15}">"#);
    let mut paras = 0;
    let mut body = |xml: &mut String, id: u32, author: &str, date: &str, text: &str| -> String {
        let _ = write!(xml, r#"<w:comment w:id="{id}" w:author="{}""#, esc(author));
        if !date.is_empty() {
            let _ = write!(xml, r#" w:date="{}""#, esc(date));
        }
        let _ = write!(xml, r#" w:initials="{}">"#, esc(&initials(author)));
        let lines: Vec<&str> = text.split('\n').collect();
        let mut last = String::new();
        for (i, line) in lines.iter().enumerate() {
            paras += 1;
            last = para_id(paras);
            let _ = write!(xml, r#"<w:p w14:paraId="{last}" w14:textId="{last}">"#);
            if i == 0 {
                xml.push_str("<w:r><w:annotationRef/></w:r>");
            }
            if !line.is_empty() {
                xml.push_str("<w:r>");
                crate::docx::write_text(xml, line);
                xml.push_str("</w:r>");
            }
            xml.push_str("</w:p>");
        }
        xml.push_str("</w:comment>");
        last
    };
    for (k, c) in order {
        let done = u8::from(c.resolved);
        let root = body(&mut xml, k, &c.author, &c.date, &c.text);
        let _ = write!(ext, r#"<w15:commentEx w15:paraId="{root}" w15:done="{done}"/>"#);
        for (i, r) in c.replies.iter().enumerate() {
            let p = body(&mut xml, k + 1 + i as u32, &r.author, &r.date, &r.text);
            let _ = write!(ext, r#"<w15:commentEx w15:paraId="{p}" w15:paraIdParent="{root}" w15:done="{done}"/>"#);
        }
    }
    xml.push_str("</w:comments>");
    ext.push_str("</w15:commentsEx>");
    Some((xml, ext))
}

pub(crate) const CONTENT_TYPES: &str = r#"<Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/><Override PartName="/word/commentsExtended.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml"/>"#;
pub(crate) const RELS: &str = r#"<Relationship Id="rIdC1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/><Relationship Id="rIdC2" Type="http://schemas.microsoft.com/office/2011/relationships/commentsExtended" Target="commentsExtended.xml"/>"#;
