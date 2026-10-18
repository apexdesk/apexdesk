//! Wildcards in file arguments. Unix shells expand them before the program
//! starts; the Windows console does not, so `*` and `?` in the file name
//! part are matched here (a pattern the shell left alone matched nothing).

use apex_fs::PathExt;
use std::path::{Path, PathBuf};

/// The files `arg` names: itself, or the sorted matches of its pattern.
pub fn expand(arg: &str) -> Result<Vec<PathBuf>, String> {
    let path = Path::new(arg);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !name.contains(['*', '?']) || path.fs_exists() {
        return Ok(vec![path.to_path_buf()]);
    }
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut out: Vec<PathBuf> = apex_fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.file_name().to_str().is_some_and(|n| matches(name, n)))
        .map(|e| if path.parent().is_some_and(|p| !p.as_os_str().is_empty()) { dir.join(e.file_name()) } else { PathBuf::from(e.file_name()) })
        .collect();
    if out.is_empty() {
        return Err(format!("no file matches '{arg}'"));
    }
    out.sort();
    Ok(out)
}

/// Whether `name` matches `pat` (`*` any run, `?` one character). Case is
/// ignored on Windows and macOS, whose file systems ignore it.
pub fn matches(pat: &str, name: &str) -> bool {
    let fold = |s: &str| -> Vec<char> {
        if cfg!(any(windows, target_os = "macos")) { s.to_lowercase().chars().collect() } else { s.chars().collect() }
    };
    let (p, n) = (fold(pat), fold(name));
    // Iterative matching with backtracking to the last '*'.
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some((i, j));
            i += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn wildcards() {
        assert!(matches("*.xlsx", "a.xlsx"));
        assert!(matches("*.xlsx", ".xlsx"));
        assert!(!matches("*.xlsx", "a.xlsx.bak"));
        assert!(matches("r?port-*.docx", "report-2024.docx"));
        assert!(!matches("r?port.docx", "rport.docx"));
        assert!(matches("*a*b*", "xxaxxbxx"));
        assert!(!matches("*a*b", "xxaxxbxxc"));
    }
}
