//! `apex-bench run`: open, save and reopen every file of a corpus, each in
//! a child process with a time and memory limit (see `run/worker.rs`),
//! and report what fails or gets lost (target/bench/run/report.md and
//! results.jsonl, one line per file).

mod child;
mod diff;
mod external;
mod judge;
mod model;
mod pages;
mod report;
mod result;
mod worker;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use result::{Diff, FileResult, Outcome, Target};
use worker::Event;

pub use judge::main as judge_main;
pub use worker::main as worker_main;

const USAGE: &str = "Usage: apex-bench run DIR [options]
       apex-bench run --compare OLD.jsonl NEW.jsonl

Opens every file of DIR (the files of DIR/manifest.tsv when there is one,
see tools/corpus.sh), saves it in its format and the other family's
(xlsx<->ods, docx<->odt; xls, xlsb, doc and rtf to both; csv to csv),
reads that back and compares; documents also compare our page count.

Options:
  --out DIR          report directory (default: target/bench/run)
  -j N               files at a time (default 4, at most 6)
  --timeout SECS     per file (default 60)
  --mem MB           memory limit per process (default 3072)
  --limit N          only the first N files
  --filter TEXT      only files whose path contains TEXT
  --no-pages         don't count pages (it runs apexdesk info)
  --apexdesk PATH    apexdesk binary (default: next to apex-bench)
  --external         check with LibreOffice and OnlyOffice too (slow)
  --sample N         files checked with --external (default 40)
  --compare OLD      compare with an earlier results.jsonl
  --keep             keep the saved files (OUT/work)";

struct Opts {
    dir: PathBuf,
    out: PathBuf,
    jobs: usize,
    timeout: Duration,
    mem_mb: u64,
    limit: Option<usize>,
    filter: Option<String>,
    pages: bool,
    apexdesk: Option<String>,
    external: bool,
    sample: usize,
    compare: Option<PathBuf>,
    keep: bool,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        dir: PathBuf::new(),
        out: PathBuf::from("target/bench/run"),
        jobs: 4,
        timeout: Duration::from_secs(60),
        mem_mb: 3072,
        limit: None,
        filter: None,
        pages: true,
        apexdesk: None,
        external: false,
        sample: 40,
        compare: None,
        keep: false,
    };
    let mut pos = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        let num = |v: String| v.parse::<u64>().map_err(|_| format!("{a}: not a number: {v}"));
        match a.as_str() {
            "--out" => o.out = PathBuf::from(val()?),
            "-j" | "--jobs" => o.jobs = num(val()?)?.clamp(1, 6) as usize,
            "--timeout" => o.timeout = Duration::from_secs(num(val()?)?),
            "--mem" => o.mem_mb = num(val()?)?,
            "--limit" => o.limit = Some(num(val()?)? as usize),
            "--filter" => o.filter = Some(val()?),
            "--no-pages" => o.pages = false,
            "--apexdesk" => o.apexdesk = Some(val()?),
            "--external" => o.external = true,
            "--sample" => o.sample = num(val()?)? as usize,
            "--compare" => o.compare = Some(PathBuf::from(val()?)),
            "--keep" => o.keep = true,
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n\n{USAGE}")),
            _ => pos.push(a.clone()),
        }
    }
    match pos.as_slice() {
        [d] => o.dir = PathBuf::from(d),
        _ => return Err(USAGE.into()),
    }
    Ok(o)
}

pub fn main(args: &[String]) -> Result<(), String> {
    // `--compare OLD NEW`: only compare two results files.
    if let [flag, old, new] = args
        && flag == "--compare"
        && new.ends_with(".jsonl")
    {
        print!("{}", report::compare(&report::read_jsonl(Path::new(old))?, &report::read_jsonl(Path::new(new))?));
        return Ok(());
    }
    let o = parse(args)?;
    let mut files = list_files(&o.dir)?;
    if let Some(f) = &o.filter {
        files.retain(|(p, _)| p.contains(f.as_str()));
    }
    if let Some(n) = o.limit {
        files.truncate(n);
    }
    let old = match &o.compare {
        Some(p) => Some(report::read_jsonl(p)?),
        None => None,
    };
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let out = std::fs::canonicalize(&o.out).map_err(|e| e.to_string())?;
    let work = out.join("work");
    let _ = std::fs::remove_dir_all(&work);
    let mut notes = Vec::new();
    let apexdesk = if o.pages { pages::find(o.apexdesk.as_deref()) } else { None };
    match (&apexdesk, o.pages) {
        (Some(p), _) => notes.push(format!("Pages counted with `{} info`.", p.display())),
        (None, true) => notes.push("Pages not counted: no apexdesk binary next to apex-bench (build it, or give --apexdesk).".into()),
        (None, false) => notes.push("Pages not counted (--no-pages).".into()),
    }
    // Files for the external check: spread over the corpus, not CSV.
    let sampled: Vec<bool> = {
        let eligible: Vec<usize> = files.iter().enumerate().filter(|(_, (p, _))| !p.to_ascii_lowercase().ends_with(".csv") && !worker::targets(Path::new(p)).is_empty()).map(|(i, _)| i).collect();
        let mut s = vec![false; files.len()];
        if o.external && !eligible.is_empty() {
            let n = o.sample.min(eligible.len()).max(1);
            for k in 0..n {
                s[eligible[k * eligible.len() / n]] = true;
            }
        }
        s
    };
    let tools = external::Tools { work: work.join("external"), onlyoffice_fonts: external::Tools::find_fonts(&tools_dir()) };
    if o.external {
        notes.push(format!(
            "External check on {} files with LibreOffice{}.",
            sampled.iter().filter(|s| **s).count(),
            if tools.onlyoffice_fonts.is_some() { " and OnlyOffice (x2t)" } else { " (OnlyOffice skipped: no AllFonts.js found)" }
        ));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let cfg = Arc::new(Cfg { exe, dir: o.dir.clone(), work: work.clone(), timeout: o.timeout, mem_mb: o.mem_mb, apexdesk, keep: o.keep, tools });
    let files = Arc::new(files);
    let sampled = Arc::new(sampled);
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<(usize, FileResult)>();
    let start = Instant::now();
    let mut handles = Vec::new();
    for slot in 0..o.jobs {
        let (cfg, files, sampled, next, tx) = (cfg.clone(), files.clone(), sampled.clone(), next.clone(), tx.clone());
        handles.push(std::thread::spawn(move || {
            loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some((path, size)) = files.get(i) else { break };
                let r = one_file(&cfg, slot, path, *size, sampled[i]);
                if tx.send((i, r)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);
    let jsonl = out.join("results.jsonl");
    let mut w = std::io::BufWriter::new(std::fs::File::create(&jsonl).map_err(|e| e.to_string())?);
    let mut results: BTreeMap<usize, FileResult> = BTreeMap::new();
    for (i, r) in rx {
        let _ = writeln!(w, "{}", serde_json::to_string(&r).unwrap_or_default());
        results.insert(i, r);
        let n = results.len();
        if n % 100 == 0 || n == files.len() {
            let _ = w.flush();
            eprintln!("apex-bench run: {n}/{} files, {:.0} s", files.len(), start.elapsed().as_secs_f64());
        }
    }
    for h in handles {
        let _ = h.join();
    }
    drop(w);
    let results: Vec<FileResult> = results.into_values().collect();
    // results.jsonl in corpus order, so that runs diff line by line.
    let body: String = results.iter().map(|r| serde_json::to_string(r).unwrap_or_default() + "\n").collect();
    std::fs::write(&jsonl, body).map_err(|e| e.to_string())?;
    notes.push(format!("{} files in {:.0} s with {} processes at a time, {} s and {} MB per file at most.", results.len(), start.elapsed().as_secs_f64(), o.jobs, o.timeout.as_secs(), o.mem_mb));
    let mut rep = report::report(&results, &o.dir.display().to_string(), &notes);
    if let Some(old) = &old {
        let c = report::compare(old, &results);
        eprint!("{c}");
        rep.push('\n');
        rep.push_str(&c);
    }
    std::fs::write(out.join("report.md"), rep).map_err(|e| e.to_string())?;
    if !o.keep {
        let _ = std::fs::remove_dir_all(&work);
    }
    eprintln!("apex-bench run: {} and {}", out.join("report.md").display(), jsonl.display());
    Ok(())
}

/// target/tools of the main checkout (where tools are downloaded).
fn tools_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.ancestors().nth(2).unwrap_or(manifest);
    // A worktree under .claude/worktrees shares the main checkout's target.
    let main = root.ancestors().find(|a| a.join(".git").is_dir()).unwrap_or(root);
    main.join("target/tools")
}

/// The files to run, relative to `dir`, with their sizes: the manifest's
/// when there is one, otherwise every file below `dir`.
fn list_files(dir: &Path) -> Result<Vec<(String, u64)>, String> {
    let manifest = dir.join("manifest.tsv");
    if manifest.exists() {
        let s = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        return Ok(s
            .lines()
            .skip(1)
            .filter_map(|l| {
                let mut f = l.split('\t');
                let path = f.next()?.to_string();
                let size = f.nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                Some((path, size))
            })
            .collect());
    }
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in entries.flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(p);
            } else if !worker::targets(&p).is_empty()
                && let Ok(rel) = p.strip_prefix(dir)
            {
                out.push((rel.to_string_lossy().into_owned(), meta.len()));
            }
        }
    }
    out.sort();
    Ok(out)
}

struct Cfg {
    exe: PathBuf,
    dir: PathBuf,
    work: PathBuf,
    timeout: Duration,
    mem_mb: u64,
    apexdesk: Option<PathBuf>,
    keep: bool,
    tools: external::Tools,
}

/// Runs the worker over one file until every target is done, starting it
/// again without the target that crashed it.
fn one_file(cfg: &Cfg, slot: usize, rel: &str, size: u64, external: bool) -> FileResult {
    let start = Instant::now();
    let path = cfg.dir.join(rel);
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let work = cfg.work.join(format!("{slot}"));
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::create_dir_all(&work);
    let mut r = FileResult { path: rel.to_string(), size, ..Default::default() };
    let mut pending: Vec<String> = worker::targets(&path);
    r.targets = pending.iter().map(|t| Target { ext: t.clone(), save: Outcome::Skipped, reopen: Outcome::Skipped, ..Default::default() }).collect();
    let mut peak_kb = 0;
    loop {
        let mut cmd = Command::new(&cfg.exe);
        cmd.arg("__run-worker").arg(&path).arg(&work).args(&pending);
        let exit = match child::run(cmd, cfg.timeout, cfg.mem_mb) {
            Ok(e) => e,
            Err(e) => {
                r.open = Outcome::Crash;
                r.error = Some(format!("can't start the worker: {e}"));
                break;
            }
        };
        peak_kb = peak_kb.max(exit.peak_kb);
        let mut last: Option<(String, String)> = None;
        let mut done: Vec<String> = Vec::new();
        for line in exit.stdout.lines() {
            let Ok(ev) = serde_json::from_str::<Event>(line) else { continue };
            let target = |r: &mut FileResult, t: &str| r.targets.iter_mut().position(|x| x.ext == t);
            match ev {
                Event::Step { target, step } => last = Some((target, step)),
                Event::Opened { kind, format, ms } => {
                    r.open = Outcome::Ok;
                    r.kind = Some(kind);
                    r.format = Some(format);
                    r.open_ms = ms;
                }
                Event::OpenFailed { outcome, msg } => {
                    r.open = outcome;
                    r.error = Some(msg);
                    r.targets.clear();
                }
                Event::Saved { target: t, ms, size } => {
                    if let Some(i) = target(&mut r, &t) {
                        let x = &mut r.targets[i];
                        (x.save, x.save_ms, x.size) = (Outcome::Ok, ms, size);
                    }
                }
                Event::Reopened { target: t, ms } => {
                    if let Some(i) = target(&mut r, &t) {
                        (r.targets[i].reopen, r.targets[i].reopen_ms) = (Outcome::Ok, ms);
                    }
                }
                Event::Failed { target: t, step, msg } => {
                    if let Some(i) = target(&mut r, &t) {
                        let x = &mut r.targets[i];
                        if step == "save" { x.save = Outcome::Error } else { x.reopen = Outcome::Error }
                        x.error = Some(msg);
                    }
                    done.push(t);
                }
                Event::Diffs { target: t, diffs } => {
                    if let Some(i) = target(&mut r, &t) {
                        r.targets[i].diffs = diffs;
                    }
                    done.push(t);
                }
                Event::Peak { kb } => peak_kb = peak_kb.max(kb),
            }
        }
        if exit.success() {
            break;
        }
        // It died: blame the step it had started.
        let (outcome, msg) = match (exit.killed, exit.panic()) {
            (Some(child::Killed::Timeout), _) => (Outcome::Timeout, format!("more than {} s", cfg.timeout.as_secs())),
            (Some(child::Killed::Memory), _) => (Outcome::Memory, format!("more than {} MB", cfg.mem_mb)),
            (None, Some(p)) => (Outcome::Panic, p),
            (None, None) => (Outcome::Crash, format!("{}: {}", exit.status.map(|s| s.to_string()).unwrap_or_default(), exit.stderr_tail())),
        };
        match last {
            Some((t, step)) if !t.is_empty() => {
                if let Some(x) = r.targets.iter_mut().find(|x| x.ext == t) {
                    match step.as_str() {
                        "save" => x.save = outcome,
                        "reopen" => x.reopen = outcome,
                        // Comparing crashed: count it against the reopen.
                        _ => x.reopen = outcome,
                    }
                    x.error = Some(format!("{step}: {msg}"));
                }
                done.push(t);
                pending.retain(|p| !done.contains(p));
                if pending.is_empty() {
                    break;
                }
            }
            _ => {
                r.open = outcome;
                r.error = Some(msg);
                r.targets.clear();
                break;
            }
        }
    }
    r.peak_mb = peak_kb / 1024;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
    let saved: Vec<(String, PathBuf)> =
        r.targets.iter().filter(|t| t.reopen == Outcome::Ok).map(|t| (t.ext.clone(), work.join(format!("{stem}.{}", t.ext)))).filter(|(_, p)| p.exists()).collect();
    if r.kind.as_deref() == Some("doc")
        && let Some(apexdesk) = &cfg.apexdesk
    {
        count_pages(cfg, apexdesk, &path, &work, &saved, &mut r);
    }
    if external && r.open == Outcome::Ok {
        r.external = external::check(&cfg.tools, slot, &path, &saved);
    }
    if !cfg.keep {
        let _ = std::fs::remove_dir_all(&work);
    }
    r.total_ms = start.elapsed().as_millis() as u64;
    r
}

/// Our page count of the original and of each saved document; a
/// different count is a difference of its target.
fn count_pages(cfg: &Cfg, apexdesk: &Path, path: &Path, work: &Path, saved: &[(String, PathBuf)], r: &mut FileResult) {
    // The original is copied to a plain name (apexdesk expands wildcards).
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let orig = work.join(format!("original.{ext}"));
    if let Err(e) = std::fs::copy(path, &orig) {
        r.pages_error = Some(e.to_string());
        return;
    }
    let mut files = vec![orig.clone()];
    files.extend(saved.iter().map(|(_, p)| p.clone()));
    let count = || pages::count(apexdesk, &files, cfg.timeout * 2, cfg.mem_mb);
    let differs = |p: &std::collections::HashMap<PathBuf, u64>| saved.iter().any(|(_, f)| p.get(f) != p.get(&orig));
    // Under load a layout can finish before every font is loaded: a
    // difference counts only when a second count agrees.
    let pages = match count() {
        Ok(p) if differs(&p) => count().or(Ok(p)),
        other => other,
    };
    match pages {
        Ok(p) => {
            r.pages = p.get(&orig).copied();
            for (ext, file) in saved {
                let Some(t) = r.targets.iter_mut().find(|t| &t.ext == ext) else { continue };
                t.pages = p.get(file).copied();
                if let (Some(a), Some(b)) = (r.pages, t.pages)
                    && a != b
                {
                    t.diffs.push(Diff { kind: "pages".into(), count: a.abs_diff(b), detail: format!("{a} → {b}") });
                }
            }
        }
        Err(e) => r.pages_error = Some(e),
    }
}
