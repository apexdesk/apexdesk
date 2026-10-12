//! East Asian typography through docx, odt and rtf: Asian fonts and
//! languages, line breaking settings, the document grid, and what is kept
//! without being shown (phonetic guides, emphasis marks, combined
//! characters, fitted text, vertical text).

use apex_doc::{Document, Emphasis, FontAlign, GridKind, TextFlow};

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn docx_with(body: &str, styles: &str, settings: &str) -> Vec<u8> {
    let mut z = apex_io::zip::ZipWriter::new();
    let mut add = |name: &str, s: &str| z.add(name, s.as_bytes(), true);
    add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/></Types>"#);
    add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#);
    add("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/></Relationships>"#);
    add("word/document.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}</w:body></w:document>"#));
    add("word/styles.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W}>{styles}</w:styles>"#));
    add("word/settings.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:settings {W}>{settings}</w:settings>"#));
    z.finish()
}

/// A Japanese document as Word writes it.
fn word_doc() -> Vec<u8> {
    let styles = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Century" w:eastAsia="ＭＳ 明朝" w:hAnsi="Century" w:cs="Times New Roman"/><w:kern w:val="2"/><w:sz w:val="21"/><w:szCs w:val="22"/><w:lang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:autoSpaceDE w:val="0"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/><w:pPr><w:widowControl w:val="0"/><w:jc w:val="both"/></w:pPr></w:style>
<w:style w:type="paragraph" w:customStyle="1" w:styleId="Kanji"><w:name w:val="Kanji"/><w:basedOn w:val="a"/><w:pPr><w:snapToGrid w:val="0"/><w:overflowPunct w:val="0"/></w:pPr><w:rPr><w:rFonts w:eastAsia="ＭＳ ゴシック"/></w:rPr></w:style>"#;
    let settings = r#"<w:zoom w:percent="100"/><w:defaultTabStop w:val="840"/><w:characterSpacingControl w:val="compressPunctuation"/><w:noLineBreaksAfter w:lang="ja-JP" w:val="$([\{£¥‘“〈《「『【〔＄（［｛｢￡￥"/><w:noLineBreaksBefore w:lang="ja-JP" w:val="!%),.:;?]}¢°’”‰′″℃、。々〉》」』】〕゛゜ゝゞ・ヽヾ！％），．：；？］｝｡｣､･ﾞﾟ￠"/><w:compat><w:spaceForUL/></w:compat>"#;
    let body = r#"<w:p><w:pPr><w:kinsoku w:val="0"/><w:wordWrap w:val="0"/><w:overflowPunct w:val="0"/><w:topLinePunct/><w:autoSpaceDN w:val="0"/><w:textAlignment w:val="center"/></w:pPr>
<w:r><w:rPr><w:rFonts w:ascii="Arial" w:eastAsia="MS Gothic" w:hAnsi="Arial" w:hint="eastAsia"/><w:em w:val="dot"/><w:lang w:eastAsia="zh-CN"/></w:rPr><w:t>強調</w:t></w:r>
<w:r><w:rPr><w:eastAsianLayout w:id="1" w:combine="1" w:combineBrackets="round"/></w:rPr><w:t>二行</w:t></w:r>
<w:r><w:rPr><w:fitText w:val="1680" w:id="7"/></w:rPr><w:t>均等割付</w:t></w:r>
<w:r><w:ruby><w:rubyPr><w:rubyAlign w:val="distributeSpace"/><w:hps w:val="10"/><w:hpsRaise w:val="18"/><w:hpsBaseText w:val="21"/><w:lid w:val="ja-JP"/></w:rubyPr><w:rt><w:r><w:rPr><w:rFonts w:ascii="ＭＳ 明朝" w:eastAsia="ＭＳ 明朝" w:hAnsi="ＭＳ 明朝"/><w:sz w:val="10"/></w:rPr><w:t>かん</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>漢</w:t></w:r></w:rubyBase></w:ruby></w:r>
<w:r><w:ruby><w:rubyPr><w:rubyAlign w:val="distributeSpace"/><w:hps w:val="10"/><w:hpsRaise w:val="18"/><w:hpsBaseText w:val="21"/><w:lid w:val="ja-JP"/></w:rubyPr><w:rt><w:r><w:t>じ</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>字</w:t></w:r></w:rubyBase></w:ruby></w:r>
<w:r><w:rPr><w:rFonts w:cs="Arial Unicode MS"/><w:szCs w:val="28"/><w:lang w:bidi="he-IL"/></w:rPr><w:t>end</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Kanji"/></w:pPr><w:r><w:t>見出し</w:t></w:r></w:p>
<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/><w:textDirection w:val="tbRlV"/></w:tcPr><w:p><w:r><w:t>縦書き</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/><w:textDirection w:val="tbRl"/></w:tcPr><w:p><w:r><w:t>縦</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t>終わり</w:t></w:r></w:p>
<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1985" w:right="1701" w:bottom="1701" w:left="1701" w:header="851" w:footer="992" w:gutter="0"/><w:cols w:space="425"/><w:textDirection w:val="tbRl"/><w:docGrid w:type="linesAndChars" w:linePitch="360" w:charSpace="4096"/></w:sectPr>"#;
    docx_with(body, styles, settings)
}

fn check_word(d: &Document, docx: bool) {
    // Defaults: the Asian font and language, no spacing between Asian and Latin letters.
    let def = d.styles.asian.text.as_deref().expect("asian defaults");
    assert_eq!(def.font.as_deref(), Some("ＭＳ 明朝"));
    assert_eq!(def.lang.as_deref(), Some("ja-JP"));
    assert_eq!(d.styles.asian.para.auto_space_de, Some(false));
    // The paragraph's line breaking.
    let p = &d.paragraphs[0];
    let a = p.style.asian;
    assert_eq!(a.kinsoku, Some(false));
    assert_eq!(a.overflow_punct, Some(false));
    assert_eq!(a.auto_space_dn, Some(false));
    assert_eq!(a.font_align, Some(FontAlign::Center));
    if docx {
        assert_eq!(a.word_wrap, Some(false));
        assert_eq!(a.top_line_punct, Some(true));
    }
    assert_eq!(p.text, "強調二行均等割付漢字end");
    let at = |i: usize| p.attrs_at(p.text.char_indices().nth(i).map(|(o, c)| o + c.len_utf8()).unwrap()).asian();
    let first = at(0);
    assert_eq!(first.font.as_deref(), Some("MS Gothic"));
    assert_eq!(first.lang.as_deref(), Some("zh-CN"));
    assert_eq!(first.emphasis, Some(Emphasis::Dot));
    if docx {
        assert!(first.hint);
    }
    let comb = at(2).combine.expect("combined");
    assert!(comb.lines);
    assert_eq!(comb.brackets.as_deref(), Some("round"));
    if docx {
        assert_eq!(at(4).fit_text.map(|f| f.0), Some(1680));
    }
    // Two guides, one per character.
    let (r1, r2) = (at(8).ruby.expect("ruby 1"), at(9).ruby.expect("ruby 2"));
    assert_eq!((&*r1.text, &*r2.text), ("かん", "じ"));
    assert_ne!(r1, r2);
    assert!(at(10).ruby.is_none());
    let end = at(10);
    assert_eq!(end.cs_font.as_deref(), Some("Arial Unicode MS"));
    assert_eq!(end.cs_size, Some(28));
    assert_eq!(end.cs_lang.as_deref(), Some("he-IL"));
    // A style with an Asian font of its own.
    let k = d.styles.custom(d.paragraphs[1].style.custom).expect("custom style");
    assert_eq!(k.asian.as_deref().and_then(|a| a.font.as_deref()), Some("ＭＳ ゴシック"));
    assert_eq!(k.asian_para.snap_to_grid, Some(false));
    assert_eq!(k.asian_para.overflow_punct, Some(false));
    // Vertical cells.
    let t = &d.tables()[0];
    let c0 = t.cell_props(&d.paragraphs, 0, 0).unwrap();
    let c1 = t.cell_props(&d.paragraphs, 0, 1).unwrap();
    // (ODF has one vertical writing mode for both.)
    if docx {
        assert_eq!(apex_doc::TextFlow::of_cell(c0), Some(TextFlow::TbRlV));
    }
    assert_eq!(c0.text_dir, apex_doc::TextDir::Down);
    assert_eq!(c1.text_dir, apex_doc::TextDir::Down);
    // The grid and the vertical section.
    assert_eq!(d.page.grid.kind, GridKind::LinesAndChars);
    assert!((d.page.grid.line_pitch - 18.0).abs() < 0.01, "{}", d.page.grid.line_pitch);
    assert_eq!(d.page.grid.char_space, 4096);
    assert_eq!(d.page.text_flow, Some(TextFlow::TbRl));
}

#[test]
fn docx_round_trip() {
    let d = apex_io::docx::read(&word_doc()).unwrap();
    check_word(&d, true);
    let bytes = apex_io::docx::write(&d);
    let back = apex_io::docx::read(&bytes).unwrap();
    check_word(&back, true);
    // What Word needs to see.
    let z = apex_io::zip::ZipReader::new(&bytes).unwrap();
    let doc = z.read_string("word/document.xml").unwrap();
    for s in [r#"w:eastAsia="MS Gothic""#, r#"w:hint="eastAsia""#, r#"<w:em w:val="dot"/>"#, r#"w:combineBrackets="round""#, r#"<w:fitText w:val="1680""#, "<w:rt>", "かん", "<w:rubyBase>", r#"<w:kinsoku w:val="0"/>"#, r#"<w:textAlignment w:val="center"/>"#, r#"<w:textDirection w:val="tbRlV"/>"#, r#"<w:docGrid w:type="linesAndChars" w:linePitch="360" w:charSpace="4096"/>"#] {
        assert!(doc.contains(s), "{s} missing");
    }
    let settings = z.read_string("word/settings.xml").unwrap();
    assert!(settings.contains(r#"<w:characterSpacingControl w:val="compressPunctuation"/>"#));
    assert!(settings.contains("noLineBreaksAfter"));
    let styles = z.read_string("word/styles.xml").unwrap();
    assert!(styles.contains(r#"w:eastAsia="ＭＳ 明朝""#));
    // Saved twice: the same.
    let again = apex_io::docx::write(&back);
    let z2 = apex_io::zip::ZipReader::new(&again).unwrap();
    for part in ["word/document.xml", "word/styles.xml", "word/settings.xml"] {
        assert_eq!(z2.read_string(part).unwrap(), z.read_string(part).unwrap(), "{part}");
    }
}

#[test]
fn odt_round_trip() {
    let d = apex_io::docx::read(&word_doc()).unwrap();
    let odt = apex_io::odt::write(&d);
    let back = apex_io::odt::read(&odt).unwrap();
    check_word(&back, false);
    let z = apex_io::zip::ZipReader::new(&odt).unwrap();
    let styles = z.read_string("styles.xml").unwrap();
    assert!(styles.contains(r#"style:layout-grid-mode="both""#));
    assert!(styles.contains(r#"style:writing-mode="tb-rl""#));
    let content = z.read_string("content.xml").unwrap();
    assert!(content.contains("<text:ruby "));
    assert!(content.contains(r#"style:text-emphasize="dot above""#));
    // And back to docx.
    let again = apex_io::docx::read(&apex_io::docx::write(&back)).unwrap();
    check_word(&again, false);
}

/// As LibreOffice writes them.
#[test]
fn reads_libreoffice() {
    let styles = r##"<?xml version="1.0" encoding="UTF-8"?><office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3"><office:styles><style:default-style style:family="paragraph"><style:paragraph-properties style:text-autospace="ideograph-alpha" style:punctuation-wrap="hanging" style:line-break="strict" style:writing-mode="page"/><style:text-properties style:font-name="Liberation Serif" fo:font-size="12pt" fo:language="en" fo:country="US" style:font-name-asian="Noto Serif CJK JP" style:font-size-asian="10.5pt" style:language-asian="ja" style:country-asian="JP" style:font-name-complex="Lohit Devanagari" style:language-complex="hi" style:country-complex="IN"/></style:default-style><style:style style:name="Standard" style:family="paragraph" style:class="text"/></office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21.001cm" fo:page-height="29.7cm" style:writing-mode="tb-rl" style:layout-grid-color="#c0c0c0" style:layout-grid-lines="20" style:layout-grid-base-height="0.706cm" style:layout-grid-ruby-height="0cm" style:layout-grid-mode="line" style:layout-grid-ruby-below="false" style:layout-grid-print="false" style:layout-grid-display="true" style:layout-grid-base-width="0.37cm" style:layout-grid-snap-to="true" fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2cm" fo:margin-right="2cm"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"/></office:master-styles></office:document-styles>"##;
    let content = r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3"><office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties style:line-break="normal" style:snap-to-layout-grid="false"/></style:style><style:style style:name="T1" style:family="text"><style:text-properties style:text-emphasize="circle above" style:text-combine="lines" style:text-combine-start-char="[" style:text-combine-end-char="]"/></style:style><style:style style:name="Ru1" style:family="ruby"><style:ruby-properties style:ruby-position="below" style:ruby-align="center"/></style:style></office:automatic-styles><office:body><office:text><text:p text:style-name="P1">前<text:span text:style-name="T1">二行</text:span><text:ruby text:style-name="Ru1"><text:ruby-base>東京</text:ruby-base><text:ruby-text>とうきょう</text:ruby-text></text:ruby>後</text:p></office:text></office:body></office:document-content>"#;
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    let d = apex_io::odt::read(&z.finish()).unwrap();
    let check = |d: &Document| {
        let def = d.styles.asian.text.as_deref().expect("defaults");
        assert_eq!(def.font.as_deref(), Some("Noto Serif CJK JP"));
        assert_eq!(def.lang.as_deref(), Some("ja-JP"));
        assert_eq!(def.size, Some(21));
        assert_eq!(def.cs_font.as_deref(), Some("Lohit Devanagari"));
        assert_eq!(def.cs_lang.as_deref(), Some("hi-IN"));
        let dp = d.styles.asian.para;
        assert_eq!((dp.kinsoku, dp.overflow_punct, dp.auto_space_de), (Some(true), Some(true), Some(true)));
        let p = &d.paragraphs[0];
        assert_eq!(p.text, "前二行東京後");
        assert_eq!((p.style.asian.kinsoku, p.style.asian.snap_to_grid), (Some(false), Some(false)));
        let at = |i: usize| p.attrs_at(p.text.char_indices().nth(i).map(|(o, c)| o + c.len_utf8()).unwrap()).asian();
        assert_eq!(at(1).emphasis, Some(Emphasis::Circle));
        assert_eq!(at(1).combine.as_ref().and_then(|c| c.brackets.as_deref()), Some("square"));
        let r = at(3).ruby.expect("ruby");
        assert_eq!(&*r.text, "とうきょう");
        assert!(r.below);
        assert_eq!(at(4).ruby, Some(r));
        assert!(at(5).ruby.is_none());
        let g = d.page.grid;
        assert_eq!(g.kind, GridKind::Lines);
        assert!((g.line_pitch - 20.0).abs() < 0.1, "{}", g.line_pitch);
        assert!((g.char_pitch(d.normal_size_pt()) - 10.49).abs() < 0.05, "{}", g.char_pitch(d.normal_size_pt()));
        assert!(g.display && !g.print);
        assert_eq!(g.color, Some(0xC0C0C0));
        assert_eq!(d.page.text_flow, Some(TextFlow::TbRl));
    };
    check(&d);
    check(&apex_io::odt::read(&apex_io::odt::write(&d)).unwrap());
    // Through docx (the grid color and display are ODF's own).
    let x = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(x.paragraphs[0].text, "前二行東京後");
    assert_eq!(x.page.grid.kind, GridKind::Lines);
    assert_eq!(x.styles.asian.text.as_deref().and_then(|a| a.font.as_deref()), Some("Noto Serif CJK JP"));
}

#[test]
fn rtf_asian_fonts() {
    let rtf = r"{\rtf1\ansi\ansicpg1252\deff0\stshfdbch2{\fonttbl{\f0\froman Times New Roman;}{\f1\fnil Arial;}{\f2\fnil\fcharset128 MS Mincho;}{\f3\fnil\fcharset128 MS Gothic;}}\pard\plain\hich\af0\dbch\af3\loch\f1\langfe1041 abc☈5?\par\pard\plain\dbch\af2\loch plain\par}";
    let d = apex_io::rtf::read(rtf.as_bytes()).unwrap();
    assert_eq!(d.styles.asian.text.as_deref().and_then(|a| a.font.as_deref()), Some("MS Mincho"));
    let a = d.paragraphs[0].attrs_at(0);
    assert_eq!(a.asian_font().map(|f| &**f), Some("MS Gothic"));
    assert_eq!(a.asian().lang.as_deref(), Some("ja-JP"));
    // The default East Asian font is not repeated on each run.
    assert!(d.paragraphs[1].attrs_at(0).asian_font().is_none());
    let back = apex_io::rtf::read(&apex_io::rtf::write(&d)).unwrap();
    assert_eq!(back.paragraphs[0].attrs_at(0).asian_font().map(|f| &**f), Some("MS Gothic"));
    assert_eq!(back.paragraphs[0].attrs_at(0).asian().lang.as_deref(), Some("ja-JP"));
    assert_eq!(back.styles.asian.text.as_deref().and_then(|a| a.font.as_deref()), Some("MS Mincho"));
}

#[test]
fn vertical_text_box_and_theme_fonts() {
    use apex_doc::{DocShape, Paragraph, Pos, Selection, Shape, ShapeKind};
    let mut d = Document::from_paragraphs(vec![Paragraph::new("縦")]);
    let mut s = Shape::new(ShapeKind::TextBox);
    s.set_plain_text("縦書きの箱");
    s.vert = Some("eaVert".into());
    d.insert_shape(Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 0) }, DocShape::new(s, 40.0, 120.0));
    let t = &mut d.styles.theme;
    t.fonts.scripts = vec![(true, "ea".into(), String::new().into()), (false, "ea".into(), String::new().into()), (false, "Jpan".into(), "游明朝".into()), (true, "Jpan".into(), "游ゴシック".into())];
    let vert = |d: &Document| d.shapes().iter().map(|(_, s)| s.shape.vert.as_deref().map(str::to_string)).collect::<Vec<_>>();
    let x = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(vert(&x), [Some("eaVert".to_string())]);
    assert_eq!(x.styles.theme.ea_font(apex_doc::theme::EA_MINOR_TOKEN, Some("ja-JP")).map(|f| &**f), Some("游明朝"));
    assert_eq!(x.styles.theme.ea_font(apex_doc::theme::EA_MAJOR_TOKEN, None).map(|f| &**f), Some("游ゴシック"));
    let o = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(vert(&o), [Some("eaVert".to_string())]);
}

/// Guides, marks and combined characters set in the editor.
fn edited_doc() -> Document {
    use apex_doc::{Combine, Paragraph, Pos, Ruby, Selection};
    let mut d = Document::default();
    d.paragraphs = vec![Paragraph::new("漢字と東京大阪、重要な氏名")];
    let sel = |a: usize, b: usize| Selection { anchor: Pos::new(0, a * 3), cursor: Pos::new(0, b * 3) };
    d.set_ruby(sel(0, 2), Some(Ruby { text: "かんじ".into(), align: Some("distributeLetter".into()), size: Some(9), raise: Some(21), base_size: Some(21), lang: Some("ja-JP".into()), ..Default::default() }));
    d.format_range(sel(3, 7), &|a| a.set_asian(|x| x.combine = Some(Combine { id: Some(1), lines: true, brackets: Some("square".into()), ..Default::default() })));
    d.format_range(sel(8, 11), &|a| a.set_asian(|x| x.emphasis = Some(Emphasis::Circle)));
    d.format_range(sel(11, 13), &|a| a.set_asian(|x| x.fit_text = Some((1260, 2))));
    d
}

fn check_edited(d: &Document, docx: bool) {
    let p = &d.paragraphs[0];
    assert_eq!(p.text, "漢字と東京大阪、重要な氏名");
    let at = |i: usize| p.attrs_at(i * 3 + 3).asian();
    let r = at(0).ruby.expect("ruby");
    assert_eq!(at(1).ruby.as_ref(), Some(&r));
    assert!(at(2).ruby.is_none());
    assert_eq!(&*r.text, "かんじ");
    assert_eq!(r.align.as_deref(), Some("distributeLetter"));
    assert_eq!(r.size, Some(9));
    if docx {
        assert_eq!(r.raise, Some(21));
    }
    let c = at(4).combine.expect("combined");
    assert!(c.lines);
    assert!(c.brackets.as_deref().is_some_and(|b| b == "square" || b.starts_with('[')), "{:?}", c.brackets);
    assert!(at(7).combine.is_none());
    assert_eq!(at(9).emphasis, Some(Emphasis::Circle));
    assert_eq!(at(7).emphasis, None);
    if docx {
        assert_eq!(at(12).fit_text, Some((1260, 2)));
    }
}

#[test]
fn edited_layout_round_trips() {
    let d = edited_doc();
    check_edited(&d, true);
    let docx = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    check_edited(&docx, true);
    let odt = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    check_edited(&odt, false);
    // ODF keeps the guide's size in a text style.
    let bytes = apex_io::odt::write(&d);
    let z = apex_io::zip::ZipReader::new(&bytes).unwrap();
    let content = z.read_string("content.xml").unwrap();
    assert!(content.contains(r#"fo:font-size="4.5pt""#), "{content}");
    let again = apex_io::odt::read(&apex_io::odt::write(&odt)).unwrap();
    check_edited(&again, false);
}
