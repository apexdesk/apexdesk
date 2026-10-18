//! Translations (`crates/apex-app/i18n/<code>.txt`), read against the
//! sources and against what the screens show:
//! - **missing**: a key the base language (en) has, or the sources use,
//!   that a language lacks;
//! - **orphan**: a key of the base language no source uses (by its name,
//!   or by a prefix a key is built from);
//! - **placeholders**: a translation whose `{name}`s differ from the
//!   base language's;
//! - **brand**: a value that names another suite's features (Ribbon,
//!   SmartArt, WordArt, PivotTable);
//! - **raw-key**: a key's name drawn on a screen or given as a control's
//!   name instead of its text;
//! - values the same as in English are only listed in the notes (many
//!   rightly are: names, units, formats).

use apex_fs::PathExt;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::common::{self, Sink};
use super::scenes::{self, WIDE};
use crate::app::audit;
use crate::i18n::langs;

/// Other suites' names for their features.
const BRANDS: [&str; 4] = ["ribbon", "smartart", "wordart", "pivottable"];

/// Words that are the same in every language: names, formats, units.
const UNIVERSAL: &[&str] = &[
    "apexdesk", "ok", "pdf", "csv", "html", "xml", "json", "markdown", "excel", "word", "powerpoint", "libreoffice", "office", "url", "email", "e-mail", "id", "sql", "sqlite", "mysql", "postgresql", "odbc", "vba", "mcp", "zoom", "online", "web", "internet", "wi-fi", "unicode", "utf-8", "ascii", "emoji", "latex", "mathml", "svg", "png", "jpeg", "gif", "tiff", "bmp", "webp", "ods", "odt", "odp", "docx", "xlsx", "pptx", "rtf", "epub", "txt", "cm", "mm", "in", "pt", "px", "dpi", "qr", "isbn", "iban", "x", "y", "z", "a", "b", "c", "info", "normal", "total", "text", "standard", "design", "layout", "tab", "status", "format", "formula", "filter", "logo", "menu", "moderne", "modern", "radio", "video", "audio", "error", "chat", "cloud", "hash", "token", "cache", "script", "macro", "macros", "python", "java", "rust", "javascript", "gantt", "bing", "google", "wikipedia", "github", "gstreamer", "ffmpeg", "pdfium", "opentype", "truetype", "ctrl", "shift", "alt", "enter", "esc", "tab", "f1", "f2", "f3", "f4", "f5", "f7", "f9", "f11", "f12",
];

/// The sources that use keys: every crate's.
fn sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut dirs: Vec<PathBuf> = apex_fs::read_dir(&root).map(|r| r.flatten().map(|e| e.path().join("src")).filter(|p| p.fs_is_dir()).collect()).unwrap_or_default();
    let mut out = Vec::new();
    while let Some(d) = dirs.pop() {
        for e in apex_fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.fs_is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "rs")
                && let Ok(t) = apex_fs::read_to_string(&p)
            {
                out.push((p, t));
            }
        }
    }
    out.sort();
    out
}

/// The keys the sources ask for by name: a literal given to `tr` or
/// `trf` in the crates that translate with these tables (keys held in
/// tables are only counted as referenced, below).
fn used(files: &[(PathBuf, String)]) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let ours = |p: &Path| ["apex-app", "apex-tui", "apex-i18n"].iter().any(|c| p.components().any(|x| x.as_os_str() == *c));
    for (_, text) in files.iter().filter(|(p, _)| ours(p)) {
        for pat in ["tr(\"", "trf(\""] {
            for (i, _) in text.match_indices(pat) {
                if text[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                    continue;
                }
                let rest = &text[i + pat.len()..];
                if let Some(end) = rest.find('"') {
                    keys.insert(rest[..end].to_string());
                }
            }
        }
    }
    keys
}

/// Whether some source refers to `key`: by its name, or by a prefix a
/// key is built from (`format!("fn.{name}")`, `concat!("menu.", …)`).
fn referenced(key: &str, literals: &HashSet<&str>, prefixes: &[&str]) -> bool {
    literals.contains(key) || prefixes.iter().any(|p| key.starts_with(p))
}

/// The real string literals of `text`: comments, char literals (`'"'` is
/// one apostrophe, not a string's edge) and escapes inside a string are
/// skipped, so a `"` in any of them can't throw off where the next
/// literal starts (naively splitting on `"` did, and called keys still
/// in real use "orphans"). Raw strings (`r"…"`, `r#"…"#`) are read too.
fn string_literals(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                let mut depth = 1;
                while i < n && depth > 0 {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'\'' => {
                // A char literal (`'x'`, `'\''`) or a lifetime (`'a`):
                // only the former can hide a `"`.
                let mut j = i + 1;
                if b.get(j) == Some(&b'\\') {
                    j += 1;
                }
                j += 1;
                i = if j > i + 1 && b.get(j) == Some(&b'\'') { j + 1 } else { i + 1 };
            }
            b'r' if matches!(b.get(i + 1), Some(b'"') | Some(b'#')) => {
                let mut j = i + 1;
                let mut hashes = 0usize;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) == Some(&b'"') {
                    j += 1;
                    let start = j;
                    let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
                    while j < n && !b[j..].starts_with(close.as_slice()) {
                        j += 1;
                    }
                    out.push(&text[start..j.min(n)]);
                    i = (j + close.len()).min(n);
                } else {
                    i += 1;
                }
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < n && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                out.push(&text[start..i.min(n)]);
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

fn holes(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s.split('{').skip(1).filter_map(|x| x.split_once('}').map(|(n, _)| n.to_string())).collect();
    v.sort();
    v.dedup();
    v
}

/// Whether a value may rightly read as in English: no words, or only
/// names, acronyms and words every language uses.
fn universal(v: &str) -> bool {
    let words: Vec<String> = v.split(|c: char| !(c.is_alphanumeric() || c == '-')).filter(|w| w.chars().any(char::is_alphabetic)).map(str::to_lowercase).collect();
    words.is_empty()
        || v.chars().filter(|c| c.is_alphabetic()).count() <= 3
        || words.iter().all(|w| UNIVERSAL.contains(&w.as_str()) || w.chars().all(|c| !c.is_alphabetic() || c.is_uppercase()))
        || v.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).all(|w| w.chars().all(|c| !c.is_lowercase()))
}

fn tables(sink: &mut Sink) {
    let all = langs();
    let base = &all[0];
    let files = sources();
    let used = used(&files);
    // Missing: what the base or the sources have that a language lacks.
    for l in all {
        let mut wanted: BTreeSet<&str> = used.iter().map(String::as_str).collect();
        if l.code != base.code {
            for k in base.keys() {
                wanted.insert(k);
            }
        }
        for k in wanted.into_iter().filter(|k| !k.starts_with("test.")) {
            sink.check();
            if l.get(k).is_none() {
                let what = if base.get(k).is_none() { "used in the sources, in no language (the key's name shows)" } else { "shows in English" };
                sink.fail(common::id(&["missing", l.code, k]), format!("[missing] {}: key {k} has no text ({what}). Fix: add it to crates/apex-app/i18n/{}.txt", l.code, l.code));
            }
        }
    }
    // Orphans.
    let literals: HashSet<&str> = files.iter().flat_map(|(_, t)| string_literals(t)).collect();
    let mut prefixes: Vec<&str> = literals
        .iter()
        .filter_map(|l| {
            // A key built from a prefix: "fn.{}", "menu." + name.
            let p = l.split('{').next().unwrap_or(l);
            (p.contains('.') && (p.len() < l.len() || p.ends_with('.') || p.ends_with('_')) && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')).then_some(p)
        })
        .filter(|p| p.len() >= 3)
        .collect();
    prefixes.sort();
    prefixes.dedup();
    let mut keys: Vec<&str> = base.keys().collect();
    keys.sort();
    for k in keys.iter().filter(|k| !k.starts_with("test.") && !k.starts_with("lang.")) {
        sink.check();
        if !referenced(k, &literals, &prefixes) {
            sink.fail(common::id(&["orphan", k]), format!("[orphan] key {k} (\"{}\") is used by no source. Fix: take it out of every language, or use it", scenes::short(base.get(k).unwrap_or(""))));
        }
    }
    // Placeholders, brands, and values as in English.
    for l in all {
        let mut same = Vec::new();
        let mut lkeys: Vec<&str> = l.keys().collect();
        lkeys.sort();
        for k in lkeys {
            if k.starts_with("fn.") || k.starts_with("test.") {
                continue;
            }
            let v = l.get(k).unwrap_or("");
            let low = v.to_lowercase();
            sink.check();
            for b in BRANDS {
                if low.contains(b) {
                    sink.fail(common::id(&["brand", l.code, k]), format!("[brand] {}: key {k} says \"{}\" (another suite's name for it)", l.code, scenes::short(v)));
                }
            }
            if l.code == base.code {
                continue;
            }
            if let Some(b) = base.get(k) {
                sink.check();
                if !matches!(k, "doc.date_format" | "doc.time_format") && holes(b) != holes(v) {
                    sink.fail(common::id(&["placeholders", l.code, k]), format!("[placeholders] {}: key {k} has {:?}, English {:?}", l.code, holes(v), holes(b)));
                }
                if v == b && !universal(v) {
                    same.push(k);
                }
            }
        }
        if !same.is_empty() {
            let first: Vec<String> = same.iter().take(30).map(|k| format!("{k} ({})", scenes::short(l.get(k).unwrap_or("")))).collect();
            sink.note(format!("{}: {} values as in English (a warning only), the first: {}", l.code, same.len(), first.join(", ")));
        }
    }
    sink.note(format!("{} source files, {} keys used by name, {} keys in English", files.len(), used.len(), base.keys().count()));
}

/// Whether `s` reads as a key's name: `prefix.name` with a known prefix.
fn raw_key(s: &str, prefixes: &HashSet<&str>) -> bool {
    let s = s.trim().trim_end_matches(['…', ':', '.']);
    let Some((pre, rest)) = s.split_once('.') else { return false };
    prefixes.contains(pre) && !rest.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// The screens: no key's name drawn, or given as a control's name.
fn screens(sink: &mut Sink) {
    audit::panic_hook();
    let base = &langs()[0];
    let prefixes: HashSet<&str> = base.keys().filter_map(|k| k.split_once('.').map(|x| x.0)).collect();
    for scene in scenes::all() {
        audit::watch(Some(format!("i18n {}", scene.name)));
        let Ok(Ok(rig)) = audit::quietly(|| scenes::open(&scene, WIDE)) else {
            audit::drop_context();
            crate::toolstrip::test::reset();
            continue;
        };
        let mut seen = HashSet::new();
        let drawn = scenes::texts(&rig).into_iter().map(|t| t.text);
        let names = rig.controls.iter().map(|c| c.key.1.clone());
        for s in drawn.chain(names) {
            sink.check();
            for piece in s.split_whitespace() {
                if raw_key(piece, &prefixes) && seen.insert(piece.to_string()) {
                    sink.fail(common::id(&["raw-key", &scene.name, piece]), format!("[raw-key] {}: \"{piece}\" shows instead of its text (in \"{}\")", scene.name, scenes::short(&s)));
                }
            }
        }
    }
    audit::watch(None);
}

fn run_job(job: &str, sink: &mut Sink) {
    match job {
        "tables" => tables(sink),
        _ => {
            crate::i18n::set(job);
            screens(sink);
        }
    }
}

#[test]
#[ignore]
fn qa_i18n() {
    // The screens in English: a key missing there shows its name in
    // every language.
    common::run("i18n", "app::qa::i18n::qa_i18n", &["tables".to_string(), "en".to_string()], run_job);
}
