//! What the QA sections share: the mode, where reports go, the known
//! failures, and the runner that checks each view in a process of its
//! own (the fonts, the language and a panic are the process's) and
//! joins what the processes found into the section's report.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex};
use web_time::{Duration, Instant};

/// The job a child process runs (a view of a section).
const JOB: &str = "APEXDESK_QA_JOB";
/// Where a child writes what it finds.
const PART: &str = "APEXDESK_QA_PART";
/// A child still running after this is stopped (the audit's own watch
/// stops a hung window sooner).
const CHILD_LIMIT: Duration = Duration::from_secs(1800);

/// The full run (`APEXDESK_QA_MODE=full`): more of everything, and the
/// checks too slow for every day.
pub(super) fn full() -> bool {
    std::env::var("APEXDESK_QA_MODE").is_ok_and(|m| m == "full")
}

pub(super) fn mode_name() -> &'static str {
    if full() { "full" } else { "quick" }
}

/// Where the reports go: `$APEXDESK_QA_OUT`, else `target/qa/last`.
pub(super) fn out_dir() -> PathBuf {
    let d = std::env::var_os("APEXDESK_QA_OUT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/qa/last")));
    let _ = apex_fs::create_dir_all(&d);
    d
}

/// The failures accepted (`known.txt`): section and id.
fn known() -> HashSet<(String, String)> {
    let text = apex_fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app/qa/known.txt")).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.splitn(3, '|');
            Some((f.next()?.trim().to_string(), f.next()?.trim().to_string()))
        })
        .collect()
}

/// An id that stays the same from run to run, fit for `known.txt`.
pub(super) fn id(parts: &[&str]) -> String {
    parts.iter().map(|p| p.replace('|', "¦").replace('\n', " ")).collect::<Vec<_>>().join(" / ")
}

/// What a child process found, written as it goes (a crash keeps what
/// came before).
pub(super) struct Sink {
    file: Option<apex_fs::File>,
    checks: usize,
}

impl Sink {
    fn open(path: Option<PathBuf>) -> Sink {
        Sink { file: path.and_then(|p| apex_fs::File::create(p).ok()), checks: 0 }
    }

    fn line(&mut self, l: String) {
        match &mut self.file {
            Some(f) => {
                let _ = writeln!(f, "{l}");
            }
            None => eprintln!("{l}"),
        }
    }

    /// One more check made.
    pub fn check(&mut self) {
        self.checks += 1;
        if self.checks % 25 == 0 {
            self.line("C\t25".into());
        }
    }

    /// A failure: its stable id, and the line of the report.
    pub fn fail(&mut self, id: String, text: String) {
        self.line(format!("F\t{}\t{}", id.replace('\t', " "), text.replace(['\t', '\n'], " ")));
    }

    /// A line for the report's notes (what was covered, the slowest).
    pub fn note(&mut self, text: String) {
        self.line(format!("N\t{}", text.replace(['\t', '\n'], " ")));
    }

    fn done(&mut self) {
        let rest = self.checks % 25;
        self.line(format!("C\t{rest}"));
        self.line("D".into());
    }
}

/// Children running at once, for all the sections of this process.
static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

fn slots() -> usize {
    std::env::var("APEXDESK_QA_JOBS").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(4)
}

/// Runs `section` over `jobs` (one child process each, a few at once),
/// `run` checking one job in the child; then writes the section's report.
/// In a child it only runs its job.
pub(super) fn run(section: &str, test: &str, jobs: &[String], run: impl Fn(&str, &mut Sink)) {
    if let Ok(job) = std::env::var(JOB) {
        let mut sink = Sink::open(std::env::var_os(PART).map(PathBuf::from));
        run(&job, &mut sink);
        sink.done();
        // The test folder of this process (fixtures, settings).
        if let Some(d) = crate::settings::config_dir() {
            let _ = apex_fs::remove_dir_all(d);
        }
        return;
    }
    let out = out_dir();
    let parts = out.join("parts");
    let _ = apex_fs::create_dir_all(&parts);
    let exe = std::env::current_exe().expect("test binary");
    let start = Instant::now();
    let results: Vec<(String, PathBuf, String)> = apex_fs::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|job| {
                let (exe, parts, out) = (&exe, &parts, &out);
                s.spawn(move || {
                    {
                        let (m, cv) = &SLOTS;
                        let mut n = cv.wait_while(m.lock().unwrap_or_else(|e| e.into_inner()), |n| *n >= slots()).unwrap_or_else(|e| e.into_inner());
                        *n += 1;
                    }
                    let part = parts.join(format!("{section}.{job}.part"));
                    let tmp = out.join("tmp").join(format!("{section}-{job}"));
                    let _ = apex_fs::create_dir_all(&tmp);
                    let err = child(exe, test, job, &part, &tmp);
                    let _ = apex_fs::remove_dir_all(&tmp);
                    let (m, cv) = &SLOTS;
                    *m.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
                    cv.notify_all();
                    (job.clone(), part, err)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("QA runner")).collect()
    });
    let _ = apex_fs::remove_dir(out.join("tmp"));
    report(section, &results, start.elapsed());
}

/// Runs `job` in a child process; what it printed when it didn't end well.
fn child(exe: &PathBuf, test: &str, job: &str, part: &PathBuf, tmp: &PathBuf) -> String {
    let spawned = Command::new(exe)
        .args([test, "--exact", "--nocapture", "--test-threads=1", "--ignored"])
        .env(JOB, job)
        .env(PART, part)
        // Temporary files (the test profile, fixtures) out of /tmp.
        .env("TMPDIR", tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut c = match spawned {
        Ok(c) => c,
        Err(e) => return format!("could not start: {e}"),
    };
    // Read as it comes (a full pipe would stop the child).
    let mut stderr = c.stderr.take().unwrap();
    let mut stdout = c.stdout.take().unwrap();
    let err = apex_fs::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut s);
        s
    });
    let outp = apex_fs::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match c.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() > CHILD_LIMIT => {
                let _ = c.kill();
                let _ = c.wait();
                break None;
            }
            Ok(None) => apex_fs::thread::sleep(Duration::from_millis(200)),
            Err(_) => break None,
        }
    };
    let (err, out) = (err.join().unwrap_or_default(), outp.join().unwrap_or_default());
    match status {
        Some(st) if st.success() && out.contains("1 passed") => String::new(),
        Some(st) => {
            let tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
            format!("{st}: {}", tail[tail.len().saturating_sub(4)..].join(" | "))
        }
        None => format!("stopped after {} s", CHILD_LIMIT.as_secs()),
    }
}

/// Joins the children's findings: `<section>.md` and `<section>.sum`.
fn report(section: &str, results: &[(String, PathBuf, String)], took: Duration) {
    let known = known();
    let mut checks = 0usize;
    // By id, the first line of each (a failure found twice counts once).
    let mut new: BTreeMap<String, String> = BTreeMap::new();
    let mut old: BTreeMap<String, String> = BTreeMap::new();
    let mut notes = Vec::new();
    for (job, part, err) in results {
        let text = apex_fs::read_to_string(part).unwrap_or_default();
        let mut done = false;
        for l in text.lines() {
            let mut f = l.splitn(3, '\t');
            match (f.next(), f.next(), f.next()) {
                (Some("C"), Some(n), _) => checks += n.parse::<usize>().unwrap_or(0),
                (Some("F"), Some(id), Some(t)) => {
                    let to = if known.contains(&(section.to_string(), id.to_string())) { &mut old } else { &mut new };
                    to.entry(id.to_string()).or_insert_with(|| t.to_string());
                }
                (Some("N"), Some(t), _) => notes.push(format!("{job}: {t}")),
                (Some("D"), _, _) => done = true,
                _ => {}
            }
        }
        if !done || !err.is_empty() {
            let id = id(&[job, "crash"]);
            let t = format!("{job}: the QA process of this view ended early ({}); the report of this view is partial. Repro: cargo test -p apex-app -- --ignored --exact app::qa::{section}::qa_{section} with {JOB}={job}", if err.is_empty() { "no end mark" } else { err.as_str() });
            let to = if known.contains(&(section.to_string(), id.clone())) { &mut old } else { &mut new };
            to.insert(id, t);
        }
        let _ = apex_fs::remove_file(part);
    }
    let _ = apex_fs::remove_dir(out_dir().join("parts"));
    let line = |(id, t): (&String, &String)| format!("{t} [id: {id}]");
    let mut md = format!("# {section} ({})\n\n{checks} checks, {} new failures, {} known, in {:.0} s.\n\n## New failures\n\n", mode_name(), new.len(), old.len(), took.as_secs_f32());
    for f in &new {
        md.push_str(&format!("- {}\n", line(f)));
    }
    md.push_str("\n## Known failures (known.txt)\n\n");
    for f in &old {
        md.push_str(&format!("* {}\n", line(f)));
    }
    md.push_str("\n## Notes\n\n");
    for n in &notes {
        md.push_str(&format!("  {n}\n"));
    }
    let out = out_dir();
    let _ = apex_fs::write(out.join(format!("{section}.md")), md);
    let sum = format!("{section}: {checks} comprobaciones, {} fallos, {} conocidos\n", new.len(), old.len());
    let _ = apex_fs::write(out.join(format!("{section}.sum")), &sum);
    eprint!("{sum}");
}

/// A small pseudo-random generator (SplitMix64): the same seed, the same
/// numbers, on every machine.
pub(super) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5DEE_CE66_D1CE_4E5B)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    /// A fraction in 0..1.
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// This process's resident memory, bytes.
pub(super) fn rss() -> u64 {
    let statm = apex_fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    pages * 4096
}

/// The errors reported since the last call.
pub(super) fn take_errors() -> Vec<String> {
    std::mem::take(&mut *crate::diag::test::ERRORS.lock().unwrap_or_else(|e| e.into_inner()))
}
