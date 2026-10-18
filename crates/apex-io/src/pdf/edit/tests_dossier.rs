use super::PdfDoc;
use super::annots::Kind;
use super::dossier::{Dossier, Part, build};
use super::tests::{sample, text_of};

fn parts() -> Vec<Part> {
    vec![
        Part { title: "Demanda".into(), data: sample() },
        Part { title: "Roto".into(), data: b"not a pdf".to_vec() },
        Part { title: "Contestación".into(), data: sample() },
    ]
}

#[test]
fn dossier_with_cover_contents_and_separators() {
    let o = Dossier { title: "Expediente 12/2026".into(), subtitle: "Juzgado n.º 3".into(), ..Dossier::default() };
    let (out, failed) = build(&parts(), &o);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, 1);
    let doc = PdfDoc::open(out.clone(), None).unwrap();
    // Cover, contents, then separator + 2 pages for each part.
    assert_eq!(doc.pages().len(), 8);
    assert!(text_of(&out, 0).contains("Expediente 12/2026"));
    let index = text_of(&out, 1);
    assert!(index.contains("Demanda") && index.contains("Contestación"), "{index}");
    // The parts start on pages 3 and 6.
    assert!(index.contains('3') && index.contains('6'));
    let links: Vec<_> = doc.annots(1).into_iter().filter(|a| a.kind == Kind::Link).collect();
    assert_eq!(links.len(), 2);
    assert!(text_of(&out, 2).contains("Demanda"));
    assert!(text_of(&out, 3).contains("Hello"));
    assert!(text_of(&out, 7).contains("8 / 8"));
    let marks: Vec<(String, usize)> = doc.bookmarks().into_iter().filter_map(|b| Some((b.title, b.page?))).collect();
    assert_eq!(marks.iter().map(|m| m.1).collect::<Vec<_>>(), [1, 2, 5]);
    assert_eq!(marks[1].0, "Demanda");
    assert_eq!(doc.info().title, "Expediente 12/2026");
}

#[test]
fn dossier_without_extras_is_a_plain_merge_with_bookmarks() {
    let o = Dossier { cover: false, index: false, separators: false, numbers: false, ..Dossier::default() };
    let (out, _) = build(&parts(), &o);
    let doc = PdfDoc::open(out.clone(), None).unwrap();
    assert_eq!(doc.pages().len(), 4);
    assert!(text_of(&out, 0).contains("Hello"));
    assert_eq!(doc.bookmarks().iter().filter_map(|b| b.page).collect::<Vec<_>>(), [0, 2]);
}
