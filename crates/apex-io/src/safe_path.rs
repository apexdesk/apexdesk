//! File names and folders for files a document hands us: an attachment's
//! or an embedded object's name is the document's to choose, so it must
//! never reach outside the folder we write it to.

use std::path::{Path, PathBuf};

/// Names Windows keeps for devices, with or without an extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
    "LPT8", "LPT9", "COM0", "LPT0", "CONIN$", "CONOUT$", "COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³",
];

/// `name` as a plain file name that is safe to create on any system: no
/// folders (only its last part is kept), no reserved or control
/// characters, no leading dots, no device names; `fallback` when nothing
/// is left.
pub fn safe_file_name(name: &str, fallback: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let n: String = last.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let n = n.trim().trim_start_matches('.').trim_end_matches([' ', '.']).to_string();
    if n.is_empty() {
        return fallback.to_string();
    }
    let stem = n.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) { format!("_{n}") } else { n }
}

/// Marks a copy taken out of a document as coming from elsewhere (the
/// Mark-of-the-Web: Windows' `Zone.Identifier` stream, zone 3), so the
/// program that opens it treats it as a download (Protected View,
/// SmartScreen, macros blocked); on macOS its quarantine attribute
/// (Gatekeeper asks first); elsewhere the origin attribute browsers set on
/// downloads (`user.xdg.origin.url`), which Protected View looks for.
pub fn mark_untrusted(path: &Path) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(p) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            let value = b"apexdesk:untrusted";
            // SAFETY: both strings are NUL-terminated and the value's length is its own.
            unsafe { libc::setxattr(p.as_ptr(), c"user.xdg.origin.url".as_ptr(), value.as_ptr().cast(), value.len(), 0) };
        }
    }
    #[cfg(windows)]
    {
        let mut ads = path.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let _ = std::fs::write(ads, "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(target_os = "macos")]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = std::process::Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", &format!("0081;{secs:x};ApexDesk;")])
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = path;
}

/// `dir` joined with `name` made safe: always a file directly inside `dir`.
pub fn join_safe(dir: &Path, name: &str, fallback: &str) -> PathBuf {
    dir.join(safe_file_name(name, fallback))
}

/// A folder of this process only (`<temp>/<prefix>-<pid>-<random>/<id>`),
/// readable by this user only, for copies of files handed to other programs.
pub fn private_temp_dir(prefix: &str, id: impl std::fmt::Display) -> std::io::Result<PathBuf> {
    let root = private_temp_root(prefix);
    make_private_dir(&root)?;
    // Someone else's folder (or a link) at our name: don't write into it.
    #[cfg(unix)]
    {
        let meta = std::fs::symlink_metadata(&root)?;
        if !meta.is_dir() || std::os::unix::fs::MetadataExt::uid(&meta) != current_uid() {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, root.display().to_string()));
        }
    }
    let dir = root.join(safe_file_name(&id.to_string(), "0"));
    make_private_dir(&dir)?;
    Ok(dir)
}

/// Creates `dir` (its parent must exist) readable by this user only from
/// the start; one already there is kept.
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    match b.create(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// A number nobody can guess, for names of temporary files and folders.
pub fn random_token() -> u64 {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        // No system randomness: the clock and the process, still unlikely to be guessed.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        return t ^ (u64::from(std::process::id()) << 32);
    }
    u64::from_le_bytes(b)
}

/// A folder of this user only (`<temp>/<prefix>-<uid>`), for sockets when
/// the system gives no runtime folder. An error when someone else made it
/// first (or put a link there): then it isn't used.
#[cfg(unix)]
pub fn user_temp_dir(prefix: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let uid = current_uid();
    let dir = std::env::temp_dir().join(format!("{prefix}-{uid}"));
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, dir.display().to_string()));
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// The root [`private_temp_dir`] makes its folders in, to remove at exit.
pub fn private_temp_root(prefix: &str) -> PathBuf {
    // Random for each run, so no one can make the folder before us.
    static TOKEN: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let token = TOKEN.get_or_init(random_token);
    std::env::temp_dir().join(format!("{prefix}-{}-{token:016x}", std::process::id()))
}

/// This process's user id, without libc: the owner of /proc/self (or of
/// the home folder where there is no /proc).
#[cfg(unix)]
pub(crate) fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or_else(|_| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        std::fs::metadata(home).map(|m| m.uid()).unwrap_or(u32::MAX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_never_leave_the_folder() {
        assert_eq!(safe_file_name("../../etc/passwd", "x"), "passwd");
        assert_eq!(safe_file_name("..\\..\\Start Menu\\Programs\\Startup\\a.bat", "x"), "a.bat");
        assert_eq!(safe_file_name("/abs/path.txt", "x"), "path.txt");
        assert_eq!(safe_file_name("C:\\Windows\\x.dll", "x"), "x.dll");
        assert_eq!(safe_file_name("C:x.txt", "x"), "C_x.txt");
        assert_eq!(safe_file_name("..", "x"), "x");
        assert_eq!(safe_file_name("", "x"), "x");
        assert_eq!(safe_file_name(".bashrc", "x"), "bashrc");
        assert_eq!(safe_file_name("a\nb.txt", "x"), "a_b.txt");
        assert_eq!(safe_file_name("con.txt", "x"), "_con.txt");
        assert_eq!(safe_file_name("report.pdf. . ", "x"), "report.pdf");
        assert_eq!(safe_file_name("Informe final.docx", "x"), "Informe final.docx");
        let d = Path::new("/tmp/d");
        assert_eq!(join_safe(d, "/etc/x", "f").parent(), Some(d));
    }

    #[cfg(unix)]
    #[test]
    fn user_folder_is_ours_or_refused() {
        use std::os::unix::fs::PermissionsExt;
        let d = user_temp_dir("apexdesk-user-dir-test").unwrap();
        assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
        // Asked again, the same folder.
        assert_eq!(user_temp_dir("apexdesk-user-dir-test").unwrap(), d);
        let _ = std::fs::remove_dir_all(&d);
        // A link planted at the name is refused.
        let target = std::env::temp_dir().join(format!("apexdesk-user-dir-target-{}", std::process::id()));
        std::fs::create_dir_all(&target).unwrap();
        let link = std::env::temp_dir().join(format!("apexdesk-user-link-test-{}", current_uid()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(user_temp_dir("apexdesk-user-link-test").is_err());
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn private_folder_is_ours() {
        let d = private_temp_dir("apexdesk-safe-path-test", "../7").unwrap();
        assert!(d.starts_with(private_temp_root("apexdesk-safe-path-test")));
        assert_eq!(d.file_name().unwrap(), "7");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&d).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        let _ = std::fs::remove_dir_all(private_temp_root("apexdesk-safe-path-test"));
    }
}

/// A program of Windows by its place in the Windows folder
/// (`System32\curl.exe`): never one of the same name in the program's
/// folder (a portable copy in Downloads) or in the PATH.
pub fn windows_program(rel: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join(rel)
}
