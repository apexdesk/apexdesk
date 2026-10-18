//! The macro corpus: hundreds of macros as people write them (from public
//! examples and our own), and hostile ones as malware writes them, each run
//! in a folder of its own. Every macro must leave what is outside its
//! folder as it was; the ordinary ones must work, the hostile ones be
//! stopped and flagged by the scanner. The report lists what failed and
//! the features macros missed most.
//!
//! A corpus file (tests/corpus/<group>/<name>.bas) is one module with a
//! header of `' corpus-<key>: <value>` lines:
//! - `kind`: `excel` (default) or `word`;
//! - `expect`: `ok` (runs to the end), `blocked` (hostile: whatever it does,
//!   nothing outside its folder changes), `gap` (a known missing feature:
//!   reported, not failed);
//! - `entry`: the macro to run (default `Main`);
//! - `prints`: a line `Debug.Print` must print, in order, trimmed (repeatable);
//! - `allow-files`: `yes` answers yes when it asks to change files;
//! - `trusted`: `yes` runs it as from a trusted location;
//! - `flagged`: `yes` (the scanner must call it suspicious) or `no`;
//! - `may-change`: `yes` when changing files outside is what it is allowed
//!   to do (trusted, with consent) and not an escape;
//! - `source`: where it came from (a URL and its license, or `generated`);
//! - `input`: the answer to the InputBoxes (repeatable, in order);
//! - `answer`: the MsgBox button pressed (repeatable, in order).
//!
//! `'--- module <Name> <standard|class|document>` starts another module.
//! In the code `%BOOK%` is the workbook's folder, `%OUTSIDE%` a folder
//! beside it with files the macro must not reach, `%HOME%` the home
//! folder the macro sees (a fake one).
//!
//! `APEX_CORPUS=<substring>` runs only the files whose path contains it.

mod common;

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use apex_doc::{Document, Pos, Selection};
use apex_vba::files::{BookFormat, DocFormat, FileAccess, FilePick};
use apex_vba::{ModuleKind, Options, Project, Source, Ui};

#[derive(Default)]
struct Case {
    word: bool,
    expect: String,
    entry: String,
    prints: Vec<String>,
    allow: bool,
    trusted: bool,
    flagged: Option<bool>,
    may_change: bool,
    inputs: VecDeque<String>,
    answers: VecDeque<i32>,
    modules: Vec<Source>,
}

fn parse_case(text: &str, book: &Path, outside: &Path, home: &Path) -> Result<Case, String> {
    let mut c = Case { expect: "ok".into(), entry: "Main".into(), ..Default::default() };
    let text = text.replace("%BOOK%", &book.display().to_string()).replace("%OUTSIDE%", &outside.display().to_string()).replace("%HOME%", &home.display().to_string());
    let mut name = "Module1".to_string();
    let mut kind = ModuleKind::Standard;
    let mut code = String::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("'--- module ") {
            c.modules.push(Source { name: std::mem::take(&mut name), kind, code: std::mem::take(&mut code) });
            let mut it = rest.split_whitespace();
            name = it.next().ok_or("module without a name")?.to_string();
            kind = match it.next().unwrap_or("standard") {
                "class" => ModuleKind::Class,
                "document" => ModuleKind::Document,
                "standard" => ModuleKind::Standard,
                k => return Err(format!("unknown module kind {k}")),
            };
            continue;
        }
        if let Some(rest) = t.strip_prefix("' corpus-") {
            let (k, v) = rest.split_once(':').ok_or_else(|| format!("bad header: {t}"))?;
            let v = v.trim().to_string();
            match k.trim() {
                "kind" => c.word = v == "word",
                "expect" => c.expect = v,
                "entry" => c.entry = v,
                "prints" => c.prints.push(v),
                "allow-files" => c.allow = v == "yes",
                "trusted" => c.trusted = v == "yes",
                "flagged" => c.flagged = Some(v == "yes"),
                "may-change" => c.may_change = v == "yes",
                "input" => c.inputs.push_back(v),
                "answer" => c.answers.push_back(v.parse().map_err(|_| format!("bad answer: {v}"))?),
                "source" | "note" => {}
                k => return Err(format!("unknown header key {k}")),
            }
            continue;
        }
        // Exported modules start with attributes the editor hides.
        if t.starts_with("Attribute VB_") {
            continue;
        }
        code.push_str(line);
        code.push('\n');
    }
    c.modules.push(Source { name, kind, code });
    if !["ok", "blocked", "gap"].contains(&c.expect.as_str()) {
        return Err(format!("unknown expect {}", c.expect));
    }
    Ok(c)
}

/// Answers dialogs from the header; saving writes a small file, as the
/// application would write the real one.
struct CorpusUi {
    allow: bool,
    inputs: VecDeque<String>,
    answers: VecDeque<i32>,
    printed: Vec<String>,
}

impl Ui for CorpusUi {
    fn msg_box(&mut self, _prompt: &str, _buttons: i32, _title: &str) -> i32 {
        self.answers.pop_front().unwrap_or(1)
    }

    fn input_box(&mut self, _prompt: &str, _title: &str, default: &str) -> Option<String> {
        Some(self.inputs.pop_front().unwrap_or_else(|| default.to_string()))
    }

    fn debug_print(&mut self, text: &str) {
        self.printed.push(text.to_string());
    }

    fn allow_file_changes(&mut self, _folder: &str) -> bool {
        self.allow
    }

    fn pick_files(&mut self, _pick: &FilePick) -> Option<Vec<String>> {
        None
    }

    fn save_workbook(&mut self, wb: &apex_sheet::Workbook, path: &str, _format: BookFormat, sheet: usize) -> Result<(), String> {
        fs::write(path, wb.sheets.get(sheet).map(|s| s.name.as_bytes()).unwrap_or_default()).map_err(|e| e.to_string())
    }

    fn save_document(&mut self, doc: &Document, path: &str, _format: DocFormat) -> Result<(), String> {
        let text: Vec<String> = doc.paragraphs.iter().map(|p| p.text.to_string()).collect();
        fs::write(path, text.join("\n")).map_err(|e| e.to_string())
    }

    fn open_document(&mut self, path: &str) -> Result<Document, String> {
        fs::read_to_string(path).map(|t| Document::from_plain_text(&t)).map_err(|e| e.to_string())
    }

    fn open_workbook(&mut self, _path: &str, _password: Option<&str>) -> Result<apex_sheet::Workbook, String> {
        Ok(common::workbook())
    }
}

/// Every file below `dir` with its contents (what must not change).
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.insert(p.clone(), Vec::new());
                stack.push(p);
            } else {
                out.insert(p.clone(), fs::read(&p).unwrap_or_default());
            }
        }
    }
    out
}

/// A fresh folder: the workbook's, one beside it with files to protect,
/// and a fake home with what malware looks for.
fn stage(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let _ = fs::remove_dir_all(root);
    let (book, outside, home) = (root.join("book"), root.join("outside"), root.join("home"));
    fs::create_dir_all(book.join("data")).unwrap();
    fs::write(book.join("data/input.txt"), "alpha\nbeta\ngamma\n").unwrap();
    fs::write(book.join("data/table.csv"), "Name,Qty\nApples,3\nPears,5\n").unwrap();
    fs::create_dir_all(outside.join("private")).unwrap();
    fs::write(outside.join("secret.txt"), "do not touch").unwrap();
    fs::write(outside.join("private/notes.docx"), "x").unwrap();
    for d in ["Documents", "Desktop", ".ssh", ".config/autostart", "AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup"] {
        fs::create_dir_all(home.join(d)).unwrap();
    }
    fs::write(home.join(".ssh/id_rsa"), "key").unwrap();
    fs::write(home.join(".bashrc"), "# rc").unwrap();
    fs::write(home.join("Documents/report.docx"), "x").unwrap();
    (book, outside, home)
}

/// Code that names a place outside the staged folders: a corpus file must
/// reach the disk only through `%BOOK%`, `%OUTSIDE%` and `%HOME%`, so a
/// gap in the sandbox can't touch the machine running the tests.
fn names_real_path(code: &str, root: &Path) -> Option<String> {
    let root = root.display().to_string();
    for lit in code.split('"').skip(1).step_by(2) {
        let l = lit.trim();
        let absolute = l.starts_with('/') || l.starts_with("\\\\") || (l.len() > 2 && l.as_bytes()[1] == b':' && l.as_bytes()[0].is_ascii_alphabetic() && matches!(l.as_bytes()[2], b'\\' | b'/'));
        if absolute && !l.starts_with(&root) {
            return Some(l.to_string());
        }
    }
    None
}

#[derive(Default)]
struct Tally {
    total: usize,
    passed: usize,
    gaps: usize,
    failures: Vec<String>,
    escapes: Vec<String>,
    unflagged: Vec<String>,
    false_alarms: Vec<String>,
    slow: Vec<String>,
    missing: BTreeMap<String, usize>,
    by_group: BTreeMap<String, (usize, usize)>,
}

#[test]
fn macro_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "bas") {
                files.push(p);
            }
        }
    }
    files.sort();
    if let Ok(only) = std::env::var("APEX_CORPUS") {
        files.retain(|p| p.display().to_string().contains(&only));
    }
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("macro-corpus");
    let home = work.join("run/home");
    // What Environ and the home folder give the macros: the fake one.
    // SAFETY: this binary's only test sets them, before running macros.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("APPDATA", home.join("AppData/Roaming"));
        std::env::set_var("TEMP", work.join("run/book"));
        std::env::set_var("TMP", work.join("run/book"));
    }
    let mut t = Tally::default();
    for path in &files {
        let rel = path.strip_prefix(&dir).unwrap_or(path).display().to_string();
        let group = rel.split('/').next().unwrap_or("").to_string();
        t.total += 1;
        let root = work.join("run");
        let (book, outside, home) = stage(&root);
        let text = fs::read_to_string(path).unwrap();
        let case = match parse_case(&text, &book, &outside, &home) {
            Ok(c) => c,
            Err(e) => {
                t.failures.push(format!("{rel}: header: {e}"));
                continue;
            }
        };
        let all_code: String = case.modules.iter().map(|m| m.code.as_str()).collect();
        if let Some(p) = names_real_path(&all_code, &root) {
            t.failures.push(format!("{rel}: names a real path ({p}); use %BOOK%, %OUTSIDE% or %HOME%"));
            continue;
        }
        // The scanner, as when the file opens.
        let serious = apex_vba::suspicious::scan(case.modules.iter().map(|m| (m.name.as_str(), m.code.as_str()))).iter().any(|s| s.serious());
        match case.flagged {
            Some(true) if !serious => t.unflagged.push(rel.clone()),
            Some(false) if serious => t.false_alarms.push(rel.clone()),
            _ => {}
        }
        let before_out = snapshot(&outside);
        let before_home = snapshot(&home);
        let mut ui = CorpusUi { allow: case.allow, inputs: case.inputs.clone(), answers: case.answers.clone(), printed: Vec::new() };
        let opts = Options {
            file_path: book.display().to_string(),
            file_name: if case.word { "Document1.docm".into() } else { "Book1.xlsm".into() },
            user_name: "Corpus".into(),
            max_steps: 5_000_000,
            max_time: Duration::from_secs(5),
            files: FileAccess { trusted: case.trusted, write: None },
            ..Options::default()
        };
        let started = std::time::Instant::now();
        let error = match Project::parse(&case.modules) {
            Err(e) => Some(format!("parse: {} line {}: {e}", e.module, e.line)),
            Ok(p) => {
                let err = if case.word {
                    let mut doc = Document::from_plain_text("The quick brown fox.\nJumps over the lazy dog.\nThe end.");
                    apex_vba::run_doc_macro(&p, &mut doc, Selection::caret(Pos::new(0, 0)), &mut ui, &opts, &case.entry).error
                } else {
                    let mut wb = common::workbook();
                    apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, &case.entry).error
                };
                err.map(|e| {
                    if e.number == 445 {
                        *t.missing.entry(e.message.trim_start_matches("Not supported in ApexDesk: ").to_string()).or_default() += 1;
                    }
                    format!("error {} at {} line {}: {}", e.number, e.module, e.line, e.message)
                })
            }
        };
        // The one thing no macro may do.
        let took = started.elapsed();
        if took > Duration::from_secs(2) {
            t.slow.push(format!("{rel}: {:.1} s", took.as_secs_f32()));
        }
        if !case.may_change && (snapshot(&outside) != before_out || snapshot(&home) != before_home) {
            t.escapes.push(format!("{rel}: changed files outside its folder ({})", error.as_deref().unwrap_or("ran to the end")));
        }
        let ok = match case.expect.as_str() {
            "ok" => match &error {
                Some(e) => Err(e.clone()),
                // Compared trimmed: VBA pads numbers with spaces.
                None if !case.prints.is_empty() && ui.printed.iter().map(|l| l.trim()).ne(case.prints.iter().map(|l| l.trim())) => Err(format!("printed {:?}, expected {:?}", ui.printed, case.prints)),
                None => Ok(()),
            },
            "gap" => {
                t.gaps += 1;
                Ok(())
            }
            _ => Ok(()),
        };
        let g = t.by_group.entry(group).or_default();
        g.0 += 1;
        match ok {
            Ok(()) => {
                t.passed += 1;
                g.1 += 1;
            }
            Err(e) => t.failures.push(format!("{rel}: {e}")),
        }
    }
    let mut r = String::new();
    let _ = writeln!(r, "# Macro corpus\n\n{} macros, {} as expected ({} known gaps).\n", t.total, t.passed, t.gaps);
    let _ = writeln!(r, "| group | macros | as expected |\n|---|---|---|");
    for (g, (n, ok)) in &t.by_group {
        let _ = writeln!(r, "| {g} | {n} | {ok} |");
    }
    let section = |r: &mut String, title: &str, items: &[String]| {
        if !items.is_empty() {
            let _ = writeln!(r, "\n## {title} ({})\n", items.len());
            for i in items {
                let _ = writeln!(r, "- {i}");
            }
        }
    };
    section(&mut r, "ESCAPED THE SANDBOX", &t.escapes);
    section(&mut r, "Failed", &t.failures);
    section(&mut r, "Hostile, not flagged by the scanner", &t.unflagged);
    section(&mut r, "Flagged though harmless", &t.false_alarms);
    section(&mut r, "Slow (over 2 s)", &t.slow);
    if !t.missing.is_empty() {
        let mut m: Vec<_> = t.missing.iter().collect();
        m.sort_by(|a, b| b.1.cmp(a.1));
        let _ = writeln!(r, "\n## Not supported, most asked first\n");
        for (what, n) in m {
            let _ = writeln!(r, "- {n} × {what}");
        }
    }
    let report = work.join("report.md");
    let _ = fs::write(&report, &r);
    println!("{r}\nReport: {}", report.display());
    assert!(t.escapes.is_empty(), "macros changed files outside their folder:\n{}", t.escapes.join("\n"));
    assert!(t.failures.is_empty() && t.unflagged.is_empty() && t.false_alarms.is_empty(), "see {}", report.display());
}
