//! Grouped shapes in docx and odt.

use std::sync::Arc;

use apex_doc::{DocShape, Document, Image, Paragraph, Pos, Selection, Shape, ShapeKind, ShapeWrap};

fn doc() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("uno"), Paragraph::new("dos")]);
    let png: Arc<[u8]> = Arc::from(std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/linux/apexdesk-256.png")).unwrap());
    let shapes = [
        (Shape::new(ShapeKind::Rect), 10.0, 20.0, 100.0, 40.0),
        (Shape::new(ShapeKind::Ellipse), 150.0, 30.0, 60.0, 60.0),
        (Shape::picture(Image::new(png, 50.0, 50.0)), 40.0, 90.0, 50.0, 50.0),
    ];
    for (i, (s, x, y, w, h)) in shapes.into_iter().enumerate() {
        let mut ds = DocShape::new(s, w, h);
        (ds.wrap, ds.x, ds.y) = (ShapeWrap::InFront, x, y);
        d.insert_shape(Selection::caret(Pos::new(i % 2, 0)), ds);
    }
    let members: Vec<_> = d.shapes().into_iter().map(|(p, s)| (p, (*s).clone())).collect();
    assert_eq!(d.group_shapes(0, members).len(), 3);
    // Another shape, not grouped.
    let mut ds = DocShape::new(Shape::new(ShapeKind::Rect), 20.0, 20.0);
    ds.wrap = ShapeWrap::InFront;
    d.insert_shape(Selection::caret(Pos::new(1, 0)), ds);
    d
}

fn summary(d: &Document) -> Vec<(Option<u32>, bool, i32, i32, i32, i32)> {
    let mut v: Vec<_> = d.shapes().iter().map(|(_, s)| (s.group, s.shape.picture.is_some(), s.x.round() as i32, s.y.round() as i32, s.width.round() as i32, s.height.round() as i32)).collect();
    v.sort_by_key(|t| (t.0.is_none(), t.2, t.3));
    v
}

#[test]
fn shape_groups_round_trip() {
    let d = doc();
    let want = summary(&d);
    assert_eq!(want.iter().filter(|t| t.0 == Some(1)).count(), 3);
    let docx = apex_io::docx::write(&d);
    let text = apex_io::zip::ZipReader::new(&docx).unwrap().read_string("word/document.xml").unwrap();
    assert_eq!(text.matches("<wpg:wgp>").count(), 1);
    assert_eq!(summary(&apex_io::docx::read(&docx).unwrap()), want);
    assert_eq!(summary(&apex_io::odt::read(&apex_io::odt::write(&d)).unwrap()), want);
}
