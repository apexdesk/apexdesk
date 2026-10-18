//! Ink through xlsx, ods and odt (docx: see `docx_ink`).

use apex_doc::{DocShape, Document, Ink, InkStroke, InkTool, Paragraph, Pos, Selection, Shape, ShapeWrap};
use apex_sheet::{Anchor, CellRef, SheetShape, Workbook};

fn ink() -> Ink {
    Ink::new(vec![
        InkStroke { points: vec![(0.1, 0.1), (0.3, 0.7), (0.6, 0.4), (0.9, 0.9)], pressure: vec![0.2, 0.6, 1.0, 0.4], ..InkStroke::new(InkTool::Pen, 0x1F4E79, 2.0) },
        InkStroke { points: vec![(0.05, 0.5), (0.95, 0.5)], ..InkStroke::new(InkTool::Highlighter, 0xFFFF00, 12.0) },
        InkStroke { points: vec![(0.5, 0.2), (0.52, 0.25)], ..InkStroke::new(InkTool::Pen, 0x1F4E79, 2.0) },
    ])
}

fn close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b}");
}

/// The strokes of ink in box `b` turned and flipped as `shape` says, with
/// absolute points, sorted by look.
fn placed(shape: &Shape, b: [f32; 4]) -> Vec<InkStroke> {
    let (cx, cy) = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
    let (sn, c) = shape.rotation.to_radians().sin_cos();
    let mut v: Vec<InkStroke> = shape
        .ink
        .as_ref()
        .unwrap()
        .strokes
        .iter()
        .map(|s| {
            let points = s
                .points
                .iter()
                .map(|&(x, y)| {
                    let (x, y) = (b[0] + (if shape.flip_h { 1.0 - x } else { x }) * b[2], b[1] + (if shape.flip_v { 1.0 - y } else { y }) * b[3]);
                    (cx + (x - cx) * c - (y - cy) * sn, cy + (x - cx) * sn + (y - cy) * c)
                })
                .collect();
            InkStroke { points, ..s.clone() }
        })
        .collect();
    v.sort_by_key(|s| (s.color, s.tool as u8, (s.width * 10.0).round() as i64, (s.points[0].0 * 10.0) as i64));
    v
}

/// Same strokes in the same places (the pressure only where the format
/// keeps it).
fn same_strokes(a: &[InkStroke], b: &[InkStroke], pressure: bool, tol: f32) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        assert_eq!((x.tool, x.color, x.points.len()), (y.tool, y.color, y.points.len()));
        close(x.width, y.width, 0.01, "width");
        assert_eq!(x.has_pressure() && pressure, y.has_pressure());
        for (p, q) in x.points.iter().zip(&y.points) {
            close(p.0, q.0, tol, "x");
            close(p.1, q.1, tol, "y");
        }
    }
}

/// Every stroke of a sheet, in sheet pixels.
fn sheet_strokes(wb: &Workbook) -> Vec<InkStroke> {
    let sh = &wb.sheets[0];
    let mut v: Vec<InkStroke> = sh.shapes.iter().flat_map(|s| placed(&s.shape, s.rect(sh))).collect();
    v.sort_by_key(|s| (s.color, s.tool as u8, (s.points[0].0 * 10.0) as i64));
    v
}

fn workbook() -> Workbook {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "notes");
    let mut s = SheetShape::new(Shape::ink(ink()), CellRef::new(2, 1), 160.0, 80.0);
    s.dx = 6.0;
    s.shape.alt = "Circled total".into();
    wb.add_shape(0, s);
    let one = Ink::new(vec![InkStroke { points: vec![(0.0, 0.0), (1.0, 1.0)], ..InkStroke::new(InkTool::Pencil, 0x404040, 1.0) }]);
    let mut s = SheetShape::new(Shape::ink(one), CellRef::new(8, 3), 40.0, 40.0);
    s.shape.rotation = 30.0;
    s.anchor = Anchor::OneCell;
    wb.add_shape(0, s);
    wb
}

fn check_sheet(back: &Workbook, pressure: bool, tol: f32) {
    let orig = workbook();
    let (a, b) = (&orig.sheets[0].shapes, &back.sheets[0].shapes);
    assert_eq!(b.len(), if pressure { 2 } else { 3 }, "{b:#?}");
    assert!(b.iter().all(|s| s.shape.is_ink()));
    same_strokes(&sheet_strokes(&orig), &sheet_strokes(back), pressure, tol);
    if pressure {
        assert_eq!(b[0].from, a[0].from);
        close(b[0].dx, a[0].dx, tol, "dx");
        close(b[0].width, a[0].width, tol, "width");
        assert_eq!(b[0].shape.alt, "Circled total");
        close(b[1].shape.rotation, 30.0, 0.01, "rotation");
    }
}

#[test]
fn xlsx_ink() {
    let bytes = crate::xlsx::write(&workbook());
    let z = crate::zip::ZipReader::new(&bytes).unwrap();
    let drawing = z.read_string("xl/drawings/drawing1.xml").unwrap();
    assert!(drawing.contains(r#"Requires="xdr14""#) && drawing.contains("<xdr14:contentPart") && drawing.contains("<a:custGeom>") && drawing.contains("<xdr:grpSp>"));
    assert!(z.read_string("xl/ink/ink1.xml").unwrap().contains("InkML"));
    assert!(z.read_string("xl/drawings/_rels/drawing1.xml.rels").unwrap().contains("../ink/ink2.xml"));
    assert!(z.read_string("[Content_Types].xml").unwrap().contains("application/inkml+xml"));
    let back = crate::xlsx::read(&bytes).unwrap();
    check_sheet(&back, true, 0.6);
    // Untouched: the ink parts are written back as they were.
    let again = crate::xlsx::write(&back);
    let z2 = crate::zip::ZipReader::new(&again).unwrap();
    assert_eq!(z2.read_string("xl/ink/ink1.xml").unwrap(), z.read_string("xl/ink/ink1.xml").unwrap());
    // Programs without ink read the fallback: freeform shapes, which come
    // back as ink too.
    let plain = drawing.replace("xdr14:contentPart", "xdr14:gone").replace("contentPart", "gone");
    let mut w = crate::zip::ZipWriter::new();
    for n in z.names() {
        let data = if n == "xl/drawings/drawing1.xml" { plain.clone().into_bytes() } else { z.read(n).unwrap() };
        w.add(n, &data, true);
    }
    let fb = crate::xlsx::read(&w.finish()).unwrap();
    assert!(fb.sheets[0].shapes.iter().all(|s| s.shape.is_ink()));
}

#[test]
fn ods_ink() {
    let bytes = crate::ods::write(&workbook());
    let z = crate::zip::ZipReader::new(&bytes).unwrap();
    let content = z.read_string("content.xml").unwrap();
    assert!(content.contains("<draw:path") && content.contains("svg:stroke-opacity"));
    let back = crate::ods::read(&bytes).unwrap();
    // One line for each look: the ink with two looks comes back as two.
    // Offsets in a cell scale with LibreOffice's row heights, sizes don't,
    // so the lines of one ink may move a little apart.
    check_sheet(&back, false, 4.0);
    let inks: Vec<&Ink> = back.sheets[0].shapes.iter().filter_map(|s| s.shape.ink.as_deref()).collect();
    assert_eq!(inks.iter().map(|i| i.strokes.len()).sum::<usize>(), 4);
    assert!(inks.iter().any(|i| i.strokes[0].tool == InkTool::Highlighter));
}

fn document() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Ink over this text"), Paragraph::new("More text")]);
    let mut s = DocShape::new(Shape::ink(ink()), 150.0, 60.0);
    (s.x, s.y, s.z) = (20.0, 6.0, 1);
    d.insert_shape(Selection::caret(Pos::new(0, 0)), s);
    let mut s = DocShape::new(Shape::ink(Ink::new(vec![InkStroke { points: vec![(0.1, 0.9), (0.9, 0.1)], ..InkStroke::new(InkTool::Pen, 0xC00000, 3.0) }])), 30.0, 30.0);
    s.wrap = ShapeWrap::Behind;
    d.insert_shape(Selection::caret(Pos::new(1, 4)), s);
    d
}

#[test]
fn odt_ink() {
    let d = document();
    let bytes = crate::odt::write(&d);
    let z = crate::zip::ZipReader::new(&bytes).unwrap();
    let content = z.read_string("content.xml").unwrap();
    assert!(content.contains("<draw:g ") && content.contains("<draw:path"));
    let back = crate::odt::read(&bytes).unwrap();
    let (a, b) = (d.shapes(), back.shapes());
    assert_eq!(b.len(), 2, "{b:#?}");
    for ((pa, sa), (pb, sb)) in a.iter().zip(&b) {
        assert_eq!(pa.para, pb.para);
        assert_eq!((sa.wrap, sa.h_rel, sa.v_rel), (sb.wrap, sb.h_rel, sb.v_rel));
        assert_eq!(sb.group, None);
        // The box shrinks to the strokes; they stay where they were.
        same_strokes(&placed(&sa.shape, [sa.x, sa.y, sa.width, sa.height]), &placed(&sb.shape, [sb.x, sb.y, sb.width, sb.height]), false, 0.05);
    }
}

#[test]
fn libreoffice_freeform_lines() {
    // A freeform line drawn in LibreOffice Writer: curves, no fill.
    let content = r##"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3"><office:automatic-styles><style:style style:name="gr1" style:family="graphic"><style:graphic-properties svg:stroke-color="#ff0000" svg:stroke-width="0.1cm" draw:fill="none" style:wrap="run-through" style:run-through="foreground"/></style:style></office:automatic-styles><office:body><office:text><text:p>Hand<draw:path text:anchor-type="paragraph" draw:z-index="0" draw:style-name="gr1" svg:width="3cm" svg:height="2cm" svg:x="1cm" svg:y="0.5cm" svg:viewBox="0 0 3000 2000" svg:d="M0 2000C500 0 1000 0 1500 1000S2500 2000 3000 0"><text:p/></draw:path><draw:path text:anchor-type="paragraph" draw:z-index="1" draw:style-name="gr1" svg:width="1cm" svg:height="1cm" svg:x="0cm" svg:y="0cm" svg:viewBox="0 0 1000 1000" svg:d="M0 0L1000 0 1000 1000Z"><text:p/></draw:path></text:p></office:text></office:body></office:document-content>"##;
    let mut w = crate::zip::ZipWriter::new();
    w.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    w.add("content.xml", content.as_bytes(), true);
    let d = crate::odt::read(&w.finish()).unwrap();
    let shapes = d.shapes();
    assert_eq!(shapes.len(), 1, "closed paths are shapes we don't read");
    let s = &shapes[0].1;
    let ink = s.shape.ink.as_ref().unwrap();
    assert_eq!((ink.strokes.len(), ink.strokes[0].color, ink.strokes[0].tool), (1, 0xFF0000, InkTool::Pen));
    close(ink.strokes[0].width, 0.1 * 72.0 / 2.54, 0.01, "width");
    // The box fits the strokes with room for their width.
    let cm = 72.0 / 2.54;
    let pad = 0.05 * cm + 1.0;
    close(s.width, 3.0 * cm + 2.0 * pad, 0.01, "box");
    let &(px, py) = ink.strokes[0].points.last().unwrap();
    close(s.x + px * s.width, 4.0 * cm, 0.01, "x");
    close(s.y + py * s.height, 0.5 * cm, 0.01, "y");
}
