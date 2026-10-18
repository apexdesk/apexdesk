//! Threaded comments of modern Excel (`xl/threadedComments/*.xml`, with
//! their authors in `xl/persons/person.xml`), kept as notes with replies.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_sheet::{CellRef, Note, Reply};

use crate::xml::{Ev, HEADER, Reader, esc};

pub const REL_THREADED: &str = "http://schemas.microsoft.com/office/2017/10/relationships/threadedComment";
pub const REL_PERSON: &str = "http://schemas.microsoft.com/office/2017/10/relationships/person";
pub const CT_THREADED: &str = "application/vnd.ms-excel.threadedcomments+xml";
pub const CT_PERSON: &str = "application/vnd.ms-excel.person+xml";
const NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments";

/// Person id -> display name.
pub fn read_persons(xml: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "person" => {
                if let Some(id) = t.attr("id") {
                    m.insert(id.to_ascii_uppercase(), t.attr("displayName").unwrap_or_default().chars().take(255).collect());
                }
            }
            _ => {}
        }
    }
    m
}

/// The threads of a sheet: each first comment with its replies.
pub fn read_threads(xml: &str, persons: &HashMap<String, String>) -> Vec<(CellRef, Note)> {
    let mut out: Vec<(CellRef, Note)> = Vec::new();
    let mut roots: HashMap<String, usize> = HashMap::new();
    let mut replies = 0;
    let mut r = Reader::new(xml);
    loop {
        let t = match r.next() {
            Ev::Eof => break,
            // A sheet's worth of comments.
            Ev::Start(_) if out.len() + replies >= 1 << 18 => break,
            Ev::Start(t) if t.local() == "threadedComment" => t,
            _ => continue,
        };
        let cell = t.attr("ref").and_then(|v| CellRef::parse(&v));
        let id = t.attr("id").unwrap_or_default().to_ascii_uppercase();
        let parent = t.attr("parentId").map(|v| v.to_ascii_uppercase());
        let author = t.attr("personId").and_then(|p| persons.get(&p.to_ascii_uppercase()).cloned()).unwrap_or_default();
        let date = t.attr("dT").map(|d| d.split('.').next().unwrap_or_default().to_string()).unwrap_or_default();
        let done = t.attr("done").is_some_and(|v| v == "1" || v == "true");
        let mut text = String::new();
        let mut depth = 1;
        while depth > 0 {
            match r.next() {
                Ev::Start(x) if x.local() == "text" => text = r.text(),
                Ev::Start(_) => depth += 1,
                Ev::End(_) => depth -= 1,
                Ev::Eof => break,
                _ => {}
            }
        }
        let text = text.replace("\r\n", "\n").trim_end().to_string();
        match parent.and_then(|p| roots.get(&p).copied()) {
            Some(i) => {
                replies += 1;
                out[i].1.replies.push(Reply { text, author, date })
            }
            None => {
                let Some(c) = cell else { continue };
                roots.insert(id, out.len());
                out.push((c, Note { text, author, threaded: true, resolved: done, date, ..Default::default() }));
            }
        }
    }
    out
}

fn guid(a: usize, b: usize) -> String {
    format!("{{{a:08X}-0000-4000-8000-{b:012X}}}")
}

/// The persons part for the authors of threaded comments.
pub fn write_persons(authors: &[String]) -> String {
    let mut s = String::from(HEADER);
    let _ = write!(s, r#"<personList xmlns="{NS}" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#);
    for (i, a) in authors.iter().enumerate() {
        let _ = write!(s, r#"<person displayName="{}" id="{}" userId="{}" providerId="None"/>"#, esc(a), guid(0, i), esc(a));
    }
    s.push_str("</personList>");
    s
}

/// The threaded comments part of sheet `n`, and for each thread the id and
/// the text older Excel versions show in its classic note.
pub fn write_threads(n: usize, notes: &[(CellRef, &Note)], authors: &mut Vec<String>) -> (String, Vec<(String, String)>) {
    let mut person = |a: &str| {
        let i = authors.iter().position(|x| x == a).unwrap_or_else(|| {
            authors.push(a.to_string());
            authors.len() - 1
        });
        guid(0, i)
    };
    let mut s = String::from(HEADER);
    let _ = write!(s, r#"<ThreadedComments xmlns="{NS}" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#);
    let mut legacy = Vec::new();
    let mut k = 0;
    for (c, note) in notes {
        let root = guid(n, k);
        let mut item = |s: &mut String, text: &str, author: &str, date: &str, parent: Option<&str>, done: bool| {
            let id = guid(n, k);
            k += 1;
            let _ = write!(s, r#"<threadedComment ref="{}""#, c.to_a1());
            if !date.is_empty() {
                let _ = write!(s, r#" dT="{}.00""#, esc(date.trim_end_matches('Z')));
            }
            let _ = write!(s, r#" personId="{}" id="{id}""#, person(author));
            if let Some(p) = parent {
                let _ = write!(s, r#" parentId="{p}""#);
            }
            if done {
                s.push_str(r#" done="1""#);
            }
            let _ = write!(s, "><text>{}</text></threadedComment>", esc(text));
        };
        item(&mut s, &note.text, &note.author, &note.date, None, note.resolved);
        let mut shown = format!("[Threaded comment]\n\nYour version of Excel allows you to read this threaded comment; however, any edits to it will get removed if the file is opened in a newer version of Excel. Learn more: https://go.microsoft.com/fwlink/?linkid=870924\n\nComment:\n    {}", note.text);
        for r in &note.replies {
            item(&mut s, &r.text, &r.author, &r.date, Some(&root), false);
            shown.push_str(&format!("\nReply:\n    {}", r.text));
        }
        legacy.push((root, shown));
    }
    s.push_str("</ThreadedComments>");
    (s, legacy)
}
