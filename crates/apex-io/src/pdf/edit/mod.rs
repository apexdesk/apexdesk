//! Editing a PDF as it is: its objects read into a table, changed in steps
//! that can be undone, and written back either as an incremental update
//! (what was there stays byte for byte, so earlier signatures stay valid)
//! or as a whole new file (after redacting, when what was there must go).
//!
//! Positions in this module's interface are points on the page as shown:
//! from the top left corner, with the page's rotation applied, as in
//! [`super::Page`].

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use super::file::{Crypt, PdfFile};
use super::obj::{Dict, Obj};
use crate::Error;

pub mod annots;
pub mod appearance;
#[cfg(feature = "empresa")]
pub mod circuit;
pub mod content;
pub mod dossier;
pub mod forms;
pub mod organize;
pub mod outline;
pub mod props;
pub mod redact;
pub mod stamp;
mod write;

pub use write::deflate;

/// One undoable change: the objects as they were before it.
struct Step {
    label: &'static str,
    before: Vec<(u32, Option<(u16, Obj)>)>,
    trailer: Option<Dict>,
    whole: bool,
}

pub struct PdfDoc {
    /// The file as last read or saved.
    base: Arc<[u8]>,
    objs: BTreeMap<u32, (u16, Obj)>,
    trailer: Dict,
    /// Objects changed or added since `base`.
    changed: BTreeSet<u32>,
    /// Only a whole new file will do (redaction).
    whole: bool,
    /// Where `base`'s last cross-reference section is, and whether it is
    /// a stream; none when they were broken.
    base_xref: Option<(usize, bool)>,
    /// Read by scanning the file, its cross-references broken: other
    /// readers may repair it otherwise, and show something else.
    repaired: bool,
    crypt: Option<Arc<Crypt>>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    open: Option<Step>,
    depth: u32,
    /// Grows with every change (and undo), for caches.
    pub revision: u64,
    saved: u64,
}

impl std::fmt::Debug for PdfDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PdfDoc({} objects)", self.objs.len())
    }
}

/// How a page's user space maps to the page as shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    /// User space to shown (points from the top left).
    pub m: [f64; 6],
    /// Size as shown.
    pub width: f64,
    pub height: f64,
    pub rotate: i64,
    /// The visible box in user space.
    pub bbox: [f64; 4],
}

pub(crate) fn apply(m: &[f64; 6], x: f64, y: f64) -> [f64; 2] {
    [x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5]]
}

pub(crate) fn invert(m: &[f64; 6]) -> [f64; 6] {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    [a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)]
}

impl Geom {
    pub fn of(bbox: [f64; 4], rotate: i64) -> Geom {
        let [x0, y0, x1, y1] = bbox;
        let (w, h) = (x1 - x0, y1 - y0);
        let (m, width, height) = match rotate {
            90 => ([0.0, 1.0, 1.0, 0.0, -y0, -x0], h, w),
            180 => ([-1.0, 0.0, 0.0, 1.0, x1, -y0], w, h),
            270 => ([0.0, -1.0, -1.0, 0.0, y1, x1], h, w),
            _ => ([1.0, 0.0, 0.0, -1.0, -x0, y1], w, h),
        };
        Geom { m, width, height, rotate, bbox }
    }

    /// A point shown at (x, y), in user space.
    pub fn to_user(&self, x: f64, y: f64) -> [f64; 2] {
        apply(&invert(&self.m), x, y)
    }

    pub fn to_view(&self, x: f64, y: f64) -> [f64; 2] {
        apply(&self.m, x, y)
    }

    /// A rectangle shown (x0, y0, x1, y1), in user space (normalized).
    pub fn rect_to_user(&self, r: [f32; 4]) -> [f64; 4] {
        let a = self.to_user(r[0] as f64, r[1] as f64);
        let b = self.to_user(r[2] as f64, r[3] as f64);
        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
    }

    pub fn rect_to_view(&self, r: [f64; 4]) -> [f32; 4] {
        let a = self.to_view(r[0], r[1]);
        let b = self.to_view(r[2], r[3]);
        [a[0].min(b[0]) as f32, a[1].min(b[1]) as f32, a[0].max(b[0]) as f32, a[1].max(b[1]) as f32]
    }

    /// The matrix of an appearance drawn upright as shown, `w` by `h`
    /// shown, from its bottom left corner: maps it onto user space. (The
    /// annotation's rectangle then is the user space box it covers.)
    pub fn upright(&self) -> [f64; 6] {
        match self.rotate {
            90 => [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
            180 => [-1.0, 0.0, 0.0, -1.0, 0.0, 0.0],
            270 => [0.0, -1.0, 1.0, 0.0, 0.0, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

/// A text string: PDFDocEncoding when it is plain ASCII, otherwise UTF-16
/// with a byte order mark.
pub fn text_obj(s: &str) -> Obj {
    if s.chars().all(|c| (' '..='~').contains(&c) || c == '\n' || c == '\r' || c == '\t') {
        Obj::Str(s.as_bytes().to_vec())
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Obj::Str(b)
    }
}

/// A string object's text.
pub fn obj_text(o: &Obj) -> String {
    o.bytes().map(super::text_string).unwrap_or_default()
}

/// Now as a PDF date (`D:YYYYMMDDHHmmSSZ`, UTC).
pub fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    pdf_date(secs)
}

/// Unix time `secs` as a PDF date in UTC.
pub fn pdf_date(secs: i64) -> String {
    let (y, mo, d) = civil(secs.div_euclid(86400));
    let t = secs.rem_euclid(86400);
    format!("D:{y:04}{mo:02}{d:02}{:02}{:02}{:02}Z", t / 3600, t / 60 % 60, t % 60)
}

/// Year, month and day of a day number from 1970-01-01.
pub(crate) fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// A PDF date shown as "YYYY-MM-DD HH:MM".
pub fn show_date(s: &str) -> String {
    let d = s.trim_start_matches("D:");
    let dig: String = d.chars().take_while(char::is_ascii_digit).collect();
    if dig.len() < 8 {
        return s.to_string();
    }
    let mut out = format!("{}-{}-{}", &dig[0..4], &dig[4..6], &dig[6..8]);
    if dig.len() >= 12 {
        out.push_str(&format!(" {}:{}", &dig[8..10], &dig[10..12]));
    }
    out
}

impl PdfDoc {
    /// Reads a PDF (encrypted ones with `password`).
    pub fn open(data: Vec<u8>, password: Option<&str>) -> Result<PdfDoc, Error> {
        // A damaged file fails the opening, not the app.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| PdfDoc::open_unguarded(data, password))).unwrap_or_else(|_| Err(Error::new("the PDF file is damaged")))
    }

    fn open_unguarded(data: Vec<u8>, password: Option<&str>) -> Result<PdfDoc, Error> {
        let base: Arc<[u8]> = Arc::from(data);
        let mut objs = BTreeMap::new();
        let (trailer, crypt, repaired) = {
            let mut f = PdfFile::open(&base, password)?;
            for (n, g) in f.entries() {
                let o = f.get(n);
                // Cross-reference and object streams are the file's
                // structure, written anew.
                if let Obj::Stream(d, _) = &o
                    && matches!(d.get("Type").and_then(Obj::name), Some("XRef" | "ObjStm"))
                {
                    continue;
                }
                if o != Obj::Null {
                    objs.insert(n, (g, o));
                }
            }
            let mut trailer = f.trailer.clone();
            for k in ["Prev", "XRefStm", "Index", "W", "Filter", "DecodeParms", "Length", "Type"] {
                trailer.remove(k);
            }
            let repaired = f.repaired();
            (trailer, f.take_crypt(), repaired)
        };
        let base_xref = last_xref(&base);
        let doc = PdfDoc { base, objs, trailer, changed: BTreeSet::new(), whole: false, base_xref, repaired, crypt: crypt.map(Arc::new), undo: Vec::new(), redo: Vec::new(), open: None, depth: 0, revision: 1, saved: 1 };
        if doc.catalog().is_none() {
            return Err(Error::new("the PDF file is damaged"));
        }
        Ok(doc)
    }

    /// The file as last read or saved.
    pub fn base(&self) -> &[u8] {
        &self.base
    }

    /// Whether the file was read by scanning it, its cross-references
    /// broken (then not one to sign as it is).
    pub fn repaired(&self) -> bool {
        self.repaired
    }

    pub fn is_encrypted(&self) -> bool {
        self.crypt.is_some()
    }

    pub fn modified(&self) -> bool {
        self.revision != self.saved
    }

    /// Marks the document as saved as it is now.
    pub fn mark_saved(&mut self) {
        self.saved = self.revision;
    }

    /// Marks the document as having unsaved changes (a recovered copy).
    pub fn mark_unsaved(&mut self) {
        self.saved = u64::MAX;
    }

    /// Whether saving adds an update to the file (keeping signatures).
    pub fn saves_incrementally(&self) -> bool {
        !self.whole && self.base_xref.is_some()
    }

    // --- objects -----------------------------------------------------------------------

    pub fn get(&self, n: u32) -> Option<&Obj> {
        self.objs.get(&n).map(|(_, o)| o)
    }

    /// Follows references.
    pub fn resolve(&self, o: &Obj) -> Obj {
        let mut o = o.clone();
        for _ in 0..16 {
            match o {
                Obj::Ref(n, _) => o = self.get(n).cloned().unwrap_or(Obj::Null),
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

    pub fn nums(&self, o: &Obj) -> Vec<f64> {
        match self.resolve(o) {
            Obj::Array(a) => a.iter().map(|x| self.resolve(x).num().unwrap_or(0.0)).collect(),
            _ => Vec::new(),
        }
    }

    /// Object `n` as a dictionary (a stream's too).
    pub fn dict(&self, n: u32) -> Option<Dict> {
        self.get(n).and_then(Obj::dict).cloned()
    }

    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    /// The largest object number in use.
    fn max_n(&self) -> u32 {
        self.objs.keys().next_back().copied().unwrap_or(0)
    }

    // --- changes -----------------------------------------------------------------------

    /// Starts a step called `label` (steps begun inside one join it).
    pub fn begin(&mut self, label: &'static str) {
        if self.depth == 0 {
            self.open = Some(Step { label, before: Vec::new(), trailer: None, whole: self.whole });
        }
        self.depth += 1;
    }

    /// Ends the step begun last: kept for undo if it changed something.
    pub fn end(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth > 0 {
            return;
        }
        if let Some(s) = self.open.take()
            && (!s.before.is_empty() || s.trailer.is_some() || s.whole != self.whole)
        {
            self.undo.push(s);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.revision += 1;
        }
    }

    fn note(&mut self, n: u32) {
        let old = self.objs.get(&n).cloned();
        if let Some(s) = &mut self.open
            && !s.before.iter().any(|(m, _)| *m == n)
        {
            s.before.push((n, old));
        }
        self.changed.insert(n);
    }

    /// Replaces object `n` (inside a step).
    pub fn set(&mut self, n: u32, o: Obj) {
        self.note(n);
        let g = self.objs.get(&n).map_or(0, |e| e.0);
        self.objs.insert(n, (g, o));
    }

    /// Adds an object; returns its number.
    pub fn add(&mut self, o: Obj) -> u32 {
        let n = self.max_n() + 1;
        self.note(n);
        self.objs.insert(n, (0, o));
        n
    }

    /// Removes object `n` (it is left out of the file when saved whole).
    pub fn remove(&mut self, n: u32) {
        if self.objs.contains_key(&n) {
            self.note(n);
            self.objs.remove(&n);
        }
    }

    pub fn set_trailer(&mut self, t: Dict) {
        if let Some(s) = &mut self.open
            && s.trailer.is_none()
        {
            s.trailer = Some(self.trailer.clone());
        }
        self.trailer = t;
    }

    /// From now on the file must be written whole (what was there goes).
    pub fn need_whole(&mut self) {
        self.whole = true;
    }

    /// After removing something: an update added to the file would leave
    /// it in the bytes under it (cut at the first %%EOF and it's back), so
    /// the file is written whole, unless signatures need it as it was.
    pub(crate) fn removing(&mut self) {
        if !self.is_signed() {
            self.need_whole();
        }
    }

    /// A reference to `n` with its generation.
    pub fn r(&self, n: u32) -> Obj {
        Obj::Ref(n, self.objs.get(&n).map_or(0, |e| e.0))
    }

    fn swap(&mut self, s: Step) -> Step {
        let mut back = Step { label: s.label, before: Vec::new(), trailer: None, whole: self.whole };
        for (n, old) in s.before.into_iter().rev() {
            back.before.push((n, self.objs.get(&n).cloned()));
            match old {
                Some(e) => {
                    self.objs.insert(n, e);
                }
                None => {
                    self.objs.remove(&n);
                }
            }
            self.changed.insert(n);
        }
        if let Some(t) = s.trailer {
            back.trailer = Some(std::mem::replace(&mut self.trailer, t));
        }
        self.whole = s.whole;
        self.revision += 1;
        back
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        let back = self.swap(s);
        self.redo.push(back);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        let back = self.swap(s);
        self.undo.push(back);
        true
    }

    pub fn undo_names(&self) -> Vec<&'static str> {
        self.undo.iter().rev().map(|s| s.label).collect()
    }

    pub fn redo_names(&self) -> Vec<&'static str> {
        self.redo.iter().rev().map(|s| s.label).collect()
    }

    // --- structure ---------------------------------------------------------------------

    /// The catalog's number and dictionary.
    pub fn catalog(&self) -> Option<(u32, Dict)> {
        match self.trailer.get("Root") {
            Some(Obj::Ref(n, _)) => self.dict(*n).filter(|d| d.get("Pages").is_some()).map(|d| (*n, d)),
            _ => None,
        }
    }

    /// Sets a catalog entry (inside a step).
    pub fn set_catalog(&mut self, key: &str, v: Option<Obj>) {
        let Some((n, mut d)) = self.catalog() else { return };
        match v {
            Some(v) => d.set(key, v),
            None => {
                d.remove(key);
            }
        }
        self.set(n, Obj::Dict(d));
    }

    /// The pages' object numbers in order.
    pub fn pages(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let Some((_, cat)) = self.catalog() else { return out };
        let mut seen = HashSet::new();
        if let Some(Obj::Ref(n, _)) = cat.get("Pages") {
            self.walk(*n, &mut out, &mut seen, 0);
        }
        out
    }

    fn walk(&self, n: u32, out: &mut Vec<u32>, seen: &mut HashSet<u32>, depth: u32) {
        if depth > 64 || !seen.insert(n) || out.len() > 100_000 {
            return;
        }
        let Some(d) = self.dict(n) else { return };
        let kids = match self.key(&d, "Kids") {
            Obj::Array(a) => Some(a),
            _ => None,
        };
        let is_pages = d.get("Type").and_then(Obj::name) == Some("Pages") || (kids.is_some() && d.get("Contents").is_none());
        if !is_pages {
            out.push(n);
            return;
        }
        for k in kids.unwrap_or_default() {
            if let Obj::Ref(m, _) = k {
                self.walk(m, out, seen, depth + 1);
            }
        }
    }

    /// Attribute `key` of page `n`, inherited from its parents.
    pub fn inherited(&self, n: u32, key: &str) -> Option<Obj> {
        let mut cur = n;
        for _ in 0..64 {
            let d = self.dict(cur)?;
            if let Some(v) = d.get(key) {
                return Some(self.resolve(v));
            }
            match d.get("Parent") {
                Some(Obj::Ref(p, _)) => cur = *p,
                _ => return None,
            }
        }
        None
    }

    /// How page `n` (object number) is shown.
    pub fn geom(&self, n: u32) -> Geom {
        let norm = |v: &[f64]| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
        let media = self.inherited(n, "MediaBox").map(|o| self.nums(&o)).filter(|v| v.len() == 4).map_or([0.0, 0.0, 612.0, 792.0], |v| norm(&v));
        let crop = self.inherited(n, "CropBox").map(|o| self.nums(&o)).filter(|v| v.len() == 4).map(|v| norm(&v));
        let mut bbox = crop.map_or(media, |c| [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])]);
        if bbox[2] - bbox[0] < 1.0 || bbox[3] - bbox[1] < 1.0 {
            bbox = media;
        }
        let rotate = self.inherited(n, "Rotate").and_then(|o| o.int()).unwrap_or(0).rem_euclid(360) / 90 * 90;
        Geom::of(bbox, rotate)
    }

    /// The page's content streams, decoded and joined.
    pub fn page_content(&self, n: u32) -> Vec<u8> {
        let Some(d) = self.dict(n) else { return Vec::new() };
        let parts = match self.key(&d, "Contents") {
            Obj::Array(a) => a,
            o => vec![o],
        };
        let mut out = Vec::new();
        for p in parts {
            if let Obj::Stream(sd, raw) = self.resolve(&p) {
                out.extend_from_slice(&self.decode(&sd, &raw));
                out.push(b'\n');
            }
        }
        out
    }

    /// A stream's bytes after its filters (images keep their codec).
    pub fn decode(&self, d: &Dict, raw: &[u8]) -> Vec<u8> {
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
                "FlateDecode" | "Fl" | "LZWDecode" | "LZW" | "ASCIIHexDecode" | "AHx" | "ASCII85Decode" | "A85" | "RunLengthDecode" | "RL" => {
                    super::file::filter(f, &data, int("Predictor", 1), int("Colors", 1), int("BitsPerComponent", 8), int("Columns", 1), int("EarlyChange", 1) != 0)
                }
                "Crypt" => data,
                _ => return data,
            };
        }
        data
    }

    /// Replaces the page's content with `content` (one Flate stream).
    pub fn set_page_content(&mut self, page: u32, content: &[u8]) {
        let Some(mut d) = self.dict(page) else { return };
        let s = self.add(deflate(Dict::default(), content));
        d.set("Contents", Obj::Ref(s, 0));
        self.set(page, Obj::Dict(d));
    }

    /// The page's resources (inherited ones copied onto it when `own`).
    pub fn resources(&self, page: u32) -> Dict {
        match self.inherited(page, "Resources") {
            Some(Obj::Dict(d)) => d,
            _ => Dict::default(),
        }
    }

    /// Whether the file has signatures.
    pub fn is_signed(&self) -> bool {
        self.objs.values().any(|(_, o)| matches!(o, Obj::Dict(d) if d.get("Type").and_then(Obj::name) == Some("Sig") || d.get("ByteRange").is_some() && d.get("Contents").is_some()))
    }

    // --- saving ------------------------------------------------------------------------

    /// The file as it is now (not marked saved).
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.saves_incrementally() {
            if self.changed.is_empty() {
                return self.base.to_vec();
            }
            write::incremental(self).0
        } else {
            write::whole(self)
        }
    }

    /// The file as it is now, written whole (no earlier revisions).
    pub fn to_bytes_whole(&self) -> Vec<u8> {
        write::whole(self)
    }

    /// The file to save; what it wrote becomes the base for the next one.
    pub fn save(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let _ = self.save_to(|b| {
            bytes = b.to_vec();
            Ok::<(), ()>(())
        });
        bytes
    }

    /// Hands the file to save to `write`; only when it is written does it
    /// become the base for the next one (and the changes saved).
    pub fn save_to<E>(&mut self, write: impl FnOnce(&[u8]) -> Result<(), E>) -> Result<(), E> {
        let (out, xref) = if self.saves_incrementally() {
            if self.changed.is_empty() {
                (self.base.to_vec(), self.base_xref)
            } else {
                let (out, at) = write::incremental(self);
                (out, Some((at, self.base_xref.is_some_and(|x| x.1))))
            }
        } else {
            let out = write::whole(self);
            let xref = last_xref(&out);
            (out, xref)
        };
        write(&out)?;
        self.base_xref = xref;
        self.base = Arc::from(out);
        self.changed.clear();
        self.whole = false;
        self.saved = self.revision;
        Ok(())
    }

    /// Every object reachable from the trailer.
    fn reachable(&self) -> BTreeSet<u32> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<u32> = Vec::new();
        let push_refs = |o: &Obj, stack: &mut Vec<u32>| refs_in(o, stack);
        for (_, v) in &self.trailer.0 {
            push_refs(v, &mut stack);
        }
        while let Some(n) = stack.pop() {
            if !seen.insert(n) {
                continue;
            }
            if let Some(o) = self.get(n) {
                push_refs(o, &mut stack);
            }
        }
        seen
    }
}

/// Every reference inside `o`.
pub(crate) fn refs_in(o: &Obj, out: &mut Vec<u32>) {
    match o {
        Obj::Ref(n, _) => out.push(*n),
        Obj::Array(a) => a.iter().for_each(|x| refs_in(x, out)),
        Obj::Dict(d) | Obj::Stream(d, _) => d.0.iter().for_each(|(_, v)| refs_in(v, out)),
        _ => {}
    }
}

/// The last cross-reference section of a file: where, and whether it is a
/// stream.
fn last_xref(data: &[u8]) -> Option<(usize, bool)> {
    let tail = data.len().saturating_sub(4096);
    let sx = tail + memchr::memmem::rfind(&data[tail..], b"startxref")?;
    let mut lx = super::obj::Lexer::new(data, sx + 9);
    let Some(super::obj::Item::Obj(Obj::Int(at))) = lx.next(false) else { return None };
    let at = usize::try_from(at).ok()?;
    let rest = data.get(at..)?;
    if rest.starts_with(b"xref") {
        return Some((at, false));
    }
    let mut lx = super::obj::Lexer::new(data, at);
    match (lx.next(false), lx.next(false), lx.next(false)) {
        (Some(super::obj::Item::Obj(Obj::Int(_))), Some(super::obj::Item::Obj(Obj::Int(_))), Some(super::obj::Item::Kw(b"obj"))) => Some((at, true)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_redact;
#[cfg(test)]
mod tests_organize;
#[cfg(test)]
mod tests_props;
#[cfg(test)]
mod tests_stamp;
#[cfg(all(test, feature = "empresa"))]
mod tests_circuit;
#[cfg(test)]
mod tests_dossier;
