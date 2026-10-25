//! Discovery of the Hunspell dictionaries installed on the system.

use apex_fs::PathExt;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

static EXTRA: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Searches `dir` first too (an organization's shared dictionaries). Call
/// it before the first `installed()`.
pub fn add_search_dir(dir: PathBuf) {
    let mut v = EXTRA.lock().unwrap_or_else(|e| e.into_inner());
    if !v.contains(&dir) {
        v.push(dir);
    }
}

pub(crate) fn extra_dirs() -> Vec<PathBuf> {
    EXTRA.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = extra_dirs();
    if let Some(p) = std::env::var_os("DICPATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/share/hunspell"));
    }
    for d in ["/usr/share/hunspell", "/usr/share/myspell", "/usr/share/myspell/dicts", "/usr/local/share/hunspell"] {
        dirs.push(PathBuf::from(d));
    }
    if cfg!(windows) {
        // Next to the program, the user's folder, and LibreOffice's.
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            dirs.push(exe.join("dictionaries"));
        }
        if let Some(a) = std::env::var_os("APPDATA") {
            dirs.push(Path::new(&a).join("apexdesk").join("dictionaries"));
        }
        for pf in ["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(std::env::var_os) {
            let ext = Path::new(&pf).join("LibreOffice").join("share").join("extensions");
            if let Ok(rd) = apex_fs::read_dir(&ext) {
                let mut found: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("dict-"))).collect();
                found.sort();
                dirs.extend(found);
            }
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(Path::new(&home).join("Library/Spelling"));
        }
    }
    dirs
}

/// Installed dictionaries: searches `$DICPATH`, `~/.local/share/hunspell`,
/// `/usr/share/hunspell`, `/usr/share/myspell` and `/usr/share/myspell/dicts`;
/// on Windows also `dictionaries` next to the program, in `%APPDATA%\apexdesk`
/// and LibreOffice's; on macOS `~/Library/Spelling`.
/// Returns `(language code like "es_ES", aff path, dic path)`, deduplicated
/// by canonical path (distros symlink es_AR, es_MX... to es_ES; the entry
/// named like the real file wins) and by language (the folder searched
/// first wins: the organization's, then the user's), sorted by language code.
pub fn installed() -> Vec<(String, PathBuf, PathBuf)> {
    list_in(&search_dirs())
}

pub(crate) fn list_in(dirs: &[PathBuf]) -> Vec<(String, PathBuf, PathBuf)> {
    // canonical aff -> (lang, aff, dic, index of its folder)
    let mut found: BTreeMap<PathBuf, (String, PathBuf, PathBuf, usize)> = BTreeMap::new();
    for (i, dir) in dirs.iter().enumerate() {
        let Ok(rd) = apex_fs::read_dir(dir) else { continue };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for aff in entries {
            if aff.extension().and_then(|e| e.to_str()) != Some("aff") {
                continue;
            }
            let Some(lang) = aff.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            if lang.starts_with("hyph_") || lang.starts_with("th_") {
                continue;
            }
            let dic = aff.with_extension("dic");
            if !dic.fs_is_file() {
                continue;
            }
            let Ok(canon) = apex_fs::canonicalize(&aff) else { continue };
            let canon_lang = canon.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            match found.get(&canon) {
                // Keep the first one found unless this one carries the real name.
                Some((l, _, _, _)) if *l == canon_lang || lang != canon_lang => {}
                _ => {
                    found.insert(canon, (lang, aff, dic, i));
                }
            }
        }
    }
    let mut v: Vec<_> = found.into_values().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.3.cmp(&b.3)).then_with(|| a.1.cmp(&b.1)));
    v.dedup_by(|b, a| a.0 == b.0);
    v.into_iter().map(|(l, aff, dic, _)| (l, aff, dic)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedups_symlinks() {
        let dir = apex_fs::temp_dir().join(format!("apex-spell-installed-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&dir);
        apex_fs::create_dir_all(&dir).expect("mkdir");
        apex_fs::write(dir.join("es_ES.aff"), "SET UTF-8\n").expect("write");
        apex_fs::write(dir.join("es_ES.dic"), "1\nhola\n").expect("write");
        apex_fs::write(dir.join("fr_FR.aff"), "SET UTF-8\n").expect("write");
        apex_fs::write(dir.join("fr_FR.dic"), "1\nbonjour\n").expect("write");
        apex_fs::write(dir.join("xx.aff"), "SET UTF-8\n").expect("write"); // no .dic
        apex_fs::write(dir.join("hyph_es.dic"), "").expect("write");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("es_ES.aff"), dir.join("es_AR.aff")).expect("symlink");
            std::os::unix::fs::symlink(dir.join("es_ES.dic"), dir.join("es_AR.dic")).expect("symlink");
        }
        // The same directory twice must not duplicate entries either.
        let v = list_in(&[dir.clone(), dir.clone()]);
        let langs: Vec<&str> = v.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(langs, ["es_ES", "fr_FR"]);
        assert!(v[0].2.ends_with("es_ES.dic"));
        let _ = apex_fs::remove_dir_all(&dir);
    }

    // The same language in two folders is listed once: the one of the
    // folder searched first.
    #[test]
    fn one_per_language() {
        let root = apex_fs::temp_dir().join(format!("apex-spell-languages-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&root);
        for d in ["first", "second"] {
            apex_fs::create_dir_all(root.join(d)).expect("mkdir");
            apex_fs::write(root.join(d).join("es_ES.aff"), "SET UTF-8\n").expect("write");
            apex_fs::write(root.join(d).join("es_ES.dic"), "1\nhola\n").expect("write");
        }
        apex_fs::write(root.join("second").join("fr_FR.aff"), "SET UTF-8\n").expect("write");
        apex_fs::write(root.join("second").join("fr_FR.dic"), "1\nbonjour\n").expect("write");
        let v = list_in(&[root.join("first"), root.join("second")]);
        let langs: Vec<&str> = v.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(langs, ["es_ES", "fr_FR"]);
        assert!(v[0].1.starts_with(root.join("first")));
        let _ = apex_fs::remove_dir_all(&root);
    }
}
