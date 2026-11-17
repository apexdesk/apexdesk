//! Protected View: a file that came from the Internet (a browser or mail
//! program marked it) opens read-only, with its macros off, under a bar
//! offering to enable editing. The mark is the one the system keeps:
//! the `Zone.Identifier` stream on Windows, the `com.apple.quarantine` (or
//! `kMDItemWhereFroms`) attribute on macOS and the `user.xdg.origin.url` or
//! `user.xdg.referrer.url` attributes browsers set on Linux, where few
//! programs set them: there a file in the downloads, temporary or cache
//! folders opens in Protected View too. Each file asks once: one the user
//! enabled editing in, or saved here, opens as usual from then on
//! (`vouch`). So do the files in the trusted locations, and everything
//! when the `ProtectedView` setting or policy is off.

use std::collections::HashSet;
use std::path::Path;

use eframe::egui::{self, RichText};

use super::App;
use crate::i18n::tr;
use crate::icons::{self, Icon};

#[derive(Default)]
pub(super) struct Protected {
    /// Tabs in Protected View.
    pub tabs: HashSet<u64>,
}

/// The zone in a `Zone.Identifier` stream: 3 is the Internet, 4 untrusted
/// sites.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn internet_zone(stream: &str) -> bool {
    stream.lines().filter_map(|l| l.trim().strip_prefix("ZoneId=")).any(|z| z.trim().parse::<u32>().is_ok_and(|z| z >= 3))
}

/// Whether the system marked `path` as downloaded from the Internet.
pub fn from_internet(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let mut ads = path.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        apex_fs::read_to_string(&ads).is_ok_and(|s| internet_zone(&s))
    }
    #[cfg(target_os = "macos")]
    {
        xattr::has(path, "com.apple.quarantine") || xattr::has(path, "com.apple.metadata:kMDItemWhereFroms")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        marked(path)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        false
    }
}

/// The marks browsers leave on Linux: where the file came from, and the
/// page that linked it.
#[cfg(all(unix, not(target_os = "macos")))]
fn marked(path: &Path) -> bool {
    xattr::has(path, "user.xdg.origin.url") || xattr::has(path, "user.xdg.referrer.url")
}

/// Folders where files from outside land unmarked: the downloads folder,
/// the temporary ones (where browsers and mail programs open
/// attachments) and the cache (some mail programs keep attachments there).
#[cfg(all(unix, not(target_os = "macos")))]
fn outside_folders() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute());
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home.as_ref().map(|h| h.join(".config")));
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home.as_ref().map(|h| h.join(".cache")));
    let downloads = config.and_then(|c| apex_fs::read_to_string(c.join("user-dirs.dirs")).ok()).and_then(|t| download_dir(&t, home.as_deref())).or_else(|| home.as_ref().map(|h| h.join("Downloads")));
    let mut out: Vec<PathBuf> = [Some(std::env::temp_dir()), Some("/tmp".into()), Some("/var/tmp".into()), downloads, cache].into_iter().flatten().collect();
    // A variable naming the home folder or the root would put every file
    // in Protected View.
    out.retain(|d| Some(d.as_path()) != home.as_deref() && d.parent().is_some());
    out
}

/// `XDG_DOWNLOAD_DIR` of a `user-dirs.dirs` file.
#[cfg(all(unix, not(target_os = "macos")))]
fn download_dir(text: &str, home: Option<&Path>) -> Option<std::path::PathBuf> {
    let v = text.lines().find_map(|l| l.trim().strip_prefix("XDG_DOWNLOAD_DIR="))?.trim().trim_matches('"');
    match v.strip_prefix("$HOME") {
        Some(rest) => Some(home?.join(rest.trim_start_matches('/'))),
        None => Some(std::path::PathBuf::from(v)).filter(|p| p.is_absolute()),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn in_folder_of_outside_files(path: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    outside_folders().iter().any(|d| path.starts_with(d) || d.canonicalize().is_ok_and(|d| path.starts_with(d)))
}

/// Whether `path` is where files from outside land on a system that
/// marks few of them. That alone opens it in Protected View; it isn't a
/// mark: what is saved from it, and its macros run from a command, are
/// as any other file's.
fn left_by_another_program(path: &Path) -> bool {
    // Tests keep their files (and settings) in the temporary folder.
    #[cfg(all(unix, not(target_os = "macos")))]
    return !cfg!(test) && in_folder_of_outside_files(path);
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = path;
        false
    }
}

/// The files vouched for, the newest last: so many at most.
const VOUCHED: usize = 1000;

fn vouched_file() -> Option<std::path::PathBuf> {
    crate::settings::config_dir().map(|d| d.join("trusted-documents.txt"))
}

/// A file as it is now, a line of the list: its length, when it last
/// changed and its path. Changed by anything else, it is another file.
fn vouched_line(path: &Path) -> Option<String> {
    let path = apex_fs::canonicalize(path).ok()?;
    let meta = apex_fs::metadata(&path).ok()?;
    let changed = meta.modified().ok()?.duration_since(web_time::UNIX_EPOCH).ok()?.as_nanos();
    let path = path.to_str().filter(|p| !p.contains(['\n', '\r']))?;
    Some(format!("{} {changed} {path}", meta.len()))
}

/// The user enabled editing in `path`, or saved it here: it opens out of
/// Protected View from now on, for as long as it stays as it is.
pub(super) fn vouch(path: &Path) {
    let (Some(file), Some(line)) = (vouched_file(), vouched_line(path)) else { return };
    let name = line.splitn(3, ' ').nth(2).unwrap_or_default().to_string();
    let text = apex_fs::read_to_string(&file).unwrap_or_default();
    let mut lines: Vec<&str> = text.lines().filter(|l| l.splitn(3, ' ').nth(2) != Some(name.as_str())).collect();
    lines.push(&line);
    let from = lines.len().saturating_sub(VOUCHED);
    if let Some(dir) = file.parent() {
        let _ = apex_fs::create_dir_all(dir);
    }
    let _ = apex_fs::write(&file, lines[from..].join("\n") + "\n");
}

fn vouched(path: &Path) -> bool {
    let (Some(file), Some(line)) = (vouched_file(), vouched_line(path)) else { return false };
    apex_fs::read_to_string(file).is_ok_and(|t| t.lines().any(|l| l == line))
}

#[cfg(unix)]
mod xattr {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn getxattr(path: *const std::ffi::c_char, name: *const std::ffi::c_char, value: *mut std::ffi::c_void, size: usize, position: u32, options: i32) -> isize;
    }
    #[cfg(not(target_os = "macos"))]
    unsafe extern "C" {
        fn getxattr(path: *const std::ffi::c_char, name: *const std::ffi::c_char, value: *mut std::ffi::c_void, size: usize) -> isize;
    }

    /// Whether `path` has the extended attribute `name`.
    pub fn has(path: &Path, name: &str) -> bool {
        let (Ok(p), Ok(n)) = (CString::new(path.as_os_str().as_bytes()), CString::new(name)) else { return false };
        // SAFETY: both strings are NUL-terminated; a null buffer of size 0
        // only asks for the value's length.
        #[cfg(target_os = "macos")]
        let len = unsafe { getxattr(p.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
        #[cfg(not(target_os = "macos"))]
        let len = unsafe { getxattr(p.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0) };
        len >= 0
    }
}

impl App {
    /// Whether a file being opened goes to Protected View.
    pub(super) fn should_protect(&self, path: &Path) -> bool {
        self.settings.protected_view && !super::macro_run::trusted(path, &self.settings.trusted_locations) && (from_internet(path) || left_by_another_program(path)) && !vouched(path)
    }

    /// The tab just opened is in Protected View: read-only, macros off.
    pub(super) fn protect_last(&mut self) {
        if let Some(t) = self.tabs.last_mut() {
            t.view.set_read_only(true);
            self.protected.tabs.insert(t.id);
        }
    }

    /// Enable Editing: the tab is as it would have opened (still read-only
    /// if it was opened so, or someone else has the file: `sync_locks`
    /// applies that). Its macros are then as in any file from outside the
    /// trusted locations: off until the user enables them.
    pub(super) fn unprotect(&mut self, id: u64) {
        // Every window of the file: they show the same workbook.
        let group = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.group());
        let ids: Vec<u64> = self.tabs.iter().filter(|t| t.id == id || (group.is_some() && t.group() == group)).map(|t| t.id).collect();
        for id in ids {
            self.protected.tabs.remove(&id);
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id && t.path.is_none()) {
                t.view.set_read_only(false);
            }
        }
        // The file asks once.
        if let Some(path) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.path.clone()) {
            vouch(&path);
        }
    }

    /// The Protected View bar of the active tab, under the toolstrip.
    pub(super) fn protected_bar(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.tabs.get(self.active).map(|t| t.id).filter(|id| self.protected.tabs.contains(id)) else { return };
        let dark = ui.visuals().dark_mode;
        let (fill, edge) = if dark { (egui::Color32::from_rgb(0x4A, 0x40, 0x1C), egui::Color32::from_rgb(0x8A, 0x74, 0x2A)) } else { (egui::Color32::from_rgb(0xFF, 0xF4, 0xCE), egui::Color32::from_rgb(0xE8, 0xD0, 0x7A)) };
        let mut enable = false;
        egui::Panel::top("protected_bar").frame(egui::Frame::new().fill(fill).stroke(egui::Stroke::new(1.0, edge)).inner_margin(egui::Margin::symmetric(10, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                icons::paint(ui.painter(), r, Icon::Protect, ui.visuals().text_color());
                ui.label(RichText::new(tr("protected.title")).strong());
                ui.label(tr("protected.bar"));
                enable = ui.button(RichText::new(tr("protected.enable")).strong()).clicked();
            });
        });
        if enable {
            self.unprotect(id);
        }
    }

    /// Whether tab `id` is in Protected View (its macros wait).
    pub(super) fn is_protected(&self, id: u64) -> bool {
        self.protected.tabs.contains(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones() {
        assert!(internet_zone("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/a.xlsx\r\n"));
        assert!(internet_zone("[ZoneTransfer]\nZoneId=4\n"));
        assert!(!internet_zone("[ZoneTransfer]\nZoneId=1\n"));
        assert!(!internet_zone("[ZoneTransfer]\nZoneId=2\n"));
        assert!(!internet_zone(""));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn origin_attribute_marks_downloads() {
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn setxattr(path: *const std::ffi::c_char, name: *const std::ffi::c_char, value: *const std::ffi::c_void, size: usize, flags: i32) -> i32;
        }
        let dir = apex_fs::temp_dir().join(format!("apexdesk-protected-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.xlsx");
        apex_fs::write(&f, b"x").unwrap();
        assert!(!from_internet(&f));
        let p = std::ffi::CString::new(f.as_os_str().as_bytes()).unwrap();
        let url = b"https://example.com/a.xlsx";
        // SAFETY: NUL-terminated path and name, value of the given size.
        let set = unsafe { setxattr(p.as_ptr(), c"user.xdg.origin.url".as_ptr(), url.as_ptr().cast(), url.len(), 0) };
        // Some file systems (tmpfs on old kernels) have no user attributes.
        if set == 0 {
            assert!(from_internet(&f));
        }
        // Unmarked, it is still in the temporary folder.
        apex_fs::write(dir.join("b.xlsx"), b"x").unwrap();
        assert!(in_folder_of_outside_files(&dir.join("b.xlsx")));
        assert!(!in_folder_of_outside_files(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml").as_path()));
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn downloads_folder_of_user_dirs() {
        let home = Path::new("/home/ana");
        let t = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Escritorio\"\nXDG_DOWNLOAD_DIR=\"$HOME/Descargas\"\n";
        assert_eq!(download_dir(t, Some(home)).unwrap(), home.join("Descargas"));
        assert_eq!(download_dir("XDG_DOWNLOAD_DIR=\"/data/dl\"\n", Some(home)).unwrap(), Path::new("/data/dl"));
        assert!(download_dir("XDG_DOWNLOAD_DIR=\"rel\"\n", Some(home)).is_none());
    }

    /// A file asks once: vouched for, it stays out of Protected View until
    /// something else changes it.
    #[test]
    fn a_file_asks_once() {
        let dir = apex_fs::temp_dir().join(format!("apexdesk-vouch-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a b.xlsx"), dir.join("b.xlsx"));
        apex_fs::write(&a, b"one").unwrap();
        apex_fs::write(&b, b"one").unwrap();
        assert!(!vouched(&a));
        vouch(&a);
        vouch(&a);
        assert!(vouched(&a) && !vouched(&b));
        let list = apex_fs::read_to_string(vouched_file().unwrap()).unwrap();
        assert_eq!(list.lines().filter(|l| l.ends_with("a b.xlsx")).count(), 1);
        // Another download of the same name isn't the file vouched for.
        apex_fs::write(&a, b"another").unwrap();
        assert!(!vouched(&a));
        // Nor is a file that is gone.
        vouch(&b);
        apex_fs::remove_file(&b).unwrap();
        assert!(!vouched(&b));
        let _ = apex_fs::remove_dir_all(&dir);
    }

    /// View > New Window of a file in Protected View is in it too, and
    /// Enable Editing in one window enables both.
    #[test]
    fn new_window_stays_protected() {
        use super::super::View;
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, std::time::Instant::now());
        app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(apex_sheet::Workbook::new()))), None, None);
        app.active = 0;
        app.protect_last();
        app.new_window();
        assert_eq!(app.tabs.len(), 2);
        let (a, b) = (app.tabs[0].id, app.tabs[1].id);
        assert!(app.is_protected(a) && app.is_protected(b));
        assert!(app.tabs[1].view.read_only());
        app.unprotect(b);
        assert!(!app.is_protected(a) && !app.is_protected(b));
    }
}
