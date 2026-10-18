//! End-to-end PDF with real system fonts. Skips when none is installed.
//! Set APEX_PDF_OUT=/path/out.pdf to keep the file for manual checks
//! (pdfinfo, pdftotext, pdffonts, mutool, pdftoppm).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use apex_pdf::{FontId, LinkTarget, Pdf};

fn find(dir: &Path, pred: &dyn Fn(&str) -> bool, depth: u8) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in &entries {
        if p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(pred) {
            return Some(p.clone());
        }
    }
    if depth == 0 {
        return None;
    }
    entries.iter().filter(|p| p.is_dir()).find_map(|p| find(p, pred, depth - 1))
}

fn font_file(names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|n| find(Path::new("/usr/share/fonts"), &|f| f == *n, 4))
}

fn load(pdf: &mut Pdf, path: &Path, index: u32) -> Option<FontId> {
    let data: Arc<[u8]> = std::fs::read(path).ok()?.into();
    pdf.add_font(data, index)
}

/// Lays a string out with the font's advances; returns the glyph run.
fn run(pdf: &Pdf, font: FontId, size: f32, x: f32, text: &str) -> Vec<(char, f32)> {
    let mut x = x;
    text.chars()
        .map(|c| {
            let g = (c, x);
            x += pdf.advance(font, c) * size / 1000.0;
            g
        })
        .collect()
}

#[test]
fn two_page_document() {
    let Some(path) = font_file(&["DejaVuSans.ttf", "NotoSans-Regular.ttf", "LiberationSans-Regular.ttf"]) else {
        eprintln!("no system font found, skipping");
        return;
    };
    let mut pdf = Pdf::new();
    pdf.set_info("Prueba de exportación", "ApexDesk ñ");
    let font = load(&mut pdf, &path, 0).expect("system font parses");
    assert!(pdf.has_char(font, 'ñ') && pdf.has_char(font, 'Ж'));
    assert!(!pdf.has_char(font, '\u{10FFFD}'));

    // Page 1
    pdf.begin_page(595.0, 842.0);
    let black = [0, 0, 0];
    let lines = ["Canción ñandú €", "Ελληνικά: αβγδε ΩΣ", "Кириллица: Жизнь"];
    for (i, text) in lines.iter().enumerate() {
        let y = 100.0 + i as f32 * 30.0;
        pdf.glyphs(font, 20.0, y, &run(&pdf, font, 20.0, 72.0, text), black);
        // Baseline guide in light grey to check glyph placement visually.
        pdf.line(60.0, y, 400.0, y, 0.5, [200, 200, 200]);
    }
    pdf.glyphs_skewed(font, 20.0, 200.0, &run(&pdf, font, 20.0, 72.0, "Cursiva sintética"), [0, 0, 160], 0.21);
    pdf.fill_rect(72.0, 230.0, 100.0, 50.0, [230, 80, 20]);
    pdf.fill_rect_alpha(120.0, 250.0, 100.0, 50.0, [20, 80, 230], 0.5);
    pdf.dashed_line(72.0, 320.0, 300.0, 320.0, 2.0, [0, 128, 0], &[6.0, 3.0]);
    pdf.circle(400.0, 270.0, 40.0, Some([255, 220, 0]), Some((2.0, [0, 0, 0])));
    pdf.polygon(&[(450.0, 230.0), (520.0, 230.0), (485.0, 300.0)], true, None, Some((1.5, [128, 0, 128])));
    // RGBA gradient with an alpha ramp.
    let (w, h) = (64u32, 32u32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(x * 4) as u8, (y * 8) as u8, 200, (255 - x * 3) as u8]);
        }
    }
    let img = pdf.add_image(w, h, &px);
    pdf.image(img, 72.0, 340.0, 128.0, 64.0);
    // Clipped text: only the top half of the line shows.
    pdf.push_clip(72.0, 420.0, 300.0, 16.0);
    pdf.glyphs(font, 24.0, 440.0, &run(&pdf, font, 24.0, 72.0, "Recortado por la mitad"), black);
    pdf.pop_clip();
    // Rotated text: 90 degrees counter-clockwise about (520, 700).
    let (cx, cy) = (520.0f32, 700.0f32);
    let (s, c) = (-std::f32::consts::FRAC_PI_2.sin(), std::f32::consts::FRAC_PI_2.cos());
    pdf.push_transform([c, s, -s, c, cx - c * cx + s * cy, cy - s * cx - c * cy]);
    pdf.glyphs(font, 16.0, cy, &run(&pdf, font, 16.0, cx, "Texto girado 90°"), [160, 0, 0]);
    pdf.pop_transform();
    pdf.fill_rect(cx - 2.0, cy - 2.0, 4.0, 4.0, [255, 0, 0]);
    let link_text = "https://example.com/ñ";
    pdf.glyphs(font, 12.0, 500.0, &run(&pdf, font, 12.0, 72.0, link_text), [0, 0, 238]);
    pdf.link(72.0, 488.0, 200.0, 16.0, LinkTarget::Uri(link_text.to_string()));
    pdf.glyphs(font, 12.0, 530.0, &run(&pdf, font, 12.0, 72.0, "Ir a la página 2"), [0, 0, 238]);
    pdf.link(72.0, 518.0, 120.0, 16.0, LinkTarget::Page { page: 1, y: 200.0 });
    pdf.outline("Capítulo 1", 0, 0, 100.0);
    pdf.outline("Sección 1.1 Ελληνικά", 1, 0, 130.0);
    pdf.outline("Sección 1.2", 1, 0, 230.0);

    // Page 2: landscape, plus a collection (.ttc) and a CFF font if present.
    pdf.begin_page(842.0, 595.0);
    pdf.outline("Capítulo 2", 0, 1, 200.0);
    pdf.glyphs(font, 18.0, 200.0, &run(&pdf, font, 18.0, 72.0, "Página 2 — destino del enlace"), black);
    if let Some(ttc) = find(Path::new("/usr/share/fonts"), &|f| f.ends_with(".ttc"), 4) {
        if let Some(cjk) = load(&mut pdf, &ttc, 0) {
            let text = if pdf.has_char(cjk, '中') { "中文字体 日本語" } else { "TTC face" };
            pdf.glyphs(cjk, 18.0, 260.0, &run(&pdf, cjk, 18.0, 72.0, text), black);
        }
    }
    let cff = font_file(&["NimbusSans-Regular.otf", "lmroman10-regular.otf", "Cantarell-Regular.otf"]);
    if let Some(otf) = cff {
        if let Some(f) = load(&mut pdf, &otf, 0) {
            pdf.glyphs(f, 18.0, 320.0, &run(&pdf, f, 18.0, 72.0, "OpenType CFF: Canción ñandú"), black);
        }
    }
    // A char no font has still maps back through ToUnicode.
    pdf.glyphs(font, 18.0, 380.0, &run(&pdf, font, 18.0, 72.0, "falta:\u{10FFFD}."), black);
    assert_eq!(pdf.page_count(), 2);

    let out = pdf.finish();
    assert!(out.starts_with(b"%PDF-1.7"));
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("/FontFile2") && text.contains("/Identity-H"));
    assert!(text.contains("/Outlines") && text.contains("/URI"));
    if let Ok(dest) = std::env::var("APEX_PDF_OUT") {
        if let Some(dir) = Path::new(&dest).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&dest, &out).expect("write APEX_PDF_OUT");
    }
    eprintln!("pdf size: {} bytes", out.len());
}

#[test]
fn truncated_system_font_never_panics() {
    let Some(path) = font_file(&["DejaVuSans.ttf", "NotoSans-Regular.ttf", "LiberationSans-Regular.ttf"]) else {
        return;
    };
    let full = std::fs::read(&path).unwrap();
    for i in 0..64 {
        let len = full.len() * i / 64 + i;
        let data: Arc<[u8]> = full[..len.min(full.len())].to_vec().into();
        let mut pdf = Pdf::new();
        pdf.begin_page(100.0, 100.0);
        if let Some(f) = pdf.add_font(data, 0) {
            pdf.glyphs(f, 10.0, 50.0, &[('a', 0.0), ('é', 5.0), ('Ж', 10.0)], [0, 0, 0]);
        }
        let _ = pdf.finish();
    }
}
