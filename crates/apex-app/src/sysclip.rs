//! The system clipboard in several formats at once, so formatting
//! crosses to other programs and to other ApexDesk windows: text, HTML
//! and RTF for everyone, and ApexDesk's own formats for ApexDesk.
//!
//! X11 (and XWayland, which hands every format on to Wayland programs),
//! Windows and macOS go through `arboard` (vendor/arboard); the Wayland
//! backend through the clipboard egui-winit runs (vendor/smithay-clipboard).
//! Formats are named by MIME type; on Windows `text/html` and `text/rtf`
//! are "HTML Format" and "Rich Text Format".
//!
//! Android's clipboard takes the text; the other formats stay in the
//! program, given back while the clipboard still holds that text.

#[cfg(not(any(test, target_os = "android")))]
use std::sync::Mutex;
use std::time::Duration;

/// Plain text.
pub const TEXT: &str = "text/plain";
/// An HTML fragment.
pub const HTML: &str = "text/html";
/// Rich Text Format.
pub const RTF: &str = "text/rtf";
/// Paragraphs of a document, as a .docx file.
pub const DOC: &str = "application/x-apexdesk-document";
/// Cells of a workbook, as an .xlsx file.
pub const CELLS: &str = "application/x-apexdesk-cells";

/// How long an owner of the clipboard has to answer.
const WAIT: Duration = Duration::from_millis(1500);

#[cfg(not(any(test, target_os = "android")))]
fn arboard() -> std::sync::MutexGuard<'static, Option<arboard::Clipboard>> {
    static CLIP: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);
    let mut c = CLIP.lock().unwrap_or_else(|e| e.into_inner());
    if c.is_none() {
        *c = arboard::Clipboard::new().ok();
    }
    c
}

/// What was last copied on Android, in each format.
#[cfg(all(target_os = "android", not(test)))]
static OWN: std::sync::Mutex<Vec<(String, Vec<u8>)>> = std::sync::Mutex::new(Vec::new());

// Tests never touch the desktop's clipboard: one per test (thread) stands
// for it.
#[cfg(test)]
thread_local! {
    static FAKE: std::cell::RefCell<Vec<(String, Vec<u8>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Puts the same content on the clipboard in each of `formats`. `false`
/// when the system's clipboard couldn't take them (the caller then puts
/// the text there as before).
pub fn write(formats: &[(&str, &[u8])]) -> bool {
    #[cfg(test)]
    {
        FAKE.with(|f| *f.borrow_mut() = formats.iter().map(|(n, b)| (n.to_string(), b.to_vec())).collect());
        true
    }
    #[cfg(not(test))]
    {
        #[cfg(target_os = "linux")]
        if smithay_clipboard::active() {
            return smithay_clipboard::store_formats(formats.iter().map(|(n, b)| (n.to_string(), b.to_vec())).collect());
        }
        #[cfg(target_os = "android")]
        {
            let Some((_, text)) = formats.iter().find(|(n, _)| *n == TEXT) else { return false };
            crate::android::set_clipboard_text(&text_of(text));
            *OWN.lock().unwrap_or_else(|e| e.into_inner()) = formats.iter().map(|(n, b)| (n.to_string(), b.to_vec())).collect();
            true
        }
        #[cfg(not(target_os = "android"))]
        arboard().as_mut().is_some_and(|c| c.set().formats(formats).is_ok())
    }
}

/// The first of `names` the clipboard holds, with the name it was found
/// under. RTF is looked for under its other name too.
pub fn read(names: &[&str]) -> Option<(String, Vec<u8>)> {
    let mut all: Vec<&str> = Vec::with_capacity(names.len() + 1);
    for &n in names {
        all.push(n);
        if n == RTF {
            all.push("application/rtf");
        }
    }
    #[cfg(test)]
    {
        let got = FAKE.with(|f| all.iter().find_map(|n| f.borrow().iter().find(|(f, _)| f == n).cloned()));
        let _ = WAIT;
        got.map(|(n, b)| (if n == "application/rtf" { RTF.to_string() } else { n }, b))
    }
    #[cfg(not(test))]
    {
        #[cfg(target_os = "linux")]
        let got = if smithay_clipboard::active() { smithay_clipboard::load_format(&all, WAIT).ok() } else { arboard().as_mut().and_then(|c| c.get().format(&all).ok()) };
        // Ours while the clipboard holds the text we put there.
        #[cfg(target_os = "android")]
        let got = {
            let own = OWN.lock().unwrap_or_else(|e| e.into_inner());
            let ours = crate::android::clipboard_text().is_some_and(|t| own.iter().any(|(n, b)| n == TEXT && text_of(b) == t));
            if ours { all.iter().find_map(|n| own.iter().find(|(f, _)| f == n).cloned()) } else { None }
        };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let got = arboard().as_mut().and_then(|c| c.get().format(&all).ok());
        let _ = WAIT;
        got.map(|(n, b)| (if n == "application/rtf" { RTF.to_string() } else { n }, b))
    }
}

/// The formats the test clipboard holds.
#[cfg(test)]
pub fn formats() -> Vec<String> {
    FAKE.with(|f| f.borrow().iter().map(|(n, _)| n.clone()).collect())
}

/// Text read from the clipboard in another format: UTF-16 with its byte
/// order mark (Firefox's HTML on Linux), UTF-8, else Latin-1.
pub fn text_of(bytes: &[u8]) -> String {
    let utf16 = |le: bool, b: &[u8]| {
        let units: Vec<u16> = b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units).trim_end_matches('\0').to_string()
    };
    if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(true, b);
    }
    if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(false, b);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_in_its_encodings() {
        assert_eq!(text_of("<b>é</b>".as_bytes()), "<b>é</b>");
        assert_eq!(text_of(&[0xFF, 0xFE, b'<', 0, 0xE9, 0, b'>', 0]), "<é>");
        assert_eq!(text_of(&[b'a', 0xE9, 0]), "aé");
    }
}
