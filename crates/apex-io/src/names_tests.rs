//! Defined names through xlsx and ods.

use apex_sheet::formula::parse_canonical;
use apex_sheet::{CellRef, DefinedName, Sheet, Value, Workbook};

fn book() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = apex_sheet::Locale::EN;
    wb.sheets[0].name = "Data".into();
    wb.execute(apex_sheet::Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("My sheet")) });
    for r in 0..3 {
        wb.set_input(0, CellRef::new(r + 1, 1), &format!("{}", (r + 1) * 10));
    }
    let mut sales = DefinedName::new("Sales", None, parse_canonical("Data!$B$2:$B$4").unwrap());
    sales.comment = "Monthly <sales>".into();
    wb.define_name(sales);
    wb.define_name(DefinedName::new("Rate", None, parse_canonical("0.5").unwrap()));
    wb.define_name(DefinedName::new("Twice", None, parse_canonical("LAMBDA(x,x*2)").unwrap()));
    wb.define_name(DefinedName::new("Local", Some("My sheet".into()), parse_canonical("'My sheet'!$A$1").unwrap()));
    wb.set_input(1, CellRef::new(0, 0), "7");
    wb.set_input(0, CellRef::new(0, 3), "=SUM(Sales)*Rate");
    wb.set_input(0, CellRef::new(0, 4), "=Twice(4)");
    wb.set_input(0, CellRef::new(0, 5), "='My sheet'!Local+1");
    wb
}

fn check(wb: &Workbook, what: &str) {
    let v = |c| wb.sheets[0].value(c).clone();
    assert_eq!(v(CellRef::new(0, 3)), Value::Number(30.0), "{what}: SUM(Sales)*Rate");
    assert_eq!(v(CellRef::new(0, 4)), Value::Number(8.0), "{what}: LAMBDA name");
    assert_eq!(v(CellRef::new(0, 5)), Value::Number(8.0), "{what}: local name");
    let sales = wb.find_name(0, "Sales").unwrap();
    assert_eq!(sales.expr.to_canonical(), "Data!$B$2:$B$4", "{what}");
    assert_eq!(wb.find_name(1, "Local").unwrap().scope.as_deref(), Some("My sheet"), "{what}");
    assert!(wb.find_name(0, "Local").is_none(), "{what}: local names stay local");
}

#[test]
fn xlsx_names() {
    let wb = book();
    check(&wb, "model");
    let back = crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap();
    check(&back, "xlsx");
    assert_eq!(back.find_name(0, "Sales").unwrap().comment, "Monthly <sales>");
    assert_eq!(back.names.len(), 4);
}

#[test]
fn ods_names() {
    let wb = book();
    let back = crate::ods::read(&crate::ods::write(&wb)).unwrap();
    check(&back, "ods");
    assert_eq!(back.names.len(), 4);
}

#[test]
fn xlsx_unknown_names_are_kept() {
    let mut wb = book();
    wb.names.push(DefinedName::new("Ext", None, apex_sheet::Expr::Name("[1]Sheet1!$A$1".into())));
    let bytes = crate::xlsx::write(&wb);
    let back = crate::xlsx::read(&bytes).unwrap();
    assert_eq!(back.find_name(0, "Ext").unwrap().expr.to_xlsx(), "[1]Sheet1!$A$1");
}

#[test]
fn external_links_xlsx() {
    let mut wb = Workbook::new();
    wb.locale = apex_sheet::Locale::EN;
    let mut cells = std::collections::BTreeMap::new();
    cells.insert(CellRef::new(0, 0), Value::Number(5.0));
    cells.insert(CellRef::new(1, 0), Value::text("a <b>"));
    wb.externals.push(apex_sheet::ExternalBook { target: "file:///C:/Books/Other%20book.xlsx".into(), sheets: vec!["Data".into(), "Q 1".into()], names: vec![("Rate".into(), None, "=Data!$A$1".into())], cells: vec![cells, [(CellRef::new(0, 0), Value::Number(1.0))].into_iter().collect()] });
    wb.set_input(0, CellRef::new(0, 0), "=[1]Data!A1*2+[1]!Rate");
    wb.set_input(0, CellRef::new(0, 1), "=[1]Data!A2&'[1]Q 1'!B7");
    let check = |wb: &Workbook| {
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), &Value::Number(15.0));
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), &Value::text("a <b>"));
        assert_eq!(wb.externals.len(), 1);
        assert_eq!(wb.externals[0].file_name(), "Other%20book.xlsx");
    };
    check(&wb);
    let back = crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap();
    check(&back);
    assert_eq!(back.sheets[0].get(CellRef::new(0, 1)).unwrap().formula().unwrap().expr().to_canonical(), "[1]Data!A2&'[1]Q 1'!B7");
}

#[test]
fn external_links_ods() {
    let mut wb = Workbook::new();
    wb.locale = apex_sheet::Locale::EN;
    wb.load_cell(0, CellRef::new(0, 0), apex_sheet::Content::Formula { expr: (parse_canonical("'[file:///home/u/B.ods]Sheet 1'!A1*2").unwrap()).into(), value: Value::Number(8.0) }, 0);
    wb.normalize_externals();
    wb.finish_load();
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), &Value::Number(8.0));
    assert_eq!(wb.externals[0].target, "file:///home/u/B.ods");
    let back = crate::ods::read(&crate::ods::write(&wb)).unwrap();
    assert_eq!(back.sheets[0].get(CellRef::new(0, 0)).unwrap().formula().unwrap().expr().to_canonical(), "'[1]Sheet 1'!A1*2");
    assert_eq!(back.sheets[0].value(CellRef::new(0, 0)), &Value::Number(8.0));
    assert_eq!(back.externals[0].target, "file:///home/u/B.ods");
}
