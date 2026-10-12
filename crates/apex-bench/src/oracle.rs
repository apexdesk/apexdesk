//! `apex-bench oracle`: compare ApexDesk with what Office stored in the
//! files it saved (cached formula values, rendered page breaks).
//!
//!   apex-bench oracle PATH... [--out DIR] [--jobs N] [--only formulas|pages]
//!                             [--apexdesk PATH] [--limit N] [--reuse]
//!   (a PATH @LIST reads the paths from the file LIST, one per line)
//!
//! Each file is checked by a child process (`apex-bench oracle-file`) with
//! a time and memory limit; the results go to DIR (default
//! target/bench/oracle): formulas.jsonl and pages.jsonl with one line per
//! file, and oracle-formulas.md / oracle-pages.md, joined into report.md.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::oracle_formulas::{self as formulas, Outcome};
use crate::oracle_pages as pages;
use crate::oracle_proc;

// Per child process (a shared machine: see the resource rules of the battery).
const MEMORY: u64 = 1536 << 20;

pub struct Opts {
    pub paths: Vec<String>,
    pub out: PathBuf,
    pub jobs: usize,
    pub limit: usize,
    pub apexdesk: PathBuf,
    pub only: Option<String>,
    /// Reports from the .jsonl files of an earlier run, without running.
    pub reuse: bool,
}

/// Default report directory: target/bench/<part> next to the executable's
/// target directory.
pub fn default_out(part: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    // target/release/apex-bench -> target/bench/<part>
    match exe.parent().and_then(Path::parent) {
        Some(target) => target.join("bench").join(part),
        None => PathBuf::from("target/bench").join(part),
    }
}

/// apexdesk next to apex-bench (same cargo profile).
pub fn default_apexdesk() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(if cfg!(windows) { "apexdesk.exe" } else { "apexdesk" })
}

pub fn parse_opts(args: &[String], part: &str) -> Result<Opts, String> {
    let mut o = Opts { paths: Vec::new(), out: default_out(part), jobs: 2, limit: 0, apexdesk: default_apexdesk(), only: None, reuse: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--out" => o.out = PathBuf::from(val()?),
            "--jobs" => o.jobs = val()?.parse().map_err(|_| "--jobs: a number")?,
            "--limit" => o.limit = val()?.parse().map_err(|_| "--limit: a number")?,
            "--apexdesk" => o.apexdesk = PathBuf::from(val()?),
            "--only" => o.only = Some(val()?),
            "--reuse" => o.reuse = true,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => o.paths.push(a.clone()),
        }
    }
    if o.paths.is_empty() {
        return Err("no files or directories given".into());
    }
    o.jobs = o.jobs.clamp(1, 2);
    Ok(o)
}

/// `apex-bench oracle-file formulas|pages FILE [--apexdesk PATH]`: the
/// child process. Prints one JSON line.
pub fn file_main(args: &[String]) -> Result<(), String> {
    let [kind, file, rest @ ..] = args else { return Err("usage: oracle-file formulas|pages FILE [--apexdesk PATH]".into()) };
    let apexdesk = match rest {
        [flag, p] if flag == "--apexdesk" => PathBuf::from(p),
        _ => default_apexdesk(),
    };
    let r = match kind.as_str() {
        "formulas" => formulas::check(Path::new(file)),
        "pages" => pages::check(Path::new(file), &apexdesk),
        _ => return Err(format!("unknown kind {kind}")),
    };
    println!("{r}");
    Ok(())
}

/// Runs the child for every file; a crash or timeout becomes a result too.
fn run_children(kind: &str, files: &[PathBuf], o: &Opts) -> Vec<Json> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("apex-bench"));
    oracle_proc::pool(
        files,
        o.jobs,
        |f| {
            let mut cmd = Command::new(&exe);
            cmd.arg("oracle-file").arg(kind).arg(f).arg("--apexdesk").arg(&o.apexdesk);
            let out = oracle_proc::run(cmd, Duration::from_secs(300), Some(MEMORY));
            let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            let mut r = match out.stdout.lines().rev().find_map(|l| serde_json::from_str::<Json>(l).ok()) {
                Some(r) => r,
                None => {
                    let status = if out.timed_out { "timeout" } else { "crash" };
                    let why = crash_reason(&out.stderr);
                    json!({"file": f.display().to_string(), "status": status, "reason": format!("exit {:?}: {why}", out.code)})
                }
            };
            r["size"] = json!(size);
            r["secs"] = json!((out.secs * 100.0).round() / 100.0);
            r
        },
        |d, n| {
            if d % 200 == 0 || d == n {
                eprintln!("oracle {kind}: {d}/{n}");
            }
        },
    )
}

/// The useful part of a crashed child's stderr: the panic message (the
/// line after "panicked at"), an allocation failure, or its last line.
pub fn crash_reason(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let why = match lines.iter().position(|l| l.contains("panicked at")) {
        Some(i) => format!("{} {}", lines[i].split("panicked at").nth(1).unwrap_or("").trim(), lines.get(i + 1).copied().unwrap_or("")),
        None => lines.iter().rev().find(|l| l.contains("memory allocation") || l.contains("out of memory")).or(lines.last()).copied().unwrap_or("").to_string(),
    };
    why.chars().take(300).collect()
}

/// A corpus path without the part before `corpus/` (shorter tables).
pub fn short_path(p: &str) -> &str {
    p.find("/corpus/").map_or(p, |i| &p[i + 8..])
}

/// Rows saved by an earlier run (`--reuse`).
fn read_jsonl(path: &Path) -> Option<Vec<Json>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
}

fn write_jsonl(path: &Path, rows: &[Json]) -> Result<(), String> {
    let mut s = String::new();
    for r in rows {
        s.push_str(&r.to_string());
        s.push('\n');
    }
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn main(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, "oracle")?;
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let limit = |mut v: Vec<PathBuf>| {
        if o.limit > 0 && v.len() > o.limit {
            // An even spread over the corpus, not its first files.
            let step = v.len() as f64 / o.limit as f64;
            v = (0..o.limit).map(|k| v[(k as f64 * step) as usize].clone()).collect();
        }
        v
    };
    if o.only.as_deref().is_none_or(|x| x == "formulas") {
        let files = limit(oracle_proc::files(&o.paths, &["xls", "xlsx", "xlsm"]));
        eprintln!("oracle formulas: {} files", files.len());
        let rows = match read_jsonl(&o.out.join("formulas.jsonl")).filter(|_| o.reuse) {
            Some(rows) => rows,
            None => run_children("formulas", &files, &o),
        };
        write_jsonl(&o.out.join("formulas.jsonl"), &rows)?;
        std::fs::write(o.out.join("oracle-formulas.md"), formulas_report(&rows)).map_err(|e| e.to_string())?;
    }
    if o.only.as_deref().is_none_or(|x| x == "pages") {
        if !o.apexdesk.exists() {
            return Err(format!("{} not found: build apex-app (same profile) or give --apexdesk", o.apexdesk.display()));
        }
        let files = limit(oracle_proc::files(&o.paths, &["docx", "docm"]));
        eprintln!("oracle pages: {} files", files.len());
        let rows = match read_jsonl(&o.out.join("pages.jsonl")).filter(|_| o.reuse) {
            Some(rows) => rows,
            None => run_children("pages", &files, &o),
        };
        write_jsonl(&o.out.join("pages.jsonl"), &rows)?;
        std::fs::write(o.out.join("oracle-pages.md"), pages_report(&rows)).map_err(|e| e.to_string())?;
    }
    assemble_report(&o.out)?;
    eprintln!("report: {}", o.out.join("report.md").display());
    Ok(())
}

/// report.md: the sections written so far, in a fixed order.
pub fn assemble_report(dir: &Path) -> Result<(), String> {
    let mut s = String::from("# Oracle report: ApexDesk against what Office stored in its files\n\n");
    s.push_str("Generated by `apex-bench oracle` and `apex-bench validate` (see crates/apex-bench).\n\n");
    for part in ["oracle-formulas.md", "oracle-pages.md", "validate.md"] {
        if let Ok(t) = std::fs::read_to_string(dir.join(part)) {
            s.push_str(&t);
            s.push('\n');
        }
    }
    std::fs::write(dir.join("report.md"), s).map_err(|e| e.to_string())
}

fn pct(a: u64, b: u64) -> String {
    if b == 0 { "-".into() } else { format!("{:.1}%", 100.0 * a as f64 / b as f64) }
}

fn md_cell(s: &str) -> String {
    let s: String = s.chars().take(90).collect();
    s.replace('|', "\\|").replace('\n', " ")
}

/// Counts of a status or reason, most frequent first.
fn counts<'a>(it: impl Iterator<Item = &'a str>) -> Vec<(String, u64)> {
    let mut m: BTreeMap<String, u64> = BTreeMap::new();
    for k in it {
        *m.entry(k.to_string()).or_default() += 1;
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// Normalizes a skip reason for counting ("saved by 'X'" keeps X).
fn reason_of(r: &Json) -> String {
    let s = r["reason"].as_str().unwrap_or("");
    let s = if s.starts_with("open: ") || s.starts_with("exit ") || s.starts_with("apexdesk info") { s.chars().take(70).collect() } else { s.to_string() };
    let status = r["status"].as_str().unwrap_or("?");
    if s.is_empty() { status.to_string() } else { format!("{status}: {s}") }
}

pub fn formulas_report(rows: &[Json]) -> String {
    let mut s = String::new();
    let all: Vec<&Json> = rows.iter().filter(|r| r["status"] == "checked").collect();
    // Manual calculation or fullCalcOnLoad: Excel's values may be stale,
    // so only automatic calculation counts in the tables.
    let checked: Vec<&Json> = all.iter().copied().filter(|r| r["calc"] == "auto").collect();
    let _ = writeln!(s, "## Formulas against Excel's cached values\n");
    let _ = writeln!(s, "Spreadsheets (.xls/.xlsx) saved by Excel: formula cells found in the file itself, cached value compared with ours after `recalc_all` (numbers within a relative 1e-9). Cells with volatile functions (NOW, TODAY, RAND, INFO, CELL...) or with no cached value are left out. The tables count only workbooks with automatic calculation.\n");
    let _ = writeln!(s, "Files: {} scanned, {} checked ({} with automatic calculation).\n", rows.len(), all.len(), checked.len());
    let _ = writeln!(s, "| status: reason | files |\n|---|---:|");
    for (k, n) in counts(rows.iter().map(|r| reason_of(r)).collect::<Vec<_>>().iter().map(String::as_str)).into_iter().take(15) {
        let _ = writeln!(s, "| {} | {n} |", md_cell(&k));
    }
    let _ = writeln!(s, "\n| calculation mode | files | formulas | agree |\n|---|---:|---:|---:|");
    for mode in ["auto", "manual", "fullCalcOnLoad"] {
        let these: Vec<&Json> = all.iter().copied().filter(|r| r["calc"] == mode).collect();
        let c = cell_outcomes(&these);
        let t: u64 = c.iter().sum();
        let _ = writeln!(s, "| {mode} | {} | {t} | {} |", these.len(), pct(c[0], t));
    }
    // Tallies.
    let mut by_func: BTreeMap<String, [u64; 5]> = BTreeMap::new();
    let mut formulas_n = 0u64;
    let mut skipped: BTreeMap<String, u64> = BTreeMap::new();
    for r in &checked {
        formulas_n += r["formulas"].as_u64().unwrap_or(0);
        if let Some(m) = r["skipped_cells"].as_object() {
            for (k, v) in m {
                *skipped.entry(k.clone()).or_default() += v.as_u64().unwrap_or(0);
            }
        }
        if let Some(t) = r["tally"].as_object() {
            for (f, v) in t {
                let e = by_func.entry(f.clone()).or_default();
                for k in 0..5 {
                    e[k] += v[k].as_u64().unwrap_or(0);
                }
            }
        }
    }
    // Per cell outcomes: a cell counts once (the tallies count it per function).
    let cells = cell_outcomes(&checked);
    let total: u64 = cells.iter().sum();
    let _ = writeln!(s, "\n**{} formulas checked, {} agree with Excel ({}).**\n", formulas_n, cells[0], pct(cells[0], total));
    let _ = writeln!(s, "| outcome | cells |\n|---|---:|");
    for o in Outcome::ALL {
        let _ = writeln!(s, "| {} | {} |", o.name(), cells[o as usize]);
    }
    for (k, v) in &skipped {
        let _ = writeln!(s, "| (left out: {k}) | {v} |");
    }
    // Per category.
    let mut by_cat: BTreeMap<&str, [u64; 5]> = BTreeMap::new();
    for (f, v) in &by_func {
        let e = by_cat.entry(formulas::category(f)).or_default();
        for k in 0..5 {
            e[k] += v[k];
        }
    }
    let _ = writeln!(s, "\n### By family of function\n\nA cell counts once for each function it uses.\n");
    let _ = writeln!(s, "| family | cells | agree | value | error | unsupported | unparsed |\n|---|---:|---:|---:|---:|---:|---:|");
    let mut cats: Vec<_> = by_cat.into_iter().collect();
    cats.sort_by_key(|(_, v)| std::cmp::Reverse(v.iter().sum::<u64>()));
    for (c, v) in cats {
        let t: u64 = v.iter().sum();
        let _ = writeln!(s, "| {c} | {t} | {} | {} | {} | {} | {} |", pct(v[0], t), v[1], v[2], v[3], v[4]);
    }
    let mut funcs: Vec<_> = by_func.iter().collect();
    funcs.sort_by_key(|(_, v)| std::cmp::Reverse(v.iter().sum::<u64>()));
    let _ = writeln!(s, "\n### By function (the 60 most used)\n");
    let _ = writeln!(s, "| function | family | cells | agree | value | error | unsupported | unparsed |\n|---|---|---:|---:|---:|---:|---:|---:|");
    for (f, v) in funcs.iter().take(60) {
        let t: u64 = v.iter().sum();
        let _ = writeln!(s, "| {f} | {} | {t} | {} | {} | {} | {} | {} |", formulas::category(f), pct(v[0], t), v[1], v[2], v[3], v[4]);
    }
    let mut worst: Vec<_> = by_func.iter().filter(|(_, v)| v.iter().sum::<u64>() >= 20).collect();
    worst.sort_by(|a, b| {
        let r = |v: &[u64; 5]| v[0] as f64 / v.iter().sum::<u64>() as f64;
        r(a.1).partial_cmp(&r(b.1)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let _ = writeln!(s, "\n### Lowest agreement (functions with 20 cells or more)\n");
    let _ = writeln!(s, "| function | cells | agree |\n|---|---:|---:|");
    for (f, v) in worst.iter().take(25) {
        let t: u64 = v.iter().sum();
        let _ = writeln!(s, "| {f} | {t} | {} |", pct(v[0], t));
    }
    let unsupported: Vec<_> = funcs.iter().filter(|(f, _)| f.as_str() != "(operators)" && !formulas::supported(f)).collect();
    if !unsupported.is_empty() {
        let _ = writeln!(s, "\n### Functions we don't have\n");
        let _ = writeln!(s, "| function | cells |\n|---|---:|");
        for (f, v) in unsupported.iter().take(30) {
            let _ = writeln!(s, "| {f} | {} |", v.iter().sum::<u64>());
        }
    }
    // Differences grouped by (kind, function), with the smallest file.
    let mut groups: BTreeMap<(String, String), (u64, u64, Option<(u64, String, Json)>)> = BTreeMap::new();
    for r in &checked {
        let size = r["size"].as_u64().unwrap_or(0);
        let file = r["file"].as_str().unwrap_or("").to_string();
        let mut seen = std::collections::BTreeSet::new();
        for d in r["diffs"].as_array().into_iter().flatten() {
            let kind = d["kind"].as_str().unwrap_or("").to_string();
            let fs = formulas::functions(d["formula"].as_str().unwrap_or(""));
            let keys: Vec<String> = if fs.is_empty() { vec!["(operators)".into()] } else { fs.into_iter().collect() };
            for f in keys {
                let g = groups.entry((kind.clone(), f.clone())).or_insert((0, 0, None));
                g.0 += 1;
                if seen.insert((kind.clone(), f.clone())) {
                    g.1 += 1;
                }
                if g.2.as_ref().is_none_or(|(sz, _, _)| size < *sz) {
                    g.2 = Some((size, file.clone(), d.clone()));
                }
            }
        }
    }
    let mut gv: Vec<_> = groups.into_iter().collect();
    gv.sort_by_key(|(_, (_, files, _))| std::cmp::Reverse(*files));
    let _ = writeln!(s, "\n### Differences by kind and function (sampled: at most 40 per file), most files first\n");
    let _ = writeln!(s, "| kind | function | cells | files | smallest file | cell | formula | Excel | ours |\n|---|---|---:|---:|---|---|---|---|---|");
    for ((kind, f), (n, files, ex)) in gv.iter().take(50) {
        let Some((_, file, d)) = ex else { continue };
        let _ = writeln!(
            s,
            "| {kind} | {f} | {n} | {files} | {} | {}!{} | `{}` | {} | {} |",
            md_cell(short_path(file)),
            md_cell(d["sheet"].as_str().unwrap_or("")),
            d["cell"].as_str().unwrap_or(""),
            md_cell(d["formula"].as_str().unwrap_or("")).replace('`', "'"),
            md_cell(d["excel"].as_str().unwrap_or("")),
            md_cell(d["ours"].as_str().unwrap_or(""))
        );
    }
    let crashes: Vec<&Json> = rows.iter().filter(|r| matches!(r["status"].as_str(), Some("crash" | "timeout"))).collect();
    if !crashes.is_empty() {
        let mut c = crashes.clone();
        c.sort_by_key(|r| r["size"].as_u64().unwrap_or(0));
        let _ = writeln!(s, "\n### Crashes and timeouts while checking ({})\n", crashes.len());
        let _ = writeln!(s, "| status | file | size | reason |\n|---|---|---:|---|");
        for r in c.iter().take(15) {
            let _ = writeln!(s, "| {} | {} | {} | {} |", r["status"].as_str().unwrap_or(""), md_cell(short_path(r["file"].as_str().unwrap_or(""))), r["size"], md_cell(r["reason"].as_str().unwrap_or("")));
        }
    }
    s
}

/// Cells by outcome, each cell once (the tallies count a cell once per
/// function it uses).
fn cell_outcomes(checked: &[&Json]) -> [u64; 5] {
    let mut out = [0u64; 5];
    for r in checked {
        if let Some(c) = r["cells"].as_array() {
            for k in 0..5 {
                out[k] += c.get(k).and_then(Json::as_u64).unwrap_or(0);
            }
        }
    }
    out
}

pub fn pages_report(rows: &[Json]) -> String {
    let mut s = String::new();
    let checked: Vec<&Json> = rows.iter().filter(|r| r["status"] == "checked").collect();
    let _ = writeln!(s, "## Pagination against Word\n");
    let _ = writeln!(s, "Documents (.docx) saved by Word whose `w:lastRenderedPageBreak` marks add up to `Pages` of docProps/app.xml. Our page starts come from `apexdesk info --page-starts`; paragraphs are matched between Word's XML and our model by their letters and digits. A page \"matches\" when it starts in the same paragraph.\n");
    let _ = writeln!(s, "Files: {} scanned, {} compared.\n", rows.len(), checked.len());
    let _ = writeln!(s, "| status: reason | files |\n|---|---:|");
    for (k, n) in counts(rows.iter().map(|r| reason_of(r)).collect::<Vec<_>>().iter().map(String::as_str)).into_iter().take(15) {
        let _ = writeln!(s, "| {} | {n} |", md_cell(&k));
    }
    let sum = |k: &str| checked.iter().map(|r| r[k].as_u64().unwrap_or(0)).sum::<u64>();
    let (compared, same, near, found) = (sum("compared"), sum("same"), sum("near"), sum("found"));
    let word_pages = sum("word_pages");
    let equal_count = checked.iter().filter(|r| r["word_pages"] == r["our_pages"]).count() as u64;
    let _ = writeln!(s, "\n**Pages: {} of {} start in the same paragraph as in Word ({}).**\n", same, compared, pct(same, compared));
    let _ = writeln!(s, "| measure | value |\n|---|---:|");
    let _ = writeln!(s, "| documents with Word's page count | {} of {} ({}) |", equal_count, checked.len(), pct(equal_count, checked.len() as u64));
    let _ = writeln!(s, "| pages starting in the same paragraph (page by page) | {} |", pct(same, compared));
    let _ = writeln!(s, "| pages starting within one paragraph | {} |", pct(near, compared));
    let _ = writeln!(s, "| Word page starts that are one of our page starts (any page) | {} of {} ({}) |", found, word_pages, pct(found, word_pages));
    let perfect = checked.iter().filter(|r| r["same"] == r["compared"] && r["word_pages"] == r["our_pages"]).count() as u64;
    let _ = writeln!(s, "| documents paginated exactly like Word | {} ({}) |", perfect, pct(perfect, checked.len() as u64));
    // Difference in page count.
    let mut diff: BTreeMap<i64, u64> = BTreeMap::new();
    for r in &checked {
        let d = r["our_pages"].as_i64().unwrap_or(0) - r["word_pages"].as_i64().unwrap_or(0);
        *diff.entry(d.clamp(-10, 10)).or_default() += 1;
    }
    let _ = writeln!(s, "\n### Our page count minus Word's (clamped to ±10)\n\n| difference | documents |\n|---:|---:|");
    for (d, n) in &diff {
        let _ = writeln!(s, "| {d:+} | {n} |");
    }
    // Where the first mismatch happens: smallest documents first.
    let mut bad: Vec<&&Json> = checked.iter().filter(|r| r["same"] != r["compared"] || r["word_pages"] != r["our_pages"]).collect();
    bad.sort_by_key(|r| r["size"].as_u64().unwrap_or(0));
    let _ = writeln!(s, "\n### Smallest documents that paginate differently\n");
    let _ = writeln!(s, "| file | size | Word pages | ours | first page that differs (Word para / ours) |\n|---|---:|---:|---:|---|");
    for r in bad.iter().take(20) {
        let first = r["rows"].as_array().and_then(|rows| {
            rows.iter().enumerate().find(|(_, x)| x[0] != x[1]).map(|(k, x)| format!("page {}: {} / {}", k + 1, x[0], x[1]))
        });
        let _ = writeln!(s, "| {} | {} | {} | {} | {} |", md_cell(short_path(r["file"].as_str().unwrap_or(""))), r["size"], r["word_pages"], r["our_pages"], first.unwrap_or_default());
    }
    let crashes: Vec<&Json> = rows.iter().filter(|r| matches!(r["status"].as_str(), Some("crash" | "timeout" | "error"))).collect();
    if !crashes.is_empty() {
        let mut c = crashes.clone();
        c.sort_by_key(|r| r["size"].as_u64().unwrap_or(0));
        let _ = writeln!(s, "\n### Errors, crashes and timeouts ({})\n", crashes.len());
        let _ = writeln!(s, "| status | file | size | reason |\n|---|---|---:|---|");
        for r in c.iter().take(15) {
            let _ = writeln!(s, "| {} | {} | {} | {} |", r["status"].as_str().unwrap_or(""), md_cell(short_path(r["file"].as_str().unwrap_or(""))), r["size"], md_cell(r["reason"].as_str().unwrap_or("")));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_reasons() {
        let e = "\nthread 'main' (1) panicked at crates/x.rs:3:5:\nindex out of bounds\nnote: run with `RUST_BACKTRACE=1`\n";
        assert_eq!(crash_reason(e), "crates/x.rs:3:5: index out of bounds");
        assert_eq!(crash_reason("memory allocation of 8 bytes failed\n"), "memory allocation of 8 bytes failed");
        assert_eq!(short_path("/a/target/corpus/enron/x.xlsx"), "enron/x.xlsx");
    }

    #[test]
    fn reports_render() {
        let rows = vec![
            json!({"file": "a.xls", "status": "checked", "calc": "auto", "size": 10, "formulas": 3, "cells": [2, 1, 0, 0, 0], "tally": {"SUM": [2, 1, 0, 0, 0]}, "diffs": [{"sheet": "S", "cell": "A1", "formula": "SUM(B1)", "excel": "1", "ours": "2", "kind": "value"}]}),
            json!({"file": "b.xls", "status": "skipped", "reason": "saved by 'X'", "size": 5}),
        ];
        let r = formulas_report(&rows);
        assert!(r.contains("3 formulas checked, 2 agree with Excel (66.7%)"), "{r}");
        assert!(r.contains("| value | SUM | 1 | 1 | a.xls |"), "{r}");
        let p = pages_report(&[json!({"file": "d.docx", "status": "checked", "size": 1, "word_pages": 2, "our_pages": 3, "compared": 2, "same": 1, "near": 2, "found": 1, "rows": [[0, 0], [5, 4], [null, 9]]})]);
        assert!(p.contains("1 of 2 start in the same paragraph"), "{p}");
        assert!(p.contains("page 2: 5 / 4"), "{p}");
    }
}
