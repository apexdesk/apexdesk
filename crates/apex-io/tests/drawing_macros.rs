//! The macro a click on a shape or picture runs, through .xlsx and .ods.

use std::sync::Arc;

use apex_doc::{Shape, ShapeKind};
use apex_sheet::image::{ImageFormat, SheetImage};
use apex_sheet::shape::SheetShape;
use apex_sheet::{CellRef, Workbook};

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn shapes_and_pictures_keep_their_macro() {
    let mut wb = Workbook::new();
    let sh = &mut wb.sheets[0];
    for (kind, mac) in [(ShapeKind::Rect, "Module1.Go"), (ShapeKind::Ellipse, ""), (ShapeKind::TextBox, "Hello")] {
        let mut s = SheetShape::new(Shape::new(kind), CellRef::new(1, 1), 120.0, 60.0);
        s.macro_name = mac.into();
        sh.shapes.push(s);
    }
    let mut img = SheetImage::new(Arc::from(PNG), ImageFormat::Png, CellRef::new(5, 1), 40.0, 40.0);
    img.macro_name = "Module2.Picture_Click".into();
    sh.images.push(img);
    for (name, back) in [("xlsx", apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap()), ("ods", apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap())] {
        let sh = &back.sheets[0];
        let macs: Vec<&str> = sh.shapes.iter().map(|s| s.macro_name.as_str()).collect();
        // .ods names a module: a bare name lands in Module1.
        let hello = if name == "ods" { "Module1.Hello" } else { "Hello" };
        assert_eq!(macs, ["Module1.Go", "", hello], "{name}");
        assert_eq!(sh.images.len(), 1, "{name}");
        assert_eq!(sh.images[0].macro_name, "Module2.Picture_Click", "{name}");
    }
}
