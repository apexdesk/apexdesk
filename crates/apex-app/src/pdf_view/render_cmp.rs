//! Compares pages drawn by our renderer with pdfium's, for a folder of
//! PDFs (run by hand):
//!
//! APEXDESK_CMP_DIR=corpus APEXDESK_CMP_OUT=out APEXDESK_PDFIUM=lib \
//!     apexdesk-test-binary render_cmp --ignored
//!
//! Writes, for each PDF, `NAME.png` (ours left, pdfium right) and a line
//! of `scores.tsv`: how different they are (0 the same, 255 opposite),
//! and the milliseconds each took. APEXDESK_CMP_ONLY picks files whose
//! name contains it; APEXDESK_CMP_WIDTH sets the width (800).

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    let Ok(file) = std::fs::File::create(path) else { return };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut wr) = enc.write_header() {
        let _ = wr.write_image_data(rgba);
    }
}

/// Mean difference of the two pictures' gray levels, over 4x4 blocks
/// (so antialiasing differences weigh little).
fn score(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    let gray = |p: &[u8], x: usize, y: usize| {
        let i = (y * w + x) * 4;
        (p[i] as u32 * 3 + p[i + 1] as u32 * 6 + p[i + 2] as u32) as f64 / 10.0
    };
    let mut total = 0.0;
    let mut n = 0.0;
    for by in (0..h.saturating_sub(3)).step_by(4) {
        for bx in (0..w.saturating_sub(3)).step_by(4) {
            let (mut sa, mut sb) = (0.0, 0.0);
            for y in by..by + 4 {
                for x in bx..bx + 4 {
                    sa += gray(a, x, y);
                    sb += gray(b, x, y);
                }
            }
            total += ((sa - sb) / 16.0).abs();
            n += 1.0;
        }
    }
    if n > 0.0 { total / n } else { 0.0 }
}

#[test]
#[ignore]
fn render_cmp() {
    let Some(dir) = std::env::var_os("APEXDESK_CMP_DIR") else { return };
    let out = std::path::PathBuf::from(std::env::var_os("APEXDESK_CMP_OUT").unwrap_or_else(|| "cmp-out".into()));
    let _ = std::fs::create_dir_all(&out);
    let only = std::env::var("APEXDESK_CMP_ONLY").unwrap_or_default();
    let width: u32 = std::env::var("APEXDESK_CMP_WIDTH").ok().and_then(|v| v.parse().ok()).unwrap_or(800);
    let page: usize = std::env::var("APEXDESK_CMP_PAGE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pdf")).collect();
    files.sort();
    let mut tsv = std::fs::OpenOptions::new().create(true).append(true).open(out.join("scores.tsv")).unwrap();
    for p in files {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        if !only.is_empty() && !name.contains(&only) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let bytes: Arc<[u8]> = Arc::from(bytes);
        let t0 = Instant::now();
        let theirs = super::engine::render_now(bytes.clone(), None, page, width);
        let t_theirs = t0.elapsed().as_millis();
        let t1 = Instant::now();
        let b2 = bytes.clone();
        let ours = std::panic::catch_unwind(move || apex_io::pdf::render::render(&b2, None, page, width, &super::sysfonts::font));
        let t_ours = t1.elapsed().as_millis();
        let line = match (ours, theirs) {
            (Err(_), _) => format!("{name}\tPANIC\t{t_ours}\t{t_theirs}"),
            // NONEP: pdfium draws it.
            (Ok(None), t) => format!("{name}\tNONE{}\t{t_ours}\t{t_theirs}", if t.is_some() { "P" } else { "" }),
            (Ok(Some(_)), None) => format!("{name}\tNOPDFIUM\t{t_ours}\t{t_theirs}"),
            (Ok(Some(o)), Some(t)) => {
                let (w, h) = (o.width as usize, o.height as usize);
                let (tw, th) = (t.size[0], t.size[1]);
                let hh = h.max(th);
                let mut side = vec![255u8; (w + tw + 8) * hh * 4];
                for y in 0..h {
                    let row = &o.rgba[y * w * 4..(y + 1) * w * 4];
                    side[y * (w + tw + 8) * 4..y * (w + tw + 8) * 4 + w * 4].copy_from_slice(row);
                }
                for y in 0..hh {
                    for x in w..w + 8 {
                        let i = (y * (w + tw + 8) + x) * 4;
                        side[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
                    }
                }
                for y in 0..th {
                    let row = &t.rgba[y * tw * 4..(y + 1) * tw * 4];
                    let at = (y * (w + tw + 8) + w + 8) * 4;
                    side[at..at + tw * 4].copy_from_slice(row);
                }
                write_png(&out.join(format!("{name}.png")), (w + tw + 8) as u32, hh as u32, &side);
                let s = if (w, h) == (tw, th) { score(&o.rgba, &t.rgba, w, h) } else { 999.0 };
                format!("{name}\t{s:.2}\t{t_ours}\t{t_theirs}")
            }
        };
        // APEXDESK_CMP_PAGES=n: also the time of our first n pages drawn
        // one after another from one opened document.
        let pages: usize = std::env::var("APEXDESK_CMP_PAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let line = if pages > 0 {
            let t2 = Instant::now();
            let b3 = bytes.clone();
            let ok = std::panic::catch_unwind(move || {
                let Some(mut d) = apex_io::pdf::render::Document::open(b3, None) else { return 0 };
                (0..d.page_count().min(pages)).filter(|&k| d.render(k, width, &super::sysfonts::font).is_some()).count()
            });
            let t_doc = t2.elapsed().as_millis();
            // The same pages each opened afresh, to compare.
            let t3 = Instant::now();
            let b4 = bytes.clone();
            let n = ok.unwrap_or(0);
            let _ = std::panic::catch_unwind(move || (0..n).for_each(|k| drop(apex_io::pdf::render::render(&b4, None, k, width, &super::sysfonts::font))));
            format!("{line}\t{t_doc}\t{}\t{n}", t3.elapsed().as_millis())
        } else {
            line
        };
        eprintln!("{line}");
        let _ = writeln!(tsv, "{line}");
    }
}
