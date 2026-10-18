//! ApexDesk as the program that opens office files (File > Options >
//! General, and the question of the first start). The installers only
//! offer it under "Open with": Windows and macOS keep the choice to the
//! user. Here, on the user's request:
//! - Linux: `xdg-mime default` for each type, with a desktop entry of its
//!   own for a portable copy;
//! - macOS: Launch Services, for the types of each extension;
//! - Windows: the program, its types and capabilities registered for the
//!   user, then Settings > Default apps opened on ApexDesk (Windows lets
//!   no program take them by itself).
//!
//! Only office files: never code or configuration ones.

/// A group of file types, chosen together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Sheets,
    Docs,
    Slides,
    Pdf,
}

pub const GROUPS: [Group; 4] = [Group::Sheets, Group::Docs, Group::Slides, Group::Pdf];

impl Group {
    /// The i18n key of its name.
    pub fn key(self) -> &'static str {
        match self {
            Group::Sheets => "default_apps.sheets",
            Group::Docs => "default_apps.docs",
            Group::Slides => "default_apps.slides",
            Group::Pdf => "default_apps.pdf",
        }
    }

    /// Its extensions and media types; the first one tells whether
    /// ApexDesk is the default.
    pub fn types(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Group::Sheets => &[
                ("xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
                ("xlsm", "application/vnd.ms-excel.sheet.macroEnabled.12"),
                ("xltx", "application/vnd.openxmlformats-officedocument.spreadsheetml.template"),
                ("xltm", "application/vnd.ms-excel.template.macroEnabled.12"),
                ("xlsb", "application/vnd.ms-excel.sheet.binary.macroEnabled.12"),
                ("xls", "application/vnd.ms-excel"),
                ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
                ("ots", "application/vnd.oasis.opendocument.spreadsheet-template"),
                ("fods", "application/vnd.oasis.opendocument.spreadsheet-flat-xml"),
                ("csv", "text/csv"),
                ("tsv", "text/tab-separated-values"),
                ("et", "application/wps-office.et"),
            ],
            Group::Docs => &[
                ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
                ("docm", "application/vnd.ms-word.document.macroEnabled.12"),
                ("dotx", "application/vnd.openxmlformats-officedocument.wordprocessingml.template"),
                ("dotm", "application/vnd.ms-word.template.macroEnabled.12"),
                ("doc", "application/msword"),
                ("odt", "application/vnd.oasis.opendocument.text"),
                ("ott", "application/vnd.oasis.opendocument.text-template"),
                ("fodt", "application/vnd.oasis.opendocument.text-flat-xml"),
                ("rtf", "application/rtf"),
                ("md", "text/markdown"),
                ("wps", "application/wps-office.wps"),
            ],
            Group::Slides => &[
                ("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
                ("pptm", "application/vnd.ms-powerpoint.presentation.macroEnabled.12"),
                ("potx", "application/vnd.openxmlformats-officedocument.presentationml.template"),
                ("potm", "application/vnd.ms-powerpoint.template.macroEnabled.12"),
                ("ppsx", "application/vnd.openxmlformats-officedocument.presentationml.slideshow"),
                ("ppsm", "application/vnd.ms-powerpoint.slideshow.macroEnabled.12"),
                ("ppt", "application/vnd.ms-powerpoint"),
                ("odp", "application/vnd.oasis.opendocument.presentation"),
                ("otp", "application/vnd.oasis.opendocument.presentation-template"),
            ],
            Group::Pdf => &[("pdf", "application/pdf")],
        }
    }

    /// Its Windows ProgID, the same as the MSI's.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn prog_id(self) -> &'static str {
        match self {
            Group::Sheets => "ApexDesk.Spreadsheet",
            Group::Docs => "ApexDesk.Document",
            Group::Slides => "ApexDesk.Presentation",
            Group::Pdf => "ApexDesk.Pdf",
        }
    }
}

/// What making ApexDesk the default did.
#[derive(Debug, PartialEq, Eq)]
pub enum Done {
    /// The types open with ApexDesk now.
    Set,
    /// The system's settings were opened for the user to confirm.
    #[cfg_attr(not(windows), allow(dead_code))]
    Settings,
}

/// Whether ApexDesk opens the group's main type now (None: can't tell).
pub fn is_default(g: Group) -> Option<bool> {
    sys::is_default(g)
}

/// Makes ApexDesk the program for the groups' types.
pub fn make_default(groups: &[Group]) -> Result<Done, String> {
    if groups.is_empty() {
        return Ok(Done::Set);
    }
    sys::make_default(groups)
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
mod sys {
    use super::{Done, Group};
    use std::path::PathBuf;
    use std::process::Command;

    /// The desktop entry of an installed ApexDesk.
    const ENTRY: &str = "apexdesk.desktop";

    fn data_dirs() -> Vec<PathBuf> {
        let home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        home.into_iter().chain(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from)).collect()
    }

    /// The entry to name: an installed one (also a Flatpak's), else one
    /// written for this copy of the program (a portable one).
    fn entry() -> Result<String, String> {
        let flatpak = std::env::var("FLATPAK_ID").ok().map(|id| format!("{id}.desktop"));
        for name in flatpak.iter().map(String::as_str).chain([ENTRY]) {
            if data_dirs().iter().any(|d| d.join("applications").join(name).is_file()) {
                return Ok(name.to_string());
            }
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = data_dirs().into_iter().next().ok_or("no HOME")?.join("applications");
        apex_fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mimes: String = super::GROUPS.iter().flat_map(|g| g.types()).map(|(_, m)| format!("{m};")).collect();
        let exe = exe.to_string_lossy().replace('\\', "\\\\").replace(' ', "\\s");
        let text = format!("[Desktop Entry]\nType=Application\nName=ApexDesk\nExec={exe} %F\nIcon=apexdesk\nTerminal=false\nCategories=Office;\nMimeType={mimes}\nNoDisplay=true\n");
        apex_fs::write(dir.join(ENTRY), text).map_err(|e| e.to_string())?;
        let _ = Command::new("update-desktop-database").arg(&dir).status();
        Ok(ENTRY.to_string())
    }

    fn query(mime: &str) -> Option<String> {
        let out = Command::new("xdg-mime").args(["query", "default", mime]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn is_default(g: Group) -> Option<bool> {
        let current = query(g.types()[0].1)?;
        let flatpak = std::env::var("FLATPAK_ID").ok().map(|id| format!("{id}.desktop"));
        Some(current == ENTRY || flatpak.is_some_and(|f| current == f))
    }

    pub fn make_default(groups: &[Group]) -> Result<Done, String> {
        let entry = entry()?;
        let mimes: Vec<&str> = groups.iter().flat_map(|g| g.types()).map(|(_, m)| *m).collect();
        let st = Command::new("xdg-mime").arg("default").arg(&entry).args(&mimes).status().map_err(|e| format!("xdg-mime: {e}"))?;
        if !st.success() {
            return Err(format!("xdg-mime: {st}"));
        }
        Ok(Done::Set)
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use super::{Done, Group};
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::{CFURL, CFURLRef};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{msg_send, sel};
    use std::ffi::c_void;
    use std::sync::mpsc;
    use web_time::{Duration, Instant};

    const ROLES_ALL: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSSetDefaultRoleHandlerForContentType(content_type: CFStringRef, role: u32, bundle_id: CFStringRef) -> i32;
        fn LSCopyDefaultRoleHandlerForContentType(content_type: CFStringRef, role: u32) -> CFStringRef;
        fn LSRegisterURL(url: CFURLRef, update: u8) -> i32;
        fn UTTypeCreatePreferredIdentifierForTag(tag_class: CFStringRef, tag: CFStringRef, conforming_to: CFStringRef) -> CFStringRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFBundleGetMainBundle() -> *const c_void;
        fn CFBundleGetIdentifier(bundle: *const c_void) -> CFStringRef;
        fn CFBundleCopyBundleURL(bundle: *const c_void) -> CFURLRef;
    }

    /// The bundle id of the running app (none outside ApexDesk.app).
    fn bundle_id() -> Option<CFString> {
        // SAFETY: plain CoreFoundation calls; the identifier belongs to
        // the bundle (get rule), so it is retained.
        unsafe {
            let b = CFBundleGetMainBundle();
            if b.is_null() {
                return None;
            }
            let id = CFBundleGetIdentifier(b);
            (!id.is_null()).then(|| CFString::wrap_under_get_rule(id))
        }
    }

    /// Where ApexDesk.app is.
    fn bundle_url() -> Option<CFURL> {
        // SAFETY: a copied URL (create rule) or null.
        unsafe {
            let b = CFBundleGetMainBundle();
            if b.is_null() {
                return None;
            }
            let u = CFBundleCopyBundleURL(b);
            (!u.is_null()).then(|| CFURL::wrap_under_create_rule(u))
        }
    }

    /// Run from the disk image or moved aside by macOS (App Translocation:
    /// an app downloaded and not moved runs from a random read-only copy),
    /// where Launch Services won't keep it as the default.
    fn temporary(path: &str) -> bool {
        path.contains("/AppTranslocation/") || path.starts_with("/Volumes/")
    }

    /// The type identifier of files with `ext`; none for the made-up
    /// `dyn.…` ones of extensions macOS doesn't know, which can't be given
    /// to a program.
    fn uti(ext: &str) -> Option<CFString> {
        let class = CFString::from_static_string("public.filename-extension");
        let tag = CFString::new(ext);
        // SAFETY: a created string (create rule) or null.
        let t = unsafe {
            let r = UTTypeCreatePreferredIdentifierForTag(class.as_concrete_TypeRef(), tag.as_concrete_TypeRef(), std::ptr::null());
            (!r.is_null()).then(|| CFString::wrap_under_create_rule(r))
        }?;
        (!t.to_string().starts_with("dyn.")).then_some(t)
    }

    fn handler(t: &CFString) -> Option<String> {
        // SAFETY: a copied string (create rule) or null.
        unsafe {
            let r = LSCopyDefaultRoleHandlerForContentType(t.as_concrete_TypeRef(), ROLES_ALL);
            (!r.is_null()).then(|| CFString::wrap_under_create_rule(r).to_string())
        }
    }

    pub fn is_default(g: Group) -> Option<bool> {
        let id = bundle_id()?;
        let t = uti(g.types()[0].0)?;
        Some(handler(&t).is_some_and(|c| c.eq_ignore_ascii_case(&id.to_string())))
    }

    /// macOS 12 and later: `NSWorkspace setDefaultApplicationAtURL:
    /// toOpenContentType:completionHandler:`, which asks Launch Services
    /// the current way. One message per type; how many were sent (none:
    /// not there, use the old call). The answers arrive on `done`.
    fn workspace(app: &CFURL, types: &[CFString], done: &mpsc::Sender<()>) -> usize {
        let (Some(ws_class), Some(ut_class)) = (AnyClass::get(c"NSWorkspace"), AnyClass::get(c"UTType")) else { return 0 };
        // SAFETY: Objective-C messages to classes that exist, checked to
        // answer them; CFURL and CFString are the same objects as NSURL
        // and NSString (toll-free bridged), alive for the calls.
        unsafe {
            let ws: *mut AnyObject = msg_send![ws_class, sharedWorkspace];
            if ws.is_null() {
                return 0;
            }
            let ok: Bool = msg_send![ws, respondsToSelector: sel!(setDefaultApplicationAtURL:toOpenContentType:completionHandler:)];
            if !ok.as_bool() {
                return 0;
            }
            let url = app.as_concrete_TypeRef() as *mut AnyObject;
            let mut sent = 0;
            for t in types {
                let ut: *mut AnyObject = msg_send![ut_class, typeWithIdentifier: t.as_concrete_TypeRef() as *mut AnyObject];
                if ut.is_null() {
                    continue;
                }
                let tx = done.clone();
                let block = block2::RcBlock::new(move |_err: *mut AnyObject| {
                    let _ = tx.send(());
                });
                let _: () = msg_send![ws, setDefaultApplicationAtURL: url, toOpenContentType: ut, completionHandler: &*block];
                sent += 1;
            }
            sent
        }
    }

    pub fn make_default(groups: &[Group]) -> Result<Done, String> {
        let id = bundle_id().ok_or("ApexDesk.app")?;
        let app = bundle_url().ok_or("ApexDesk.app")?;
        let path = app.to_path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if temporary(&path) {
            return Err(crate::i18n::tr("default_apps.move_app").to_string());
        }
        // So that Launch Services knows this copy and its types.
        // SAFETY: a valid URL for the call.
        unsafe { LSRegisterURL(app.as_concrete_TypeRef(), 1) };
        let types: Vec<(&str, CFString)> = groups.iter().flat_map(|g| g.types()).filter_map(|(ext, _)| uti(ext).map(|t| (*ext, t))).collect();
        let (tx, rx) = mpsc::channel();
        let sent = workspace(&app, &types.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(), &tx);
        if sent > 0 {
            let until = Instant::now() + Duration::from_secs(5);
            for _ in 0..sent {
                if rx.recv_timeout(until.saturating_duration_since(Instant::now())).is_err() {
                    break;
                }
            }
        }
        // Then what each type opens with: the old call where it isn't
        // ApexDesk yet, and the ones that still aren't, named.
        let me = id.to_string();
        let mut failed = Vec::new();
        for (ext, t) in &types {
            if handler(t).is_some_and(|c| c.eq_ignore_ascii_case(&me)) {
                continue;
            }
            // SAFETY: valid strings for the call.
            let st = unsafe { LSSetDefaultRoleHandlerForContentType(t.as_concrete_TypeRef(), ROLES_ALL, id.as_concrete_TypeRef()) };
            if st != 0 || !handler(t).is_some_and(|c| c.eq_ignore_ascii_case(&me)) {
                failed.push(format!(".{ext}"));
            }
        }
        if failed.is_empty() { Ok(Done::Set) } else { Err(failed.join(", ")) }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn temporary_places() {
            assert!(super::temporary("/private/var/folders/x/AppTranslocation/1234/d/ApexDesk.app"));
            assert!(super::temporary("/Volumes/ApexDesk/ApexDesk.app"));
            assert!(!super::temporary("/Applications/ApexDesk.app"));
        }
    }
}

#[cfg(windows)]
mod sys {
    use super::{Done, Group};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
    use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_PROGID, AssocQueryStringW, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, ShellExecuteW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, PWSTR, w};

    /// Sets `name` (None: the default value) of HKCU\`path` to `value`.
    fn set(path: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let mut key = HKEY::default();
        let p = HSTRING::from(path);
        // SAFETY: valid strings and an out key closed below.
        unsafe {
            let r = RegCreateKeyExW(HKEY_CURRENT_USER, &p, None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None);
            if r != ERROR_SUCCESS {
                return Err(format!("{path}: {r:?}"));
            }
            let data: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
            let n = name.map(HSTRING::from);
            let r = RegSetValueExW(key, n.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())), None, REG_SZ, Some(&data));
            let _ = RegCloseKey(key);
            if r != ERROR_SUCCESS {
                return Err(format!("{path}: {r:?}"));
            }
        }
        Ok(())
    }

    /// The program, its ProgIDs and capabilities, for this user.
    fn register() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe.to_string_lossy();
        let caps = r"Software\ApexDesk\Capabilities";
        set(caps, Some("ApplicationName"), "ApexDesk")?;
        set(caps, Some("ApplicationDescription"), &crate::i18n::tr("default_apps.description"))?;
        for g in super::GROUPS {
            let id = g.prog_id();
            let classes = format!(r"Software\Classes\{id}");
            set(&classes, None, &crate::i18n::tr(g.key()))?;
            set(&format!(r"{classes}\DefaultIcon"), None, &format!("\"{exe}\",0"))?;
            set(&format!(r"{classes}\shell\open\command"), None, &format!("\"{exe}\" \"%1\""))?;
            for (ext, mime) in g.types() {
                set(&format!(r"Software\Classes\.{ext}\OpenWithProgids"), Some(id), "")?;
                set(&format!(r"{caps}\FileAssociations"), Some(&format!(".{ext}")), id)?;
                set(&format!(r"{caps}\MimeAssociations"), Some(mime), id)?;
            }
        }
        set(r"Software\RegisteredApplications", Some("ApexDesk"), caps)?;
        // SAFETY: no item: tells the shell associations changed.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
        Ok(())
    }

    pub fn is_default(g: Group) -> Option<bool> {
        let ext = HSTRING::from(format!(".{}", g.types()[0].0));
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        // SAFETY: a buffer of `len` characters.
        let r = unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_PROGID, &ext, w!("open"), Some(PWSTR(buf.as_mut_ptr())), &mut len) };
        if r.is_err() {
            return Some(false);
        }
        let s = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1).min(buf.len())]);
        Some(s.trim_end_matches('\0').eq_ignore_ascii_case(g.prog_id()))
    }

    pub fn make_default(_groups: &[Group]) -> Result<Done, String> {
        register()?;
        // Windows 11 opens ApexDesk's page; Windows 10, the list of apps.
        // SAFETY: plain strings for the call.
        let r = unsafe { ShellExecuteW(None, w!("open"), w!("ms-settings:defaultapps?registeredAppUser=ApexDesk"), None, None, SW_SHOWNORMAL) };
        if r.0 as usize <= 32 {
            return Err(format!("ms-settings: {}", r.0 as usize));
        }
        Ok(Done::Settings)
    }
}

#[cfg(any(target_os = "android", target_arch = "wasm32"))]
mod sys {
    use super::{Done, Group};

    pub fn is_default(_: Group) -> Option<bool> {
        None
    }

    pub fn make_default(_: &[Group]) -> Result<Done, String> {
        Err("Android".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Office types only: no code, configuration or plain text ones, and
    /// none twice.
    #[test]
    fn office_types_only() {
        let mut seen = std::collections::HashSet::new();
        for g in GROUPS {
            for (ext, mime) in g.types() {
                assert!(!apex_io::CODE_EXTS.contains(ext), "{ext} is code");
                assert!(*ext != "txt" && !mime.starts_with("text/plain"), "{ext}");
                assert!(seen.insert(*ext), "{ext} twice");
            }
        }
        // Every type listed opens in ApexDesk.
        for ext in seen {
            let known = [apex_io::SHEET_EXTS, apex_io::DOC_EXTS, apex_io::SLIDES_EXTS].iter().any(|l| l.contains(&ext));
            assert!(known, "{ext} doesn't open");
        }
    }
}
