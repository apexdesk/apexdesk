//! report.md of a run (summary and failures grouped by kind, each with
//! the smallest file that shows it) and the comparison of two runs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;
use std::path::Path;

use super::result::FileResult;

/// Statuses from worst to best (see `FileResult::status`).
const STATUSES: &[&str] = &["crash", "timeout", "memory", "open error", "save error", "loses content", "password", "unsupported", "ok"];

fn rank(status: &str) -> usize {
    STATUSES.iter().position(|s| *s == status).unwrap_or(0)
}

/// Where a failure group sorts: crashes first, lost content last.
fn severity(key: &str) -> usize {
    let mut w = key.split([' ', ':']);
    match (w.next().unwrap_or(""), w.next().unwrap_or("")) {
        (_, "panic" | "crash") => 0,
        (_, "timeout" | "memory") => 1,
        ("open", _) => 2,
        ("save" | "reopen", _) => 3,
        ("changed", "text" | "cells.value" | "cells.formula" | "sheets" | "pages") => 4,
        ("changed", _) => 5,
        ("external", _) => 6,
        _ => 7,
    }
}

pub fn read_jsonl(path: &Path) -> Result<Vec<FileResult>, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    s.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", path.display()))).collect()
}

struct Group {
    files: usize,
    smallest: (u64, String, String),
}

fn groups(results: &[FileResult]) -> Vec<(String, Group)> {
    let mut g: HashMap<String, Group> = HashMap::new();
    for r in results {
        let mut seen = BTreeSet::new();
        for (key, detail) in r.failures() {
            if !seen.insert(key.clone()) {
                continue;
            }
            let e = g.entry(key).or_insert(Group { files: 0, smallest: (u64::MAX, String::new(), String::new()) });
            e.files += 1;
            if r.size < e.smallest.0 {
                e.smallest = (r.size, r.path.clone(), detail);
            }
        }
    }
    let mut v: Vec<_> = g.into_iter().collect();
    v.sort_by(|a, b| severity(&a.0).cmp(&severity(&b.0)).then(b.1.files.cmp(&a.1.files)).then(a.0.cmp(&b.0)));
    v
}

fn percentile(v: &mut [u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn md(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// The whole report.
pub fn report(results: &[FileResult], dir: &str, notes: &[String]) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# apex-bench run\n\nCorpus: `{dir}` — {} files.\n", results.len());
    for n in notes {
        let _ = writeln!(o, "- {n}");
    }
    if !notes.is_empty() {
        o.push('\n');
    }
    // Status by extension.
    let mut by_ext: BTreeMap<String, BTreeMap<&str, usize>> = BTreeMap::new();
    let mut total: BTreeMap<&str, usize> = BTreeMap::new();
    for r in results {
        *by_ext.entry(r.ext()).or_default().entry(r.status()).or_default() += 1;
        *total.entry(r.status()).or_default() += 1;
    }
    let _ = writeln!(o, "## Summary\n");
    let _ = writeln!(o, "| status | files |\n|---|---:|");
    for s in STATUSES {
        let _ = writeln!(o, "| {s} | {} |", total.get(s).copied().unwrap_or(0));
    }
    let _ = writeln!(o, "\n| ext | files | {} |\n|---|---:|{}", STATUSES.join(" | "), "---:|".repeat(STATUSES.len()));
    for (ext, m) in &by_ext {
        let n: usize = m.values().sum();
        let cells: Vec<String> = STATUSES.iter().map(|s| m.get(s).map(|c| c.to_string()).unwrap_or_default()).collect();
        let _ = writeln!(o, "| {ext} | {n} | {} |", cells.join(" | "));
    }
    // Times and memory.
    let mut open: Vec<u64> = results.iter().filter(|r| r.open == super::result::Outcome::Ok).map(|r| r.open_ms).collect();
    let mut all: Vec<u64> = results.iter().map(|r| r.total_ms).collect();
    let mut mem: Vec<u64> = results.iter().map(|r| r.peak_mb).collect();
    let _ = writeln!(
        o,
        "\nOpening: median {} ms, 95% {} ms, max {} ms. Whole file (open, saves, reopens, pages): median {} ms, max {} ms. Peak memory: median {} MB, max {} MB.\n",
        percentile(&mut open, 0.5),
        percentile(&mut open, 0.95),
        open.last().copied().unwrap_or(0),
        percentile(&mut all, 0.5),
        all.last().copied().unwrap_or(0),
        percentile(&mut mem, 0.5),
        mem.last().copied().unwrap_or(0)
    );
    let mut slow: Vec<&FileResult> = results.iter().collect();
    slow.sort_by_key(|r| std::cmp::Reverse(r.open_ms));
    let _ = writeln!(o, "Slowest to open: {}\n", slow.iter().take(5).map(|r| format!("`{}` ({} ms, {} kB)", r.path, r.open_ms, r.size / 1024)).collect::<Vec<_>>().join(", "));
    slow.sort_by_key(|r| std::cmp::Reverse(r.peak_mb));
    let _ = writeln!(o, "Most memory: {}\n", slow.iter().take(5).map(|r| format!("`{}` ({} MB, {} kB)", r.path, r.peak_mb, r.size / 1024)).collect::<Vec<_>>().join(", "));
    let ext_n = results.iter().filter(|r| !r.external.is_empty()).count();
    if ext_n > 0 {
        let _ = writeln!(o, "External check (LibreOffice, OnlyOffice) on {ext_n} files.\n");
    }
    // Failures.
    let g = groups(results);
    let _ = writeln!(o, "## Failures by kind\n\n{} kinds. Each row: files with it, and the smallest one with an example.\n", g.len());
    let _ = writeln!(
        o,
        "`open`/`save`/`reopen`: the step failed (panic, timeout, memory, error). `changed K (a→b)`: K differs between the file as opened \
         and the file saved as .b and read back (`#N`: item N of a list, `Sheet!A1`: a cell, `∅`: missing). `pages`: our page count. \
         `external T`: program T (LibreOffice, OnlyOffice) sees other pages or words in what we saved than in the original.\n"
    );
    let _ = writeln!(o, "| kind | files | smallest file | example |\n|---|---:|---|---|");
    for (key, grp) in &g {
        let (size, path, detail) = &grp.smallest;
        let detail: String = detail.chars().take(200).collect();
        let _ = writeln!(o, "| {} | {} | `{}` ({} B) | {} |", md(key), grp.files, md(path), size, md(&detail));
    }
    o
}

/// What got better and worse from `old` to `new` (files in both).
pub fn compare(old: &[FileResult], new: &[FileResult]) -> String {
    let mut o = String::new();
    let old_by: HashMap<&str, &FileResult> = old.iter().map(|r| (r.path.as_str(), r)).collect();
    let mut better = Vec::new();
    let mut worse = Vec::new();
    let mut fixed: BTreeMap<String, usize> = BTreeMap::new();
    let mut broken: BTreeMap<String, usize> = BTreeMap::new();
    let mut both = 0;
    for n in new {
        let Some(p) = old_by.get(n.path.as_str()) else { continue };
        both += 1;
        let (a, b) = (rank(p.status()), rank(n.status()));
        if b > a {
            better.push(format!("`{}`: {} → {}", n.path, p.status(), n.status()));
        } else if b < a {
            worse.push(format!("`{}`: {} → {}", n.path, p.status(), n.status()));
        }
        let ka: BTreeSet<String> = p.failures().into_iter().map(|f| f.0).collect();
        let kb: BTreeSet<String> = n.failures().into_iter().map(|f| f.0).collect();
        for k in ka.difference(&kb) {
            *fixed.entry(k.clone()).or_default() += 1;
        }
        for k in kb.difference(&ka) {
            *broken.entry(k.clone()).or_default() += 1;
        }
    }
    let _ = writeln!(o, "## Compared with the previous run\n\n{both} files in both runs ({} only in the new one, {} only in the old one).", new.len() - both, old.len() - both);
    let _ = writeln!(o, "{} files better, {} files worse.\n", better.len(), worse.len());
    for (title, list) in [("Worse", &worse), ("Better", &better)] {
        if !list.is_empty() {
            let _ = writeln!(o, "### {title}\n");
            for l in list.iter().take(100) {
                let _ = writeln!(o, "- {l}");
            }
            if list.len() > 100 {
                let _ = writeln!(o, "- … and {} more", list.len() - 100);
            }
            o.push('\n');
        }
    }
    for (title, m) in [("Failures that appeared (files)", &broken), ("Failures that went away (files)", &fixed)] {
        if !m.is_empty() {
            let _ = writeln!(o, "### {title}\n");
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            for (k, n) in v.iter().take(60) {
                let _ = writeln!(o, "- {n} × {k}");
            }
            o.push('\n');
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::super::result::{Diff, Outcome, Target};
    use super::*;

    fn file(path: &str, size: u64, open: Outcome) -> FileResult {
        FileResult { path: path.into(), size, open, error: Some("bad 1".into()), ..Default::default() }
    }

    #[test]
    fn groups_keep_the_smallest() {
        let r = vec![file("a.xls", 300, Outcome::Error), file("b.xls", 100, Outcome::Error), file("c.xls", 50, Outcome::Ok)];
        let g = groups(&r);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].0, "open error: bad N");
        assert_eq!(g[0].1.files, 2);
        assert_eq!(g[0].1.smallest.1, "b.xls");
        let rep = report(&r, "x", &[]);
        assert!(rep.contains("| open error: bad N | 2 | `b.xls` (100 B) | bad 1 |"), "{rep}");
    }

    #[test]
    fn compare_finds_better_and_worse() {
        let old = vec![file("a.xls", 1, Outcome::Error), file("b.xls", 1, Outcome::Ok)];
        let mut b = file("b.xls", 1, Outcome::Ok);
        b.targets.push(Target { ext: "ods".into(), diffs: vec![Diff { kind: "text".into(), count: 1, detail: String::new() }], ..Default::default() });
        let new = vec![file("a.xls", 1, Outcome::Ok), b];
        let c = compare(&old, &new);
        assert!(c.contains("1 files better, 1 files worse."), "{c}");
        assert!(c.contains("1 × changed text (xls→ods)"), "{c}");
        assert!(c.contains("1 × open error: bad N"), "{c}");
    }

    #[test]
    fn crashes_sort_first() {
        assert!(severity("open panic: x") < severity("open error: y"));
        assert!(severity("changed text (docx→odt)") < severity("changed styles (docx→odt)"));
    }
}
