//! Gradient fills of shapes, text boxes, slides and pages through every
//! format that has them.

use apex_doc::{DocShape, Document, GradKind, Gradient, Paragraph, Pos, Selection, Shape, ShapeKind, ShapeWrap};
use apex_sheet::{CellRef, Workbook};
use apex_slides::{Background, Fill};

fn gradients() -> Vec<Gradient> {
    let mut lin = Gradient::two(0x1F4E79, 0xDEEAF6, 45.0);
    lin.add_stop(0.25);
    lin.stops[1].color = 0xC00000;
    lin.stops[2].alpha = 0.5;
    let radial = Gradient { kind: GradKind::Radial, angle: 0.0, stops: Gradient::two(0xFFFFFF, 0x2E75B6, 0.0).stops };
    let rect = Gradient { kind: GradKind::Rect, ..radial.clone() };
    vec![lin, radial, rect]
}

fn close(a: &Gradient, b: &Gradient) -> bool {
    a.kind == b.kind
        && (a.kind != GradKind::Linear || (a.angle - b.angle).abs() < 0.1)
        && a.stops.len() == b.stops.len()
        && a.stops.iter().zip(&b.stops).all(|(x, y)| (x.pos - y.pos).abs() < 1e-3 && x.color == y.color && (x.alpha - y.alpha).abs() < 0.01)
}

fn shaped(g: &Gradient, kind: ShapeKind) -> Shape {
    let mut s = Shape::new(kind);
    s.fill = Some(g.main_color());
    s.gradient = Some(g.clone());
    s
}

#[test]
fn document_shapes_and_pages() {
    for g in gradients() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("uno")]);
        for kind in [ShapeKind::Rect, ShapeKind::TextBox] {
            let mut ds = DocShape::new(shaped(&g, kind), 100.0, 50.0);
            ds.wrap = ShapeWrap::InFront;
            d.insert_shape(Selection::caret(Pos::new(0, 0)), ds);
        }
        d.deco.color = Some(g.main_color());
        d.deco.gradient = Some(g.clone());
        for (name, back) in [("docx", apex_io::docx::read(&apex_io::docx::write(&d)).unwrap()), ("odt", apex_io::odt::read(&apex_io::odt::write(&d)).unwrap())] {
            let shapes = back.shapes();
            assert_eq!(shapes.len(), 2, "{name}");
            for (_, s) in shapes {
                let got = s.shape.gradient.as_ref().unwrap_or_else(|| panic!("{name}: no gradient"));
                assert!(close(got, &g), "{name}: {got:?} vs {g:?}");
            }
            let mut page = back.deco.gradient.clone().unwrap_or_else(|| panic!("{name}: no page gradient"));
            // Word's page fills (VML) have an opacity at each end only.
            if name == "docx" {
                let n = page.stops.len();
                for (i, s) in page.stops.iter_mut().enumerate().take(n - 1).skip(1) {
                    s.alpha = g.stops[i].alpha;
                }
            }
            assert!(close(&page, &g), "{name} page: {page:?} vs {g:?}");
            assert!(back.deco.color.is_some());
        }
    }
}

#[test]
fn sheet_shapes() {
    for g in gradients() {
        let mut wb = Workbook::new();
        wb.sheets[0].shapes.push(apex_sheet::shape::SheetShape::new(shaped(&g, ShapeKind::Ellipse), CellRef::new(1, 1), 120.0, 60.0));
        for (name, back) in [("xlsx", apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap()), ("ods", apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap())] {
            let got = back.sheets[0].shapes[0].shape.gradient.as_ref().unwrap_or_else(|| panic!("{name}: no gradient"));
            assert!(close(got, &g), "{name}: {got:?} vs {g:?}");
        }
    }
}

#[test]
fn slide_backgrounds() {
    let dir = std::env::temp_dir().join(format!("apex-grad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for g in gradients() {
        let mut deck = apex_slides::build::new_deck(0);
        deck.slide_mut(0).background = Some(Background::Fill(Fill::Gradient(apex_slides::fill::Gradient::from_doc(&g))));
        for (ext, fmt) in [("pptx", apex_io::Format::Pptx), ("odp", apex_io::Format::Odp)] {
            let path = dir.join(format!("g.{ext}"));
            apex_io::save_slides(&deck, &path, fmt).unwrap();
            let data = std::fs::read(&path).unwrap();
            let back = if ext == "pptx" { apex_io::pptx::read(&data) } else { apex_io::odp::read(&data) }.unwrap();
            let Some(Background::Fill(Fill::Gradient(got))) = &back.slides[0].background else { panic!("{ext}: {:?}", back.slides[0].background) };
            let theme = back.masters[0].theme.theme.clone();
            let got = got.resolve(&apex_slides::ColorCx { theme: &theme, map: Default::default(), ph: None });
            assert!(close(&got, &g), "{ext}: {got:?} vs {g:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes sample files to `APEX_SAMPLES` (to open them in other suites):
/// `cargo test -p apex-io --test gradients -- --ignored`.
#[test]
#[ignore]
fn write_samples() {
    let Some(dir) = std::env::var_os("APEX_SAMPLES").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let gs = gradients();
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Gradients"), Paragraph::new("")]);
    for (i, g) in gs.iter().enumerate() {
        let mut ds = DocShape::new(shaped(g, if i == 1 { ShapeKind::Ellipse } else { ShapeKind::Rect }), 140.0, 90.0);
        (ds.wrap, ds.x, ds.y) = (ShapeWrap::InFront, 20.0 + 160.0 * i as f32, 80.0);
        d.insert_shape(Selection::caret(Pos::new(0, 0)), ds);
    }
    let page = apex_doc::gradient::variation(0xDEEAF6, false, 1);
    d.deco.color = Some(page.main_color());
    d.deco.gradient = Some(page);
    std::fs::write(dir.join("grad.docx"), apex_io::docx::write(&d)).unwrap();
    std::fs::write(dir.join("grad.odt"), apex_io::odt::write(&d)).unwrap();
    let mut wb = Workbook::new();
    for (i, g) in gs.iter().enumerate() {
        wb.sheets[0].shapes.push(apex_sheet::shape::SheetShape::new(shaped(g, ShapeKind::Rect), CellRef::new(1, 1 + 3 * i as u32), 140.0, 90.0));
    }
    std::fs::write(dir.join("grad.xlsx"), apex_io::xlsx::write(&wb)).unwrap();
    std::fs::write(dir.join("grad.ods"), apex_io::ods::write(&wb)).unwrap();
    let mut deck = apex_slides::build::new_deck(0);
    deck.slide_mut(0).background = Some(Background::Fill(Fill::Gradient(apex_slides::fill::Gradient::from_doc(&gs[0]))));
    apex_io::save_slides(&deck, &dir.join("grad.pptx"), apex_io::Format::Pptx).unwrap();
    apex_io::save_slides(&deck, &dir.join("grad.odp"), apex_io::Format::Odp).unwrap();
}
