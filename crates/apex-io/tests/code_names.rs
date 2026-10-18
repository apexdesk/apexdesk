//! Sheet code names (the names VBA code uses, `<sheetPr codeName>`) are
//! kept through xlsx.

use apex_sheet::{Sheet, Workbook};

#[test]
fn code_names_roundtrip() {
    let mut wb = Workbook::new();
    wb.sheets[0].code_name = Some("Hoja1".into());
    wb.sheets[0].tab_color = Some(0xFF0000);
    wb.sheets.push(Sheet::new("Data"));
    wb.sheets[1].code_name = Some("shData".into());
    wb.sheets.push(Sheet::new("Plain"));
    let back = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
    let names: Vec<Option<&str>> = back.sheets.iter().map(|s| s.code_name.as_deref()).collect();
    assert_eq!(names, [Some("Hoja1"), Some("shData"), None]);
    assert_eq!(back.sheets[0].tab_color, Some(0xFF0000));
}

#[test]
fn workbook_code_name_roundtrip() {
    let mut wb = Workbook::new();
    wb.code_name = Some("ThisWorkbook".into());
    let back = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
    assert_eq!(back.code_name.as_deref(), Some("ThisWorkbook"));
    assert!(!back.calc.date1904);
    wb.calc.date1904 = true;
    let back = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
    assert_eq!((back.code_name.as_deref(), back.calc.date1904), (Some("ThisWorkbook"), true));
    let back = apex_io::xlsx::read(&apex_io::xlsx::write(&Workbook::new())).unwrap();
    assert_eq!(back.code_name, None);
}

#[test]
fn copies_drop_the_code_name() {
    let mut wb = Workbook::new();
    wb.sheets[0].code_name = Some("Sheet1".into());
    let i = wb.duplicate_sheet(0);
    assert_eq!(wb.sheets[i].code_name, None);
    let mut other = Workbook::new();
    let j = other.adopt_sheet(&wb, 0, 1);
    assert_eq!(other.sheets[j].code_name, None);
    assert_eq!(wb.sheets[0].code_name.as_deref(), Some("Sheet1"));
}

#[test]
fn names_excel_refuses_are_renamed() {
    let mut wb = Workbook::new();
    wb.sheets.push(Sheet::new("'Admin\\Library"));
    wb.sheets.push(Sheet::new("'file:///C:/sources/libo-core/base.ods'#Sheet1"));
    wb.set_input(0, apex_sheet::CellRef::parse("A1").unwrap(), "='''Admin\\Library'!B2");
    let back = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
    let names: Vec<&str> = back.sheets.iter().map(|s| s.name.as_ref()).collect();
    assert_eq!(names[1], "Admin_Library");
    assert_eq!(names[2], "file____C__sources_libo-core_ba");
    assert!(back.edit_text(0, apex_sheet::CellRef::parse("A1").unwrap()).contains("Admin_Library!B2"));
}
