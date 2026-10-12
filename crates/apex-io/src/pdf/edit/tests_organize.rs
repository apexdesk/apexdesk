//! Organizing pages, extracting, splitting and merging.

use super::organize::parse_ranges;
use super::tests::{sample, text_of};
use super::*;

fn texts(data: &[u8]) -> Vec<String> {
    let n = crate::pdf::pages(data, None).unwrap().len();
    (0..n).map(|i| text_of(data, i)).collect()
}

#[test]
fn move_rotate_delete_and_undo() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    doc.move_pages(&[1], 0);
    assert_eq!(texts(&doc.to_bytes()), ["Second page", "Hello secret world"]);
    doc.move_pages(&[0], 2);
    assert_eq!(texts(&doc.to_bytes())[0], "Hello secret world");
    doc.undo();
    doc.rotate_pages(&[0], 1);
    assert_eq!(doc.geom(doc.pages()[0]).rotate, 90);
    doc.rotate_pages(&[0], -1);
    assert_eq!(doc.geom(doc.pages()[0]).rotate, 0);
    assert!(!doc.delete_pages(&[0, 1]));
    assert!(doc.delete_pages(&[0]));
    let out = doc.to_bytes();
    assert_eq!(texts(&out), ["Hello secret world"]);
    // Saved incrementally, reopened by another reader of ours.
    assert_eq!(PdfDoc::open(out, None).unwrap().pages().len(), 1);
    doc.undo();
    assert_eq!(doc.pages().len(), 2);
}

#[test]
fn duplicate_blank_and_insert_from_another_file() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    doc.duplicate_pages(&[0]);
    assert_eq!(texts(&doc.to_bytes()), ["Hello secret world", "Hello secret world", "Second page"]);
    doc.insert_blank(1);
    assert_eq!(texts(&doc.to_bytes())[1], "");
    let n = doc.insert_from(sample(), None, &[1], 0).unwrap();
    assert_eq!(n, 1);
    let t = texts(&doc.to_bytes_whole());
    assert_eq!(t.len(), 5);
    assert_eq!(t[0], "Second page");
    assert_eq!(doc.undo_names()[0], "pdf.undo.insert_pages");
    doc.undo();
    assert_eq!(doc.pages().len(), 4);
}

#[test]
fn extract_split_and_ranges() {
    let doc = PdfDoc::open(sample(), None).unwrap();
    let one = doc.extract(&[1]);
    assert_eq!(texts(&one), ["Second page"]);
    assert!(memchr::memmem::find(&one, b"secret").is_none());
    let parts = doc.split(&[(0, 0), (1, 1)]);
    assert_eq!(parts.len(), 2);
    assert_eq!(texts(&parts[0]), ["Hello secret world"]);
    assert_eq!(parse_ranges("1-2, 2", 2), Some(vec![(0, 1), (1, 1)]));
    assert_eq!(parse_ranges("2-", 3), Some(vec![(1, 2)]));
    assert_eq!(parse_ranges("0", 3), None);
    assert_eq!(parse_ranges("x", 3), None);
    let mut doc = doc;
    doc.add_bookmark("Two", 1, 0.0, None);
    assert_eq!(doc.bookmark_ranges(), vec![(String::new(), 0, 0), ("Two".to_string(), 1, 1)]);
}

#[test]
fn deleted_pages_leave_no_way_back() {
    let data = super::tests::file_of(&[
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R /OpenAction [4 0 R /Fit] /Dests << /gone [4 0 R /Fit] /here [3 0 R /Fit] >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 7 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        "<< /Type /StructTreeRoot /K << /S /P /Pg 4 0 R /ActualText (deleted words) >> >>",
        "<< /Length 21 >>\nstream\nBT (secret page) Tj ET\nendstream",
    ]);
    let mut doc = PdfDoc::open(data, None).unwrap();
    assert!(doc.delete_pages(&[1]));
    let out = doc.to_bytes();
    for gone in [&b"deleted words"[..], b"secret page", b"/gone [4"] {
        assert!(memchr::memmem::find(&out, gone).is_none(), "{}", String::from_utf8_lossy(gone));
    }
    assert!(memchr::memmem::find(&out, b"/here").is_some());
}
