//! Password-protected files of other programs, all with the password
//! "secreto": .xls and .doc encrypted with RC4 by LibreOffice (the .xls
//! also with sheet and workbook protection and "read-only recommended"),
//! with RC4 CryptoAPI and XOR obfuscation (made from LibreOffice files and
//! checked with msoffcrypto-tool), and .odt/.ods encrypted by LibreOffice
//! 26.2 as ODF 1.4 (AES-GCM, Argon2id), ODF 1.2 (AES-CBC) and ODF 1.1
//! (Blowfish). Also .odt/.ods saved here with a password.

use std::path::PathBuf;

use apex_doc::{Document, Paragraph};
use apex_io::{Format, Loaded};
use apex_sheet::{CellRef, Password, Protection, Workbook};

const PW: &str = "secreto";

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/passwords").join(name)
}

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// Asserts the file asks for its password and refuses a wrong one.
fn locked(path: &PathBuf) {
    assert!(apex_io::is_encrypted(&std::fs::read(path).unwrap()), "{}", path.display());
    assert!(apex_io::open(path).err().unwrap().needs_password(), "{}", path.display());
    assert!(apex_io::open_with(path, Some("Secreto")).err().unwrap().wrong_password(), "{}", path.display());
}

fn book(path: &PathBuf, pw: &str) -> Workbook {
    match apex_io::open_with(path, Some(pw)) {
        Ok((Loaded::Sheet(wb), _)) => *wb,
        Ok(_) => panic!("not a workbook"),
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

fn doc(path: &PathBuf, pw: &str) -> Document {
    match apex_io::open_with(path, Some(pw)) {
        Ok((Loaded::Doc(d), _)) => *d,
        Ok(_) => panic!("not a document"),
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

fn cell(wb: &Workbook, a1: &str) -> String {
    wb.display(0, CellRef::parse(a1).unwrap()).0
}

#[test]
fn xls() {
    for name in ["lo-rc4.xls", "cryptoapi.xls", "cryptoapi-40.xls", "xor.xls"] {
        let p = data(name);
        locked(&p);
        let wb = book(&p, PW);
        assert_eq!((cell(&wb, "A1").as_str(), cell(&wb, "B3").as_str()), ("Nombre", "2"), "{name}");
    }
    // LibreOffice's protection of the sheet and the structure (password
    // "abc"), and its "open read-only" setting.
    let wb = book(&data("lo-rc4.xls"), PW);
    let sheet = wb.sheets[0].protection.as_ref().unwrap();
    assert_eq!(sheet.passwords, vec![Password::Legacy(0xCC1A)]);
    assert!(sheet.unlocks("abc") && !sheet.unlocks("abd"));
    assert_eq!(sheet.allow, apex_sheet::protect::allow::DEFAULT);
    assert!(wb.protection.as_ref().is_some_and(|p: &Protection| p.unlocks("abc") && !p.unlocks("x")));
    assert!(wb.meta.read_only_recommended);
    assert!(!book(&data("xor.xls"), PW).meta.read_only_recommended);
}

#[test]
fn doc_files() {
    for name in ["lo-rc4.doc", "cryptoapi.doc"] {
        let p = data(name);
        locked(&p);
        let d = doc(&p, PW);
        assert_eq!(d.paragraphs[0].text, "Hola mundo secreto", "{name}");
        assert_eq!(d.paragraphs[1].text, "Segunda línea ñ", "{name}");
    }
}

#[test]
fn odf_from_libreoffice() {
    for name in ["lo-aes-gcm.odt", "lo-blowfish.odt"] {
        let p = data(name);
        locked(&p);
        let d = doc(&p, PW);
        assert_eq!(d.paragraphs[0].text, "Hola mundo secreto", "{name}");
    }
    let p = data("lo-aes-cbc.ods");
    locked(&p);
    let wb = book(&p, PW);
    assert_eq!((cell(&wb, "A2").as_str(), cell(&wb, "B2").as_str()), ("uno", "1"));
}

#[test]
fn odf_saved_with_password() {
    let mut wb = Workbook::empty();
    wb.sheets.push(apex_sheet::Sheet::new("Hoja1"));
    wb.set_input(0, CellRef::new(0, 0), "Clave ñ");
    wb.set_input(0, CellRef::new(1, 0), "=2*21");
    wb.sheets[0].protection = Some(Protection::with_password("abc", apex_sheet::protect::allow::DEFAULT));
    let p = temp("pw.ods");
    apex_io::save_sheet_with(&wb, &p, Format::Ods, Some("Ábrete")).unwrap();
    locked(&p);
    let back = book(&p, "Ábrete");
    assert_eq!((cell(&back, "A1").as_str(), cell(&back, "A2").as_str()), ("Clave ñ", "42"));
    // The sheet's own protection hash stays.
    assert!(back.sheets[0].protection.as_ref().is_some_and(|s| s.unlocks("abc") && !s.unlocks("abd")));
    // Saved again without it, the file is plain.
    apex_io::save_sheet_with(&back, &p, Format::Ods, None).unwrap();
    assert!(!apex_io::is_encrypted(&std::fs::read(&p).unwrap()));

    let d = Document::from_paragraphs(vec![Paragraph::new("Texto secreto"), Paragraph::new("Segunda")]);
    let p = temp("pw.odt");
    apex_io::save_doc_with(&d, &p, Format::Odt, Some("Ábrete")).unwrap();
    locked(&p);
    let back = doc(&p, "Ábrete");
    assert_eq!(back.paragraphs[0].text, "Texto secreto");
    assert_eq!(back.paragraphs[1].text, "Segunda");
}

#[test]
fn read_only_recommended() {
    let mut wb = Workbook::empty();
    wb.sheets.push(apex_sheet::Sheet::new("Hoja1"));
    wb.meta.read_only_recommended = true;
    let p = temp("ro.xlsx");
    apex_io::save_sheet(&wb, &p, Format::Xlsx).unwrap();
    assert!(book(&p, "").meta.read_only_recommended);
    let mut d = Document::from_paragraphs(vec![Paragraph::new("x")]);
    d.meta.read_only_recommended = true;
    let p = temp("ro.docx");
    apex_io::save_doc(&d, &p, Format::Docx).unwrap();
    assert!(doc(&p, "").meta.read_only_recommended);
    d.meta.read_only_recommended = false;
    apex_io::save_doc(&d, &p, Format::Docx).unwrap();
    assert!(!doc(&p, "").meta.read_only_recommended);
}
