//! Hunspell-compatible spell checking for ApexDesk.
//!
//! Reads the same `.aff`/`.dic` dictionaries LibreOffice and Firefox use
//! (`/usr/share/hunspell`), checks words with Hunspell's affix, case and
//! compounding rules and produces ranked suggestions. Pure logic: no I/O
//! besides loading dictionaries and personal word lists.
//!
//! ```no_run
//! use apex_spell::{Dictionary, words};
//! let dict = Dictionary::load("/usr/share/hunspell/en_US.aff".as_ref(), "/usr/share/hunspell/en_US.dic".as_ref())?;
//! for (range, word) in words("Teh quick brown fox") {
//!     if !dict.check(word) {
//!         println!("{range:?} {word}: {:?}", dict.suggest(word, 5));
//!     }
//! }
//! # Ok::<(), apex_spell::Error>(())
//! ```

mod aff;
mod casing;
mod charsets;
mod check;
mod encoding;
mod grammar;
mod hash;
mod installed;
mod morph;
mod personal;
mod suggest;
mod thesaurus;
mod tokenize;

use std::fmt;
use std::path::Path;

use aff::{Aff, FlagSets};
use casing::Cap;
use hash::FxHashMap;

pub use grammar::{Issue, RULES, check_grammar};
pub use installed::{add_search_dir, installed};
pub use morph::Analysis;
pub use personal::Personal;
pub use suggest::set_time_budget;
pub use thesaurus::{Meaning, Thesaurus, installed_thesauri, thesaurus_for};
pub use tokenize::{Words, words};

/// Error loading a dictionary.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The files could be read but don't look like a Hunspell dictionary.
    Format(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Format(m) => write!(f, "invalid dictionary: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Format(_) => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

/// Flag sets of the dictionary entries sharing one spelling (homonyms).
pub(crate) enum Homs {
    One(u32),
    Many(Box<[u32]>),
}

impl Homs {
    #[inline]
    pub(crate) fn as_slice(&self) -> &[u32] {
        match self {
            Homs::One(x) => std::slice::from_ref(x),
            Homs::Many(v) => v,
        }
    }
}

fn insert(words: &mut FxHashMap<Box<str>, Homs>, word: &str, flags: u32) {
    match words.get_mut(word) {
        Some(h) => match h {
            Homs::One(x) if *x != flags => *h = Homs::Many(Box::new([*x, flags])),
            Homs::Many(v) if !v.contains(&flags) => {
                let mut n = v.to_vec();
                n.push(flags);
                *h = Homs::Many(n.into_boxed_slice());
            }
            _ => {}
        },
        None => {
            words.insert(word.into(), Homs::One(flags));
        }
    }
}

/// A loaded Hunspell dictionary. Immutable and `Send + Sync`, so it can be
/// shared between the UI and a background checking thread behind an `Arc`.
pub struct Dictionary {
    pub(crate) aff: Aff,
    pub(crate) sets: FlagSets,
    /// Stem -> flag sets of its homonyms.
    pub(crate) words: FxHashMap<Box<str>, Homs>,
    lang: String,
    pub(crate) ngram_index: std::sync::OnceLock<suggest::NgramIndex>,
}

impl fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dictionary").field("lang", &self.lang).field("words", &self.words.len()).finish()
    }
}

impl Dictionary {
    /// Loads a dictionary from its `.aff` and `.dic` files. The language code
    /// is taken from the file name ("es_ES.aff" -> "es_ES").
    pub fn load(aff: &Path, dic: &Path) -> Result<Dictionary, Error> {
        let aff_bytes = apex_fs::read(aff)?;
        let dic_bytes = apex_fs::read(dic)?;
        let mut d = Dictionary::from_bytes(&aff_bytes, &dic_bytes)?;
        if let Some(stem) = aff.file_stem().and_then(|s| s.to_str())
            && !stem.is_empty()
        {
            d.lang = stem.to_string();
        }
        Ok(d)
    }

    /// Builds a dictionary from the contents of the `.aff` and `.dic` files.
    /// The language code comes from the `LANG` directive, if any.
    pub fn from_strings(aff: &str, dic: &str) -> Result<Dictionary, Error> {
        // Already decoded: force UTF-8 whatever SET says.
        Dictionary::build(aff, dic)
    }

    fn from_bytes(aff: &[u8], dic: &[u8]) -> Result<Dictionary, Error> {
        if aff.contains(&0) || dic.contains(&0) {
            return Err(Error::Format("binary data in .aff/.dic".into()));
        }
        let enc = encoding::detect(aff);
        let aff = encoding::decode(aff, enc);
        let dic = encoding::decode(dic, enc);
        Dictionary::build(&aff, &dic)
    }

    fn build(aff_text: &str, dic_text: &str) -> Result<Dictionary, Error> {
        let mut sets = FlagSets::new();
        let aff = aff::parse(aff_text, &mut sets);
        let mut lines = dic_text.lines();
        let mut first = lines.next();
        let mut capacity = 0;
        if let Some(l) = first
            && let Ok(n) = l.trim().trim_start_matches('\u{feff}').parse::<usize>()
        {
            capacity = n;
            first = None;
        }
        let mut words: FxHashMap<Box<str>, Homs> = FxHashMap::default();
        words.reserve(capacity.min(4_000_000));
        let mut word_buf = String::new();
        for line in first.into_iter().chain(lines) {
            // Morphological fields follow a tab (or a space); we don't use them.
            let entry = line.split('\t').next().unwrap_or("");
            let entry = entry.split(' ').next().unwrap_or("").trim_end_matches('\r');
            if entry.is_empty() {
                continue;
            }
            let (word, flags) = match aff::split_unescaped_slash(entry) {
                Some((w, f)) => (w, aff.parse_flags(f, &mut sets)),
                None => (entry, 0),
            };
            word_buf.clear();
            if word.contains("\\/") || !aff.ignore.is_empty() {
                word_buf.extend(word.replace("\\/", "/").chars().filter(|c| !aff.ignore.contains(c)));
            } else {
                word_buf.push_str(word);
            }
            if word_buf.is_empty() {
                continue;
            }
            insert(&mut words, &word_buf, flags);
            // Hidden capitalized copy so upper-case input matches mixed-case
            // stems and affixed acronyms: "iOS" -> "IOS", "CIA's" -> "CIA'S".
            let cap = casing::captype(&word_buf);
            let flagged = !sets.get(flags).is_empty();
            if (matches!(cap, Cap::Huh | Cap::HuhInit) || (cap == Cap::All && flagged))
                && !sets.has_opt(flags, aff.forbidden)
            {
                let hidden = casing::initcap_lower(&word_buf);
                if hidden != word_buf {
                    let mut v = sets.get(flags).to_vec();
                    v.push(aff::ONLYUPCASE);
                    let hf = sets.intern(v);
                    insert(&mut words, &hidden, hf);
                }
            }
        }
        words.shrink_to_fit();
        sets.freeze();
        let lang = aff.lang.clone();
        Ok(Dictionary { aff, sets, words, lang, ngram_index: std::sync::OnceLock::new() })
    }

    /// The language code, e.g. "es_ES": the file name for [`load`](Self::load),
    /// the `LANG` directive for [`from_strings`](Self::from_strings) (may be empty).
    pub fn language(&self) -> &str {
        &self.lang
    }

    /// Characters the dictionary considers part of words besides letters
    /// (Hunspell's `WORDCHARS`), for callers that tokenize text themselves.
    pub fn word_chars(&self) -> &str {
        &self.aff.wordchars
    }

    /// Number of stems in the dictionary.
    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// True for a dictionary without any stem.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_is_send_sync() {
        fn assert<T: Send + Sync>() {}
        assert::<Dictionary>();
        assert::<Personal>();
    }

    #[test]
    fn homonyms_and_counts() {
        let d = Dictionary::from_strings("LANG xx_XX\nSFX S Y 1\nSFX S 0 s .\n", "3\nrápido\nrápido/S\nrápido/S\n")
            .expect("parse");
        assert_eq!(d.language(), "xx_XX");
        assert_eq!(d.len(), 1);
        assert_eq!(d.words.get("rápido").map(|h| h.as_slice().len()), Some(2));
    }
}
