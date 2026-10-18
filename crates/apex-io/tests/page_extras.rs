//! Line numbers and other page settings.

use apex_doc::{Document, LineNumbering, LineRestart, ParaStyle, Paragraph};

fn sample() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("One"), Paragraph::new("Two").with_style(ParaStyle { no_line_numbers: true, ..Default::default() })]);
    d.page.line_numbers = Some(LineNumbering { count_by: 5, start: 3, restart: LineRestart::EachSection, distance: Some(20.0) });
    d
}

#[test]
fn line_numbers_round_trip() {
    let d = sample();
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(back.page.line_numbers, d.page.line_numbers);
    assert!(!back.paragraphs[0].style.no_line_numbers && back.paragraphs[1].style.no_line_numbers);
    // OpenDocument: no start value, and restarts on each page or never.
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(back.page.line_numbers, Some(LineNumbering { start: 1, restart: LineRestart::Continuous, ..d.page.line_numbers.unwrap() }));
    assert!(!back.paragraphs[0].style.no_line_numbers && back.paragraphs[1].style.no_line_numbers);
    let plain = Document::from_paragraphs(vec![Paragraph::new("x")]);
    assert_eq!(apex_io::docx::read(&apex_io::docx::write(&plain)).unwrap().page.line_numbers, None);
    assert_eq!(apex_io::odt::read(&apex_io::odt::write(&plain)).unwrap().page.line_numbers, None);
}

#[test]
fn hyphens_round_trip() {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("biblio\u{AD}teca e-mail no\u{2011}break")]);
    d.auto_hyphenation = true;
    let docx = apex_io::docx::write(&d);
    let back = apex_io::docx::read(&docx).unwrap();
    assert_eq!(back.paragraphs[0].text, d.paragraphs[0].text);
    assert!(back.auto_hyphenation);
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(back.paragraphs[0].text, d.paragraphs[0].text);
    assert!(back.auto_hyphenation);
    let plain = Document::from_paragraphs(vec![Paragraph::new("x")]);
    assert!(!apex_io::docx::read(&apex_io::docx::write(&plain)).unwrap().auto_hyphenation);
    assert!(!apex_io::odt::read(&apex_io::odt::write(&plain)).unwrap().auto_hyphenation);
}

fn zip_part(bytes: &[u8], name: &str) -> String {
    apex_io::zip::ZipReader::new(bytes).unwrap().read_string(name).unwrap()
}

/// A continuous section with other margins or headers on paper of the
/// same size stays on the page in odt too: a text section, its master
/// page following the current one (as LibreOffice imports docx).
#[test]
fn odt_continuous_section_with_its_own_page_setup() {
    use apex_doc::{HfSlot, Section, SectionStart};
    use std::sync::Arc;
    let mut d = Document::from_paragraphs((0..4).map(|i| Paragraph::new(&format!("P{i}"))).collect());
    d.headers.set(HfSlot::Header, Some(Arc::new(vec![Paragraph::new("H")])));
    // A new page like the first (it shows the same master page), then a
    // continuous section with other margins and its own footer.
    d.paragraphs[1].section = Some(Box::new(Section { start: SectionStart::NextPage, page: d.page, columns: Default::default(), headers: Default::default() }));
    let mut page = d.page;
    page.margin_top += 20.0;
    page.margin_left += 10.0;
    let mut headers = apex_doc::Headers::default();
    headers.set(HfSlot::Footer, Some(Arc::new(vec![Paragraph::new("F")])));
    d.paragraphs[3].section = Some(Box::new(Section { start: SectionStart::Continuous, page, columns: Default::default(), headers }));
    let bytes = apex_io::odt::write(&d);
    let content = zip_part(&bytes, "content.xml");
    assert!(content.contains("<text:section ") && content.contains(r#"fo:margin-left="10pt""#), "{content}");
    let styles = zip_part(&bytes, "styles.xml");
    assert!(styles.contains("style:next-style-name="), "{styles}");
    let again = apex_io::odt::write(&apex_io::odt::read(&bytes).unwrap());
    for back in [apex_io::odt::read(&bytes).unwrap(), apex_io::odt::read(&again).unwrap()] {
        let s1 = back.paragraphs[1].section.as_ref().unwrap();
        assert_eq!(s1.start, SectionStart::NextPage);
        assert!((s1.page.margin_top - d.page.margin_top).abs() < 0.1);
        assert!(back.paragraphs[2].section.is_none());
        let s = back.paragraphs[3].section.as_ref().unwrap();
        assert_eq!(s.start, SectionStart::Continuous);
        assert!((s.page.margin_top - page.margin_top).abs() < 0.1 && (s.page.margin_left - page.margin_left).abs() < 0.1, "{:?}", s.page);
        assert_eq!(s.headers.get(HfSlot::Footer).unwrap()[0].text, "F");
        // The header is the one before (linked).
        assert!(s.headers.get(HfSlot::Header).is_none());
        assert_eq!(back.hf_story(2, HfSlot::Header).unwrap()[0].text, "H");
    }
}

/// A footer holding only an empty table is kept (it shows), with a
/// paragraph after the table as Writer and Word have.
#[test]
fn odt_footer_with_an_empty_table() {
    use apex_doc::HfSlot;
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0""#;
    let styles = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns} office:version="1.3"><office:automatic-styles><style:page-layout style:name="pm1"/></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"><style:footer><table:table table:name="T"><table:table-column/><table:table-row><table:table-cell><text:p/></table:table-cell></table:table-row></table:table></style:footer></style:master-page></office:master-styles></office:document-styles>"#);
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns} office:version="1.3"><office:body><office:text><text:p>a</text:p></office:text></office:body></office:document-content>"#);
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    let d = apex_io::odt::read(&z.finish()).unwrap();
    for back in [apex_io::odt::read(&apex_io::odt::write(&d)).unwrap(), apex_io::docx::read(&apex_io::docx::write(&d)).unwrap(), d] {
        let f = back.headers.get(HfSlot::Footer).expect("footer");
        assert_eq!(f.len(), 2);
        assert!(f[0].cell.is_some() && f[1].cell.is_none());
    }
}
