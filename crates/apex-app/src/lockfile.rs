//! Lock files that tell others a document is open, in the formats of
//! LibreOffice and Microsoft Office, so that each program sees the others.
//!
//! LibreOffice writes `.~lock.NAME#` next to the document: one line of
//! comma separated fields (`,`, `;` and `\` escaped with `\`) ended by `;`:
//! the user's name, the login name, the computer, the time
//! ("26.09.2026 14:03") and the URL of the program's profile. For Microsoft
//! Office formats it also writes Office's own `~$NAME` (Word drops the first
//! one or two letters of long names): 162 bytes for Word, 165 for Excel,
//! with the user's name twice, first as a length byte and 8-bit characters
//! and then, at byte 54 (Word) or 55 (Excel), as a 16-bit length and UTF-16
//! (see LibreOffice's `svl/source/misc/msodocumentlockfile.cxx`).
//!
//! ApexDesk writes both too, with the profile URL ending in `#pid=N` so
//! that a lock left by an ApexDesk that died is recognised and replaced.

use apex_fs::PathExt;
use apex_fs::File;
use std::path::{Path, PathBuf};
use web_time::SystemTime;

/// Longest user name Office stores.
const MSO_NAME_MAX: usize = 52;
const MSO_WORD_SIZE: usize = 162;
const MSO_EXCEL_SIZE: usize = 165;

/// Who holds a lock (or who we are).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    /// The name the user gave (may be empty in LibreOffice's locks).
    pub user: String,
    /// Login name.
    pub sys_user: String,
    pub host: String,
    /// "26.09.2026 14:03" (LibreOffice's format, local time).
    pub time: String,
    /// The program's profile URL.
    pub url: String,
}

impl Owner {
    /// The name to show: the user's name, else the login name.
    pub fn display_name(&self) -> &str {
        if self.user.trim().is_empty() { &self.sys_user } else { &self.user }
    }

    /// The process id of an ApexDesk lock.
    fn pid(&self) -> Option<u32> {
        self.url.rsplit_once("#pid=").and_then(|(_, p)| p.parse().ok())
    }

    /// The profile URL without the process id.
    fn profile(&self) -> &str {
        self.url.rsplit_once("#pid=").map_or(&self.url, |(u, _)| u)
    }
}

/// A lock found next to a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub owner: Owner,
    /// When the lock was written.
    pub since: Option<SystemTime>,
    /// Only Office's lock (no LibreOffice lock with it).
    pub office: bool,
}

/// What stands in the way of opening a document for editing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Free,
    /// Left by an ApexDesk of this user and computer that isn't running.
    Orphan,
    /// Held by this process.
    Mine,
    Other(Found),
}

// --- names ----------------------------------------------------------------------------

fn ext_of(doc: &Path) -> String {
    doc.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_word(ext: &str) -> bool {
    matches!(ext, "doc" | "dot" | "docx" | "dotx" | "docm" | "dotm" | "rtf")
}

fn is_excel(ext: &str) -> bool {
    matches!(ext, "xls" | "xlt" | "xlsx" | "xltx" | "xlsm" | "xltm" | "xlsb")
}

/// LibreOffice's lock: `.~lock.NAME#`.
pub fn lo_path(doc: &Path) -> Option<PathBuf> {
    let name = doc.file_name()?.to_string_lossy();
    Some(doc.with_file_name(format!(".~lock.{name}#")))
}

/// Office's lock (`~$NAME`), for Word and Excel formats only.
pub fn mso_path(doc: &Path) -> Option<PathBuf> {
    let name = doc.file_name()?.to_string_lossy().into_owned();
    let ext = ext_of(doc);
    let name = if is_word(&ext) {
        // Word shortens names of 7 letters or more (before the extension).
        let stem = name.chars().count() - ext.chars().count() - 1;
        let skip = match stem {
            8.. => 2,
            7 => 1,
            _ => 0,
        };
        name.chars().skip(skip).collect()
    } else if is_excel(&ext) {
        name
    } else {
        return None;
    };
    Some(doc.with_file_name(format!("~${name}")))
}

/// Whether `path` is a lock file (never locked or offered itself).
pub fn is_lock_name(path: &Path) -> bool {
    path.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n.starts_with("~$") || n.starts_with(".~lock.")
    })
}

// --- LibreOffice format -----------------------------------------------------------------

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, ',' | ';' | '\\') {
            out.push('\\');
        }
        // A lock is one line.
        out.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    out
}

/// The text of a LibreOffice lock.
pub fn encode_lo(o: &Owner) -> String {
    format!("{},{},{},{},{};", escape(&o.user), escape(&o.sys_user), escape(&o.host), escape(&o.time), escape(&o.url))
}

/// Reads a LibreOffice lock (its first entry).
pub fn parse_lo(text: &str) -> Option<Owner> {
    let mut fields = vec![String::new()];
    let mut chars = text.chars();
    let mut ended = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => fields.last_mut()?.extend(chars.next()),
            ',' => fields.push(String::new()),
            ';' => {
                ended = true;
                break;
            }
            _ => fields.last_mut()?.push(c),
        }
    }
    if !ended && fields.len() < 5 {
        return None;
    }
    let f = |i: usize| fields.get(i).cloned().unwrap_or_default();
    let o = Owner { user: f(0), sys_user: f(1), host: f(2), time: f(3), url: f(4).trim_end().to_string() };
    (!o.user.is_empty() || !o.sys_user.is_empty()).then_some(o)
}

/// LibreOffice's time format for now, in local time.
pub fn now_text() -> String {
    let (y, m, d, h, mi) = crate::common::local_now();
    format!("{d:02}.{m:02}.{y} {h:02}:{mi:02}")
}

// --- Office format ----------------------------------------------------------------------

/// The bytes of an Office lock for the user `name`.
pub fn encode_mso(name: &str, word: bool) -> Vec<u8> {
    let chars: Vec<char> = name.chars().take(MSO_NAME_MAX).collect();
    let n = chars.len();
    let size = if word { MSO_WORD_SIZE } else { MSO_EXCEL_SIZE };
    let mut b = Vec::with_capacity(size);
    b.push(n as u8);
    // 8-bit copy: Latin-1 where it fits.
    b.extend(chars.iter().map(|&c| if (c as u32) < 256 { c as u8 } else { b'?' }));
    let (pad, until) = if word { (0u8, MSO_NAME_MAX + 2) } else { (0x20u8, MSO_NAME_MAX + 3) };
    b.resize(until, pad);
    b.extend([n as u8, 0]);
    let units: Vec<u16> = chars.iter().collect::<String>().encode_utf16().take(MSO_NAME_MAX).collect();
    // The length counts characters; names outside the BMP are rare enough.
    b[until] = units.len() as u8;
    for u in units {
        b.extend(u.to_le_bytes());
    }
    if word {
        b.resize(size, 0);
    } else {
        while b.len() < size {
            b.push(if (b.len() - until) % 2 == 0 { 0x20 } else { 0 });
        }
    }
    b.truncate(size);
    b
}

/// The user name in an Office lock (Word's or Excel's layout).
pub fn parse_mso(b: &[u8]) -> Option<String> {
    if b.len() < MSO_WORD_SIZE {
        return None;
    }
    let acp = b[0] as usize;
    if acp == 0 || acp > MSO_NAME_MAX {
        return None;
    }
    let mut at = MSO_NAME_MAX + 2;
    let mut n = b[at] as usize;
    // A length of 0x20 here is Excel's padding when the bytes before it
    // aren't Word's zeros: the length is one byte further.
    if n == 0x20 && (b[at - 1] != 0 || b[at - 2] != 0) {
        at += 1;
        n = b[at] as usize;
    }
    if n == 0 || n > MSO_NAME_MAX || b.len() < at + 2 + n * 2 {
        return None;
    }
    let units: Vec<u16> = b[at + 2..at + 2 + n * 2].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(&units))
}

// --- who we are ---------------------------------------------------------------------------

pub fn login_name() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).or_else(|_| std::env::var("LOGNAME")).unwrap_or_default()
}

#[cfg(unix)]
pub fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    if ok { String::from_utf8_lossy(&buf[..end]).into_owned() } else { String::new() }
}

#[cfg(not(unix))]
pub fn host_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// A `file://` URL of a local path.
fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if s.starts_with('/') { "file://" } else { "file:///" });
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// This process as a lock owner, named `user`.
pub fn me(user: &str) -> Owner {
    let profile = crate::settings::config_dir().map(|d| file_url(&d)).unwrap_or_default();
    Owner { user: user.to_string(), sys_user: login_name(), host: host_name(), time: now_text(), url: format!("{profile}#pid={}", apex_fs::process_id()) }
}

/// Whether process `pid` is running.
#[cfg(unix)]
fn running(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        // A reused id belongs to some other program.
        return apex_fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| std::iter::once("apexdesk").chain(crate::settings::OLD_NAMES).any(|n| c.trim().starts_with(n)));
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn running(pid: u32) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn GetExitCodeProcess(process: isize, code: *mut u32) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: the handle is checked and closed; `code` is a valid u32.
    unsafe {
        let h = OpenProcess(QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE
    }
}

#[cfg(not(any(unix, windows)))]
fn running(_pid: u32) -> bool {
    true
}

// --- finding locks --------------------------------------------------------------------------

/// The content of a lock file: small regular files only (a lock planted
/// next to a document could be a pipe, or a link to an endless device).
fn read_lock(p: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    // Looked at before opening: opening a pipe waits for a writer.
    if !apex_fs::metadata(p)?.is_file() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let f = apex_fs::File::open(p)?;
    let mut out = Vec::new();
    f.take(64 << 10).read_to_end(&mut out)?;
    Ok(out)
}

/// Reads the locks of `doc`.
pub fn read(doc: &Path) -> Option<Found> {
    if let Some(p) = lo_path(doc)
        && let Ok(text) = read_lock(&p)
        && let Some(owner) = parse_lo(&String::from_utf8_lossy(&text))
    {
        let since = apex_fs::metadata(&p).and_then(|m| m.modified()).ok();
        return Some(Found { owner, since, office: false });
    }
    let p = mso_path(doc)?;
    let bytes = read_lock(&p).ok()?;
    let since = apex_fs::metadata(&p).and_then(|m| m.modified()).ok();
    // Unreadable Office locks still mean the file is open.
    let user = parse_mso(&bytes).unwrap_or_default();
    Some(Found { owner: Owner { user, ..Default::default() }, since, office: true })
}

/// Whether `doc` is locked, and by whom, as seen by `me`.
pub fn check(doc: &Path, me: &Owner) -> State {
    let Some(f) = read(doc) else { return State::Free };
    // Office holds its lock open while the file is: one nobody holds was
    // left behind (Office closed unexpectedly), and Office replaces it.
    if f.office && mso_path(doc).is_some_and(|p| !held_open(&p)) {
        return State::Orphan;
    }
    let o = &f.owner;
    let ours = !f.office && o.sys_user == me.sys_user && o.host == me.host && o.profile() == me.profile();
    match o.pid() {
        Some(pid) if ours && pid == apex_fs::process_id() => State::Mine,
        Some(pid) if ours && !running(pid) => State::Orphan,
        _ => State::Other(f),
    }
}

/// Whether some program has `p` open (Windows: it can't be opened
/// without sharing it). Elsewhere locks are not held open: always true.
#[cfg(windows)]
fn held_open(p: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    let mut o = apex_fs::OpenOptions::new();
    o.read(true).share_mode(0);
    match o.open(p) {
        Ok(_) => false,
        // ERROR_SHARING_VIOLATION; anything else (no access) says nothing.
        Err(e) => e.raw_os_error() == Some(32) || e.kind() != std::io::ErrorKind::NotFound,
    }
}

#[cfg(not(windows))]
fn held_open(_p: &Path) -> bool {
    true
}

/// Whether a lock was written by `me`'s own account on this computer (or,
/// for Office's locks, which only keep a name, under `me`'s name): most
/// likely left behind by a program that closed unexpectedly.
pub fn left_by_me(f: &Found, me: &Owner) -> bool {
    let o = &f.owner;
    if f.office {
        let name = o.user.trim();
        !name.is_empty() && (name.eq_ignore_ascii_case(me.user.trim()) || name.eq_ignore_ascii_case(&me.sys_user))
    } else {
        !o.sys_user.is_empty() && o.sys_user.eq_ignore_ascii_case(&me.sys_user) && o.host.eq_ignore_ascii_case(&me.host)
    }
}

/// Removes the locks of `doc`, whoever wrote them (the user said they
/// were left behind).
pub fn remove(doc: &Path) {
    remove_all(doc);
}

// --- holding a lock -----------------------------------------------------------------------

/// The locks this process wrote for a document; removed when dropped.
#[derive(Debug)]
pub struct Lock {
    doc: PathBuf,
    url: String,
    /// Kept open: on Windows, others can read them but not delete them.
    files: Vec<(PathBuf, File)>,
}

fn create(path: &Path, data: &[u8]) -> std::io::Result<File> {
    use std::io::Write;
    let mut o = apex_fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Shared for reading and writing, hidden like Office's.
        o.share_mode(1 | 2).attributes(2);
    }
    let mut f = o.open(path)?;
    f.write_all(data)?;
    f.flush()?;
    Ok(f)
}

/// Writes the locks of `doc` for `me`, replacing an orphan of ours. Fails
/// when the folder can't be written, or when someone else holds it.
pub fn acquire(doc: &Path, me: &Owner) -> std::io::Result<Lock> {
    use std::io::{Error, ErrorKind};
    match check(doc, me) {
        State::Other(_) => return Err(Error::new(ErrorKind::AlreadyExists, "locked")),
        State::Orphan | State::Mine => remove_all(doc),
        State::Free => {}
    }
    let mut lock = Lock { doc: doc.to_path_buf(), url: me.url.clone(), files: Vec::new() };
    let lo = lo_path(doc).ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
    lock.files.push((lo.clone(), create(&lo, encode_lo(me).as_bytes())?));
    if let Some(p) = mso_path(doc) {
        // Office's lock is extra: a failure there keeps LibreOffice's.
        let word = is_word(&ext_of(doc));
        if let Ok(f) = create(&p, &encode_mso(me.display_name(), word)) {
            lock.files.push((p, f));
        }
    }
    Ok(lock)
}

fn remove_all(doc: &Path) {
    for p in [lo_path(doc), mso_path(doc)].into_iter().flatten() {
        let _ = apex_fs::remove_file(p);
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let files = std::mem::take(&mut self.files);
        let paths: Vec<PathBuf> = files.into_iter().map(|(p, _)| p).collect();
        // Removed only while they are still ours (someone may have opened
        // the file anyway and written their own).
        let still = lo_path(&self.doc).and_then(|p| read_lock(&p).ok()).and_then(|t| parse_lo(&String::from_utf8_lossy(&t))).is_some_and(|o| o.url == self.url);
        if still {
            for p in paths {
                let _ = apex_fs::remove_file(p);
            }
        }
    }
}

/// Whether ApexDesk should lock `doc`: not the recovery copies, temporary
/// files or lock files.
pub fn lockable(doc: &Path) -> bool {
    if is_lock_name(doc) {
        return false;
    }
    let under = |dir: Option<PathBuf>| dir.and_then(|d| d.fs_canonicalize().ok()).is_some_and(|d| doc.starts_with(d));
    !under(crate::settings::config_dir().map(|d| d.join("recovery"))) && !under(Some(apex_fs::temp_dir()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = apex_fs::temp_dir().join(format!("apexdesk-lock-{name}-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&d);
        apex_fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names() {
        let p = Path::new("/x/Informe anual.docx");
        assert_eq!(lo_path(p).unwrap(), Path::new("/x/.~lock.Informe anual.docx#"));
        // Word drops two letters of long names, one of 7-letter names.
        assert_eq!(mso_path(p).unwrap(), Path::new("/x/~$forme anual.docx"));
        assert_eq!(mso_path(Path::new("/x/Informe.doc")).unwrap(), Path::new("/x/~$nforme.doc"));
        assert_eq!(mso_path(Path::new("/x/Carta01.docx")).unwrap(), Path::new("/x/~$arta01.docx"));
        assert_eq!(mso_path(Path::new("/x/Carta.docx")).unwrap(), Path::new("/x/~$Carta.docx"));
        // Excel keeps the name; formats Office doesn't open get none.
        assert_eq!(mso_path(Path::new("/x/Presupuesto 2026.xlsx")).unwrap(), Path::new("/x/~$Presupuesto 2026.xlsx"));
        assert_eq!(mso_path(Path::new("/x/notas.odt")), None);
        assert!(is_lock_name(Path::new("/x/~$forme.docx")) && is_lock_name(Path::new("/x/.~lock.a.odt#")));
    }

    #[test]
    fn libreoffice_lines() {
        // As written by LibreOffice 7 on Linux (no name in its options).
        let real = ",ana,portatil,26.09.2026 09:41,file:///home/ana/.config/libreoffice/4;";
        let o = parse_lo(real).unwrap();
        assert_eq!((o.user.as_str(), o.sys_user.as_str(), o.host.as_str()), ("", "ana", "portatil"));
        assert_eq!(o.time, "26.09.2026 09:41");
        assert_eq!(o.url, "file:///home/ana/.config/libreoffice/4");
        assert_eq!(o.display_name(), "ana");
        // And on Windows, with a name.
        let o = parse_lo("Ana García,ana,PC-OFICINA,03.02.2025 17:05,file:///C:/Users/ana/AppData/Roaming/LibreOffice/4;").unwrap();
        assert_eq!(o.display_name(), "Ana García");
        assert_eq!(o.url, "file:///C:/Users/ana/AppData/Roaming/LibreOffice/4");
        // Separators in fields are escaped.
        let odd = Owner { user: "Pérez, Juan; \\jr".into(), sys_user: "juan".into(), host: "h".into(), time: now_text(), url: "file:///p#pid=7".into() };
        let text = encode_lo(&odd);
        assert!(text.starts_with("Pérez\\, Juan\\; \\\\jr,juan,h,") && text.ends_with(";"));
        assert_eq!(parse_lo(&text).unwrap(), odd);
        assert_eq!(odd.pid(), Some(7));
        assert!(parse_lo("").is_none() && parse_lo("garbage").is_none());
    }

    #[test]
    fn office_bytes() {
        // Word's layout: length, 8-bit name, zeros up to 54, then the
        // length and the name in UTF-16, zeros up to 162 bytes.
        let w = encode_mso("Ana", true);
        assert_eq!(w.len(), 162);
        assert_eq!(&w[..4], &[3, b'A', b'n', b'a']);
        assert!(w[4..54].iter().all(|&b| b == 0));
        assert_eq!(&w[54..62], &[3, 0, b'A', 0, b'n', 0, b'a', 0]);
        assert!(w[62..].iter().all(|&b| b == 0));
        assert_eq!(parse_mso(&w).as_deref(), Some("Ana"));
        // Excel's: spaces up to 55, the UTF-16 copy one byte later.
        let x = encode_mso("José", false);
        assert_eq!(x.len(), 165);
        assert_eq!(&x[..5], &[4, b'J', b'o', b's', 0xE9]);
        assert!(x[5..55].iter().all(|&b| b == 0x20));
        assert_eq!(&x[55..65], &[4, 0, b'J', 0, b'o', 0, b's', 0, 0xE9, 0]);
        assert_eq!(parse_mso(&x).as_deref(), Some("José"));
        // A 32-letter name puts 0x20 where Word's length goes.
        let long = "abcdefghijklmnopqrstuvwxyzABCDEF";
        assert_eq!(parse_mso(&encode_mso(long, true)).as_deref(), Some(long));
        assert_eq!(parse_mso(&encode_mso(long, false)).as_deref(), Some(long));
        // A Word lock as Word 2016 writes it (user "Usuario").
        let mut real = vec![7u8];
        real.extend(b"Usuario");
        real.resize(54, 0);
        real.extend([7, 0]);
        for c in "Usuario".encode_utf16() {
            real.extend(c.to_le_bytes());
        }
        real.resize(162, 0);
        assert_eq!(real, encode_mso("Usuario", true));
        assert_eq!(parse_mso(&real).as_deref(), Some("Usuario"));
        assert!(parse_mso(&[0u8; 162]).is_none() && parse_mso(&w[..100]).is_none());
    }

    #[test]
    fn locks_of_others_and_orphans() {
        let d = dir("state");
        let doc = d.join("Informe anual.docx");
        apex_fs::write(&doc, b"x").unwrap();
        let me = me("Yo");
        assert_eq!(check(&doc, &me), State::Free);
        // Our lock: both files, recognised as this process's.
        let lock = acquire(&doc, &me).unwrap();
        let lo = apex_fs::read_to_string(lo_path(&doc).unwrap()).unwrap();
        assert!(lo.starts_with(&format!("Yo,{},", login_name())) && lo.ends_with(&format!("#pid={};", apex_fs::process_id())));
        assert_eq!(parse_mso(&apex_fs::read(mso_path(&doc).unwrap()).unwrap()).as_deref(), Some("Yo"));
        assert_eq!(check(&doc, &me), State::Mine);
        drop(lock);
        assert!(!lo_path(&doc).unwrap().fs_exists() && !mso_path(&doc).unwrap().fs_exists());
        // Left by an ApexDesk of ours that died: replaced without asking.
        let dead = Owner { url: format!("{}#pid={}", me.profile(), u32::MAX - 3), ..me.clone() };
        apex_fs::write(lo_path(&doc).unwrap(), encode_lo(&dead)).unwrap();
        apex_fs::write(mso_path(&doc).unwrap(), encode_mso("Yo", true)).unwrap();
        assert_eq!(check(&doc, &me), State::Orphan);
        let lock = acquire(&doc, &me).unwrap();
        assert_eq!(check(&doc, &me), State::Mine);
        drop(lock);
        // LibreOffice on another computer.
        let lo_line = "Ana García,ana,PC-OFICINA,03.02.2025 17:05,file:///C:/Users/ana/AppData/Roaming/LibreOffice/4;";
        apex_fs::write(lo_path(&doc).unwrap(), lo_line).unwrap();
        match check(&doc, &me) {
            State::Other(f) => {
                assert_eq!(f.owner.display_name(), "Ana García");
                assert!(!f.office && f.since.is_some());
            }
            s => panic!("{s:?}"),
        }
        assert!(acquire(&doc, &me).is_err());
        // Theirs stays when ours would go.
        assert!(lo_path(&doc).unwrap().fs_exists());
        apex_fs::remove_file(lo_path(&doc).unwrap()).unwrap();
        // Word alone.
        apex_fs::write(mso_path(&doc).unwrap(), encode_mso("Luis", true)).unwrap();
        match check(&doc, &me) {
            State::Other(f) => assert!(f.office && f.owner.display_name() == "Luis"),
            s => panic!("{s:?}"),
        }
        // A live ApexDesk of this user (another process) is someone else.
        apex_fs::remove_file(mso_path(&doc).unwrap()).unwrap();
        let other = Owner { url: format!("{}#pid=1", me.profile()), ..me.clone() };
        apex_fs::write(lo_path(&doc).unwrap(), encode_lo(&other)).unwrap();
        if cfg!(not(target_os = "linux")) {
            assert!(matches!(check(&doc, &me), State::Other(_)));
        }
        let _ = apex_fs::remove_dir_all(&d);
    }

    #[test]
    fn not_locked() {
        assert!(!lockable(Path::new("/x/~$forme.docx")));
        assert!(!lockable(&apex_fs::temp_dir().fs_canonicalize().unwrap().join("a.docx")));
    }
}
