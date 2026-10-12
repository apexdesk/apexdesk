//! Organizing pages: moving, turning, removing, duplicating and adding
//! them (blank or from another file), and new files made of some pages
//! (extract, split, merge). The page tree is laid out flat under its root
//! when it changes, with what pages inherited copied onto them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use super::super::obj::{Dict, Obj};
use super::{PdfDoc, refs_in};
use crate::Error;

/// What a page may inherit from its parents.
const INHERITED: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

impl PdfDoc {
    /// The root of the page tree.
    fn pages_root(&self) -> Option<u32> {
        match self.catalog()?.1.get("Pages") {
            Some(Obj::Ref(n, _)) => Some(*n),
            _ => None,
        }
    }

    /// Makes `order` (page object numbers) the pages, all children of the
    /// root.
    fn set_pages(&mut self, order: &[u32]) {
        let Some(root) = self.pages_root() else { return };
        for &pn in order {
            let Some(mut d) = self.dict(pn) else { continue };
            let mut changed = false;
            for k in INHERITED {
                if d.get(k).is_none()
                    && let Some(v) = self.inherited(pn, k)
                {
                    d.set(k, v);
                    changed = true;
                }
            }
            if d.get("Parent") != Some(&Obj::Ref(root, 0)) {
                d.set("Parent", self.r(root));
                changed = true;
            }
            if changed {
                self.set(pn, Obj::Dict(d));
            }
        }
        let mut rd = self.dict(root).unwrap_or_default();
        rd.set("Type", Obj::name_of("Pages"));
        rd.set("Kids", Obj::Array(order.iter().map(|&n| self.r(n)).collect()));
        rd.set("Count", Obj::Int(order.len() as i64));
        rd.remove("Parent");
        self.set(root, Obj::Dict(rd));
    }

    /// Moves pages `which` (indices) to stand before page `to` of the
    /// current order (`to` = the page count puts them at the end).
    pub fn move_pages(&mut self, which: &[usize], to: usize) {
        let pages = self.pages();
        let set: BTreeSet<usize> = which.iter().copied().filter(|&i| i < pages.len()).collect();
        if set.is_empty() {
            return;
        }
        let moving: Vec<u32> = set.iter().map(|&i| pages[i]).collect();
        let before = (0..to.min(pages.len())).filter(|i| !set.contains(i)).count();
        let mut rest: Vec<u32> = pages.iter().enumerate().filter(|(i, _)| !set.contains(i)).map(|(_, &n)| n).collect();
        let tail = rest.split_off(before);
        rest.extend(moving);
        rest.extend(tail);
        if rest == pages {
            return;
        }
        self.begin("pdf.undo.move_pages");
        self.set_pages(&rest);
        self.end();
    }

    /// Turns pages `which` by `quarter` quarter turns clockwise.
    pub fn rotate_pages(&mut self, which: &[usize], quarter: i64) {
        let pages = self.pages();
        self.begin("pdf.undo.rotate_pages");
        for &i in which {
            let Some(&pn) = pages.get(i) else { continue };
            let Some(mut d) = self.dict(pn) else { continue };
            let r = self.inherited(pn, "Rotate").and_then(|o| o.int()).unwrap_or(0);
            d.set("Rotate", Obj::Int((r + quarter * 90).rem_euclid(360)));
            self.set(pn, Obj::Dict(d));
        }
        self.end();
    }

    /// Removes pages `which`; at least one page stays.
    pub fn delete_pages(&mut self, which: &[usize]) -> bool {
        let pages = self.pages();
        let set: HashSet<usize> = which.iter().copied().collect();
        let keep: Vec<u32> = pages.iter().enumerate().filter(|(i, _)| !set.contains(i)).map(|(_, &n)| n).collect();
        if keep.is_empty() || keep.len() == pages.len() {
            return false;
        }
        self.begin("pdf.undo.delete_pages");
        self.removing();
        self.set_pages(&keep);
        self.prune(&keep);
        self.end();
        true
    }

    /// Adds a copy of each of pages `which` right after it.
    pub fn duplicate_pages(&mut self, which: &[usize]) {
        let pages = self.pages();
        let set: HashSet<usize> = which.iter().copied().collect();
        if set.is_empty() {
            return;
        }
        self.begin("pdf.undo.duplicate_pages");
        let mut order = Vec::new();
        for (i, &pn) in pages.iter().enumerate() {
            order.push(pn);
            if set.contains(&i)
                && let Some(mut d) = self.dict(pn)
            {
                for k in INHERITED {
                    if d.get(k).is_none()
                        && let Some(v) = self.inherited(pn, k)
                    {
                        d.set(k, v);
                    }
                }
                // Annotations belong to one page: the copy goes without.
                d.remove("Annots");
                d.remove("StructParents");
                order.push(self.add(Obj::Dict(d)));
            }
        }
        self.set_pages(&order);
        self.end();
    }

    /// Adds a blank page before page `at`, the size of its neighbour.
    pub fn insert_blank(&mut self, at: usize) {
        let pages = self.pages();
        let near = pages.get(at.min(pages.len().saturating_sub(1))).copied();
        let g = near.map(|n| self.geom(n));
        let (w, h) = g.map_or((612.0, 792.0), |g| if g.rotate % 180 == 0 { (g.width, g.height) } else { (g.height, g.width) });
        self.begin("pdf.undo.insert_pages");
        let root = self.pages_root().unwrap_or(0);
        let c = self.add(Obj::Stream(Dict::default(), Arc::from(&b""[..])));
        let d = Dict::of(vec![
            ("Type", Obj::name_of("Page")),
            ("Parent", self.r(root)),
            ("MediaBox", Obj::Array(vec![Obj::Int(0), Obj::Int(0), Obj::Real(w), Obj::Real(h)])),
            ("Resources", Obj::Dict(Dict::default())),
            ("Contents", Obj::Ref(c, 0)),
        ]);
        let n = self.add(Obj::Dict(d));
        let mut order = pages;
        order.insert(at.min(order.len()), n);
        self.set_pages(&order);
        self.end();
    }

    /// Adds pages `which` (all when empty) of the PDF `data` before page
    /// `at`; returns how many.
    pub fn insert_from(&mut self, data: Vec<u8>, password: Option<&str>, which: &[usize], at: usize) -> Result<usize, Error> {
        let other = PdfDoc::open(data, password)?;
        let theirs = other.pages();
        let picked: Vec<u32> = if which.is_empty() { theirs.clone() } else { which.iter().filter_map(|&i| theirs.get(i).copied()).collect() };
        if picked.is_empty() {
            return Ok(0);
        }
        let chosen: HashSet<u32> = picked.iter().copied().collect();
        let other_pages: HashSet<u32> = theirs.iter().copied().collect();
        // Everything the chosen pages use, but not their parents or other
        // pages (links to those are dropped).
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut next = self.max_n() + 1;
        let mut stack: Vec<u32> = picked.clone();
        let mut order: Vec<u32> = Vec::new();
        while let Some(n) = stack.pop() {
            if map.contains_key(&n) || other.get(n).is_none() {
                continue;
            }
            map.insert(n, next);
            next += 1;
            order.push(n);
            let mut o = other.get(n).cloned().unwrap_or(Obj::Null);
            if let Obj::Dict(d) = &mut o {
                d.remove("Parent");
            }
            let mut refs = Vec::new();
            refs_in(&o, &mut refs);
            stack.extend(refs.into_iter().filter(|r| !other_pages.contains(r) || chosen.contains(r)));
        }
        self.begin("pdf.undo.insert_pages");
        for &n in &order {
            let mut o = other.get(n).cloned().unwrap_or(Obj::Null);
            if chosen.contains(&n)
                && let Obj::Dict(d) = &mut o
            {
                for k in INHERITED {
                    if d.get(k).is_none()
                        && let Some(v) = other.inherited(n, k)
                    {
                        d.set(k, v);
                    }
                }
                d.remove("Parent");
                d.remove("StructParents");
            }
            let o = renumber(&o, &map);
            self.set(map[&n], o);
        }
        let mut pages = self.pages();
        let at = at.min(pages.len());
        for (k, n) in picked.iter().enumerate() {
            pages.insert(at + k, map[n]);
        }
        self.set_pages(&pages);
        self.end();
        Ok(picked.len())
    }

    /// Drops what points at pages that are gone: bookmarks leading there
    /// lose their target and form fields only there leave the form.
    fn prune(&mut self, keep: &[u32]) {
        let kept: HashSet<u32> = keep.iter().copied().collect();
        let all: Vec<u32> = self.objs.keys().copied().collect();
        let is_page = |d: &Dict| d.get("Type").and_then(Obj::name) == Some("Page");
        let gone: HashSet<u32> = all.iter().copied().filter(|n| !kept.contains(n) && self.dict(*n).is_some_and(|d| is_page(&d))).collect();
        if gone.is_empty() {
            return;
        }
        // Bookmarks and links with a destination on a page that is gone.
        for n in all {
            let Some(mut d) = self.dict(n) else { continue };
            let points_gone = |o: Option<&Obj>| matches!(o, Some(Obj::Array(a)) if matches!(a.first(), Some(Obj::Ref(p, _)) if gone.contains(p)));
            let mut changed = false;
            if points_gone(d.get("Dest")) {
                d.remove("Dest");
                changed = true;
            }
            if let Some(Obj::Dict(a)) = d.get("A")
                && points_gone(a.get("D"))
            {
                d.remove("A");
                changed = true;
            }
            if changed {
                self.set(n, Obj::Dict(d));
            }
        }
        // Destinations elsewhere (named ones, the opening view) on pages
        // that are gone; the structure tree, which names every page and
        // carries their text as alternate text, and article threads go.
        self.forget(&gone);
        self.set_catalog("StructTreeRoot", None);
        self.set_catalog("MarkInfo", None);
        self.set_catalog("Threads", None);
        // Fields whose widgets were all on pages that are gone.
        let on_kept: HashSet<u32> = keep
            .iter()
            .flat_map(|&pn| match self.dict(pn).map(|d| self.key(&d, "Annots")) {
                Some(Obj::Array(a)) => a.iter().filter_map(|o| if let Obj::Ref(m, _) = o { Some(*m) } else { None }).collect(),
                _ => Vec::new(),
            })
            .collect();
        if let Some((_, cat)) = self.catalog()
            && let Some(Obj::Ref(af, _)) = cat.get("AcroForm").cloned()
            && let Some(mut form) = self.dict(af)
            && let Obj::Array(fields) = self.key(&form, "Fields")
        {
            let alive = |me: &PdfDoc, o: &Obj| -> bool {
                let Obj::Ref(m, _) = o else { return true };
                let mut stack = vec![*m];
                let mut seen = HashSet::new();
                let mut any_widget = false;
                while let Some(x) = stack.pop() {
                    if !seen.insert(x) || seen.len() > 10_000 {
                        continue;
                    }
                    if on_kept.contains(&x) {
                        return true;
                    }
                    let Some(d) = me.dict(x) else { continue };
                    if d.get("Subtype").and_then(Obj::name) == Some("Widget") {
                        any_widget = true;
                    }
                    if let Obj::Array(k) = me.key(&d, "Kids") {
                        stack.extend(k.iter().filter_map(|o| if let Obj::Ref(r, _) = o { Some(*r) } else { None }));
                    }
                }
                !any_widget
            };
            let left: Vec<Obj> = fields.iter().filter(|o| alive(self, o)).cloned().collect();
            if left.len() != fields.len() {
                form.set("Fields", Obj::Array(left));
                self.set(af, Obj::Dict(form));
            }
        }
    }

    /// Leaves out of every other object what still leads to objects
    /// `gone`: destinations on them and references to them.
    pub(crate) fn forget(&mut self, gone: &HashSet<u32>) {
        let all: Vec<u32> = self.objs.keys().copied().filter(|n| !gone.contains(n)).collect();
        for n in all {
            let Some(mut o) = self.get(n).cloned() else { continue };
            if forget_in(&mut o, gone, 0) {
                self.set(n, o);
            }
        }
    }

    /// A copy of this document, not encrypted and with no undo history.
    fn copy(&self) -> PdfDoc {
        let mut trailer = self.trailer.clone();
        trailer.remove("Encrypt");
        PdfDoc { base: self.base.clone(), objs: self.objs.clone(), trailer, changed: BTreeSet::new(), whole: true, base_xref: None, repaired: false, crypt: None, undo: Vec::new(), redo: Vec::new(), open: None, depth: 0, revision: 1, saved: 1 }
    }

    /// A new file of pages `which` (in that order).
    pub fn extract(&self, which: &[usize]) -> Vec<u8> {
        let pages = self.pages();
        let mut d = self.copy();
        let keep: Vec<u32> = which.iter().filter_map(|&i| pages.get(i).copied()).collect();
        if keep.is_empty() {
            return d.to_bytes_whole();
        }
        d.set_pages(&keep);
        d.prune(&keep);
        d.to_bytes_whole()
    }

    /// One file for each range of pages (first and last, inclusive).
    pub fn split(&self, ranges: &[(usize, usize)]) -> Vec<Vec<u8>> {
        ranges.iter().map(|&(a, b)| self.extract(&(a..=b.max(a)).collect::<Vec<_>>())).collect()
    }

    /// The ranges where the top-level bookmarks begin, to split by.
    pub fn bookmark_ranges(&self) -> Vec<(String, usize, usize)> {
        let n = self.pages().len();
        let mut starts: Vec<(String, usize)> = self.bookmarks().into_iter().filter(|b| b.level == 0).filter_map(|b| b.page.map(|p| (b.title, p))).collect();
        starts.sort_by_key(|s| s.1);
        starts.dedup_by_key(|s| s.1);
        if starts.first().is_none_or(|s| s.1 > 0) {
            starts.insert(0, (String::new(), 0));
        }
        let mut out = Vec::new();
        for (i, (t, p)) in starts.iter().enumerate() {
            let end = starts.get(i + 1).map_or(n, |s| s.1);
            if end > *p {
                out.push((t.clone(), *p, end - 1));
            }
        }
        out
    }
}

/// `o` with its references renumbered by `map` (others become null).
fn renumber(o: &Obj, map: &HashMap<u32, u32>) -> Obj {
    match o {
        Obj::Ref(n, _) => map.get(n).map_or(Obj::Null, |&m| Obj::Ref(m, 0)),
        Obj::Array(a) => Obj::Array(a.iter().map(|x| renumber(x, map)).collect()),
        Obj::Dict(d) => Obj::Dict(Dict(d.0.iter().map(|(k, v)| (k.clone(), renumber(v, map))).collect())),
        Obj::Stream(d, raw) => Obj::Stream(Dict(d.0.iter().map(|(k, v)| (k.clone(), renumber(v, map))).collect()), raw.clone()),
        o => o.clone(),
    }
}

/// Page ranges typed as "1-3, 5, 8-" (1-based, inclusive) for `count`
/// pages, as 0-based pairs; none when it cannot be read.
pub fn parse_ranges(s: &str, count: usize) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    for part in s.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => {
                let a = if a.trim().is_empty() { 1 } else { a.trim().parse().ok()? };
                let b = if b.trim().is_empty() { count } else { b.trim().parse().ok()? };
                (a, b)
            }
            None => {
                let v: usize = part.parse().ok()?;
                (v, v)
            }
        };
        if a == 0 || b < a || b > count {
            return None;
        }
        out.push((a - 1, b - 1));
    }
    (!out.is_empty()).then_some(out)
}

/// `o` without destinations on objects `gone` (left as null, so name
/// trees keep their pairs) and references to them; whether it changed.
fn forget_in(o: &mut Obj, gone: &HashSet<u32>, depth: u32) -> bool {
    if depth > 64 {
        return false;
    }
    let is_gone = |x: &Obj| matches!(x, Obj::Ref(m, _) if gone.contains(m));
    if let Obj::Array(a) = o
        && a.first().is_some_and(is_gone)
        && matches!(a.get(1), Some(Obj::Name(_)))
    {
        *o = Obj::Null;
        return true;
    }
    match o {
        Obj::Array(a) => {
            let before = a.len();
            a.retain(|x| !is_gone(x));
            let mut changed = a.len() != before;
            for x in a.iter_mut() {
                changed |= forget_in(x, gone, depth + 1);
            }
            changed
        }
        Obj::Dict(d) | Obj::Stream(d, _) => {
            let before = d.0.len();
            d.0.retain(|(_, v)| !is_gone(v));
            let mut changed = d.0.len() != before;
            for (_, v) in d.0.iter_mut() {
                changed |= forget_in(v, gone, depth + 1);
            }
            changed
        }
        _ => false,
    }
}
