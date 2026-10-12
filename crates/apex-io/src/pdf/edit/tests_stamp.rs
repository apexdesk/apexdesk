//! Bates numbers, page numbers and watermarks stamped on pages.

use super::stamp::{Place, Stamp};
use super::tests::{sample, text_of};
use super::*;

fn chars(data: &[u8], page: usize) -> Vec<crate::pdf::Char> {
    crate::pdf::pages(data, None).unwrap()[page].chars.clone()
}

#[test]
fn bates_numbers_in_the_corner() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    let s = Stamp { text: "EXP-{n}".into(), ..Stamp::default() };
    assert_eq!(doc.stamp_pages(&[], &s, 41, None), 43);
    let out = doc.to_bytes();
    assert_eq!(text_of(&out, 0), "Hello secret worldEXP-000041");
    assert_eq!(text_of(&out, 1), "Second pageEXP-000042");
    // Bottom right, as shown.
    let c = chars(&out, 0);
    let last = c.last().unwrap();
    assert!(last.x > 500.0 && last.x < 612.0 - 24.0, "{}", last.x);
    assert!(last.y > 700.0, "{}", last.y);
    doc.undo();
    assert_eq!(text_of(&doc.to_bytes(), 0), "Hello secret world");
}

#[test]
fn page_of_total_on_a_turned_page() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    doc.rotate_pages(&[1], 1);
    let s = Stamp { text: "{n} / {total}".into(), place: Place::TopLeft, digits: 0, ..Stamp::default() };
    doc.stamp_pages(&[1], &s, 2, Some(9));
    let out = doc.to_bytes();
    assert_eq!(text_of(&out, 1), "Second page2 / 9");
    // Upright and at the top left of the page as it is shown.
    let c = chars(&out, 1);
    let first = c.iter().find(|c| c.ch == '2').unwrap();
    assert!(first.upright);
    assert!(first.x < 40.0 && first.y < 50.0, "{} {}", first.x, first.y);
}

#[test]
fn diagonal_watermark() {
    let mut doc = PdfDoc::open(sample(), None).unwrap();
    let s = Stamp { text: "OBSOLETO".into(), place: Place::Diagonal, color: 0xC00000, ..Stamp::default() };
    doc.stamp_pages(&[0], &s, 1, None);
    let out = doc.save();
    assert!(text_of(&out, 0).contains("OBSOLETO"));
    assert_eq!(text_of(&out, 1), "Second page");
    assert_eq!(PdfDoc::open(out, None).unwrap().pages().len(), 2);
}
