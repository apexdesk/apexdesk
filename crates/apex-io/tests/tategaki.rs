//! Vertical pages (tategaki) and horizontal digits in vertical text
//! (tate-chu-yoko) through docx and odt, section by section.

use apex_doc::{Columns, Combine, Document, Paragraph, Pos, Section, SectionStart, Selection, TextFlow};

/// A vertical first section with horizontal digits, a horizontal one and
/// one with everything turned (Word's tbRlV).
fn doc() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("平成31年12月"), Paragraph::new("Horizontal"), Paragraph::new("全部回転")]);
    let horizontal = d.page;
    d.page.set_vertical(true);
    d.page.grid.kind = apex_doc::GridKind::Lines;
    d.page.grid.line_pitch = 18.0;
    let mut turned = horizontal;
    turned.text_flow = Some(TextFlow::TbRlV);
    for (i, page) in [(1, horizontal), (2, turned)] {
        d.paragraphs[i].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::NextPage, page, columns: Columns::default() }));
    }
    let sel = |a: usize, b: usize| Selection { anchor: Pos::new(0, a), cursor: Pos::new(0, b) };
    d.format_range(sel(6, 8), &|a| a.set_asian(|x| x.combine = Some(Combine { id: Some(1), vert: true, vert_compress: true, ..Default::default() })));
    d.format_range(sel(11, 13), &|a| a.set_asian(|x| x.combine = Some(Combine { id: Some(2), vert: true, ..Default::default() })));
    d
}

fn check(d: &Document, docx: bool) {
    assert_eq!(d.paragraphs.len(), 3);
    let flow = |i: usize| d.section_setup(i).0.text_flow;
    assert_eq!(flow(0), Some(TextFlow::TbRl));
    assert!(d.section_setup(0).0.upright());
    assert_eq!(flow(1), None);
    // ODF has one vertical writing mode.
    assert_eq!(flow(2), Some(if docx { TextFlow::TbRlV } else { TextFlow::TbRl }));
    let p = &d.paragraphs[0];
    assert_eq!(p.text, "平成31年12月");
    let at = |b: usize| p.attrs_at(b).asian().combine;
    let (c1, c2) = (at(7).expect("31"), at(12).expect("12"));
    assert!(c1.vert && c1.vert_compress && !c1.lines && !c1.letters);
    assert!(c2.vert && !c2.vert_compress);
    assert!(at(3).is_none() && at(10).is_none());
}

#[test]
fn docx_round_trip() {
    let d = doc();
    check(&d, true);
    let bytes = apex_io::docx::write(&d);
    let z = apex_io::zip::ZipReader::new(&bytes).unwrap();
    let xml = z.read_string("word/document.xml").unwrap();
    for s in [r#"<w:textDirection w:val="tbRl"/>"#, r#"<w:textDirection w:val="tbRlV"/>"#, r#"w:vert="1""#, r#"w:vertCompress="1""#] {
        assert!(xml.contains(s), "{s} missing");
    }
    let back = apex_io::docx::read(&bytes).unwrap();
    check(&back, true);
    // Saved again: the same.
    assert_eq!(apex_io::zip::ZipReader::new(&apex_io::docx::write(&back)).unwrap().read_string("word/document.xml").unwrap(), xml);
}

#[test]
fn odt_round_trip() {
    let d = doc();
    let bytes = apex_io::odt::write(&d);
    let z = apex_io::zip::ZipReader::new(&bytes).unwrap();
    let styles = z.read_string("styles.xml").unwrap();
    let content = z.read_string("content.xml").unwrap();
    assert!(styles.contains(r#"style:writing-mode="tb-rl""#) || content.contains(r#"style:writing-mode="tb-rl""#));
    // The lines of the vertical page follow each other across its width.
    let p = &d.page;
    let (lines, _) = p.grid.counts(p.content_height(), p.content_width(), d.normal_size_pt());
    assert!(styles.contains(&format!(r#"style:layout-grid-lines="{lines}""#)), "{lines}");
    // As LibreOffice writes Word's horizontal in vertical.
    assert!(content.contains(r#"style:text-rotation-angle="90""#), "{content}");
    let back = apex_io::odt::read(&bytes).unwrap();
    check(&back, false);
    check(&apex_io::docx::read(&apex_io::docx::write(&back)).unwrap(), false);
}
