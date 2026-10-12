//! How pages connect: links to other pages (with what the link means),
//! pages that name another without linking it, and `#tags`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use super::Notebook;
use super::search::{find_all, fold};
use crate::{Document, Pos, Selection};

/// What a link to a page points at: "note:12", or "note:12?rel=supports".
pub const SCHEME: &str = "note:";

/// What a link says about the two pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rel {
    #[default]
    Link,
    Supports,
    Contradicts,
    Expands,
    Example,
    Question,
    Causes,
}

impl Rel {
    pub const ALL: [Rel; 7] = [Rel::Link, Rel::Supports, Rel::Contradicts, Rel::Expands, Rel::Example, Rel::Question, Rel::Causes];

    pub fn key(self) -> &'static str {
        match self {
            Rel::Link => "link",
            Rel::Supports => "supports",
            Rel::Contradicts => "contradicts",
            Rel::Expands => "expands",
            Rel::Example => "example",
            Rel::Question => "question",
            Rel::Causes => "causes",
        }
    }

    pub fn from_key(s: &str) -> Rel {
        Rel::ALL.into_iter().find(|r| r.key() == s).unwrap_or_default()
    }
}

/// The target of a link to page `id`.
pub fn target(id: i64, rel: Rel) -> Arc<str> {
    if rel == Rel::Link { format!("{SCHEME}{id}").into() } else { format!("{SCHEME}{id}?rel={}", rel.key()).into() }
}

/// The page and meaning of a link's target (None: not to a page).
pub fn parse(target: &str) -> Option<(i64, Rel)> {
    let rest = target.strip_prefix(SCHEME)?;
    let (id, rel) = rest.split_once("?rel=").map_or((rest, Rel::Link), |(i, r)| (i, Rel::from_key(r)));
    Some((id.parse().ok()?, rel))
}

/// The pages `doc` links to, each once (with the first meaning given).
/// A page shown in it (an embed) counts as a link; the links in the copy
/// shown are that page's.
pub fn page_links(doc: &Document) -> Vec<(i64, Rel)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in &doc.paragraphs {
        for (_, _, a) in p.spans() {
            if a.control.as_ref().is_some_and(|c| c.tag.starts_with(super::embed::EMBED)) {
                continue;
            }
            if let Some((id, rel)) = a.link.as_deref().and_then(parse)
                && seen.insert(id)
            {
                out.push((id, rel));
            }
        }
    }
    for id in super::embed::embedded_pages(doc) {
        if seen.insert(id) {
            out.push((id, Rel::Link));
        }
    }
    out
}

/// Every link to page `id` in `doc`: its extent and target.
fn links_to(doc: &Document, id: i64) -> Vec<(Selection, Arc<str>)> {
    let mut out = Vec::new();
    for (n, p) in doc.paragraphs.iter().enumerate() {
        let mut cur: Option<(usize, usize, Arc<str>)> = None;
        for (s, e, a) in p.spans() {
            let here = a.link.clone().filter(|t| parse(t).is_some_and(|(to, _)| to == id));
            match (&mut cur, here) {
                (Some((_, end, t)), Some(h)) if *end == s && *t == h => *end = e,
                (_, h) => {
                    if let Some((a, b, t)) = cur.take() {
                        out.push((Selection { anchor: Pos::new(n, a), cursor: Pos::new(n, b) }, t));
                    }
                    cur = h.map(|t| (s, e, t));
                }
            }
        }
        if let Some((a, b, t)) = cur {
            out.push((Selection { anchor: Pos::new(n, a), cursor: Pos::new(n, b) }, t));
        }
    }
    out
}

/// Links to page `id` whose text is its old title `old` get `new`
/// instead (the page was renamed). Whether anything changed.
pub fn retitle_links(doc: &mut Document, id: i64, old: &str, new: &str) -> bool {
    let mut changed = false;
    // From the end, so the extents before stay right.
    for (sel, t) in links_to(doc, id).into_iter().rev() {
        let text = &doc.paragraphs[sel.anchor.para].text[sel.anchor.offset..sel.cursor.offset];
        if text == old {
            doc.insert_link(sel, new, t, None);
            changed = true;
        }
    }
    changed
}

/// Links the first place `name` is in `doc` (a whole word, not already a
/// link) to page `id`. Whether it found one.
pub fn link_mention(doc: &mut Document, name: &str, id: i64) -> bool {
    let w = fold(name.trim());
    for n in 0..doc.paragraphs.len() {
        let text = doc.paragraphs[n].text.clone();
        for (a, b) in find_all(&text, &w) {
            if !whole_word(&text, a, b) || doc.link_at(Pos::new(n, a)).is_some_and(|(s, _, _)| s.start().offset <= a && a < s.end().offset) {
                continue;
            }
            doc.set_link(Selection { anchor: Pos::new(n, a), cursor: Pos::new(n, b) }, Some(target(id, Rel::Link)), None);
            return true;
        }
    }
    false
}

/// Which of `names` (folded) is written, as whole words, over byte
/// `at` of `text`: the longest when several are. Its index and where.
pub fn name_at(text: &str, at: usize, names: &[String]) -> Option<(usize, std::ops::Range<usize>)> {
    let mut best: Option<(usize, std::ops::Range<usize>)> = None;
    for (i, n) in names.iter().enumerate() {
        if n.is_empty() || best.as_ref().is_some_and(|b| names[b.0].len() >= n.len()) {
            continue;
        }
        if let Some((a, b)) = find_all(text, n).into_iter().find(|&(a, b)| a <= at && at < b && whole_word(text, a, b)) {
            best = Some((i, a..b));
        }
    }
    best
}

fn whole_word(text: &str, a: usize, b: usize) -> bool {
    let before = text[..a].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
    let after = text[b..].chars().next().is_none_or(|c| !c.is_alphanumeric());
    before && after
}

/// The `#tags` in `text`, as written (without the #), each once. A tag
/// starts with a letter and may have levels: `#lugar/costa`.
pub fn tags_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut it = text.char_indices().peekable();
    let mut prev: Option<char> = None;
    while let Some((i, c)) = it.next() {
        if c == '#' && prev.is_none_or(|p| !p.is_alphanumeric() && p != '#') && text[i + 1..].chars().next().is_some_and(char::is_alphabetic) {
            let rest = &text[i + 1..];
            let end = rest.char_indices().find(|&(_, c)| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))).map_or(rest.len(), |(j, _)| j);
            let tag = rest[..end].trim_end_matches('/');
            if !out.iter().any(|t| t == tag) {
                out.push(tag.to_string());
            }
        }
        prev = Some(c);
    }
    out
}

/// A page and what joins it to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Related {
    pub page: i64,
    pub rel: Rel,
    /// Two steps away: the page in between.
    pub via: Option<i64>,
}

impl Notebook {
    /// Pages to offer for `query`: title or alias with every word, those
    /// starting with it first. Live pages only.
    pub fn find_pages(&self, query: &str) -> Vec<i64> {
        let q = fold(query.trim());
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut v: Vec<(u8, String, i64)> = Vec::new();
        for p in &self.pages {
            if !self.is_live(p.id) {
                continue;
            }
            let names: Vec<String> = std::iter::once(&p.title).chain(&p.aliases).map(|n| fold(n)).collect();
            let rank = names.iter().filter_map(|n| words.iter().all(|w| n.contains(w)).then_some(if n == &q { 0 } else if n.starts_with(&q) { 1 } else { 2 })).min();
            if let Some(r) = rank {
                v.push((r, fold(&p.title), p.id));
            }
        }
        v.sort();
        v.into_iter().map(|t| t.2).collect()
    }

    /// The live page named `name` (title or alias), if any.
    pub fn page_named(&self, name: &str) -> Option<i64> {
        let n = fold(name.trim());
        self.pages.iter().find(|p| self.is_live(p.id) && std::iter::once(&p.title).chain(&p.aliases).any(|t| fold(t) == n)).map(|p| p.id)
    }

    /// The links arrows on live canvases make.
    fn arrow_links(&self) -> impl Iterator<Item = (i64, i64, Rel)> + '_ {
        self.canvas_links.iter().filter(|(c, _)| self.is_live(**c)).flat_map(|(_, v)| v.iter().copied())
    }

    /// The live pages page `id` links to (in its text or by an arrow).
    pub fn outgoing(&self, id: i64) -> Vec<(i64, Rel)> {
        let mut v: Vec<(i64, Rel)> = self.links.get(&id).map(|v| v.to_vec()).unwrap_or_default();
        for (a, b, rel) in self.arrow_links() {
            if a == id && !v.iter().any(|t| t.0 == b) {
                v.push((b, rel));
            }
        }
        v.retain(|(to, _)| *to != id && self.is_live(*to));
        v
    }

    /// The live pages that link to page `id`.
    pub fn backlinks(&self, id: i64) -> Vec<(i64, Rel)> {
        let mut v: Vec<(i64, Rel)> = self.links.iter().filter(|(from, _)| **from != id && self.is_live(**from)).filter_map(|(from, to)| to.iter().find(|(t, _)| *t == id).map(|(_, r)| (*from, *r))).collect();
        for (a, b, rel) in self.arrow_links() {
            if b == id && a != id && self.is_live(a) && !v.iter().any(|t| t.0 == a) {
                v.push((a, rel));
            }
        }
        v.sort();
        v
    }

    /// Live pages whose text names page `id` (its title or an alias, as a
    /// whole word) without linking it.
    pub fn mentions(&self, id: i64) -> Vec<i64> {
        let Some(page) = self.page(id) else { return Vec::new() };
        let names: Vec<String> = std::iter::once(&page.title).chain(&page.aliases).map(|n| fold(n.trim())).filter(|n| n.chars().count() >= 3).collect();
        let linked: HashSet<i64> = self.backlinks(id).into_iter().map(|t| t.0).collect();
        let mut v: Vec<i64> = self
            .texts
            .iter()
            .filter(|(p, _)| **p != id && !linked.contains(p) && self.is_live(**p))
            .filter(|(_, text)| names.iter().any(|n| find_all(text, n).into_iter().any(|(a, b)| whole_word(text, a, b))))
            .map(|(p, _)| *p)
            .collect();
        v.sort();
        v
    }

    /// Everything related to page `id`: the pages it links to and that
    /// link to it, then those two steps away (through one of them),
    /// nearest first.
    pub fn related(&self, id: i64) -> Vec<Related> {
        let mut out: Vec<Related> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::from([id]);
        let near: Vec<(i64, Rel)> = self.outgoing(id).into_iter().chain(self.backlinks(id)).collect();
        for &(p, rel) in &near {
            if seen.insert(p) {
                out.push(Related { page: p, rel, via: None });
            }
        }
        for &(mid, _) in &near {
            for (p, rel) in self.outgoing(mid).into_iter().chain(self.backlinks(mid)) {
                if seen.insert(p) {
                    out.push(Related { page: p, rel, via: Some(mid) });
                }
            }
        }
        out
    }

    /// Every tag in the notebook with the live pages that have it; a tag
    /// with levels also counts for those above it (`a/b` for `a`).
    pub fn tags(&self) -> BTreeMap<String, Vec<i64>> {
        let mut out: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        let mut ids: Vec<&i64> = self.texts.keys().collect();
        ids.sort();
        for id in ids {
            if !self.is_live(*id) {
                continue;
            }
            for tag in tags_in(&self.texts[id]) {
                let mut at = tag.len();
                loop {
                    let key = tag[..at].to_lowercase();
                    let v = out.entry(key).or_default();
                    if !v.contains(id) {
                        v.push(*id);
                    }
                    match tag[..at].rfind('/') {
                        Some(i) => at = i,
                        None => break,
                    }
                }
            }
        }
        out
    }

    /// The live pages with tag `tag` (or one under it).
    pub fn tagged(&self, tag: &str) -> Vec<i64> {
        self.tags().remove(&tag.to_lowercase()).unwrap_or_default()
    }
}
