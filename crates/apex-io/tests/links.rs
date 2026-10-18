//! Values taken from a linked workbook (Edit Links > Update Values) are
//! saved with the link, and a broken link is gone from the file.

use apex_sheet::{CellRef, Value, Workbook};

fn c(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

#[test]
fn updated_link_values_survive_saving() {
    let mut wb = Workbook::new();
    wb.set_input(0, c("A1"), "=[Budget.xlsx]Data!A1*2");
    wb.set_input(0, c("A2"), "=[Other.xlsx]S!A1");
    let mut src = Workbook::new();
    src.sheets[0].name = "Data".into();
    src.set_input(0, c("A1"), "21");
    wb.update_external(0, &src);
    for (name, bytes, read) in [
        ("xlsx", apex_io::xlsx::write(&wb), apex_io::xlsx::read as fn(&[u8]) -> Result<Workbook, apex_io::Error>),
        ("ods", apex_io::ods::write(&wb), apex_io::ods::read),
    ] {
        let back = read(&bytes).unwrap();
        assert_eq!(*back.sheets[0].value(c("A1")), Value::Number(42.0), "{name}");
        assert_eq!(back.externals.len(), 2, "{name}");
    }
    wb.break_external(0);
    let back = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
    assert_eq!(back.externals.len(), 1);
    assert_eq!(back.externals[0].file_name(), "Other.xlsx");
    assert_eq!(*back.sheets[0].value(c("A1")), Value::Number(42.0));
    assert!(back.sheets[0].get(c("A1")).unwrap().formula().is_none());
}
