//! System fonts for the text of PDFs that don't embed their fonts, when
//! pages are drawn without pdfium: metric-compatible faces for the
//! standard 14 fonts, the family the PDF names if installed, or a sans,
//! serif or monospace face like it.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use apex_io::pdf::Style;

/// Faces of each family (lowercase): regular, bold, italic, bold italic.
type Families = BTreeMap<String, [Option<(PathBuf, u32)>; 4]>;

fn families() -> &'static Families {
    static FAMS: OnceLock<Families> = OnceLock::new();
    FAMS.get_or_init(|| {
        let mut paths = Vec::new();
        for d in crate::fonts::font_dirs() {
            crate::fonts::scan(&d, 4, &mut paths);
        }
        paths.sort();
        let mut out: Families = BTreeMap::new();
        for p in paths {
            for f in crate::fonts::read_faces(&p) {
                let slot = (f.weight >= 600) as usize + 2 * f.italic as usize;
                let e = out.entry(f.family.to_lowercase()).or_default();
                // The weight nearest to regular (400) or bold (700).
                if e[slot].is_none() || matches!(f.weight, 400 | 700) {
                    e[slot] = Some((f.path.clone(), f.index));
                }
            }
        }
        out
    })
}

fn bytes(path: &PathBuf) -> Option<Arc<[u8]>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<[u8]>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(b) = cache.lock().ok()?.get(path) {
        return Some(b.clone());
    }
    let b: Arc<[u8]> = Arc::from(std::fs::read(path).ok()?);
    if let Ok(mut c) = cache.lock() {
        c.insert(path.clone(), b.clone());
    }
    Some(b)
}

const SANS: &[&str] = &["liberation sans", "arimo", "arial", "nimbus sans", "helvetica", "dejavu sans", "noto sans"];
const SERIF: &[&str] = &["liberation serif", "tinos", "times new roman", "nimbus roman", "dejavu serif", "noto serif"];
const MONO: &[&str] = &["liberation mono", "cousine", "courier new", "nimbus mono ps", "dejavu sans mono", "noto sans mono"];
const CJK: &[&str] = &["noto sans cjk jp", "noto sans cjk sc", "noto serif cjk jp", "source han sans", "droid sans fallback", "wenquanyi zen hei", "ipagothic", "takao gothic"];
const CJK_NAMES: &[&str] = &["ming", "song", "hei", "gothic", "mincho", "kai", "batang", "gulim", "dotum", "kozmin", "kozgo", "heisei", "msung", "mhei", "hysmyeongjo", "hygothic", "simsun", "simhei", "fangsong", "ryumin", "gothicbbb", "sun-extb", "malgun", "meiryo", "yugoth", "pmingliu", "mingliu"];

const CJK_SERIF: &[&str] = &["ming", "song", "mincho", "ryumin", "kozmin", "batang", "simsun", "fangsong", "msung", "hysmyeongjo"];

/// The system font for a PDF font: its file's bytes and the face's index.
pub fn font(style: &Style, base: &str) -> Option<(Arc<[u8]>, u32)> {
    let fams = families();
    let name = base.split_once('+').filter(|(t, _)| t.len() == 6).map_or(base, |(_, r)| r).to_ascii_lowercase();
    let name = name.replace(['-', ',', '_'], " ");
    let mut want: Vec<String> = Vec::new();
    // The standard 14: URW's clones of them first (the shapes, as pdfium's
    // own fonts have them), then fonts with their widths.
    let std14: Option<&[&str]> = if name.starts_with("helvetica") {
        Some(&["nimbus sans", "liberation sans", "arimo", "arial", "dejavu sans"])
    } else if name.starts_with("arial") {
        Some(SANS)
    } else if name.starts_with("times") {
        Some(&["nimbus roman", "liberation serif", "tinos", "times new roman", "dejavu serif"])
    } else if name.starts_with("courier") {
        Some(&["nimbus mono ps", "liberation mono", "cousine", "courier new", "dejavu sans mono"])
    } else if name.starts_with("symbol") {
        Some(&["standard symbols ps", "symbol", "opensymbol"])
    } else if name.contains("dingbats") {
        Some(&["d050000l", "dingbats", "opensymbol"])
    } else {
        None
    };
    match std14 {
        Some(list) => {
            // The family itself first (Arial, Times New Roman): composite
            // fonts reach its glyphs by number.
            let fam = style.family.to_lowercase();
            if name.replace(' ', "").starts_with(&fam.replace(' ', "")) {
                want.push(fam);
            }
            want.extend(list.iter().map(|s| s.to_string()));
        }
        None => {
            want.push(style.family.to_lowercase());
            match style.family.to_lowercase().as_str() {
                "calibri" => want.push("carlito".into()),
                "cambria" => want.push("caladea".into()),
                _ => {}
            }
            if CJK_NAMES.iter().any(|c| name.contains(c)) || !base.is_ascii() {
                // Mincho, Ming and Song faces are serif.
                if CJK_SERIF.iter().any(|c| name.contains(c)) || style.serif {
                    want.extend(["noto serif cjk jp", "noto serif cjk sc"].map(String::from));
                }
                want.extend(CJK.iter().map(|s| s.to_string()));
            }
            let list = if style.mono {
                MONO
            } else if style.serif {
                SERIF
            } else {
                SANS
            };
            want.extend(list.iter().map(|s| s.to_string()));
        }
    }
    let slot = style.bold as usize + 2 * style.italic as usize;
    for w in &want {
        let Some(faces) = fams.get(w) else { continue };
        // The style asked for, else the closest one there is.
        let pick = faces[slot].as_ref().or(faces[slot & 1].as_ref()).or(faces[0].as_ref()).or(faces.iter().flatten().next());
        if let Some((p, i)) = pick
            && let Some(b) = bytes(p)
        {
            return Some((b, *i));
        }
    }
    None
}
