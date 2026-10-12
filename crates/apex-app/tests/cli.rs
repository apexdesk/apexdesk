//! The command line, run as a program on files the tests write.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use apex_io::{Format, Loaded};
use apex_sheet::{CellRef, Workbook};

/// An empty folder of its own for each test.
fn folder(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Runs `apexdesk args...` in `dir` with English number formats.
fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_apexdesk")).args(args).current_dir(dir).env("LC_ALL", "C").env("LANG", "C").env("APEXDESK_SCRIPT", "1").output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let o = run(dir, args);
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    stdout(&o)
}

/// A workbook with two sheets and a formula.
fn book(dir: &Path) {
    let mut wb = Workbook::new();
    wb.locale = apex_sheet::Locale::EN;
    let rows = [["Name", "City", "Amount"], ["Ana", "Lugo", "10.5"], ["Luis", "Vigo", "20"], ["Total", "", "=SUM(C2:C3)"]];
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            wb.set_input(0, CellRef::new(r as u32, c as u32), v);
        }
    }
    wb.sheets.push(apex_sheet::Sheet::new("Notes"));
    wb.set_input(1, CellRef::new(0, 0), "hello, world");
    apex_io::save_sheet(&wb, &dir.join("t_book.xlsx"), Format::Xlsx).unwrap();
}

fn letter(dir: &Path) {
    let doc = apex_io::markdown::read("# Letter\n\nDear **\u{ab}Name\u{bb}**, from \u{ab}City\u{bb}.\n\n| Item | Qty |\n|---|---|\n| Pears | \u{ab}Amount\u{bb} |\n");
    apex_io::save_doc(&doc, &dir.join("t_letter.docx"), Format::Docx).unwrap();
}

fn open_sheet(p: &Path) -> Workbook {
    match apex_io::open(p).unwrap().0 {
        Loaded::Sheet(wb) => *wb,
        _ => panic!("not a sheet"),
    }
}

fn open_doc(p: &Path) -> apex_doc::Document {
    match apex_io::open(p).unwrap().0 {
        Loaded::Doc(d) => *d,
        _ => panic!("not a document"),
    }
}

#[test]
fn convert_files() {
    let d = folder("convert");
    book(&d);
    letter(&d);
    // Wildcards are matched by the program too (no shell here).
    let out = ok(&d, &["convert", "t_*.xlsx", "t_letter.docx", "--to", "pdf", "--out", "pdf"]);
    assert!(out.contains("t_book.xlsx -> pdf") && out.contains("t_letter.docx -> pdf"), "{out}");
    for f in ["pdf/t_book.pdf", "pdf/t_letter.pdf"] {
        let bytes = std::fs::read(d.join(f)).unwrap();
        assert!(bytes.starts_with(b"%PDF") && bytes.len() > 1000, "{f}");
    }
    // Every sheet to CSV, one file each.
    ok(&d, &["convert", "t_book.xlsx", "--to", "csv", "--sheet", "all", "-q"]);
    assert_eq!(std::fs::read_to_string(d.join("t_book-Sheet1.csv")).unwrap().replace("\r\n", "\n"), "Name,City,Amount\nAna,Lugo,10.5\nLuis,Vigo,20\nTotal,,30.5\n");
    assert!(std::fs::read_to_string(d.join("t_book-Notes.csv")).unwrap().contains("\"hello, world\""));
    // Round trips keep the content.
    ok(&d, &["convert", "t_book.xlsx", "--to", "ods", "-q"]);
    let wb = open_sheet(&d.join("t_book.ods"));
    assert_eq!(wb.sheets.len(), 2);
    assert_eq!(wb.edit_text(0, CellRef::new(3, 2)), "=SUM(C2:C3)");
    ok(&d, &["convert", "t_letter.docx", "--to", "md", "-o", "letter.md"]);
    let md = std::fs::read_to_string(d.join("letter.md")).unwrap();
    assert!(md.contains("# Letter") && md.contains("| Pears |"), "{md}");
    // Errors: the input is never replaced; wrong usage exits with 2.
    let o = run(&d, &["convert", "t_book.xlsx", "--to", "xlsx"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("replace the input"));
    assert_eq!(run(&d, &["convert", "t_book.xlsx"]).status.code(), Some(2));
    assert_eq!(run(&d, &["convert", "t_book.xlsx", "--to", "zzz"]).status.code(), Some(2));
    assert_eq!(run(&d, &["convert", "missing.xlsx", "--to", "pdf"]).status.code(), Some(1));
    assert_eq!(run(&d, &["convert", "t_letter.docx", "--to", "xlsx"]).status.code(), Some(1));
}

#[test]
fn cat_cells_and_text() {
    let d = folder("cat");
    book(&d);
    letter(&d);
    let t = ok(&d, &["cat", "t_book.xlsx"]);
    assert!(t.contains("| Name  | City | Amount") && t.contains("4 | Total |      |   30.5"), "{t}");
    assert_eq!(ok(&d, &["cat", "t_book.xlsx", "-r", "A2:C3", "-f", "csv"]), "Ana,Lugo,10.5\nLuis,Vigo,20\n");
    assert_eq!(ok(&d, &["cat", "t_book.xlsx", "-r", "B2:C2", "-f", "json"]), "[\n  [\"Lugo\", 10.5]\n]\n");
    assert_eq!(ok(&d, &["cat", "t_book.xlsx", "-r", "C4", "--formulas", "-f", "tsv"]), "'=SUM(C2:C3)\n", "kept as text in other spreadsheets");
    assert_eq!(ok(&d, &["cat", "t_book.xlsx", "--sheet", "Notes", "-f", "tsv"]), "hello, world\n");
    assert_eq!(ok(&d, &["cat", "t_book.xlsx", "--sheet", "2", "-f", "md"]), "| hello, world |\n|---|\n");
    let text = ok(&d, &["cat", "t_letter.docx"]);
    assert!(text.starts_with("Letter\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n"), "{text}");
    assert!(text.contains("\u{2502} Pears \u{2502} \u{ab}Amount\u{bb} \u{2502}"), "{text}");
    assert!(ok(&d, &["cat", "t_letter.docx", "-f", "md"]).contains("**\u{ab}Name\u{bb}**"));
    assert_eq!(run(&d, &["cat", "t_book.xlsx", "--sheet", "9"]).status.code(), Some(1));
}

#[test]
fn eval_get_and_set() {
    let d = folder("eval");
    book(&d);
    assert_eq!(ok(&d, &["eval", "t_book.xlsx", "SUM(C2:C3)*2"]), "61\n");
    assert_eq!(ok(&d, &["eval", "t_book.xlsx", "=COUNTA(Notes!A:A)"]), "1\n");
    assert_eq!(ok(&d, &["eval", "=ROUND(2/3, 2)"]), "0.67\n");
    assert_eq!(ok(&d, &["eval", "SEQUENCE(2,2)"]), "1\t2\n3\t4\n");
    assert_eq!(ok(&d, &["eval", "1/0"]), "#DIV/0!\n");
    assert_eq!(run(&d, &["eval", "SUM(1,"]).status.code(), Some(1));
    assert_eq!(ok(&d, &["get", "t_book.xlsx", "C4", "Notes!A1"]), "30.5\nhello, world\n");
    // set saves the file, and formulas follow.
    ok(&d, &["set", "t_book.xlsx", "C2", "100", "D1", "=C4*2"]);
    assert_eq!(ok(&d, &["get", "t_book.xlsx", "C4", "D1"]), "120\n240\n");
    assert_eq!(ok(&d, &["get", "t_book.xlsx", "A1:B2"]), "Name\tCity\nAna\tLugo\n");
    // To another file, which keeps the second sheet.
    ok(&d, &["set", "t_book.xlsx", "A1", "Who", "-o", "copy.ods"]);
    let wb = open_sheet(&d.join("copy.ods"));
    assert_eq!(wb.display(0, CellRef::new(0, 0)).0, "Who");
    assert_eq!(wb.sheets[1].name, "Notes");
    assert_eq!(open_sheet(&d.join("t_book.xlsx")).display(0, CellRef::new(0, 0)).0, "Name");
    assert_eq!(run(&d, &["set", "t_book.xlsx", "A1"]).status.code(), Some(2));
    ok(&d, &["recalc", "t_book.xlsx"]);
    let info = ok(&d, &["info", "t_book.xlsx", "--json"]);
    assert!(info.contains("\"type\": \"spreadsheet\"") && info.contains("\"name\": \"Notes\"") && info.contains("\"macros\": false"), "{info}");
}

#[test]
fn merge_letters() {
    let d = folder("merge");
    letter(&d);
    std::fs::write(d.join("t_people.csv"), "Name,City,Amount\nAna,Lugo,3\nLuis,Vigo,5\n").unwrap();
    let out = ok(&d, &["merge", "t_letter.docx", "t_people.csv", "--out", "letters", "--name", "{n}-{Name}"]);
    assert_eq!(out.lines().count(), 2, "{out}");
    let doc = open_doc(&d.join("letters/2-Luis.docx"));
    let text = doc.to_plain_text();
    assert!(text.contains("Dear Luis, from Vigo.") && text.contains("5"), "{text}");
    // The formatting of the template is kept.
    let p = doc.paragraphs.iter().find(|p| p.text.starts_with("Dear")).unwrap();
    assert!(p.spans().any(|(s, e, a)| a.bold && &p.text[s..e] == "Luis"));
    assert!(doc.paragraphs.iter().any(|p| p.cell.is_some()));
    ok(&d, &["merge", "t_letter.docx", "t_people.csv", "--out", "letters", "--single", "-q"]);
    let all = open_doc(&d.join("letters/t_letter-merged.docx")).to_plain_text();
    assert!(all.contains("Dear Ana") && all.contains("Dear Luis"));
    ok(&d, &["merge", "t_letter.docx", "t_people.csv", "--out", "pdf", "--pdf", "-q"]);
    assert!(std::fs::read(d.join("pdf/t_letter-1.pdf")).unwrap().starts_with(b"%PDF"));
    assert!(d.join("pdf/t_letter-2.pdf").exists());
    assert_eq!(run(&d, &["merge", "t_people.csv", "t_letter.docx"]).status.code(), Some(1));
}

#[test]
fn help_and_usage() {
    let d = folder("help");
    assert!(ok(&d, &["--help"]).contains("convert"));
    assert!(ok(&d, &["help", "merge"]).contains("--single"));
    assert!(ok(&d, &["cat", "--help"]).contains("--range"));
    assert!(ok(&d, &["tui", "--help"]).contains("F10"));
    assert_eq!(run(&d, &["cat", "--bogus"]).status.code(), Some(2));
    assert_eq!(run(&d, &["help", "bogus"]).status.code(), Some(2));
}

/// The test certificates of apex-sign: Alice (RSA, password "secreto")
/// and Bob (ECDSA, no password).
fn certificates(dir: &Path) {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../apex-sign/tests/data");
    for f in ["alice-aes.p12", "bob-empty.p12"] {
        std::fs::copy(data.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("pw.txt"), "secreto\n").unwrap();
}

#[test]
fn sign_and_verify() {
    let d = folder("sign");
    certificates(&d);
    book(&d);
    letter(&d);
    apex_io::save_doc(&open_doc(&d.join("t_letter.docx")), &d.join("t_letter.odt"), Format::Odt).unwrap();
    apex_io::save_sheet(&open_sheet(&d.join("t_book.xlsx")), &d.join("t_book.ods"), Format::Ods).unwrap();
    for f in ["t_book.xlsx", "t_letter.docx", "t_letter.odt", "t_book.ods"] {
        // Not signed yet: verify fails.
        let o = run(&d, &["verify", f]);
        assert_eq!(o.status.code(), Some(1));
        assert!(stdout(&o).contains("not signed"), "{f}");
        // Alice with a password file and a comment, then Bob in place.
        let out = format!("s_{f}");
        assert!(ok(&d, &["sign", f, "--cert", "alice-aes.p12", "--password-file", "pw.txt", "--reason", "Revisado", "-o", &out]).contains("signed by Alice Tester"));
        ok(&d, &["sign", &out, "--cert", "bob-empty.p12", "-q"]);
        let text = ok(&d, &["verify", &out]);
        assert!(text.contains("2 signatures") && text.contains("Alice Tester") && text.contains("Bob Tester") && text.contains("reason: Revisado"), "{f}: {text}");
        assert!(!text.contains("invalid"), "{f}: {text}");
        let json = ok(&d, &["verify", &out, "--json"]);
        assert!(json.contains("\"signed\": true") && json.contains("\"intact\": \"yes\""), "{json}");
        // The signed file still opens.
        assert!(apex_io::open(&d.join(&out)).is_ok());
    }
    // The password from the environment; a wrong one on the standard input.
    let o = Command::new(env!("CARGO_BIN_EXE_apexdesk")).args(["sign", "t_book.xlsx", "--cert", "alice-aes.p12", "--password-env", "APEX_TEST_PW", "-o", "e.xlsx"]).current_dir(&d).env("APEX_TEST_PW", "secreto").output().unwrap();
    assert!(o.status.success());
    let o = run(&d, &["sign", "t_book.xlsx", "--cert", "alice-aes.p12", "-o", "w.xlsx"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("wrong password"));
    assert!(!d.join("w.xlsx").exists());
    // What can't be signed, and usage errors.
    ok(&d, &["convert", "t_book.xlsx", "--to", "csv", "-q"]);
    assert_eq!(run(&d, &["sign", "t_book.csv", "--cert", "bob-empty.p12"]).status.code(), Some(1));
    assert_eq!(run(&d, &["sign", "t_book.xlsx"]).status.code(), Some(2));
    assert_eq!(run(&d, &["sign", "t_book.xlsx", "--cert", "x", "--password", "secreto"]).status.code(), Some(2));
    // Keys in a store or on a token: one source, and its options with it.
    assert_eq!(run(&d, &["sign", "t_book.xlsx", "--cert", "alice-aes.p12", "--store"]).status.code(), Some(2));
    assert_eq!(run(&d, &["sign", "t_book.xlsx", "--cert", "alice-aes.p12", "--cert-sha1", "00"]).status.code(), Some(2));
    assert_eq!(run(&d, &["sign", "t_book.xlsx", "--store", "--slot", "1"]).status.code(), Some(2));
    assert_eq!(run(&d, &["sign", "--cert", "alice-aes.p12", "--list"]).status.code(), Some(2));
    let o = run(&d, &["sign", "--pkcs11", "no-such-module.so", "--list"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("no-such-module.so"));
}

// --- page breaks -------------------------------------------------------------

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

/// A docx of `paras` on a page of `sect` (`w:pgSz`, `w:pgMar`...).
fn docx(paras: &[String], sect: &str, styles: &str) -> Vec<u8> {
    let mut z = apex_io::zip::ZipWriter::new();
    let mut add = |name: &str, s: &str| z.add(name, s.as_bytes(), true);
    add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>"#);
    add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#);
    add("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>"#);
    add("word/document.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W_NS}><w:body>{}<w:sectPr>{sect}</w:sectPr></w:body></w:document>"#, paras.concat()));
    add("word/styles.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {W_NS}>{styles}</w:styles>"#));
    add("word/numbering.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering {W_NS}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#));
    z.finish()
}

const LETTER: &str = r#"<w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>"#;

fn defaults(font: &str, size: u32, ppr: &str) -> String {
    format!(r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}"/><w:sz w:val="{size}"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr>{ppr}</w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>"#)
}

/// Where the pages of a document start (paragraph numbers), as
/// `apexdesk info --page-starts` tells.
fn starts(docx: &[u8]) -> Vec<usize> {
    let dir = folder("page_starts");
    std::fs::write(dir.join("t.docx"), docx).unwrap();
    let out = ok(&dir, &["info", "--json", "--page-starts", "t.docx"]);
    out.split("\"paragraph\":").skip(1).map(|s| s.trim_start().split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap()).collect()
}

fn plain(n: usize, text: &str) -> Vec<String> {
    (0..n).map(|i| format!(r#"<w:p><w:r><w:t xml:space="preserve">{i} {text}</w:t></w:r></w:p>"#)).collect()
}

/// Pages as Word breaks them (counted in Word for these files).
#[test]
fn page_breaks_as_in_word() {
    list_items_of_one_style_have_no_space_between_them();
    spaces_at_the_end_of_a_line_dont_wrap();
    fonts_not_installed_keep_their_line_height();
    japanese_lines_take_one_grid_line();
}

fn list_items_of_one_style_have_no_space_between_them() {
    // Word 2013: 11 pt Calibri at 1.08 lines, 8 pt after; List
    // Paragraph drops the space between its items: 44 a page, not 28.
    let styles = defaults("Calibri", 22, r#"<w:spacing w:after="160" w:line="259" w:lineRule="auto"/>"#)
        + r#"<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style>"#;
    let items: Vec<String> = (0..60).map(|i| format!(r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Item {i}</w:t></w:r></w:p>"#)).collect();
    assert_eq!(starts(&docx(&items, LETTER, &styles)), [0, 44]);
}

fn spaces_at_the_end_of_a_line_dont_wrap() {
    // Each paragraph is one line in Word, however many spaces follow.
    let text = format!("Lorem ipsum dolor sit amet consectetur adipiscing elit sed do{}", " ".repeat(120));
    let styles = defaults("Arial", 20, r#"<w:spacing w:after="0" w:line="240" w:lineRule="auto"/>"#);
    assert_eq!(starts(&docx(&plain(60, &text), LETTER, &styles)), [0, 56]);
}

fn fonts_not_installed_keep_their_line_height() {
    // Cambria 12 pt: 46 lines a page in Word (Noto's taller lines
    // would give 39).
    let styles = defaults("Cambria", 24, r#"<w:spacing w:after="0" w:line="240" w:lineRule="auto"/>"#);
    assert_eq!(starts(&docx(&plain(60, "Line "), LETTER, &styles)), [0, 46]);
}

fn japanese_lines_take_one_grid_line() {
    // A Japanese template: MS Mincho 10.5 pt on an 18 pt line grid,
    // 36 lines a page (a stand-in font's taller lines took two).
    let styles = r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Century" w:eastAsia="ＭＳ 明朝" w:hAnsi="Century" w:cs="Times New Roman"/><w:kern w:val="2"/><w:sz w:val="21"/><w:szCs w:val="22"/><w:lang w:val="en-US" w:eastAsia="ja-JP" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault/></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/><w:pPr><w:widowControl w:val="0"/><w:jc w:val="both"/></w:pPr></w:style>"#;
    let sect = r#"<w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1985" w:right="1701" w:bottom="1701" w:left="1701" w:header="851" w:footer="992" w:gutter="0"/><w:cols w:space="425"/><w:docGrid w:type="lines" w:linePitch="360"/>"#;
    assert_eq!(starts(&docx(&plain(80, "あいうえお漢字テスト"), sect, styles)), [0, 36, 72]);
}
