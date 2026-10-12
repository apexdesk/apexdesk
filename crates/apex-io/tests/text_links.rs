//! Linked text boxes in docx and odt.

use apex_doc::{DocShape, Document, Paragraph, Pos, Selection, Shape, ShapeKind, ShapeWrap};

fn doc() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("uno"), Paragraph::new("dos"), Paragraph::new("tres")]);
    for i in 0..3 {
        let mut s = Shape::new(ShapeKind::TextBox);
        if i == 0 {
            s.set_plain_text("Un texto que sigue en otros cuadros");
        }
        let mut ds = DocShape::new(s, 80.0, 30.0);
        ds.wrap = ShapeWrap::InFront;
        d.insert_shape(Selection { anchor: Pos::new(i, 0), cursor: Pos::new(i, 0) }, ds);
    }
    assert!(d.link_text_boxes(Pos::new(0, 0), Pos::new(1, 0)));
    assert!(d.link_text_boxes(Pos::new(1, 0), Pos::new(2, 0)));
    d
}

fn chain(d: &Document) -> Vec<(Option<u16>, String)> {
    d.shapes().iter().map(|(_, s)| (s.shape.link.map(|l| l.seq), s.shape.plain_text())).collect()
}

#[test]
fn linked_text_boxes_round_trip() {
    let d = doc();
    let want = vec![(Some(0), "Un texto que sigue en otros cuadros".to_string()), (Some(1), String::new()), (Some(2), String::new())];
    assert_eq!(chain(&d), want);
    assert_eq!(chain(&apex_io::docx::read(&apex_io::docx::write(&d)).unwrap()), want);
    assert_eq!(chain(&apex_io::odt::read(&apex_io::odt::write(&d)).unwrap()), want);
}
