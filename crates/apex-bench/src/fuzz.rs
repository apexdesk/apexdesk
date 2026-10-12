//! `apex-bench fuzz`: mutation fuzzing of the readers with stable Rust
//! (no cargo-fuzz). Seeds are mutated with knowledge of their container
//! (see [`mutate`]), each test case is opened, recalculated, saved and
//! opened again in a child process with limits of time and memory
//! ([`child`], [`exec`]), and every new failure is reduced ([`minimize`])
//! and kept in OUT/crashes/ with its trace. OUT/report.md sums it up.
//!
//! Build for long runs with symbols and unwinding, so traces have lines
//! and the parent survives a bad seed:
//!   CARGO_PROFILE_RELEASE_STRIP=false CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
//!   CARGO_PROFILE_RELEASE_PANIC=unwind CARGO_PROFILE_RELEASE_LTO=thin cargo build --release -p apex-bench

mod alloc;
mod bytes;
pub(crate) mod child;
pub(crate) mod exec;
mod minimize;
mod mutate;
mod ole;
pub(crate) mod report;
pub(crate) mod rng;
pub(crate) mod seeds;
mod xml;
mod zip;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use exec::{Kind, Limits, Outcome};
use mutate::Seed;
use rng::Rng;

pub use alloc::{peak as heap_peak, reset_peak as reset_heap_peak, set_limit as set_heap_limit};

pub const USAGE: &str = "apex-bench fuzz [SEEDS...] [--time 1h] [--format xlsx,docx,...] [--jobs 2] [--out DIR]
                 [--timeout 10] [--mem 1536] [--max-seed-kb 512] [--seeds-per-reader 300] [--min-secs 120]
apex-bench fuzz --make-seeds DIR [--lo] [--python PYTHON]
apex-bench fuzz --replay FILE      run one file like a test case and say what happens
apex-bench fuzz --report [--out DIR]

Readers: xlsx xlsb ods csv docx odt md rtf txt xls doc vba crypto.
Default seeds: target/samples, target/corpus, the repository's test files and OUT/seeds. Default OUT: target/bench/fuzz.";

struct Opts {
    seeds: Vec<PathBuf>,
    time: Duration,
    formats: Vec<String>,
    jobs: usize,
    out: PathBuf,
    timeout: Duration,
    memory: usize,
    max_seed: u64,
    per_reader: usize,
    min_secs: u64,
}

pub fn parse_duration(s: &str) -> Option<Duration> {
    let (num, unit) = s.find(|c: char| !c.is_ascii_digit() && c != '.').map(|i| s.split_at(i)).unwrap_or((s, "s"));
    let n: f64 = num.parse().ok()?;
    let mult = match unit {
        "s" | "" => 1.0,
        "m" | "min" => 60.0,
        "h" => 3600.0,
        _ => return None,
    };
    Some(Duration::from_secs_f64(n * mult))
}

pub(crate) fn value<'a>(args: &'a [String], i: &mut usize, name: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i).map(String::as_str).ok_or_else(|| format!("{name} needs a value"))
}

pub fn main(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("--child") => return child::main(&args[1..]),
        Some("--make-seeds") => return make_seeds(&args[1..]),
        Some("--replay") => return replay(&args[1..]),
        _ => {}
    }
    let mut o = Opts {
        seeds: Vec::new(),
        time: Duration::from_secs(600),
        formats: Vec::new(),
        jobs: 2,
        out: PathBuf::from("target/bench/fuzz"),
        timeout: Duration::from_secs(10),
        memory: 1536 << 20,
        max_seed: 512 << 10,
        per_reader: 300,
        min_secs: 120,
    };
    let mut report_only = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--time" => o.time = parse_duration(value(args, &mut i, "--time")?).ok_or("bad --time")?,
            "--format" => o.formats = value(args, &mut i, "--format")?.split(',').map(|s| s.trim().to_ascii_lowercase()).collect(),
            "--jobs" => o.jobs = value(args, &mut i, "--jobs")?.parse().map_err(|_| "bad --jobs")?,
            "--out" => o.out = PathBuf::from(value(args, &mut i, "--out")?),
            "--timeout" => o.timeout = parse_duration(value(args, &mut i, "--timeout")?).ok_or("bad --timeout")?,
            "--mem" => o.memory = value(args, &mut i, "--mem")?.parse::<usize>().map_err(|_| "bad --mem")? << 20,
            "--max-seed-kb" => o.max_seed = value(args, &mut i, "--max-seed-kb")?.parse::<u64>().map_err(|_| "bad --max-seed-kb")? << 10,
            "--seeds-per-reader" => o.per_reader = value(args, &mut i, "--seeds-per-reader")?.parse().map_err(|_| "bad --seeds-per-reader")?,
            "--min-secs" => o.min_secs = value(args, &mut i, "--min-secs")?.parse().map_err(|_| "bad --min-secs")?,
            "--report" => report_only = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            a if a.starts_with("--") => return Err(format!("unknown option {a}\n{USAGE}")),
            a => o.seeds.push(PathBuf::from(a)),
        }
        i += 1;
    }
    if report_only {
        return report::write(&o.out);
    }
    // The machine is shared: at most 6 children at a time.
    o.jobs = o.jobs.clamp(1, 6);
    run(o)
}

fn make_seeds(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or("--make-seeds needs a folder")?);
    let lo = args.iter().any(|a| a == "--lo");
    let python = args.iter().position(|a| a == "--python").and_then(|i| args.get(i + 1));
    let mut made = seeds::make(&dir)?;
    if let Some(py) = python {
        made.extend(seeds::openpyxl(&dir, py));
    }
    if lo {
        let mut sources = made.clone();
        // A few real Word files of the samples, when there are.
        let mut docs = Vec::new();
        seeds_walk(Path::new("target/samples"), &mut docs);
        docs.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("doc")) && std::fs::metadata(p).is_ok_and(|m| m.len() < 100 << 10));
        docs.truncate(20);
        sources.extend(docs);
        made.extend(seeds::libreoffice(&dir, &sources));
    }
    made.extend(seeds::vba_from(&dir, Path::new("target/corpus"), 60));
    // Markdown is rare in the corpora: the repository's own documents.
    let md = dir.join("markdown");
    let _ = std::fs::create_dir_all(&md);
    let mut docs = vec![PathBuf::from("README.md")];
    seeds_walk(Path::new("docs"), &mut docs);
    for d in docs.iter().filter(|p| p.extension().is_some_and(|e| e == "md")) {
        let to = md.join(d.file_name().unwrap_or_default());
        if std::fs::copy(d, &to).is_ok() {
            made.push(to);
        }
    }
    println!("{} seeds in {}", made.len(), dir.display());
    Ok(())
}

fn seeds_walk(p: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                seeds_walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
}

fn replay(args: &[String]) -> Result<(), String> {
    let file = PathBuf::from(args.first().ok_or("--replay needs a file")?);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Next to the file: what is saved can be big (/tmp may be in memory).
    let work = file.with_file_name(format!(".apex-bench-replay-{}", std::process::id()));
    let o = exec::run(&exe, &file, &work, &Limits { timeout: Duration::from_secs(10), memory: 1536 << 20 }, true);
    let _ = std::fs::remove_dir_all(&work);
    println!("{} at {} in {:.2} s, {} MB\n{}\n{}\n{}", o.kind.name(), o.stage, o.secs, o.max_rss_kb >> 10, o.signature, o.message, o.trace);
    Ok(())
}

/// A failure already known: the file that shows it and how often it came.
struct Known {
    file: PathBuf,
    hits: usize,
    new_hits: usize,
}

#[derive(Default, Clone)]
struct Stats {
    tests: usize,
    ok: usize,
    rejected: usize,
    failures: BTreeMap<&'static str, usize>,
    secs: f64,
}

struct Shared {
    stats: BTreeMap<String, Stats>,
    known: HashMap<String, Known>,
}

pub fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn key(reader: &str, o: &Outcome) -> String {
    format!("{reader}|{}|{}", o.kind.name(), o.signature)
}

fn run(o: Opts) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let crashes = o.out.join("crashes");
    std::fs::create_dir_all(&crashes).map_err(|e| e.to_string())?;
    let mut seed_dirs = o.seeds.clone();
    if seed_dirs.is_empty() {
        seed_dirs = vec![o.out.join("seeds"), PathBuf::from("crates/apex-io/tests/data"), PathBuf::from("crates/apex-sign/tests/data"), PathBuf::from("target/samples"), PathBuf::from("target/corpus")];
    }
    let all = seeds::collect(&seed_dirs, o.max_seed, o.per_reader);
    let mut groups: BTreeMap<String, Vec<Seed>> = BTreeMap::new();
    for s in all {
        if o.formats.is_empty() || o.formats.contains(&s.reader) {
            groups.entry(s.reader.clone()).or_default().push(s);
        }
    }
    for f in &o.formats {
        if !groups.contains_key(f) {
            eprintln!("apex-bench: no seeds for {f}");
        }
    }
    if groups.is_empty() {
        return Err("no seeds (make some with --make-seeds DIR)".into());
    }
    let summary: Vec<String> = groups.iter().map(|(k, v)| format!("{k} {}", v.len())).collect();
    eprintln!("fuzz: {} jobs for {:?}; seeds: {}", o.jobs, o.time, summary.join(", "));
    let readers: Vec<String> = groups.keys().cloned().collect();
    let known = report::known(&crashes).into_iter().map(|(k, (file, hits))| (k, Known { file, hits, new_hits: 0 })).collect();
    let shared = Mutex::new(Shared { stats: BTreeMap::new(), known });
    let limits = Limits { timeout: o.timeout, memory: o.memory };
    let start = Instant::now();
    let deadline = start + o.time;
    let work_root = o.out.join("work");
    std::thread::scope(|scope| {
        for w in 0..o.jobs {
            let (groups, readers, shared, limits, exe, crashes) = (&groups, &readers, &shared, &limits, &exe, &crashes);
            let work = work_root.join(format!("w{w}"));
            let min_secs = o.min_secs;
            scope.spawn(move || worker(w, &work, exe, groups, readers, shared, limits, crashes, deadline, min_secs));
        }
        // Progress every minute.
        let mut last = Instant::now();
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(1));
            if last.elapsed() >= Duration::from_secs(60) {
                last = Instant::now();
                if let Ok(s) = shared.lock() {
                    let tests: usize = s.stats.values().map(|x| x.tests).sum();
                    let fails: usize = s.stats.values().map(|x| x.failures.values().sum::<usize>()).sum();
                    eprintln!("fuzz: {:>5.0} s, {tests} tests, {fails} failures, {} distinct known", start.elapsed().as_secs_f64(), s.known.len());
                }
            }
        }
    });
    let _ = std::fs::remove_dir_all(&work_root);
    let s = shared.into_inner().map_err(|_| "poisoned")?;
    // Hits of the failures already known.
    for k in s.known.values().filter(|k| k.new_hits > 0) {
        report::add_hits(&k.file, k.hits + k.new_hits);
    }
    let rows: Vec<report::RunRow> = s
        .stats
        .iter()
        .map(|(r, st)| report::RunRow {
            reader: r.clone(),
            secs: st.secs,
            tests: st.tests,
            ok: st.ok,
            rejected: st.rejected,
            failures: st.failures.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "),
            wall: start.elapsed().as_secs_f64(),
            jobs: o.jobs,
        })
        .collect();
    report::append_runs(&o.out, &rows)?;
    for r in &rows {
        eprintln!("fuzz: {:<7} {:>7} tests  ok {:>7}  rejected {:>7}  {}", r.reader, r.tests, r.ok, r.rejected, r.failures);
    }
    report::write(&o.out)
}

#[allow(clippy::too_many_arguments)]
fn worker(
    w: usize,
    work: &Path,
    exe: &Path,
    groups: &BTreeMap<String, Vec<Seed>>,
    readers: &[String],
    shared: &Mutex<Shared>,
    limits: &Limits,
    crashes: &Path,
    deadline: Instant,
    min_secs: u64,
) {
    let mut rng = Rng::from_time(w as u64);
    let _ = std::fs::create_dir_all(work);
    let mut turn = w;
    while Instant::now() < deadline {
        let reader = &readers[turn % readers.len()];
        turn += 1;
        let group = &groups[reader];
        let seed = rng.pick(group);
        let pool: Vec<&Seed> = (0..3).map(|_| rng.pick(group)).collect();
        let data = mutate::mutate(&mut rng, seed, &pool);
        let case = work.join(format!("case.{}", seed.ext));
        if std::fs::write(&case, &data).is_err() {
            continue;
        }
        let out = exec::run(exe, &case, &work.join("out"), limits, true);
        let fresh = {
            let Ok(mut s) = shared.lock() else { return };
            let st = s.stats.entry(reader.clone()).or_default();
            st.tests += 1;
            st.secs += out.secs;
            match out.kind {
                Kind::Ok => st.ok += 1,
                Kind::Rejected => st.rejected += 1,
                k => *st.failures.entry(k.name()).or_default() += 1,
            }
            if out.kind.is_failure() {
                let k = key(reader, &out);
                match s.known.get_mut(&k) {
                    Some(known) => {
                        known.new_hits += 1;
                        false
                    }
                    None => {
                        s.known.insert(k, Known { file: PathBuf::new(), hits: 0, new_hits: 1 });
                        true
                    }
                }
            } else {
                false
            }
        };
        if fresh {
            eprintln!("fuzz: new {} in {reader} at {}: {} ({})", out.kind.name(), out.stage, out.signature, out.message);
            let file = keep(&out, reader, seed, &data, work, exe, limits, crashes, min_secs);
            if let Ok(mut s) = shared.lock()
                && let Some(k) = s.known.get_mut(&key(reader, &out))
            {
                k.file = file;
            }
        }
    }
}

/// Reduces a new failure and writes it to the crashes folder with its
/// trace; returns the path of the trace file.
#[allow(clippy::too_many_arguments)]
fn keep(first: &Outcome, reader: &str, seed: &Seed, data: &[u8], work: &Path, exe: &Path, limits: &Limits, crashes: &Path, min_secs: u64) -> PathBuf {
    let slow = matches!(first.kind, Kind::Hang | Kind::Stack);
    let budget = minimize::Budget { until: Instant::now() + Duration::from_secs(if slow { min_secs * 2 } else { min_secs }), tests: if slow { 12 } else { 600 } };
    let candidate = work.join(format!("min.{}", seed.ext));
    let mut last: Option<Outcome> = None;
    let mut fails = |d: &[u8]| -> bool {
        if std::fs::write(&candidate, d).is_err() {
            return false;
        }
        let o = exec::run(exe, &candidate, &work.join("out-min"), limits, false);
        let same = o.kind == first.kind && (slow || o.signature == first.signature);
        if same {
            last = Some(o);
        }
        same
    };
    let mut ctx = minimize::Ctx { fails: &mut fails, budget };
    let small = minimize::minimize(data, &mut ctx);
    // The trace of the reduced file (with the debugger for a hang).
    let _ = std::fs::write(&candidate, &small);
    let final_run = if first.kind == Kind::Hang { Some(exec::run(exe, &candidate, &work.join("out-min"), limits, true)) } else { last };
    let shown = final_run.as_ref().filter(|o| o.kind == first.kind).unwrap_or(first);
    let hash = fnv(&key(reader, first));
    let name = format!("{reader}-{}-{:08x}", first.kind.name(), hash as u32);
    let file = crashes.join(format!("{name}.{}", seed.ext));
    let _ = std::fs::write(&file, &small);
    let txt = crashes.join(format!("{name}{}", report::TRACE_EXT));
    let info = report::CrashInfo {
        reader: reader.to_string(),
        kind: first.kind.name().to_string(),
        stage: first.stage.clone(),
        signature: first.signature.clone(),
        message: first.message.clone(),
        seed: seed.path.display().to_string(),
        file: file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        original: data.len(),
        minimized: small.len(),
        hits: 1,
        trace: shown.trace.clone(),
    };
    let _ = std::fs::write(&txt, info.to_text());
    eprintln!("fuzz: kept {} ({} -> {} bytes)", file.display(), data.len(), small.len());
    txt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("1.5h"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("x"), None);
    }
}
