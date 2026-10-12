//! What `apex-bench run` records for each file: one line of results.jsonl.

use serde::{Deserialize, Serialize};

/// How a step ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    Ok,
    /// Not reached (an earlier step failed).
    Skipped,
    /// An error returned by apex-io.
    Error,
    /// The file asks for a password (not a failure).
    Password,
    /// An extension ApexDesk doesn't read (.fods, .fodt...).
    Unsupported,
    Panic,
    Timeout,
    /// Killed past the memory limit.
    Memory,
    /// The process died without a panic message (abort, signal).
    Crash,
}

impl Outcome {
    pub fn is_failure(self) -> bool {
        matches!(self, Outcome::Error | Outcome::Panic | Outcome::Timeout | Outcome::Memory | Outcome::Crash)
    }

    pub fn name(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Skipped => "skipped",
            Outcome::Error => "error",
            Outcome::Password => "password",
            Outcome::Unsupported => "unsupported",
            Outcome::Panic => "panic",
            Outcome::Timeout => "timeout",
            Outcome::Memory => "memory",
            Outcome::Crash => "crash",
        }
    }
}

/// One difference between the original model and the one read back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    /// "text", "cells.value", "styles.cell", "pages"...
    pub kind: String,
    /// How many items differ.
    pub count: u64,
    /// The first few, as "where: before → after".
    pub detail: String,
}

/// The original saved in one format and read back.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Target {
    /// Extension written ("xlsx", "odt"...).
    pub ext: String,
    pub save: Outcome,
    pub save_ms: u64,
    pub reopen: Outcome,
    pub reopen_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Size of the saved file.
    #[serde(default)]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<Diff>,
    /// Our page count of the saved file (documents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u64>,
}

/// What another program sees in the original and in what we saved.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct External {
    /// "libreoffice" or "onlyoffice".
    pub tool: String,
    /// "orig" or a target extension.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u64>,
    /// Words of its text in common with the original's (0..=1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileResult {
    /// Relative to the corpus directory.
    pub path: String,
    pub size: u64,
    /// "sheet" or "doc" once opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Format detected by apex-io.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub open: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub open_ms: u64,
    pub total_ms: u64,
    /// Peak resident memory of the worker process, in MB.
    pub peak_mb: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Target>,
    /// Our page count of the original (documents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external: Vec<External>,
}

impl FileResult {
    pub fn ext(&self) -> String {
        std::path::Path::new(&self.path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
    }

    /// Every failure of the file as (group key, detail): what the report
    /// groups by and what `--compare` matches.
    pub fn failures(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let ext = self.ext();
        if self.open.is_failure() {
            out.push((format!("open {}: {}", self.open.name(), normalize(self.error.as_deref().unwrap_or(""))), self.error.clone().unwrap_or_default()));
        }
        for t in &self.targets {
            let conv = format!("{ext}→{}", t.ext);
            if t.save.is_failure() {
                out.push((format!("save {} ({conv}): {}", t.save.name(), normalize(t.error.as_deref().unwrap_or(""))), t.error.clone().unwrap_or_default()));
            } else if t.reopen.is_failure() {
                out.push((format!("reopen {} ({conv}): {}", t.reopen.name(), normalize(t.error.as_deref().unwrap_or(""))), t.error.clone().unwrap_or_default()));
            }
            for d in &t.diffs {
                out.push((format!("changed {} ({conv})", d.kind), d.detail.clone()));
            }
        }
        if let Some(e) = &self.pages_error {
            out.push((format!("pages error: {}", normalize(e)), e.clone()));
        }
        for x in &self.external {
            if x.file == "orig" {
                continue;
            }
            let orig = self.external.iter().find(|o| o.tool == x.tool && o.file == "orig");
            let conv = format!("{ext}→{}", x.file);
            if x.error.is_some() && orig.is_some_and(|o| o.error.is_none()) {
                out.push((format!("external {} can't open ({conv})", x.tool), x.error.clone().unwrap_or_default()));
                continue;
            }
            let Some(o) = orig else { continue };
            if let (Some(a), Some(b)) = (o.pages, x.pages)
                && a != b
            {
                out.push((format!("external {} pages ({conv})", x.tool), format!("{a} → {b}")));
            }
            if let Some(s) = x.similarity
                && s < 0.98
            {
                out.push((format!("external {} text ({conv})", x.tool), format!("{:.0}% of the words in common", s * 100.0)));
            }
        }
        out
    }

    /// The worst thing that happened, for the summary.
    pub fn status(&self) -> &'static str {
        let steps = std::iter::once(self.open).chain(self.targets.iter().flat_map(|t| [t.save, t.reopen]));
        let worst = |o: Outcome| steps.clone().any(|s| s == o);
        if worst(Outcome::Panic) || worst(Outcome::Crash) {
            "crash"
        } else if worst(Outcome::Timeout) {
            "timeout"
        } else if worst(Outcome::Memory) {
            "memory"
        } else if self.open == Outcome::Unsupported {
            "unsupported"
        } else if self.open == Outcome::Password {
            "password"
        } else if self.open == Outcome::Error {
            "open error"
        } else if self.targets.iter().any(|t| t.save.is_failure() || t.reopen.is_failure()) {
            "save error"
        } else if self.targets.iter().any(|t| !t.diffs.is_empty()) {
            "loses content"
        } else {
            "ok"
        }
    }
}

/// An error message with its numbers, paths and quoted parts blurred, so
/// that the same failure in different files groups together.
pub fn normalize(msg: &str) -> String {
    let first = msg.lines().next().unwrap_or("").trim();
    let mut out = String::new();
    let mut quote = None;
    let mut last_digit = false;
    for c in first.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
                out.push(c);
            }
            continue;
        }
        if c == '\'' || c == '"' || c == '`' {
            quote = Some(c);
            out.push(c);
            out.push('…');
            continue;
        }
        if c.is_ascii_digit() {
            if !last_digit {
                out.push('N');
            }
            last_digit = true;
            continue;
        }
        last_digit = false;
        out.push(c);
    }
    // Panic locations "src/x.rs:N:N" keep the file.
    let mut s: String = out.chars().take(140).collect();
    if out.chars().count() > 140 {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_blurs_numbers_and_quotes() {
        assert_eq!(normalize("bad record 0x0012 at 4096"), "bad record NxN at N");
        assert_eq!(normalize("missing part 'xl/sheet3.xml'\nmore"), "missing part '…'");
    }

    #[test]
    fn status_picks_the_worst() {
        let mut r = FileResult { path: "a.docx".into(), ..Default::default() };
        assert_eq!(r.status(), "ok");
        r.targets.push(Target { ext: "odt".into(), diffs: vec![Diff { kind: "text".into(), count: 1, detail: String::new() }], ..Default::default() });
        assert_eq!(r.status(), "loses content");
        assert_eq!(r.failures()[0].0, "changed text (docx→odt)");
        r.targets.push(Target { ext: "docx".into(), save: Outcome::Panic, error: Some("boom at 12".into()), ..Default::default() });
        assert_eq!(r.status(), "crash");
        assert_eq!(r.failures()[1].0, "save panic (docx→docx): boom at N");
    }
}
