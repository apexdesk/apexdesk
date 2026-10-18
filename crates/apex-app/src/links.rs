//! Links that come from documents (hyperlinks in text, cells and PDFs,
//! embedded objects) are checked before the system opens them: a link is
//! the usual way a document gets a program run or makes Windows send the
//! user's credentials to a server.
//!
//! - Web and e-mail links (`http`, `https`, `mailto`) open.
//! - Programs and scripts (`.exe`, `.bat`, `.lnk`, `.desktop`, `.sh`...),
//!   schemes known to run code (`javascript:`, `ms-msdt:`, `search-ms:`...)
//!   and, on Windows, network paths (`\\server\share`) never open.
//! - Anything else (a local file, another scheme) asks first, showing the
//!   whole target. The `LinkSecurity` policy or setting makes that "allow"
//!   (open without asking) or "block" (never open).
//!
//! Links the program itself opens (update notes, the support address)
//! use [`crate::common::open_external`] directly.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui::{self, RichText};

use crate::i18n::{tr, trf};

/// What happens to a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Open,
    Ask,
    /// Never opened; the reason's i18n key.
    Block(&'static str),
}

/// Extensions of files that run something when opened.
const RUNS: &[&str] = &[
    "exe", "com", "scr", "pif", "cpl", "msi", "msp", "mst", "msc", "msix", "msixbundle", "appx", "appxbundle", "appinstaller", "application", "appref-ms", "gadget", "bat", "cmd", "ps1", "ps1xml", "ps2", "psc1", "psd1", "psm1", "vb", "vbe", "vbs", "js", "jse", "ws", "wsc", "wsf", "wsh", "hta", "jar", "lnk", "url", "scf", "inf", "reg", "settingcontent-ms", "library-ms", "search-ms", "searchconnector-ms", "diagcab", "diagcfg", "diagpkg", "iso", "img", "vhd", "vhdx", "chm", "hlp", "xll", "xlam", "ppam", "dll", "ocx", "sys", "drv", "py", "pyw", "pyc", "pl", "rb", "php", "sh", "bash", "zsh", "csh", "ksh", "fish", "command", "tool", "desktop", "run", "bin", "appimage", "deb", "rpm", "snap", "flatpakref", "flatpakrepo", "pkg", "mpkg", "dmg", "app", "action", "workflow", "scpt", "applescript", "terminal", "kext", "prefpane",
    // Office files with macros, Access databases and files Office or
    // Windows act on when opened (queries, remote desktop, themes, web
    // archives, sandboxes, notebooks, Java Web Start, updates, scriptlets).
    "docm", "dotm", "xlsm", "xltm", "xlsb", "xla", "pptm", "potm", "ppsm", "sldm", "ppa", "accdb", "accde", "accda", "mdb", "mde", "ade", "adp", "mda", "iqy", "slk", "dqy", "rqy", "oqy", "rdp", "theme", "themepack", "deskthemepack", "mht", "mhtml", "wsb", "one", "onepkg", "jnlp", "msu", "xbap", "website", "sct", "cab", "cer", "crt", "der",
    // Python zip apps, Excel 4 macro sheets, Citrix and Office add-in
    // installers, Monad shell scripts, Windows scrap files and the macOS
    // links that open their target.
    "pyz", "pyzw", "xlm", "ica", "vsto", "msh", "msh1", "msh2", "mshxml", "msh1xml", "msh2xml", "ps2xml", "psc2", "shs", "shb", "fileloc", "inetloc", "webloc",
    // AutoHotkey and AutoIt scripts, Tcl, macOS configuration profiles.
    "ahk", "au3", "a3x", "tcl", "tk", "mobileconfig", "lua", "vbscript",
    // More of what Windows blocks as attachments.
    "ins", "isp", "its", "job", "prf", "xnk", "printerexport", "mam", "maq", "mar", "mas", "mat", "mav", "maw", "maf", "htc", "bas", "mcf", "vsmacros", "plg", "osd", "mdt", "mdw", "mdz",
    // Windows contacts (their links run things) and playlists or data
    // links that reach out to other machines when opened.
    "contact", "vcf", "wab", "m3u", "m3u8", "asx", "wax", "wvx", "wpl", "udl",
];

/// Schemes whose handlers run code or fetch and open things silently.
const BAD_SCHEMES: &[&str] = &["javascript", "vbscript", "data", "blob", "ms-msdt", "search-ms", "search", "ms-search", "ms-officecmd", "ms-appinstaller", "ms-cxh", "ms-cxh-full", "shell", "hcp", "ms-its", "its", "mk", "mhtml", "jar", "vscode", "vscode-insiders", "ms-excel", "ms-word", "ms-powerpoint", "ms-access", "ms-visio", "ms-project", "ms-publisher", "ms-infopath", "ms-spd", "ms-settings", "ms-windows-store", "microsoft-edge", "ssh", "telnet", "smb", "cifs", "nfs", "afp", "webdav", "davs", "dav"];

/// What "ask" becomes (see `set_mode`): 0 ask, 1 allow, 2 block.
static MODE: AtomicU8 = AtomicU8::new(0);

/// The setting or policy: "allow", "block", or anything else to ask.
pub fn set_mode(mode: &str) {
    MODE.store(
        match mode.trim().to_ascii_lowercase().as_str() {
            "allow" => 1,
            "block" => 2,
            _ => 0,
        },
        Ordering::Relaxed,
    );
}

/// The scheme of `t` ("https" in "https://…"), not a drive letter.
fn scheme(t: &str) -> Option<String> {
    let (s, _) = t.split_once(':')?;
    let ok = s.len() > 1 && s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    ok.then(|| s.to_ascii_lowercase())
}

fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether the file a path names runs something: its last name's
/// extension, as Windows reads it (trailing dots and spaces, and an
/// alternate stream after ':', don't count). A folder named with a final
/// separator (a macOS `.app` bundle) counts by its own name, and `C:x.exe`
/// (a path relative to a drive) by what follows the drive.
pub fn runs(path: &str) -> bool {
    let path = path.trim_end_matches(|c: char| matches!(c, '/' | '\\' | '.') || c.is_whitespace() || c.is_control());
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let name = match name.as_bytes() {
        [d, b':', ..] if d.is_ascii_alphabetic() => &name[2..],
        _ => name,
    };
    let name = name.split(':').next().unwrap_or(name).trim_end_matches(|c: char| c == '.' || c.is_whitespace() || c.is_control());
    name.rsplit_once('.').is_some_and(|(_, ext)| RUNS.contains(&ext.trim().to_ascii_lowercase().as_str()))
}

/// Whether a file written out of a document (an object, an attachment),
/// named `name` and holding `data`, would run something when the system
/// opens it: by its name, or by being a program. A name without an
/// extension counts as one everywhere: openers then go by the contents
/// (and a Windows program opens with Wine).
pub fn copy_runs(name: &str, data: &[u8]) -> bool {
    runs(name) || bare(name) || is_program(data)
}

/// Whether `data` is a program or a launcher: Windows, Linux or macOS
/// executables, scripts (`#!`) and desktop entries.
fn is_program(data: &[u8]) -> bool {
    const MACHO: [[u8; 4]; 5] = [[0xFE, 0xED, 0xFA, 0xCE], [0xFE, 0xED, 0xFA, 0xCF], [0xCE, 0xFA, 0xED, 0xFE], [0xCF, 0xFA, 0xED, 0xFE], [0xCA, 0xFE, 0xBA, 0xBE]];
    let head = &data[..data.len().min(4096)];
    data.starts_with(b"MZ") || data.starts_with(b"\x7fELF") || data.starts_with(b"#!") || MACHO.iter().any(|m| data.starts_with(m)) || head.windows(15).any(|w| w == b"[Desktop Entry]")
}

/// Whether `path` names a file without an extension: Windows looks for
/// `calc` as `calc.exe`, `calc.bat`... and runs what it finds. A folder
/// (ending in a separator) is not one.
fn bare(path: &str) -> bool {
    let path = path.trim_end_matches(|c: char| c.is_whitespace() || c.is_control());
    if path.is_empty() || path.ends_with(['/', '\\']) {
        return false;
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let name = match name.as_bytes() {
        [d, b':', ..] if d.is_ascii_alphabetic() => &name[2..],
        _ => name,
    };
    let name = name.split(':').next().unwrap_or(name).trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    !name.is_empty() && !name.contains('.')
}

fn network_path(windows: bool) -> Verdict {
    if windows { Verdict::Block("links.why_network") } else { Verdict::Ask }
}

/// What to do with a link target from a document, before the mode.
pub fn verdict_on(target: &str, windows: bool) -> Verdict {
    let t = target.trim();
    // Bidirectional and invisible marks would make the target shown in
    // the question read as another one.
    let odd = |c: char| c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
    // Decoded too: `evil.exe%00.pdf` would end at the NUL for some openers.
    if t.is_empty() || t.chars().any(odd) || percent_decoded(t).chars().any(odd) {
        return Verdict::Block("links.why_odd");
    }
    // Some openers decode `%5C%5Chost` to `\\host`.
    if on_share(t) || (scheme(t).is_none() && on_share(&percent_decoded(t))) {
        return if runs(t) || runs(&percent_decoded(t)) { Verdict::Block("links.why_program") } else { network_path(windows) };
    }
    let Some(s) = scheme(t) else {
        // A local path.
        let path = percent_decoded(t);
        return if runs(&path) || (windows && bare(&path)) { Verdict::Block("links.why_program") } else { Verdict::Ask };
    };
    match s.as_str() {
        // Some mail programs attach the local file a mailto: link names.
        "mailto" if percent_decoded(t).to_ascii_lowercase().split(['?', '&']).skip(1).any(|p| p.trim_start().starts_with("attach")) => Verdict::Ask,
        "http" | "https" | "mailto" => Verdict::Open,
        "file" => {
            // Cut before decoding: `a%23.exe` names `a#.exe` (and the whole
            // decoded target is checked too, as some openers decode first).
            let path = percent_decoded(t[5..].split(['?', '#']).next().unwrap_or(""));
            let path = path.as_str();
            if runs(path) || runs(&percent_decoded(&t[5..])) || (windows && bare(path)) {
                return Verdict::Block("links.why_program");
            }
            // file://host/share is a network path, as are file:\\host\share
            // and file:////host/share; file:///C:/x and file://localhost/x
            // are local. Mixed leading slashes (file:/\\host\share) are
            // read as a share by some openers.
            let lead: Vec<char> = path.chars().take_while(|c| matches!(c, '/' | '\\')).collect();
            if lead.len() >= 2 && lead.contains(&'\\') {
                return network_path(windows);
            }
            let path = path.replace('\\', "/");
            if apex_sheet::query::is_network_path(&format!("file:{path}")) {
                return network_path(windows);
            }
            match path.strip_prefix("//") {
                Some(p) if p.starts_with("//") => network_path(windows),
                Some(p) if !p.starts_with('/') && !p.to_ascii_lowercase().starts_with("localhost/") => network_path(windows),
                _ => Verdict::Ask,
            }
        }
        s if BAD_SCHEMES.contains(&s) => Verdict::Block("links.why_scheme"),
        _ => Verdict::Ask,
    }
}

/// Why a file a document points at may not be opened in ApexDesk itself
/// (a link back to a fragment's original), None when it may: no files on
/// other computers (on Windows, reaching one signs in there) and only the
/// formats ApexDesk opens.
/// Whether `path` names a file on another computer (`\\\\host\\share`):
/// on Windows, even looking for it there sends the user's sign-in.
pub fn on_share(path: &str) -> bool {
    apex_sheet::query::is_network_path(path) && !path.trim_start().get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:"))
}

pub fn in_app_refusal(path: &str, windows: bool) -> Option<&'static str> {
    let p = path.trim_start();
    if p.chars().any(char::is_control) || p.contains("://") {
        return Some("links.why_odd");
    }
    if windows && on_share(p) {
        return Some("links.why_network");
    }
    apex_io::Format::from_path(std::path::Path::new(p)).is_none().then_some("links.why_program")
}

/// [`verdict_on`] on this system, with the mode applied.
pub fn verdict(target: &str) -> Verdict {
    match (verdict_on(target, cfg!(windows)), MODE.load(Ordering::Relaxed)) {
        (Verdict::Ask, 1) => Verdict::Open,
        (Verdict::Ask, 2) => Verdict::Block("links.why_policy"),
        (v, _) => v,
    }
}

/// A link waiting for the user to say whether to open it.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// Follows a link from a document: opens it, asks first (the answer is
/// [`dialog`]'s), or refuses; the error is for a notice.
pub fn follow(target: &str) -> Result<(), String> {
    let target = target.trim();
    match verdict(target) {
        Verdict::Open => {
            if crate::common::open_external(target) {
                Ok(())
            } else {
                Err(trf("link.open_failed", &[("target", &target)]))
            }
        }
        Verdict::Ask => {
            *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(target.to_string());
            Ok(())
        }
        Verdict::Block(why) => Err(format!("{}\n{target}\n{}", tr("links.blocked"), tr(why))),
    }
}

/// [`follow`] for a plain click that may not have meant a link: a PDF's
/// link areas don't show and can cover the page, so web pages ask too
/// (unless links are set to open without asking).
pub fn follow_asked(target: &str) -> Result<(), String> {
    let target = target.trim();
    if matches!(verdict(target), Verdict::Open) && MODE.load(Ordering::Relaxed) != 1 {
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(target.to_string());
        return Ok(());
    }
    follow(target)
}

/// The question about a pending link, drawn by the window every frame.
pub fn dialog(ctx: &egui::Context) {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = pending.clone() else { return };
    let res = crate::widgets::dialog(ctx, "links-ask", tr("links.ask_title"), 480.0, |ui| {
        ui.label(tr("links.ask"));
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(&target).monospace()).wrap());
        });
        ui.add_space(4.0);
        ui.label(RichText::new(tr("links.ask_note")).weak());
    });
    match res {
        crate::widgets::DialogResult::None => {}
        crate::widgets::DialogResult::Cancel => *pending = None,
        crate::widgets::DialogResult::Ok => {
            *pending = None;
            drop(pending);
            crate::common::open_external(&target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_that_are_programs_are_refused() {
        assert!(copy_runs("Factura", b"%PDF-1.7") && copy_runs("Factura.dat", b"MZ\x90\0") && copy_runs("a.txt", b"\x7fELF"));
        assert!(copy_runs("a.txt", b"#!/bin/sh") && copy_runs("a.cfg", b"# x\n[Desktop Entry]\nExec=x") && copy_runs("x.exe", b""));
        assert!(!copy_runs("Factura.pdf", b"%PDF-1.7") && !copy_runs("datos.csv", b"a,b"));
    }

    fn v(t: &str) -> Verdict {
        verdict_on(t, true)
    }

    #[test]
    fn web_and_mail_open() {
        assert_eq!(v("https://example.com/a?b=1"), Verdict::Open);
        assert_eq!(v("HTTP://example.com"), Verdict::Open);
        assert_eq!(v("mailto:someone@example.com"), Verdict::Open);
        // Some mail programs attach the file named: asked first.
        assert_eq!(v("mailto:a@example.com?subject=x&attach=/home/u/.ssh/id_rsa"), Verdict::Ask);
        assert_eq!(v("file:///C:/x/a%23.exe"), Verdict::Block("links.why_program"));
        assert_eq!(v("file:///C:/x/a%3F.exe"), Verdict::Block("links.why_program"));
        assert_eq!(v("mailto:a@example.com?Attachment=%2Fetc%2Fpasswd"), Verdict::Ask);
        // A download is the browser's to handle.
        assert_eq!(v("https://example.com/setup.exe"), Verdict::Open);
    }

    #[test]
    fn programs_never_open() {
        for t in [
            "C:\\Users\\Public\\invoice.exe",
            "invoice.pdf.exe",
            "run.BAT",
            "..\\tools\\x.ps1",
            "file:///C:/Windows/System32/calc.exe",
            "file:///C:/x/calc%2Eexe",
            "file:///home/u/evil.desktop",
            "C:\\x\\a.lnk",
            "C:\\x\\a.exe.",
            "C:\\x\\a.exe::$DATA",
            "/tmp/x.sh",
            "\\\\server\\share\\setup.msi",
            "tool.pyz",
            "C:\\x\\addin.vsto",
            "/Users/u/Desktop/x.webloc",
        ] {
            assert_eq!(v(t), Verdict::Block("links.why_program"), "{t}");
        }
    }

    #[test]
    fn dangerous_schemes_never_open() {
        for t in ["javascript:alert(1)", "ms-msdt:/id PCWDiagnostic", "search-ms:query=x&crumb=location:\\\\evil", "ms-word:ofe|u|https://x/a.docx", "data:text/html,<b>"] {
            assert_eq!(v(t), Verdict::Block("links.why_scheme"), "{t}");
        }
    }

    #[test]
    fn links_back_open_only_local_documents() {
        assert_eq!(in_app_refusal("\\\\attacker\\share\\a.pdf", true), Some("links.why_network"));
        assert_eq!(in_app_refusal("//attacker/share/a.pdf", true), Some("links.why_network"));
        assert_eq!(in_app_refusal("C:\\x\\tool.exe", true), Some("links.why_program"));
        assert_eq!(in_app_refusal("C:\\x\\notes.zzz", true), Some("links.why_program"));
        assert_eq!(in_app_refusal("C:\\Users\\ana\\report.pdf", true), None);
        assert_eq!(in_app_refusal("/home/ana/report.docx", false), None);
        // Macros stay off in ApexDesk: its own formats open there.
        assert_eq!(in_app_refusal("/home/ana/book.xlsm", false), None);
    }

    #[test]
    fn hidden_or_macro_extensions_count() {
        // Unicode spaces and controls after the extension, as Windows and
        // our own file name cleaning drop them.
        assert!(runs("evil.exe\u{a0}"));
        assert!(runs("evil.exe\u{2003}."));
        assert!(runs("evil.exe\u{0}"));
        assert!(runs("C:payload.exe"));
        assert!(runs("C:\\x\\evil.exe\\"));
        assert!(runs("/Users/u/x.app/"));
        for n in ["a.docm", "a.XLSM", "a.pptm", "a.iqy", "a.rdp", "a.mht", "a.accdb", "a.wsb", "a.one", "a.jnlp", "a.slk"] {
            assert!(runs(n), "{n}");
        }
        assert!(!runs("a.docx"));
        assert!(!runs("a.xlsx"));
    }

    #[test]
    fn network_paths_on_windows() {
        assert_eq!(v("\\\\server\\share\\report.pdf"), Verdict::Block("links.why_network"));
        assert_eq!(v("//server/share/report.pdf"), Verdict::Block("links.why_network"));
        assert_eq!(v("file://server/share/report.pdf"), Verdict::Block("links.why_network"));
        assert_eq!(v("file:\\\\server\\share\\report.pdf"), Verdict::Block("links.why_network"));
        assert_eq!(v("file:////server/share/report.pdf"), Verdict::Block("links.why_network"));
        assert_eq!(v("file://///server/share/report.pdf"), Verdict::Block("links.why_network"));
        assert_eq!(v("file:/\\\\server\\share\\x.pdf"), Verdict::Block("links.why_network"));
        for t in ["\\/server/share/x.pdf", "/\\server\\share\\x.pdf", "\\??\\UNC\\server\\share\\x.pdf", "file:///\\??\\UNC\\server\\share\\x.pdf"] {
            assert_eq!(v(t), Verdict::Block("links.why_network"), "{t}");
        }
        assert_eq!(verdict_on("\\\\server\\share\\report.pdf", false), Verdict::Ask);
    }

    #[test]
    fn encoded_shares_are_network_paths() {
        assert_eq!(v("%5C%5Cserver%5Cshare%5Cx.pdf"), Verdict::Block("links.why_network"));
        assert_eq!(v("%5C%5Cserver%5Cshare%5Cx.exe"), Verdict::Block("links.why_program"));
    }

    #[test]
    fn encoded_controls_are_blocked() {
        assert_eq!(verdict_on("file:///C:/Users/x/Downloads/evil.exe%00.pdf", true), Verdict::Block("links.why_odd"));
        assert_eq!(verdict_on("docs/a%E2%80%AEfdp.exe", false), Verdict::Block("links.why_odd"));
    }

    #[test]
    fn names_without_extension_are_blocked_on_windows() {
        for t in ["C:\\Windows\\System32\\calc", "calc", "file:///C:/Windows/System32/calc", "C:\\x\\calc. ", "C:\\x\\calc:stream"] {
            assert_eq!(v(t), Verdict::Block("links.why_program"), "{t}");
            assert_eq!(verdict_on(t, false), Verdict::Ask, "{t}");
        }
        assert_eq!(v("C:\\Users\\docs\\"), Verdict::Ask);
    }

    #[test]
    fn other_targets_ask() {
        for t in ["C:\\docs\\report.pdf", "report.docx", "file:///C:/docs/a.pdf", "file://localhost/home/u/a.pdf", "tel:+34600000000", "ftp://example.com/a"] {
            assert_eq!(v(t), Verdict::Ask, "{t}");
        }
        assert_eq!(v(""), Verdict::Block("links.why_odd"));
        assert_eq!(v("https://x\n.exe"), Verdict::Block("links.why_odd"));
    }
}
