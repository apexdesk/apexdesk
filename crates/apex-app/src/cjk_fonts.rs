//! Fonts for Chinese, Japanese and Korean text in documents: the free
//! fonts that stand in for the usual Windows and macOS ones (MS Mincho,
//! SimSun, Batang...), and an installed font of the right script and look
//! (serif or sans) when the document's is missing, so the text never shows
//! as boxes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use eframe::egui::FontId;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Script {
    Japanese,
    Simplified,
    Traditional,
    Korean,
}

/// Well known fonts: lowercase name, script, sans serif.
const KNOWN: &[(&str, Script, bool)] = &[
    ("ms mincho", Script::Japanese, false),
    ("ｍｓ 明朝", Script::Japanese, false),
    ("ms pmincho", Script::Japanese, false),
    ("ｍｓ ｐ明朝", Script::Japanese, false),
    ("yu mincho", Script::Japanese, false),
    ("游明朝", Script::Japanese, false),
    ("yumincho", Script::Japanese, false),
    ("hiragino mincho pron", Script::Japanese, false),
    ("hiragino mincho pro", Script::Japanese, false),
    ("ヒラギノ明朝 pron", Script::Japanese, false),
    ("ms gothic", Script::Japanese, true),
    ("ｍｓ ゴシック", Script::Japanese, true),
    ("ms pgothic", Script::Japanese, true),
    ("ｍｓ ｐゴシック", Script::Japanese, true),
    ("ms ui gothic", Script::Japanese, true),
    ("meiryo", Script::Japanese, true),
    ("メイリオ", Script::Japanese, true),
    ("meiryo ui", Script::Japanese, true),
    ("yu gothic", Script::Japanese, true),
    ("游ゴシック", Script::Japanese, true),
    ("yugothic", Script::Japanese, true),
    ("yu gothic ui", Script::Japanese, true),
    ("hiragino sans", Script::Japanese, true),
    ("hiragino kaku gothic pron", Script::Japanese, true),
    ("ヒラギノ角ゴ pron", Script::Japanese, true),
    ("osaka", Script::Japanese, true),
    ("simsun", Script::Simplified, false),
    ("宋体", Script::Simplified, false),
    ("nsimsun", Script::Simplified, false),
    ("新宋体", Script::Simplified, false),
    ("songti sc", Script::Simplified, false),
    ("stsong", Script::Simplified, false),
    ("fangsong", Script::Simplified, false),
    ("仿宋", Script::Simplified, false),
    ("kaiti", Script::Simplified, false),
    ("楷体", Script::Simplified, false),
    ("simhei", Script::Simplified, true),
    ("黑体", Script::Simplified, true),
    ("microsoft yahei", Script::Simplified, true),
    ("微软雅黑", Script::Simplified, true),
    ("dengxian", Script::Simplified, true),
    ("等线", Script::Simplified, true),
    ("pingfang sc", Script::Simplified, true),
    ("heiti sc", Script::Simplified, true),
    ("stheiti", Script::Simplified, true),
    ("pmingliu", Script::Traditional, false),
    ("新細明體", Script::Traditional, false),
    ("mingliu", Script::Traditional, false),
    ("細明體", Script::Traditional, false),
    ("mingliu_hkscs", Script::Traditional, false),
    ("songti tc", Script::Traditional, false),
    ("microsoft jhenghei", Script::Traditional, true),
    ("微軟正黑體", Script::Traditional, true),
    ("pingfang tc", Script::Traditional, true),
    ("heiti tc", Script::Traditional, true),
    ("batang", Script::Korean, false),
    ("바탕", Script::Korean, false),
    ("batangche", Script::Korean, false),
    ("gungsuh", Script::Korean, false),
    ("궁서", Script::Korean, false),
    ("applemyungjo", Script::Korean, false),
    ("gulim", Script::Korean, true),
    ("굴림", Script::Korean, true),
    ("dotum", Script::Korean, true),
    ("돋움", Script::Korean, true),
    ("malgun gothic", Script::Korean, true),
    ("맑은 고딕", Script::Korean, true),
    ("apple sd gothic neo", Script::Korean, true),
    ("applegothic", Script::Korean, true),
];

/// Installed fonts to use for a script and look, best first (Linux,
/// Windows and macOS names).
fn candidates(script: Script, sans: bool) -> &'static [&'static str] {
    match (script, sans) {
        (Script::Japanese, false) => &["noto serif cjk jp", "noto serif jp", "source han serif jp", "source han serif", "ipaexmincho", "ipamincho", "takaomincho", "yu mincho", "ms mincho", "hiragino mincho pron"],
        (Script::Japanese, true) => &["noto sans cjk jp", "noto sans jp", "source han sans jp", "source han sans", "ipaexgothic", "ipagothic", "takaogothic", "vl gothic", "yu gothic", "meiryo", "ms gothic", "hiragino sans", "hiragino kaku gothic pron"],
        (Script::Simplified, false) => &["noto serif cjk sc", "noto serif sc", "source han serif sc", "ar pl uming cn", "simsun", "songti sc"],
        (Script::Simplified, true) => &["noto sans cjk sc", "noto sans sc", "source han sans sc", "wenquanyi micro hei", "wenquanyi zen hei", "microsoft yahei", "simhei", "pingfang sc"],
        (Script::Traditional, false) => &["noto serif cjk tc", "noto serif tc", "noto serif cjk hk", "ar pl uming tw", "pmingliu", "mingliu", "songti tc"],
        (Script::Traditional, true) => &["noto sans cjk tc", "noto sans tc", "noto sans cjk hk", "microsoft jhenghei", "pingfang tc"],
        (Script::Korean, false) => &["noto serif cjk kr", "noto serif kr", "un batang", "nanummyeongjo", "batang", "applemyungjo"],
        (Script::Korean, true) => &["noto sans cjk kr", "noto sans kr", "nanumgothic", "un dotum", "malgun gothic", "gulim", "apple sd gothic neo"],
    }
}

/// The script of a language tag (Japanese when unknown).
pub fn script_of_lang(lang: Option<&str>) -> Script {
    let l = lang.unwrap_or("").to_ascii_lowercase();
    if l.starts_with("ko") {
        Script::Korean
    } else if l.starts_with("zh-tw") || l.starts_with("zh-hk") || l.starts_with("zh-mo") || l.contains("hant") {
        Script::Traditional
    } else if l.starts_with("zh") {
        Script::Simplified
    } else {
        Script::Japanese
    }
}

/// Whether a font name looks sans serif (gothic, hei, dotum...).
fn looks_sans(name: &str) -> bool {
    let n = name.to_lowercase();
    ["gothic", "sans", "hei", "yahei", "dotum", "gulim", "meiryo", "ゴシック", "黑", "黒", "고딕", "돋움", "굴림", "pingfang", "dengxian", "等线"].iter().any(|k| n.contains(k))
}

/// Whether a font name looks like one of a CJK font.
fn looks_cjk(name: &str) -> bool {
    !name.is_ascii()
        || KNOWN.iter().any(|k| k.0 == name)
        || name.split([' ', '-', '_']).any(|w| matches!(w, "cjk" | "jp" | "sc" | "tc" | "hk" | "kr" | "cn" | "tw" | "mincho" | "gothic" | "song" | "hei" | "kai" | "ming" | "myungjo" | "ipa" | "takao" | "nanum" | "han" | "meiryo" | "fallback"))
        || ["batang", "gulim", "dotum", "gungsuh", "ipaex", "wenquanyi", "simsun", "simhei", "mingliu", "kaiti", "fangsong"].iter().any(|k| name.contains(k))
}

/// The installed family (as the font list names it) to show text in
/// `name` for language `lang`: the font itself, a stand-in for it, or one
/// of the script's usual fonts. None while fonts are still being scanned
/// or when no font of the script is installed.
pub fn installed(name: Option<&str>, lang: Option<&str>) -> Option<String> {
    static CACHE: OnceLock<Mutex<(u64, HashMap<(String, Script), Option<String>>)>> = OnceLock::new();
    let script = script_of_lang(lang);
    let name = name.map(str::trim).filter(|n| !n.is_empty() && !n.starts_with('+')).unwrap_or("");
    let key = (name.to_lowercase(), script);
    let generation = crate::fonts::generation();
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(c) = cache.lock() {
        if c.0 == generation {
            if let Some(v) = c.1.get(&key) {
                return v.clone();
            }
        }
    }
    let families = crate::fonts::families();
    let found = find(&families, &key.0, script);
    if let Ok(mut c) = cache.lock() {
        if c.0 != generation {
            *c = (generation, HashMap::new());
        }
        c.1.insert(key, found.clone());
    }
    found
}

fn find(families: &[String], name: &str, script: Script) -> Option<String> {
    if families.is_empty() {
        return None;
    }
    let get = |k: &str| families.iter().find(|f| f.to_lowercase() == k).cloned();
    // Latin fonts have no CJK characters: those take a CJK font.
    if !name.is_empty() && looks_cjk(name) {
        if let Some(f) = get(name) {
            return Some(f);
        }
    }
    // A known font stands for its own script, whatever the language.
    let (script, sans) = match KNOWN.iter().find(|k| k.0 == name) {
        Some(&(_, s, sans)) => (s, sans),
        None => (script, looks_sans(name)),
    };
    // The script's fonts of the same look, then of the other look, then
    // any CJK font (all have the common ideographs).
    let order = [(script, sans), (script, !sans)];
    let others = [Script::Japanese, Script::Simplified, Script::Traditional, Script::Korean].into_iter().filter(|s| *s != script).flat_map(|s| [(s, sans), (s, !sans)]);
    order.into_iter().chain(others).flat_map(|(s, b)| candidates(s, b).iter()).find_map(|c| get(c))
}

/// An installed stand-in for a CJK font used as a text's (Latin) font,
/// when it is one and isn't installed.
pub fn stand_in(name: &str) -> Option<String> {
    let key = name.trim().to_lowercase();
    if name.is_ascii() && !KNOWN.iter().any(|k| k.0 == key) {
        return None;
    }
    installed(Some(name), None).filter(|f| f.to_lowercase() != key)
}

/// The font for Asian text in `name` (language `lang`), if one is
/// installed; and whether italics must be synthesized.
pub fn font_for(name: Option<&str>, lang: Option<&str>, bold: bool, italic: bool, size: f32) -> Option<(FontId, bool)> {
    let fam = installed(name, lang)?;
    Some(crate::fonts::font_for(Some(&fam), bold, italic, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_ins() {
        let fams: Vec<String> = ["Noto Sans CJK JP", "Noto Serif CJK JP", "Noto Sans CJK KR", "Liberation Serif"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find(&fams, "ｍｓ 明朝", Script::Japanese).as_deref(), Some("Noto Serif CJK JP"));
        assert_eq!(find(&fams, "ms gothic", Script::Japanese).as_deref(), Some("Noto Sans CJK JP"));
        assert_eq!(find(&fams, "맑은 고딕", Script::Japanese).as_deref(), Some("Noto Sans CJK KR"));
        // Unknown fonts: by the language, then any CJK font.
        assert_eq!(find(&fams, "", Script::Korean).as_deref(), Some("Noto Sans CJK KR"));
        assert_eq!(find(&fams, "some mincho", Script::Simplified).as_deref(), Some("Noto Serif CJK JP"));
        assert_eq!(find(&fams, "liberation serif", Script::Japanese).as_deref(), Some("Noto Serif CJK JP"));
        assert_eq!(script_of_lang(Some("zh-TW")), Script::Traditional);
    }
}
