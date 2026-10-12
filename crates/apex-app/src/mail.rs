//! Sending files by e-mail: a new message of the system's mail program
//! with the files attached, left open for the user to check and send.
//!
//! - Windows: Simple MAPI (`MAPISendMailW`), which Outlook (classic),
//!   Thunderbird, eM Client and others provide.
//! - macOS: the default mail program (the `mailto` handler) opens the
//!   files, which Mail and Outlook take as a new message.
//! - Linux and other Unix: `xdg-email --attach`.
//!
//! With no program that takes attachments (web mail, the new Outlook) the
//! caller shows the file in its folder instead ([`reveal`]).
//!
//! Also here: `.eml` messages for mail merge ([`eml`]), which mail
//! programs open as drafts.

use std::path::{Path, PathBuf};

/// Opens a new message with `files` attached. Blocks until the program
/// has taken them (on Windows, until its window closes): call it off the
/// UI thread. An error means no program took them.
pub fn compose(files: &[PathBuf], subject: &str) -> Result<(), String> {
    // Tests start no program; they count the messages asked for.
    if cfg!(test) {
        #[cfg(test)]
        test::COMPOSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Ok(());
    }
    platform::compose(files, subject)
}

#[cfg(test)]
pub mod test {
    /// Messages composed since the count was last taken.
    pub static COMPOSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
}

/// Shows `path` selected in its folder (or opens the folder).
pub fn reveal(path: &Path) {
    if cfg!(test) {
        return;
    }
    platform::reveal(path)
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    #[repr(C)]
    struct MapiFileDescW {
        reserved: u32,
        flags: u32,
        position: u32,
        path_name: *const u16,
        file_name: *const u16,
        file_type: *mut c_void,
    }

    #[repr(C)]
    struct MapiMessageW {
        reserved: u32,
        subject: *const u16,
        note_text: *const u16,
        message_type: *const u16,
        date_received: *const u16,
        conversation_id: *const u16,
        flags: u32,
        originator: *mut c_void,
        recip_count: u32,
        recips: *mut c_void,
        file_count: u32,
        files: *mut MapiFileDescW,
    }

    type SendMailW = unsafe extern "system" fn(session: usize, ui_param: usize, message: *mut MapiMessageW, flags: u32, reserved: u32) -> u32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    const MAPI_LOGON_UI: u32 = 0x1;
    const MAPI_DIALOG: u32 = 0x8;
    const MAPI_USER_ABORT: u32 = 1;
    /// Only System32: a MAPI32.DLL left beside a portable copy (in
    /// Downloads, say) isn't loaded.
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn compose(files: &[PathBuf], subject: &str) -> Result<(), String> {
        let dll = wide(std::ffi::OsStr::new("MAPI32.DLL"));
        // SAFETY: plain Win32 calls with NUL-terminated strings; the
        // function pointer has MAPISendMailW's documented signature.
        unsafe {
            let module = LoadLibraryExW(dll.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if module.is_null() {
                return Err("no MAPI".into());
            }
            let f = GetProcAddress(module, c"MAPISendMailW".as_ptr().cast());
            if f.is_null() {
                return Err("no MAPISendMailW".into());
            }
            let send: SendMailW = std::mem::transmute(f);
            let paths: Vec<Vec<u16>> = files.iter().map(|p| wide(p.as_os_str())).collect();
            let names: Vec<Vec<u16>> = files.iter().map(|p| wide(p.file_name().unwrap_or_default())).collect();
            let mut descs: Vec<MapiFileDescW> = paths
                .iter()
                .zip(&names)
                .map(|(p, n)| MapiFileDescW { reserved: 0, flags: 0, position: u32::MAX, path_name: p.as_ptr(), file_name: n.as_ptr(), file_type: std::ptr::null_mut() })
                .collect();
            let subject = wide(std::ffi::OsStr::new(subject));
            let mut msg = MapiMessageW {
                reserved: 0,
                subject: subject.as_ptr(),
                note_text: std::ptr::null(),
                message_type: std::ptr::null(),
                date_received: std::ptr::null(),
                conversation_id: std::ptr::null(),
                flags: 0,
                originator: std::ptr::null_mut(),
                recip_count: 0,
                recips: std::ptr::null_mut(),
                file_count: descs.len() as u32,
                files: descs.as_mut_ptr(),
            };
            match send(0, 0, &mut msg, MAPI_LOGON_UI | MAPI_DIALOG, 0) {
                0 | MAPI_USER_ABORT => Ok(()),
                e => Err(format!("MAPI error {e}")),
            }
        }
    }

    pub fn reveal(path: &Path) {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        let _ = std::process::Command::new(apex_io::safe_path::windows_program("explorer.exe")).arg(arg).spawn();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// The bundle id of the default mail program.
    fn mail_app() -> Option<String> {
        let js = "ObjC.import('CoreServices'); ObjC.unwrap($.LSCopyDefaultHandlerForURLScheme($('mailto')))";
        let out = Command::new("osascript").args(["-l", "JavaScript", "-e", js]).output().ok()?;
        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !id.is_empty() && id != "undefined").then_some(id)
    }

    pub fn compose(files: &[PathBuf], _subject: &str) -> Result<(), String> {
        let id = mail_app().unwrap_or_else(|| "com.apple.mail".into());
        // A browser handles mailto for web mail: it can't take files.
        let lower = id.to_lowercase();
        if ["safari", "chrome", "firefox", "edge", "opera", "brave", "vivaldi"].iter().any(|b| lower.contains(b)) {
            return Err(format!("{id} takes no attachments"));
        }
        let ok = Command::new("open").arg("-b").arg(&id).args(files).status().map_err(|e| e.to_string())?.success();
        if ok { Ok(()) } else { Err(format!("{id} did not open the files")) }
    }

    pub fn reveal(path: &Path) {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn compose(files: &[PathBuf], subject: &str) -> Result<(), String> {
        let mut c = Command::new("xdg-email");
        // The subject goes into the same option (a received file's name).
        let subject: String = subject.chars().map(|c| if c.is_control() || "'\\,\"".contains(c) { ' ' } else { c }).collect();
        c.arg("--utf8").arg("--subject").arg(subject);
        for f in files {
            c.arg("--attach").arg(plain_path(f)?);
        }
        let ok = c.status().map_err(|e| e.to_string())?.success();
        if ok { Ok(()) } else { Err("xdg-email failed".into()) }
    }

    /// `f`, or a copy of it under a plain name: xdg-email hands paths to
    /// some mail programs (Thunderbird) inside a quoted, comma-separated
    /// option, where a quote, comma or backslash in a received file's name
    /// would add attachments or recipients of its choosing.
    fn plain_path(f: &Path) -> Result<PathBuf, String> {
        let plain = |s: &str| s.chars().all(|c| c.is_alphanumeric() || " ._-/()+".contains(c));
        if f.to_str().is_some_and(plain) {
            return Ok(f.to_path_buf());
        }
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name: String = name.chars().map(|c| if c.is_alphanumeric() || "._-()+ ".contains(c) { c } else { '_' }).collect();
        let dir = apex_io::safe_path::private_temp_dir("apexdesk-mail", std::process::id()).map_err(|e| e.to_string())?;
        let to = dir.join(if name.trim_matches(['.', ' ']).is_empty() { "file".to_string() } else { name });
        std::fs::copy(f, &to).map_err(|e| e.to_string())?;
        Ok(to)
    }

    pub fn reveal(path: &Path) {
        // The file manager's D-Bus interface selects the file; else the folder opens.
        // A comma would split dbus-send's array.
        let uri = format!("file://{}", path.display()).replace('%', "%25").replace(',', "%2C");
        let shown = Command::new("dbus-send")
            .args(["--session", "--dest=org.freedesktop.FileManager1", "--type=method_call", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"])
            .arg(format!("array:string:{uri}"))
            .arg("string:")
            .status()
            .is_ok_and(|s| s.success());
        if !shown {
            if let Some(dir) = path.parent() {
                let _ = Command::new("xdg-open").arg(dir).spawn();
            }
        }
    }
}

// --- .eml messages ---------------------------------------------------------------------

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Base64 in lines of 76 characters, as MIME wants.
fn base64_lines(data: &[u8]) -> String {
    let b = base64(data);
    b.as_bytes().chunks(76).map(|l| std::str::from_utf8(l).unwrap_or_default()).collect::<Vec<_>>().join("\r\n")
}

/// A header value: as is when plain ASCII, else encoded (RFC 2047).
fn header_text(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if s.is_ascii() { s } else { format!("=?UTF-8?B?{}?=", base64(s.as_bytes())) }
}

/// A message to `to` with an HTML body and its plain text, marked as
/// not sent so that mail programs open it as a draft.
pub fn eml(to: &str, subject: &str, html: &str, text: &str) -> String {
    let boundary = "=_apexdesk_alternative";
    let mut m = String::new();
    m.push_str(&format!("To: {}\r\n", header_text(to.trim())));
    m.push_str(&format!("Subject: {}\r\n", header_text(subject)));
    m.push_str("X-Unsent: 1\r\nMIME-Version: 1.0\r\n");
    m.push_str(&format!("Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n"));
    for (kind, body) in [("plain", text), ("html", html)] {
        m.push_str(&format!("--{boundary}\r\nContent-Type: text/{kind}; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"));
        m.push_str(&base64_lines(body.as_bytes()));
        m.push_str("\r\n");
    }
    m.push_str(&format!("--{boundary}--\r\n"));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message() {
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man!"), "TWFuIQ==");
        let m = eml("ana@example.org", "Cita médica", "<p>Hola</p>", "Hola");
        assert!(m.starts_with("To: ana@example.org\r\nSubject: =?UTF-8?B?Q2l0YSBtw6lkaWNh?=\r\nX-Unsent: 1\r\n"), "{m}");
        assert!(m.contains("text/html") && m.contains(&base64(b"<p>Hola</p>")) && m.ends_with("--\r\n"));
    }
}
