//! Artistic text (fill, outline, shadow, glow, reflection, bevel) of
//! document text, shape text and slide text through every format.

use std::sync::Arc;

use apex_doc::text_fx::{STYLES, style};
use apex_doc::{Attrs, DocShape, Document, Paragraph, Pos, Selection, Shape, ShapeKind, ShapePara, ShapeRun, ShapeWrap, TextFill, TextFx};
use apex_sheet::{CellRef, Workbook};

fn all() -> Vec<TextFx> {
    let acc = [0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47];
    (0..STYLES).map(|i| style(i, 0x1F1F1F, &acc)).collect()
}

/// What DrawingML runs keep: no bevel, and an opaque solid fill is the
/// run's color.
fn dml(fx: &TextFx) -> TextFx {
    let mut f = TextFx { bevel: false, ..fx.clone() };
    if matches!(f.fill, TextFill::Solid(_, 255)) {
        f.fill = TextFill::Auto;
    }
    f
}

/// What ODF keeps: the shadow (opaque), hollow letters and the bevel (as
/// an embossed relief).
fn odf_same(got: Option<&TextFx>, want: &TextFx) -> bool {
    let hollow = want.fill == TextFill::None && want.outline.is_some();
    if want.shadow.is_none() && !hollow && !want.bevel {
        return got.is_none();
    }
    let Some(got) = got else { return false };
    let shadow = match (got.shadow, want.shadow) {
        (Some(a), Some(b)) => a.color == b.color && a.dist.abs_diff(b.dist) <= 2 && a.blur.abs_diff(b.blur) <= 2 && (a.angle as i32 - b.angle as i32).rem_euclid(360).min((b.angle as i32 - a.angle as i32).rem_euclid(360)) <= 1,
        (None, None) => true,
        _ => false,
    };
    shadow && (got.fill == TextFill::None) == hollow && got.bevel == want.bevel
}

fn styled(text: &str, fx: &TextFx) -> Paragraph {
    let mut p = Paragraph::new("");
    p.push(text, Attrs { text_fx: Some(Arc::new(fx.clone())), ..Attrs::default() });
    p
}

fn text_fx(p: &Paragraph) -> Option<&TextFx> {
    p.runs.iter().find_map(|r| r.attrs.text_fx.as_deref())
}

fn box_with(fx: &TextFx) -> Shape {
    let mut s = Shape::new(ShapeKind::TextBox);
    s.text = vec![ShapePara { runs: vec![ShapeRun { color: fx.main_color(), fx: Some(Arc::new(fx.clone())), ..ShapeRun::new("Art") }], ..Default::default() }];
    s
}

fn shape_fx(s: &Shape) -> Option<&TextFx> {
    s.text.iter().flat_map(|p| &p.runs).find_map(|r| r.fx.as_deref())
}

#[test]
fn document_text_and_shapes() {
    for fx in all() {
        let mut d = Document::from_paragraphs(vec![styled("Artistic", &fx)]);
        let mut ds = DocShape::new(box_with(&fx), 200.0, 60.0);
        ds.wrap = ShapeWrap::InFront;
        d.insert_shape(Selection::caret(Pos::new(0, 0)), ds);
        let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
        assert_eq!(text_fx(&back.paragraphs[0]), Some(&fx), "docx text");
        let shapes = back.shapes();
        assert_eq!(shape_fx(&shapes[0].1.shape), Some(&fx), "docx shape");
        let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
        assert!(odf_same(text_fx(&back.paragraphs[0]), &fx), "odt text: {:?} vs {fx:?}", text_fx(&back.paragraphs[0]));
        let shapes = back.shapes();
        assert!(odf_same(shape_fx(&shapes[0].1.shape), &fx), "odt shape: {:?} vs {fx:?}", shape_fx(&shapes[0].1.shape));
    }
}

#[test]
fn a_bevel_alone_survives_odt() {
    let fx = TextFx { bevel: true, ..Default::default() };
    let d = Document::from_paragraphs(vec![styled("Raised", &fx)]);
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(text_fx(&back.paragraphs[0]).map(|f| f.bevel), Some(true));
}

#[test]
fn sheet_shapes() {
    for fx in all() {
        let mut wb = Workbook::new();
        wb.sheets[0].shapes.push(apex_sheet::shape::SheetShape::new(box_with(&fx), CellRef::new(1, 1), 200.0, 60.0));
        let back = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
        assert_eq!(shape_fx(&back.sheets[0].shapes[0].shape), Some(&dml(&fx)).filter(|f| !f.is_empty()), "xlsx");
        let back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
        assert!(odf_same(shape_fx(&back.sheets[0].shapes[0].shape), &fx), "ods: {fx:?}");
    }
}

#[test]
fn slide_text() {
    let dir = std::env::temp_dir().join(format!("apex-textfx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for fx in all() {
        let mut deck = apex_slides::build::new_deck(0);
        let s = deck.slide_mut(0);
        let t = s.objects.iter_mut().find_map(|o| o.text_or_new()).expect("a text object");
        *t = apex_slides::TextBody::plain("");
        t.paras[0] = styled("Artistic", &fx);
        for (ext, fmt) in [("pptx", apex_io::Format::Pptx), ("odp", apex_io::Format::Odp)] {
            let path = dir.join(format!("t.{ext}"));
            apex_io::save_slides(&deck, &path, fmt).unwrap();
            let data = std::fs::read(&path).unwrap();
            let back = if ext == "pptx" { apex_io::pptx::read(&data) } else { apex_io::odp::read(&data) }.unwrap();
            let got = back.slides[0].objects.iter().filter_map(|o| o.text()).flat_map(|t| &t.paras).find_map(text_fx);
            if ext == "pptx" {
                assert_eq!(got, Some(&dml(&fx)).filter(|f| !f.is_empty()), "pptx");
            } else {
                assert!(odf_same(got, &fx), "odp: {got:?} vs {fx:?}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes sample files to `APEX_SAMPLES` (to open them in other suites):
/// `cargo test -p apex-io --test text_fx -- --ignored`.
#[test]
#[ignore]
fn write_samples() {
    let Some(dir) = std::env::var_os("APEX_SAMPLES").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let mut paras = vec![Paragraph::new("Artistic text")];
    for (i, fx) in all().iter().enumerate() {
        let mut p = styled(&format!("Style {}", i + 1), fx);
        for r in &mut p.runs {
            r.attrs.size = Some(56);
        }
        paras.push(p);
    }
    let d = Document::from_paragraphs(paras);
    std::fs::write(dir.join("textfx.docx"), apex_io::docx::write(&d)).unwrap();
    std::fs::write(dir.join("textfx.odt"), apex_io::odt::write(&d)).unwrap();
    let mut deck = apex_slides::build::new_deck(0);
    let t = deck.slide_mut(0).objects.iter_mut().find_map(|o| o.text_or_new()).expect("a text object");
    *t = apex_slides::TextBody::plain("");
    t.paras[0] = styled("Artistic text", &all()[4]);
    apex_io::save_slides(&deck, &dir.join("textfx.pptx"), apex_io::Format::Pptx).unwrap();
}
