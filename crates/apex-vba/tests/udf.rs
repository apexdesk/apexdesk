//! Functions of the code called from cells.

mod common;

use std::sync::Arc;

use apex_sheet::{CellRef, ErrorKind, Value};
use apex_vba::udf::Functions;
use apex_vba::{ModuleKind, Options, Source};

const CODE: &str = r#"
Function IVA(x)
    IVA = x * 1.21
End Function

Function Total(r As Range, Optional factor = 1)
    Dim c, s
    For Each c In r
        s = s + c.Value
    Next
    Total = s * factor
End Function

Function Writes(x)
    Range("C1").Value = 5
    Writes = x
End Function

Private Function Hidden(x)
    Hidden = x
End Function

Function Where()
    Where = Application.Caller.Address
End Function

Function Forever()
    Do
    Loop
End Function

Function Pair()
    Pair = Array(1, "two")
End Function

Function Again()
    Again = Evaluate("=IVA(100)")
End Function

Function Fail()
    Fail = 1 / 0
End Function

Function Now2()
    Application.Volatile
    Now2 = 1
End Function
"#;

fn set(wb: &mut apex_sheet::Workbook, cell: &str, text: &str) {
    wb.set_input(0, CellRef::parse(cell).unwrap(), text);
}

fn get(wb: &apex_sheet::Workbook, cell: &str) -> Value {
    wb.cell(0, CellRef::parse(cell).unwrap()).map(|c| c.value().clone()).unwrap_or_default()
}

fn functions() -> Arc<Functions> {
    let sources = Arc::new(vec![Source { name: "Module1".into(), kind: ModuleKind::Standard, code: CODE.into() }]);
    Arc::new(Functions::new(sources, &Options::default()).expect("functions"))
}

#[test]
fn cells_call_the_code() {
    let mut wb = common::workbook();
    set(&mut wb, "A1", "100");
    set(&mut wb, "A2", "2");
    set(&mut wb, "B1", "=IVA(A1)");
    // Not yet given: an unknown name.
    assert_eq!(get(&wb, "B1"), Value::Error(ErrorKind::Name));
    wb.set_udf(Some(functions()));
    assert_eq!(get(&wb, "B1"), Value::Number(121.0));
    // Calculated again when A1 changes.
    set(&mut wb, "A1", "200");
    assert_eq!(get(&wb, "B1"), Value::Number(242.0));
    set(&mut wb, "B2", "=Total(A1:A2, 2)");
    assert_eq!(get(&wb, "B2"), Value::Number(404.0));
    set(&mut wb, "B3", "=total(A1:A2)");
    assert_eq!(get(&wb, "B3"), Value::Number(202.0));
    set(&mut wb, "B4", "=Where()");
    assert_eq!(get(&wb, "B4"), Value::Text("$B$4".into()));
    // An array spills.
    set(&mut wb, "D1", "=Pair()");
    assert_eq!(get(&wb, "E1"), Value::Text("two".into()));
    // Taken away (macros off): the cells keep their last values, a new
    // formula shows #NAME?.
    wb.set_udf(None);
    assert_eq!(get(&wb, "B2"), Value::Number(404.0));
    set(&mut wb, "B5", "=IVA(1)");
    assert_eq!(get(&wb, "B5"), Value::Error(ErrorKind::Name));
}

#[test]
fn functions_may_not_change_the_workbook_or_run_away() {
    let mut wb = common::workbook();
    wb.set_udf(Some(functions()));
    set(&mut wb, "B1", "=Writes(1)");
    assert_eq!(get(&wb, "B1"), Value::Error(ErrorKind::Value));
    assert_eq!(get(&wb, "C1"), Value::Empty);
    set(&mut wb, "B2", "=Hidden(1)");
    assert_eq!(get(&wb, "B2"), Value::Error(ErrorKind::Name));
    set(&mut wb, "B3", "=Forever()");
    assert_eq!(get(&wb, "B3"), Value::Error(ErrorKind::Value));
    set(&mut wb, "B4", "=Again()");
    assert_eq!(get(&wb, "B4"), Value::Error(ErrorKind::Value));
    set(&mut wb, "B5", "=Fail()");
    assert_eq!(get(&wb, "B5"), Value::Error(ErrorKind::Value));
    set(&mut wb, "B6", "=IVA(1,2)");
    assert_eq!(get(&wb, "B6"), Value::Error(ErrorKind::Value));
}

#[test]
fn volatile_functions_are_known() {
    use apex_sheet::udf::Udf;
    let f = functions();
    assert!(f.is_volatile("NOW2") && !f.is_volatile("IVA"));
    assert!(f.has("iva") && !f.has("Hidden"));
    assert!(f.names().contains(&("Total".to_string(), vec!["r".to_string(), "[factor]".to_string()])));
}

#[test]
fn library_functions_come_after_the_workbook_ones() {
    let lib = apex_vba::Library {
        name: "personal.xlsm".into(),
        sources: Arc::new(vec![Source { name: "Module1".into(), kind: ModuleKind::Standard, code: "Function IVA(x)\n IVA = 0\nEnd Function\nFunction Doble(x)\n Doble = x * 2\nEnd Function\n".into() }]),
    };
    let own = Arc::new(vec![Source { name: "Module1".into(), kind: ModuleKind::Standard, code: CODE.into() }]);
    let mut wb = common::workbook();
    wb.set_udf(Some(Arc::new(Functions::with_libraries(Some(own), std::slice::from_ref(&lib), &Options::default()).unwrap())));
    set(&mut wb, "B1", "=IVA(100)");
    assert_eq!(get(&wb, "B1"), Value::Number(121.0));
    set(&mut wb, "B2", "=Doble(4)");
    assert_eq!(get(&wb, "B2"), Value::Number(8.0));
    // Only the library's.
    let mut wb = common::workbook();
    wb.set_udf(Some(Arc::new(Functions::with_libraries(None, &[lib], &Options::default()).unwrap())));
    set(&mut wb, "B1", "=IVA(100)");
    assert_eq!(get(&wb, "B1"), Value::Number(0.0));
    assert!(wb.udf.as_ref().unwrap().names().iter().filter(|(n, _)| n == "IVA").count() == 1);
}
