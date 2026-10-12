//! `apex-bench big`: big files (see [`make`]) opened, recalculated,
//! paginated and saved by ApexDesk, each step timed, in a child process
//! whose peak memory is measured; the same files converted by LibreOffice
//! (headless) as a reference. Results in OUT/big/results.md, which the
//! fuzzer's report includes.

pub mod make;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use apex_io::{Format, Loaded};

use crate::fuzz;

pub const USAGE: &str = "apex-bench big [--out DIR] [--only sheet1m,xlsx200,doc1000,csv500] [--scale F]
                [--apexdesk PATH] [--no-lo] [--regen] [--mem MB]
One file at a time; ApexDesk stops past --mem (default 6144 MB of heap; the
command line and LibreOffice past 10 GB of address space) and that is
reported as a failure.
Writes the files to OUT/big/ (default OUT: target/bench/fuzz) unless they are there,
and measures ApexDesk and LibreOffice on them.";

struct Case {
    name: &'static str,
    file: &'static str,
    what: String,
}

pub fn main(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) == Some("--measure") {
        return measure(&args[1..]);
    }
    let mut out = PathBuf::from("target/bench/fuzz");
    let mut only: Vec<String> = Vec::new();
    let mut scale = 1.0f64;
    let mut apexdesk: Option<PathBuf> = None;
    let mut lo = true;
    let mut regen = false;
    let mut mem: u64 = 6144;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--out" => out = PathBuf::from(fuzz::value(args, &mut i, a)?),
            "--only" => only = fuzz::value(args, &mut i, a)?.split(',').map(str::to_string).collect(),
            "--scale" => scale = fuzz::value(args, &mut i, a)?.parse().map_err(|_| "bad --scale")?,
            "--apexdesk" => apexdesk = Some(PathBuf::from(fuzz::value(args, &mut i, a)?)),
            "--no-lo" => lo = false,
            "--regen" => regen = true,
            "--mem" => mem = fuzz::value(args, &mut i, a)?.parse().map_err(|_| "bad --mem")?,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {a}\n{USAGE}")),
        }
        i += 1;
    }
    let dir = fuzz::seeds::absolute(&out.join("big"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let apexdesk = apexdesk.or_else(|| exe.parent().map(|d| d.join(format!("apexdesk{}", std::env::consts::EXE_SUFFIX)))).filter(|p| p.exists());
    let rows = ((1_000_000f64 * scale) as u32).max(10);
    let cases = [
        Case { name: "sheet1m", file: "sheet-1m-rows.xlsx", what: format!("{rows} filas × 6 columnas, 4 con fórmulas (una en cadena)") },
        Case { name: "xlsx200", file: "big-200mb.xlsx", what: "20 columnas de números, fechas y textos".into() },
        Case { name: "doc1000", file: "doc-1000-pages.docx", what: format!("{} páginas: títulos, párrafos, tablas e imágenes", ((1000f64 * scale) as u32).max(2)) },
        Case { name: "csv500", file: "big-500mb.csv", what: "10 columnas mixtas".into() },
    ];
    if lo {
        // The first start of LibreOffice makes its profile: not measured.
        let tiny = dir.join("warm-up.csv");
        let _ = std::fs::write(&tiny, "a\n");
        let args: Vec<std::ffi::OsString> = vec!["--convert-to".into(), "ods".into(), "--outdir".into(), dir.join("work").into(), tiny.into()];
        let _ = fuzz::seeds::soffice(&dir.join("lo-profile"), &args, Duration::from_secs(300));
    }
    let mut rows_md = Vec::new();
    for c in &cases {
        if !only.is_empty() && !only.iter().any(|o| o == c.name) {
            continue;
        }
        let path = dir.join(c.file);
        if regen || !path.exists() {
            eprintln!("big: writing {}", path.display());
            let t = Instant::now();
            let r = match c.name {
                "sheet1m" => make::sheet_million(&path, rows),
                "xlsx200" => make::sheet_bytes(&path, (200e6 * scale) as u64).map(|_| ()),
                "doc1000" => make::document(&path, ((1000f64 * scale) as u32).max(2)),
                _ => make::csv(&path, (500e6 * scale) as u64).map(|_| ()),
            };
            r.map_err(|e| format!("{}: {e}", path.display()))?;
            eprintln!("big: written in {:.1} s", t.elapsed().as_secs_f64());
        }
        rows_md.push(bench(&exe, apexdesk.as_deref(), &path, c, &dir, lo, mem));
    }
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mut md = String::new();
    md.push_str("Tiempos en segundos y memoria máxima residente en MB (con los procesos hijos). ApexDesk: cada paso medido dentro de un proceso `apex-bench big --measure`; paginación = `apexdesk info` (abre y pagina, como el diálogo de imprimir) menos su apertura y menos el arranque de `info` con un fichero pequeño (fuentes). LibreOffice 1 proceso headless con perfil propio: «abrir+guardar» = `--convert-to` al mismo formato, «a PDF» = `--convert-to pdf` (abre, pagina y escribe el PDF).\n\n");
    md.push_str("| fichero | tamaño | contenido | ApexDesk abrir | recálculo | paginación (págs.) | guardar | memoria ApexDesk | LO abrir+guardar | LO a PDF (págs.) | memoria LO | notas |\n|---|--:|---|--:|--:|--:|--:|--:|--:|--:|--:|---|\n");
    // Each file's row is kept on its own (row-NAME.md), so that files
    // measured in separate runs, one at a time, make one table.
    for (c, r) in cases.iter().filter(|c| only.is_empty() || only.iter().any(|o| o == c.name)).zip(&rows_md) {
        let p = dir.join(c.file);
        let row = format!("| {} | {:.0} MB | {} | {r}\n", c.file, size(&p) as f64 / 1e6, c.what);
        std::fs::write(dir.join(format!("row-{}.md", c.name)), row).map_err(|e| e.to_string())?;
    }
    for c in &cases {
        if let Ok(row) = std::fs::read_to_string(dir.join(format!("row-{}.md", c.name))) {
            md.push_str(&row);
        }
    }
    std::fs::write(dir.join("results.md"), &md).map_err(|e| e.to_string())?;
    println!("{md}");
    fuzz::report::write(&out)
}

fn secs(v: Option<f64>) -> String {
    v.map(|s| format!("{s:.1}")).unwrap_or_else(|| "—".into())
}

/// Runs a child and returns its output, whether it ended well, the time
/// and its peak memory in MB.
fn timed(cmd: &mut Command, limit: Duration) -> (String, bool, f64, u64) {
    let start = Instant::now();
    let dir = std::env::temp_dir().join(format!("apex-bench-big-{}.out", std::process::id()));
    let Ok(file) = std::fs::File::create(&dir) else { return (String::new(), false, 0.0, 0) };
    let err = file.try_clone().ok();
    cmd.stdin(Stdio::null()).stdout(file);
    if let Some(e) = err {
        cmd.stderr(e);
    }
    fuzz::exec::limit_memory(cmd, 10 << 30);
    let Ok(mut ch) = cmd.spawn() else { return (String::new(), false, 0.0, 0) };
    let (ok, rss) = fuzz::exec::wait_limited(&mut ch, limit);
    let out = std::fs::read_to_string(&dir).unwrap_or_default();
    let _ = std::fs::remove_file(&dir);
    (out, ok, start.elapsed().as_secs_f64(), rss >> 10)
}

fn field(out: &str, key: &str) -> Option<f64> {
    out.lines().find_map(|l| l.strip_prefix(key).and_then(|v| v.trim().split_whitespace().next()?.parse().ok()))
}

/// One row of the table (from the ApexDesk columns on).
fn bench(exe: &Path, apexdesk: Option<&Path>, path: &Path, c: &Case, dir: &Path, lo: bool, mem: u64) -> String {
    let work = dir.join("work");
    let _ = std::fs::create_dir_all(&work);
    let mut notes = Vec::new();
    eprintln!("big: ApexDesk on {}", path.display());
    let (out, ok, _, wait_rss) = timed(Command::new(exe).args(["big", "--measure"]).arg(path).arg(&work).env("APEX_BENCH_MEM_LIMIT", (mem << 20).to_string()), Duration::from_secs(3600));
    // The child says its own peak (see `resident_peak_kb`); when it died,
    // what wait4 says (an upper bound).
    let rss = field(&out, "rss-peak ").map(|k| k as u64 >> 10).unwrap_or(wait_rss);
    if !ok {
        let last = out
            .lines()
            .find(|l| l.starts_with("APEX-CRASH"))
            .or_else(|| out.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("")
            .chars()
            .take(200)
            .collect::<String>();
        notes.push(format!("ApexDesk no terminó: {last}"));
    }
    let open = field(&out, "open ");
    let (recalc, save) = (field(&out, "recalc "), field(&out, "save "));
    if let Some(n) = field(&out, "cells ") {
        notes.push(format!("{n:.0} celdas leídas"));
    }
    if let Some(h) = field(&out, "heap-peak ") {
        notes.push(format!("montón máx. {:.0} MB", h / 1e6));
    }
    // Pagination with the command line.
    let mut pages = "—".to_string();
    if let (Some(app), Some(open)) = (apexdesk, open) {
        let tiny = work.join("tiny.txt");
        let _ = std::fs::write(&tiny, "x\n");
        let (_, _, base, _) = timed(Command::new(app).arg("info").arg(&tiny).env("LC_ALL", "C"), Duration::from_secs(300));
        eprintln!("big: apexdesk info on {}", path.display());
        let (iout, iok, isecs, irss) = timed(Command::new(app).arg("info").arg(path).env("LC_ALL", "C"), Duration::from_secs(3600));
        let n = iout.lines().find_map(|l| l.trim().strip_prefix("pages").map(|v| v.trim_start_matches([':', ' ']).trim().to_string())).unwrap_or_default();
        if iok {
            pages = format!("{:.1} ({n})", (isecs - base - open).max(0.0));
            notes.push(format!("`apexdesk info`: {isecs:.1} s, {irss} MB"));
        } else {
            pages = "no terminó".into();
            notes.push(format!("`apexdesk info` sin terminar en {isecs:.0} s"));
        }
    } else if apexdesk.is_none() {
        notes.push("sin binario apexdesk para paginar".into());
    }
    // LibreOffice.
    let (mut lo_save, mut lo_pdf, mut lo_rss) = ("—".to_string(), "—".to_string(), 0u64);
    if lo {
        let profile = dir.join("lo-profile");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let same = work.join("lo-same");
        let _ = std::fs::create_dir_all(&same);
        eprintln!("big: LibreOffice on {}", path.display());
        let args: Vec<std::ffi::OsString> = vec!["--convert-to".into(), ext.into(), "--outdir".into(), same.into(), path.into()];
        match fuzz::seeds::soffice(&profile, &args, Duration::from_secs(3600)) {
            Some((s, r)) => {
                lo_save = format!("{s:.1}");
                lo_rss = lo_rss.max(r >> 10);
            }
            None => lo_save = "falló".into(),
        }
        if ext == "docx" || c.name == "sheet1m" {
            let pdf_dir = work.join("lo-pdf");
            let _ = std::fs::create_dir_all(&pdf_dir);
            let args: Vec<std::ffi::OsString> = vec!["--convert-to".into(), "pdf".into(), "--outdir".into(), pdf_dir.clone().into(), path.into()];
            match fuzz::seeds::soffice(&profile, &args, Duration::from_secs(3600)) {
                Some((s, r)) => {
                    let pdf = pdf_dir.join(path.file_stem().unwrap_or_default()).with_extension("pdf");
                    let n = std::fs::read(&pdf).map(|d| count_pages(&d)).unwrap_or(0);
                    lo_pdf = format!("{s:.1} ({n})");
                    lo_rss = lo_rss.max(r >> 10);
                }
                None => lo_pdf = "falló".into(),
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    format!(
        "{} | {} | {pages} | {} | {rss} | {lo_save} | {lo_pdf} | {} | {} |",
        secs(open),
        secs(recalc),
        secs(save),
        if lo { lo_rss.to_string() } else { "—".into() },
        notes.join("; ")
    )
}

/// Pages of a PDF: its page objects.
fn count_pages(pdf: &[u8]) -> usize {
    let pat = b"/Type /Page";
    let pat2 = b"/Type/Page";
    let is_page = |i: usize, n: usize| pdf.get(i + n).is_none_or(|c| *c != b's');
    pdf.windows(pat.len()).enumerate().filter(|(i, w)| *w == pat && is_page(*i, pat.len())).count()
        + pdf.windows(pat2.len()).enumerate().filter(|(i, w)| *w == pat2 && is_page(*i, pat2.len())).count()
}

/// The child: `big --measure FILE WORK` prints the time of each step.
fn measure(args: &[String]) -> Result<(), String> {
    let [file, work, ..] = args else { return Err("usage: big --measure FILE WORK".into()) };
    let (file, work) = (Path::new(file), Path::new(work));
    if let Some(limit) = std::env::var("APEX_BENCH_MEM_LIMIT").ok().and_then(|v| v.parse().ok()) {
        fuzz::set_heap_limit(limit);
    }
    fuzz::reset_heap_peak();
    let t = Instant::now();
    let (loaded, fmt) = apex_io::open(file).map_err(|e| format!("open: {e}"))?;
    println!("open {:.3}", t.elapsed().as_secs_f64());
    println!("heap-open {}", fuzz::heap_peak());
    match loaded {
        Loaded::Sheet(mut wb) => {
            println!("cells {}", wb.sheets.iter().map(|s| s.cell_count()).sum::<usize>());
            let t = Instant::now();
            wb.recalc_all();
            println!("recalc {:.3}", t.elapsed().as_secs_f64());
            let target = if fmt == Format::Csv { Format::Csv } else { Format::Xlsx };
            let out = work.join(format!("saved.{}", target.extension()));
            let t = Instant::now();
            apex_io::save_sheet(&wb, &out, target).map_err(|e| format!("save: {e}"))?;
            println!("save {:.3}", t.elapsed().as_secs_f64());
        }
        Loaded::Doc(doc) => {
            println!("paragraphs {}", doc.paragraphs.len());
            let out = work.join("saved.docx");
            let t = Instant::now();
            apex_io::save_doc(&doc, &out, Format::Docx).map_err(|e| format!("save: {e}"))?;
            println!("save {:.3}", t.elapsed().as_secs_f64());
        }
        Loaded::Slides(deck) => {
            println!("slides {}", deck.slides.len());
            let out = work.join("saved.pptx");
            let t = Instant::now();
            apex_io::save_slides(&deck, &out, Format::Pptx).map_err(|e| format!("save: {e}"))?;
            println!("save {:.3}", t.elapsed().as_secs_f64());
        }
    }
    println!("heap-peak {}", fuzz::heap_peak());
    println!("rss-peak {}", fuzz::child::resident_peak_kb());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pdf_pages() {
        assert_eq!(super::count_pages(b"<< /Type /Pages /Count 2 >> << /Type /Page >> <</Type/Page/Parent 1 0 R>>"), 2);
    }
}
