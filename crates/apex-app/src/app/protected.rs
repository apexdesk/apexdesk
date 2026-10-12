//! Protected View: a file that came from the Internet (a browser or mail
//! program marked it) opens read-only, with its macros off, under a bar
//! offering to enable editing. The mark is the one the system keeps:
//! the `Zone.Identifier` stream on Windows, the `com.apple.quarantine`
//! attribute on macOS and the `user.xdg.origin.url` attribute browsers set
//! on Linux. Files in the trusted locations, and everything when the
//! `ProtectedView` setting or policy is off, open as usual.

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
        std::fs::read_to_string(&ads).is_ok_and(|s| internet_zone(&s))
    }
    #[cfg(target_os = "macos")]
    {
        xattr::has(path, "com.apple.quarantine")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xattr::has(path, "user.xdg.origin.url")
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        false
    }
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
        self.settings.protected_view && !super::macro_run::trusted(path, &self.settings.trusted_locations) && from_internet(path)
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
        self.protected.tabs.remove(&id);
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id && t.path.is_none()) {
            t.view.set_read_only(false);
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
        let dir = std::env::temp_dir().join(format!("apexdesk-protected-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.xlsx");
        std::fs::write(&f, b"x").unwrap();
        assert!(!from_internet(&f));
        let p = std::ffi::CString::new(f.as_os_str().as_bytes()).unwrap();
        let url = b"https://example.com/a.xlsx";
        // SAFETY: NUL-terminated path and name, value of the given size.
        let set = unsafe { setxattr(p.as_ptr(), c"user.xdg.origin.url".as_ptr(), url.as_ptr().cast(), url.len(), 0) };
        // Some file systems (tmpfs on old kernels) have no user attributes.
        if set == 0 {
            assert!(from_internet(&f));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
