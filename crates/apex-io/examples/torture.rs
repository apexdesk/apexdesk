//! Robustness check over a folder of real-world files:
//! `torture <dir> [out-dir]` opens every file, saves it in each compatible
//! format, re-opens the result, and reports errors, panics and slow files.
//! Build with `CARGO_PROFILE_RELEASE_PANIC=unwind` so panics are caught.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use apex_io::{Format, Kind, Loaded};

fn check(path: &Path, out: &Path) -> Result<String, String> {
    let t = Instant::now();
    let (loaded, fmt) = apex_io::open(path).map_err(|e| format!("open: {e}"))?;
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let targets: &[Format] = match fmt.kind() {
        Kind::Sheet => &[Format::Xlsx, Format::Ods, Format::Csv],
        Kind::Doc => &[Format::Docx, Format::Odt, Format::Markdown],
        Kind::Slides => &[Format::Pptx],
    };
    let summary = match &loaded {
        Loaded::Sheet(wb) => format!("{} sheets, {} cells", wb.sheets.len(), wb.sheets.iter().map(|s| s.cell_count()).sum::<usize>()),
        Loaded::Doc(d) => format!("{} paragraphs, {} words", d.paragraphs.len(), d.word_count()),
        Loaded::Slides(d) => format!("{} slides", d.slides.len()),
    };
    for &f in targets {
        let dst = out.join(format!("{}.{}", path.file_name().unwrap().to_string_lossy(), f.extension()));
        match &loaded {
            Loaded::Sheet(wb) => apex_io::save_sheet(wb, &dst, f),
            Loaded::Doc(d) => apex_io::save_doc(d, &dst, f),
            Loaded::Slides(d) => apex_io::save_slides(d, &dst, f),
        }
        .map_err(|e| format!("save {}: {e}", f.extension()))?;
        let (back, _) = apex_io::open(&dst).map_err(|e| format!("reopen {}: {e}", f.extension()))?;
        // Rich formats must keep the content.
        if f.is_rich() {
            match (&loaded, &back) {
                (Loaded::Sheet(a), Loaded::Sheet(b)) => {
                    // Cells with real content (formatting-only cells may be
                    // merged or dropped by a format without visible change).
                    let filled = |wb: &apex_sheet::Workbook| -> usize {
                        wb.sheets.iter().map(|s| s.iter().filter(|(_, c)| match &c.content {
                            apex_sheet::Content::Empty => false,
                            apex_sheet::Content::Formula { .. } => true,
                            apex_sheet::Content::Value(v) | apex_sheet::Content::Spill(v) => !matches!(v, apex_sheet::Value::Empty) && !matches!(v, apex_sheet::Value::Text(t) if t.is_empty()),
                        }).count()).sum()
                    };
                    let (ca, cb) = (filled(a), filled(b));
                    if a.sheets.len() != b.sheets.len() || ca != cb {
                        return Err(format!("roundtrip {}: {} sheets/{} cells -> {}/{}", f.extension(), a.sheets.len(), ca, b.sheets.len(), cb));
                    }
                }
                (Loaded::Doc(a), Loaded::Doc(b)) => {
                    if a.word_count() != b.word_count() {
                        return Err(format!("roundtrip {}: {} words -> {}", f.extension(), a.word_count(), b.word_count()));
                    }
                }
                (Loaded::Slides(a), Loaded::Slides(b)) => {
                    if a.slides.len() != b.slides.len() {
                        return Err(format!("roundtrip {}: {} slides -> {}", f.extension(), a.slides.len(), b.slides.len()));
                    }
                }
                _ => return Err("kind changed".into()),
            }
        }
    }
    Ok(format!("{load_ms:.0} ms, {summary}"))
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: torture <dir> [out-dir]"));
    let out = std::env::args().nth(2).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    std::panic::set_hook(Box::new(|_| {}));
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    let (mut ok, mut err, mut panics, mut hangs) = (0, 0, 0, 0);
    for f in files {
        let (tx, rx) = mpsc::channel();
        let (p, o) = (f.clone(), out.clone());
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| check(&p, &o));
            let _ = tx.send(r);
        });
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(Ok(s))) => {
                ok += 1;
                println!("OK     {name}: {s}");
            }
            Ok(Ok(Err(e))) => {
                err += 1;
                println!("ERROR  {name}: {e}");
            }
            Ok(Err(p)) => {
                panics += 1;
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                println!("PANIC  {name}: {msg}");
            }
            Err(_) => {
                hangs += 1;
                println!("HANG   {name}");
            }
        }
    }
    println!("\nok {ok}, errors {err}, panics {panics}, hangs {hangs}");
}
