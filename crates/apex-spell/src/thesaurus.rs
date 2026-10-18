//! Thesauri in the MyThes format LibreOffice uses (`th_es_ES_v2.dat`):
//! a first line with the encoding, then entries `word|count` followed by
//! `count` lines `(part of speech)|synonym|synonym…`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::encoding::{by_name, decode};

/// One meaning of a word: its part of speech ("(noun)") and synonyms
/// (with MyThes' notes like "(similar term)" taken off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meaning {
    pub pos: String,
    pub words: Vec<String>,
}

pub struct Thesaurus {
    text: String,
    /// Lowercase word -> byte offset of its first meaning line and the
    /// number of lines.
    index: HashMap<String, (usize, usize)>,
}

impl Thesaurus {
    pub fn load(dat: &Path) -> std::io::Result<Thesaurus> {
        let bytes = apex_fs::read(dat)?;
        let first = bytes.split(|&b| b == b'\n').next().unwrap_or_default();
        let enc = by_name(String::from_utf8_lossy(first).trim().to_ascii_uppercase().as_str());
        Ok(Thesaurus::from_text(decode(&bytes, enc)))
    }

    /// From the decoded file (its first line is the encoding).
    pub fn from_text(text: String) -> Thesaurus {
        let mut index = HashMap::new();
        let mut pos = text.find('\n').map_or(text.len(), |i| i + 1);
        while pos < text.len() {
            let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
            let line = text[pos..end].trim_end_matches('\r');
            pos = (end + 1).min(text.len());
            let Some((word, n)) = line.rsplit_once('|') else { continue };
            let Ok(n) = n.trim().parse::<usize>() else { continue };
            index.entry(word.to_lowercase()).or_insert((pos, n));
            // Skip its meaning lines.
            for _ in 0..n {
                if pos >= text.len() {
                    break;
                }
                pos = text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1);
            }
        }
        Thesaurus { text, index }
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The meanings of a word (any case).
    pub fn lookup(&self, word: &str) -> Vec<Meaning> {
        let key = word.replace('\u{AD}', "").trim().to_lowercase();
        let Some(&(start, n)) = self.index.get(&key) else { return Vec::new() };
        self.text[start..]
            .lines()
            .take(n)
            .filter_map(|line| {
                let mut parts = line.trim_end_matches('\r').split('|');
                let pos = parts.next()?.trim().to_string();
                let words: Vec<String> = parts
                    .map(|w| match w.find(" (") {
                        Some(i) if w.ends_with(" term)") || w.ends_with("(antonym)") => w[..i].trim().to_string(),
                        _ => w.trim().to_string(),
                    })
                    .filter(|w| !w.is_empty() && w.to_lowercase() != key)
                    .collect();
                (!words.is_empty()).then_some(Meaning { pos, words })
            })
            .collect()
    }
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = crate::installed::extra_dirs();
    if let Some(p) = std::env::var_os("DICPATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/share/mythes"));
        dirs.push(Path::new(&home).join(".local/share/hunspell"));
    }
    for d in ["/usr/share/mythes", "/usr/share/myspell", "/usr/share/myspell/dicts", "/usr/local/share/mythes"] {
        dirs.push(PathBuf::from(d));
    }
    // LibreOffice's dictionary extensions.
    let mut ext_roots: Vec<PathBuf> = vec![PathBuf::from("/usr/lib/libreoffice/share/extensions"), PathBuf::from("/opt/libreoffice/share/extensions")];
    if cfg!(windows) {
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            dirs.push(exe.join("dictionaries"));
        }
        if let Some(a) = std::env::var_os("APPDATA") {
            dirs.push(Path::new(&a).join("apexdesk").join("dictionaries"));
        }
        ext_roots.extend(["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(std::env::var_os).map(|pf| Path::new(&pf).join("LibreOffice").join("share").join("extensions")));
    }
    if cfg!(target_os = "macos") {
        ext_roots.push(PathBuf::from("/Applications/LibreOffice.app/Contents/Resources/extensions"));
    }
    for ext in ext_roots {
        if let Ok(rd) = apex_fs::read_dir(&ext) {
            let mut found: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("dict-"))).collect();
            found.sort();
            dirs.extend(found);
        }
    }
    dirs
}

/// The language of a thesaurus file ("th_es_ES_v2.dat" -> "es_ES").
fn lang_of(name: &str) -> Option<String> {
    let stem = name.strip_prefix("th_")?.strip_suffix(".dat")?;
    Some(stem.strip_suffix("_v2").unwrap_or(stem).to_string())
}

/// Installed thesauri: (language like "es_ES", .dat path), by language.
pub fn installed_thesauri() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for dir in search_dirs() {
        let Ok(rd) = apex_fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let Some(lang) = f.file_name().and_then(|n| lang_of(&n.to_string_lossy())) else { continue };
            if !out.iter().any(|(l, _)| *l == lang) {
                out.push((lang, f));
            }
        }
    }
    out.sort();
    out
}

/// The thesaurus for a spelling language: the same code, else the same
/// language ("es_MX" -> "es_ES" or "es").
pub fn thesaurus_for(lang: &str) -> Option<PathBuf> {
    let all = installed_thesauri();
    let base = lang.split(['_', '-']).next().unwrap_or(lang);
    all.iter().find(|(l, _)| l == lang).or_else(|| all.iter().find(|(l, _)| l.split(['_', '-']).next() == Some(base))).map(|(_, p)| p.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "UTF-8\ncasa|2\n(nombre)|hogar|vivienda|domicilio\n(nombre)|familia|linaje\nhogar|1\n(nombre)|casa|lar\nGood|1\n(adj)|great (similar term)|bad (antonym)|fine\n";

    #[test]
    fn lookup() {
        let t = Thesaurus::from_text(SAMPLE.to_string());
        assert_eq!(t.len(), 3);
        let m = t.lookup("Casa");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], Meaning { pos: "(nombre)".into(), words: vec!["hogar".into(), "vivienda".into(), "domicilio".into()] });
        assert_eq!(m[1].words, ["familia", "linaje"]);
        assert_eq!(t.lookup("hogar")[0].words, ["casa", "lar"]);
        assert_eq!(t.lookup("good")[0].words, ["great", "bad", "fine"]);
        assert!(t.lookup("nada").is_empty());
        assert_eq!(t.lookup("ca\u{AD}sa").len(), 2);
    }

    #[test]
    fn names() {
        assert_eq!(lang_of("th_es_ES_v2.dat").as_deref(), Some("es_ES"));
        assert_eq!(lang_of("th_de_DE.dat").as_deref(), Some("de_DE"));
        assert_eq!(lang_of("es_ES.dic"), None);
    }
}
