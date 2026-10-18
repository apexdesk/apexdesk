//! The user's own word list ("Add to dictionary").

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use crate::casing::{Cap, captype, initcap_lower, lower};

/// A user word list (one word per line, UTF-8), checked before the
/// dictionary. Case works like Hunspell: "hello" also accepts "Hello" and
/// "HELLO"; "Paris" accepts "PARIS" but not "paris".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Personal {
    words: BTreeSet<String>,
}

impl Personal {
    pub fn new() -> Personal {
        Personal::default()
    }

    /// Adds a word; returns false if it was already there.
    pub fn add(&mut self, word: &str) -> bool {
        let w = word.trim();
        !w.is_empty() && self.words.insert(w.to_string())
    }

    /// Removes a word; returns false if it wasn't there.
    pub fn remove(&mut self, word: &str) -> bool {
        self.words.remove(word.trim())
    }

    /// Is the word (or its lower-case/capitalized form, per its case) listed?
    pub fn contains(&self, word: &str) -> bool {
        let w = word.trim();
        if self.words.contains(w) {
            return true;
        }
        match captype(w) {
            Cap::Init => self.words.contains(&lower(w)),
            Cap::All => self.words.contains(&lower(w)) || self.words.contains(&initcap_lower(w)),
            _ => false,
        }
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The words, sorted.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.words.iter().map(String::as_str)
    }

    /// Reads a word list. A missing file is an empty list, not an error.
    /// Lines starting with '#' and blank lines are ignored.
    pub fn load(path: &Path) -> io::Result<Personal> {
        let bytes = match apex_fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Personal::default()),
            Err(e) => return Err(e),
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut p = Personal::default();
        for line in text.lines() {
            let l = line.trim().trim_start_matches('\u{feff}');
            if !l.is_empty() && !l.starts_with('#') {
                // Hunspell personal dictionaries may carry "word/flags".
                let w = l.split('/').next().unwrap_or(l);
                p.add(w);
            }
        }
        Ok(p)
    }

    /// Writes the list (sorted, one word per line), creating the parent
    /// directory if needed. The file is replaced atomically.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            apex_fs::create_dir_all(dir)?;
        }
        let mut out = String::new();
        for w in &self.words {
            out.push_str(w);
            out.push('\n');
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        apex_fs::write(&tmp, out)?;
        apex_fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_rules() {
        let mut p = Personal::new();
        assert!(p.add("apexdesk"));
        assert!(!p.add("apexdesk"));
        p.add("Zaragoza");
        assert!(p.contains("apexdesk") && p.contains("Apexdesk") && p.contains("APEXDESK"));
        assert!(p.contains("Zaragoza") && p.contains("ZARAGOZA"));
        assert!(!p.contains("zaragoza"));
        assert!(p.remove("apexdesk"));
        assert!(!p.contains("apexdesk"));
    }

    #[test]
    fn round_trip() {
        let dir = apex_fs::temp_dir().join(format!("apex-spell-personal-{}", apex_fs::process_id()));
        let path = dir.join("sub").join("words.txt");
        let mut p = Personal::new();
        p.add("señor");
        p.add("ApexDesk");
        p.save(&path).expect("save");
        let q = Personal::load(&path).expect("load");
        assert_eq!(p, q);
        assert!(Personal::load(&dir.join("missing.txt")).expect("missing is empty").is_empty());
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
