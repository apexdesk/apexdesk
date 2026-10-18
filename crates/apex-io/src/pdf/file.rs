//! The file structure of a PDF: cross-reference tables and streams, object
//! streams, the stream filters and the standard security handler. Broken
//! files are read by scanning for their objects, as viewers do.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use md5::Digest as _;

use super::obj::{Dict, Item, Lexer, Obj};
use crate::Error;

#[derive(Clone, Copy, Debug)]
enum Entry {
    At(usize),
    /// In object stream `n`, at index `i`.
    In(u32, u32),
}

type ObjStm = Arc<(Vec<u8>, Vec<(u32, usize)>)>;

pub struct PdfFile<'a> {
    data: &'a [u8],
    xref: HashMap<u32, Entry>,
    pub trailer: Dict,
    crypt: Option<Crypt>,
    cache: RefCell<HashMap<u32, Obj>>,
    streams: RefCell<HashMap<u32, ObjStm>>,
    /// What the object streams kept may still add up to.
    streams_left: std::cell::Cell<usize>,
    /// While a page is drawn: what its streams may still inflate to (the
    /// same stream named again and again is inflated each time).
    decode_left: std::cell::Cell<Option<usize>>,
    loading: RefCell<HashSet<u32>>,
    scanned: RefCell<Option<HashMap<u32, usize>>>,
    /// What reading objects from the file may still lex and copy: a
    /// damaged file whose objects never close is read in linear time.
    read_left: std::cell::Cell<usize>,
}

/// A page of the page tree with what it inherits.
/// What a page tree's node passes to its pages.
struct Inherited {
    res: std::rc::Rc<Dict>,
    /// The objects `res` is made of.
    weight: usize,
    media: Option<Vec<f64>>,
    crop: Option<Vec<f64>>,
    rotate: i64,
}

/// The pages' resources still affordable, and the weights of shared ones.
struct Walk {
    left: usize,
    weights: HashMap<u32, usize>,
}

/// Objects the pages' copies of their resources may have in all.
const MAX_PAGE_RESOURCES: usize = 1 << 23;

pub struct PageRef {
    pub dict: Dict,
    pub resources: Dict,
    /// The visible box (crop box within the media box): x0, y0, x1, y1.
    pub bbox: [f64; 4],
    /// Clockwise rotation: 0, 90, 180 or 270.
    pub rotate: i64,
}

/// A stream's bytes after its filters, and the image format that was left
/// encoded (JPEG and the like are kept as they are).
pub struct Decoded {
    pub data: Vec<u8>,
    pub codec: Option<Arc<str>>,
}

impl<'a> PdfFile<'a> {
    pub fn open(data: &'a [u8], password: Option<&str>) -> Result<PdfFile<'a>, Error> {
        // A ZIP package with a PDF inside its first bytes is a package (its
        // signatures would vouch for the PDF shown otherwise).
        let start = if data.starts_with(b"PK\x03\x04") { None } else { memchr::memmem::find(&data[..data.len().min(1024)], b"%PDF-") };
        let Some(start) = start else {
            return Err(Error::new("not a PDF file"));
        };
        let mut f = PdfFile { data, xref: HashMap::new(), trailer: Dict::default(), crypt: None, cache: RefCell::default(), streams: RefCell::default(), streams_left: STREAMS_BUDGET.into(), decode_left: None.into(), loading: RefCell::default(), scanned: RefCell::new(None), read_left: data.len().saturating_mul(8).saturating_add(256 << 20).into() };
        let loaded = f.load_xref(start);
        if !loaded || f.catalog().is_none() || f.pages().is_empty() {
            // A broken cross-reference table (or one that loses the pages):
            // the objects are found by scanning the file.
            f.cache.borrow_mut().clear();
            f.xref.clear();
            let (map, trailer) = scan(data);
            f.xref = map.iter().map(|(&n, &o)| (n, Entry::At(o))).collect();
            // Objects in object streams are found through the streams.
            let objstms: Vec<u32> = map.keys().copied().collect();
            for n in objstms {
                if let Obj::Stream(d, _) = f.get(n)
                    && d.get("Type").and_then(Obj::name) == Some("ObjStm")
                    && let Some(s) = f.objstm(n)
                {
                    for (i, (m, _)) in s.1.iter().enumerate() {
                        f.xref.entry(*m).or_insert(Entry::In(n, i as u32));
                    }
                }
            }
            *f.scanned.borrow_mut() = Some(map);
            f.trailer = trailer;
            f.cache.borrow_mut().clear();
            if f.trailer.get("Root").is_none() {
                let root = f.xref.keys().copied().find(|&n| f.get(n).dict().and_then(|d| d.get("Type")).and_then(Obj::name) == Some("Catalog"));
                if let Some(r) = root {
                    f.trailer.0.push((Arc::from("Root"), Obj::Ref(r, 0)));
                }
            }
        }
        if let Some(enc) = f.trailer.get("Encrypt").cloned() {
            let enc = f.resolve(&enc);
            if let Some(d) = enc.dict() {
                let id0 = f.trailer.get("ID").and_then(Obj::array).and_then(|a| a.first()).map(|o| f.resolve(o)).and_then(|o| o.bytes().map(<[u8]>::to_vec)).unwrap_or_default();
                let enc_ref = match f.trailer.get("Encrypt") {
                    Some(Obj::Ref(n, _)) => Some(*n),
                    _ => None,
                };
                f.crypt = Some(Crypt::new(d, &id0, password, enc_ref)?);
                f.cache.borrow_mut().clear();
            }
        }
        if f.catalog().is_none() {
            return Err(Error::new("the PDF file is damaged"));
        }
        Ok(f)
    }

    /// A file made of objects already read (those of a
    /// [`super::edit::PdfDoc`]), to load its fonts and images.
    pub(super) fn of_objects(objs: HashMap<u32, Obj>, trailer: Dict) -> PdfFile<'static> {
        PdfFile { data: &[], xref: HashMap::new(), trailer, crypt: None, cache: RefCell::new(objs), streams: RefCell::default(), streams_left: STREAMS_BUDGET.into(), decode_left: None.into(), loading: RefCell::default(), scanned: RefCell::new(Some(HashMap::new())), read_left: 0.into() }
    }

    /// Every object the cross-references list (not the free ones), with
    /// its generation.
    pub(super) fn entries(&self) -> Vec<(u32, u16)> {
        let mut v: Vec<(u32, u16)> = self
            .xref
            .iter()
            .filter_map(|(&n, e)| match *e {
                Entry::At(0) => None,
                Entry::At(p) => {
                    let mut lx = Lexer::new(self.data, p);
                    let g = match (lx.next(false), lx.next(false)) {
                        (Some(Item::Obj(Obj::Int(_))), Some(Item::Obj(Obj::Int(g)))) => g.clamp(0, u16::MAX as i64) as u16,
                        _ => 0,
                    };
                    Some((n, g))
                }
                Entry::In(..) => Some((n, 0)),
            })
            .collect();
        v.sort_unstable();
        v
    }

    /// The security handler, for writing objects encrypted again.
    pub(super) fn take_crypt(&mut self) -> Option<Crypt> {
        self.crypt.take()
    }

    /// The numbers of the objects the cross-reference tables list.
    pub fn numbers(&self) -> Vec<u32> {
        self.xref.keys().copied().collect()
    }

    pub fn catalog(&self) -> Option<Dict> {
        let root = self.trailer.get("Root")?.clone();
        match self.resolve(&root) {
            Obj::Dict(d) if d.get("Pages").is_some() => Some(d),
            _ => None,
        }
    }

    /// Reads the chain of cross-reference sections from `startxref`.
    fn load_xref(&mut self, header: usize) -> bool {
        let data = self.data;
        let Some(sx) = memchr::memmem::rfind(&data[data.len().saturating_sub(4096)..], b"startxref") else { return false };
        let sx = data.len().saturating_sub(4096) + sx;
        let mut lx = Lexer::new(data, sx + 9);
        let Some(Item::Obj(Obj::Int(first))) = lx.next(false) else { return false };
        // Sections still to read, the next one last (earlier ones win).
        let mut todo = vec![first as usize];
        let mut seen = HashSet::new();
        let mut trailer: Option<Dict> = None;
        while let Some(p) = todo.pop() {
            if !seen.insert(p) || seen.len() > 1000 {
                continue;
            }
            // Files with junk before the header have offsets from it.
            let p = if self.xref_at(p) { p } else if self.xref_at(p + header) { p + header } else if trailer.is_some() { continue } else { return false };
            let t = if data[p..].starts_with(b"xref") { self.xref_table(p) } else { self.xref_stream(p) };
            let Some(t) = t else { break };
            // A hybrid file's stream right after its table, then the
            // previous section.
            if let Some(v) = t.get("Prev").and_then(Obj::int) {
                todo.push(v as usize);
            }
            if let Some(s) = t.get("XRefStm").and_then(Obj::int) {
                todo.push(s as usize);
            }
            if trailer.is_none() {
                trailer = Some(t);
            }
        }
        self.done(trailer)
    }

    fn done(&mut self, trailer: Option<Dict>) -> bool {
        match trailer {
            Some(t) => {
                self.trailer = t;
                true
            }
            None => false,
        }
    }

    fn xref_at(&self, p: usize) -> bool {
        let Some(rest) = self.data.get(p..) else { return false };
        if rest.starts_with(b"xref") {
            return true;
        }
        let mut lx = Lexer::new(self.data, p);
        matches!((lx.next(false), lx.next(false), lx.next(false)), (Some(Item::Obj(Obj::Int(_))), Some(Item::Obj(Obj::Int(_))), Some(Item::Kw(b"obj"))))
    }

    fn xref_table(&mut self, p: usize) -> Option<Dict> {
        let mut lx = Lexer::new(self.data, p + 4);
        loop {
            match lx.next(false)? {
                Item::Kw(b"trailer") => break,
                Item::Obj(Obj::Int(start)) => {
                    let Item::Obj(Obj::Int(count)) = lx.next(false)? else { return None };
                    for i in 0..count.max(0) {
                        let (Some(Item::Obj(off)), Some(Item::Obj(_gen)), Some(Item::Kw(kind))) = (lx.next(false), lx.next(false), lx.next(false)) else { return None };
                        let n = (start + i) as u32;
                        if kind == b"n" && !self.xref.contains_key(&n) {
                            let off = off.int().unwrap_or(0).max(0) as usize;
                            if off > 0 {
                                self.xref.insert(n, Entry::At(off));
                            }
                        } else if kind == b"f" {
                            self.xref.entry(n).or_insert(Entry::At(0));
                        }
                    }
                }
                _ => return None,
            }
        }
        match lx.next(true)? {
            Item::Obj(Obj::Dict(d)) => Some(d),
            _ => None,
        }
    }

    fn xref_stream(&mut self, p: usize) -> Option<Dict> {
        let (_, _, obj) = self.object_at(p)?;
        let Obj::Stream(d, raw) = obj else { return None };
        if d.get("Type").and_then(Obj::name) != Some("XRef") {
            return None;
        }
        let w: Vec<usize> = d.get("W")?.array()?.iter().map(|o| o.int().unwrap_or(0).max(0) as usize).collect();
        // A field wider than 8 bytes holds nothing a u64 can (and the sum
        // of huge ones would wrap).
        if w.len() < 3 || w.iter().any(|&n| n > 8) || w.iter().sum::<usize>() == 0 {
            return None;
        }
        let size = d.get("Size").and_then(Obj::int).unwrap_or(0);
        let index: Vec<i64> = match d.get("Index").and_then(Obj::array) {
            Some(a) => a.iter().filter_map(Obj::int).collect(),
            None => vec![0, size],
        };
        let data = self.decode(&d, &raw).data;
        let row = w[0] + w[1] + w[2];
        let field = |b: &[u8]| b.iter().fold(0u64, |a, &x| a << 8 | x as u64);
        let mut rows = data.chunks_exact(row);
        for pair in index.chunks(2) {
            let (start, count) = (pair[0], *pair.get(1).unwrap_or(&0));
            for i in 0..count.max(0) {
                let Some(r) = rows.next() else { break };
                let t = if w[0] == 0 { 1 } else { field(&r[..w[0]]) };
                let a = field(&r[w[0]..w[0] + w[1]]);
                let b = field(&r[w[0] + w[1]..]);
                let n = (start + i) as u32;
                if self.xref.contains_key(&n) {
                    continue;
                }
                if self.xref.len() >= MAX_ENTRIES {
                    return Some(d);
                }
                match t {
                    0 => {
                        self.xref.insert(n, Entry::At(0));
                    }
                    1 if a > 0 => {
                        self.xref.insert(n, Entry::At(a as usize));
                    }
                    2 => {
                        self.xref.insert(n, Entry::In(a as u32, b as u32));
                    }
                    _ => {}
                }
            }
        }
        Some(d)
    }

    /// The object `n g obj ... endobj` at `p`.
    fn object_at(&self, p: usize) -> Option<(u32, u16, Obj)> {
        let data = self.data;
        if self.read_left.get() == 0 {
            return None;
        }
        let mut lx = Lexer::new(data, p);
        let (Some(Item::Obj(Obj::Int(n))), Some(Item::Obj(Obj::Int(g))), Some(Item::Kw(b"obj"))) = (lx.next(false), lx.next(false), lx.next(false)) else { return None };
        let (n, g) = (n as u32, g as u16);
        let obj = match lx.next(true)? {
            Item::Obj(o) => o,
            Item::Kw(_) => Obj::Null,
        };
        self.read_left.set(self.read_left.get().saturating_sub(lx.p.saturating_sub(p)));
        let Obj::Dict(d) = obj else { return Some((n, g, obj)) };
        if !matches!(lx.next(false), Some(Item::Kw(b"stream"))) {
            return Some((n, g, Obj::Dict(d)));
        }
        let mut s = lx.p;
        if data.get(s) == Some(&b'\r') {
            s += 1;
        }
        if data.get(s) == Some(&b'\n') {
            s += 1;
        }
        let len = match d.get("Length") {
            Some(Obj::Ref(m, _)) if *m != n => self.get(*m).int(),
            Some(o) => o.int(),
            None => None,
        };
        let fits = |len: usize| {
            let end = s.checked_add(len)?;
            let mut l = Lexer::new(data, end.min(data.len()));
            l.skip_ws();
            data.get(l.p..)?.starts_with(b"endstream").then_some(end)
        };
        let end = len.filter(|&l| l >= 0).and_then(|l| fits(l as usize)).or_else(|| {
            let Some(e) = memchr::memmem::find(&data[s..], b"endstream") else {
                // Searched to the end: charged like a copy.
                self.read_left.set(self.read_left.get().saturating_sub(data.len() - s));
                return None;
            };
            let e = s + e;
            let mut e2 = e;
            if e2 > s && data[e2 - 1] == b'\n' {
                e2 -= 1;
            }
            if e2 > s && data[e2 - 1] == b'\r' {
                e2 -= 1;
            }
            Some(e2)
        })?;
        let raw = data.get(s..end.min(data.len())).unwrap_or_default();
        match self.read_left.get().checked_sub(raw.len()) {
            Some(left) => self.read_left.set(left),
            None => {
                self.read_left.set(0);
                return None;
            }
        }
        Some((n, g, Obj::Stream(d, Arc::from(raw))))
    }

    /// Object `n`, resolved from the file (Null when missing).
    pub fn get(&self, n: u32) -> Obj {
        if let Some(o) = self.cache.borrow().get(&n) {
            return o.clone();
        }
        if !self.loading.borrow_mut().insert(n) {
            return Obj::Null;
        }
        let o = self.load(n).unwrap_or(Obj::Null);
        self.loading.borrow_mut().remove(&n);
        self.cache.borrow_mut().insert(n, o.clone());
        o
    }

    fn load(&self, n: u32) -> Option<Obj> {
        match self.xref.get(&n).copied() {
            Some(Entry::At(0)) | None => self.scan_for(n),
            Some(Entry::At(p)) => {
                let (m, g, o) = match self.object_at(p) {
                    Some(t) if t.0 == n => t,
                    _ => {
                        let o = self.scan_for(n)?;
                        return Some(o);
                    }
                };
                debug_assert_eq!(m, n);
                Some(self.decrypted(n, g, o))
            }
            Some(Entry::In(s, i)) => {
                let stm = self.objstm(s)?;
                let (m, off) = *stm.1.get(i as usize).filter(|e| e.0 == n).or_else(|| stm.1.iter().find(|e| e.0 == n))?;
                debug_assert_eq!(m, n);
                let mut lx = Lexer::new(&stm.0, off);
                match lx.next(true)? {
                    Item::Obj(o) => Some(o),
                    Item::Kw(_) => None,
                }
            }
        }
    }

    /// Some object was found by scanning the file, not by its
    /// cross-references (which were broken or missed it).
    pub(crate) fn repaired(&self) -> bool {
        self.scanned.borrow().is_some()
    }

    /// An object the cross-reference table misses, found by scanning.
    fn scan_for(&self, n: u32) -> Option<Obj> {
        if self.scanned.borrow().is_none() {
            let (map, _) = scan(self.data);
            *self.scanned.borrow_mut() = Some(map);
        }
        let p = *self.scanned.borrow().as_ref()?.get(&n)?;
        let (_, g, o) = self.object_at(p)?;
        Some(self.decrypted(n, g, o))
    }

    fn decrypted(&self, n: u32, g: u16, o: Obj) -> Obj {
        match &self.crypt {
            Some(c) if c.enc_ref != Some(n) => c.object(n, g, o),
            _ => o,
        }
    }

    fn objstm(&self, s: u32) -> Option<ObjStm> {
        if let Some(o) = self.streams.borrow().get(&s) {
            return Some(o.clone());
        }
        let Obj::Stream(d, raw) = self.get(s) else { return None };
        let mut data = self.decode(&d, &raw).data;
        // Each stream may inflate a lot, and they are all kept: past the
        // budget they read as empty (once).
        match self.streams_left.get().checked_sub(data.len()) {
            Some(left) => self.streams_left.set(left),
            None => data = Vec::new(),
        }
        let count = d.get("N").and_then(Obj::int).unwrap_or(0).clamp(0, 1 << 20);
        let first = d.get("First").and_then(Obj::int).unwrap_or(0).max(0) as usize;
        let mut lx = Lexer::new(&data, 0);
        let mut offs = Vec::new();
        for _ in 0..count {
            let (Some(Item::Obj(Obj::Int(m))), Some(Item::Obj(Obj::Int(o)))) = (lx.next(false), lx.next(false)) else { break };
            offs.push((m as u32, first + o.max(0) as usize));
        }
        let s2 = Arc::new((data, offs));
        self.streams.borrow_mut().insert(s, s2.clone());
        Some(s2)
    }

    /// Follows a reference (other objects are returned as they are).
    pub fn resolve(&self, o: &Obj) -> Obj {
        let mut o = o.clone();
        for _ in 0..16 {
            match o {
                Obj::Ref(n, _) => o = self.get(n),
                _ => return o,
            }
        }
        Obj::Null
    }

    /// `d[key]` resolved.
    pub fn key(&self, d: &Dict, key: &str) -> Obj {
        d.get(key).map_or(Obj::Null, |o| self.resolve(o))
    }

    pub fn key_dict(&self, d: &Dict, key: &str) -> Option<Dict> {
        match self.key(d, key) {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }

    pub fn key_num(&self, d: &Dict, key: &str) -> Option<f64> {
        match d.get(key)? {
            o @ Obj::Ref(..) => self.resolve(o).num(),
            o => o.num(),
        }
    }

    /// A number array, each element resolved.
    pub fn nums(&self, o: &Obj) -> Vec<f64> {
        match self.resolve(o) {
            Obj::Array(a) => a.iter().map(|x| if let Obj::Ref(..) = x { self.resolve(x).num() } else { x.num() }.unwrap_or(0.0)).collect(),
            _ => Vec::new(),
        }
    }

    /// The document information dictionary.
    pub fn info(&self) -> Option<Dict> {
        self.key_dict(&self.trailer, "Info")
    }

    /// The pages in order.
    pub fn pages(&self) -> Vec<PageRef> {
        let mut out = Vec::new();
        let Some(root) = self.catalog() else { return out };
        let mut seen = HashSet::new();
        let inherited = Inherited { res: std::rc::Rc::default(), weight: 1, media: None, crop: None, rotate: 0 };
        // Each page keeps its own copy of its resources: so many objects
        // in all (pages sharing a huge dictionary would take terabytes),
        // later pages get none.
        let mut walk = Walk { left: MAX_PAGE_RESOURCES, weights: HashMap::new() };
        if let Some(pages) = root.get("Pages") {
            self.walk(pages, &inherited, &mut out, &mut seen, 0, &mut walk);
        }
        out
    }

    /// `d`'s resources (or `inh`, inherited) for a page, if they are
    /// still affordable; with what they weigh.
    fn resources(&self, d: &Dict, inh: &Inherited, walk: &mut Walk) -> (std::rc::Rc<Dict>, usize) {
        let w = match d.get("Resources") {
            None => inh.weight,
            Some(Obj::Ref(n, _)) => {
                let n = *n;
                *walk.weights.entry(n).or_insert_with(|| self.get(n).weight())
            }
            Some(o) => o.weight(),
        };
        match walk.left.checked_sub(w) {
            Some(left) => {
                walk.left = left;
                (self.key_dict(d, "Resources").map_or_else(|| inh.res.clone(), std::rc::Rc::new), w)
            }
            None => (std::rc::Rc::default(), 1),
        }
    }

    fn walk(&self, node: &Obj, inh: &Inherited, out: &mut Vec<PageRef>, seen: &mut HashSet<u32>, depth: u32, walk: &mut Walk) {
        if depth > 64 || out.len() > 100_000 {
            return;
        }
        if let Obj::Ref(n, _) = node
            && !seen.insert(*n)
        {
            return;
        }
        let Some(d) = self.resolve(node).dict().cloned() else { return };
        let media = d.get("MediaBox").map(|o| self.nums(o)).filter(|v| v.len() == 4).or_else(|| inh.media.clone());
        let crop = d.get("CropBox").map(|o| self.nums(o)).filter(|v| v.len() == 4).or_else(|| inh.crop.clone());
        let rotate = self.key_num(&d, "Rotate").map_or(inh.rotate, |r| r as i64);
        let kids = match self.key(&d, "Kids") {
            Obj::Array(a) => Some(a),
            _ => None,
        };
        let is_pages = d.get("Type").and_then(Obj::name) == Some("Pages") || (kids.is_some() && d.get("Contents").is_none());
        if is_pages {
            // A node's resources are kept while its pages are read.
            let (res, weight) = if d.get("Resources").is_some() { self.resources(&d, inh, walk) } else { (inh.res.clone(), inh.weight) };
            let next = Inherited { res, weight, media, crop, rotate };
            for k in kids.unwrap_or_default() {
                self.walk(&k, &next, out, seen, depth + 1, walk);
            }
            return;
        }
        let (res, _) = self.resources(&d, inh, walk);
        let res = std::rc::Rc::unwrap_or_clone(res);
        let m = media.unwrap_or_else(|| vec![0.0, 0.0, 612.0, 792.0]);
        let norm = |v: &[f64]| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
        let mut mb = norm(&m);
        // An empty media box: US Letter, as pdfium has it.
        if mb[2] - mb[0] < 1.0 || mb[3] - mb[1] < 1.0 {
            mb = [0.0, 0.0, 612.0, 792.0];
        }
        let mut bbox = crop.map_or(mb, |c| {
            let c = norm(&c);
            [c[0].max(mb[0]), c[1].max(mb[1]), c[2].min(mb[2]), c[3].min(mb[3])]
        });
        if bbox[2] - bbox[0] < 1.0 || bbox[3] - bbox[1] < 1.0 {
            bbox = mb;
        }
        out.push(PageRef { dict: d, resources: res, bbox, rotate: rotate.rem_euclid(360) / 90 * 90 });
    }

    /// The page's content streams, decoded and joined.
    pub fn contents(&self, page: &Dict) -> Vec<u8> {
        let mut out = Vec::new();
        let parts = match self.key(page, "Contents") {
            Obj::Array(a) => a,
            o => vec![o],
        };
        for p in parts {
            if let Obj::Stream(d, raw) = self.resolve(&p) {
                // As much as one stream may give, for all of them (an array
                // may name one huge stream many times).
                if out.len() >= MAX_DECODED {
                    break;
                }
                let data = self.decode(&d, &raw).data;
                out.extend_from_slice(&data[..data.len().min(MAX_DECODED - out.len())]);
                out.push(b'\n');
            }
        }
        out
    }

    /// Sets what streams may inflate to from now on (None: no limit);
    /// gives the limit there was.
    pub(crate) fn set_decode_budget(&self, b: Option<usize>) -> Option<usize> {
        self.decode_left.replace(b)
    }

    /// Runs the stream's filters.
    pub fn decode(&self, d: &Dict, raw: &[u8]) -> Decoded {
        if self.decode_left.get() == Some(0) {
            return Decoded { data: Vec::new(), codec: None };
        }
        let out = self.decode_all(d, raw);
        if let Some(left) = self.decode_left.get() {
            self.decode_left.set(Some(left.saturating_sub(out.data.len().max(raw.len()))));
        }
        out
    }

    fn decode_all(&self, d: &Dict, raw: &[u8]) -> Decoded {
        let filters: Vec<Arc<str>> = match self.key(d, "Filter") {
            Obj::Name(n) => vec![n],
            Obj::Array(a) => a.iter().filter_map(|o| self.resolve(o).name().map(Arc::from)).collect(),
            _ => Vec::new(),
        };
        let parms: Vec<Option<Dict>> = match self.key(d, "DecodeParms") {
            Obj::Dict(p) => vec![Some(p)],
            Obj::Array(a) => a.iter().map(|o| self.resolve(o).dict().cloned()).collect(),
            _ => Vec::new(),
        };
        let mut data = raw.to_vec();
        for (i, f) in filters.iter().enumerate() {
            let parm = parms.get(i).cloned().flatten();
            let int = |k: &str, def: i64| parm.as_ref().and_then(|p| p.get(k)).and_then(|o| self.resolve(o).int()).unwrap_or(def);
            data = match &**f {
                "FlateDecode" | "Fl" => predict(inflate(&data), int("Predictor", 1), int("Colors", 1), int("BitsPerComponent", 8), int("Columns", 1)),
                "LZWDecode" | "LZW" => predict(lzw(&data, int("EarlyChange", 1) != 0), int("Predictor", 1), int("Colors", 1), int("BitsPerComponent", 8), int("Columns", 1)),
                "ASCIIHexDecode" | "AHx" => ascii_hex(&data),
                "ASCII85Decode" | "A85" => ascii85(&data),
                "RunLengthDecode" | "RL" => run_length(&data),
                "Crypt" => data,
                _ => return Decoded { data, codec: Some(f.clone()) },
            };
        }
        Decoded { data, codec: None }
    }
}

/// Runs the standard filter `name` (not the image codecs).
pub(super) fn filter(name: &str, data: &[u8], pred: i64, colors: i64, bpc: i64, columns: i64, early: bool) -> Vec<u8> {
    match name {
        "FlateDecode" | "Fl" => predict(inflate(data), pred, colors, bpc, columns),
        "LZWDecode" | "LZW" => predict(lzw(data, early), pred, colors, bpc, columns),
        "ASCIIHexDecode" | "AHx" => ascii_hex(data),
        "ASCII85Decode" | "A85" => ascii85(data),
        "RunLengthDecode" | "RL" => run_length(data),
        _ => data.to_vec(),
    }
}

/// Finds every `n g obj` in the file, and the last trailer dictionary.
fn scan(data: &[u8]) -> (HashMap<u32, usize>, Dict) {
    let mut map = HashMap::new();
    let mut trailer = Dict::default();
    for at in memchr::memmem::find_iter(data, b"obj") {
        // Back over "n g " to the object number.
        let mut i = at;
        let mut fields = 0;
        let mut start = at;
        while i > 0 && fields < 2 {
            while i > 0 && matches!(data[i - 1], b' ' | b'\r' | b'\n' | b'\t') {
                i -= 1;
            }
            let e = i;
            while i > 0 && data[i - 1].is_ascii_digit() {
                i -= 1;
            }
            if i == e {
                break;
            }
            fields += 1;
            start = i;
        }
        if fields == 2 && data.get(at + 3).is_none_or(|&b| !b.is_ascii_alphanumeric()) && (start == 0 || !data[start - 1].is_ascii_alphanumeric()) {
            let mut lx = Lexer::new(data, start);
            if let Some(Item::Obj(Obj::Int(n))) = lx.next(false) {
                map.insert(n as u32, start);
            }
        }
    }
    let mut roots: Vec<Dict> = Vec::new();
    // Dictionaries that never close are lexed up to a few times the file.
    let mut left = data.len().saturating_mul(4).saturating_add(64 << 20);
    for at in memchr::memmem::find_iter(data, b"trailer") {
        if left == 0 {
            break;
        }
        let mut lx = Lexer::new(data, at + 7);
        let d = lx.next(true);
        left = left.saturating_sub(lx.p.saturating_sub(at));
        if let Some(Item::Obj(Obj::Dict(d))) = d {
            roots.push(d);
        }
    }
    // Cross-reference streams carry the trailer's keys too.
    // The last '<' before each, looked for once over the whole file.
    let (mut seen, mut last_lt, mut tried) = (0, None, None);
    for at in memchr::memmem::find_iter(data, b"/XRef") {
        if let Some(p) = data[seen..at].iter().rposition(|&b| b == b'<') {
            last_lt = Some(seen + p);
        }
        seen = at;
        let s = last_lt.map(|p: usize| p.saturating_sub(1));
        if s == tried {
            continue;
        }
        tried = s;
        if let Some(s) = s
            && left > 0
        {
            let mut lx = Lexer::new(data, s);
            let d = lx.next(true);
            left = left.saturating_sub(lx.p.saturating_sub(s));
            if let Some(Item::Obj(Obj::Dict(d))) = d
                && d.get("Root").is_some()
            {
                roots.push(d);
            }
        }
    }
    for d in roots {
        for (k, v) in d.0 {
            if matches!(&*k, "Root" | "Info" | "Encrypt" | "ID") {
                trailer.0.retain(|(k2, _)| *k2 != k);
                trailer.0.push((k, v));
            }
        }
    }
    (map, trailer)
}

/// The most one filter of a stream may make: a few kilobytes inflating
/// to gigabytes (a "bomb") stop here, with what came before.
pub(super) const MAX_DECODED: usize = 256 << 20;

/// The most objects a cross-reference stream may list.
const MAX_ENTRIES: usize = 1 << 23;

/// The most all the object streams of a file kept together may hold.
const STREAMS_BUDGET: usize = 1 << 30;

fn inflate(data: &[u8]) -> Vec<u8> {
    use miniz_oxide::inflate::{decompress_to_vec_with_limit, decompress_to_vec_zlib_with_limit};
    match decompress_to_vec_zlib_with_limit(data, MAX_DECODED) {
        Ok(v) => v,
        Err(e) if !e.output.is_empty() => e.output,
        Err(_) => match decompress_to_vec_with_limit(data, MAX_DECODED) {
            Ok(v) => v,
            Err(e) => e.output,
        },
    }
}

fn predict(data: Vec<u8>, pred: i64, colors: i64, bpc: i64, columns: i64) -> Vec<u8> {
    if pred < 2 {
        return data;
    }
    // The sizes come from the file: a row longer than the data is no
    // row (and would be a huge allocation).
    let (colors, bpc, columns) = (colors.clamp(1, 32) as i128, bpc.clamp(1, 16) as i128, columns.max(1) as i128);
    let bpp = ((colors * bpc + 7) / 8).max(1) as usize;
    let row = (colors * bpc * columns + 7) / 8;
    if row > data.len() as i128 {
        return data;
    }
    let row = row.max(1) as usize;
    if pred == 2 {
        // TIFF predictor (8-bit components only).
        let mut out = data;
        if bpc == 8 {
            for r in out.chunks_mut(row) {
                for i in bpp..r.len() {
                    r[i] = r[i].wrapping_add(r[i - bpp]);
                }
            }
        }
        return out;
    }
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        let kind = chunk[0];
        let mut cur = chunk[1..].to_vec();
        cur.resize(row, 0);
        for i in 0..row {
            let a = if i >= bpp { cur[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            cur[i] = cur[i].wrapping_add(match kind {
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => {
                    let p = a as i16 + b as i16 - c as i16;
                    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => 0,
            });
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    out
}

fn lzw(data: &[u8], early: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut table: Vec<Vec<u8>> = (0..256).map(|i| vec![i as u8]).collect();
    table.push(Vec::new());
    table.push(Vec::new());
    let mut width = 9;
    let (mut buf, mut bits) = (0u32, 0u32);
    let mut prev: Option<Vec<u8>> = None;
    for &b in data {
        buf = buf << 8 | b as u32;
        bits += 8;
        while bits >= width {
            let code = (buf >> (bits - width)) & ((1 << width) - 1);
            bits -= width;
            buf &= (1 << bits) - 1;
            match code {
                256 => {
                    table.truncate(258);
                    width = 9;
                    prev = None;
                    continue;
                }
                257 => return out,
                _ => {}
            }
            let entry = if (code as usize) < table.len() {
                table[code as usize].clone()
            } else if let Some(p) = &prev {
                let mut e = p.clone();
                e.push(p[0]);
                e
            } else {
                return out;
            };
            out.extend_from_slice(&entry);
            if out.len() > MAX_DECODED {
                out.truncate(MAX_DECODED);
                return out;
            }
            // Codes are 12 bits at most: the table stops growing there.
            if let Some(p) = prev
                && table.len() < 4096
            {
                let mut e = p;
                e.push(entry[0]);
                table.push(e);
            }
            prev = Some(entry);
            let limit = table.len() + usize::from(early);
            width = if limit >= 2048 { 12 } else if limit >= 1024 { 11 } else if limit >= 512 { 10 } else { 9 };
        }
    }
    out
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut hi: Option<u8> = None;
    for &b in data {
        if b == b'>' {
            break;
        }
        if let Some(v) = (b as char).to_digit(16) {
            match hi.take() {
                Some(h) => out.push(h << 4 | v as u8),
                None => hi = Some(v as u8),
            }
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    out
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0u32; 5];
    let mut n = 0;
    let data = data.strip_prefix(b"<~").unwrap_or(data);
    for &b in data {
        match b {
            b'~' => break,
            _ if out.len() >= MAX_DECODED => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = (b - b'!') as u32;
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u32, |a, &x| a.wrapping_mul(85).wrapping_add(x));
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let v = group.iter().fold(0u32, |a, &x| a.wrapping_mul(85).wrapping_add(x));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let l = data[i];
        i += 1;
        match l {
            128 => break,
            0..=127 => {
                let e = (i + l as usize + 1).min(data.len());
                out.extend_from_slice(&data[i..e]);
                i = e;
            }
            _ => {
                if let Some(&b) = data.get(i) {
                    out.extend(std::iter::repeat_n(b, 257 - l as usize));
                }
                i += 1;
            }
        }
        // Each byte may give 128: as far as the other filters go.
        if out.len() > MAX_DECODED {
            out.truncate(MAX_DECODED);
            break;
        }
    }
    out
}

// ---- The standard security handler -------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Method {
    None,
    Rc4,
    Aes,
}

pub(super) struct Crypt {
    key: Vec<u8>,
    stm: Method,
    strs: Method,
    /// AES-256 (revisions 5 and 6): the key is used as it is.
    v5: bool,
    pub(super) enc_ref: Option<u32>,
}

const PAD: [u8; 32] = [0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08, 0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A];

fn md5(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = md5::Md5::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn padded(pw: &[u8]) -> [u8; 32] {
    let mut out = PAD;
    let n = pw.len().min(32);
    out[..n].copy_from_slice(&pw[..n]);
    out[n..].copy_from_slice(&PAD[..32 - n]);
    out
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    if !key.is_empty() {
        crate::bin_crypto::Rc4::new(key).apply(&mut out);
    }
    out
}

impl Crypt {
    fn new(d: &Dict, id0: &[u8], password: Option<&str>, enc_ref: Option<u32>) -> Result<Crypt, Error> {
        if d.get("Filter").and_then(Obj::name).is_some_and(|f| f != "Standard") {
            return Err(Error::new("unsupported PDF encryption"));
        }
        let v = d.get("V").and_then(Obj::int).unwrap_or(0);
        let r = d.get("R").and_then(Obj::int).unwrap_or(2);
        let o = d.get("O").and_then(Obj::bytes).unwrap_or_default().to_vec();
        let u = d.get("U").and_then(Obj::bytes).unwrap_or_default().to_vec();
        let p = d.get("P").and_then(Obj::int).unwrap_or(-1) as i32;
        let meta = d.get("EncryptMetadata") != Some(&Obj::Bool(false));
        // Crypt filters (V 4 and 5).
        let method = |name: Option<&str>| -> Method {
            match name {
                None | Some("Identity") => Method::None,
                Some(n) => match d.get("CF").and_then(Obj::dict).and_then(|cf| cf.get(n)).and_then(Obj::dict).and_then(|f| f.get("CFM")).and_then(Obj::name) {
                    Some("AESV2" | "AESV3") => Method::Aes,
                    Some("None") => Method::None,
                    _ => Method::Rc4,
                },
            }
        };
        let (stm, strs) = if v >= 4 { (method(d.get("StmF").and_then(Obj::name)), method(d.get("StrF").and_then(Obj::name))) } else { (Method::Rc4, Method::Rc4) };
        // Only attachments encrypted (/EFF): the document opens as it is.
        if v >= 4 && stm == Method::None && strs == Method::None {
            return Ok(Crypt { key: Vec::new(), stm, strs, v5: r >= 5, enc_ref });
        }
        let pw = password.unwrap_or("");
        let fail = || if password.is_some() { Error::new(crate::WRONG_PASSWORD) } else { crate::no_password() };
        if r >= 5 {
            let pwb: Vec<u8> = pw.as_bytes().iter().take(127).copied().collect();
            let ue = d.get("UE").and_then(Obj::bytes).unwrap_or_default();
            let oe = d.get("OE").and_then(Obj::bytes).unwrap_or_default();
            if u.len() < 48 || o.len() < 48 {
                return Err(Error::new("the PDF file is damaged"));
            }
            let key = if hash_v5(r, &pwb, &u[32..40], &[]) == u[..32] {
                aes256_unwrap(&hash_v5(r, &pwb, &u[40..48], &[]), ue)
            } else if hash_v5(r, &pwb, &o[32..40], &u[..48]) == o[..32] {
                aes256_unwrap(&hash_v5(r, &pwb, &o[40..48], &u[..48]), oe)
            } else {
                return Err(fail());
            };
            return Ok(Crypt { key, stm, strs, v5: true, enc_ref });
        }
        let len = if r == 2 { 5 } else { (d.get("Length").and_then(Obj::int).unwrap_or(40) / 8).clamp(5, 16) as usize };
        let len = if stm == Method::Aes || strs == Method::Aes { 16 } else { len };
        let key_of = |pad: &[u8; 32]| -> Vec<u8> {
            let mut h = md5::Md5::new();
            h.update(pad);
            h.update(&o[..o.len().min(32)]);
            h.update(p.to_le_bytes());
            h.update(id0);
            if r >= 4 && !meta {
                h.update([0xFF; 4]);
            }
            let mut k: [u8; 16] = h.finalize().into();
            if r >= 3 {
                for _ in 0..50 {
                    k = md5(&[&k[..len]]);
                }
            }
            k[..len].to_vec()
        };
        let user_ok = |key: &[u8]| -> bool {
            if r == 2 {
                rc4(key, &PAD) == u.get(..32).unwrap_or_default()
            } else {
                let mut x = md5(&[&PAD, id0]).to_vec();
                for i in 0..20u8 {
                    let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
                    x = rc4(&k, &x);
                }
                u.len() >= 16 && x[..16] == u[..16]
            }
        };
        let latin: Vec<u8> = pw.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
        let key = key_of(&padded(&latin));
        if user_ok(&key) {
            return Ok(Crypt { key, stm, strs, v5: false, enc_ref });
        }
        // The owner password gives the user's.
        let mut h = md5(&[&padded(&latin)]);
        if r >= 3 {
            for _ in 0..50 {
                h = md5(&[&h]);
            }
        }
        let okey = &h[..len];
        let mut up = o[..o.len().min(32)].to_vec();
        if r == 2 {
            up = rc4(okey, &up);
        } else {
            for i in (0..20u8).rev() {
                let k: Vec<u8> = okey.iter().map(|b| b ^ i).collect();
                up = rc4(&k, &up);
            }
        }
        let key = key_of(&padded(&up));
        if user_ok(&key) {
            return Ok(Crypt { key, stm, strs, v5: false, enc_ref });
        }
        Err(fail())
    }

    fn object_key(&self, n: u32, g: u16, aes: bool) -> Vec<u8> {
        if self.v5 {
            return self.key.clone();
        }
        let mut h = md5::Md5::new();
        h.update(&self.key);
        h.update(&n.to_le_bytes()[..3]);
        h.update(g.to_le_bytes());
        if aes {
            h.update(b"sAlT");
        }
        let full: [u8; 16] = h.finalize().into();
        full[..(self.key.len() + 5).min(16)].to_vec()
    }

    fn apply(&self, m: Method, n: u32, g: u16, data: &[u8]) -> Vec<u8> {
        match m {
            Method::None => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(n, g, false), data),
            Method::Aes => {
                if data.len() < 32 {
                    return Vec::new();
                }
                let key = self.object_key(n, g, true);
                let body = &data[16..data.len() - (data.len() - 16) % 16];
                let mut out = crate::crypto::cbc_decrypt(&key, &data[..16], body).unwrap_or_default();
                if let Some(&pad) = out.last()
                    && (1..=16).contains(&pad)
                    && out.len() >= pad as usize
                {
                    out.truncate(out.len() - pad as usize);
                }
                out
            }
        }
    }

    fn apply_enc(&self, m: Method, n: u32, g: u16, data: &[u8]) -> Vec<u8> {
        match m {
            Method::None => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(n, g, false), data),
            Method::Aes => {
                let key = self.object_key(n, g, true);
                let iv = crate::crypto::random(16);
                // PKCS#7 padding.
                let pad = 16 - data.len() % 16;
                let mut body = data.to_vec();
                body.extend(std::iter::repeat_n(pad as u8, pad));
                let mut out = iv.clone();
                out.extend(crate::crypto::cbc_encrypt(&key, &iv, &body).unwrap_or_default());
                out
            }
        }
    }

    /// Object `n` encrypted for writing (the reverse of [`Crypt::object`]).
    pub(super) fn encrypt(&self, n: u32, g: u16, o: &Obj) -> Obj {
        if self.enc_ref == Some(n) {
            return o.clone();
        }
        match o {
            Obj::Str(s) => Obj::Str(self.apply_enc(self.strs, n, g, s)),
            Obj::Array(a) => Obj::Array(a.iter().map(|x| self.encrypt(n, g, x)).collect()),
            Obj::Dict(d) => Obj::Dict(Dict(d.0.iter().map(|(k, v)| (k.clone(), self.encrypt(n, g, v))).collect())),
            Obj::Stream(d, data) => {
                let xref = d.get("Type").and_then(Obj::name) == Some("XRef");
                let data = if xref { data.clone() } else { Arc::from(self.apply_enc(self.stm, n, g, data)) };
                Obj::Stream(Dict(d.0.iter().map(|(k, v)| (k.clone(), self.encrypt(n, g, v))).collect()), data)
            }
            o => o.clone(),
        }
    }

    fn object(&self, n: u32, g: u16, o: Obj) -> Obj {
        match o {
            Obj::Str(s) => Obj::Str(self.apply(self.strs, n, g, &s)),
            Obj::Array(a) => Obj::Array(a.into_iter().map(|x| self.object(n, g, x)).collect()),
            Obj::Dict(d) => Obj::Dict(self.dict(n, g, d)),
            Obj::Stream(d, data) => {
                let xref = d.get("Type").and_then(Obj::name) == Some("XRef");
                let identity = matches!(d.get("Filter"), Some(Obj::Array(a)) if a.first().and_then(Obj::name) == Some("Crypt"))
                    || d.get("Filter").and_then(Obj::name) == Some("Crypt");
                let data = if xref || identity { data } else { Arc::from(self.apply(self.stm, n, g, &data)) };
                Obj::Stream(self.dict(n, g, d), data)
            }
            o => o,
        }
    }

    fn dict(&self, n: u32, g: u16, d: Dict) -> Dict {
        Dict(d.0.into_iter().map(|(k, v)| (k, self.object(n, g, v))).collect())
    }
}

/// The hash of revisions 5 (SHA-256) and 6 (ISO 32000-2's algorithm 2.B).
fn hash_v5(r: i64, pw: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut k: Vec<u8> = sha2::Sha256::new().chain_update(pw).chain_update(salt).chain_update(udata).finalize().to_vec();
    if r == 5 {
        return k;
    }
    let mut i = 0u32;
    loop {
        let mut k1 = Vec::with_capacity(64 * (pw.len() + k.len() + udata.len()));
        for _ in 0..64 {
            k1.extend_from_slice(pw);
            k1.extend_from_slice(&k);
            k1.extend_from_slice(udata);
        }
        let e = aes128_cbc(&k[..16], &k[16..32], &k1);
        let m = e[..16].iter().map(|&b| b as u32).sum::<u32>() % 3;
        k = match m {
            0 => sha2::Sha256::digest(&e).to_vec(),
            1 => sha2::Sha384::digest(&e).to_vec(),
            _ => sha2::Sha512::digest(&e).to_vec(),
        };
        i += 1;
        if i >= 64 && (*e.last().unwrap_or(&0) as u32) <= i - 32 {
            break;
        }
    }
    k.truncate(32);
    k
}

fn aes128_cbc(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let Ok(aes) = crate::crypto::Aes::new(key) else { return Vec::new() };
    let mut prev = [0u8; 16];
    prev.copy_from_slice(&iv[..16]);
    let mut out = Vec::with_capacity(data.len());
    for c in data.chunks_exact(16) {
        let mut b = [0u8; 16];
        for i in 0..16 {
            b[i] = c[i] ^ prev[i];
        }
        aes.encrypt(&mut b);
        out.extend_from_slice(&b);
        prev = b;
    }
    out
}

/// Decrypts the file key (UE or OE) with a zero IV and no padding.
fn aes256_unwrap(key: &[u8], data: &[u8]) -> Vec<u8> {
    crate::crypto::cbc_decrypt(key, &[0u8; 16], data).unwrap_or_default()
}

use sha2::Digest as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        assert_eq!(ascii85(b"<~87cURD]i,\"Ebo80~>"), b"Hello World!".to_vec());
        assert_eq!(ascii_hex(b"48 65 6c6c6f>"), b"Hello".to_vec());
        assert_eq!(run_length(&[2, b'a', b'b', b'c', 254, b'x', 128]), b"abcxxx".to_vec());
        // "-----A---B" as LZW, from the PDF specification.
        let lzw_data = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw(&lzw_data, true), b"-----A---B".to_vec());
        // PNG "up" predictor over two rows of three bytes.
        assert_eq!(predict(vec![2, 1, 2, 3, 2, 1, 1, 1], 12, 1, 8, 3), vec![1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn hostile_filter_parameters_stay_bounded() {
        // Rows longer than the data, or absurd colours and depths, leave it as is.
        assert_eq!(predict(vec![2, 1, 2], 12, 1, 8, i64::MAX), vec![2, 1, 2]);
        assert_eq!(predict(vec![2, 1, 2], 12, i64::MAX, i64::MAX, 1), vec![2, 1, 2]);
        // A long run of 12-bit codes never grows the table past 4096 entries.
        let codes = vec![0x55u8; 20_000];
        let _ = lzw(&codes, true);
        let _ = lzw(&codes, false);
    }
}
