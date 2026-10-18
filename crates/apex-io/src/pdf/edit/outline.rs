//! Bookmarks (the outline): reading the tree with the page each one goes
//! to, and adding, renaming and deleting them.

use std::collections::HashSet;

use super::super::obj::{Dict, Obj};
use super::{PdfDoc, obj_text, text_obj};

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub id: u32,
    pub title: String,
    /// Page index it goes to.
    pub page: Option<usize>,
    /// Points from the top of the page (shown).
    pub top: Option<f32>,
    pub level: usize,
    pub children: usize,
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Page(usize, Option<f32>),
    Uri(String),
}

impl PdfDoc {
    /// The links of page `i`: their areas (shown) and targets.
    pub fn links(&self, i: usize) -> Vec<([f32; 4], Target)> {
        let pages = self.pages();
        let Some(&pn) = pages.get(i) else { return Vec::new() };
        let g = self.geom(pn);
        let Some(pd) = self.dict(pn) else { return Vec::new() };
        let Obj::Array(list) = self.key(&pd, "Annots") else { return Vec::new() };
        let mut out = Vec::new();
        for o in list.iter().take(5000) {
            let Some(d) = self.resolve(o).dict().cloned() else { continue };
            if self.key(&d, "Subtype").name() != Some("Link") {
                continue;
            }
            let r = self.nums(d.get("Rect").unwrap_or(&Obj::Null));
            if r.len() != 4 {
                continue;
            }
            let rect = g.rect_to_view([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
            let uri = self.key_dict(&d, "A").filter(|a| self.key(a, "S").name() == Some("URI")).map(|a| obj_text(&self.key(&a, "URI")));
            match uri {
                Some(u) if !u.is_empty() => out.push((rect, Target::Uri(u))),
                _ => {
                    if let (Some(p), top) = self.dest_of(&d, &pages) {
                        out.push((rect, Target::Page(p, top)));
                    }
                }
            }
        }
        out
    }

    /// The bookmarks in order, flattened with their levels.
    pub fn bookmarks(&self) -> Vec<Bookmark> {
        let mut out = Vec::new();
        let Some((_, cat)) = self.catalog() else { return out };
        let Some(outlines) = self.key_dict(&cat, "Outlines") else { return out };
        let pages = self.pages();
        let mut seen = HashSet::new();
        self.walk_outline(outlines.get("First"), 0, &pages, &mut out, &mut seen);
        out
    }

    fn walk_outline(&self, first: Option<&Obj>, level: usize, pages: &[u32], out: &mut Vec<Bookmark>, seen: &mut HashSet<u32>) {
        let mut cur = first.cloned();
        while let Some(Obj::Ref(n, _)) = cur {
            if level > 32 || !seen.insert(n) || out.len() > 20_000 {
                return;
            }
            let Some(d) = self.dict(n) else { return };
            let (page, top) = self.dest_of(&d, pages);
            let at = out.len();
            out.push(Bookmark { id: n, title: obj_text(&self.key(&d, "Title")).trim().to_string(), page, top, level, children: 0 });
            self.walk_outline(d.get("First"), level + 1, pages, out, seen);
            out[at].children = out.len() - at - 1;
            cur = d.get("Next").cloned();
        }
    }

    /// Where an outline item or link goes: its `Dest` or GoTo action.
    pub fn dest_of(&self, d: &Dict, pages: &[u32]) -> (Option<usize>, Option<f32>) {
        let dest = match d.get("Dest") {
            Some(x) => self.resolve(x),
            None => match self.key_dict(d, "A") {
                Some(a) if self.key(&a, "S").name() == Some("GoTo") => self.key(&a, "D"),
                _ => Obj::Null,
            },
        };
        let dest = self.named_dest(dest);
        let arr = match &dest {
            Obj::Array(a) => a.clone(),
            Obj::Dict(dd) => match self.key(dd, "D") {
                Obj::Array(a) => a,
                _ => return (None, None),
            },
            _ => return (None, None),
        };
        let page = match arr.first() {
            Some(Obj::Ref(p, _)) => pages.iter().position(|x| x == p),
            Some(Obj::Int(i)) => usize::try_from(*i).ok().filter(|i| *i < pages.len()),
            _ => None,
        };
        let top = match (page, arr.get(1).and_then(Obj::name)) {
            (Some(p), Some("XYZ")) => arr.get(3).and_then(Obj::num).map(|y| {
                let g = self.geom(pages[p]);
                g.to_view(g.bbox[0], y)[1].max(0.0) as f32
            }),
            _ => None,
        };
        (page, top)
    }

    /// A named destination looked up (catalog `Dests` or the name tree).
    fn named_dest(&self, d: Obj) -> Obj {
        let name: Vec<u8> = match &d {
            Obj::Name(n) => n.as_bytes().to_vec(),
            Obj::Str(s) => s.clone(),
            _ => return d,
        };
        let Some((_, cat)) = self.catalog() else { return Obj::Null };
        if let Some(dests) = self.key_dict(&cat, "Dests") {
            let key = String::from_utf8_lossy(&name).into_owned();
            if let Some(v) = dests.get(&key) {
                return self.resolve(v);
            }
        }
        let Some(tree) = self.key_dict(&cat, "Names").and_then(|n| self.key_dict(&n, "Dests")) else { return Obj::Null };
        self.name_tree_find(&tree, &name, 0).unwrap_or(Obj::Null)
    }

    fn name_tree_find(&self, node: &Dict, name: &[u8], depth: u32) -> Option<Obj> {
        if depth > 32 {
            return None;
        }
        if let Obj::Array(names) = self.key(node, "Names") {
            for pair in names.chunks_exact(2) {
                if self.resolve(&pair[0]).bytes() == Some(name) {
                    return Some(self.resolve(&pair[1]));
                }
            }
        }
        if let Obj::Array(kids) = self.key(node, "Kids") {
            for k in kids {
                let Some(kd) = self.resolve(&k).dict().cloned() else { continue };
                if let Obj::Array(lim) = self.key(&kd, "Limits")
                    && lim.len() == 2
                {
                    let (lo, hi) = (self.resolve(&lim[0]), self.resolve(&lim[1]));
                    if lo.bytes().is_some_and(|l| name < l) || hi.bytes().is_some_and(|h| name > h) {
                        continue;
                    }
                }
                if let Some(v) = self.name_tree_find(&kd, name, depth + 1) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// The outline dictionary, made when missing.
    fn outlines(&mut self) -> u32 {
        if let Some((_, cat)) = self.catalog()
            && let Some(Obj::Ref(n, _)) = cat.get("Outlines")
            && self.dict(*n).is_some()
        {
            return *n;
        }
        let n = self.add(Obj::Dict(Dict::of(vec![("Type", Obj::name_of("Outlines")), ("Count", Obj::Int(0))])));
        self.set_catalog("Outlines", Some(Obj::Ref(n, 0)));
        n
    }

    /// Adds a bookmark called `title` to page `page` at `top` after
    /// bookmark `after` (at its level), or at the end.
    pub fn add_bookmark(&mut self, title: &str, page: usize, top: f32, after: Option<u32>) -> u32 {
        let pages = self.pages();
        let Some(&pn) = pages.get(page) else { return 0 };
        self.begin("pdf.undo.bookmark_add");
        let root = self.outlines();
        let parent = after.and_then(|a| match self.dict(a)?.get("Parent") {
            Some(Obj::Ref(p, _)) => Some(*p),
            _ => None,
        }).unwrap_or(root);
        let g = self.geom(pn);
        let y = g.to_user(0.0, top as f64)[1];
        let dest = Obj::Array(vec![Obj::Ref(pn, 0), Obj::name_of("XYZ"), Obj::Null, Obj::Real(super::appearance::round(y)), Obj::Null]);
        let mut d = Dict::of(vec![("Title", text_obj(title)), ("Parent", Obj::Ref(parent, 0)), ("Dest", dest)]);
        let n = self.add(Obj::Null);
        let mut pd = self.dict(parent).unwrap_or_default();
        let prev = match after.filter(|a| self.dict(*a).is_some()) {
            Some(a) => Some(a),
            None => match pd.get("Last") {
                Some(Obj::Ref(l, _)) => Some(*l),
                _ => None,
            },
        };
        match prev {
            Some(p) => {
                let mut prevd = self.dict(p).unwrap_or_default();
                if let Some(next) = prevd.get("Next").cloned() {
                    d.set("Next", next.clone());
                    if let Obj::Ref(nx, _) = next
                        && let Some(mut nd) = self.dict(nx)
                    {
                        nd.set("Prev", Obj::Ref(n, 0));
                        self.set(nx, Obj::Dict(nd));
                    }
                } else {
                    pd.set("Last", Obj::Ref(n, 0));
                }
                d.set("Prev", Obj::Ref(p, 0));
                prevd.set("Next", Obj::Ref(n, 0));
                self.set(p, Obj::Dict(prevd));
            }
            None => {
                pd.set("First", Obj::Ref(n, 0));
                pd.set("Last", Obj::Ref(n, 0));
            }
        }
        let count = pd.get("Count").and_then(Obj::int).unwrap_or(0);
        pd.set("Count", Obj::Int(if count < 0 { count - 1 } else { count + 1 }));
        self.set(parent, Obj::Dict(pd));
        self.set(n, Obj::Dict(d));
        self.end();
        n
    }

    pub fn rename_bookmark(&mut self, id: u32, title: &str) {
        let Some(mut d) = self.dict(id) else { return };
        self.begin("pdf.undo.bookmark_rename");
        d.set("Title", text_obj(title));
        self.set(id, Obj::Dict(d));
        self.end();
    }

    /// Deletes a bookmark and the ones under it.
    pub fn delete_bookmark(&mut self, id: u32) {
        let Some(d) = self.dict(id) else { return };
        self.begin("pdf.undo.bookmark_delete");
        let r = |k: &str| match d.get(k) {
            Some(Obj::Ref(n, _)) => Some(*n),
            _ => None,
        };
        let (prev, next, parent) = (r("Prev"), r("Next"), r("Parent"));
        if let Some(p) = prev
            && let Some(mut pd) = self.dict(p)
        {
            match next {
                Some(nx) => pd.set("Next", Obj::Ref(nx, 0)),
                None => {
                    pd.remove("Next");
                }
            }
            self.set(p, Obj::Dict(pd));
        }
        if let Some(nx) = next
            && let Some(mut nd) = self.dict(nx)
        {
            match prev {
                Some(p) => nd.set("Prev", Obj::Ref(p, 0)),
                None => {
                    nd.remove("Prev");
                }
            }
            self.set(nx, Obj::Dict(nd));
        }
        if let Some(par) = parent
            && let Some(mut pd) = self.dict(par)
        {
            if r("Prev").is_none() {
                match next {
                    Some(nx) => pd.set("First", Obj::Ref(nx, 0)),
                    None => {
                        pd.remove("First");
                    }
                }
            }
            if r("Next").is_none() {
                match prev {
                    Some(p) => pd.set("Last", Obj::Ref(p, 0)),
                    None => {
                        pd.remove("Last");
                    }
                }
            }
            let count = pd.get("Count").and_then(Obj::int).unwrap_or(1);
            pd.set("Count", Obj::Int(if count < 0 { (count + 1).min(0) } else { (count - 1).max(0) }));
            self.set(par, Obj::Dict(pd));
        }
        self.end();
    }
}
