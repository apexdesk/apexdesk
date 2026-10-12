//! Portable mode and the program folder. A file named `portable` (or
//! `portable.txt`) next to the program, or `APEXDESK_PORTABLE=1`, keeps
//! settings, templates, building blocks, recovery files and logs in a
//! `data` folder next to it: nothing goes to the user's profile and the
//! registry is only read (policies, the system proxy). The optional
//! components the installers add (FFmpeg, dictionaries) are looked
//! for in the program folder too.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the program is: the folder of the executable, or on macOS the
/// folder holding ApexDesk.app.
pub fn program_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    if cfg!(target_os = "macos")
        && let Some(app) = dir.ancestors().find(|a| a.extension().is_some_and(|e| e == "app"))
    {
        return app.parent().map(Path::to_path_buf);
    }
    Some(dir)
}

fn detect(dir: Option<&Path>, env: Option<&str>) -> bool {
    match env.map(str::trim) {
        Some("1" | "true" | "yes") => return true,
        Some("0" | "false" | "no") => return false,
        _ => {}
    }
    dir.is_some_and(|d| ["portable", "portable.txt"].iter().any(|m| d.join(m).is_file()))
}

/// Whether this copy runs portable.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if cfg!(test) {
            return false;
        }
        detect(program_dir().as_deref(), std::env::var("APEXDESK_PORTABLE").ok().as_deref())
    })
}

/// The data folder of a portable copy.
pub fn data_dir() -> Option<PathBuf> {
    enabled().then(program_dir).flatten().map(|d| d.join("data"))
}

/// Where the installers put the optional components: the program folder
/// (on macOS, inside ApexDesk.app: the folder holding it may be Downloads,
/// where anything can be) and, on macOS, /Library/Application Support/ApexDesk.
pub fn install_dirs() -> Vec<PathBuf> {
    let own = if cfg!(target_os = "macos") { std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.join("Resources"))) } else { program_dir() };
    let mut v: Vec<PathBuf> = own.into_iter().collect();
    if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/Application Support/ApexDesk"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers() {
        let dir = std::env::temp_dir().join(format!("apex-portable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!detect(Some(&dir), None));
        assert!(detect(Some(&dir), Some("1")));
        std::fs::write(dir.join("portable"), "").unwrap();
        assert!(detect(Some(&dir), None) && detect(Some(&dir), Some("")));
        assert!(!detect(Some(&dir), Some("0")), "the variable wins");
        assert!(!detect(None, None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
