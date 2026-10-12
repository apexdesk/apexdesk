//! Document properties, the initial view and PDF/A.

use super::props::{Info, View, xmp};
use super::tests::sample;
use super::*;

#[test]
fn properties_are_kept_and_undone() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    assert_eq!(doc.info().title, "Informe");
    let info = Info { title: "Informe año".into(), author: "Ana".into(), keywords: "a, b".into(), ..Default::default() };
    doc.set_info(&info);
    let back = PdfDoc::open(doc.to_bytes(), None).unwrap();
    let got = back.info();
    assert_eq!((got.title.as_str(), got.author.as_str(), got.keywords.as_str()), ("Informe año", "Ana", "a, b"));
    assert!(got.modified.starts_with("D:"));
    // No XMP was there, so none is added.
    assert!(back.pdfa_claim().is_none());
    assert_eq!(doc.undo_names().first(), Some(&"pdf.undo.properties"));
    doc.undo();
    assert_eq!(doc.info().title, "Informe");
}

#[test]
fn how_the_file_opens() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    assert_eq!(doc.view(), View::default());
    let v = View { layout: "TwoColumnLeft".into(), mode: "UseOutlines".into(), fit_window: true, center_window: false, show_title: true };
    doc.set_view(&v);
    assert_eq!(PdfDoc::open(doc.to_bytes(), None).unwrap().view(), v);
    doc.set_view(&View::default());
    let cat = doc.catalog().unwrap().1;
    assert!(cat.get("PageLayout").is_none() && cat.get("ViewerPreferences").is_none());
    doc.undo();
    assert_eq!(doc.view(), v);
}

#[test]
fn making_a_file_pdfa() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    doc.begin("t");
    doc.set_catalog("OpenAction", Some(Obj::Dict(Dict::of(vec![("S", Obj::name_of("JavaScript"))]))));
    doc.end();
    let rep = doc.make_pdfa(b"fake profile").unwrap();
    assert!(rep.unembedded_fonts.contains(&"Helvetica".to_string()));
    let out = doc.to_bytes();
    let back = PdfDoc::open(out, None).unwrap();
    assert_eq!(back.pdfa_claim(), Some(("2".into(), "B".into())));
    let cat = back.catalog().unwrap().1;
    assert!(cat.get("OpenAction").is_none());
    let intents = back.key(&cat, "OutputIntents");
    let intent = back.resolve(&intents.array().unwrap()[0]);
    assert_eq!(intent.dict().unwrap().get("S").and_then(Obj::name), Some("GTS_PDFA1"));
    assert_eq!(back.info().producer, "ApexDesk");
    // Later property changes keep the claim and the XMP in step.
    let mut back = back;
    back.set_info(&Info { title: "T".into(), ..back.info() });
    let again = PdfDoc::open(back.to_bytes(), None).unwrap();
    assert_eq!(again.pdfa_claim(), Some(("2".into(), "B".into())));
    let x = xmp(&Info { title: "A & B".into(), created: "D:20260927101500Z".into(), ..Default::default() }, None);
    assert!(x.contains("A &amp; B") && x.contains("2026-09-27T10:15:00Z") && !x.contains("pdfaid:part"));
}
