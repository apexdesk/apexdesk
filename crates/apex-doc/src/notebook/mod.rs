//! A notebook (Cuaderno): sections of pages, a page with subpages, each
//! page formatted text of its own. The model only: what there is, how it
//! is ordered, searched and kept in the trash. The file is apex-io's.

use std::collections::{HashMap, HashSet};

pub mod canvas;
pub mod claims;
pub mod links;
pub mod embed;
pub mod query;
pub mod tasks;
pub mod types;
mod search;
pub use canvas::PageKind;
pub use claims::{Claim, ClaimKind, Evidence, Status};
pub use links::{Rel, Related};
pub use types::{Field, FieldKind, PageType};
pub use search::{SearchHit, fold};

/// Days something deleted stays in the trash.
pub const TRASH_DAYS: i64 = 30;
const DAY: i64 = 86_400;

/// Colors offered for sections (RGB).
pub const SECTION_COLORS: [u32; 8] = [0x4A7FD4, 0x3FA672, 0xD9A23B, 0xD4584A, 0x8A63C9, 0x2FA3B5, 0xC9608F, 0x7D8794];

#[derive(Clone, Debug, PartialEq)]
pub struct NoteSection {
    pub id: i64,
    pub name: String,
    /// RGB.
    pub color: u32,
    pub pos: i64,
    /// When it went to the trash (seconds since 1970).
    pub deleted: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub id: i64,
    pub section: i64,
    /// The page it is a subpage of.
    pub parent: Option<i64>,
    pub pos: i64,
    pub title: String,
    /// Other names it goes by (links and mentions find it by them too).
    pub aliases: Vec<String>,
    pub kind: PageKind,
    /// What it describes when it is a source (a book, an article…), to
    /// cite it.
    pub source: Option<crate::biblio::Source>,
    /// What it asks or proposes, when it is a question or a hypothesis.
    pub claim: Option<Claim>,
    /// Its page type, and the values of its fields (by field name).
    pub ptype: Option<i64>,
    pub fields: Vec<(String, String)>,
    /// Seconds since 1970.
    pub created: i64,
    pub modified: i64,
    pub deleted: Option<i64>,
}

/// Something in the trash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trashed {
    Section(i64),
    Page(i64),
}

/// The sections and pages of a notebook, and the text of each page (to
/// search it). The structure is kept apart from the text so that it can
/// be copied for undo cheaply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notebook {
    pub sections: Vec<NoteSection>,
    pub pages: Vec<Page>,
    /// Each page's plain text.
    pub texts: HashMap<i64, String>,
    /// The pages each page links to, and what each link means.
    pub links: HashMap<i64, Vec<(i64, Rel)>>,
    /// The links the arrows of each canvas page make: (from, to, meaning).
    pub canvas_links: HashMap<i64, Vec<(i64, i64, Rel)>>,
    /// The tasks (check boxes) of each page.
    pub tasks: HashMap<i64, Vec<tasks::Task>>,
    /// The page types, in order.
    pub types: Vec<PageType>,
    /// The saved queries, in order.
    pub queries: Vec<query::Query>,
    /// Sections kept under a password and not unlocked: their pages are
    /// out of sight.
    pub sealed: std::collections::HashSet<i64>,
    /// Where pages were last found by id (checked on each use).
    index: PageIndex,
}

/// Page ids to their place in `pages`, as last seen: a hint checked on
/// each use (the pages change freely), so looking a page up isn't a walk
/// over all of them each time (thousands of pages, every frame).
#[derive(Debug, Default)]
struct PageIndex(std::sync::Mutex<HashMap<i64, usize>>);

impl Clone for PageIndex {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for PageIndex {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// The structure of a notebook, as undo keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Structure {
    pub sections: Vec<NoteSection>,
    pub pages: Vec<Page>,
    pub types: Vec<PageType>,
    pub queries: Vec<query::Query>,
}

impl Notebook {
    pub fn structure(&self) -> Structure {
        Structure { sections: self.sections.clone(), pages: self.pages.clone(), types: self.types.clone(), queries: self.queries.clone() }
    }

    pub fn set_structure(&mut self, s: Structure) {
        self.sections = s.sections;
        self.pages = s.pages;
        self.types = s.types;
        self.queries = s.queries;
    }

    pub fn section(&self, id: i64) -> Option<&NoteSection> {
        self.sections.iter().find(|s| s.id == id)
    }

    pub fn section_mut(&mut self, id: i64) -> Option<&mut NoteSection> {
        self.sections.iter_mut().find(|s| s.id == id)
    }

    pub fn page(&self, id: i64) -> Option<&Page> {
        let mut index = self.index.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = index.get(&id).and_then(|&i| self.pages.get(i)).filter(|p| p.id == id) {
            return Some(p);
        }
        let i = self.pages.iter().position(|p| p.id == id)?;
        index.insert(id, i);
        self.pages.get(i)
    }

    /// The pages there to see (see `is_live`), found in one pass.
    pub fn live_set(&self) -> HashSet<i64> {
        let by_id: HashMap<i64, &Page> = self.pages.iter().rev().map(|p| (p.id, p)).collect();
        let mut live: HashMap<i64, bool> = HashMap::new();
        for p in &self.pages {
            if live.contains_key(&p.id) {
                continue;
            }
            // Up to a page known or the top (the tree is at most 64 deep).
            let mut chain = Vec::new();
            let mut cur = Some(p);
            let mut ok = false;
            while let Some(q) = cur {
                if let Some(&k) = live.get(&q.id) {
                    ok = k;
                    break;
                }
                if q.deleted.is_some() || chain.len() > 64 {
                    chain.push(q.id);
                    break;
                }
                chain.push(q.id);
                match q.parent {
                    Some(up) => cur = by_id.get(&up).copied(),
                    None => {
                        ok = self.section(q.section).is_some_and(|s| s.deleted.is_none()) && !self.sealed.contains(&q.section);
                        break;
                    }
                }
            }
            // Pages under a deleted one (or a missing parent) aren't live.
            let ok = ok && cur.is_some();
            for id in chain {
                live.insert(id, ok);
            }
        }
        live.into_iter().filter(|e| e.1).map(|e| e.0).collect()
    }

    pub fn page_mut(&mut self, id: i64) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    /// An id no section or page has.
    pub fn new_id(&self) -> i64 {
        self.sections.iter().map(|s| s.id).chain(self.pages.iter().map(|p| p.id)).max().unwrap_or(0) + 1
    }

    /// The sections not in the trash, in order.
    pub fn live_sections(&self) -> Vec<&NoteSection> {
        let mut v: Vec<&NoteSection> = self.sections.iter().filter(|s| s.deleted.is_none()).collect();
        v.sort_by_key(|s| (s.pos, s.id));
        v
    }

    /// Puts at the top of their section the pages whose parent is missing,
    /// in another section, among their own subpages or too deep: a file's
    /// tree must be one, for the walks over it to end.
    pub fn fix_parents(&mut self) {
        const MAX_DEPTH: usize = 64;
        let mut up: HashMap<i64, (Option<i64>, i64)> = self.pages.iter().map(|p| (p.id, (p.parent, p.section))).collect();
        let ids: Vec<i64> = self.pages.iter().map(|p| p.id).collect();
        let mut cut = HashSet::new();
        for id in ids {
            let section = up[&id].1;
            let (mut cur, mut steps, mut ok) = (id, 0, true);
            while let Some(parent) = up[&cur].0 {
                steps += 1;
                if parent == id || steps > MAX_DEPTH || up.get(&parent).is_none_or(|p| p.1 != section) {
                    ok = false;
                    break;
                }
                cur = parent;
            }
            if !ok {
                if let Some(e) = up.get_mut(&id) {
                    e.0 = None;
                }
                cut.insert(id);
            }
        }
        for p in &mut self.pages {
            if cut.contains(&p.id) {
                p.parent = None;
            }
        }
    }

    /// Whether page `id` is there to see: neither it, a page above it nor
    /// its section in the trash.
    pub fn is_live(&self, id: i64) -> bool {
        let mut cur = self.page(id);
        // The tree is at most 64 deep (`fix_parents`).
        let mut steps = 0;
        while let Some(p) = cur {
            steps += 1;
            if p.deleted.is_some() || steps > 66 {
                return false;
            }
            match p.parent {
                Some(up) => cur = self.page(up),
                None => return self.section(p.section).is_some_and(|s| s.deleted.is_none()) && !self.sealed.contains(&p.section),
            }
        }
        false
    }

    /// The pages under `parent` (None: the top of section `section`) not
    /// in the trash, in order.
    pub fn children(&self, section: i64, parent: Option<i64>) -> Vec<&Page> {
        let mut v: Vec<&Page> = self.pages.iter().filter(|p| p.deleted.is_none() && p.parent == parent && (parent.is_some() || p.section == section)).collect();
        v.sort_by_key(|p| (p.pos, p.id));
        v
    }

    /// The pages of section `section` as a tree read from the top: each
    /// with its depth (0 a page, 1 its subpage…).
    pub fn tree(&self, section: i64) -> Vec<(usize, &Page)> {
        let mut out = Vec::new();
        if self.sealed.contains(&section) {
            return out;
        }
        self.walk(section, None, 0, &mut out);
        out
    }

    fn walk<'a>(&'a self, section: i64, parent: Option<i64>, depth: usize, out: &mut Vec<(usize, &'a Page)>) {
        // Each page's children found in one pass over the pages, not one
        // pass per page.
        let mut kids: HashMap<Option<i64>, Vec<&Page>> = HashMap::new();
        for p in self.pages.iter().filter(|p| p.deleted.is_none() && (p.parent.is_some() || p.section == section)) {
            kids.entry(p.parent).or_default().push(p);
        }
        for v in kids.values_mut() {
            v.sort_by_key(|p| (p.pos, p.id));
        }
        fn go<'a>(kids: &HashMap<Option<i64>, Vec<&'a Page>>, parent: Option<i64>, depth: usize, seen: &mut HashSet<i64>, out: &mut Vec<(usize, &'a Page)>) {
            for &p in kids.get(&parent).into_iter().flatten() {
                // Each id once: a file may repeat ids and make cycles.
                if !seen.insert(p.id) {
                    continue;
                }
                out.push((depth, p));
                // The tree is at most 64 deep (`fix_parents`).
                if depth < 64 {
                    go(kids, Some(p.id), depth + 1, seen, out);
                }
            }
        }
        go(&kids, parent, depth, &mut HashSet::new(), out);
    }

    /// Page `id` and every page under it.
    pub fn subtree(&self, id: i64) -> Vec<i64> {
        self.subtrees(&[id])
    }

    /// The pages `ids` and all under them, each once.
    fn subtrees(&self, ids: &[i64]) -> Vec<i64> {
        let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
        for p in &self.pages {
            if let Some(up) = p.parent {
                children.entry(up).or_default().push(p.id);
            }
        }
        let mut seen: HashSet<i64> = HashSet::new();
        let mut out: Vec<i64> = ids.iter().copied().filter(|id| seen.insert(*id)).collect();
        let mut i = 0;
        while i < out.len() {
            for c in children.get(&out[i]).into_iter().flatten() {
                if seen.insert(*c) {
                    out.push(*c);
                }
            }
            i += 1;
        }
        out
    }

    /// Numbers the pages under `parent` (or the sections) in their order.
    fn renumber_pages(&mut self, section: i64, parent: Option<i64>, order: &[i64]) {
        let _ = section;
        for (n, id) in order.iter().enumerate() {
            if let Some(p) = self.page_mut(*id) {
                p.pos = n as i64;
                p.parent = parent;
            }
        }
    }

    pub fn add_section(&mut self, name: &str, now: i64) -> i64 {
        let _ = now;
        let id = self.new_id();
        let pos = self.sections.iter().map(|s| s.pos + 1).max().unwrap_or(0);
        let color = SECTION_COLORS[self.sections.len() % SECTION_COLORS.len()];
        self.sections.push(NoteSection { id, name: name.to_string(), color, pos, deleted: None });
        id
    }

    /// A new page in `section`: under `parent`, after `after` (None: at
    /// the end).
    pub fn add_page(&mut self, section: i64, parent: Option<i64>, after: Option<i64>, title: &str, now: i64) -> i64 {
        let id = self.new_id();
        let mut order: Vec<i64> = self.children(section, parent).iter().map(|p| p.id).collect();
        let at = after.and_then(|a| order.iter().position(|&x| x == a)).map_or(order.len(), |i| i + 1);
        order.insert(at, id);
        self.pages.push(Page { id, section, parent, pos: 0, title: title.to_string(), aliases: Vec::new(), kind: PageKind::Text, source: None, claim: None, ptype: None, fields: Vec::new(), created: now, modified: now, deleted: None });
        self.texts.insert(id, String::new());
        self.renumber_pages(section, parent, &order);
        id
    }

    /// Moves page `id` with its subpages to `section`, under `parent`,
    /// before `before` (None: at the end). False when it can't go there
    /// (under itself).
    pub fn move_page(&mut self, id: i64, section: i64, parent: Option<i64>, before: Option<i64>) -> bool {
        if self.page(id).is_none() || self.section(section).is_none() || parent.is_some_and(|p| self.subtree(id).contains(&p)) {
            return false;
        }
        let section = parent.and_then(|p| self.page(p)).map_or(section, |p| p.section);
        let mut order: Vec<i64> = self.children(section, parent).iter().map(|p| p.id).filter(|&x| x != id).collect();
        let at = before.and_then(|b| order.iter().position(|&x| x == b)).unwrap_or(order.len());
        order.insert(at, id);
        for p in self.subtree(id) {
            if let Some(p) = self.page_mut(p) {
                p.section = section;
            }
        }
        self.renumber_pages(section, parent, &order);
        true
    }

    /// Moves section `id` before `before` (None: at the end).
    pub fn move_section(&mut self, id: i64, before: Option<i64>) {
        let mut order: Vec<i64> = self.live_sections().iter().map(|s| s.id).filter(|&x| x != id).collect();
        let at = before.and_then(|b| order.iter().position(|&x| x == b)).unwrap_or(order.len());
        order.insert(at, id);
        for (n, s) in order.iter().enumerate() {
            if let Some(s) = self.section_mut(*s) {
                s.pos = n as i64;
            }
        }
    }

    /// Puts page `id` and its subpages in the trash.
    pub fn delete_page(&mut self, id: i64, now: i64) {
        for p in self.subtree(id) {
            if let Some(p) = self.page_mut(p)
                && p.deleted.is_none()
            {
                p.deleted = Some(now);
            }
        }
    }

    pub fn delete_section(&mut self, id: i64, now: i64) {
        if let Some(s) = self.section_mut(id) {
            s.deleted = Some(now);
        }
    }

    /// What is in the trash, most recent first: sections, and pages put
    /// there on their own (not with the page above them or in a section
    /// in the trash).
    pub fn trash(&self) -> Vec<(Trashed, i64)> {
        let mut v: Vec<(Trashed, i64)> = self.sections.iter().filter_map(|s| s.deleted.map(|d| (Trashed::Section(s.id), d))).collect();
        for p in &self.pages {
            let Some(d) = p.deleted else { continue };
            let with_parent = p.parent.and_then(|up| self.page(up)).is_some_and(|up| up.deleted == Some(d));
            let section_gone = self.section(p.section).is_none_or(|s| s.deleted.is_some());
            if !with_parent && !section_gone {
                v.push((Trashed::Page(p.id), d));
            }
        }
        v.sort_by_key(|&(t, d)| (std::cmp::Reverse(d), matches!(t, Trashed::Page(_))));
        v
    }

    /// Takes `t` out of the trash. A page whose page above is gone goes
    /// to the top of its section.
    pub fn restore(&mut self, t: Trashed) {
        match t {
            Trashed::Section(id) => {
                let pos = self.sections.iter().filter(|s| s.deleted.is_none()).map(|s| s.pos + 1).max().unwrap_or(0);
                if let Some(s) = self.section_mut(id) {
                    s.deleted = None;
                    s.pos = pos;
                }
            }
            Trashed::Page(id) => {
                let Some(d) = self.page(id).and_then(|p| p.deleted) else { return };
                for p in self.subtree(id) {
                    if let Some(p) = self.page_mut(p)
                        && p.deleted == Some(d)
                    {
                        p.deleted = None;
                    }
                }
                let (section, parent) = self.page(id).map(|p| (p.section, p.parent)).unwrap_or_default();
                if parent.is_some_and(|up| !self.is_live(up)) {
                    self.move_page(id, section, None, None);
                }
            }
        }
    }

    /// Removes for good what has been in the trash more than
    /// [`TRASH_DAYS`], or `only` when given; returns the pages removed.
    pub fn purge(&mut self, now: i64, only: Option<Trashed>) -> Vec<i64> {
        let old = |d: Option<i64>| d.is_some_and(|d| now - d > TRASH_DAYS * DAY);
        let sections: HashSet<i64> = self.sections.iter().filter(|s| if only.is_some() { only == Some(Trashed::Section(s.id)) } else { old(s.deleted) }).map(|s| s.id).collect();
        let roots: Vec<i64> = self
            .pages
            .iter()
            .filter(|p| sections.contains(&p.section) || if only.is_some() { only == Some(Trashed::Page(p.id)) } else { old(p.deleted) })
            .map(|p| p.id)
            .collect();
        let mut pages = self.subtrees(&roots);
        pages.sort_unstable();
        let gone: HashSet<i64> = pages.iter().copied().collect();
        self.sections.retain(|s| !sections.contains(&s.id));
        self.pages.retain(|p| !gone.contains(&p.id));
        for p in &pages {
            self.texts.remove(p);
            self.links.remove(p);
            self.canvas_links.remove(p);
        }
        pages
    }
}

#[cfg(test)]
mod tests;
