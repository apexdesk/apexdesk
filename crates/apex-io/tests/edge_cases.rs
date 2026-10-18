//! Odd and hostile documents, built here: empty and very deep structures,
//! cells missing from ODF tables, RTF headers with tables, unbalanced RTF,
//! huge Markdown tables, repeated ODF changes; and saved files that follow
//! the schemas (unique style ids, no negative spacing or hanging indents,
//! valid ODF languages and page backgrounds).

use std::sync::Arc;

use apex_doc::{Attrs, Document, ParaKind, Paragraph};

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn docx(body: &str, styles: &str) -> Vec<u8> {
    let mut z = apex_io::zip::ZipWriter::new();
    let mut add = |name: &str, s: &str| z.add(name, s.as_bytes(), true);
    add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#);
    add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#);
    add("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#);
    add("word/document.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}</w:body></w:document>"#));
    add("word/styles.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W}>{styles}</w:styles>"#));
    z.finish()
}

fn odt(body: &str) -> Vec<u8> {
    odt_with(body, None)
}

fn odt_with(body: &str, styles: Option<&str>) -> Vec<u8> {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" office:version="1.3"><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
    );
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    if let Some(s) = styles {
        z.add("styles.xml", s.as_bytes(), true);
    }
    z.finish()
}

fn part(bytes: &[u8], name: &str) -> String {
    apex_io::zip::ZipReader::new(bytes).unwrap().read_string(name).unwrap()
}

fn texts(d: &Document) -> Vec<&str> {
    d.paragraphs.iter().map(|p| p.text.as_str()).collect()
}

fn all_text(d: &Document) -> String {
    texts(d).join("\n")
}

#[test]
fn docx_empty_paragraphs_and_line_ends_in_text() {
    // `<w:p/>` is a paragraph; a line end inside w:t is a space to Word.
    let d = apex_io::docx::read(&docx("<w:p/><w:p><w:r><w:t>one\ntwo</w:t></w:r></w:p><w:p/><w:p><w:r><w:t>end</w:t></w:r></w:p>", "")).unwrap();
    assert_eq!(texts(&d), ["", "one two", "", "end"]);
}

#[test]
fn docx_plain_outline_paragraphs_are_not_headings() {
    // A level given to a Normal paragraph (empty or not) changes no looks.
    let d = apex_io::docx::read(&docx(r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:p><w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>"#, "")).unwrap();
    let kinds: Vec<_> = d.paragraphs.iter().map(|p| p.style.kind).collect();
    assert_eq!(kinds, [apex_doc::ParaKind::Normal, apex_doc::ParaKind::Normal]);
}

#[test]
fn docx_plain_text_style_in_a_proportional_font_is_not_code() {
    let body = r#"<w:p><w:pPr><w:pStyle w:val="pt"/></w:pPr><w:r><w:t>19.02.2025</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="pre"/></w:pPr><w:r><w:t>x = 1</w:t></w:r></w:p>"#;
    let styles = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:styleId="pt"><w:name w:val="Plain Text"/></w:style><w:style w:type="paragraph" w:styleId="pre"><w:name w:val="HTML Preformatted"/><w:rPr><w:rFonts w:ascii="Courier New"/></w:rPr></w:style>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    let kinds: Vec<_> = d.paragraphs.iter().map(|p| p.style.kind).collect();
    assert_eq!(kinds, [apex_doc::ParaKind::Normal, apex_doc::ParaKind::Code]);
}

#[test]
fn docx_default_size_on_text_beats_its_styles_size() {
    // 12 pt is the default, but the style makes it 11: the run's 12 counts.
    // A heading inherits Normal's space after, not ours.
    let body = r#"<w:p><w:pPr><w:pStyle w:val="lp"/></w:pPr><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>twelve</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Heading3"/></w:pPr><w:r><w:t>h</w:t></w:r></w:p>"#;
    let styles = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="24"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:pPr><w:spacing w:after="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="lp"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="22"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="40"/></w:pPr></w:style>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    assert_eq!(d.paragraphs[0].runs[0].attrs.size, Some(24));
    assert_eq!(d.paragraphs[1].style.space_after, Some(0.0));
}

#[test]
fn docx_table_style_sizes_the_text_of_its_cells() {
    // Its 11 pt where the text sets none; 12 pt set in a cell stays.
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="grid"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/><w:p><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>01</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>out</w:t></w:r></w:p>"#;
    let styles = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="24"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="table" w:styleId="grid"><w:name w:val="Table Grid"/><w:rPr><w:sz w:val="22"/></w:rPr></w:style>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    let sizes: Vec<_> = d.paragraphs.iter().map(|p| p.runs[0].attrs.size).collect();
    assert_eq!(sizes, [Some(22), Some(24), None]);
}

#[test]
fn docx_table_style_cell_margins_sit_under_the_tables_own() {
    // Top and bottom from the style's base, left from the table itself.
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="mine"/><w:tblCellMar><w:left w:w="200" w:type="dxa"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let styles = r#"<w:style w:type="table" w:styleId="base"><w:name w:val="Base"/><w:tblPr><w:tblCellMar><w:top w:w="85" w:type="dxa"/><w:bottom w:w="85" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="table" w:styleId="mine"><w:name w:val="Mine"/><w:basedOn w:val="base"/></w:style>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    let m = d.paragraphs[0].cell.as_ref().unwrap().table_props.as_ref().unwrap().margins();
    assert_eq!(m, [4.25, 10.0, 4.25, apex_doc::CELL_PAD_X]);
}

#[test]
fn docx_table_style_bands_both_ways_and_no_fill_on_a_cell_hides_them() {
    let row = |shd: &str, t: &str| format!(r#"<w:tr><w:tc><w:tcPr>{shd}</w:tcPr><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc></w:tr>"#);
    let auto = r#"<w:shd w:val="clear" w:color="auto" w:fill="auto"/>"#;
    let body = format!(r#"<w:tbl><w:tblPr><w:tblStyle w:val="b"/><w:tblLook w:firstRow="1" w:noHBand="0"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>{}{}{}{}</w:tbl><w:p/>"#, row("", "h"), row(auto, "a"), row("", "b"), row("", "c"));
    let styles = r#"<w:style w:type="table" w:styleId="b"><w:name w:val="Bands"/><w:tblStylePr w:type="band1Horz"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="00FF00"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="band2Horz"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="0000FF"/></w:tcPr></w:tblStylePr></w:style>"#;
    let d = apex_io::docx::read(&docx(&body, styles)).unwrap();
    let st = d.styles.tables.iter().find(|s| s.id == "b").unwrap();
    let tp = d.paragraphs[0].cell.as_ref().unwrap().table_props.as_ref().unwrap();
    let fills: Vec<_> = (0..4).map(|r| tp.cell_look_in(Some(st), &d.paragraphs[r].cell.as_ref().unwrap().cell.as_deref().cloned().unwrap_or_default(), r, 0, 4, 1).fill).collect();
    assert_eq!(fills, [None, Some(0xFFFFFF), Some(0x0000FF), Some(0x00FF00)]);
}

#[test]
fn docx_symbols_read_as_unicode() {
    let d = apex_io::docx::read(&docx(r#"<w:p><w:r><w:sym w:font="Wingdings" w:char="F0A3"/><w:t xml:space="preserve"> Oui </w:t><w:sym w:font="Symbol" w:char="F061"/></w:r></w:p>"#, "")).unwrap();
    assert_eq!(texts(&d), ["○ Oui α"]);
}

#[test]
fn docx_elements_inside_text_are_dropped() {
    // Markup pasted into a w:t: Word shows only the w:t's own text.
    let d = apex_io::docx::read(&docx("<w:p><w:r><w:t>kept<w:p><w:r><w:t>hidden</w:t></w:r></w:p> too</w:t></w:r></w:p>", "")).unwrap();
    assert_eq!(texts(&d), ["kept too"]);
}

#[test]
fn deep_nesting_is_capped_without_losing_text() {
    let n = 200;
    let mut body = String::new();
    for i in 0..n {
        body.push_str(&format!("<w:tbl><w:tr><w:tc><w:p><w:r><w:t>L{i}</w:t></w:r></w:p>"));
    }
    body.push_str("<w:p><w:r><w:t>deep</w:t></w:r></w:p>");
    for _ in 0..n {
        body.push_str("</w:tc></w:tr></w:tbl><w:p/>");
    }
    let d = apex_io::docx::read(&docx(&body, "")).unwrap();
    let depth = |d: &Document| d.tables().iter().map(|t| t.depth).max().unwrap_or(0);
    assert!(depth(&d) < apex_doc::MAX_NESTING);
    let text = all_text(&d);
    assert!(text.contains("deep") && text.contains("L0") && text.contains(&format!("L{}", n - 1)));
    // The same through ODF, which is read recursively.
    let o = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert!(all_text(&o).contains("deep"));
    let mut body = String::new();
    for i in 0..n {
        body.push_str(&format!("<table:table><table:table-column/><table:table-row><table:table-cell><text:p>L{i}</text:p>"));
    }
    body.push_str("<text:p>deep</text:p>");
    for _ in 0..n {
        body.push_str("</table:table-cell></table:table-row></table:table>");
    }
    let o = apex_io::odt::read(&odt(&body)).unwrap();
    assert!(depth(&o) < apex_doc::MAX_NESTING);
    let text = all_text(&o);
    assert!(text.contains("deep") && text.contains(&format!("L{}", n - 1)));
}

#[test]
fn odt_cells_after_a_span_without_covered_cells() {
    // The covered cells of the span are missing: the next cell still goes
    // after the span, as LibreOffice places it.
    let body = r#"<table:table><table:table-column table:number-columns-repeated="3"/><table:table-row><table:table-cell table:number-columns-spanned="2"><text:p>AB</text:p></table:table-cell><table:table-cell><text:p>C</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell table:number-columns-spanned="3"><text:p>wide</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell><text:p>x</text:p></table:table-cell><table:table-cell><text:p>y</text:p></table:table-cell><table:table-cell><text:p>z</text:p></table:table-cell></table:table-row></table:table>"#;
    let d = apex_io::odt::read(&odt(body)).unwrap();
    let t = &d.tables()[0];
    assert_eq!((t.rows, t.cols), (3, 3));
    let cell = |r: usize, c: usize| d.paragraphs[t.cells[r * 3 + c].0].text.as_str();
    assert_eq!([cell(0, 0), cell(0, 2), cell(1, 0), cell(2, 2)], ["AB", "C", "wide", "z"]);
    assert_eq!(t.span[0], (1, 2));
    // And to Word and back.
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    let t = &back.tables()[0];
    assert_eq!(back.paragraphs[t.cells[2].0].text, "C");
}

#[test]
fn odt_change_used_many_times_is_placed_once() {
    let marks = r#"<text:change text:change-id="ct1"/>"#.repeat(500);
    let body = format!(
        r#"<text:tracked-changes><text:changed-region text:id="ct1"><text:deletion><office:change-info><dc:creator>A</dc:creator><dc:date>2020-01-01T00:00:00</dc:date></office:change-info><text:p>gone</text:p></text:deletion></text:changed-region></text:tracked-changes><text:p>kept{marks}</text:p>"#
    );
    let d = apex_io::odt::read(&odt(&body)).unwrap();
    let text = all_text(&d);
    assert!(text.contains("kept"));
    assert!(text.matches("gone").count() <= 1, "{text}");
}

#[test]
fn rtf_header_tables_and_unbalanced_groups() {
    let src = br"{\rtf1\ansi{\header\trowd\cellx2000\cellx4000\pard\intbl H1\cell H2\cell\row\pard\par}\pard Body\par}";
    let d = apex_io::rtf::read(src).unwrap();
    for bytes in [apex_io::rtf::write(&d), apex_io::docx::write(&d), apex_io::odt::write(&d)] {
        assert!(!bytes.is_empty());
    }
    let back = apex_io::rtf::read(&apex_io::rtf::write(&d)).unwrap();
    assert!(all_text(&back).contains("Body"));
    // A note closed with no group open, and stray closing braces.
    for src in [&br"{\rtf\footnote }"[..], br"{\rtf1 a{\footnote b}}}}} c", br"{\rtf1 {\b x}}}{\i y"] {
        if let Ok(d) = apex_io::rtf::read(src) {
            let _ = apex_io::rtf::write(&d);
        }
    }
}

#[test]
fn markdown_tables_longer_than_the_row_limit_are_split() {
    let rows = apex_doc::MAX_ROWS + 5;
    let mut md = String::from("| a |\n|---|\n");
    for i in 0..rows {
        md.push_str(&format!("| r{i} |\n"));
    }
    let d = apex_io::markdown::read(&md);
    let tables = d.tables();
    assert_eq!(tables.len(), 2);
    assert_eq!(tables.iter().map(|t| t.rows).sum::<usize>(), rows + 1);
    assert!(d.paragraphs.iter().any(|p| p.text == format!("r{}", rows - 1)));
}

#[test]
fn docx_styles_follow_the_schema() {
    // The file's own "ListParagraph", negative spacing (Word files have -1).
    let styles = r#"<w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:before="-1" w:after="-1"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="-1" w:after="-1"/><w:ind w:left="720"/></w:pPr></w:style>"#;
    let body = r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/></w:pPr><w:r><w:t>item</w:t></w:r></w:p>"#;
    let d = apex_io::docx::read(&docx(body, styles)).unwrap();
    let out = apex_io::docx::write(&d);
    let styles = part(&out, "word/styles.xml");
    assert_eq!(styles.matches(r#"w:styleId="ListParagraph""#).count(), 1, "{styles}");
    assert!(!styles.contains(r#"="-1""#), "{styles}");
    // Word's extensions in styles.xml are ignorable.
    assert!(styles.contains(r#"mc:Ignorable="w14 w15""#));
    // A list item whose first line starts past the text: w:firstLine, as
    // w:hanging can't be negative.
    let mut p = Paragraph::new("item");
    p.style.kind = ParaKind::Bullet;
    p.style.first_line = 36.0;
    let doc = Document::from_paragraphs(vec![p]);
    let xml = part(&apex_io::docx::write(&doc), "word/document.xml");
    assert!(!xml.contains(r#"w:hanging="-"#), "{xml}");
    assert!(xml.contains(r#"w:firstLine="360""#), "{xml}");
    let back = apex_io::docx::read(&apex_io::docx::write(&doc)).unwrap();
    assert_eq!(back.paragraphs[0].style.first_line, 36.0);
}

#[test]
fn docx_note_settings_only_for_the_notes_there_are() {
    let d = apex_io::rtf::read(br"{\rtf1 Text{\footnote Note}\par}").unwrap();
    let settings = part(&apex_io::docx::write(&d), "word/settings.xml");
    assert!(settings.contains("w:footnotePr") && !settings.contains("w:endnotePr"), "{settings}");
    let plain = Document::from_paragraphs(vec![Paragraph::new("x")]);
    let settings = part(&apex_io::docx::write(&plain), "word/settings.xml");
    assert!(!settings.contains("w:footnotePr") && !settings.contains("w:endnotePr"), "{settings}");
}

#[test]
fn odt_languages_margins_and_page_background_follow_the_schema() {
    let mut p = Paragraph::new("");
    p.push("Здраво", Attrs { lang: Some(Arc::from("sr-Latn-RS")), ..Default::default() });
    p.push(" none", Attrs { lang: Some(Arc::from("x-none")), ..Default::default() });
    p.style.space_before = Some(-0.05);
    let mut doc = Document::from_paragraphs(vec![p]);
    doc.deco.color = Some(0xFFEEDD);
    let out = apex_io::odt::write(&doc);
    let content = part(&out, "content.xml");
    assert!(content.contains(r#"fo:language="sr" fo:country="RS""#), "{content}");
    assert!(!content.contains("Latn") && !content.contains("none\""), "{content}");
    assert!(!content.contains(r#"fo:margin-top="-"#), "{content}");
    let styles = part(&out, "styles.xml");
    assert!(styles.contains(r##"fo:background-color="#ffeedd""##) && !styles.contains(r#"draw:fill="solid""#), "{styles}");
    let back = apex_io::odt::read(&out).unwrap();
    assert_eq!(back.deco.color, Some(0xFFEEDD));
}

#[test]
fn odt_zero_hyphenation_counts_are_dropped() {
    let styles = r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3"><office:styles><style:default-style style:family="paragraph"><style:text-properties fo:font-size="11pt" fo:hyphenation-remain-char-count="0" fo:hyphenation-push-char-count="2"/></style:default-style></office:styles></office:document-styles>"#;
    let out = apex_io::odt::write(&apex_io::odt::read(&odt_with("<text:p>x</text:p>", Some(styles))).unwrap());
    let styles = part(&out, "styles.xml");
    assert!(!styles.contains(r#"fo:hyphenation-remain-char-count="0""#), "{styles}");
    assert!(styles.contains(r#"fo:hyphenation-push-char-count="2""#), "{styles}");
}

#[test]
fn continuous_section_changing_only_the_grid_stays_on_the_page() {
    let page = r#"<w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1296" w:right="1008" w:bottom="1296" w:left="1008" w:header="720" w:footer="720" w:gutter="0"/>"#;
    let body = format!(
        r#"<w:p><w:r><w:t>First</w:t></w:r></w:p><w:p><w:pPr><w:sectPr>{page}<w:cols w:num="2" w:sep="1" w:space="720"/></w:sectPr></w:pPr><w:r><w:t>Second</w:t></w:r></w:p><w:p/><w:sectPr><w:type w:val="continuous"/>{page}<w:cols w:num="2" w:space="720"/><w:docGrid w:linePitch="299"/></w:sectPr>"#
    );
    let d = apex_io::docx::read(&docx(&body, "")).unwrap();
    assert_eq!(texts(&d), ["First", "Second", ""]);
    let content = part(&apex_io::odt::write(&d), "content.xml");
    assert!(!content.contains("master-page-name"), "{content}");
}

/// A document with one footnote, saved here, with its styles and notes
/// parts replaced.
fn docx_with_note(styles: &str, notes: &str) -> Vec<u8> {
    use apex_doc::{NoteKind, Pos, Selection};
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Body")]);
    d.insert_note(Selection::caret(Pos::new(0, 4)), NoteKind::Foot);
    let saved = apex_io::docx::write(&d);
    let zr = apex_io::zip::ZipReader::new(&saved).unwrap();
    let mut z = apex_io::zip::ZipWriter::new();
    for name in zr.names() {
        let data = match name {
            "word/styles.xml" => format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W}>{styles}</w:styles>"#).into_bytes(),
            "word/footnotes.xml" => format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:footnotes {W}>{notes}</w:footnotes>"#).into_bytes(),
            _ => zr.read(name).unwrap(),
        };
        z.add(name, &data, true);
    }
    z.finish()
}

#[test]
fn docx_note_spacing_and_separator_round_trip() {
    use apex_doc::{LineRule, NoteKind, NoteSeparator, Pos, Selection};
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Body text")]);
    let at = Pos::new(0, 4);
    d.insert_note(Selection::caret(at), NoteKind::Foot);
    let (_, n) = d.note_at(at).unwrap();
    let mut n = (*n).clone();
    n.set_text("A note");
    n.paras[0].style.space_after = Some(6.0);
    n.paras[0].style.line_spacing = 1.5;
    d.set_note(at, n);
    let sep = NoteSeparator { size: 20, line: 100, rule: LineRule::Multiple, before: 0, after: 120 };
    d.notes.separator = Some(sep);
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(back.notes.separator, Some(sep));
    let (_, n) = back.note_at(at).unwrap();
    assert_eq!(n.paras[0].style.space_after, Some(6.0));
    assert!((n.paras[0].style.line_spacing - 1.5).abs() < 0.01, "{}", n.paras[0].style.line_spacing);
    // And in .odt.
    let odt = apex_io::odt::read(&apex_io::odt::write(&back)).unwrap();
    let (_, n) = odt.note_at(at).unwrap();
    assert_eq!(n.paras[0].style.space_after, Some(6.0));
    assert!((n.paras[0].style.line_spacing - 1.5).abs() < 0.01, "{}", n.paras[0].style.line_spacing);
    // Without one, the separator is a 12 pt line.
    d.notes.separator = None;
    let notes = part(&apex_io::docx::write(&d), "word/footnotes.xml");
    assert!(notes.contains(r#"w:line="240" w:lineRule="exact""#), "{notes}");
}

#[test]
fn docx_note_paragraphs_take_their_style_spacing_and_size() {
    // The note style: 10 pt, single lines, nothing after; the defaults have
    // 8 pt after and 1.08 lines.
    let styles = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/></w:rPr></w:style>"#;
    let notes = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:t>Note</w:t></w:r></w:p></w:footnote>"#;
    let d = apex_io::docx::read(&docx_with_note(styles, notes)).unwrap();
    let (_, n) = d.note_at(apex_doc::Pos::new(0, 4)).expect("the note");
    let p = &n.paras[0];
    assert_eq!(p.style.space_after, Some(0.0));
    assert!(p.style.line_spacing == 0.0 || (p.style.line_spacing - 1.0).abs() < 0.01, "{}", p.style.line_spacing);
    assert!(p.runs.iter().all(|r| r.attrs.size == Some(20)), "{:?}", p.runs);
    // The separator: an 11 pt paragraph, single lines, no spacing.
    let sep = d.notes.separator.expect("separator");
    assert_eq!((sep.size, sep.line, sep.before, sep.after), (22, 100, 0, 0));
}

#[test]
fn docx_empty_paragraphs_keep_their_mark_look() {
    // An empty paragraph is as tall as its mark: 20 pt Arial here.
    let body = r#"<w:p><w:pPr><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:sz w:val="40"/></w:rPr></w:pPr></w:p><w:p><w:pPr><w:rPr><w:sz w:val="40"/></w:rPr></w:pPr><w:r><w:t>text</w:t></w:r></w:p>"#;
    let d = apex_io::docx::read(&docx(body, "")).unwrap();
    let a = &d.paragraphs[0].runs[0].attrs;
    assert_eq!((a.size, a.font.as_deref()), (Some(40), Some("Arial")));
    // The mark of a paragraph with text doesn't change its runs.
    assert_eq!(d.paragraphs[1].runs[0].attrs.size, None);
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(back.paragraphs[0].runs[0].attrs.size, Some(40));
}

#[test]
fn rtf_nested_past_any_real_file() {
    // Each group copies its parent's formatting: 300 000 levels used to
    // take gigabytes. Past the limit groups keep their parent's formatting.
    let n = 300_000;
    let rtf = format!("{{\\rtf1 {}\\b x{} y}}", "{".repeat(n), "}".repeat(n));
    let d = apex_io::rtf::read(rtf.as_bytes()).unwrap();
    let text = all_text(&d);
    assert!(text.contains('x') && text.contains('y'), "{text}");
}

/// `bytes` (a package) with part `name` replaced by `data`.
fn swap_part(bytes: &[u8], name: &str, data: &[u8]) -> Vec<u8> {
    let z = apex_io::zip::ZipReader::new(bytes).unwrap();
    let mut w = apex_io::zip::ZipWriter::new();
    let names: Vec<String> = z.names().map(str::to_string).collect();
    for n in names {
        let d = if n == name { data.to_vec() } else { z.read(&n).unwrap() };
        w.add(&n, &d, true);
    }
    w.finish()
}

#[test]
fn pptx_slide_of_millions_of_elements() {
    // 30 MB of empty elements (a few KB compressed) under the slide's root.
    let deck = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/pptx/tbl-cell.pptx")).unwrap();
    let slide = part(&deck, "ppt/slides/slide1.xml");
    let at = slide.find("<p:cSld").unwrap();
    let bomb = format!("{}{}{}", &slide[..at], "<x/>".repeat(2_000_000), &slide[at..]);
    let d = apex_io::pptx::read(&swap_part(&deck, "ppt/slides/slide1.xml", bomb.as_bytes())).unwrap();
    // The slide's shapes, after the junk, are still read.
    let before = apex_io::pptx::read(&deck).unwrap();
    assert_eq!(d.slides[0].objects.len(), before.slides[0].objects.len());
    assert!(!d.slides[0].objects.is_empty());
}

#[test]
fn pptx_same_slide_listed_many_times() {
    // 50 000 entries of the slide list naming one slide part: it is read
    // once, and the deck saves quickly.
    let deck = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/pptx/tbl-cell.pptx")).unwrap();
    let before = apex_io::pptx::read(&deck).unwrap().slides.len();
    let pres = part(&deck, "ppt/presentation.xml");
    let at = pres.find("<p:sldId ").unwrap();
    let end = at + pres[at..].find("/>").unwrap() + 2;
    let many = format!("{}{}{}", &pres[..end], pres[at..end].repeat(50_000), &pres[end..]);
    let d = apex_io::pptx::read(&swap_part(&deck, "ppt/presentation.xml", many.as_bytes())).unwrap();
    assert_eq!(d.slides.len(), before);
    let t = std::time::Instant::now();
    apex_io::pptx::write(&d);
    assert!(t.elapsed().as_secs() < 5);
}

#[test]
fn markdown_of_a_long_numbered_list() {
    // Numbering each item walked the list back: quadratic.
    let paras: Vec<Paragraph> = (0..100_000)
        .map(|i| {
            let mut p = Paragraph::new(format!("item {i}"));
            p.style.kind = ParaKind::Numbered;
            p
        })
        .collect();
    let d = Document::from_paragraphs(paras);
    let t = std::time::Instant::now();
    let md = apex_io::markdown::write(&d);
    assert!(t.elapsed().as_secs() < 5);
    assert!(md.contains("100000. item 99999"), "{}", &md[md.len() - 40..]);
}

#[test]
fn text_of_millions_of_lines_opens_in_part() {
    // Two million empty lines took gigabytes: the document keeps the
    // first MAX_PARAGRAPHS and says it left the rest out.
    let text = "\n".repeat(2_000_000);
    let d = Document::from_plain_text(&text);
    assert!(d.truncated);
    assert_eq!(d.paragraphs.len(), apex_doc::MAX_PARAGRAPHS);
    let md = "- a\n".repeat(apex_doc::MAX_PARAGRAPHS + 10);
    let apex_io::Loaded::Doc(d) = apex_io::text_contents(md.as_bytes(), apex_io::Format::Markdown, apex_io::Encoding::Utf8, None).unwrap() else { panic!() };
    assert!(d.truncated);
    assert_eq!(d.paragraphs.len(), apex_doc::MAX_PARAGRAPHS);
    assert!(!Document::from_plain_text("a\nb").truncated);
}

#[cfg(unix)]
#[test]
fn saving_keeps_permissions_and_links() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("apex-atomic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let real = dir.join("real.txt");
    std::fs::write(&real, b"old").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    apex_io::write_atomic(&link, b"new").unwrap();
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&real).unwrap(), b"new");
    assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
    // No temporary file is left behind.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}
