//! The document defaults (Word's docDefaults, the ODF default paragraph
//! style) and Normal survive opening and saving as the file had them, and
//! carry over between docx and odt; new documents get ours.

use apex_doc::{Attrs, Document, LineRule, ParaKind, ParaStyle, Paragraph};

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn docx_with(styles: &str) -> Vec<u8> {
    let mut z = apex_io::zip::ZipWriter::new();
    let mut add = |name: &str, s: &str| z.add(name, s.as_bytes(), true);
    add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#);
    add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#);
    add("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#);
    let body = r#"<w:p><w:r><w:t>本文 text</w:t></w:r></w:p><w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr></w:pPr><w:r><w:t>More</w:t></w:r></w:p>"#;
    add("word/document.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}</w:body></w:document>"#));
    add("word/styles.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W}>{styles}</w:styles>"#));
    z.finish()
}

fn part(bytes: &[u8], name: &str) -> String {
    apex_io::zip::ZipReader::new(bytes).unwrap().read_string(name).unwrap()
}

/// The first `<tag ...>...</tag>` (or empty `<tag .../>`) of `xml`.
fn element<'a>(xml: &'a str, tag: &str) -> &'a str {
    let i = xml.find(&format!("<{tag}")).unwrap_or_else(|| panic!("no {tag} in {xml}"));
    let rest = &xml[i..];
    let open_end = rest.find('>').unwrap();
    if rest[..open_end].ends_with('/') {
        return &rest[..=open_end];
    }
    let close = format!("</{tag}>");
    &rest[..rest.find(&close).unwrap() + close.len()]
}

/// The Normal style of styles.xml.
fn normal(xml: &str) -> &str {
    let i = xml.find(r#"w:styleId="Normal""#).unwrap();
    let start = xml[..i].rfind("<w:style ").unwrap();
    &xml[start..start + xml[start..].find("</w:style>").unwrap() + 10]
}

/// Japanese Word: no paragraph defaults, 10.5 pt, Normal with widow
/// control off and justified.
const WORD_JA: &str = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Century" w:eastAsia="ＭＳ 明朝" w:hAnsi="Century" w:cs="Times New Roman"/><w:kern w:val="2"/><w:sz w:val="21"/><w:szCs w:val="22"/><w:lang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault/></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/><w:qFormat/><w:pPr><w:widowControl w:val="0"/><w:jc w:val="both"/></w:pPr></w:style>"#;

/// Word 2016: 11 pt, 8 pt after and 1.08 lines in the defaults, theme fonts.
const WORD_EN: &str = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:eastAsiaTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi" w:cstheme="minorBidi"/><w:kern w:val="2"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/><w14:ligatures xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" w14:val="standardContextual"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>"#;

#[test]
fn word_defaults_kept() {
    for styles in [WORD_JA, WORD_EN] {
        let d = apex_io::docx::read(&docx_with(styles)).unwrap();
        let defaults = d.styles.defaults.as_ref().expect("defaults");
        let out = part(&apex_io::docx::write(&d), "word/styles.xml");
        // As they were, and twice.
        assert_eq!(element(&out, "w:docDefaults"), element(styles, "w:docDefaults"));
        let again = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
        assert_eq!(again.styles.defaults.as_ref(), Some(defaults));
        assert_eq!(element(&part(&apex_io::docx::write(&again), "word/styles.xml"), "w:docDefaults"), element(styles, "w:docDefaults"));
    }
    let d = apex_io::docx::read(&docx_with(WORD_JA)).unwrap();
    let defaults = d.styles.defaults.as_ref().unwrap();
    assert_eq!((defaults.size, defaults.cs_size, defaults.lang.as_deref()), (Some(21), Some(22), Some("en-US")));
    assert_eq!((defaults.space_after, defaults.line), (None, None));
    // Normal keeps its own settings, not the defaults' size.
    let n = normal(styles_of(&d));
    assert!(n.contains(r#"<w:widowControl w:val="0"/>"#) && n.contains(r#"<w:jc w:val="both"/>"#) && !n.contains("w:sz"), "{n}");
    // Text starts from the defaults' size, list paragraphs too.
    assert_eq!(d.char_format(&d.paragraphs[0].style, &Attrs::default()).size, 21);
    assert_eq!(d.char_format(&ParaStyle { kind: ParaKind::Bullet, ..Default::default() }, &Attrs::default()).size, 21);
    assert!(d.paragraphs[0].style.no_widow_control);
    let en = apex_io::docx::read(&docx_with(WORD_EN)).unwrap();
    let defaults = en.styles.defaults.as_ref().unwrap();
    assert_eq!((defaults.size, defaults.space_after, defaults.line), (Some(22), Some(8.0), Some((LineRule::Multiple, 259.0 / 240.0))));
}

fn styles_of(d: &Document) -> &'static str {
    Box::leak(part(&apex_io::docx::write(d), "word/styles.xml").into_boxed_str())
}

#[test]
fn changed_defaults_are_written_in_order() {
    let mut d = apex_io::docx::read(&docx_with(WORD_JA)).unwrap();
    d.default_font = Some("Yu Mincho".into());
    let out = styles_of(&d);
    let defaults = element(out, "w:docDefaults");
    // Ours from the values, with what we don't read kept in its place.
    let at = |s: &str| defaults.find(s).unwrap_or_else(|| panic!("{s} in {defaults}"));
    assert!(at(r#"w:ascii="Yu Mincho""#) < at(r#"<w:kern w:val="2"/>"#));
    assert!(at("<w:kern ") < at(r#"<w:sz w:val="21"/>"#) && at("<w:sz ") < at(r#"<w:szCs w:val="22"/>"#) && at("<w:szCs ") < at(r#"<w:lang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/>"#));
    assert!(!defaults.contains("w:spacing") && !defaults.contains("widowControl"), "{defaults}");
}

#[test]
fn new_documents_get_ours() {
    let d = Document::from_paragraphs(vec![Paragraph::new("x")]);
    assert!(d.styles.defaults.is_none());
    let docx = apex_io::docx::write(&d);
    let defaults = element(&part(&docx, "word/styles.xml"), "w:docDefaults").to_string();
    assert!(defaults.contains(r#"<w:sz w:val="24"/>"#) && defaults.contains(r#"w:after="160" w:line="240""#), "{defaults}");
    // Ours read back are still ours.
    assert!(apex_io::docx::read(&docx).unwrap().styles.defaults.is_none());
    let odt = apex_io::odt::write(&d);
    assert!(element(&part(&odt, "styles.xml"), "style:default-style").contains(r#"fo:font-size="12pt""#));
    let back = apex_io::odt::read(&odt).unwrap();
    assert!(back.styles.defaults.is_none() && back.default_font.is_none() && back.styles.builtin(ParaKind::Normal).is_none());
    assert!(part(&apex_io::odt::write(&back), "styles.xml").contains(r#"fo:margin-bottom="0.247cm" fo:line-height="115%""#));
}

/// An odt without a default paragraph style is in LibreOffice's font, in
/// docx too (not in our Calibri, wider or narrower pages after saving).
#[test]
fn odt_without_defaults_keeps_libreoffice_font() {
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#;
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns} office:version="1.2"><office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>"#);
    let styles = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns} office:version="1.2"><office:styles/></office:document-styles>"#);
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    let d = apex_io::odt::read(&z.finish()).unwrap();
    assert_eq!(d.default_font.as_deref(), Some("Liberation Serif"));
    let docx = apex_io::docx::write(&d);
    assert!(element(&part(&docx, "word/styles.xml"), "w:docDefaults").contains(r#"w:ascii="Liberation Serif""#));
    assert_eq!(apex_io::docx::read(&docx).unwrap().default_font.as_deref(), Some("Liberation Serif"));
}

/// As LibreOffice writes a Japanese document: fonts by declared names.
fn lo_odt() -> Vec<u8> {
    let styles = r##"<?xml version="1.0" encoding="UTF-8"?><office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" office:version="1.3"><office:font-face-decls><style:font-face style:name="MS Mincho" svg:font-family="'MS Mincho'" style:font-family-generic="roman"/><style:font-face style:name="MS Mincho1" svg:font-family="'MS Mincho'" style:font-family-generic="system"/></office:font-face-decls><office:styles><style:default-page-layout><style:page-layout-properties style:layout-grid-standard-mode="true"/></style:default-page-layout><style:default-style style:family="paragraph"><style:paragraph-properties style:text-autospace="ideograph-alpha" style:punctuation-wrap="hanging" style:line-break="strict" style:tab-stop-distance="1.251cm" style:writing-mode="page"/><style:text-properties style:use-window-font-color="true" style:font-name="MS Mincho" fo:font-size="10.5pt" fo:language="en" fo:country="US" style:letter-kerning="true" style:font-name-asian="MS Mincho1" style:font-size-asian="10.5pt" style:language-asian="ja" style:country-asian="JP" style:font-size-complex="10pt" fo:hyphenate="false"/></style:default-style><style:style style:name="Standard" style:family="paragraph" style:class="text"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0cm" fo:text-align="justify" fo:orphans="0" fo:widows="0"/></style:style></office:styles></office:document-styles>"##;
    let content = r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3"><office:body><office:text><text:p text:style-name="Standard">吾輩は猫である。</text:p></office:text></office:body></office:document-content>"#;
    let settings = r#"<?xml version="1.0" encoding="UTF-8"?><office:document-settings xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0" xmlns:ooo="http://openoffice.org/2004/office" office:version="1.3"><office:settings><config:config-item-set config:name="ooo:configuration-settings"><config:config-item config:name="MsWordCompGridMetrics" config:type="boolean">true</config:config-item></config:config-item-set></office:settings></office:document-settings>"#;
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    z.add("settings.xml", settings.as_bytes(), true);
    z.finish()
}

#[test]
fn libreoffice_defaults_kept() {
    let d = apex_io::odt::read(&lo_odt()).unwrap();
    let defaults = d.styles.defaults.as_ref().expect("defaults");
    assert_eq!((defaults.size, defaults.lang.as_deref()), (Some(21), Some("en-US")));
    assert_eq!(d.default_font.as_deref(), Some("MS Mincho"));
    assert_eq!(d.styles.asian.text.as_ref().and_then(|a| a.font.as_deref()), Some("MS Mincho"));
    assert_eq!(d.base_size(ParaKind::Normal), 21);
    // Normal paragraphs have Normal's spacing and widow control.
    assert_eq!((d.paragraphs[0].style.space_after, d.paragraphs[0].style.no_widow_control), (Some(0.0), true));
    let out = apex_io::odt::write(&d);
    let styles = part(&out, "styles.xml");
    let def = element(&styles, "style:default-style");
    for a in [r#"fo:font-size="10.5pt""#, r#"fo:font-family="'MS Mincho'""#, r#"style:font-family-asian="'MS Mincho'""#, r#"fo:language="en" fo:country="US""#, r#"style:font-size-asian="10.5pt""#, r#"style:font-size-complex="10pt""#, r#"style:tab-stop-distance="1.251cm""#, r#"style:letter-kerning="true""#, r#"style:writing-mode="page""#, r#"style:line-break="strict""#] {
        assert!(def.contains(a), "{a} in {def}");
    }
    assert!(!def.contains("fo:margin") && !def.contains("style:font-name="), "{def}");
    let std = &styles[styles.find(r#"style:name="Standard""#).unwrap()..];
    let std = &std[..std.find("</style:style>").unwrap()];
    assert!(std.contains(r#"fo:text-align="justify""#) && std.contains(r#"fo:widows="0""#) && std.contains(r#"fo:margin-bottom="0"#) && !std.contains("line-height"), "{std}");
    assert!(styles.contains(r#"<style:default-page-layout><style:page-layout-properties style:layout-grid-standard-mode="true"/></style:default-page-layout>"#));
    let settings = part(&out, "settings.xml");
    assert!(settings.contains(r#"xmlns:ooo="http://openoffice.org/2004/office""#) && settings.contains(r#"config:name="MsWordCompGridMetrics" config:type="boolean">true"#), "{settings}");
    // Read back: the same defaults.
    let back = apex_io::odt::read(&out).unwrap();
    let b = back.styles.defaults.as_ref().unwrap();
    assert_eq!((b.size, &b.lang, &back.default_font), (Some(21), &defaults.lang, &d.default_font));
    assert_eq!(back.styles.builtin(ParaKind::Normal), d.styles.builtin(ParaKind::Normal));
}

#[test]
fn defaults_carry_over_between_formats() {
    // odt to docx: the size, fonts and language in docDefaults, Normal as it was.
    let d = apex_io::odt::read(&lo_odt()).unwrap();
    let docx = apex_io::docx::write(&d);
    let styles = part(&docx, "word/styles.xml");
    let def = element(&styles, "w:docDefaults");
    for a in [r#"w:ascii="MS Mincho""#, r#"w:eastAsia="MS Mincho""#, r#"<w:sz w:val="21"/>"#, r#"<w:szCs w:val="20"/>"#, r#"<w:lang w:val="en-US" w:eastAsia="ja-JP"/>"#, "<w:kinsoku/>"] {
        assert!(def.contains(a), "{a} in {def}");
    }
    assert!(!def.contains("w:spacing"), "{def}");
    let n = normal(&styles);
    assert!(n.contains(r#"w:after="0""#) && n.contains(r#"<w:jc w:val="both"/>"#) && n.contains(r#"<w:widowControl w:val="0"/>"#), "{n}");
    let back = apex_io::docx::read(&docx).unwrap();
    assert_eq!(back.base_size(ParaKind::Normal), 21);
    // docx to odt: spacing and line spacing of the defaults in the default style.
    let en = apex_io::docx::read(&docx_with(WORD_EN)).unwrap();
    let odt = apex_io::odt::write(&en);
    let styles = part(&odt, "styles.xml");
    let def = element(&styles, "style:default-style");
    assert!(def.contains(r#"fo:margin-bottom="8pt""#) && def.contains(r#"fo:line-height="108%""#) && def.contains(r#"fo:font-size="11pt""#) && def.contains(r#"fo:language="en" fo:country="US""#), "{def}");
    // Standard takes all that from the defaults.
    assert!(styles.contains(r#"<style:style style:name="Standard" style:family="paragraph" style:class="text"></style:style>"#));
    // LibreOffice lays the grid of a document from Word out as Word does.
    assert!(part(&odt, "settings.xml").contains("MsWordCompGridMetrics"));
    let back = apex_io::odt::read(&odt).unwrap();
    let b = back.styles.defaults.as_ref().unwrap();
    assert_eq!((b.size, b.space_after, b.line), (Some(22), Some(8.0), Some((LineRule::Multiple, 1.08))));
}

/// A custom style named "Default" beside the real Normal (default="1")
/// doesn't stand for Normal: plain paragraphs get Normal's spacing every
/// time the file is read, whatever the hashing order.
#[test]
fn normal_is_the_default_style_not_one_named_default() {
    let styles = r#"<w:docDefaults><w:pPrDefault/></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:pPr><w:spacing w:after="200"/></w:pPr></w:style><w:style w:type="paragraph" w:customStyle="1" w:styleId="Default"><w:name w:val="Default"/><w:rPr><w:sz w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Other"><w:name w:val="Other"/><w:pPr><w:spacing w:after="0"/></w:pPr></w:style>"#;
    let file = docx_with(styles);
    for _ in 0..32 {
        let d = apex_io::docx::read(&file).unwrap();
        assert_eq!(d.paragraphs[0].style.space_after, Some(10.0));
    }
}

/// Likewise in odt: "Standard" is Normal even beside a style named
/// "Default".
#[test]
fn odt_normal_is_standard_every_time() {
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#;
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns} office:version="1.3"><office:styles><style:style style:name="Default" style:family="paragraph"><style:paragraph-properties fo:margin-bottom="0pt"/></style:style><style:style style:name="Standard" style:family="paragraph"><style:paragraph-properties fo:margin-bottom="12pt"/></style:style></office:styles></office:document-styles>"#
    );
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns} office:version="1.3"><office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>"#);
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    let file = z.finish();
    for _ in 0..32 {
        let d = apex_io::odt::read(&file).unwrap();
        assert_eq!(d.paragraphs[0].style.space_after, Some(12.0));
    }
}

#[test]
fn code_style_properties_in_schema_order() {
    // CT_PPrBase puts w:shd before w:spacing: Word's validator rejects the
    // other order (every docx we saved failed it before).
    let styles = part(&apex_io::docx::write(&Document::new()), "word/styles.xml");
    let i = styles.find(r#"w:styleId="Code""#).unwrap();
    let ppr = element(&styles[i..], "w:pPr");
    assert!(ppr.find("<w:shd").unwrap() < ppr.find("<w:spacing").unwrap(), "{ppr}");
}

/// A document without defaults of its own keeps its font through odt (the
/// default style we write names it; read back it is still the font).
#[test]
fn odt_keeps_the_font_of_documents_without_defaults() {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("x")]);
    d.default_font = Some("Times New Roman".into());
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(back.default_font.as_deref(), Some("Times New Roman"));
    assert!(back.styles.defaults.is_none());
}

/// Space the ODF default paragraph style gives paragraphs whose Normal
/// doesn't set it applies to them (as in docx), and survives saving in
/// docx and odt.
#[test]
fn odt_default_style_spacing_applies_to_normal_paragraphs() {
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0""#;
    let styles = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns} office:version="1.2"><office:styles><style:default-style style:family="paragraph"><style:paragraph-properties fo:margin-bottom="3.53mm" fo:line-height="115%"/><style:text-properties fo:font-size="11pt" style:font-name="Calibri"/></style:default-style><style:style style:name="Normal" style:display-name="Normal" style:family="paragraph"/></office:styles></office:document-styles>"#);
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns} office:version="1.2"><office:body><office:text><text:p text:style-name="Normal">a</text:p><text:p>b</text:p></office:text></office:body></office:document-content>"#);
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    let d = apex_io::odt::read(&z.finish()).unwrap();
    let after = |d: &Document| d.paragraphs.iter().map(|p| p.style.space_after.map(|a| (a * 10.0).round() / 10.0)).collect::<Vec<_>>();
    assert_eq!(after(&d), [Some(10.0), Some(10.0)]);
    assert_eq!(after(&apex_io::docx::read(&apex_io::docx::write(&d)).unwrap()), [Some(10.0), Some(10.0)]);
    assert_eq!(after(&apex_io::odt::read(&apex_io::odt::write(&d)).unwrap()), [Some(10.0), Some(10.0)]);
}

/// Cells whose line spacing is the document's keep it in docx: the table
/// styles we write set no line spacing of their own.
#[test]
fn docx_cells_keep_the_document_line_spacing() {
    let mut d = apex_io::docx::read(&docx_with(WORD_EN)).unwrap();
    d.insert_table(apex_doc::Selection::caret(apex_doc::Pos::default()), 2, 2, 300.0);
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    let cells: Vec<f32> = back.paragraphs.iter().filter(|p| p.cell.is_some()).map(|p| p.style.line_spacing).collect();
    assert_eq!(cells, [0.0; 4]);
    assert!((back.styles.default_line(&back.paragraphs[0].style) - 1.08).abs() < 0.01);
}

/// A line spacing of at least 0 (a line as tall as its text, used for
/// tiny spacer paragraphs) isn't the document's line spacing.
#[test]
fn line_spacing_at_least_zero_is_kept() {
    let mut d = apex_io::docx::read(&docx_with(WORD_EN)).unwrap();
    d.paragraphs[0].style.line_rule = LineRule::AtLeast;
    d.paragraphs[0].style.line_spacing = 0.0;
    for back in [apex_io::docx::read(&apex_io::docx::write(&d)).unwrap(), apex_io::odt::read(&apex_io::odt::write(&d)).unwrap()] {
        let st = &back.paragraphs[0].style;
        assert_eq!((st.line_rule, st.line_spacing), (LineRule::AtLeast, 0.0));
    }
}
