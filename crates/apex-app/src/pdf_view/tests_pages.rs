//! The PDF tab's Redact and Pages tabs driven through the view.

use apex_io::pdf::edit::annots::Kind;
use eframe::egui::Modifiers;

use super::PdfView;
use super::organize_ui::{SplitBox, SplitBy};
use super::redact_ui::Pattern;
use super::tests::{file_of, ready, sample};

/// The text of each page of the view's file as it is now.
fn page_texts(v: &mut PdfView) -> Vec<String> {
    apex_io::pdf::text_pages(&v.doc.to_bytes(), None).unwrap().iter().map(|p| p.chars.iter().map(|c| c.ch).collect()).collect()
}

#[test]
fn redacting_what_the_search_finds() {
    let (_ctx, mut v) = ready();
    v.find.query = "alpha".into();
    assert_eq!(v.mark_pattern(Pattern::Search), 2);
    assert_eq!(v.doc.redaction_marks().len(), 2);
    v.apply_redactions();
    let t = page_texts(&mut v);
    assert!(t[1].contains("two") && !t[1].contains("alpha") && !t[2].contains("alpha"), "{t:?}");
    assert!(v.doc.redaction_marks().is_empty());
    v.undo();
    assert_eq!(v.doc.redaction_marks().len(), 2);
}

#[test]
fn patterns_to_redact() {
    let c = "BT /F1 10 Tf 20 350 Td (Mail ana.gil@example.com, DNI 12345678Z) Tj 0 -20 Td (IBAN ES91 2100 0418 4502 0005 1332 tel +34 912 345 678) Tj ET";
    let data = file_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ]);
    let p = &apex_io::pdf::text_pages(&data, None).unwrap()[0];
    let text = |r: (usize, usize)| p.chars[r.0..=r.1].iter().map(|c| c.ch).filter(|c| *c != ' ').collect::<String>();
    let found = |pat: Pattern| pat.find(p).into_iter().map(text).collect::<Vec<_>>();
    assert_eq!(found(Pattern::Email), ["ana.gil@example.com"]);
    assert_eq!(found(Pattern::IdNumber), ["12345678Z"]);
    assert_eq!(found(Pattern::Iban), ["ES9121000418450200051332"]);
    assert_eq!(found(Pattern::Phone), ["+34912345678"]);
}

#[test]
fn a_dragged_area_is_a_mark() {
    let (ctx, mut v) = ready();
    v.set_tool(super::Tool::Draw(Kind::Redact));
    let p = eframe::egui::Pos2::new(500.0, 200.0);
    let press = |pressed| eframe::egui::Event::PointerButton { pos: p, button: eframe::egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    super::tests::frame(&ctx, &mut v, vec![eframe::egui::Event::PointerMoved(p), press(true)]);
    for k in 1..6 {
        super::tests::frame(&ctx, &mut v, vec![eframe::egui::Event::PointerMoved(p + eframe::egui::vec2(k as f32 * 20.0, k as f32 * 10.0))]);
    }
    let end = p + eframe::egui::vec2(100.0, 50.0);
    let release = eframe::egui::Event::PointerButton { pos: end, button: eframe::egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE };
    super::tests::frame(&ctx, &mut v, vec![release]);
    let marks = v.doc.redaction_marks();
    assert_eq!(marks.len(), 1, "{marks:?}");
}

#[test]
fn choosing_and_organizing_pages() {
    let (_ctx, mut v) = ready();
    v.click_thumb(0, Modifiers::NONE);
    v.click_thumb(2, Modifiers::COMMAND);
    assert_eq!(v.chosen_pages(), [0, 2]);
    v.click_thumb(1, Modifiers::SHIFT);
    assert_eq!(v.chosen_pages(), [1, 2]);
    // Page 3 dragged before page 1.
    v.page_sel.clear();
    v.move_to(2, 0);
    assert_eq!(page_texts(&mut v)[0], "Page three alpha");
    assert_eq!(v.chosen_pages(), [0]);
    v.rotate_chosen(1);
    assert_eq!(v.doc.geom(v.doc.pages()[0]).rotate, 90);
    v.duplicate_chosen();
    assert_eq!(v.page_count(), 4);
    v.page_sel = [0, 1, 2, 3].into();
    v.delete_chosen();
    assert_eq!(v.page_count(), 4);
    assert!(!v.notices.is_empty());
    v.insert_blank_page(true);
    assert_eq!((v.page_count(), v.cur), (5, 1));
    let n = v.insert_files(vec![("s.pdf".into(), sample())], 5).unwrap();
    assert_eq!((n, v.page_count()), (3, 8));
    for _ in 0..5 {
        v.undo();
    }
    assert_eq!(page_texts(&mut v), ["Page one", "Page two alpha", "Page three alpha"]);
    let b = SplitBox { by: SplitBy::Every, ranges: String::new(), every: 2, bad: false };
    assert_eq!(v.split_ranges(&b).unwrap().iter().map(|r| (r.1, r.2)).collect::<Vec<_>>(), [(0, 1), (2, 2)]);
    let b = SplitBox { by: SplitBy::Ranges, ranges: "9".into(), every: 1, bad: false };
    assert!(v.split_ranges(&b).is_none());
}
