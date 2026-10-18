//! Spacing between paragraphs as Word works it out: no space between
//! paragraphs of one style that ask for it (contextualSpacing), the
//! spacing of list items and table cells from their styles, and the line
//! spacing of Normal and the document defaults.

use apex_doc::{LineRule, ParaKind};

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn docx(body: &str, styles: &str) -> Vec<u8> {
    docx_numbered(body, styles, r#"<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl>"#)
}

fn docx_numbered(body: &str, styles: &str, levels: &str) -> Vec<u8> {
    let mut z = apex_io::zip::ZipWriter::new();
    let mut add = |name: &str, s: &str| z.add(name, s.as_bytes(), true);
    add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>"#);
    add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#);
    add("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>"#);
    add("word/document.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}</w:body></w:document>"#));
    add("word/styles.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W}>{styles}</w:styles>"#));
    add("word/numbering.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering {W}><w:abstractNum w:abstractNumId="0">{levels}</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#));
    z.finish()
}

/// Word 2013 and later: 8 pt after and 1.08 lines in the defaults, List
/// Paragraph with no space between its paragraphs, Table Grid with none
/// in its cells.
const STYLES: &str = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:style>"#;

fn item(text: &str) -> String {
    format!(r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#)
}

#[test]
fn list_items_keep_their_styles_fonts() {
    let styles = format!(r#"{STYLES}<w:style w:type="paragraph" w:customStyle="1" w:styleId="MyBullet"><w:name w:val="My Bullet"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:sz w:val="22"/></w:rPr></w:style>"#);
    let body = r#"<w:p><w:pPr><w:pStyle w:val="MyBullet"/></w:pPr><w:r><w:t>Item</w:t></w:r></w:p>"#;
    let d = apex_io::docx::read(&docx(body, &styles)).unwrap();
    let p = &d.paragraphs[0];
    assert_eq!(p.style.kind, ParaKind::Bullet);
    let f = d.char_format(&p.style, &p.runs[0].attrs);
    assert_eq!(f.font.as_deref(), Some("Arial"));
    assert_eq!(p.runs[0].attrs.size, Some(22));
}

#[test]
fn numbering_goes_on_after_a_table() {
    let table = r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let body = format!(r#"{}{table}<w:p><w:r><w:t>Text</w:t></w:r></w:p>{}{}"#, item("One"), item("Two"), item("Three"));
    let levels: String = (0..9).map(|l| format!(r#"<w:lvl w:ilvl="{l}"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%{}."/></w:lvl>"#, l + 1)).collect();
    let d = apex_io::docx::read(&docx_numbered(&body, STYLES, &levels)).unwrap();
    let marks: Vec<_> = d.list_markers().into_iter().flatten().collect();
    assert_eq!(marks, ["1.", "2.", "3."].map(|t| apex_doc::Marker::Text(t.into())));
}

#[test]
fn list_paragraphs_keep_their_spacing_and_contextual_spacing() {
    let body = format!(r#"<w:p><w:r><w:t>Intro</w:t></w:r></w:p>{}{}<w:p><w:pPr><w:pStyle w:val="ListParagraph"/></w:pPr><w:r><w:t>No bullet</w:t></w:r></w:p><w:p><w:pPr><w:contextualSpacing/></w:pPr><w:r><w:t>Direct</w:t></w:r></w:p>"#, item("One"), item("Two"));
    let d = apex_io::docx::read(&docx(&body, STYLES)).unwrap();
    let p = &d.paragraphs;
    assert_eq!(p[1].style.kind, ParaKind::Bullet);
    // Items: the style chain's spacing (the defaults' 8 pt and 1.08
    // lines, not our list kind's 2 pt), without space between them.
    for i in [1, 2] {
        assert_eq!(p[i].style.spacing(), (0.0, 8.0), "item {i}");
        assert!(p[i].style.contextual_spacing, "item {i}");
    }
    assert!(!p[0].style.contextual_spacing);
    assert!(p[3].style.contextual_spacing && p[4].style.contextual_spacing);
    // Saved and opened again (the paragraphs keep it; List Paragraph as a
    // custom style too).
    let out = apex_io::docx::write(&d);
    let xml = apex_io::zip::ZipReader::new(&out).unwrap().read_string("word/document.xml").unwrap();
    assert!(xml.contains("<w:contextualSpacing/>"));
    let again = apex_io::docx::read(&out).unwrap();
    let flags: Vec<bool> = again.paragraphs.iter().map(|p| p.style.contextual_spacing).collect();
    assert_eq!(flags, [false, true, true, true, true]);
    assert_eq!(again.paragraphs[1].style.space_after, Some(8.0));
    // And in odt (style:contextual-spacing).
    let odt = apex_io::odt::write(&d);
    let xml = apex_io::zip::ZipReader::new(&odt).unwrap().read_string("content.xml").unwrap();
    assert!(xml.contains(r#"style:contextual-spacing="true""#));
    let from_odt = apex_io::odt::read(&odt).unwrap();
    let flags: Vec<bool> = from_odt.paragraphs.iter().map(|p| p.style.contextual_spacing).collect();
    assert_eq!(flags, [false, true, true, true, true]);
}

#[test]
fn table_style_spacing_reaches_the_cells() {
    let cell = |t: &str| format!(r#"<w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>"#);
    let body = format!(r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr>{}</w:tr></w:tbl><w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr>{}</w:tr></w:tbl><w:p/>"#, cell("Grid"), cell("Plain"));
    let d = apex_io::docx::read(&docx(&body, STYLES)).unwrap();
    let grid = &d.paragraphs[0].style;
    assert!(d.paragraphs[0].cell.is_some());
    assert_eq!((grid.space_after, grid.line_rule, grid.line_spacing), (Some(0.0), LineRule::Multiple, 1.0));
    // A table without a style: Normal's spacing, written out.
    let plain = &d.paragraphs[1].style;
    assert_eq!(plain.space_after, Some(8.0));
    assert_eq!(plain.line_spacing, 0.0);
}

#[test]
fn normal_line_spacing_beats_the_defaults() {
    let styles = r#"<w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:pPr><w:spacing w:after="200" w:line="276" w:lineRule="auto"/></w:pPr></w:style>"#;
    let body = r#"<w:p><w:r><w:t>Normal</w:t></w:r></w:p><w:p><w:pPr><w:spacing w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:t>Single</w:t></w:r></w:p>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    assert_eq!(d.paragraphs[0].style.line_spacing, 1.15);
    // Single spacing set on the paragraph stays single (not "the
    // defaults'"), and is written as such.
    assert_eq!(d.paragraphs[1].style.line_spacing, 1.0);
    let again = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(again.paragraphs[1].style.line_spacing, 1.0);
    assert_eq!(again.paragraphs[0].style.line_spacing, 1.15);
    // Single in Normal over 1.08 in the defaults: the paragraphs are
    // single, not the defaults' (0 would lay them out at 1.08).
    let styles = r#"<w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:style>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    for p in &d.paragraphs {
        assert_eq!((p.style.line_rule, p.style.line_spacing), (LineRule::Multiple, 1.0));
    }
    let again = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(again.paragraphs[0].style.line_spacing, 1.0);
}

/// The line spacing a paragraph is laid out with (its own or its
/// style's).
fn line(d: &apex_doc::Document, i: usize) -> f32 {
    let st = &d.paragraphs[i].style;
    if st.line_spacing > 0.0 { st.line_spacing } else { d.styles.default_line(st) }
}

#[test]
fn single_spacing_survives_odt_over_wider_defaults() {
    // 1.15 lines in the defaults, one paragraph single.
    let styles = r#"<w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:after="200" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>"#;
    let body = r#"<w:p><w:r><w:t>Defaults</w:t></w:r></w:p><w:p><w:pPr><w:spacing w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:t>Single</w:t></w:r></w:p>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    assert_eq!((line(&d, 0), line(&d, 1)), (1.15, 1.0));
    let odt = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!((line(&odt, 0), line(&odt, 1)), (1.15, 1.0));
}

#[test]
fn odt_paragraphs_take_their_style_line_spacing() {
    // Standard at 300%, paragraphs that don't set theirs (LibreOffice lays
    // this out on two pages): in docx too.
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#;
    let styles = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns} office:version="1.3"><office:styles><style:style style:name="Standard" style:family="paragraph"><style:paragraph-properties fo:line-height="300%"/></style:style></office:styles></office:document-styles>"#);
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns} office:version="1.3"><office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:parent-style-name="Standard"/><style:style style:name="P2" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:line-height="100%"/></style:style></office:automatic-styles><office:body><office:text><text:p text:style-name="P1">Triple</text:p><text:p text:style-name="P2">Single</text:p></office:text></office:body></office:document-content>"#);
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    let d = apex_io::odt::read(&z.finish()).unwrap();
    assert_eq!((line(&d, 0), line(&d, 1)), (3.0, 1.0));
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!((line(&back, 0), line(&back, 1)), (3.0, 1.0));
}
