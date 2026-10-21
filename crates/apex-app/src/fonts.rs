//! Fonts. At startup we load one regular and one bold face for sans, serif
//! and monospace text (egui's bundled fonts stay as glyph fallback). A
//! background thread then reads the names of every installed font so that
//! documents can use any family; those are loaded on first use. Chinese,
//! Japanese and Korean fonts, and those of other scripts, are loaded when
//! needed (see `cjk`).

mod cjk;
pub mod embedded;

pub use cjk::note_text;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Face {
    Sans,
    Serif,
    Mono,
}

const FACES: [(Face, bool, &str, &[&str]); 6] = [
    (Face::Sans, false, "sans", &["notosans-regular.ttf", "segoeui.ttf", "dejavusans.ttf", "liberationsans-regular.ttf", "arial.ttf", "ubuntu-r.ttf", "helvetica.ttf"]),
    (Face::Sans, true, "sans-b", &["notosans-bold.ttf", "segoeuib.ttf", "dejavusans-bold.ttf", "liberationsans-bold.ttf", "arialbd.ttf", "arial bold.ttf", "ubuntu-b.ttf"]),
    (Face::Serif, false, "serif", &["notoserif-regular.ttf", "liberationserif-regular.ttf", "times.ttf", "times new roman.ttf", "dejavuserif.ttf", "georgia.ttf"]),
    (Face::Serif, true, "serif-b", &["notoserif-bold.ttf", "liberationserif-bold.ttf", "timesbd.ttf", "times new roman bold.ttf", "dejavuserif-bold.ttf", "georgiab.ttf"]),
    (Face::Mono, false, "mono", &["dejavusansmono.ttf", "liberationmono-regular.ttf", "notosansmono-regular.ttf", "consola.ttf", "cascadiamono.ttf", "courier new.ttf", "ubuntumono-r.ttf"]),
    (Face::Mono, true, "mono-b", &["dejavusansmono-bold.ttf", "liberationmono-bold.ttf", "notosansmono-bold.ttf", "consolab.ttf", "courier new bold.ttf", "ubuntumono-b.ttf"]),
];

/// Metric-compatible replacements for common proprietary fonts.
const SUBSTITUTES: &[(&str, &[&str])] = &[
    ("calibri light", &["carlito"]),
    ("calibri", &["carlito"]),
    ("cambria", &["caladea"]),
    ("arial", &["liberation sans", "arimo"]),
    ("helvetica", &["liberation sans", "arimo", "nimbus sans"]),
    ("times new roman", &["liberation serif", "tinos", "nimbus roman"]),
    ("times", &["liberation serif", "tinos", "nimbus roman"]),
    ("courier new", &["liberation mono", "cousine", "nimbus mono ps"]),
    ("courier", &["liberation mono", "cousine", "nimbus mono ps"]),
    ("arial narrow", &["liberation sans narrow"]),
    ("georgia", &["gelasio"]),
    ("segoe ui", &["noto sans", "dejavu sans"]),
    ("verdana", &["dejavu sans"]),
];

/// Replacements the settings or the organization add, tried before the
/// built-in ones: (missing family, replacement), lowercase.
static OWN_SUBSTITUTES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Sets the replacements from "Name=Other; Name2=Other2".
pub fn set_substitutes(spec: &str) {
    *OWN_SUBSTITUTES.lock().unwrap_or_else(|e| e.into_inner()) = parse_substitutes(spec);
}

fn parse_substitutes(spec: &str) -> Vec<(String, String)> {
    spec.split([';', '\n'])
        .filter_map(|p| p.split_once('='))
        .map(|(a, b)| (a.trim().to_lowercase(), b.trim().to_lowercase()))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .collect()
}

pub(crate) fn font_dirs() -> Vec<PathBuf> {
    // The browser's own, which came with the page.
    #[cfg(target_arch = "wasm32")]
    return vec![PathBuf::from(crate::web::FONTS)];
    #[cfg_attr(target_arch = "wasm32", allow(unreachable_code))]
    let mut v = Vec::new();
    if cfg!(windows) {
        let root = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        v.push(root.join("Fonts"));
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        v.push("/System/Library/Fonts/Supplemental".into());
        v.push("/Library/Fonts".into());
        v.push("/System/Library/Fonts".into());
    } else if cfg!(target_os = "android") {
        // The system's, and those a phone maker adds.
        v.push("/system/fonts".into());
        v.push("/product/fonts".into());
        v.push("/system_ext/fonts".into());
    } else {
        v.push("/usr/share/fonts".into());
        v.push("/usr/local/share/fonts".into());
    }
    if let Some(h) = std::env::var_os("HOME") {
        let h = PathBuf::from(h);
        v.push(h.join(".local/share/fonts"));
        v.push(h.join(".fonts"));
        v.push(h.join("Library/Fonts"));
    }
    v
}

pub(crate) fn scan(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = apex_fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if depth > 0 {
                scan(&p, depth - 1, out);
            }
        } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc") || lower.ends_with(".otc") {
                out.push(p);
            }
        }
    }
}

// --- font file names ---------------------------------------------------------------

/// One face of an installed font file.
#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub family: String,
    pub path: PathBuf,
    pub index: u32,
    pub weight: u16,
    pub italic: bool,
    pub size: u64,
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}

fn read_at(f: &mut apex_fs::File, at: u64, len: usize) -> Option<Vec<u8>> {
    if len > 4 << 20 {
        return None;
    }
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut v = vec![0; len];
    f.read_exact(&mut v).ok()?;
    Some(v)
}

/// Reads the family name, weight and slant of the faces in a font file,
/// touching only the table directory and the `name` and `OS/2` tables.
pub fn read_faces(path: &Path) -> Vec<FaceInfo> {
    let mut out = Vec::new();
    let Ok(mut f) = apex_fs::File::open(path) else { return out };
    let Some(head) = read_at(&mut f, 0, 12) else { return out };
    let offsets: Vec<u32> = if &head[..4] == b"ttcf" {
        let n = be32(&head, 8).unwrap_or(0).min(64);
        match read_at(&mut f, 12, n as usize * 4) {
            Some(b) => (0..n as usize).filter_map(|i| be32(&b, i * 4)).collect(),
            None => return out,
        }
    } else {
        vec![0]
    };
    let size = f.metadata().map_or(0, |m| m.len());
    for (index, off) in offsets.into_iter().enumerate() {
        if let Some(face) = read_face(&mut f, off as u64) {
            out.push(FaceInfo { family: face.0, path: path.to_path_buf(), index: index as u32, weight: face.1, italic: face.2, size });
        }
    }
    out
}

fn read_face(f: &mut apex_fs::File, off: u64) -> Option<(String, u16, bool)> {
    let dir = read_at(f, off, 12)?;
    let n = be16(&dir, 4)? as usize;
    let recs = read_at(f, off + 12, n.min(64) * 16)?;
    let mut name = None;
    let mut os2 = None;
    for i in 0..n.min(64) {
        let r = &recs[i * 16..];
        let (o, l) = (be32(r, 8)? as u64, be32(r, 12)? as usize);
        match &r[..4] {
            b"name" => name = Some((o, l)),
            b"OS/2" => os2 = Some((o, l)),
            _ => {}
        }
    }
    let (no, nl) = name?;
    let nb = read_at(f, no, nl)?;
    let count = be16(&nb, 2)? as usize;
    let strings = be16(&nb, 4)? as usize;
    // Best string per name id: Windows English, then any Windows, then Mac.
    let mut best: HashMap<u16, (u8, String)> = HashMap::new();
    for i in 0..count {
        let r = 6 + i * 12;
        let (plat, enc, lang, id) = (be16(&nb, r)?, be16(&nb, r + 2)?, be16(&nb, r + 4)?, be16(&nb, r + 6)?);
        if !matches!(id, 1 | 2 | 16 | 17) {
            continue;
        }
        let (len, at) = (be16(&nb, r + 8)? as usize, be16(&nb, r + 10)? as usize);
        let Some(raw) = nb.get(strings + at..strings + at + len) else { continue };
        let (rank, s) = match (plat, enc) {
            (3, 0 | 1 | 10) | (0, _) => {
                let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                (if plat == 3 && lang == 0x409 { 3 } else { 2 }, String::from_utf16_lossy(&units))
            }
            (1, 0) => (1, raw.iter().map(|&b| b as char).collect()),
            _ => continue,
        };
        if best.get(&id).is_none_or(|(r, _)| *r < rank) {
            best.insert(id, (rank, s));
        }
    }
    let family = best.get(&16).or_else(|| best.get(&1))?.1.trim().to_string();
    if family.is_empty() || family.starts_with('.') {
        return None;
    }
    let sub = best.get(&17).or_else(|| best.get(&2)).map(|x| x.1.to_ascii_lowercase()).unwrap_or_default();
    let (mut weight, mut italic) = (if sub.contains("bold") { 700 } else { 400 }, sub.contains("italic") || sub.contains("oblique"));
    if let Some((o, l)) = os2 {
        if let Some(b) = read_at(f, o, l.min(64)) {
            if let (Some(w), Some(sel)) = (be16(&b, 4), be16(&b, 62)) {
                weight = w;
                italic = sel & 1 != 0 || italic;
            }
        }
    }
    Some((family, weight, italic))
}

// --- the installed families ----------------------------------------------------------

#[derive(Default)]
struct Family {
    name: String,
    /// Regular, bold, italic, bold italic.
    faces: [Option<(PathBuf, u32, u16, u64)>; 4],
}

fn slot(bold: bool, italic: bool) -> usize {
    bold as usize + 2 * italic as usize
}

#[derive(Default)]
struct Db {
    /// Lowercase family name -> faces.
    families: BTreeMap<String, Family>,
    names: Arc<Vec<String>>,
    ready: bool,
    defs: Option<FontDefinitions>,
    /// Families registered with egui and usable from this pass on.
    usable: HashSet<String>,
    /// Registered but only usable once the next pass starts.
    applied: Vec<(String, u64)>,
    requested: Vec<String>,
    /// Loaded file data, shared between faces of one collection.
    data: HashMap<(PathBuf, u32), String>,
    /// Every font file found, for the CJK fallback fonts.
    paths: Vec<PathBuf>,
    /// Pass in which CJK fonts were registered.
    cjk_pass: Option<u64>,
    /// Families embedded in open documents, and their data by (pseudo) path.
    embedded: BTreeMap<String, Family>,
    mem: HashMap<PathBuf, Arc<[u8]>>,
    /// Families two documents embed differently: neither is used.
    embedded_clash: HashSet<String>,
    /// How many open documents hold each embedded face.
    embedded_users: HashMap<PathBuf, usize>,
}

fn db() -> &'static Mutex<Db> {
    static DB: OnceLock<Mutex<Db>> = OnceLock::new();
    DB.get_or_init(Default::default)
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Glyph atlas fill ratio seen by the last `apply`, as bits.
static ATLAS_FILL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Changes whenever newly loaded fonts can alter text layout, and when the
/// glyph atlas is rebuilt: cached galleys must be laid out again.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Families installed on the system, sorted; empty while still scanning.
pub fn families() -> Arc<Vec<String>> {
    db().lock().map(|d| d.names.clone()).unwrap_or_default()
}

fn build_db(files: Vec<PathBuf>) -> BTreeMap<String, Family> {
    let mut fams: BTreeMap<String, Family> = BTreeMap::new();
    for p in files {
        for face in read_faces(&p) {
            let key = face.family.to_lowercase();
            let fam = fams.entry(key).or_insert_with(|| Family { name: face.family.clone(), ..Default::default() });
            let bold = face.weight >= 600;
            let target = if bold { 700 } else { 400 };
            let s = slot(bold, face.italic);
            let better = match &fam.faces[s] {
                None => true,
                Some((_, _, w, _)) => (face.weight as i32 - target).abs() < (*w as i32 - target).abs(),
            };
            if better {
                fam.faces[s] = Some((face.path.clone(), face.index, face.weight, face.size));
            }
        }
    }
    fams.retain(|_, f| f.faces.iter().any(Option::is_some));
    fams
}

/// Loads the base faces and starts the background scan of every family.
pub fn install(ctx: &egui::Context) {
    let mut paths = Vec::new();
    for d in font_dirs() {
        scan(&d, 4, &mut paths);
    }
    let by_name: HashMap<String, &PathBuf> = paths.iter().filter_map(|p| Some((p.file_name()?.to_str()?.to_ascii_lowercase(), p))).collect();
    let mut defs = FontDefinitions::default();
    let fallback_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut names: HashMap<(Face, bool), String> = HashMap::new();
    for (face, bold, name, candidates) in FACES {
        let found = candidates.iter().find_map(|c| by_name.get(*c)).and_then(|p| apex_fs::read(p).ok());
        if let Some(bytes) = found {
            defs.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
            names.insert((face, bold), name.to_string());
        }
    }
    for (face, bold, name, _) in FACES {
        let mut chain = Vec::new();
        if let Some(n) = names.get(&(face, bold)) {
            chain.push(n.clone());
        }
        if bold {
            // Missing bold face: fall back to the regular one.
            if let Some(n) = names.get(&(face, false)) {
                chain.push(n.clone());
            }
        }
        if face == Face::Serif && chain.is_empty() {
            if let Some(n) = names.get(&(Face::Sans, bold)).or_else(|| names.get(&(Face::Sans, false))) {
                chain.push(n.clone());
            }
        }
        chain.extend(if face == Face::Mono { fallback_mono.iter().cloned() } else { fallback_prop.iter().cloned() });
        defs.families.insert(FontFamily::Name(name.into()), chain);
    }
    // UI text uses the system sans face.
    if let Some(n) = names.get(&(Face::Sans, false)) {
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, n.clone());
    }
    if let Some(n) = names.get(&(Face::Mono, false)) {
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, n.clone());
    }
    cjk::update(&mut defs, &paths);
    if let Ok(mut d) = db().lock() {
        // Families already loaded (for another context: tests) stay, as
        // `font_for` hands them out.
        if let Some(old) = d.defs.take() {
            for (k, v) in old.font_data {
                defs.font_data.entry(k).or_insert(v);
            }
            for (k, v) in old.families {
                if matches!(&k, FontFamily::Name(n) if n.starts_with("f:")) {
                    defs.families.entry(k).or_insert(v);
                }
            }
        }
        d.defs = Some(defs.clone());
        d.paths = paths.clone();
    }
    ctx.set_fonts(defs);
    let ctx = ctx.clone();
    apex_fs::thread::Builder::new()
        .name("font-scan".into())
        .spawn(move || {
            let fams = build_db(paths);
            let names: Vec<String> = fams.values().map(|f| f.name.clone()).collect();
            if let Ok(mut d) = db().lock() {
                d.families = fams;
                embedded::merge(&mut d);
                d.names = Arc::new(names);
                d.ready = true;
            }
            GENERATION.fetch_add(1, Ordering::Relaxed);
            ctx.request_repaint();
        })
        .ok();
}

/// Resolves a document font name to an installed family (lowercase key),
/// trying metric-compatible substitutes.
fn resolve(d: &Db, name: &str) -> Option<String> {
    let key = name.trim().to_lowercase();
    if d.families.contains_key(&key) {
        return Some(key);
    }
    let own = OWN_SUBSTITUTES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());
    if let Some(v) = own.filter(|v| d.families.contains_key(v)) {
        return Some(v);
    }
    let subs = SUBSTITUTES.iter().find(|(k, _)| *k == key)?;
    subs.1.iter().find(|s| d.families.contains_key(**s)).map(|s| s.to_string())
}

fn egui_name(key: &str, s: usize) -> String {
    format!("f:{key}:{s}")
}

/// Font for text in the named family. Returns the font and whether the
/// italics must be synthesized (no real italic face). Unknown or not yet
/// loaded families use the closest built-in face.
pub fn font_for(name: Option<&str>, bold: bool, italic: bool, size: f32) -> (FontId, bool) {
    let fallback = || (font(face_for(name), bold, size), italic);
    let Some(name) = name.filter(|n| !n.trim().is_empty()) else { return (font(Face::Sans, bold, size), italic) };
    let Ok(mut d) = db().lock() else { return fallback() };
    let Some(key) = resolve(&d, name) else { return fallback() };
    let fam = &d.families[&key];
    // The family chain falls back to other faces; italics are synthesized
    // when the family has no italic face.
    let synth = italic && fam.faces[slot(bold, true)].is_none() && fam.faces[slot(false, true)].is_none();
    let en = egui_name(&key, slot(bold, italic));
    if d.usable.contains(&en) {
        return (FontId::new(size, FontFamily::Name(en.into())), synth);
    }
    if !d.requested.contains(&key) && !d.applied.iter().any(|(k, _)| *k == key) {
        d.requested.push(key);
    }
    fallback()
}

/// Font to show a family name in its own face (font menus). Big files
/// (CJK collections) are not loaded just for a preview.
pub fn preview_font(name: &str, size: f32) -> FontId {
    let small = db().lock().is_ok_and(|d| {
        d.families.get(&name.to_lowercase()).is_some_and(|f| f.faces[0].as_ref().or(f.faces.iter().flatten().next()).is_some_and(|x| x.3 < 3 << 20))
    });
    if small { font_for(Some(name), false, false, size).0 } else { FontId::proportional(size) }
}

/// Registers the families requested since the last call. Call once per
/// frame before any text is laid out.
pub fn apply(ctx: &egui::Context) {
    // egui rebuilds the glyph atlas when it fills up; galleys kept from
    // earlier frames then point at glyphs that are gone.
    let fill = ctx.fonts(|f| f.font_atlas_fill_ratio());
    if fill < f32::from_bits(ATLAS_FILL.swap(fill.to_bits(), Ordering::Relaxed)) {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    // It rebuilds it too when the text options change (Sharper text, or
    // the theme's color transfer), with an atlas that may already be as
    // full as the old one was.
    static OPTIONS: Mutex<Option<egui::epaint::text::TextOptions>> = Mutex::new(None);
    let options = ctx.fonts(|f| *f.options());
    if let Ok(mut last) = OPTIONS.lock()
        && last.replace(options).is_some_and(|o| o != options)
    {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    // Text typed or pasted may need a CJK font.
    ctx.input(|i| {
        for e in &i.events {
            match e {
                egui::Event::Text(t) | egui::Event::Paste(t) | egui::Event::Ime(egui::ImeEvent::Commit(t)) => note_text(t),
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => note_text(text),
                _ => {}
            }
        }
    });
    let pass = ctx.cumulative_pass_nr();
    let Ok(mut d) = db().lock() else { return };
    if let Some(p) = d.cjk_pass
        && p < pass
    {
        d.cjk_pass = None;
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    if cjk::pending()
        && let Some(mut defs) = d.defs.take()
    {
        if cjk::update(&mut defs, &d.paths) {
            ctx.set_fonts(defs.clone());
            d.cjk_pass = Some(pass);
            ctx.request_repaint();
        }
        d.defs = Some(defs);
    }
    let before = d.usable.len();
    let applied = std::mem::take(&mut d.applied);
    for (k, p) in applied {
        if p < pass {
            for s in 0..4 {
                d.usable.insert(egui_name(&k, s));
            }
        } else {
            d.applied.push((k, p));
        }
    }
    if d.usable.len() != before {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    if d.requested.is_empty() {
        return;
    }
    let requested = std::mem::take(&mut d.requested);
    let Some(mut defs) = d.defs.take() else { return };
    for key in requested {
        // A document's embedded family may have gone with it since.
        let Some(faces) = d.families.get(&key).map(|f| f.faces.clone()) else { continue };
        let mut loaded: [Option<String>; 4] = Default::default();
        for (s, f) in faces.iter().enumerate() {
            let Some((path, index, _, _)) = f else { continue };
            let id = (path.clone(), *index);
            if let Some(n) = d.data.get(&id) {
                loaded[s] = Some(n.clone());
                continue;
            }
            let bytes = match d.mem.get(path) {
                Some(b) => b.to_vec(),
                None => match apex_fs::read(path) {
                    Ok(b) => b,
                    Err(_) => continue,
                },
            };
            let n = format!("file:{}#{}", path.display(), index);
            let mut fd = FontData::from_owned(bytes);
            fd.index = *index;
            defs.font_data.insert(n.clone(), Arc::new(fd));
            d.data.insert(id, n.clone());
            loaded[s] = Some(n);
        }
        let prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        for s in 0..4 {
            let bold = s & 1 == 1;
            let italic = s & 2 == 2;
            let mut chain = Vec::new();
            let order = [slot(bold, italic), slot(bold, false), 0, slot(false, italic)];
            for o in order {
                if let Some(n) = &loaded[o] {
                    if !chain.contains(n) {
                        chain.push(n.clone());
                    }
                }
                // A missing bold face: the built-in bold beats a regular one.
                if bold && o == slot(bold, false) && loaded[o].is_none() {
                    if let Some(b) = defs.families.get(&FontFamily::Name("sans-b".into())).and_then(|c| c.first()).cloned() {
                        chain.push(b);
                    }
                }
            }
            for f in &prop {
                // (Fallback fonts of other scripts in their bold face.)
                if let Some(b) = cjk::bold_of(f).filter(|b| bold && !chain.contains(b)) {
                    chain.push(b);
                }
                if !chain.contains(f) {
                    chain.push(f.clone());
                }
            }
            defs.families.insert(FontFamily::Name(egui_name(&key, s).into()), chain);
        }
        d.applied.push((key, pass));
    }
    ctx.set_fonts(defs.clone());
    d.defs = Some(defs);
    ctx.request_repaint();
}

/// The font files behind an egui family, in fallback order.
pub fn chain(family: &FontFamily) -> Vec<(String, Arc<FontData>)> {
    let Ok(d) = db().lock() else { return Vec::new() };
    let Some(defs) = &d.defs else { return Vec::new() };
    let Some(names) = defs.families.get(family) else { return Vec::new() };
    names.iter().filter_map(|n| Some((n.clone(), defs.font_data.get(n)?.clone()))).collect()
}

/// Single line spacing (as `line_factor` measures it) of common Windows
/// and Office fonts, for text in them where they aren't installed: the
/// stand-in's own (Noto's 1.36) would make every line too tall.
const KNOWN_LINES: &[(&str, f32)] = &[
    ("arial", 1.1499),
    ("arial narrow", 1.1499),
    ("arial unicode ms", 1.3403),
    ("book antiqua", 1.1704),
    ("bookman old style", 1.1704),
    ("calibri", 1.2207),
    ("calibri light", 1.2207),
    ("cambria", 1.1724),
    ("century gothic", 1.2261),
    ("comic sans ms", 1.3936),
    ("consolas", 1.1709),
    ("courier new", 1.1328),
    ("garamond", 1.1235),
    ("georgia", 1.1362),
    ("segoe ui", 1.3301),
    ("tahoma", 1.2070),
    ("times new roman", 1.1499),
    ("trebuchet ms", 1.1611),
    ("verdana", 1.2153),
    // East Asian fonts of Windows (their ascent and descent make one
    // em; Word adds 30% to the lines of fonts for these scripts).
    ("ｍｓ 明朝", 1.3),
    ("ｍｓ ゴシック", 1.3),
    ("ｍｓ ｐ明朝", 1.3),
    ("ｍｓ ｐゴシック", 1.3),
    ("ms mincho", 1.3),
    ("ms gothic", 1.3),
    ("ms pmincho", 1.3),
    ("ms pgothic", 1.3),
    ("ms ui gothic", 1.3),
    ("宋体", 1.3),
    ("simsun", 1.3),
    ("新宋体", 1.3),
    ("nsimsun", 1.3),
    ("黑体", 1.3),
    ("simhei", 1.3),
    ("仿宋", 1.3),
    ("fangsong", 1.3),
    ("仿宋_gb2312", 1.3),
    ("楷体", 1.3),
    ("kaiti", 1.3),
    ("楷体_gb2312", 1.3),
    ("新細明體", 1.3),
    ("pmingliu", 1.3),
    ("細明體", 1.3),
    ("mingliu", 1.3),
    ("標楷體", 1.3),
    ("dfkai-sb", 1.3),
    ("メイリオ", 1.5),
    ("meiryo", 1.5),
];

/// The known line spacing of a font (see `KNOWN_LINES`).
pub fn known_line(name: &str) -> Option<f32> {
    let key = name.trim().to_lowercase();
    KNOWN_LINES.iter().find(|(k, _)| *k == key).map(|k| k.1)
}

/// Whether `family` is the installed font named `name` (not a stand-in).
pub fn is_font(family: &FontFamily, name: &str) -> bool {
    matches!(family, FontFamily::Name(f) if f.strip_prefix("f:").and_then(|r| r.rsplit_once(':')).is_some_and(|(k, _)| k == name.trim().to_lowercase()))
}

/// `line_factor` of `family` laying out text in the document font `name`:
/// the known one of that font when `family` stands in for it, else (a
/// built-in face standing in for a font we know nothing about) a usual
/// one: Word's fonts are about 1.15 to 1.25 high, Noto's 1.36.
pub fn line_factor_for(name: Option<&str>, family: &FontFamily) -> f32 {
    if let Some(n) = name.filter(|n| !is_font(family, n)) {
        if let Some(l) = known_line(n) {
            return l;
        }
        if matches!(family, FontFamily::Name(f) if !f.starts_with("f:")) {
            return 1.2;
        }
    }
    line_factor(family)
}

/// Single line spacing of a family as a multiple of the font size: the
/// first face's ascent, descent and line gap, as Word and LibreOffice use.
pub fn line_factor(family: &FontFamily) -> f32 {
    static CACHE: OnceLock<Mutex<HashMap<FontFamily, f32>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(f) = cache.lock().ok().and_then(|c| c.get(family).copied()) {
        return f;
    }
    let Some((_, data)) = chain(family).into_iter().next() else { return 1.2 };
    let factor = line_factor_of(&data.font, data.index).unwrap_or(1.2);
    if let Ok(mut c) = cache.lock() {
        c.insert(family.clone(), factor);
    }
    factor
}

fn line_factor_of(font: &[u8], index: u32) -> Option<f32> {
    use skrifa::MetadataProvider;
    use skrifa::raw::TableProvider;
    let f = skrifa::FontRef::from_index(font, index).ok()?;
    let m = f.metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let mut factor = (m.ascent - m.descent + m.leading) / m.units_per_em.max(1) as f32;
    // Fonts for Chinese, Japanese or Korean (code pages JIS, GB 2312,
    // Wansung, Big5, Johab) get extra leading, as LibreOffice gives them
    // for compatibility with Windows: Noto Serif CJK at 10.5 pt makes
    // 19.15 pt lines there, not 15.1.
    if f.os2().is_ok_and(|o| o.ul_code_page_range_1().is_some_and(|r| r & CJK_CODE_PAGES != 0)) {
        factor *= CJK_LEADING;
    }
    (0.8..3.0).contains(&factor).then_some(factor)
}

/// OS/2 code page bits of CJK fonts (17 to 21).
const CJK_CODE_PAGES: u32 = 0x1F << 17;

/// Single line height of CJK fonts over their ascent plus descent
/// (measured in LibreOffice: 1.269 at every size and for any metrics).
pub const CJK_LEADING: f32 = 1.269;

pub fn family(face: Face, bold: bool) -> FontFamily {
    let name = match (face, bold) {
        (Face::Sans, false) => "sans",
        (Face::Sans, true) => "sans-b",
        (Face::Serif, false) => "serif",
        (Face::Serif, true) => "serif-b",
        (Face::Mono, false) => "mono",
        (Face::Mono, true) => "mono-b",
    };
    FontFamily::Name(name.into())
}

pub fn font(face: Face, bold: bool, size: f32) -> FontId {
    FontId::new(size, family(face, bold))
}

/// Maps a document font name to one of our faces.
pub fn face_for(name: Option<&str>) -> Face {
    let Some(n) = name else { return Face::Serif };
    let n = n.to_ascii_lowercase();
    if ["mono", "courier", "consolas", "menlo", "code"].iter().any(|m| n.contains(m)) {
        Face::Mono
    } else if ["sans", "arial", "helvetica", "calibri", "segoe", "verdana", "tahoma", "ubuntu", "roboto", "inter", "carlito", "cantarell", "aptos"].iter().any(|m| n.contains(m)) {
        Face::Sans
    } else {
        Face::Serif
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_substitutes() {
        let s = parse_substitutes("Corp Sans = Liberation Sans; bad; =x;Old=New\n");
        assert_eq!(s, [("corp sans".to_string(), "liberation sans".to_string()), ("old".into(), "new".into())]);
    }

    #[test]
    fn reads_system_font_names() {
        let mut paths = Vec::new();
        for d in font_dirs() {
            scan(&d, 4, &mut paths);
        }
        let Some(p) = paths.iter().find(|p| p.to_string_lossy().to_lowercase().contains("dejavusans-bold.ttf")) else { return };
        let faces = read_faces(p);
        assert_eq!(faces[0].family, "DejaVu Sans");
        assert!(faces[0].weight >= 600 && !faces[0].italic);
    }
}
