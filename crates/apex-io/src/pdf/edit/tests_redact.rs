//! Redaction and sanitizing.

use super::annots::{Annot, Kind};
use super::tests::{file_of, sample, text_of};
use super::*;

/// The characters of page `page` with their x positions.
fn chars_of(data: &[u8], page: usize) -> Vec<(char, f32)> {
    crate::pdf::pages(data, None).unwrap()[page].chars.iter().map(|c| (c.ch, c.x)).collect()
}

#[test]
fn redaction_removes_the_text_under_it() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    let before = chars_of(&doc.to_bytes(), 0);
    let world_x = before.iter().find(|c| c.0 == 'w').unwrap().1;
    // "secret" runs from about 102.7 to 135.4 on the baseline at y 100.
    let r = doc.redact(&[(0, vec![[102.0, 88.0, 136.0, 104.0]])]);
    assert_eq!(r.glyphs, 6);
    assert!(!doc.saves_incrementally());
    let out = doc.to_bytes();
    let text = text_of(&out, 0);
    assert!(text.contains("Hello") && text.contains("world") && !text.contains("secret"), "{text}");
    // What follows stays where it was.
    let after = chars_of(&out, 0);
    let x = after.iter().find(|c| c.0 == 'w').unwrap().1;
    assert!((x - world_x).abs() < 0.01, "{x} {world_x}");
    // Nothing of it is left anywhere in the file.
    assert!(memchr::memmem::find(&out, b"secret").is_none());
    assert_eq!(text_of(&out, 1), "Second page");
    // pdftotext agrees, when it is there.
    let dir = apex_fs::temp_dir().join(format!("apex-redact-{}", apex_fs::process_id()));
    apex_fs::create_dir_all(&dir).unwrap();
    let f = dir.join("r.pdf");
    apex_fs::write(&f, &out).unwrap();
    if let Ok(o) = std::process::Command::new("pdftotext").arg(&f).arg("-").output()
        && o.status.success()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        assert!(s.contains("Hello") && !s.contains("secret"), "{s}");
    }
    apex_fs::remove_dir_all(&dir).ok();
    assert!(doc.undo());
    assert!(text_of(&doc.to_bytes(), 0).contains("secret"));
}

#[test]
fn redaction_marks_images_and_drawings() {
    // A 2 × 1 image over the page's top left, a box inside the area and
    // a line across it.
    let content = "q 200 0 0 100 0 600 cm /Im1 Do Q 0 0 1 rg 20 500 10 10 re f 0 G 0 520 m 300 520 l S";
    let data = file_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 792] /Resources << /XObject << /Im1 5 0 R >> >> /Contents 4 0 R >>",
        &format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 6 >>\nstream\n\u{7f}\u{7f}\u{7f}\u{7f}\u{7f}\u{7f}\nendstream",
    ]);
    let mut doc = PdfDoc::open(data, None).unwrap();
    // View y runs down: user y 480..700 is view 92..312; x 0..100 takes the
    // image's left pixel, the box and part of the line.
    let r = doc.redact(&[(0, vec![[0.0, 92.0, 100.0, 312.0]])]);
    assert_eq!((r.images, r.paths), (1, 2));
    let pn = doc.pages()[0];
    let res = doc.resources(pn);
    let xo = doc.key_dict(&res, "XObject").unwrap();
    assert_eq!(xo.0.len(), 1);
    let Obj::Stream(d, raw) = doc.resolve(&xo.0[0].1) else { panic!() };
    assert_eq!(doc.decode(&d, &raw), vec![0, 0, 0, 0x7f, 0x7f, 0x7f]);
    let c = String::from_utf8(doc.page_content(pn)).unwrap();
    assert!(!c.contains("20 500 10 10 re"), "{c}");
    assert!(c.contains("W*") && c.contains("300 520 l"), "{c}");
}

#[test]
fn applying_marks_and_sanitizing() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    doc.add_annot(&Annot { page: 0, kind: Kind::Redact, rect: [102.0, 88.0, 136.0, 104.0], quads: vec![[102.0, 88.0, 136.0, 104.0]], ..Default::default() });
    assert_eq!(doc.redaction_marks().len(), 1);
    let r = doc.apply_redactions();
    assert_eq!(r.glyphs, 6);
    assert!(doc.all_annots().is_empty());
    assert!(!text_of(&doc.to_bytes(), 0).contains("secret"));
    assert_eq!(doc.undo_names().first(), Some(&"pdf.undo.redact"));
    assert!(doc.trailer().get("Info").is_some());
    doc.sanitize();
    let out = doc.to_bytes();
    assert!(memchr::memmem::find(&out, b"Informe").is_none());
    doc.undo();
    assert!(doc.trailer().get("Info").is_some());
}

#[test]
fn redacted_form_fields_leave_the_form() {
    use super::tests::form_sample;
    let mut doc = PdfDoc::open(form_sample(), None).unwrap();
    // The "name" field (value "Ana") sits at [100, 72, 300, 92].
    doc.redact(&[(0, vec![[90.0, 60.0, 310.0, 100.0]])]);
    let names: Vec<String> = doc.fields().into_iter().map(|f| f.name).collect();
    assert!(!names.iter().any(|n| n == "name") && names.iter().any(|n| n == "agree"), "{names:?}");
    let out = doc.to_bytes();
    assert!(memchr::memmem::find(&out, b"(Ana)").is_none());
}

#[test]
fn redaction_drops_thumbnails_and_tags() {
    let data = file_of(&[
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Thumb 6 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        "<< /Type /StructTreeRoot /K << /S /P /ActualText (hidden name) >> >>",
        "<< /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\nX\nendstream",
    ]);
    let mut doc = PdfDoc::open(data, None).unwrap();
    doc.redact(&[(0, vec![[10.0, 10.0, 50.0, 50.0]])]);
    let out = doc.to_bytes();
    assert!(memchr::memmem::find(&out, b"hidden name").is_none());
    assert!(memchr::memmem::find(&out, b"/Thumb").is_none());
}

#[test]
fn printing_shows_what_is_shown() {
    let data = file_of(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [7 0 R 8 0 R] /D << /OFF [7 0 R] /AS [<< /Event /View /Category [/View] /OCGs [8 0 R] >> << /Event /Print /Category [/Print] /OCGs [7 0 R] >>] >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Annots [5 0 R 6 0 R 9 0 R] >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        "<< /Type /Annot /Subtype /FreeText /Rect [10 10 90 30] /F 36 /Contents (not seen) >>",
        "<< /Type /Annot /Subtype /Square /Rect [10 40 90 60] /F 0 /Contents (seen) >>",
        "<< /Type /OCG /Name (print only) /Usage << /Print << /PrintState /ON >> >> >>",
        "<< /Type /OCG /Name (view off) /Usage << /View << /ViewState /OFF >> >> >>",
        "<< /Type /Annot /Subtype /Square /Rect [10 70 90 90] /OC 8 0 R /Contents (layer) >>",
    ]);
    // Groups a view turns off are off here as well.
    let f = crate::pdf::PdfFile::open(&data, None).unwrap();
    let cat = f.catalog().unwrap();
    assert_eq!(crate::pdf::render::ocg_off_in(&cat, &|o| f.resolve(o)), [7, 8].into_iter().collect());
    let mut doc = PdfDoc::open(data.clone(), None).unwrap();
    doc.for_print();
    let out = doc.to_bytes_whole();
    let has = |s: &[u8]| memchr::memmem::find(&out, s).is_some();
    assert!(!has(b"not seen") && !has(b"(layer)") && has(b"(seen)"));
    assert!(!has(b"/AS") && !has(b"/Event"));
    let doc = PdfDoc::open(out, None).unwrap();
    let a = doc.all_annots();
    assert_eq!(a.len(), 1);
    let f = crate::pdf::PdfFile::open(doc.base(), None).unwrap();
    let cat = f.catalog().unwrap();
    assert_eq!(crate::pdf::render::ocg_off_in(&cat, &|o| f.resolve(o)).len(), 2);
}

#[test]
fn sanitizing_drops_associated_files_and_keeps_bookmarks() {
    let data = file_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AF [6 0 R] /Outlines 7 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /XObject << /Fm 5 0 R >> >> >>",
        "<< /Length 8 >>\nstream\nq /Fm Do Q\nendstream",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /PieceInfo << /Illustrator << /Private (OriginalArtwork) >> >> /Length 0 >>\nstream\n\nendstream",
        "<< /Type /Filespec /F (factura.xml) /EF << /F 10 0 R >> >>",
        "<< /Type /Outlines /First 8 0 R /Last 9 0 R /Count 2 >>",
        "<< /Title (First) /Parent 7 0 R /Next 9 0 R /Dest [3 0 R /Fit] >>",
        "<< /Title (Second) /Parent 7 0 R /Prev 8 0 R /Dest [3 0 R /Fit] >>",
        "<< /Type /EmbeddedFile /Length 14 >>\nstream\n<Invoice>9</I>\nendstream",
    ]);
    let mut doc = PdfDoc::open(data, None).unwrap();
    doc.sanitize();
    let out = doc.to_bytes();
    for gone in [&b"factura.xml"[..], b"Invoice", b"OriginalArtwork"] {
        assert!(memchr::memmem::find(&out, gone).is_none(), "{}", String::from_utf8_lossy(gone));
    }
    assert!(memchr::memmem::find(&out, b"(Second)").is_some());
}
