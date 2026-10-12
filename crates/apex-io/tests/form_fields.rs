//! Legacy form fields (text, check box, drop-down) in .docx, .odt and
//! .doc.

use std::sync::Arc;

use apex_doc::{Attrs, CHECKED, ContentControl, ControlKind, Document, FORM_BLANK, FormField, FormTextType, Paragraph, UNCHECKED, form_items};

fn sample() -> Document {
    let mut p = Paragraph::new("");
    let mut push = |text: &str, c: Option<ContentControl>, bold: bool| p.push(text, Attrs { control: c.map(Arc::new), bold, ..Default::default() });
    let name = FormField { name: "Nombre".into(), default_text: "Juan".into(), max_len: 10, format: "UPPERCASE".into(), help: "Ayuda".into(), status: "Estado".into(), ..Default::default() };
    push("Name: ", None, false);
    push("ANA", Some(ContentControl::form(1, ControlKind::PlainText, name)), true);
    push(" Age: ", None, false);
    let age = FormField { name: "Edad".into(), text_type: FormTextType::Number, max_len: 3, exit_macro: "Check".into(), tab_index: Some(2), ..Default::default() };
    push(FORM_BLANK, Some(ContentControl { placeholder: true, ..ContentControl::form(2, ControlKind::PlainText, age) }), false);
    push(" OK: ", None, false);
    let check = FormField { name: "Check1".into(), check_size: Some(20), ..Default::default() };
    push(&CHECKED.to_string(), Some(ContentControl::form(3, ControlKind::CheckBox { checked: true }, check)), false);
    push(" ", None, false);
    let off = FormField { name: "Check2".into(), default_checked: true, enabled: false, ..Default::default() };
    push(&UNCHECKED.to_string(), Some(ContentControl::form(4, ControlKind::CheckBox { checked: false }, off)), false);
    push(" Color: ", None, false);
    let color = FormField { name: "Color".into(), default_item: 2, ..Default::default() };
    push("Verde", Some(ContentControl::form(5, ControlKind::List { items: form_items(&["Rojo", "Verde", "Azul"]), editable: false }, color)), false);
    push(".", None, false);
    Document::from_paragraphs(vec![p])
}

/// The form fields as (text, control without id).
fn fields(d: &Document) -> Vec<(String, ContentControl)> {
    d.controls().into_iter().map(|c| (d.paragraphs[c.para].text[c.range].to_string(), ContentControl { id: 0, ..(*c.control).clone() })).collect()
}

#[test]
fn docx_round_trip() {
    let d = sample();
    let bytes = apex_io::docx::write(&d);
    let back = apex_io::docx::read(&bytes).unwrap();
    assert_eq!(back.paragraphs[0].text, d.paragraphs[0].text);
    assert_eq!(fields(&back), fields(&d));
    // The text field keeps its formatting; its bookmark comes along.
    let at = back.controls()[0].clone();
    assert!(back.paragraphs[0].attrs_at(at.range.start + 1).bold);
    assert!(back.bookmark("Nombre").is_some());
    // Saved again: the same.
    let again = apex_io::docx::read(&apex_io::docx::write(&back)).unwrap();
    assert_eq!(fields(&again), fields(&d));
    assert_eq!(again.bookmarks().len(), back.bookmarks().len());
}

fn docx_with(body: &str) -> Vec<u8> {
    let mut z = apex_io::zip::ZipWriter::new();
    let mut add = |name: &str, s: &str| z.add(name, s.as_bytes(), true);
    add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#);
    add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#);
    add("word/document.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#));
    z.finish()
}

/// As Word writes them: a bookmark around each field, check boxes and
/// drop-downs without a result, and LibreOffice's drop-down with one.
#[test]
fn reads_word_form_fields() {
    let body = r#"<w:p><w:r><w:t xml:space="preserve">Name: </w:t></w:r><w:bookmarkStart w:id="0" w:name="Text1"/><w:r><w:rPr><w:i/></w:rPr><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Text1"/><w:enabled/><w:calcOnExit w:val="0"/><w:textInput><w:maxLength w:val="4"/></w:textInput></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMTEXT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:rPr><w:noProof/></w:rPr><w:t>     </w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:bookmarkEnd w:id="0"/>
<w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox><w:sizeAuto/><w:default w:val="0"/><w:checked/></w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>
<w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Dropdown1"/><w:enabled/><w:ddList><w:result w:val="1"/><w:listEntry w:val="Rojo"/><w:listEntry w:val="Verde"/></w:ddList></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMDROPDOWN </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Rojo</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> end</w:t></w:r></w:p>"#;
    let d = apex_io::docx::read(&docx_with(&body.replace("     ", FORM_BLANK))).unwrap();
    assert_eq!(d.paragraphs[0].text, format!("Name: {FORM_BLANK}{CHECKED}Verde end"));
    let f = fields(&d);
    assert_eq!(f.len(), 3);
    assert!(f[0].1.placeholder && f[0].1.legacy.as_ref().unwrap().max_len == 4);
    assert_eq!(f[1].1.kind, ControlKind::CheckBox { checked: true });
    assert!(matches!(&f[2].1.kind, ControlKind::List { items, editable: false } if items.len() == 2));
    assert!(d.paragraphs[0].attrs_at(7).italic || d.paragraphs[0].attrs_at(7).no_proof);
    assert!(d.bookmark("Text1").is_some());
}

#[test]
fn odt_round_trip() {
    let d = sample();
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(back.paragraphs[0].text, d.paragraphs[0].text);
    assert_eq!(fields(&back), fields(&d));
    let at = back.controls()[0].clone();
    assert!(back.paragraphs[0].attrs_at(at.range.start + 1).bold);
    // Across formats: docx to odt and back.
    let docx = apex_io::docx::read(&apex_io::docx::write(&back)).unwrap();
    assert_eq!(fields(&docx), fields(&d));
}

/// As LibreOffice writes them (what it makes of Word's fields).
#[test]
fn reads_libreoffice_fieldmarks() {
    let body = r#"<text:p>Name: <text:bookmark-start text:name="Nombre Copy 1"/><field:fieldmark-start text:name="Nombre" field:type="vnd.oasis.opendocument.field.FORMTEXT"><field:param field:name="Content" field:value="Juan"/><field:param field:name="Format" field:value="UPPERCASE"/><field:param field:name="Help" field:value="Ayuda F1"/><field:param field:name="Hint" field:value="Estado"/></field:fieldmark-start><field:fieldmark-separator/><text:span text:style-name="T1">Ana</text:span><field:fieldmark-end/><text:bookmark-end text:name="Nombre Copy 1"/></text:p><text:p>Age: <field:fieldmark-start text:name="Edad" field:type="vnd.oasis.opendocument.field.FORMTEXT"><field:param field:name="Type" field:value="number"/></field:fieldmark-start><field:fieldmark-separator/>BLANK<field:fieldmark-end/></text:p><text:p>OK: <field:fieldmark text:name="Check1" field:type="vnd.oasis.opendocument.field.FORMCHECKBOX"><field:param field:name="Checkbox_Checked" field:value="true"/></field:fieldmark></text:p><text:p>Color: <field:fieldmark text:name="Dropdown1" field:type="vnd.oasis.opendocument.field.FORMDROPDOWN"><field:param field:name="Dropdown_ListEntry" field:value="Rojo"/><field:param field:name="Dropdown_ListEntry" field:value="Verde"/><field:param field:name="Dropdown_ListEntry" field:value="Azul"/><field:param field:name="Dropdown_Selected" field:value="1"/></field:fieldmark></text:p>"#.replace("BLANK", FORM_BLANK);
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:field="urn:openoffice:names:experimental:ooo-ms-interop:xmlns:field:1.0" office:version="1.3"><office:body><office:text>{body}</office:text></office:body></office:document-content>"#);
    let settings = r#"<?xml version="1.0" encoding="UTF-8"?><office:document-settings xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0"><office:settings><config:config-item-set config:name="ooo:configuration-settings"><config:config-item config:name="ProtectForm" config:type="boolean">true</config:config-item></config:config-item-set></office:settings></office:document-settings>"#;
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    z.add("content.xml", content.as_bytes(), true);
    z.add("settings.xml", settings.as_bytes(), true);
    let d = apex_io::odt::read(&z.finish()).unwrap();
    let texts: Vec<&str> = d.paragraphs.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["Name: Ana", &format!("Age: {FORM_BLANK}"), &format!("OK: {CHECKED}"), "Color: Verde"]);
    let f = fields(&d);
    assert_eq!(f.len(), 4);
    let name = f[0].1.legacy.as_ref().unwrap();
    assert_eq!((name.name.as_str(), name.default_text.as_str(), name.format.as_str(), name.help.as_str(), name.status.as_str()), ("Nombre", "Juan", "UPPERCASE", "Ayuda F1", "Estado"));
    assert!(f[1].1.placeholder && f[1].1.legacy.as_ref().unwrap().text_type == FormTextType::Number);
    assert_eq!(f[2].1.kind, ControlKind::CheckBox { checked: true });
    assert_eq!(d.edit_limit(), Some(apex_doc::EditLimit::Forms));
    // Saved: the protection stays.
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(back.edit_limit(), Some(apex_doc::EditLimit::Forms));
    assert_eq!(fields(&back), f);
}

fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// An `Xstz`: length, UTF-16 text and a terminating zero.
fn xstz(s: &str) -> Vec<u8> {
    let u: Vec<u16> = s.encode_utf16().collect();
    [u16s(&[u.len() as u16]), u16s(&u), u16s(&[0])].concat()
}

/// A form field's settings in the Data stream, after the 68-byte header.
fn ff_data(bits: u16, max_len: u16, name: &str, default_text: Option<&str>, def: Option<u16>, help: &str, entries: &[&str]) -> Vec<u8> {
    let mut d = [&[0xFF; 4][..], &u16s(&[bits, max_len, 0])].concat();
    d.extend(xstz(name));
    if let Some(t) = default_text {
        d.extend(xstz(t));
    }
    if let Some(w) = def {
        d.extend(u16s(&[w]));
    }
    for s in ["", help, "", "", ""] {
        d.extend(xstz(s));
    }
    if !entries.is_empty() {
        d.extend(u16s(&[0xFFFF, entries.len() as u16, 0]));
        for e in entries {
            let u: Vec<u16> = e.encode_utf16().collect();
            d.extend(u16s(&[u.len() as u16]));
            d.extend(u16s(&u));
        }
    }
    let mut out = ((d.len() + 68) as u32).to_le_bytes().to_vec();
    out.extend(u16s(&[0x44]));
    out.resize(68, 0);
    out.extend(d);
    out
}

/// A Word 97 file with a text field, a check box and a drop-down,
/// protected for filling in forms.
#[test]
fn reads_doc_form_fields() {
    let text = "Nombre: \x13 FORMTEXT \x01\x14Ana\x15 \x13 FORMCHECKBOX \x01\x14\x15 \x13 FORMDROPDOWN \x01\x14\x15.\r";
    let n = text.len() as u32;
    // The settings of each field, in the Data stream.
    let mut data = vec![0u8; 16];
    let mut at = Vec::new();
    for ff in [
        ff_data(0x180, 5, "Nombre", Some("Juan"), None, "Ayuda", &[]),
        ff_data(1 | 1 << 2, 0, "Check1", None, Some(0), "", &[]),
        ff_data(2 | 1 << 2, 0, "Color", None, Some(0), "", &["Rojo", "Verde", "Azul"]),
    ] {
        at.push(data.len() as u32);
        data.extend(ff);
    }
    let mut lw = [0u32; 22];
    lw[0] = 1024 + n;
    lw[3] = n;
    let mut fclcb = [(0u32, 0u32); 93];
    let mut table = Vec::new();
    let clx = [&[2][..], &16u32.to_le_bytes(), &0u32.to_le_bytes(), &n.to_le_bytes(), &u16s(&[0]), &(2048u32 | 0x4000_0000).to_le_bytes(), &u16s(&[0])].concat();
    fclcb[33] = (table.len() as u32, clx.len() as u32);
    table.extend(clx);
    // Protected for forms (fProtEnabled).
    fclcb[31] = (table.len() as u32, 8);
    table.extend([0, 0, 0, 0, 0, 0, 0, 2]);
    // Character runs: each field's picture character points to its data.
    let pics: Vec<u32> = text.match_indices('\x01').map(|(i, _)| 1024 + i as u32).collect();
    let mut runs: Vec<(u32, Vec<u8>)> = Vec::new();
    for (k, &fc) in pics.iter().enumerate() {
        runs.push((fc, Vec::new()));
        runs.push((fc + 1, [&[0x03, 0x6A][..], &at[k].to_le_bytes(), &[0x06, 0x08, 1, 0x55, 0x08, 1]].concat()));
    }
    runs.push((1024 + n, Vec::new()));
    let mut fkp = vec![0u8; 512];
    fkp[..4].copy_from_slice(&1024u32.to_le_bytes());
    let crun = runs.len();
    let mut off = 510;
    for (i, (end, g)) in runs.iter().enumerate() {
        fkp[(i + 1) * 4..(i + 2) * 4].copy_from_slice(&end.to_le_bytes());
        if !g.is_empty() {
            off = (off - g.len() - 1) & !1;
            fkp[off] = g.len() as u8;
            fkp[off + 1..off + 1 + g.len()].copy_from_slice(g);
            fkp[(crun + 1) * 4 + i] = (off / 2) as u8;
        }
    }
    fkp[511] = crun as u8;
    let bte = [&1024u32.to_le_bytes()[..], &(1024 + n).to_le_bytes(), &3u32.to_le_bytes()].concat();
    fclcb[12] = (table.len() as u32, bte.len() as u32);
    table.extend(bte);
    let mut wd = [u16s(&[0xA5EC, 0x00C1, 0, 0, 0, 0x0200]), vec![0; 20], u16s(&[14]), vec![0; 28], u16s(&[22])].concat();
    wd.extend(lw.iter().flat_map(|x| x.to_le_bytes()));
    wd.extend(u16s(&[93]));
    wd.extend(fclcb.iter().flat_map(|(a, b)| [a.to_le_bytes(), b.to_le_bytes()].concat()));
    wd.resize(1024, 0);
    wd.extend(text.bytes());
    wd.resize(1536, 0);
    wd.extend(fkp);
    let mut c = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
    for (name, d) in [("WordDocument", &wd), ("1Table", &table), ("Data", &data)] {
        std::io::Write::write_all(&mut c.create_stream(name).unwrap(), d).unwrap();
    }
    c.flush().unwrap();
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("forms.doc");
    std::fs::write(&path, c.into_inner().into_inner()).unwrap();
    let (loaded, _) = apex_io::open(&path).unwrap();
    let apex_io::Loaded::Doc(d) = loaded else { panic!("not a document") };
    assert_eq!(d.paragraphs[0].text, format!("Nombre: Ana {CHECKED} Verde."));
    let f = fields(&d);
    assert_eq!(f.len(), 3);
    let name = f[0].1.legacy.as_ref().unwrap();
    assert_eq!((f[0].0.as_str(), name.name.as_str(), name.default_text.as_str(), name.max_len, name.help.as_str()), ("Ana", "Nombre", "Juan", 5, "Ayuda"));
    assert_eq!(f[1].1.kind, ControlKind::CheckBox { checked: true });
    assert!(matches!(&f[2].1.kind, ControlKind::List { items, .. } if items.len() == 3));
    assert_eq!(d.edit_limit(), Some(apex_doc::EditLimit::Forms));
    // Saved as .docx, nothing is lost.
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(fields(&back), f);
}
