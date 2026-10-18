//! User interface translations: the shared tables of the apex-i18n crate
//! (the terminal interface uses them too). The tests below check the keys
//! used in the sources of both interfaces.


pub use apex_i18n::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key used in the sources exists in the base language, and other
    /// languages only contain keys the base language knows.
    #[test]
    fn keys_are_consistent() {
        let base = &langs()[0];
        assert_eq!(base.code, BASE);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut missing = Vec::new();
        let mut files = Vec::new();
        // The terminal interface (apex-tui) and the shared function help use
        // the same keys.
        let mut dirs = vec![root.join("src"), root.join("../apex-tui/src"), root.join("../apex-i18n/src")];
        while let Some(d) = dirs.pop() {
            for e in apex_fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    dirs.push(e.path());
                } else {
                    files.push(e.path());
                }
            }
        }
        for path in &files {
            let text = apex_fs::read_to_string(path).unwrap();
            for pat in ["tr(\"", "trf(\""] {
                for (i, _) in text.match_indices(pat) {
                    if text[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let rest = &text[i + pat.len()..];
                    let key = &rest[..rest.find('"').unwrap()];
                    if base.get(key).is_none() {
                        missing.push(format!("{}: {}", path.display(), key));
                    }
                }
            }
        }
        // Keys held in tables: any "prefix.name" literal whose prefix is a
        // key prefix of the base language.
        let prefixes: std::collections::HashSet<&str> = base.keys().filter_map(|k| k.split_once('.').map(|x| x.0)).collect();
        for path in &files {
            let text = apex_fs::read_to_string(path).unwrap();
            for lit in text.split('"').skip(1).step_by(2) {
                let Some((pre, rest)) = lit.split_once('.') else { continue };
                // File names such as "index.html", not keys.
                if matches!(rest, "html" | "css" | "js" | "json" | "wasm" | "txt") {
                    continue;
                }
                let ok_chars = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                if prefixes.contains(pre) && ok_chars(pre) && ok_chars(rest) && base.get(lit).is_none() {
                    missing.push(format!("{}: {}", path.display(), lit));
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "keys missing from en.txt:\n{}", missing.join("\n"));
        for l in &langs()[1..] {
            let extra: Vec<_> = l.keys().filter(|k| base.get(k).is_none()).collect();
            assert!(extra.is_empty(), "{} has unknown keys: {:?}", l.code, extra);
        }
    }

    /// Every language translates every key of the base language (the
    /// `test.` keys are for the tests only).
    #[test]
    fn languages_are_complete() {
        let base = &langs()[0];
        let mut missing = Vec::new();
        for l in &langs()[1..] {
            let mut keys: Vec<_> = base.keys().filter(|k| !k.starts_with("test.") && l.get(k).is_none()).collect();
            keys.sort();
            if !keys.is_empty() {
                missing.push(format!("{} ({}): {}", l.code, keys.len(), keys.join(", ")));
            }
        }
        assert!(missing.is_empty(), "untranslated keys:\n{}", missing.join("\n"));
        for code in ["zh", "zh-Hant", "ja", "ko"] {
            assert!(langs().iter().any(|l| l.code == code), "no {code}.txt");
        }
    }

    /// Translations keep the base text's placeholders (date and time
    /// patterns pick their own).
    #[test]
    fn placeholders_match() {
        let holes = |s: &str| {
            let mut v: Vec<String> = s.split('{').skip(1).filter_map(|x| x.split_once('}').map(|(n, _)| n.to_string())).collect();
            v.sort();
            v.dedup();
            v
        };
        let base = &langs()[0];
        let mut bad = Vec::new();
        for l in &langs()[1..] {
            for (k, v) in l.keys().map(|k| (k, l.get(k).unwrap())) {
                if let Some(b) = base.get(k)
                    && !matches!(k, "doc.date_format" | "doc.time_format")
                    && holes(b) != holes(v)
                {
                    bad.push(format!("{}: {k}", l.code));
                }
            }
        }
        bad.sort();
        assert!(bad.is_empty(), "placeholders differ from en.txt:\n{}", bad.join("\n"));
    }

    #[test]
    fn fallback_and_placeholders() {
        assert!(set("es_ES.UTF-8") || langs().len() == 1);
        set(BASE);
        assert_eq!(trf("test.placeholder", &[("n", &3)]), "3 items");
    }
}
