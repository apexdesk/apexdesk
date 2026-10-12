//! Encrypted files and Restrict Editing through the file functions.

use apex_doc::{Document, EditLimit, Paragraph, Protection, ProtectionHash};

use crate::{Format, Loaded};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("apex-protect-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

#[test]
fn encrypted_files() {
    let doc = Document::from_paragraphs(vec![Paragraph::new("Secreto")]);
    let p = temp("a.docx");
    crate::save_doc_with(&doc, &p, Format::Docx, Some("clave")).unwrap();
    assert!(crate::crypto::is_encrypted(&std::fs::read(&p).unwrap()));
    assert!(crate::open(&p).err().unwrap().needs_password());
    assert!(crate::open_with(&p, Some("otra")).err().unwrap().wrong_password());
    let Ok((Loaded::Doc(back), Format::Docx)) = crate::open_with(&p, Some("clave")) else { panic!("docx") };
    assert_eq!(back.paragraphs[0].text, "Secreto");

    let mut wb = apex_sheet::Workbook::empty();
    wb.sheets.push(apex_sheet::Sheet::new("Hoja1"));
    wb.set_input(0, apex_sheet::CellRef::new(0, 0), "42");
    let p = temp("b.xlsx");
    crate::save_sheet_with(&wb, &p, Format::Xlsx, Some("clave")).unwrap();
    let Ok((Loaded::Sheet(back), Format::Xlsx)) = crate::open_with(&p, Some("clave")) else { panic!("xlsx") };
    assert_eq!(back.display(0, apex_sheet::CellRef::new(0, 0)).0, "42");
    // So are .odt files; formats without encryption are saved as they are.
    let p = temp("c.odt");
    crate::save_doc_with(&doc, &p, Format::Odt, Some("clave")).unwrap();
    assert!(crate::open(&p).err().unwrap().needs_password());
    let p = temp("c.rtf");
    crate::save_doc_with(&doc, &p, Format::Rtf, Some("clave")).unwrap();
    assert!(crate::open(&p).is_ok());
}

#[test]
fn restrict_editing() {
    let mut doc = Document::from_paragraphs(vec![Paragraph::new("Texto")]);
    let hash = ProtectionHash { algorithm: "SHA-512".into(), hash: "aGFzaA==".into(), salt: "c2FsdA==".into(), spin: 100000, legacy: Vec::new() };
    doc.protection = Some(Protection { limit: EditLimit::Comments, enforced: true, password: Some(hash) });
    let back = crate::docx::read(&crate::docx::write(&doc)).unwrap();
    assert_eq!(back.protection, doc.protection);
    assert_eq!(back.edit_limit(), Some(EditLimit::Comments));
    // Word's older hashes are kept as they are.
    let legacy = vec![("w:cryptProviderType".to_string(), "rsaAES".to_string()), ("w:hash".to_string(), "abc=".to_string())];
    doc.protection = Some(Protection { limit: EditLimit::ReadOnly, enforced: false, password: Some(ProtectionHash { algorithm: String::new(), hash: String::new(), salt: String::new(), spin: 0, legacy }) });
    let back = crate::docx::read(&crate::docx::write(&doc)).unwrap();
    assert_eq!(back.protection, doc.protection);
    assert_eq!(back.edit_limit(), None);
}
