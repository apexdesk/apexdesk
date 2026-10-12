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
