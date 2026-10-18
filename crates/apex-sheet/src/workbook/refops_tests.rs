//! 3D references and the union and intersection operators.

use super::*;
use crate::formula::parse_canonical;

fn wb() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = crate::format::Locale::EN;
    wb.sheets[0].name = "Jan".into();
    wb.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("Feb")) });
    wb.execute(Op::AddSheet { index: 2, sheet: Box::new(Sheet::new("Mar Q1")) });
    wb.execute(Op::AddSheet { index: 3, sheet: Box::new(Sheet::new("Total")) });
    for s in 0..3 {
        for r in 0..3 {
            wb.set_input(s, CellRef::new(r, 0), &format!("{}", (s + 1) * 10 + r as usize));
        }
    }
    wb
}

fn val(wb: &Workbook, s: usize, r: u32, c: u32) -> Value {
    wb.sheets[s].value(CellRef::new(r, c)).clone()
}

#[test]
fn three_d() {
    let mut wb = wb();
    wb.set_input(3, CellRef::new(0, 0), "=SUM(Jan:'Mar Q1'!A1)");
    assert_eq!(val(&wb, 3, 0, 0), Value::Number(60.0));
    wb.set_input(3, CellRef::new(1, 0), "=SUM(Jan:Feb!A1:A3)");
    assert_eq!(val(&wb, 3, 1, 0), Value::Number(10.0 + 11.0 + 12.0 + 20.0 + 21.0 + 22.0));
    wb.set_input(3, CellRef::new(2, 0), "=COUNT(Jan:Total!A1)+MAX(Jan:'Mar Q1'!A2)");
    assert_eq!(val(&wb, 3, 2, 0), Value::Number(4.0 + 31.0));
    // A change in a middle sheet recalculates.
    wb.set_input(1, CellRef::new(0, 0), "100");
    assert_eq!(val(&wb, 3, 0, 0), Value::Number(140.0));
    // Written back the way Excel writes it.
    let e = parse_canonical("SUM(Jan:'Mar Q1'!A1,Jan:Feb!$B$2:C3)").unwrap();
    assert_eq!(e.to_canonical(), "SUM('Jan:Mar Q1'!A1,Jan:Feb!$B$2:C3)");
    assert_eq!(parse_canonical(&e.to_canonical()).unwrap(), e);
    assert_eq!(e.to_odf(), "of:=SUM([$Jan.A1:$'Mar Q1'.A1];[$Jan.$B$2:$Feb.C3])");
    // Renaming an end sheet renames the span.
    wb.execute(Op::RenameSheet { index: 0, name: "Enero".into() });
    assert_eq!(wb.sheets[3].get(CellRef::new(0, 0)).and_then(|c| c.formula()).unwrap().expr().to_canonical(), "SUM('Enero:Mar Q1'!A1)");
    assert_eq!(val(&wb, 3, 0, 0), Value::Number(140.0));
    // Functions that need one range don't take several.
    wb.set_input(3, CellRef::new(3, 0), "=VLOOKUP(1,Enero:Feb!A1:B3,2)");
    assert!(matches!(val(&wb, 3, 3, 0), Value::Error(_)));
}

#[test]
fn union_and_intersection() {
    let mut wb = wb();
    for r in 0..3 {
        for c in 1..4 {
            wb.set_input(0, CellRef::new(r, c), &format!("{}", r * 10 + c));
        }
    }
    // B1:D3 holds 1 2 3 / 11 12 13 / 21 22 23.
    wb.set_input(0, CellRef::new(5, 0), "=B1:D2 C2:C9");
    assert_eq!(val(&wb, 0, 5, 0), Value::Number(12.0));
    wb.set_input(0, CellRef::new(6, 0), "=SUM(B1:D3 C1:D2)");
    assert_eq!(val(&wb, 0, 6, 0), Value::Number(2.0 + 3.0 + 12.0 + 13.0));
    wb.set_input(0, CellRef::new(7, 0), "=B1:B2 D1:D2");
    assert_eq!(val(&wb, 0, 7, 0), Value::Error(ErrorKind::Null));
    wb.set_input(0, CellRef::new(8, 0), "=SUM((B1,D3),C2)");
    assert_eq!(val(&wb, 0, 8, 0), Value::Number(1.0 + 23.0 + 12.0));
    wb.set_input(0, CellRef::new(9, 0), "=AREAS((B1:C2,D3,A1))");
    assert_eq!(val(&wb, 0, 9, 0), Value::Number(3.0));
    wb.set_input(0, CellRef::new(13, 0), "=RANK(C1,(B1,C1,D1),1)");
    assert_eq!(val(&wb, 0, 13, 0), Value::Number(2.0));
    // A list of numbers in parentheses isn't a union.
    assert!(parse_canonical("A1-(-4,3)").is_err());
    wb.set_input(0, CellRef::new(10, 0), "=INDEX((B1:B3,D1:D3),2,1,2)");
    assert_eq!(val(&wb, 0, 10, 0), Value::Number(13.0));
    // A call keeps working with a space before its parenthesis? No: Excel
    // reads `A1 (B1)` as an intersection, but a function name is a call.
    wb.set_input(0, CellRef::new(11, 0), "=SUM (B1:B3)");
    assert_eq!(val(&wb, 0, 11, 0), Value::Number(33.0));
    // Spaces elsewhere are just spaces.
    wb.set_input(0, CellRef::new(12, 0), "= B1 + C1 ");
    assert_eq!(val(&wb, 0, 12, 0), Value::Number(3.0));
    let e = parse_canonical("SUM((A1:B2,C3),A1:C3 B2:B9)").unwrap();
    assert_eq!(e.to_canonical(), "SUM((A1:B2,C3),A1:C3 B2:B9)");
    assert_eq!(e.to_odf(), "of:=SUM(([.A1:.B2]~[.C3]);[.A1:.C3]![.B2:.B9])");
    assert_eq!(e.to_display(&crate::format::Locale::ES), "=SUM((A1:B2;C3);A1:C3 B2:B9)");
    // Dependencies of an intersection.
    wb.set_input(0, CellRef::new(1, 2), "100");
    assert_eq!(val(&wb, 0, 5, 0), Value::Number(100.0));
}

#[test]
fn external_and_unknown() {
    use crate::external::ExternalBook;
    let mut wb = wb();
    let mut cells = std::collections::BTreeMap::new();
    cells.insert(CellRef::new(0, 0), Value::Number(5.0));
    cells.insert(CellRef::new(1, 0), Value::Number(7.0));
    cells.insert(CellRef::new(0, 1), Value::text("hi"));
    wb.externals.push(ExternalBook { target: "Other.xlsx".into(), sheets: vec!["Data".into(), "My Sheet".into()], names: vec![("Rate".into(), None, "Data!$A$2".into())], cells: vec![cells, Default::default()] });
    wb.set_input(0, CellRef::new(5, 0), "=[1]Data!A1*2");
    assert_eq!(val(&wb, 0, 5, 0), Value::Number(10.0));
    wb.set_input(0, CellRef::new(6, 0), "=SUM([1]Data!A1:A2)+[1]!Rate");
    assert_eq!(val(&wb, 0, 6, 0), Value::Number(19.0));
    wb.set_input(0, CellRef::new(7, 0), "='[Other.xlsx]Data'!B1");
    assert_eq!(val(&wb, 0, 7, 0), Value::text("hi"));
    wb.set_input(0, CellRef::new(8, 0), "='[1]My Sheet'!B1+[1]Data!Z9");
    // A linked sheet without saved values can't be calculated.
    assert_eq!(val(&wb, 0, 8, 0), Value::Error(ErrorKind::Ref));
    wb.set_input(0, CellRef::new(8, 0), "=[1]Data!Z9");
    assert_eq!(val(&wb, 0, 8, 0), Value::Number(0.0));
    wb.set_input(0, CellRef::new(12, 0), "=[Other.xlsx]Data!A2");
    assert_eq!(val(&wb, 0, 12, 0), Value::Number(7.0));
    assert_eq!(wb.sheets[0].get(CellRef::new(12, 0)).unwrap().formula().unwrap().expr().to_canonical(), "[1]Data!A2");
    assert_eq!(wb.edit_text(0, CellRef::new(12, 0)), "=[Other.xlsx]Data!A2");
    for f in ["[1]Data!A1*2", "SUM([1]Data!A1:A2)+[1]!Rate", "'[1]My Sheet'!B1"] {
        assert_eq!(parse_canonical(f).unwrap().to_canonical(), f);
    }
    wb.set_input(0, CellRef::new(11, 0), "=[0]!Here+[0]Jan!A2");
    wb.define_name(crate::DefinedName::new("Here", None, parse_canonical("4").unwrap()));
    assert_eq!(val(&wb, 0, 11, 0), Value::Number(15.0));
    // A macro function keeps the saved value; a new formula shows #NAME?.
    wb.load_cell(0, CellRef::new(9, 0), Content::Formula { expr: (parse_canonical("CROUND(A1,2)").unwrap()).into(), value: Value::Number(3.0) }, 0);
    wb.recalc_all();
    assert_eq!(val(&wb, 0, 9, 0), Value::Number(3.0));
    wb.set_input(0, CellRef::new(10, 0), "=CROUND(A1,2)");
    assert_eq!(val(&wb, 0, 10, 0), Value::Error(ErrorKind::Name));
}
