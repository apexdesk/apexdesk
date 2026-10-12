//! Formulas keep their meaning through xlsx and ods: ranges taken whole,
//! whole columns and rows, 3D references, logical constants, errors and
//! structured references.

use apex_sheet::{CellRef, ErrorKind, Locale, Range, Sheet, Value, Workbook};

type Read = fn(&[u8]) -> Result<Workbook, apex_io::Error>;

fn c(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn both(wb: &Workbook) -> [(&'static str, Vec<u8>, Read); 2] {
    [("xlsx", apex_io::xlsx::write(wb), apex_io::xlsx::read as Read), ("ods", apex_io::ods::write(wb), apex_io::ods::read as Read)]
}

fn content_xml(ods: &[u8]) -> String {
    let z = apex_io::zip::ZipReader::new(ods).unwrap();
    z.read_string("content.xml").unwrap()
}

#[test]
fn formulas_keep_their_text() {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    wb.sheets.push(Sheet::new("Sheet2"));
    wb.sheets.push(Sheet::new("Sheet3"));
    for (i, v) in ["1", "2", "3"].iter().enumerate() {
        wb.set_input(0, CellRef::new(i as u32 + 1, 1), v);
        wb.set_input(i, CellRef::new(0, 0), v);
    }
    wb.finish_load();
    let formulas = [
        // A range where one value is due: taken whole (not `@B2:B4`).
        ("E10", "=SUM(VALUE(B2:B4))", Value::Number(6.0)),
        ("E11", "=SUM(1*ISBLANK(B2:B4))", Value::Number(0.0)),
        ("E12", "=SUM(B:B)", Value::Number(6.0)),
        ("E13", "=SUM(3:3)", Value::Number(2.0)),
        ("E14", "=SUM(Sheet2:Sheet3!A1)", Value::Number(5.0)),
        ("E15", "=IF(TRUE,1,2)", Value::Number(1.0)),
        ("E16", "=SUM(IF({TRUE;FALSE},B2:B3,0))", Value::Number(1.0)),
    ];
    for (a, f, _) in &formulas {
        wb.set_input(0, c(a), f);
    }
    for (name, bytes, read) in both(&wb) {
        let mut back = read(&bytes).unwrap();
    back.locale = Locale::EN;
        for (a, f, v) in &formulas {
            assert_eq!(back.edit_text(0, c(a)), *f, "{name} {a}");
            assert_eq!(back.sheets[0].value(c(a)), v, "{name} {a}");
        }
    }
    // LibreOffice's spelling of whole columns and rows.
    let xml = content_xml(&apex_io::ods::write(&wb));
    assert!(xml.contains("of:=SUM([.B:.B])") && xml.contains("of:=SUM([.3:.3])"), "{xml}");
}

#[test]
fn errors_stay_errors_in_ods() {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    wb.set_input(0, c("A1"), "=1/0");
    wb.set_input(0, c("A2"), "=NOSUCHFUNCTION(1)");
    wb.load_cell(0, c("A3"), apex_sheet::Content::Value(Value::Error(ErrorKind::Na)), 0);
    let mut back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
    back.locale = Locale::EN;
    assert_eq!(back.sheets[0].value(c("A1")), &Value::Error(ErrorKind::Div0));
    assert_eq!(back.sheets[0].value(c("A2")), &Value::Error(ErrorKind::Name));
    assert_eq!(back.sheets[0].value(c("A3")), &Value::Error(ErrorKind::Na));
}

#[test]
fn ods_reads_libreoffice_errors() {
    // As LibreOffice writes a cached error (string for older readers),
    // including its own codes.
    let cell = |text: &str| format!(r#"<table:table-cell office:value-type="string" office:string-value="" calcext:value-type="error"><text:p>{text}</text:p></table:table-cell>"#);
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    wb.set_input(0, c("A1"), "1");
    let bytes = apex_io::ods::write(&wb);
    let xml = content_xml(&bytes);
    let row = format!("<table:table-row>{}{}</table:table-row>", cell("#N/A"), cell("Err:502"));
    let xml = xml.replacen("</table:table>", &format!("{row}</table:table>"), 1);
    let mut back = apex_io::ods::read(&rezip(&bytes, "content.xml", &xml)).unwrap();
    back.locale = Locale::EN;
    assert_eq!(back.sheets[0].value(c("A2")), &Value::Error(ErrorKind::Na));
    assert_eq!(back.sheets[0].value(c("B2")), &Value::Error(ErrorKind::Value));
}

fn rezip(bytes: &[u8], part: &str, text: &str) -> Vec<u8> {
    let z = apex_io::zip::ZipReader::new(bytes).unwrap();
    let mut w = apex_io::zip::ZipWriter::new();
    for name in z.names() {
        if name == part {
            w.add(name, text.as_bytes(), true);
        } else {
            w.add(name, &z.read(name).unwrap(), name != "mimetype");
        }
    }
    w.finish()
}

fn table_book() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    for (r, row) in [["Item", "Qty", "Double"], ["a", "2", ""], ["b", "3", ""], ["c", "5", ""]].iter().enumerate() {
        for (k, v) in row.iter().enumerate() {
            if !v.is_empty() {
                wb.set_input(0, CellRef::new(r as u32, k as u32), v);
            }
        }
    }
    let id = wb.create_table(0, Range::parse("A1:C4").unwrap(), true, "TableStyleMedium2").unwrap();
    for r in 2..=4 {
        wb.set_input(0, c(&format!("C{r}")), "=[@Qty]*2");
    }
    wb.set_table_totals(id, true);
    wb.set_input(0, c("E1"), "=SUM(Table1[Qty])");
    wb.set_input(0, c("E2"), "=COUNTA(Table1[[#Headers],[Item]:[Qty]])");
    wb
}

#[test]
fn structured_references_in_ods() {
    let wb = table_book();
    let before: Vec<String> = ["C2", "C5", "E1", "E2"].iter().map(|a| wb.edit_text(0, c(a))).collect();
    assert_eq!(before, ["=[@Qty]*2", "=SUBTOTAL(109,[Double])", "=SUM(Table1[Qty])", "=COUNTA(Table1[[#Headers],[Item]:[Qty]])"]);
    let bytes = apex_io::ods::write(&wb);
    let xml = content_xml(&bytes);
    // How LibreOffice writes them.
    assert!(xml.contains("of:=Table1[[#This Row];[Qty]]*2"), "{xml}");
    assert!(xml.contains("of:=SUM(Table1[Qty])"), "{xml}");
    assert!(xml.contains("of:=COUNTA(Table1[[#Headers];[Item]:[Qty]])"), "{xml}");
    let mut back = apex_io::ods::read(&bytes).unwrap();
    back.locale = Locale::EN;
    let after: Vec<String> = ["C2", "C5", "E1", "E2"].iter().map(|a| back.edit_text(0, c(a))).collect();
    assert_eq!(after, before);
    assert_eq!(back.sheets[0].value(c("C4")), &Value::Number(10.0));
    assert_eq!(back.sheets[0].value(c("C5")), &Value::Number(20.0));
    assert_eq!(back.sheets[0].value(c("E1")), &Value::Number(10.0));
    assert_eq!(back.sheets[0].value(c("E2")), &Value::Number(2.0));
    assert!(back.sheets[0].tables[0].totals);
}

#[test]
fn own_table_references_in_xlsx() {
    let wb = table_book();
    let bytes = apex_io::xlsx::write(&wb);
    let z = apex_io::zip::ZipReader::new(&bytes).unwrap();
    let sheet = z.read_string("xl/worksheets/sheet1.xml").unwrap();
    // Written with the table's name (LibreOffice needs it)...
    assert!(sheet.contains("<f>SUBTOTAL(109,Table1[Double])</f>"), "{sheet}");
    // ...and shown without it inside the table, as Excel does.
    let mut back = apex_io::xlsx::read(&bytes).unwrap();
    back.locale = Locale::EN;
    assert_eq!(back.edit_text(0, c("C5")), "=SUBTOTAL(109,[Double])");
    assert_eq!(back.edit_text(0, c("C3")), "=[@Qty]*2");
    assert_eq!(back.edit_text(0, c("E1")), "=SUM(Table1[Qty])");
}

#[test]
fn multibyte_names_do_not_panic() {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    wb.set_input(0, c("A1"), "2");
    let e = apex_sheet::formula::parse_canonical("Sheet1!$A$1").unwrap();
    wb.names.push(apex_sheet::names::DefinedName::new("Прибыль", None, e));
    wb.set_input(0, c("B1"), "=Прибыль*2");
    for (name, bytes, read) in both(&wb) {
        let mut back = read(&bytes).unwrap();
    back.locale = Locale::EN;
        assert_eq!(back.edit_text(0, c("B1")), "=Прибыль*2", "{name}");
    }
}

#[test]
fn empty_text_cells_in_ods() {
    // COUNTIF and lookups tell an empty text from an empty cell.
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    wb.load_cell(0, c("A1"), apex_sheet::Content::Value(Value::text("")), 0);
    wb.set_input(0, c("C1"), "=COUNTIF(B1,A1)");
    wb.finish_load();
    let back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
    assert_eq!(back.sheets[0].value(c("A1")), &Value::text(""));
    assert_eq!(back.sheets[0].value(c("C1")), wb.sheets[0].value(c("C1")));
}

#[test]
fn array_formulas_keep_their_size() {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    for (a, v) in [("A1", "1"), ("A2", "5"), ("B1", "2"), ("B2", "6"), ("D2", "x")] {
        wb.set_input(0, c(a), v);
    }
    wb.set_input(0, c("D1"), "=FREQUENCY(A1:A2,B1:B2)");
    wb.sheets[0].mark_fixed_array(&Range::single(c("D1")));
    wb.recalc_all();
    assert_eq!(wb.sheets[0].value(c("D1")), &Value::Number(1.0));
    let xml = |x: &[u8]| apex_io::zip::ZipReader::new(x).unwrap().read_string("xl/worksheets/sheet1.xml").unwrap();
    let bytes = apex_io::xlsx::write(&wb);
    assert!(xml(&bytes).contains(r#"<c r="D1"><f t="array" ref="D1">"#), "{}", xml(&bytes));
    let back = apex_io::xlsx::read(&bytes).unwrap();
    assert_eq!(back.sheets[0].get(c("D1")).and_then(|x| x.extra.as_ref()?.array_size), Some((1, 1)));
    assert_eq!(back.sheets[0].value(c("D1")), &Value::Number(1.0));
}
