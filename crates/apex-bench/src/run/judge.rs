//! `apex-bench judge`: presentations against LibreOffice Impress. Each
//! pptx or odp of the corpus goes to PDF twice, through our `apexdesk
//! convert` (timed, with its peak memory) and through a headless
//! LibreOffice; the pages and words of both are compared. The files we
//! save in pptx and odp go through LibreOffice too, and must still give
//! the pages and words it saw in the original (the round trip as another
//! program reads it). LibreOffice runs one process at a time, over
//! batches of files, with its own profile.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{child, external};

const USAGE: &str = "Usage: apex-bench judge DIR [options]

Converts every presentation of DIR (pptx, pptm, potx, ppsx, odp, otp) to
PDF with apexdesk and with LibreOffice, and compares pages and words;
also saves each in pptx and odp and has LibreOffice read those back.

Options:
  --soffice PATH     LibreOffice with Impress (default: soffice; see
                     tools/lo-private.sh when the system's lacks Impress)
  --out DIR          report directory (default: target/bench/judge)
  -j N               apexdesk processes at a time (default 4, at most 6)
  --timeout SECS     per conversion (default 120)
  --mem MB           memory limit of apexdesk (default 3072)
  --limit N          only the first N files
  --filter TEXT      only files whose path contains TEXT
  --apexdesk PATH    apexdesk binary (default: next to apex-bench)
  --batch N          files per LibreOffice process (default 25)";

/// Words in common under this are a text difference.
const SAME_TEXT: f64 = 0.9;

struct Opts {
    dir: PathBuf,
    out: PathBuf,
    soffice: String,
    jobs: usize,
    timeout: Duration,
    mem_mb: u64,
    limit: Option<usize>,
    filter: Option<String>,
    apexdesk: Option<String>,
    batch: usize,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        dir: PathBuf::new(),
        out: PathBuf::from("target/bench/judge"),
        soffice: if cfg!(windows) { "soffice.exe" } else { "soffice" }.into(),
        jobs: 4,
        timeout: Duration::from_secs(120),
        mem_mb: 3072,
        limit: None,
        filter: None,
        apexdesk: None,
        batch: 25,
    };
    let mut pos = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        let num = |v: String| v.parse::<u64>().map_err(|_| format!("{a}: not a number: {v}"));
        match a.as_str() {
            "--soffice" => o.soffice = val()?,
            "--out" => o.out = PathBuf::from(val()?),
            "-j" | "--jobs" => o.jobs = num(val()?)?.clamp(1, 6) as usize,
            "--timeout" => o.timeout = Duration::from_secs(num(val()?)?),
            "--mem" => o.mem_mb = num(val()?)?,
            "--limit" => o.limit = Some(num(val()?)? as usize),
            "--filter" => o.filter = Some(val()?),
            "--apexdesk" => o.apexdesk = Some(val()?),
            "--batch" => o.batch = num(val()?)?.max(1) as usize,
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

/// Whether the judge takes this file, and its family ("pptx" or "odp").
fn family(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pptx" | "pptm" | "potx" | "ppsx" => Some("pptx"),
        "odp" | "otp" => Some("odp"),
        _ => None,
    }
}

/// What one PDF gave: its pages and words, or why there is none.
#[derive(Default, Clone)]
struct Pdf {
    pages: Option<u64>,
    words: Vec<String>,
    error: Option<String>,
}

impl Pdf {
    fn read(pdf: Result<PathBuf, String>) -> Pdf {
        match pdf.and_then(|p| external::read_pdf(&p)) {
            Ok((pages, mut words)) => {
                // Bullets and symbols are drawn, each program with its own
                // characters (Wingdings' private ones, or what they mean).
                words.retain(|w| w.chars().any(char::is_alphanumeric));
                Pdf { pages: Some(pages), words, error: None }
            }
            Err(e) => Pdf { error: Some(e), ..Default::default() },
        }
    }
}

/// One file's results.
#[derive(Default)]
struct Row {
    file: String,
    family: &'static str,
    /// apexdesk: open and export to PDF, in ms, and its peak memory in MB.
    ms: u64,
    peak_mb: u64,
    ours: Pdf,
    lo: Pdf,
    /// Our pptx and odp as LibreOffice reads them (missing: we could not
    /// save it).
    saved: Vec<(&'static str, Pdf)>,
    save_errors: Vec<(&'static str, String)>,
}

/// Runs apexdesk convert; the path of what it wrote.
fn convert(apexdesk: &Path, input: &Path, to: &str, out_dir: &Path, timeout: Duration, mem_mb: u64) -> (Result<PathBuf, String>, u64, u64) {
    let _ = std::fs::create_dir_all(out_dir);
    let mut cmd = Command::new(apexdesk);
    cmd.arg("convert").arg(input).args(["--to", to, "-q", "--out"]).arg(out_dir);
    let start = Instant::now();
    let exit = match child::run(cmd, timeout, mem_mb) {
        Ok(e) => e,
        Err(e) => return (Err(format!("apexdesk: {e}")), 0, 0),
    };
    let ms = start.elapsed().as_millis() as u64;
    let out = out_dir.join(input.file_stem().unwrap_or_default()).with_extension(to);
    let r = match exit.killed {
        Some(child::Killed::Timeout) => Err("timeout".into()),
        Some(child::Killed::Memory) => Err("memory limit".into()),
        None if exit.panic().is_some() => Err(format!("panic: {}", exit.panic().unwrap_or_default())),
        None if out.exists() && exit.success() => Ok(out),
        None => Err(format!("convert failed: {}", exit.stderr_tail())),
    };
    (r, ms, exit.peak_kb / 1024)
}

/// Converts `files` to PDF in `out_dir` with one LibreOffice process;
/// files it left without a PDF go again one by one, so that a file that
/// crashes it fails alone. The PDF of each file, or why there is none.
fn libreoffice(soffice: &str, profile: &Path, files: &[PathBuf], out_dir: &Path, timeout: Duration) -> Vec<Result<PathBuf, String>> {
    let pdf_of = |f: &Path| out_dir.join(f.file_stem().unwrap_or_default()).with_extension("pdf");
    let run = |batch: &[PathBuf], timeout: Duration| -> Result<(), String> {
        let mut cmd = Command::new(soffice);
        cmd.arg(format!("-env:UserInstallation=file://{}", profile.display()))
            .args(["--headless", "--norestore", "--convert-to", "pdf", "--outdir"])
            .arg(out_dir)
            .args(batch);
        let exit = child::run(cmd, timeout, 0).map_err(|e| format!("soffice: {e}"))?;
        match exit.killed {
            Some(k) => Err(format!("{k:?}").to_lowercase()),
            None if exit.success() => Ok(()),
            None => Err(format!("soffice failed: {}", exit.stderr_tail())),
        }
    };
    // A batch that hangs on one file must not hold up the rest for long:
    // those left without a PDF go again one by one.
    let _ = run(files, timeout + Duration::from_secs(5) * files.len() as u32);
    files
        .iter()
        .map(|f| {
            let pdf = pdf_of(f);
            if pdf.exists() {
                return Ok(pdf);
            }
            match run(std::slice::from_ref(f), timeout) {
                _ if pdf.exists() => Ok(pdf),
                Ok(()) => Err("LibreOffice wrote no PDF".into()),
                Err(e) => Err(e),
            }
        })
        .collect()
}

pub fn main(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let mut files: Vec<String> = super::list_files(&o.dir)?.into_iter().map(|(p, _)| p).filter(|p| family(p).is_some()).collect();
    if let Some(f) = &o.filter {
        files.retain(|p| p.contains(f.as_str()));
    }
    if let Some(n) = o.limit {
        files.truncate(n);
    }
    let apexdesk = super::pages::find(o.apexdesk.as_deref()).ok_or("no apexdesk binary next to apex-bench (build it, or give --apexdesk)")?;
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let out = std::fs::canonicalize(&o.out).map_err(|e| e.to_string())?;
    let work = out.join("work");
    let _ = std::fs::remove_dir_all(&work);
    let start = Instant::now();

    // apexdesk, several processes at a time: PDF, pptx and odp of each file.
    let rows: Arc<Mutex<BTreeMap<usize, Row>>> = Arc::default();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..o.jobs {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(path) = files.get(i) else { break };
                    let row = ours(&o, &apexdesk, &work, i, path);
                    let mut rows = rows.lock().unwrap_or_else(|e| e.into_inner());
                    rows.insert(i, row);
                    if rows.len() % 50 == 0 {
                        eprintln!("apex-bench judge: apexdesk {}/{} files, {:.0} s", rows.len(), files.len(), start.elapsed().as_secs_f64());
                    }
                }
            });
        }
    });
    let mut rows = Arc::try_unwrap(rows).map_err(|_| "threads left")?.into_inner().unwrap_or_else(|e| e.into_inner());

    // LibreOffice, one process at a time: the original and our two files.
    let profile = work.join("lo-profile");
    let jobs: Vec<(usize, Option<&'static str>, PathBuf)> = rows
        .iter()
        .flat_map(|(i, r)| {
            let dir = work.join(format!("f{i}"));
            let orig = dir.join(format!("orig.{}", ext_of(&r.file)));
            std::iter::once((*i, None, orig)).chain(r.saved.iter().map(move |(fam, _)| (*i, Some(*fam), dir.join(format!("ours-{fam}.{fam}")))))
        })
        .collect();
    // Unique names in one folder, as LibreOffice names the PDF after the file.
    let lo_in = work.join("lo-in");
    let lo_out = work.join("lo-out");
    let _ = std::fs::create_dir_all(&lo_in);
    let _ = std::fs::create_dir_all(&lo_out);
    let mut inputs = Vec::new();
    for (n, (_, _, p)) in jobs.iter().enumerate() {
        let to = lo_in.join(format!("{n:05}.{}", p.extension().and_then(|e| e.to_str()).unwrap_or("bin")));
        let _ = std::fs::copy(p, &to);
        inputs.push(to);
    }
    let mut done = 0;
    for (chunk_jobs, chunk_inputs) in jobs.chunks(o.batch).zip(inputs.chunks(o.batch)) {
        let pdfs = libreoffice(&o.soffice, &profile, chunk_inputs, &lo_out, o.timeout);
        for ((i, fam, _), pdf) in chunk_jobs.iter().zip(pdfs) {
            let pdf = Pdf::read(pdf);
            let row = rows.get_mut(i).expect("row");
            match fam {
                None => row.lo = pdf,
                Some(f) => {
                    if let Some(slot) = row.saved.iter_mut().find(|(g, _)| g == f) {
                        slot.1 = pdf;
                    }
                }
            }
        }
        done += chunk_jobs.len();
        eprintln!("apex-bench judge: LibreOffice {done}/{} files, {:.0} s", jobs.len(), start.elapsed().as_secs_f64());
    }

    let rows: Vec<Row> = rows.into_values().collect();
    let jsonl = out.join("results.jsonl");
    let mut w = std::io::BufWriter::new(std::fs::File::create(&jsonl).map_err(|e| e.to_string())?);
    for r in &rows {
        let _ = writeln!(w, "{}", json(r));
    }
    drop(w);
    let notes = format!("{} files in {:.0} s; LibreOffice: `{}`.", rows.len(), start.elapsed().as_secs_f64(), o.soffice);
    std::fs::write(out.join("report.md"), report(&rows, &notes)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&work);
    eprintln!("apex-bench judge: {} and {}", out.join("report.md").display(), jsonl.display());
    Ok(())
}

fn ext_of(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("bin").to_ascii_lowercase()
}

/// Our side of one file: its PDF (timed) and its pptx and odp.
fn ours(o: &Opts, apexdesk: &Path, work: &Path, i: usize, path: &str) -> Row {
    let fam = family(path).unwrap_or("pptx");
    let mut row = Row { file: path.to_string(), family: fam, ..Default::default() };
    let dir = work.join(format!("f{i}"));
    let _ = std::fs::create_dir_all(&dir);
    let orig = dir.join(format!("orig.{}", ext_of(path)));
    if let Err(e) = std::fs::copy(o.dir.join(path), &orig) {
        row.ours.error = Some(e.to_string());
        return row;
    }
    let (pdf, ms, peak) = convert(apexdesk, &orig, "pdf", &dir.join("pdf"), o.timeout, o.mem_mb);
    row.ms = ms;
    row.peak_mb = peak;
    row.ours = Pdf::read(pdf);
    for to in ["pptx", "odp"] {
        let (saved, _, peak) = convert(apexdesk, &orig, to, &dir.join(to), o.timeout, o.mem_mb);
        row.peak_mb = row.peak_mb.max(peak);
        match saved.and_then(|p| std::fs::rename(&p, dir.join(format!("ours-{to}.{to}"))).map_err(|e| e.to_string())) {
            Ok(()) => row.saved.push((to, Pdf::default())),
            Err(e) => row.save_errors.push((to, e)),
        }
    }
    row
}

fn json(r: &Row) -> String {
    let pdf = |p: &Pdf, base: Option<&Pdf>| {
        serde_json::json!({
            "pages": p.pages,
            "similarity": base.filter(|b| b.pages.is_some() && p.pages.is_some()).map(|b| (external::similarity(&b.words, &p.words) * 1000.0).round() / 1000.0),
            "error": p.error,
        })
    };
    let mut saved = serde_json::Map::new();
    for (f, p) in &r.saved {
        saved.insert(f.to_string(), pdf(p, Some(&r.lo)));
    }
    for (f, e) in &r.save_errors {
        saved.insert(f.to_string(), serde_json::json!({ "error": format!("save: {e}") }));
    }
    serde_json::json!({
        "file": r.file,
        "ms": r.ms,
        "peak_mb": r.peak_mb,
        "ours": pdf(&r.ours, Some(&r.lo)),
        "libreoffice": pdf(&r.lo, None),
        "saved": saved,
    })
    .to_string()
}

/// What went wrong with one file, as short labels (empty: all agree).
fn problems(r: &Row) -> Vec<String> {
    let mut out = Vec::new();
    match (&r.ours.error, &r.lo.error) {
        (Some(e), _) => out.push(format!("ours: {}", short(e))),
        (None, Some(_)) => {}
        (None, None) => {
            if r.ours.pages != r.lo.pages {
                out.push("pages differ".into());
            }
            if external::similarity(&r.lo.words, &r.ours.words) < SAME_TEXT {
                out.push("text differs".into());
            }
        }
    }
    for (f, e) in &r.save_errors {
        out.push(format!("{f}: save failed ({})", short(e)));
    }
    if r.lo.error.is_none() {
        for (f, p) in &r.saved {
            if let Some(e) = &p.error {
                out.push(format!("{f}: LibreOffice can't read ours ({})", short(e)));
            } else {
                if p.pages != r.lo.pages {
                    out.push(format!("{f}: pages change"));
                }
                if external::similarity(&r.lo.words, &p.words) < SAME_TEXT {
                    out.push(format!("{f}: text changes"));
                }
            }
        }
    }
    out
}

fn short(e: &str) -> String {
    let e = e.lines().next().unwrap_or("");
    if e.chars().count() > 60 { e.chars().take(60).collect::<String>() + "…" } else { e.to_string() }
}

fn percentile(v: &mut [u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn report(rows: &[Row], notes: &str) -> String {
    let mut s = String::from("# Presentations against LibreOffice\n\n");
    s.push_str(notes);
    s.push_str("\n\n| | pptx | odp | all |\n|---|---:|---:|---:|\n");
    let fams: [&[&str]; 3] = [&["pptx"], &["odp"], &["pptx", "odp"]];
    type Test = Box<dyn Fn(&Row) -> bool>;
    let both = |r: &Row| r.ours.error.is_none() && r.lo.error.is_none();
    let saved_ok = |f: &'static str| {
        move |r: &Row| r.lo.error.is_none() && r.saved.iter().any(|(g, p)| *g == f && p.pages == r.lo.pages && external::similarity(&r.lo.words, &p.words) >= SAME_TEXT)
    };
    let lines: Vec<(&str, Test)> = vec![
        ("files", Box::new(|_| true)),
        ("LibreOffice opens", Box::new(|r| r.lo.error.is_none())),
        ("we open and export", Box::new(|r| r.ours.error.is_none())),
        ("same pages", Box::new(move |r| both(r) && r.ours.pages == r.lo.pages)),
        ("same text (≥ 90 % words)", Box::new(move |r| both(r) && external::similarity(&r.lo.words, &r.ours.words) >= SAME_TEXT)),
        ("our pptx: same pages and text in LibreOffice", Box::new(saved_ok("pptx"))),
        ("our odp: same pages and text in LibreOffice", Box::new(saved_ok("odp"))),
    ];
    for (name, test) in &lines {
        s.push_str(&format!("| {name} |"));
        for f in fams {
            let n = rows.iter().filter(|r| f.contains(&r.family) && test(r)).count();
            s.push_str(&format!(" {n} |"));
        }
        s.push('\n');
    }
    let mut ms: Vec<u64> = rows.iter().filter(|r| r.ours.error.is_none()).map(|r| r.ms).collect();
    let mut mb: Vec<u64> = rows.iter().filter(|r| r.ours.error.is_none()).map(|r| r.peak_mb).collect();
    s.push_str(&format!(
        "\napexdesk open + PDF: median {} ms, 95 % {} ms, max {} ms; peak memory median {} MB, 95 % {} MB, max {} MB.\n",
        percentile(&mut ms, 0.5),
        percentile(&mut ms, 0.95),
        ms.last().copied().unwrap_or(0),
        percentile(&mut mb, 0.5),
        percentile(&mut mb, 0.95),
        mb.last().copied().unwrap_or(0)
    ));
    // The most frequent problems, then every file with one.
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for r in rows {
        for p in problems(r) {
            let key = p.split(" (").next().unwrap_or(&p).to_string();
            *count.entry(key).or_default() += 1;
        }
    }
    let mut count: Vec<(String, usize)> = count.into_iter().collect();
    count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    s.push_str("\n## Problems\n\n| problem | files |\n|---|---:|\n");
    for (p, n) in &count {
        s.push_str(&format!("| {p} | {n} |\n"));
    }
    s.push_str("\n## Files\n\n| file | LibreOffice pages | ours | words | problems |\n|---|---:|---:|---:|---|\n");
    for r in rows {
        let p = problems(r);
        if p.is_empty() {
            continue;
        }
        let pages = |x: &Pdf| x.pages.map(|n| n.to_string()).unwrap_or("-".into());
        let sim = if both(r) { format!("{:.0} %", external::similarity(&r.lo.words, &r.ours.words) * 100.0) } else { "-".into() };
        s.push_str(&format!("| {} | {} | {} | {sim} | {} |\n", r.file, pages(&r.lo), pages(&r.ours), p.join("; ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(pages: u64, text: &str) -> Pdf {
        Pdf { pages: Some(pages), words: text.split_whitespace().map(String::from).collect(), error: None }
    }

    #[test]
    fn families_and_problems() {
        assert_eq!(family("a/b.PPTX"), Some("pptx"));
        assert_eq!(family("b.otp"), Some("odp"));
        assert_eq!(family("b.docx"), None);
        let mut r = Row { file: "x.pptx".into(), family: "pptx", ours: pdf(2, "a b c"), lo: pdf(2, "a b c"), ..Default::default() };
        r.saved = vec![("pptx", pdf(2, "a b c")), ("odp", pdf(3, "a b c"))];
        assert_eq!(problems(&r), ["odp: pages change"]);
        r.ours = pdf(1, "a");
        assert_eq!(problems(&r), ["pages differ", "text differs", "odp: pages change"]);
        // When LibreOffice can't open the original, nothing is compared.
        r.lo = Pdf { error: Some("no PDF".into()), ..Default::default() };
        assert!(problems(&r).is_empty());
        let rep = report(&[r], "");
        assert!(rep.contains("| files | 1 | 0 | 1 |"), "{rep}");
    }
}
