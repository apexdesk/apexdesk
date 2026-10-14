//! Shapes and text boxes through every format that keeps them.

use std::sync::Arc;

use apex_doc::{Align, Attrs, Dash, DocShape, Document, HRel, OBJECT, Paragraph, Shape, ShapeKind, ShapeWrap, VAnchor, VRel};
use apex_sheet::{Anchor, CellRef, SheetShape, Workbook};

fn looks() -> Vec<Shape> {
    let mut star = Shape::new(ShapeKind::Star5);
    star.rotation = 30.0;
    star.fill = Some(0xFFC000);
    star.dash = Dash::Dash;
    star.line_width = 2.0;
    star.set_plain_text("Star\nsecond line");
    star.format_text(&|r| {
        r.bold = true;
        r.size = Some(14.0);
    });
    let mut arrow = Shape::new(ShapeKind::Line);
    arrow.arrow_end = true;
    arrow.line = Some(0xC00000);
    let mut text = Shape::new(ShapeKind::TextBox);
    text.set_plain_text("Text box");
    text.format_text(&|r| r.color = Some(0x0070C0));
    text.valign = VAnchor::Bottom;
    let mut oval = Shape::new(ShapeKind::Ellipse);
    oval.fill = None;
    oval.flip_h = true;
    oval.alt = "An oval".into();
    let mut right = Shape::new(ShapeKind::RightArrow);
    right.set_plain_text("Go");
    right.format_text(&|r| r.italic = true);
    right.text[0].align = Align::Left;
    vec![star, arrow, text, oval, right]
}

fn same_look(a: &Shape, b: &Shape) {
    let mut b = b.clone();
    // Names are made up when there are none.
    if a.name.is_empty() {
        b.name.clear();
    }
    assert_eq!(a, &b);
}

fn close(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 0.6, "{what}: {a} vs {b}");
}

fn doc_sample() -> Document {
    let wraps = [ShapeWrap::InFront, ShapeWrap::Inline, ShapeWrap::Behind, ShapeWrap::Square, ShapeWrap::TopBottom];
    let mut paras = Vec::new();
    for (i, s) in looks().into_iter().enumerate() {
        let mut d = DocShape::new(s, 100.0 + i as f32 * 10.0, if i == 1 { 0.0 } else { 50.0 });
        d.wrap = wraps[i];
        if d.wrap.floats() {
            d.x = 36.0 + i as f32;
            d.y = 12.0;
            d.z = i as i32;
        }
        if i == 2 {
            (d.h_rel, d.v_rel) = (HRel::Page, VRel::Page);
        }
        if i == 3 {
            d.h_align = Some(Align::Center);
        }
        let mut p = Paragraph::new("Before ");
        p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(d)), ..Default::default() });
        p.push(" after", Attrs::default());
        paras.push(p);
    }
    Document::from_paragraphs(paras)
}

fn check_doc(back: &Document, format: &str) {
    let want = doc_sample().shapes();
    let got = back.shapes();
    assert_eq!(want.len(), got.len(), "{format}");
    for ((wp, w), (gp, g)) in want.iter().zip(&got) {
        assert_eq!(wp.para, gp.para, "{format}");
        same_look(&w.shape, &g.shape);
        close(w.width, g.width, format);
        close(w.height, g.height, format);
        assert_eq!((w.wrap, w.h_rel, w.v_rel, w.h_align), (g.wrap, g.h_rel, g.v_rel, g.h_align), "{format}");
        if w.wrap.floats() {
            close(w.x, g.x, format);
            close(w.y, g.y, format);
            assert_eq!(w.z, g.z, "{format}");
        }
    }
    assert_eq!(back.paragraphs[0].text, format!("Before {OBJECT} after"));
}

#[test]
fn docx_shapes() {
    let back = crate::docx::read(&crate::docx::write(&doc_sample())).expect("docx");
    check_doc(&back, "docx");
}

#[test]
fn odt_shapes() {
    let back = crate::odt::read(&crate::odt::write(&doc_sample())).expect("odt");
    check_doc(&back, "odt");
}

fn book() -> Workbook {
    let mut wb = Workbook::new();
    let anchors = [Anchor::TwoCell, Anchor::OneCell, Anchor::Absolute, Anchor::TwoCell, Anchor::OneCell];
    for (i, s) in looks().into_iter().enumerate() {
        let mut sh = SheetShape::new(s, CellRef::new(2 + i as u32 * 6, 1 + i as u32), 120.0, if i == 1 { 0.0 } else { 60.0 });
        (sh.dx, sh.dy) = (10.0, 5.0);
        sh.anchor = anchors[i];
        wb.add_shape(0, sh);
    }
    wb
}

fn check_book(back: &Workbook, format: &str) {
    let want = book();
    let (w, g) = (&want.sheets[0], &back.sheets[0]);
    assert_eq!(w.shapes.len(), g.shapes.len(), "{format}");
    for (a, b) in w.shapes.iter().zip(&g.shapes) {
        same_look(&a.shape, &b.shape);
        assert_eq!(a.anchor, b.anchor, "{format}");
        let (pa, pb) = (a.pos(w), b.pos(g));
        close(pa.0, pb.0, format);
        close(pa.1, pb.1, format);
        close(a.width, b.width, format);
        close(a.height, b.height, format);
    }
}

/// Shapes, a text box and equations in one document: the ODF frames
/// of each are told apart.
#[test]
fn shapes_with_equations() {
    let mut doc = doc_sample();
    let n = doc.paragraphs.len();
    doc.paragraphs.push(Paragraph::new("x"));
    doc.insert_equation(apex_doc::Selection::caret(apex_doc::Pos::new(n, 1)), apex_doc::math::Equation::new("a/b"));
    for (back, format) in [(crate::odt::read(&crate::odt::write(&doc)).expect("odt"), "odt"), (crate::docx::read(&crate::docx::write(&doc)).expect("docx"), "docx")] {
        check_doc(&back, format);
        let eqs = back.paragraphs.iter().flat_map(|p| p.runs.iter()).filter(|r| r.attrs.math.is_some()).count();
        assert_eq!(eqs, 1, "{format}");
    }
}

#[test]
fn xlsx_shapes() {
    let back = crate::xlsx::read(&crate::xlsx::write(&book())).expect("xlsx");
    check_book(&back, "xlsx");
}

#[test]
fn ods_shapes() {
    let back = crate::ods::read(&crate::ods::write(&book())).expect("ods");
    check_book(&back, "ods");
}

#[test]
fn hostile_shapes() {
    // Broken numbers and missing parts load without panicking.
    let sp = r#"<wps:wsp><wps:spPr><a:xfrm rot="NaN"><a:ext cx="-5" cy="1e40"/></a:xfrm><a:prstGeom prst="nonsense"/><a:ln w="-3"><a:prstDash val="bogus"/></a:ln></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>x</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp>"#;
    let xml = format!(r#"<w:drawing><wp:anchor behindDoc="x" relativeHeight="-99999999999999"><wp:positionH relativeFrom="page"><wp:posOffset>1e99</wp:posOffset></wp:positionH><wp:extent cx="abc" cy="-1"/><a:graphic><a:graphicData>{sp}</a:graphicData></a:graphic></wp:anchor></w:drawing>"#);
    let root = crate::drawingml::read_fragment(&xml);
    let s = crate::docx_shape::read(&root).expect("shape");
    assert!(s.width.is_finite() && s.x.is_finite());
    let st = crate::odf_shape::Styles::default();
    for x in [
        r#"<draw:custom-shape svg:width="-1cm" svg:height="NaNcm" draw:transform="rotate (1e40) translate (x y)"><draw:enhanced-geometry draw:type="ooxml-"/></draw:custom-shape>"#,
        r#"<draw:line svg:x1="inf" svg:y1="1e39cm"/>"#,
        r#"<draw:frame><draw:text-box><text:p><text:s text:c="99999999999"/></text:p></draw:text-box></draw:frame>"#,
    ] {
        let root = crate::drawingml::read_fragment(x);
        if let Some(r) = crate::odf_shape::read(&root.kids[0], &st) {
            let d = crate::odf_shape::to_doc(r, "paragraph", &st);
            assert!(d.width.is_finite() && d.height.is_finite() && d.x.is_finite());
        }
    }
}

/// Writes the samples (to check them in other programs) and prints what
/// those programs saved back (in `back/`).
#[test]
#[ignore]
fn samples() {
    let Ok(dir) = std::env::var("APEX_F11_OUT") else { return };
    let dir = std::path::Path::new(&dir);
    let _ = std::fs::write(dir.join("shapes.docx"), crate::docx::write(&doc_sample()));
    let _ = std::fs::write(dir.join("shapes.odt"), crate::odt::write(&doc_sample()));
    let _ = std::fs::write(dir.join("shapes.xlsx"), crate::xlsx::write(&book()));
    let _ = std::fs::write(dir.join("shapes.ods"), crate::ods::write(&book()));
    // Fifty shapes, to time drawing them.
    let mut many = Workbook::new();
    let mut paras = Vec::new();
    for i in 0..50u32 {
        let s = looks()[i as usize % 5].clone();
        many.add_shape(0, SheetShape::new(s.clone(), CellRef::new(1 + (i / 5) * 3, 1 + (i % 5) * 2), 100.0, 50.0));
        let mut d = DocShape::new(s, 60.0, 30.0);
        (d.x, d.y) = ((i % 5) as f32 * 80.0, 0.0);
        let mut p = Paragraph::new("Line ");
        p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(d)), ..Default::default() });
        paras.push(p);
    }
    let _ = std::fs::write(dir.join("perf50.xlsx"), crate::xlsx::write(&many));
    let _ = std::fs::write(dir.join("perf50.docx"), crate::docx::write(&Document::from_paragraphs(paras)));
    let wb = book();
    for s in &wb.sheets[0].shapes {
        println!("  want {:?} {}x{} at {:?}", s.shape.kind, s.width, s.height, s.pos(&wb.sheets[0]));
    }
    let Ok(list) = std::fs::read_dir(dir.join("back")) else { return };
    for p in list.flatten().map(|e| e.path()) {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        let shapes: Vec<String> = match ext {
            "docx" | "odt" => {
                let d = if ext == "docx" { crate::docx::read(&bytes) } else { crate::odt::read(&bytes) };
                d.map(|d| d.shapes().iter().map(|(p, s)| format!("{} {:?} {:?} {}x{} at {},{} {:?}/{:?} {:?} z{} rot {} [{}]", p.para, s.shape.kind, s.wrap, s.width, s.height, s.x, s.y, s.h_rel, s.v_rel, s.h_align, s.z, s.shape.rotation, s.shape.plain_text())).collect()).unwrap_or_default()
            }
            "xlsx" | "ods" => {
                let w = if ext == "xlsx" { crate::xlsx::read(&bytes) } else { crate::ods::read(&bytes) };
                w.map(|w| w.sheets[0].shapes.iter().map(|s| format!("{:?} {:?} {}x{} at {:?} {:?}+{},{} colw {} rot {} fill {:?} line {:?} [{}]", s.shape.kind, s.anchor, s.width, s.height, s.pos(&w.sheets[0]), s.from, s.dx, s.dy, w.sheets[0].col_x(1), s.shape.rotation, s.shape.fill, s.shape.line, s.shape.plain_text())).collect()).unwrap_or_default()
            }
            _ => continue,
        };
        println!("{}:", p.display());
        for s in shapes {
            println!("  {s}");
        }
    }
}

/// Real files load without panicking.
#[test]
#[ignore]
fn real_files() {
    let Ok(dir) = std::env::var("APEX_F11_REAL") else { return };
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for p in list.flatten().map(|e| e.path()) {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let n: usize = match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "docx" => crate::docx::read(&bytes).map(|d| d.shapes().len()).unwrap_or(0),
            "odt" => crate::odt::read(&bytes).map(|d| d.shapes().len()).unwrap_or(0),
            "xlsx" | "xlsm" => crate::xlsx::read(&bytes).map(|w| w.sheets.iter().map(|s| s.shapes.len()).sum()).unwrap_or(0),
            "ods" => crate::ods::read(&bytes).map(|w| w.sheets.iter().map(|s| s.shapes.len()).sum()).unwrap_or(0),
            _ => continue,
        };
        println!("{}: {n} shapes", p.display());
    }
}

/// Pictures behind and in front of the text, placed on the page.
#[test]
fn floating_pictures() {
    let png = crate::xlsx_drawing::tests::png();
    let img = apex_doc::Image { crop: [0.1, 0.0, 0.0, 0.0], alt: "Logo".into(), ..apex_doc::Image::new(png, 60.0, 40.0) };
    let mut behind = DocShape::new(Shape::picture(img.clone()), 120.0, 80.0);
    (behind.wrap, behind.x, behind.y, behind.h_rel, behind.v_rel, behind.z) = (ShapeWrap::Behind, 36.0, 72.0, HRel::Page, VRel::Page, 0);
    behind.shape.rotation = 90.0;
    let mut front = DocShape::new(Shape::picture(img), 50.0, 30.0);
    (front.wrap, front.x, front.y, front.z) = (ShapeWrap::InFront, 10.0, 5.0, 1);
    let mut p = Paragraph::new("Text ");
    for d in [behind, front] {
        p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(d)), ..Default::default() });
    }
    let doc = Document::from_paragraphs(vec![p]);
    for (fmt, back) in [("docx", crate::docx::read(&crate::docx::write(&doc)).expect("docx")), ("odt", crate::odt::read(&crate::odt::write(&doc)).expect("odt"))] {
        let shapes = back.shapes();
        assert_eq!(shapes.len(), 2, "{fmt}");
        let (b, f) = (&shapes[0].1, &shapes[1].1);
        assert_eq!((b.wrap, b.h_rel, b.v_rel), (ShapeWrap::Behind, HRel::Page, VRel::Page), "{fmt}");
        close(b.x, 36.0, fmt);
        close(b.y, 72.0, fmt);
        close(b.width, 120.0, fmt);
        close(b.shape.rotation, 90.0, fmt);
        let pic = b.shape.picture.as_ref().expect(fmt);
        close(pic.crop[0], 0.1, fmt);
        assert_eq!(b.shape.alt, "Logo", "{fmt}");
        assert_eq!(f.wrap, ShapeWrap::InFront, "{fmt}");
        assert!(f.z > b.z, "{fmt}");
        close(f.x, 10.0, fmt);
        assert!(back.images().is_empty(), "{fmt}");
    }
}

/// Objects hidden and named in the Selection Pane: shapes, floating
/// pictures and pictures in the text keep both.
#[test]
fn hidden_and_named_objects() {
    let png = crate::xlsx_drawing::tests::png();
    let mut tb = Shape::new(ShapeKind::TextBox);
    tb.set_plain_text("Note");
    (tb.name, tb.hidden) = ("Secret box".into(), true);
    let mut boxed = DocShape::new(tb, 100.0, 40.0);
    boxed.z = 1;
    let img = apex_doc::Image { name: "Logo pic".into(), hidden: true, ..apex_doc::Image::new(png, 60.0, 40.0) };
    let mut front = DocShape::new(Shape::picture(apex_doc::Image { name: "Front pic".into(), ..img.clone() }), 50.0, 30.0);
    front.wrap = ShapeWrap::InFront;
    let mut visible = DocShape::new(Shape::new(ShapeKind::Ellipse), 30.0, 30.0);
    visible.shape.name = "Shown".into();
    let mut p = Paragraph::new("Text ");
    for d in [boxed, front, visible] {
        p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(d)), ..Default::default() });
    }
    p.push(&OBJECT.to_string(), Attrs { image: Some(Arc::new(img)), ..Default::default() });
    let doc = Document::from_paragraphs(vec![p]);
    for (fmt, back) in [("docx", crate::docx::read(&crate::docx::write(&doc)).expect("docx")), ("odt", crate::odt::read(&crate::odt::write(&doc)).expect("odt"))] {
        let shapes = back.shapes();
        let by = |n: &str| shapes.iter().find(|(_, s)| s.shape.name == n).map(|(_, s)| s.clone()).unwrap_or_else(|| panic!("{fmt} {n}"));
        assert!(by("Secret box").shape.hidden, "{fmt}");
        assert!(by("Front pic").shape.hidden, "{fmt}");
        assert!(!by("Shown").shape.hidden, "{fmt}");
        let imgs = back.images();
        assert_eq!(imgs.len(), 1, "{fmt}");
        assert_eq!(imgs[0].1.name, "Logo pic", "{fmt}");
        assert!(imgs[0].1.hidden, "{fmt}");
    }
}

const FREEFORM: &str = r#"<a:custGeom xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst><a:path w="100" h="50"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="100" y="10"/></a:lnTo><a:lnTo><a:pt x="40" y="50"/></a:lnTo><a:close/></a:path></a:pathLst></a:custGeom>"#;

fn freeform() -> Shape {
    let mut s = Shape::new(ShapeKind::Rect);
    s.custom = Some(Arc::from(FREEFORM));
    s.fill = Some(0x336699);
    s.flip_v = true;
    s
}

/// A drawn geometry keeps its paths through every format.
fn check_freeform(back: &Shape, format: &str) {
    let c = back.custom.as_ref().unwrap_or_else(|| panic!("{format}: no drawn geometry"));
    let (a, b) = (apex_doc::custom_paths(FREEFORM, 100.0, 50.0), apex_doc::custom_paths(c, 100.0, 50.0));
    assert_eq!(a.len(), b.len(), "{format}");
    assert_eq!(a[0].subpaths.len(), b[0].subpaths.len(), "{format}");
    for (p, q) in a[0].subpaths[0].0.iter().zip(&b[0].subpaths[0].0) {
        close(p.0, q.0, format);
        close(p.1, q.1, format);
    }
    assert_eq!((back.fill, back.flip_v), (Some(0x336699), true), "{format}");
}

#[test]
fn freeforms_in_documents() {
    let mut d = DocShape::new(freeform(), 100.0, 50.0);
    d.wrap = ShapeWrap::InFront;
    let mut p = Paragraph::new("");
    p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(d)), ..Default::default() });
    let doc = Document::from_paragraphs(vec![p]);
    for (back, format) in [(crate::docx::read(&crate::docx::write(&doc)).expect("docx"), "docx"), (crate::odt::read(&crate::odt::write(&doc)).expect("odt"), "odt")] {
        let shapes = back.shapes();
        assert_eq!(shapes.len(), 1, "{format}");
        check_freeform(&shapes[0].1.shape, format);
    }
}

#[test]
fn freeforms_in_workbooks() {
    let mut wb = Workbook::new();
    wb.add_shape(0, SheetShape::new(freeform(), CellRef::new(2, 1), 100.0, 50.0));
    for (back, format) in [(crate::xlsx::read(&crate::xlsx::write(&wb)).expect("xlsx"), "xlsx"), (crate::ods::read(&crate::ods::write(&wb)).expect("ods"), "ods")] {
        assert_eq!(back.sheets[0].shapes.len(), 1, "{format}");
        check_freeform(&back.sheets[0].shapes[0].shape, format);
    }
}
