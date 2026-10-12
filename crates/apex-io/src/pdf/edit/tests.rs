use super::annots::{Annot, Kind};
use super::*;
use crate::pdf::tests::{pdf, t};

pub(crate) fn sample() -> Vec<u8> {
    pdf(&[t("F1", 12.0, 72.0, 100.0, "Hello secret world"), t("F1", 12.0, 72.0, 100.0, "Second page")])
}

pub(crate) fn text_of(data: &[u8], page: usize) -> String {
    crate::pdf::pages(data, None).unwrap()[page].chars.iter().filter(|c| !c.hidden).map(|c| c.ch).collect()
}

#[test]
fn open_and_pages() {
    let doc = PdfDoc::open(sample(), None).unwrap();
    assert_eq!(doc.pages().len(), 2);
    let g = doc.geom(doc.pages()[0]);
    assert_eq!((g.width, g.height), (612.0, 792.0));
    assert_eq!(g.to_view(72.0, 692.0), [72.0, 100.0]);
    assert!(!doc.modified());
}

#[test]
fn annotate_save_incrementally_and_reopen() {
    let data = sample();
    let mut doc = PdfDoc::open(data.clone(), None).unwrap();
    let a = Annot { page: 0, kind: Kind::Highlight, quads: vec![[72.0, 90.0, 150.0, 103.0]], contents: "check".into(), author: "Ana".into(), ..Default::default() };
    let id = doc.add_annot(&a);
    assert!(id > 0 && doc.modified());
    let note = Annot { page: 1, kind: Kind::Note, rect: [300.0, 300.0, 320.0, 320.0], contents: "Ñandú".into(), ..Default::default() };
    doc.add_annot(&note);
    let out = doc.save();
    // Incremental: the original bytes are still there, untouched.
    assert!(out.starts_with(&data));
    assert!(!doc.modified());
    let again = PdfDoc::open(out.clone(), None).unwrap();
    let got = again.annots(0);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, Kind::Highlight);
    assert_eq!(got[0].contents, "check");
    assert_eq!(got[0].author, "Ana");
    let q = got[0].quads[0];
    assert!((q[0] - 72.0).abs() < 0.01 && (q[3] - 103.0).abs() < 0.01, "{q:?}");
    assert_eq!(again.annots(1)[0].contents, "Ñandú");
    assert!(text_of(&out, 0).contains("Hello"));
}

#[test]
fn undo_redo_and_delete() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    let id = doc.add_annot(&Annot { page: 0, kind: Kind::Square, rect: [10.0, 10.0, 100.0, 60.0], ..Default::default() });
    doc.reply(id, "ok", "Luis");
    assert_eq!(doc.annots(0).len(), 2);
    doc.delete_annot(id);
    assert!(doc.annots(0).is_empty());
    assert_eq!(doc.undo_names(), ["pdf.undo.annot_delete", "pdf.undo.reply", "pdf.undo.annotate"]);
    doc.undo();
    assert_eq!(doc.annots(0).len(), 2);
    doc.undo();
    doc.undo();
    assert!(doc.annots(0).is_empty());
    doc.redo();
    assert_eq!(doc.annots(0).len(), 1);
    let mut moved = doc.annots(0)[0].moved(5.0, 5.0);
    moved.color = 0xFF0000;
    doc.update_annot(&moved);
    let back = &doc.annots(0)[0];
    assert_eq!(back.color, 0xFF0000);
    assert!((back.rect[0] - moved.bounds()[0]).abs() < 0.01);
}

#[test]
fn whole_file_round_trip() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    doc.add_annot(&Annot { page: 0, kind: Kind::Ink, ink: vec![vec![[10.0, 10.0], [50.0, 40.0]]], ..Default::default() });
    doc.need_whole();
    let out = doc.save();
    assert!(out.starts_with(b"%PDF-1.4"));
    let again = PdfDoc::open(out.clone(), None).unwrap();
    assert_eq!(again.pages().len(), 2);
    assert_eq!(again.annots(0)[0].kind, Kind::Ink);
    assert!(text_of(&out, 0).contains("Hello"));
}

#[test]
fn bookmarks() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    assert!(doc.bookmarks().is_empty());
    let a = doc.add_bookmark("Intro", 0, 100.0, None);
    let b = doc.add_bookmark("Second", 1, 0.0, None);
    doc.add_bookmark("Between", 0, 300.0, Some(a));
    let bm = doc.bookmarks();
    let titles: Vec<&str> = bm.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(titles, ["Intro", "Between", "Second"]);
    assert_eq!(bm[0].page, Some(0));
    assert!((bm[0].top.unwrap() - 100.0).abs() < 0.01);
    assert_eq!(bm[2].page, Some(1));
    doc.rename_bookmark(b, "Two");
    doc.delete_bookmark(a);
    let out = doc.save();
    let again = PdfDoc::open(out, None).unwrap();
    let titles: Vec<String> = again.bookmarks().into_iter().map(|b| b.title).collect();
    assert_eq!(titles, ["Between", "Two"]);
}

#[test]
fn rotated_page_geometry() {
    let g = Geom::of([0.0, 0.0, 612.0, 792.0], 90);
    assert_eq!((g.width, g.height), (792.0, 612.0));
    let v = g.to_view(100.0, 200.0);
    let u = g.to_user(v[0], v[1]);
    assert!((u[0] - 100.0).abs() < 1e-9 && (u[1] - 200.0).abs() < 1e-9);
}

/// A file from object bodies (object i + 1 is `objs[i]`, 1 the catalog).
pub(crate) fn file_of(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

pub(crate) fn form_sample() -> Vec<u8> {
    file_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 6 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R] >>",
        "<< /Fields [5 0 R 6 0 R 7 0 R 10 0 R 11 0 R 12 0 R] /DA (/Helv 0 Tf 0 g) >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [100 700 300 720] /V (Ana) >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /Rect [100 650 115 665] /V /Off /AS /Off /AP << /N << /Yes 13 0 R /Off 13 0 R >> >> >>",
        "<< /FT /Btn /Ff 49152 /T (color) /V /Off /Kids [8 0 R 9 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [100 600 115 615] /AS /Off /AP << /N << /red 13 0 R /Off 13 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [130 600 145 615] /AS /Off /AP << /N << /blue 13 0 R /Off 13 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (a) /Rect [100 550 150 570] /V (2) >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (b) /Rect [160 550 210 570] /V (3,5) >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (total) /Ff 1 /Rect [220 550 300 570] /AA << /C << /S /JavaScript /JS (AFSimple_Calculate(\"SUM\", new Array(\"a\", \"b\"));) >> /F << /S /JavaScript /JS (AFNumber_Format(2, 0, 0, 0, \"\", true);) >> >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 15 15] /Length 0 >>\nstream\n\nendstream",
    ])
}

#[test]
fn form_fields_fill_and_calculate() {
    use super::forms::FieldKind;
    let mut doc = PdfDoc::open(form_sample(), None).unwrap();
    let f = doc.fields();
    let names: Vec<&str> = f.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["name", "agree", "color", "a", "b", "total"]);
    assert_eq!(f[0].value, "Ana");
    assert_eq!(f[0].widgets[0].rect, [100.0, 72.0, 300.0, 92.0]);
    assert_eq!(f[1].kind, FieldKind::Check);
    assert_eq!(f[2].kind, FieldKind::Radio);
    assert_eq!(f[2].widgets.iter().map(|w| w.on.as_str()).collect::<Vec<_>>(), ["red", "blue"]);
    assert!(f[5].read_only && f[5].calc.is_some());
    doc.set_field("a", "4");
    doc.set_field("agree", "Yes");
    doc.set_field("color", "blue");
    doc.set_field("name", "Ana María");
    let get = |d: &PdfDoc, n: &str| d.fields().into_iter().find(|f| f.name == n).unwrap().value;
    // 4 + 3,5 with two decimals.
    assert_eq!(get(&doc, "total"), "7.50");
    assert_eq!(get(&doc, "color"), "blue");
    let kids = doc.fields()[2].widgets.clone();
    assert_eq!(doc.dict(kids[1].id).unwrap().get("AS").and_then(Obj::name), Some("blue"));
    assert_eq!(doc.dict(kids[0].id).unwrap().get("AS").and_then(Obj::name), Some("Off"));
    // Read-only fields don't change by hand.
    doc.set_field("total", "1");
    assert_eq!(get(&doc, "total"), "7.50");
    doc.undo();
    assert_eq!(get(&doc, "name"), "Ana");
    doc.redo();
    let out = doc.save();
    let again = PdfDoc::open(out, None).unwrap();
    assert_eq!(get(&again, "name"), "Ana María");
    assert_eq!(get(&again, "agree"), "Yes");
    assert_eq!(get(&again, "total"), "7.50");
}

#[test]
fn signature_field_with_look() {
    use super::forms::{FieldKind, SigLook};
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    let look = SigLook { lines: vec!["Signed by Ana".into(), "2026-09-27".into()], ink: vec![vec![[0.0, 0.0], [10.0, 5.0], [20.0, 0.0]]], color: 0x000080, ..Default::default() };
    let name = doc.add_signature_field(1, [100.0, 600.0, 300.0, 660.0], &look).unwrap();
    assert_eq!(name, "Signature1");
    let out = doc.save();
    let again = PdfDoc::open(out, None).unwrap();
    let f = again.fields();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, FieldKind::Signature);
    assert_eq!(f[0].widgets[0].page, 1);
    assert_eq!(f[0].value, "");
}

#[test]
fn stamps_keep_their_words() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    for s in ["Not Approved", "No aprobado", "Draft"] {
        doc.add_annot(&Annot { page: 0, kind: Kind::Stamp, rect: [10.0, 10.0, 170.0, 54.0], stamp: s.into(), ..Default::default() });
    }
    let again = PdfDoc::open(doc.save(), None).unwrap();
    let words: Vec<String> = again.annots(0).into_iter().map(|a| a.stamp).collect();
    assert_eq!(words, ["Not Approved", "No aprobado", "Draft"]);
}
