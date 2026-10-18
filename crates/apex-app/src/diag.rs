//! Diagnostics kept on this computer: a log of errors, rotated when it
//! grows (`apexdesk.log`, `.1`, `.2`, `.3`), and a report for each crash
//! under `crashes/`. Nothing is ever sent by itself: File > Diagnostics
//! can open the user's mail program with a report, for them to read and
//! send (to the organization's support address when a policy gives one).

use apex_fs::PathExt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const LOG: &str = "apexdesk.log";
/// The log is rotated past this size.
const MAX_LOG: u64 = 1 << 20;
/// Old logs kept besides the current one.
const KEEP: usize = 3;
/// Crash reports kept (the oldest go first).
const KEEP_CRASHES: usize = 20;
/// Marks the newest report already shown.
const SEEN: &str = ".seen";

static LOCK: Mutex<()> = Mutex::new(());

/// The errors reported, for the QA tools (tests keep no log).
#[cfg(test)]
pub mod test {
    pub static ERRORS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
}

/// Where the log and the crash reports go.
pub fn dir() -> Option<PathBuf> {
    // Tests write their own folders, never the user's.
    if cfg!(test) {
        return None;
    }
    crate::settings::config_dir().map(|d| d.join("logs"))
}

pub fn log_path() -> Option<PathBuf> {
    dir().map(|d| d.join(LOG))
}

/// Writes an error to the log.
pub fn error(text: &str) {
    #[cfg(test)]
    test::ERRORS.lock().unwrap_or_else(|e| e.into_inner()).push(text.to_string());
    if let Some(d) = dir() {
        write_log(&d, "error", text);
    }
}

/// Writes a warning to the log.
pub fn warning(text: &str) {
    if let Some(d) = dir() {
        write_log(&d, "warning", text);
    }
}

fn write_log(dir: &Path, level: &str, text: &str) {
    let _g = LOCK.lock();
    let _ = apex_fs::create_dir_all(dir);
    let path = dir.join(LOG);
    if apex_fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_LOG) {
        rotate(dir);
    }
    let line = format!("{} {level}: {}\n", stamp(), text.trim().replace('\n', " | "));
    if let Ok(mut f) = apex_fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// apexdesk.log becomes .1, .1 becomes .2...; the oldest is dropped.
fn rotate(dir: &Path) {
    let name = |i: usize| if i == 0 { dir.join(LOG) } else { dir.join(format!("{LOG}.{i}")) };
    let _ = apex_fs::remove_file(name(KEEP));
    for i in (0..KEEP).rev() {
        let _ = apex_fs::rename(name(i), name(i + 1));
    }
}

/// The local time as `2024-05-01 10:30:05`.
fn stamp() -> String {
    let (y, m, d, h, mi) = crate::common::local_now();
    let s = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs() % 60);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// The last `n` lines of the log.
fn log_tail(dir: &Path, n: usize) -> String {
    let text = apex_fs::read_to_string(dir.join(LOG)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Saves a crash report when the program panics (after the usual message
/// on the standard error).
pub fn install() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        if let Some(d) = dir() {
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let _ = save_crash(&d, &report(&msg, &place, &bt, &log_tail(&d, 20)));
            write_log(&d, "crash", &format!("{msg} ({place})"));
        }
    }));
}

fn report(msg: &str, place: &str, backtrace: &str, log: &str) -> String {
    let thread = apex_fs::thread::current().name().unwrap_or("?").to_string();
    format!(
        "ApexDesk {} closed unexpectedly\nWhen: {}\nSystem: {} {}\nThread: {thread}\nWhere: {place}\nMessage: {msg}\n\nBacktrace:\n{backtrace}\n\nLast lines of the log:\n{log}\n",
        env!("CARGO_PKG_VERSION"),
        stamp(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn crash_dir(dir: &Path) -> PathBuf {
    dir.join("crashes")
}

fn save_crash(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    let cd = crash_dir(dir);
    apex_fs::create_dir_all(&cd)?;
    let (y, m, d, h, mi) = crate::common::local_now();
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs() % 60);
    let base = format!("crash-{y:04}{m:02}{d:02}-{h:02}{mi:02}{secs:02}");
    let mut path = cd.join(format!("{base}.txt"));
    let mut n = 1;
    while path.fs_exists() {
        n += 1;
        path = cd.join(format!("{base}_{n:02}.txt"));
    }
    apex_fs::write(&path, text)?;
    for old in crashes_in(dir).into_iter().skip(KEEP_CRASHES) {
        let _ = apex_fs::remove_file(old);
    }
    Ok(path)
}

/// The crash reports, newest first.
pub fn crashes() -> Vec<PathBuf> {
    dir().map(|d| crashes_in(&d)).unwrap_or_default()
}

fn crashes_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = apex_fs::read_dir(crash_dir(dir))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))).collect())
        .unwrap_or_default();
    v.sort();
    v.reverse();
    v
}

/// Whether a crash report was saved since the last call (which marks the
/// reports as shown).
pub fn new_crash() -> bool {
    dir().is_some_and(|d| take_new(&d))
}

fn take_new(dir: &Path) -> bool {
    let Some(newest) = crashes_in(dir).first().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()) else { return false };
    let marker = crash_dir(dir).join(SEEN);
    let seen = apex_fs::read_to_string(&marker).unwrap_or_default();
    if seen.trim() >= newest.as_str() {
        return false;
    }
    let _ = apex_fs::write(marker, &newest);
    true
}

/// A `mailto:` link with `report` in the body, for the user's mail program
/// (cut to a size mail programs take in a link; the file has the rest).
pub fn mail_link(to: &str, name: &str, report: &str) -> String {
    const MAX_BODY: usize = 6000;
    let mut body: String = report.chars().take(MAX_BODY).collect();
    if body.len() < report.len() {
        body.push_str("\n[...]");
    }
    format!("mailto:{}?subject={}&body={}", encode(to.trim()), encode(&format!("ApexDesk crash report {name}")), encode(&body))
}

/// Percent-encoding for `mailto:` links.
pub fn url_encode(s: &str) -> String {
    encode(s)
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = apex_fs::temp_dir().join(format!("apexdesk-diag-{name}-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn log_rotates() {
        let d = tmp("log");
        let big = "x".repeat(4096);
        for _ in 0..(MAX_LOG as usize / 4096 * (KEEP + 2)) {
            write_log(&d, "error", &big);
        }
        write_log(&d, "error", "last\nline");
        assert!(d.join(LOG).fs_exists());
        for i in 1..=KEEP {
            assert!(d.join(format!("{LOG}.{i}")).fs_exists(), "{i}");
        }
        assert!(!d.join(format!("{LOG}.{}", KEEP + 1)).fs_exists());
        assert!(apex_fs::metadata(d.join(LOG)).unwrap().len() <= MAX_LOG + 5000);
        assert!(log_tail(&d, 1).ends_with("error: last | line"));
        let _ = apex_fs::remove_dir_all(&d);
    }

    #[test]
    fn crash_reports() {
        let d = tmp("crash");
        assert!(!take_new(&d));
        let r = report("index out of bounds", "src/x.rs:3", "0: main", "a line");
        assert!(r.contains("Message: index out of bounds") && r.contains("src/x.rs:3") && r.contains("a line"));
        let p = save_crash(&d, &r).unwrap();
        let q = save_crash(&d, "second").unwrap();
        assert_ne!(p, q);
        assert_eq!(crashes_in(&d).len(), 2);
        assert!(take_new(&d));
        assert!(!take_new(&d));
        let link = mail_link("help@corp.example", "crash-1", "a b&c\nd");
        assert_eq!(link, "mailto:help@corp.example?subject=ApexDesk%20crash%20report%20crash-1&body=a%20b%26c%0Ad");
        let _ = apex_fs::remove_dir_all(&d);
    }
}
