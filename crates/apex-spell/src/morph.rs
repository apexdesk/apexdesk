//! Which dictionary stems and affixes make a word: what the grammar rules
//! use to tell a noun from a verb, or a plural from a singular.

use crate::Dictionary;
use crate::aff::{Affix, Flag, FlagMode};
use crate::check::Hit;

/// One way a word comes from a dictionary stem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    /// The stem as the dictionary has it ("casa" for "casas").
    pub root: String,
    /// The stem's flags.
    pub flags: Vec<String>,
    /// Flags of the affixes added: the prefix, then the inner suffix, then
    /// the outer one. Empty for the stem itself.
    pub affixes: Vec<String>,
}

impl Analysis {
    pub fn has_flag(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x == f)
    }

    /// The last suffix (or prefix) added.
    pub fn last_affix(&self) -> Option<&str> {
        self.affixes.last().map(String::as_str)
    }
}

/// `word` without the suffix's appended text, with its stripped text back.
fn unsuffix(word: &str, s: &Affix) -> Option<String> {
    word.strip_suffix(&*s.append).map(|h| format!("{h}{}", s.strip))
}

fn unprefix(word: &str, p: &Affix) -> Option<String> {
    word.strip_prefix(&*p.append).map(|t| format!("{}{t}", p.strip))
}

impl Dictionary {
    fn flag_name(&self, f: Flag) -> String {
        match self.aff.flag_mode {
            FlagMode::Char | FlagMode::Utf8 => char::from_u32(f).map(String::from).unwrap_or_default(),
            FlagMode::Long => [f >> 16, f & 0xFFFF].iter().filter(|&&c| c != 0).filter_map(|&c| char::from_u32(c)).collect(),
            FlagMode::Num => f.to_string(),
        }
    }

    fn set_names(&self, set: u32) -> Vec<String> {
        self.sets.get(set).iter().map(|&f| self.flag_name(f)).collect()
    }

    fn analysis(&self, word: &str, hit: &Hit) -> Option<Analysis> {
        let mut w = word.to_string();
        if let Some(p) = hit.pfx {
            w = unprefix(&w, p)?;
        }
        if let Some(s) = hit.sfx {
            w = unsuffix(&w, s)?;
        }
        if let Some(s) = hit.sfx2 {
            w = unsuffix(&w, s)?;
        }
        let affixes = hit.pfx.into_iter().chain(hit.sfx2).chain(hit.sfx).map(|a| self.flag_name(a.flag)).collect();
        Some(Analysis { root: w, flags: self.set_names(hit.root), affixes })
    }

    /// The ways the word comes from the dictionary's stems: as a stem of
    /// its own, or a stem with a prefix and suffixes. Exactly as written,
    /// then in lower case if it starts with a capital. Compounds aren't
    /// analysed.
    pub fn analyze(&self, word: &str) -> Vec<Analysis> {
        let word = self.normalize(word.trim());
        let mut out = Vec::new();
        let lower = word.to_lowercase();
        let mut forms = vec![word.as_ref()];
        if lower != word {
            forms.push(&lower);
        }
        for w in forms {
            if !out.is_empty() {
                break;
            }
            if let Some(h) = self.words.get(w) {
                for &s in h.as_slice() {
                    if !self.sets.has_opt(s, self.aff.forbidden) && !self.sets.has_opt(s, self.aff.needaffix) {
                        out.push(Analysis { root: w.to_string(), flags: self.set_names(s), affixes: Vec::new() });
                    }
                }
            }
            crate::check::budgeted_with(1 << 16, || {
                self.affix_search(w, true, true, &mut |hit| {
                    if !self.sets.has_opt(hit.root, self.aff.forbidden)
                        && let Some(a) = self.analysis(w, hit)
                        && !out.contains(&a)
                    {
                        out.push(a);
                    }
                    false
                })
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_and_affixes() {
        let aff = "SET UTF-8\nSFX S Y 2\nSFX S 0 s [aeo]\nSFX S 0 es [^aeo]\nSFX G Y 1\nSFX G o a/S o\nSFX R Y 1\nSFX R ar o ar\nPFX d Y 1\nPFX d 0 des .\n";
        let d = Dictionary::from_strings(aff, "4\ncasa/S\nniño/GS\ncasar/Rd\nárbol/S\n").expect("parse");
        let a = d.analyze("casas");
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].root.as_str(), a[0].last_affix()), ("casa", Some("S")));
        let a = d.analyze("niñas");
        assert_eq!((a[0].root.as_str(), a[0].affixes.clone()), ("niño", vec!["G".to_string(), "S".to_string()]));
        // "caso" is a form of the verb.
        assert!(d.analyze("caso").iter().any(|a| a.root == "casar" && a.last_affix() == Some("R")));
        assert_eq!(d.analyze("descasar")[0].affixes, ["d"]);
        assert_eq!(d.analyze("árboles")[0].root, "árbol");
        // The stem itself, and capitalised.
        let a = d.analyze("Casa");
        assert!(a[0].affixes.is_empty() && a[0].has_flag("S"));
        assert!(d.analyze("xyz").is_empty());
    }
}
