//! Fallback fonts for Chinese, Japanese and Korean text. A system CJK font
//! weighs 15-20 MB, so one is only loaded when the interface language is
//! CJK or CJK text shows up. One Han character is drawn differently in
//! China, Taiwan, Japan and Korea, so the face follows the interface
//! language, then the system's, then the scripts in the text. Arabic,
//! Hebrew, Thai and Devanagari text gets a fallback font the same way.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{FontData, FontDefinitions};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Region {
    Sc,
    Tc,
    Jp,
    Kr,
}

const REGIONS: [Region; 4] = [Region::Sc, Region::Tc, Region::Jp, Region::Kr];

// Scripts, as bits.
const HAN: u8 = 1;
const KANA: u8 = 2;
const HANGUL: u8 = 4;
const ARABIC: u8 = 8;
const HEBREW: u8 = 16;
const THAI: u8 = 32;
const DEVANAGARI: u8 = 64;
const CJK: u8 = HAN | KANA | HANGUL;

/// Font files with a script other than CJK ones, in order: Windows,
/// macOS, then Linux ones.
fn script_files(bit: u8) -> &'static [&'static str] {
    match bit {
        ARABIC => &["arial.ttf", "tahoma.ttf", "segoeui.ttf", "times.ttf", "geezapro.ttc", "arial unicode.ttf", "notonaskharabic-regular.ttf", "notosansarabic-regular.ttf", "notosansarabicui-regular.ttf", "dejavusans.ttf", "freesans.ttf"],
        HEBREW => &["arial.ttf", "tahoma.ttf", "segoeui.ttf", "times.ttf", "arialhb.ttc", "arial unicode.ttf", "notosanshebrew-regular.ttf", "notoserifhebrew-regular.ttf", "dejavusans.ttf", "freesans.ttf"],
        THAI => &["leelawui.ttf", "tahoma.ttf", "thonburi.ttc", "arial unicode.ttf", "notosansthai-regular.ttf", "notosansthaiui-regular.ttf", "garuda.ttf", "loma.ttf", "freeserif.ttf"],
        _ => &["nirmala.ttf", "mangal.ttf", "kohinoordevanagari.ttc", "devanagari sangam mn.ttc", "arial unicode.ttf", "notosansdevanagari-regular.ttf", "notosansdevanagariui-regular.ttf", "lohit-devanagari.ttf", "freeserif.ttf"],
    }
}

impl Region {
    pub fn of_lang(code: &str) -> Option<Region> {
        Some(match apex_i18n::cjk_region(code)? {
            "sc" => Region::Sc,
            "tc" => Region::Tc,
            "jp" => Region::Jp,
            _ => Region::Kr,
        })
    }

    /// Scripts the interface in this region's language writes.
    fn scripts(self) -> u8 {
        match self {
            Region::Sc | Region::Tc => HAN,
            Region::Jp => HAN | KANA,
            Region::Kr => HANGUL,
        }
    }

    /// Font files to try, in order: Windows, macOS, then Linux ones.
    fn files(self) -> &'static [&'static str] {
        const PAN: [&str; 3] = ["notosanscjk-regular.ttc", "sourcehansans-regular.ttc", "notosanscjk-vf.ttc"];
        match self {
            Region::Sc => &["msyh.ttc", "deng.ttf", "simsun.ttc", "simhei.ttf", "pingfang.ttc", "hiragino sans gb.ttc", "hiragino sans gb w3.ttc", "stheiti light.ttc", "stheiti medium.ttc",
                PAN[0], "notosanscjksc-regular.otf", "notosanssc-regular.otf", "notosanssc-regular.ttf", "sourcehansanssc-regular.otf", PAN[1], "wqy-microhei.ttc", "wqy-zenhei.ttc", "droidsansfallbackfull.ttf", "droidsansfallback.ttf", PAN[2]],
            Region::Tc => &["msjh.ttc", "mingliu.ttc", "pingfang.ttc", "stheiti light.ttc", "stheiti medium.ttc",
                PAN[0], "notosanscjktc-regular.otf", "notosanstc-regular.otf", "notosanstc-regular.ttf", "sourcehansanstc-regular.otf", PAN[1], "uming.ttc", "wqy-microhei.ttc", "wqy-zenhei.ttc", PAN[2]],
            Region::Jp => &["yugothr.ttc", "yugothm.ttc", "meiryo.ttc", "msgothic.ttc", "ヒラギノ角ゴシック w3.ttc", "ヒラギノ角ゴ pron w3.otf", "hiragino sans w3.ttc", "osaka.ttf",
                PAN[0], "notosanscjkjp-regular.otf", "notosansjp-regular.otf", "notosansjp-regular.ttf", "sourcehansansjp-regular.otf", PAN[1], "ipaexg.ttf", "ipagp.ttf", "ipag.ttf", "vl-pgothic-regular.ttf", "vl-gothic-regular.ttf", "takaopgothic.ttf", PAN[2]],
            Region::Kr => &["malgun.ttf", "gulim.ttc", "applesdgothicneo.ttc", "applegothic.ttf",
                PAN[0], "notosanscjkkr-regular.otf", "notosanskr-regular.otf", "notosanskr-regular.ttf", "sourcehansanskr-regular.otf", PAN[1], "nanumgothic.ttf", "nanumbarungothic.ttf", "undotum.ttf", PAN[2]],
        }
    }

    /// Faces to pick in font collections, as lowercase family names.
    fn faces(self) -> &'static [&'static str] {
        match self {
            Region::Sc => &["noto sans cjk sc", "source han sans sc", "microsoft yahei ui", "pingfang sc", "heiti sc", "hiragino sans gb"],
            Region::Tc => &["noto sans cjk tc", "source han sans tc", "microsoft jhenghei ui", "pingfang tc", "heiti tc", "pmingliu"],
            Region::Jp => &["noto sans cjk jp", "source han sans", "source han sans jp", "yu gothic ui", "meiryo ui", "ms ui gothic", "hiragino sans"],
            Region::Kr => &["noto sans cjk kr", "source han sans k", "source han sans kr", "malgun gothic", "apple sd gothic neo", "gulim"],
        }
    }
}

fn script(c: char) -> u8 {
    match c as u32 {
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => KANA,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => HANGUL,
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => HEBREW,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => ARABIC,
        0x0900..=0x097F | 0xA8E0..=0xA8FF => DEVANAGARI,
        0x0E00..=0x0E7F => THAI,
        0x2E80..=0x2FDF | 0x3000..=0x303F | 0x3190..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF65 | 0xFFE0..=0xFFEF | 0x20000..=0x3FFFF => HAN,
        _ => 0,
    }
}

/// Scripts seen in text so far.
static SEEN: AtomicU8 = AtomicU8::new(0);

/// Notes the scripts of text about to be shown, so that a CJK font is
/// loaded for it if needed. Cheap: call it with any text.
pub fn note_text(s: &str) {
    if s.is_ascii() {
        return;
    }
    let bits = s.chars().fold(0, |b, c| b | script(c));
    if bits & !SEEN.load(Ordering::Relaxed) != 0 {
        SEEN.fetch_or(bits, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct State {
    /// Region of the loaded fonts.
    region: Option<Region>,
    /// Scripts the loaded fonts cover, and those no installed font covers.
    covered: u8,
    failed: u8,
    /// Names of the loaded fonts in the font definitions.
    names: Vec<String>,
    /// Bold faces of loaded fonts (other scripts): (regular, bold) names.
    bold: Vec<(String, String)>,
}

fn state() -> &'static Mutex<State> {
    static S: Mutex<State> = Mutex::new(State { region: None, covered: 0, failed: 0, names: Vec::new(), bold: Vec::new() });
    &S
}

fn wanted() -> (Option<Region>, u8) {
    (Region::of_lang(apex_i18n::current()), SEEN.load(Ordering::Relaxed))
}

/// Whether `update` has something to do: cheap, for every frame.
pub(super) fn pending() -> bool {
    let (ui, seen) = wanted();
    let Ok(st) = state().lock() else { return false };
    (ui.is_some() && ui != st.region) || (seen | ui.map_or(0, Region::scripts)) & !(st.covered | st.failed) != 0
}

/// Loads the CJK fonts the interface language and the text seen need (or
/// swaps them for another region's) and appends them to every family of
/// `defs`. Returns whether `defs` changed.
pub(super) fn update(defs: &mut FontDefinitions, paths: &[PathBuf]) -> bool {
    let (ui, seen) = wanted();
    let Ok(mut st) = state().lock() else { return false };
    let mut changed = false;
    if let Some(r) = ui
        && st.region != Some(r)
    {
        // Another region: its faces replace the loaded ones.
        for chain in defs.families.values_mut() {
            chain.retain(|n| !st.names.contains(n));
        }
        for n in std::mem::take(&mut st.names) {
            defs.font_data.remove(&n);
        }
        changed |= st.region.is_some();
        *st = State { region: Some(r), ..Default::default() };
    }
    let mut missing = (seen | ui.map_or(0, Region::scripts)) & !(st.covered | st.failed);
    if missing == 0 {
        return changed;
    }
    let region = if missing & CJK != 0 { *st.region.get_or_insert_with(|| guess(seen)) } else { st.region.unwrap_or_else(|| guess(seen)) };
    let by_name: HashMap<String, &PathBuf> = paths.iter().filter_map(|p| Some((p.file_name()?.to_str()?.to_lowercase(), p))).collect();
    for bit in [HAN, KANA, HANGUL, ARABIC, HEBREW, THAI, DEVANAGARI] {
        if missing & bit == 0 {
            continue;
        }
        let files: Vec<&PathBuf> = if bit & CJK == 0 {
            script_files(bit).iter().filter_map(|f| by_name.get(*f).copied()).collect()
        } else {
            // The region's own fonts first, then those of the region that
            // writes the script, then any other.
            let own = match bit {
                KANA => Region::Jp,
                HANGUL => Region::Kr,
                _ => region,
            };
            let order = [region, own].into_iter().chain(REGIONS);
            order.flat_map(|r| r.files().iter().filter_map(|f| by_name.get(*f).copied())).collect()
        };
        let found = files.into_iter().find_map(|p| {
            let name = format!("cjk:{}", p.display());
            if st.names.iter().any(|n| n.starts_with(&name)) {
                return None;
            }
            let index = if bit & CJK == 0 { 0 } else { face_index(p, region) };
            let bytes = apex_fs::read(p).ok()?;
            let covers = coverage(&bytes, index);
            (covers & bit != 0).then(|| (format!("{name}#{index}"), bytes, index, covers, p))
        });
        let Some((name, bytes, index, covers, path)) = found else {
            st.failed |= bit;
            continue;
        };
        let mut fd = FontData::from_owned(bytes);
        fd.index = index;
        defs.font_data.insert(name.clone(), Arc::new(fd));
        // Bold text of other scripts gets the font's bold face.
        let bold = (bit & CJK == 0)
            .then(|| bold_file(path).and_then(|f| by_name.get(&f).copied()))
            .flatten()
            .and_then(|b| Some((format!("cjk:{}#0", b.display()), apex_fs::read(b).ok()?)))
            .filter(|(b, _)| !st.names.contains(b));
        if let Some((b, bytes)) = &bold {
            defs.font_data.insert(b.clone(), Arc::new(FontData::from_owned(bytes.clone())));
        }
        for (family, chain) in defs.families.iter_mut() {
            if let Some((b, _)) = bold.as_ref().filter(|_| is_bold(family))
                && !chain.contains(b)
            {
                chain.push(b.clone());
            }
            if !chain.contains(&name) {
                chain.push(name.clone());
            }
        }
        if let Some((b, _)) = bold {
            st.names.push(b.clone());
            st.bold.push((name.clone(), b));
        }
        st.names.push(name);
        st.covered |= covers;
        missing &= !covers;
        changed = true;
    }
    changed
}

/// Whether a family of the font definitions is a bold one (`fonts::FACES`
/// and the faces of installed families).
pub(super) fn is_bold(f: &eframe::egui::FontFamily) -> bool {
    matches!(f, eframe::egui::FontFamily::Name(n) if n.ends_with(":1") || n.ends_with(":3") || n.ends_with("-b"))
}

/// The bold face of a fallback font loaded as `name`, if any.
pub(super) fn bold_of(name: &str) -> Option<String> {
    let st = state().lock().ok()?;
    st.bold.iter().find(|(r, _)| r == name).map(|(_, b)| b.clone())
}

/// File name of the bold face of a regular font file, as fonts name them.
fn bold_file(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_lowercase();
    if name.contains("-regular.") {
        return Some(name.replace("-regular.", "-bold."));
    }
    let (stem, ext) = name.rsplit_once('.')?;
    let bold = match stem {
        "arial" | "times" | "tahoma" => format!("{stem}bd"),
        "segoeui" => "segoeuib".into(),
        "freesans" | "freeserif" => format!("{stem}bold"),
        _ => format!("{stem}-bold"),
    };
    Some(format!("{bold}.{ext}"))
}

/// The region for CJK text in a non-CJK interface: the system's, else the
/// one that writes the scripts seen.
fn guess(seen: u8) -> Region {
    if let Some(r) = apex_i18n::env_locale().and_then(|l| Region::of_lang(&l)) {
        return r;
    }
    if seen & KANA != 0 {
        Region::Jp
    } else if seen & HANGUL != 0 && seen & HAN == 0 {
        Region::Kr
    } else {
        Region::Sc
    }
}

/// The face of a font collection for `region`, else the first.
fn face_index(path: &Path, region: Region) -> u32 {
    let faces = super::read_faces(path);
    region.faces().iter().find_map(|want| faces.iter().find(|f| f.family.to_lowercase() == *want)).map_or(0, |f| f.index)
}

/// Scripts a font has glyphs for.
fn coverage(font: &[u8], index: u32) -> u8 {
    use skrifa::MetadataProvider;
    let Ok(f) = skrifa::FontRef::from_index(font, index) else { return 0 };
    let cmap = f.charmap();
    [('漢', HAN), ('か', KANA), ('한', HANGUL), ('ب', ARABIC), ('א', HEBREW), ('ก', THAI), ('क', DEVANAGARI)].iter().filter(|(c, _)| cmap.map(*c).is_some()).fold(0, |b, (_, bit)| b | bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_of_text() {
        assert_eq!(script('漢'), HAN);
        assert_eq!(script('の'), KANA);
        assert_eq!(script('ア'), KANA);
        assert_eq!(script('한'), HANGUL);
        assert_eq!(script('。'), HAN);
        assert_eq!(script('é'), 0);
        assert_eq!(script('ب'), ARABIC);
        assert_eq!(script('ש'), HEBREW);
        assert_eq!(Region::of_lang("ja"), Some(Region::Jp));
        assert_eq!(Region::of_lang("zh-Hant"), Some(Region::Tc));
        assert_eq!(Region::of_lang("zh_CN.UTF-8"), Some(Region::Sc));
        assert_eq!(Region::of_lang("ko"), Some(Region::Kr));
        assert_eq!(Region::of_lang("en"), None);
    }

    /// With Noto Sans CJK installed (Linux), each region gets its own face
    /// of the collection.
    #[test]
    fn picks_regional_face() {
        let mut paths = Vec::new();
        for d in super::super::font_dirs() {
            super::super::scan(&d, 4, &mut paths);
        }
        let Some(p) = paths.iter().find(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("NotoSansCJK-Regular.ttc"))) else { return };
        let faces = super::super::read_faces(p);
        for (r, fam) in [(Region::Sc, "Noto Sans CJK SC"), (Region::Tc, "Noto Sans CJK TC"), (Region::Jp, "Noto Sans CJK JP"), (Region::Kr, "Noto Sans CJK KR")] {
            let i = face_index(p, r);
            assert_eq!(faces.iter().find(|f| f.index == i).unwrap().family, fam);
        }
    }
}
