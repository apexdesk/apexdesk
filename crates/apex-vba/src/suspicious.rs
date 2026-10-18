//! What in a project's code is typical of malware, to warn louder before
//! its macros are enabled: code that runs on open together with running
//! programs, downloading, calling into Windows or hiding what it does.
//! ApexDesk can't run most of it anyway; the file may still reach someone
//! with Excel, and code like this is rarely honest.

use crate::lexer::{Tok, tokenize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Signal {
    /// A macro that runs by itself when the file opens or closes.
    AutoRun,
    /// Starts other programs (Shell, WScript.Shell, PowerShell...).
    RunsPrograms,
    /// Reaches the internet (URLDownloadToFile, XMLHTTP, web addresses).
    Downloads,
    /// Calls Windows functions directly (Declare).
    WindowsApi,
    /// Text built so it can't be read: long runs of Chr, StrReverse,
    /// long encoded strings.
    Obfuscated,
}

impl Signal {
    /// Whether it alone makes the code suspicious (code that runs on open
    /// is common and fine by itself).
    pub fn serious(self) -> bool {
        self != Signal::AutoRun
    }
}

const AUTO_RUN: &[&str] = &["auto_open", "autoopen", "auto_close", "autoclose", "autoexec", "autonew", "workbook_open", "workbook_activate", "workbook_beforeclose", "document_open", "document_close", "document_new"];

const PROGRAMS: &[&str] = &["wscript.shell", "shell.application", "powershell", "cmd.exe", "cmd /c", "cmd /k", "mshta", "rundll32", "regsvr32", "certutil", "bitsadmin", "schtasks", "wmic", "winmgmts", "win32_process", "cscript", "wscript.exe", "/bin/sh", "/bin/bash", "do shell script"];

const NETWORK: &[&str] = &["urldownloadtofile", "xmlhttp", "winhttp", "serverxmlhttp", "internetexplorer.application", "adodb.stream", "\\\\"];

/// A web address is fine (links, documentation) unless it fetches what
/// runs: a program or script.
const FETCHED: &[&str] = &[".exe", ".dll", ".scr", ".bat", ".cmd", ".ps1", ".vbs", ".js", ".hta", ".jar", ".msi", ".sh", ".lnk"];

const PROGRAM_WORDS: &[&str] = &["shell", "shellexecute", "shellexecutea", "shellexecutew", "macscript", "applescripttask"];

const API_WORDS: &[&str] = &["declare", "virtualalloc", "rtlmovememory", "createthread", "callwindowproc", "enumsystemlanguagegroups"];

/// Chr calls in a module from which the code reads as hidden.
const MANY_CHR: usize = 40;
/// A string of only letters, digits, `+/=` this long reads as encoded.
const ENCODED: usize = 120;

/// The signals found in the modules (name, code), each once, in order.
pub fn scan<'a>(modules: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Signal> {
    let mut found = Vec::new();
    for (name, code) in modules {
        match tokenize(code, name) {
            Ok(tokens) => scan_tokens(&tokens.into_iter().map(|t| t.tok).collect::<Vec<_>>(), &mut found),
            // Code the lexer can't read: the raw text, comments and all.
            Err(_) => scan_text(code, &mut found),
        }
    }
    found.sort();
    found.dedup();
    found
}

fn scan_tokens(tokens: &[Tok], found: &mut Vec<Signal>) {
    let mut chr = 0;
    for (i, t) in tokens.iter().enumerate() {
        match t {
            Tok::Word(w) => {
                let w = w.to_ascii_lowercase();
                let prev = i.checked_sub(1).map(|j| &tokens[j]);
                // `Sub Workbook_Open`, not a call of it.
                if AUTO_RUN.contains(&w.as_str()) && matches!(prev, Some(Tok::Word(p)) if p.eq_ignore_ascii_case("sub") || p.eq_ignore_ascii_case("function")) {
                    found.push(Signal::AutoRun);
                }
                // Not `.Shell` of some object's member nor a name being declared.
                let member = matches!(prev, Some(Tok::Sym(".")));
                if !member && PROGRAM_WORDS.contains(&w.as_str()) {
                    found.push(Signal::RunsPrograms);
                }
                if !member && API_WORDS.contains(&w.as_str()) && !(w == "declare" && harmless_declare(&tokens[i + 1..])) {
                    found.push(Signal::WindowsApi);
                }
                if matches!(w.as_str(), "chr" | "chrw" | "chrb") {
                    chr += 1;
                }
                if w == "callbyname" {
                    found.push(Signal::Obfuscated);
                }
            }
            Tok::Str(s) => scan_string(s, found),
            _ => {}
        }
    }
    if chr >= MANY_CHR {
        found.push(Signal::Obfuscated);
    }
}

/// Whether the `Declare` these tokens follow names a function ApexDesk acts
/// out (Sleep, GetTickCount...), which macros use only to wait or time.
fn harmless_declare(tokens: &[Tok]) -> bool {
    let word = |i: usize| match tokens.get(i) {
        Some(Tok::Word(w)) => Some(w.to_ascii_lowercase()),
        _ => None,
    };
    let text = |i: usize| match tokens.get(i) {
        Some(Tok::Str(s)) => Some(s.to_string()),
        _ => None,
    };
    let mut i = usize::from(word(0).as_deref() == Some("ptrsafe"));
    if !matches!(word(i).as_deref(), Some("sub" | "function")) {
        return false;
    }
    let Some(name) = word(i + 1) else { return false };
    i += 2;
    let (Some("lib"), Some(lib)) = (word(i).as_deref(), text(i + 1)) else { return false };
    i += 2;
    let api = if word(i).as_deref() == Some("alias") {
        match text(i + 1) {
            Some(a) => a,
            None => return false,
        }
    } else {
        name
    };
    crate::api::harmless(&lib, &api)
}

fn scan_string(s: &str, found: &mut Vec<Signal>) {
    let before = found.len();
    scan_plain(s, found);
    // Written backwards for StrReverse (reversing a word is common, a
    // command line backwards is not).
    if found.len() == before {
        let reversed: String = s.chars().rev().collect();
        scan_plain(&reversed, found);
        if found.len() > before {
            found.push(Signal::Obfuscated);
        }
    }
    if s.len() >= ENCODED && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
        found.push(Signal::Obfuscated);
    }
}

fn scan_plain(s: &str, found: &mut Vec<Signal>) {
    let lower = s.to_ascii_lowercase();
    if PROGRAMS.iter().any(|p| lower.contains(p)) {
        found.push(Signal::RunsPrograms);
    }
    let web = ["http://", "https://", "ftp://"].iter().any(|p| lower.contains(p));
    let fetches = web && lower.split(|c: char| c.is_whitespace() || c == '"' || c == '?' || c == '#').any(|w| FETCHED.iter().any(|x| w.ends_with(x)));
    if fetches || NETWORK.iter().any(|p| lower.contains(p)) {
        found.push(Signal::Downloads);
    }
}

fn scan_text(code: &str, found: &mut Vec<Signal>) {
    let lower = code.to_ascii_lowercase();
    if AUTO_RUN.iter().any(|a| lower.contains(&format!("sub {a}"))) {
        found.push(Signal::AutoRun);
    }
    scan_string(code, found);
    let words: Vec<&str> = lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).collect();
    if words.iter().any(|w| PROGRAM_WORDS.contains(w)) {
        found.push(Signal::RunsPrograms);
    }
    if words.iter().any(|w| API_WORDS.contains(w)) {
        found.push(Signal::WindowsApi);
    }
    if words.iter().filter(|w| matches!(**w, "chr" | "chrw" | "chrb")).count() >= MANY_CHR {
        found.push(Signal::Obfuscated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(code: &str) -> Vec<Signal> {
        scan([("Module1", code)])
    }

    #[test]
    fn plain_code_is_fine() {
        assert_eq!(one("Sub Workbook_Open()\n    Range(\"A1\").Value = \"Hi\" ' Shell(\"cmd.exe\")\nEnd Sub\n"), [Signal::AutoRun]);
        assert!(one("Sub Main()\n    MsgBox \"hello\"\nEnd Sub\n").is_empty());
        // A member named Shell, a variable named like a function isn't.
        assert!(one("Sub A()\n    x = obj.Shell\nEnd Sub\n").is_empty());
    }

    #[test]
    fn dangerous_code() {
        let code = "Private Declare PtrSafe Function URLDownloadToFile Lib \"urlmon\" Alias \"URLDownloadToFileA\" () As Long\nSub AutoOpen()\n    CreateObject(\"WScript.Shell\").Run \"powershell -enc AAAA\"\n    Shell \"calc\"\n    x = \"https://example.com/a.exe\"\nEnd Sub\n";
        assert_eq!(one(code), [Signal::AutoRun, Signal::RunsPrograms, Signal::Downloads, Signal::WindowsApi]);
    }

    #[test]
    fn harmless_declares() {
        assert!(one("Private Declare PtrSafe Sub Sleep Lib \"kernel32\" (ByVal ms As Long)\nPrivate Declare Function T Lib \"winmm.dll\" Alias \"timeGetTime\" () As Long\n").is_empty());
        // Another function under a harmless name, or a DLL of its own, isn't.
        assert_eq!(one("Private Declare PtrSafe Sub Sleep Lib \"kernel32\" Alias \"VirtualAlloc\" ()\n"), [Signal::WindowsApi]);
        assert_eq!(one("Private Declare PtrSafe Sub Sleep Lib \"evil.dll\" ()\n"), [Signal::WindowsApi]);
    }

    #[test]
    fn hidden_code() {
        let chrs = (0..MANY_CHR).map(|i| format!("Chr({})", 65 + i % 26)).collect::<Vec<_>>().join(" & ");
        assert_eq!(one(&format!("Sub A()\n    x = {chrs}\nEnd Sub\n")), [Signal::Obfuscated]);
        assert_eq!(one("Sub A()\n    x = StrReverse(\"exe.dmc\")\nEnd Sub\n"), [Signal::RunsPrograms, Signal::Obfuscated]);
        // Reversing words, and links to web pages, are ordinary.
        assert!(one("Sub A()\n    x = StrReverse(\"level\")\n    ActiveSheet.Hyperlinks.Add Range(\"A1\"), \"https://example.com/docs\"\nEnd Sub\n").is_empty());
        assert_eq!(one("Sub A()\n    x = \"https://example.com/setup.exe\"\nEnd Sub\n"), [Signal::Downloads]);
        let b64 = "QUJD".repeat(40);
        assert_eq!(one(&format!("Sub A()\n    x = \"{b64}\"\nEnd Sub\n")), [Signal::Obfuscated]);
        // What the lexer can't read is still looked at.
        assert_eq!(one("Sub A()\n    x = \"unterminated\n    Shell y\nEnd Sub\n").contains(&Signal::RunsPrograms), true);
    }
}
