//! The PDF tab's Tools tab: comparing, exporting pictures, properties and
//! PDF/A, driven through the view.

use apex_io::pdf::edit::props::{Info, View};
use eframe::egui::{Event, Key, Modifiers};

use super::compare_ui::ChangeKind;
use super::tests::{file_of, frame, ready};
use super::tools_ui::ExportBox;
use super::{PdfView, Request};

/// A file like the view's sample with these page texts.
fn version(texts: &[&str]) -> Vec<u8> {
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 >>".to_string()];
    for (i, t) in texts.iter().enumerate() {
        let content = 4 + i * 2;
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font << /F1 9 0 R >> >> /Contents {content} 0 R >>"));
        let c = format!("BT /F1 20 Tf 20 350 Td ({t}) Tj ET");
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    file_of(&objs)
}

#[test]
fn comparing_with_an_earlier_version() {
    let (ctx, mut v) = ready();
    let old = version(&["Page one", "Page two beta", "Page three alpha"]);
    assert_eq!(v.start_compare("old.pdf".into(), &old, None).unwrap(), 1);
    let c = v.compare.as_ref().unwrap();
    let ch = &c.changes[0];
    assert_eq!((ch.kind, ch.old.as_str(), ch.new.as_str(), ch.page), (ChangeKind::Changed, "beta", "alpha", 1));
    assert_eq!(v.cur, 1);
    // The pane and the marks draw.
    frame(&ctx, &mut v, vec![]);
    // The same text: nothing to show.
    let same = super::tests::sample();
    assert_eq!(v.start_compare("same.pdf".into(), &same, None).unwrap(), 0);
    assert!(v.compare.is_none() && !v.notices.is_empty());
}

#[test]
fn properties_and_initial_view_are_one_step() {
    let (ctx, mut v) = ready();
    v.open_props();
    // The dialog has the keys: Ctrl+End does not move the view.
    frame(&ctx, &mut v, vec![Event::Key { key: Key::End, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL }]);
    assert_eq!(v.cur, 0);
    let info = Info { title: "Report".into(), keywords: "x".into(), ..Default::default() };
    let view = View { layout: "SinglePage".into(), mode: "UseOutlines".into(), ..Default::default() };
    v.apply_props(&info, &view);
    assert_eq!((v.doc.info().title.as_str(), v.doc.view().layout.as_str()), ("Report", "SinglePage"));
    assert_eq!(v.doc.undo_names(), ["pdf.undo.properties"]);
    // Opened again, the file shows as it asks.
    let w = PdfView::open(v.doc.to_bytes(), None).unwrap();
    assert_eq!((w.layout, w.pane), (super::Layout::Single, super::Pane::Bookmarks));
    v.undo();
    assert_eq!(v.doc.info().title, "");
    assert_eq!(v.doc.view(), View::default());
}

#[test]
fn saving_as_pdfa() {
    let (_ctx, mut v) = ready();
    v.make_pdfa();
    assert_eq!(v.doc.pdfa_claim(), Some(("2".into(), "B".into())));
    assert!(matches!(v.requests.last(), Some(Request::SaveAs)));
    // Helvetica is not embedded: the user is told.
    assert!(!v.notices.is_empty());
    v.undo();
    assert!(v.doc.pdfa_claim().is_none());
}

#[test]
fn exporting_pages_as_pictures() {
    let (_ctx, mut v) = ready();
    let b = |r: &str| ExportBox { ranges: r.into(), dpi: 72, bad: false };
    assert_eq!(v.export_pages(&b("")).unwrap(), [0, 1, 2]);
    assert_eq!(v.export_pages(&b("2-3")).unwrap(), [1, 2]);
    assert!(v.export_pages(&b("7")).is_none());
    let png = super::tools_ui::encode_png(&[255; 16], 2, 2, 150).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    if !super::engine::available() {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-pdf-export");
    std::fs::create_dir_all(&dir).unwrap();
    let n = v.export_images(&[0, 2], 72, |k| dir.join(format!("p{k}.png"))).unwrap();
    assert_eq!(n, 2);
    let data = std::fs::read(dir.join("p2.png")).unwrap();
    let img = crate::images::decode(&data).unwrap();
    assert_eq!((img.width, img.height), (300, 400));
    let _ = std::fs::remove_dir_all(&dir);
}
