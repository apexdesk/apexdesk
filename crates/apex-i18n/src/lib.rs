//! User interface translations, shared by the window (apex-app) and the
//! terminal interface (apex-tui).
//!
//! Each language lives in `crates/apex-app/i18n/<code>.txt` as `key = text`
//! lines (`#` starts a comment, `\n` inserts a line break, `{name}` is a
//! placeholder). English (`en.txt`) is the base language: it must define
//! every key, and any key missing from another language falls back to it.
//! The build script embeds every file in the folder, so adding a language
//! only needs a new file.

pub mod functions;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

include!(concat!(env!("OUT_DIR"), "/langs.rs"));

pub const BASE: &str = "en";

pub struct Lang {
    pub code: &'static str,
    /// Native name, from the file's `lang.name` key.
    pub name: &'static str,
    src: &'static str,
    /// Parsed on first use: only the languages shown are paid for.
    table: OnceLock<HashMap<&'static str, String>>,
}

impl Lang {
    /// The text of a key in this language only.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.table().get(key).map(String::as_str)
    }

    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.table().keys().copied()
    }

    fn table(&self) -> &HashMap<&'static str, String> {
        self.table.get_or_init(|| parse(self.src))
    }
}

fn parse(src: &'static str) -> HashMap<&'static str, String> {
    let mut m = HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim(), v.trim().replace("\\n", "\n"));
        }
    }
    m
}

/// Every language, the base one first.
pub fn langs() -> &'static [Lang] {
    static L: OnceLock<Vec<Lang>> = OnceLock::new();
    L.get_or_init(|| {
        let mut v: Vec<Lang> = LANG_FILES
            .iter()
            .map(|&(code, src)| {
                let name = src
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("lang.name").map(str::trim_start).and_then(|r| r.strip_prefix('=')))
                    .map_or(code, str::trim);
                Lang { code, name, src, table: OnceLock::new() }
            })
            .collect();
        // Base language first.
        v.sort_by_key(|l| (l.code != BASE, l.code));
        v
    })
}

static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn available() -> impl Iterator<Item = &'static Lang> {
    langs().iter()
}

pub fn current() -> &'static str {
    langs()[CURRENT.load(Ordering::Relaxed)].code
}

/// Selects a language by code (`es`, `pt-BR`, `zh_TW.UTF-8`...); unknown
/// codes are ignored.
pub fn set(code: &str) -> bool {
    let code = code.trim();
    let found = langs()
        .iter()
        .position(|l| l.code.eq_ignore_ascii_case(code))
        .or_else(|| {
            let tag = normalize(code)?;
            langs().iter().position(|l| l.code.eq_ignore_ascii_case(&tag))
        });
    if let Some(i) = found {
        CURRENT.store(i, Ordering::Relaxed);
    }
    found.is_some()
}

/// The language file for a locale name: "es_ES.UTF-8" -> "es". Chinese
/// picks its script: Taiwan, Hong Kong and Macau write traditional
/// characters ("zh_TW" -> "zh-Hant"), the rest simplified ones.
fn normalize(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next()?.replace('_', "-").to_ascii_lowercase();
    let mut parts = base.split('-');
    let lang = parts.next().filter(|l| !l.is_empty())?;
    if lang == "zh" {
        let hant = parts.any(|p| matches!(p, "hant" | "tw" | "hk" | "mo"));
        return Some(if hant { "zh-Hant" } else { "zh" }.to_string());
    }
    Some(lang.to_string())
}

/// East Asian languages need a font with Han, kana or Hangul glyphs, in
/// the regional shape of the Han characters: "sc" (simplified Chinese),
/// "tc" (traditional), "jp" or "kr". None for other languages.
pub fn cjk_region(code: &str) -> Option<&'static str> {
    match normalize(code)?.as_str() {
        "zh" => Some("sc"),
        "zh-Hant" => Some("tc"),
        "ja" => Some("jp"),
        "ko" => Some("kr"),
        _ => None,
    }
}

/// Picks the language of the environment (`LC_ALL`, `LC_MESSAGES`, `LANG`);
/// for programs that have no settings of their own to read it from.
pub fn set_from_env() {
    if let Some(v) = env_locale() {
        set(&v);
    }
}

/// The locale name of the environment ("ja_JP.UTF-8"), if any: its
/// variables, else what [`set_system_locale`] said.
pub fn env_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty())).or_else(|| SYSTEM_LOCALE.get().cloned())
}

static SYSTEM_LOCALE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The system's locale where there are no environment variables to say it
/// (the browser's language, "es_ES").
pub fn set_system_locale(locale: &str) {
    let _ = SYSTEM_LOCALE.set(locale.to_string());
}

/// Translates a key into the current language.
pub fn tr(key: &'static str) -> &'static str {
    let l = langs();
    let cur = &l[CURRENT.load(Ordering::Relaxed)];
    match cur.table().get(key).or_else(|| l[0].table().get(key)) {
        Some(s) => s.as_str(),
        None => key,
    }
}

/// Translates a key built at run time; None when no language defines it.
pub fn lookup(key: &str) -> Option<&'static str> {
    let l = langs();
    let cur = &l[CURRENT.load(Ordering::Relaxed)];
    cur.table().get(key).or_else(|| l[0].table().get(key)).map(|s| s.as_str())
}

/// Translates a key and fills `{name}` placeholders.
pub fn trf(key: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = tr(key).to_string();
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), &value.to_string());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_names() {
        assert_eq!(normalize("es_ES.UTF-8").as_deref(), Some("es"));
        assert_eq!(normalize("zh_CN.UTF-8").as_deref(), Some("zh"));
        assert_eq!(normalize("zh-Hans-SG").as_deref(), Some("zh"));
        assert_eq!(normalize("zh_TW.UTF-8").as_deref(), Some("zh-Hant"));
        assert_eq!(normalize("zh_HK").as_deref(), Some("zh-Hant"));
        assert_eq!(normalize("zh-Hant").as_deref(), Some("zh-Hant"));
        assert_eq!(normalize("ja_JP.eucJP").as_deref(), Some("ja"));
        assert_eq!(normalize("ko_KR.UTF-8@euro").as_deref(), Some("ko"));
        assert_eq!(cjk_region("ja_JP.UTF-8"), Some("jp"));
        assert_eq!(cjk_region("zh_TW"), Some("tc"));
        assert_eq!(cjk_region("zh"), Some("sc"));
        assert_eq!(cjk_region("fr_FR"), None);
    }

    #[test]
    fn picks_east_asian_languages() {
        for (locale, code) in [("ja_JP.UTF-8", "ja"), ("zh_CN.UTF-8", "zh"), ("zh_TW.UTF-8", "zh-Hant"), ("zh_HK.Big5", "zh-Hant"), ("ko_KR.UTF-8", "ko")] {
            assert!(set(locale), "{locale}");
            assert_eq!(current(), code);
        }
        set(BASE);
    }
}
