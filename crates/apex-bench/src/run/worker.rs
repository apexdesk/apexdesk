//! The child process of `apex-bench run` (hidden subcommand
//! `__run-worker FILE OUTDIR TARGET...`): opens one file, saves it in each
//! target format, reads that back and compares. It reports each step as a
//! JSON line on stdout before doing it, so a panic, a hang or the memory
//! limit tell the parent where it happened.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use apex_io::{Format, Loaded};
use serde::{Deserialize, Serialize};

use super::diff::diff;
use super::model::{self, Digest};
use super::result::{Diff, Outcome};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum Event {
    /// About to run `step` ("open", "save", "reopen", "compare") of
    /// `target` ("" for the original).
    Step { target: String, step: String },
    Opened { kind: String, format: String, ms: u64 },
    OpenFailed { outcome: Outcome, msg: String },
    Saved { target: String, ms: u64, size: u64 },
    Reopened { target: String, ms: u64 },
    Failed { target: String, step: String, msg: String },
    Diffs { target: String, diffs: Vec<Diff> },
    /// Peak resident memory of the process, in kB.
    Peak { kb: u64 },
}

/// Extensions a file is saved to: its own format (keeping a macro or
/// template extension) and the other family's. Legacy and read-only
/// formats go to both rich ones; CSV only to CSV.
pub fn targets(path: &Path) -> Vec<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let own = |e: &str| vec![ext.clone(), e.to_string()];
    match ext.as_str() {
        "xlsx" | "xlsm" | "xltx" | "xltm" => own("ods"),
        "ods" | "ots" => own("xlsx"),
        "docx" | "docm" | "dotx" | "dotm" => own("odt"),
        "odt" | "ott" => own("docx"),
        "xls" | "xlt" | "xlsb" | "fods" => vec!["xlsx".into(), "ods".into()],
        "doc" | "dot" | "rtf" | "fodt" => vec!["docx".into(), "odt".into()],
        "csv" => vec!["csv".into()],
        "tsv" => vec!["tsv".into()],
        "pptx" | "pptm" | "potx" | "ppsx" => own("odp"),
        "odp" | "otp" => own("pptx"),
        _ => Vec::new(),
    }
}

fn emit(ev: &Event) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::to_string(ev).unwrap_or_default());
    let _ = out.flush();
}

fn step(target: &str, step: &str) {
    emit(&Event::Step { target: target.into(), step: step.into() });
}

fn ms(t: Instant) -> u64 {
    t.elapsed().as_millis() as u64
}

pub fn main(args: &[String]) -> Result<(), String> {
    let [file, out, targets @ ..] = args else { return Err("usage: __run-worker FILE OUTDIR TARGET...".into()) };
    work(Path::new(file), Path::new(out), targets);
    emit(&Event::Peak { kb: super::child::own_peak_kb() });
    Ok(())
}

fn work(file: &Path, out: &Path, targets: &[String]) {
    step("", "open");
    let t = Instant::now();
    let (loaded, fmt) = match apex_io::open(file) {
        Ok(l) => l,
        Err(e) => {
            let outcome = if e.needs_password() || e.rights_managed() {
                Outcome::Password
            } else if Format::from_path(file).is_none() {
                Outcome::Unsupported
            } else {
                Outcome::Error
            };
            emit(&Event::OpenFailed { outcome, msg: e.to_string() });
            return;
        }
    };
    let kind = if matches!(loaded, Loaded::Sheet(_)) { "sheet" } else { "doc" };
    emit(&Event::Opened { kind: kind.into(), format: fmt.extension().into(), ms: ms(t) });
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
    let mut full: Option<Digest> = None;
    for target in targets {
        let path = out.join(format!("{stem}.{target}"));
        let Some(tfmt) = Format::from_path(&path) else {
            emit(&Event::Failed { target: target.clone(), step: "save".into(), msg: format!("unknown target format {target}") });
            continue;
        };
        step(target, "save");
        let t = Instant::now();
        let saved = match &loaded {
            Loaded::Sheet(wb) => apex_io::save_sheet(wb, &path, tfmt),
            Loaded::Doc(doc) => apex_io::save_doc(doc, &path, tfmt),
            Loaded::Slides(deck) => apex_io::save_slides(deck, &path, tfmt),
        };
        if let Err(e) = saved {
            emit(&Event::Failed { target: target.clone(), step: "save".into(), msg: e.to_string() });
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        emit(&Event::Saved { target: target.clone(), ms: ms(t), size });
        step(target, "reopen");
        let t = Instant::now();
        let back = match apex_io::open(&path) {
            Ok((l, _)) => l,
            Err(e) => {
                emit(&Event::Failed { target: target.clone(), step: "reopen".into(), msg: e.to_string() });
                continue;
            }
        };
        emit(&Event::Reopened { target: target.clone(), ms: ms(t) });
        step(target, "compare");
        let values_only = matches!(tfmt, Format::Csv | Format::Tsv);
        let diffs = match (&loaded, &back) {
            (Loaded::Sheet(a), Loaded::Sheet(b)) if values_only => model::cells(a, b, true),
            (Loaded::Sheet(a), Loaded::Sheet(b)) => {
                let mut d = model::cells(a, b, false);
                d.extend(diff(full.get_or_insert_with(|| model::sheet(a)), &model::sheet(b)));
                d
            }
            (Loaded::Doc(a), Loaded::Doc(b)) => diff(full.get_or_insert_with(|| model::doc(a)), &model::doc(b)),
            (Loaded::Slides(a), Loaded::Slides(b)) => diff(full.get_or_insert_with(|| model::slides(a)), &model::slides(b)),
            _ => {
                emit(&Event::Failed { target: target.clone(), step: "reopen".into(), msg: "read back as another kind of file".into() });
                continue;
            }
        };
        // Macros can't go into .xlsx and .docx (Office wants .xlsm, .docm).
        let mut diffs = diffs;
        if matches!(target.as_str(), "xlsx" | "xltx" | "docx" | "dotx") {
            diffs.retain(|d| d.kind != "macros");
        }
        emit(&Event::Diffs { target: target.clone(), diffs });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_by_extension() {
        assert_eq!(targets(Path::new("a.XLSM")), vec!["xlsm", "ods"]);
        assert_eq!(targets(Path::new("a.doc")), vec!["docx", "odt"]);
        assert_eq!(targets(Path::new("a.csv")), vec!["csv"]);
        assert_eq!(targets(Path::new("a.pptx")), vec!["pptx", "odp"]);
        assert_eq!(targets(Path::new("a.odp")), vec!["odp", "pptx"]);
        assert!(targets(Path::new("a.pdf")).is_empty());
    }

    #[test]
    fn events_round_trip() {
        let e = Event::OpenFailed { outcome: Outcome::Password, msg: "x".into() };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"ev":"open_failed","outcome":"password","msg":"x"}"#);
        assert!(matches!(serde_json::from_str::<Event>(&s).unwrap(), Event::OpenFailed { outcome: Outcome::Password, .. }));
    }
}
