//! Formulas that calculate as in Excel: cases found by comparing with
//! the values Excel saved in real workbooks (apex-bench oracle).

use apex_sheet::*;

fn wb() -> Workbook {
    let mut w = Workbook::new();
    w.locale = Locale::EN;
    w
}

fn c(a1: &str) -> CellRef {
    CellRef::parse(a1).unwrap()
}

fn show(w: &Workbook, a1: &str) -> String {
    w.display(0, c(a1)).0
}

fn value(w: &Workbook, a1: &str) -> Value {
    w.sheets[0].value(c(a1)).clone()
}

#[test]
fn leading_plus_keeps_text() {
    let mut w = wb();
    w.set_input(0, c("A1"), "abc");
    w.set_input(0, c("B1"), "=+A1");
    w.set_input(0, c("B2"), "=+CONCATENATE(A1,\"d\")");
    w.set_input(0, c("B3"), "=+C9");
    w.set_input(0, c("B4"), "=+TRUE");
    assert_eq!(value(&w, "B1"), Value::text("abc"));
    assert_eq!(value(&w, "B2"), Value::text("abcd"));
    assert_eq!(value(&w, "B3"), Value::Number(0.0));
    assert_eq!(show(&w, "B4"), "TRUE");
}

#[test]
fn order_found_while_calculating() {
    let mut w = wb();
    // The base of OFFSET is the cell itself: not a cycle.
    w.set_input(0, c("A1"), "1");
    for r in 2..=5 {
        w.set_input(0, c(&format!("A{r}")), &format!("=1+OFFSET(A{r},-1,0)"));
    }
    // ROW of a range around the cell reads no value.
    w.set_input(0, c("B5"), "=SUM(ROW(B1:B9))");
    // INDIRECT reaches a formula calculated later.
    w.set_input(0, c("E1"), "=INDIRECT(\"E3\")*2");
    w.set_input(0, c("E3"), "=E4+1");
    w.set_input(0, c("E4"), "5");
    // Real cycles, written and reached by OFFSET.
    w.set_input(0, c("C1"), "=C2+1");
    w.set_input(0, c("C2"), "=C1+1");
    w.set_input(0, c("D1"), "=OFFSET(D2,-1,0)");
    w.recalc_all();
    assert_eq!(value(&w, "A5"), Value::Number(5.0));
    assert_eq!(value(&w, "B5"), Value::Number(45.0));
    assert_eq!(value(&w, "E1"), Value::Number(12.0));
    for a in ["C1", "C2", "D1"] {
        assert_eq!(value(&w, a), Value::Error(ErrorKind::Circular), "{a}");
    }
    assert!(w.circular_cell().is_some());
    w.set_input(0, c("E4"), "6");
    assert_eq!(value(&w, "E1"), Value::Number(14.0));
    // Numbering that skips blank rows, looking back with OFFSET.
    let mut w = wb();
    w.set_input(0, c("A1"), "No.");
    for r in [2, 3, 5, 6] {
        let f = format!("=1+IF(ISNUMBER(OFFSET(A{r},-1,0)),OFFSET(A{r},-1,0),IF(ISNUMBER(OFFSET(A{r},-2,0)),OFFSET(A{r},-2,0),0))");
        w.set_input(0, c(&format!("A{r}")), &f);
    }
    w.recalc_all();
    assert_eq!(value(&w, "A6"), Value::Number(4.0));
}

#[test]
fn empty_cells_are_not_numbers() {
    let mut w = wb();
    w.set_input(0, c("A2"), "5");
    w.set_input(0, c("J1"), "=1");
    for (a, f) in [("B1", "=ISNUMBER(A1)"), ("B2", "=ISNUMBER(A2)"), ("B3", "=ISNUMBER(OFFSET(A2,-1,0))"), ("B4", "=ISBLANK(INDEX(A1:A3,3))"), ("B5", "=ISNUMBER(INDEX(A1:A3,2))"), ("J2", "=ISBLANK(@K1:K5)")] {
        w.set_input(0, c(a), f);
    }
    assert_eq!(show(&w, "B1"), "FALSE");
    assert_eq!(show(&w, "B2"), "TRUE");
    assert_eq!(show(&w, "B3"), "FALSE");
    assert_eq!(show(&w, "B4"), "TRUE");
    assert_eq!(show(&w, "B5"), "TRUE");
    assert_eq!(show(&w, "J2"), "TRUE");
}

#[test]
fn criteria_numbers_match_numeric_text() {
    let mut w = wb();
    for (a, t) in [("A1", "'003607"), ("A2", "3607"), ("A3", "'3608"), ("A4", "x"), ("B1", "10"), ("B2", "20"), ("B3", "40"), ("B4", "80")] {
        w.set_input(0, c(a), t);
    }
    w.set_input(0, c("C1"), "=SUMIF(A1:A4,\"003607\",B1:B4)");
    w.set_input(0, c("C2"), "=COUNTIF(A1:A4,3608)");
    w.set_input(0, c("C3"), "=COUNTIF(A1:A4,\">3606\")");
    w.set_input(0, c("C4"), "=MATCH(3607,A1:A4,0)");
    assert_eq!(value(&w, "A1"), Value::text("003607"));
    assert_eq!(value(&w, "C1"), Value::Number(30.0));
    assert_eq!(value(&w, "C2"), Value::Number(1.0));
    assert_eq!(value(&w, "C3"), Value::Number(1.0));
    // Lookups don't: the number is in A2.
    assert_eq!(value(&w, "C4"), Value::Number(2.0));
}

#[test]
fn empty_arguments_are_zero() {
    let mut w = wb();
    for (a, t) in [("A1", "a"), ("A2", "b"), ("A3", "c"), ("B1", "1"), ("B2", "2"), ("B3", "3"), ("E1", "1")] {
        w.set_input(0, c(a), t);
    }
    w.set_input(0, c("C1"), "=MATCH(\"b\",A1:A3,)");
    w.set_input(0, c("C2"), "=VLOOKUP(\"b\",A1:B3,2,)");
    w.set_input(0, c("C3"), "=OR(E26,)");
    w.set_input(0, c("C4"), "=AND(30)");
    w.set_input(0, c("C5"), "=AND(E1,)");
    w.set_input(0, c("C6"), "=OR(E1,)");
    assert_eq!(value(&w, "C1"), Value::Number(2.0));
    assert_eq!(value(&w, "C2"), Value::Number(2.0));
    assert_eq!(show(&w, "C3"), "FALSE");
    assert_eq!(show(&w, "C4"), "TRUE");
    assert_eq!(show(&w, "C5"), "FALSE");
    assert_eq!(show(&w, "C6"), "TRUE");
}

#[test]
fn dates_before_march_1900() {
    let mut w = wb();
    for (a, f) in [
        ("A1", "=TEXT(0,\"mm/dd/yyyy\")"),
        ("A2", "=DATEVALUE(TEXT(B9,\"mm/dd/yy\"))"),
        ("A3", "=YEAR(0)"),
        ("A4", "=DAY(60)"),
        ("A5", "=DATE(1900,1,1)"),
        ("A6", "=DATE(1900,3,1)"),
        ("A7", "=DATE(1900,2,29)"),
        ("A8", "=TEXT(59,\"d/m/yyyy\")"),
        ("A9", "=DATEVALUE(\"1/15/1900\")"),
        ("B1", "=EOMONTH(0,0)"),
        ("B2", "=EDATE(0,1)"),
        ("B3", "=EOMONTH(15,1)"),
        ("B4", "=EDATE(45000,-1)"),
        ("B5", "=EOMONTH(32,0)"),
        ("B6", "=EOMONTH(60,0)"),
        ("B7", "=EDATE(31,1)"),
    ] {
        w.set_input(0, c(a), f);
    }
    assert_eq!(value(&w, "A1"), Value::text("01/00/1900"));
    assert_eq!(value(&w, "A2"), Value::Error(ErrorKind::Value));
    assert_eq!(value(&w, "A3"), Value::Number(1900.0));
    assert_eq!(value(&w, "A4"), Value::Number(29.0));
    assert_eq!(value(&w, "A5"), Value::Number(1.0));
    assert_eq!(value(&w, "A6"), Value::Number(61.0));
    assert_eq!(value(&w, "A7"), Value::Number(60.0));
    assert_eq!(value(&w, "A8"), Value::text("28/2/1900"));
    assert_eq!(value(&w, "A9"), Value::Number(15.0));
    assert_eq!(value(&w, "B1"), Value::Number(31.0));
    assert_eq!(value(&w, "B2"), Value::Number(31.0));
    assert_eq!(value(&w, "B3"), Value::Number(59.0));
    assert_eq!(value(&w, "B4"), Value::Number(44972.0));
    // EDATE and EOMONTH keep February 1900 at 28 days.
    assert_eq!(value(&w, "B5"), Value::Number(59.0));
    assert_eq!(value(&w, "B6"), Value::Number(59.0));
    assert_eq!(value(&w, "B7"), Value::Number(59.0));
}

#[test]
fn plain_formulas_take_one_cell_of_named_ranges() {
    let mut w = wb();
    for (a, t) in [("A1", "3"), ("A2", "5"), ("A3", "7"), ("C1", "4")] {
        w.set_input(0, c(a), t);
    }
    w.names.push(DefinedName::new("Col", None, formula::parse_canonical("Sheet1!$A$1:$A$3").unwrap()));
    w.names.push(DefinedName::new("Lim", None, formula::parse_canonical("Sheet1!$C$1").unwrap()));
    // As read from a file: formulas that predate dynamic arrays.
    for (a, f) in [("B1", "IF(Col<=Lim,1,2)"), ("B2", "IF(Col<=Lim,1,2)"), ("B3", "Col*2"), ("B4", "SUM(Col)")] {
        let e = formula::parse_canonical(f).unwrap().legacy();
        w.load_cell(0, c(a), Content::Formula { expr: (e).into(), value: Value::Empty }, 0);
    }
    w.finish_load();
    assert_eq!(value(&w, "B1"), Value::Number(1.0));
    assert_eq!(value(&w, "B2"), Value::Number(2.0));
    assert_eq!(value(&w, "B3"), Value::Number(14.0));
    assert_eq!(value(&w, "B4"), Value::Number(15.0));
    assert_eq!(w.sheets[0].get(c("B1")).unwrap().formula().unwrap().expr().to_canonical(), "IF(@Col<=Lim,1,2)");
}

#[test]
fn range_operator_between_any_references() {
    let mut w = wb();
    for (a, t) in [("A1", "1"), ("A2", "2"), ("A3", "4"), ("A4", "8"), ("B1", "3")] {
        w.set_input(0, c(a), t);
    }
    w.names.push(DefinedName::new("Last", None, formula::parse_canonical("Sheet1!$A$3").unwrap()));
    w.names.push(DefinedName::new("Col", None, formula::parse_canonical("Sheet1!$A$1:Last").unwrap()));
    for (a, f) in [
        ("C1", "=SUM(A1:INDEX(A1:A4,B1))"),
        ("C2", "=SUM(INDEX(A1:A4,2):INDEX(A1:A4,4))"),
        ("C3", "=SUM(Col)"),
        ("C4", "=SUM(A2:OFFSET(A1,3,0))"),
        ("C5", "=ROWS(A1:Last)"),
        ("C6", "=SUM(A1:A2)"),
    ] {
        w.set_input(0, c(a), f);
    }
    assert_eq!(value(&w, "C1"), Value::Number(7.0));
    assert_eq!(value(&w, "C2"), Value::Number(14.0));
    assert_eq!(value(&w, "C3"), Value::Number(7.0));
    assert_eq!(value(&w, "C4"), Value::Number(14.0));
    assert_eq!(value(&w, "C5"), Value::Number(3.0));
    assert_eq!(value(&w, "C6"), Value::Number(3.0));
    let e = formula::parse_canonical("SUM($A$1:INDEX(A:A,B1),Start:End)").unwrap();
    assert_eq!(e.to_canonical(), "SUM($A$1:INDEX(A:A,B1),Start:End)");
    // A formula over the range is recalculated when a cell in it changes.
    w.set_input(0, c("A2"), "20");
    assert_eq!(value(&w, "C3"), Value::Number(25.0));
}

#[test]
fn plain_formulas_calculate_one_value_in_reference_arguments() {
    let legacy = |f: &str| formula::parse_canonical(f).unwrap().legacy().to_canonical();
    // SUM and CHOOSE take ranges whole, but not what is calculated from them.
    assert_eq!(legacy("SUM(A1:A3)+SUM(A1:A3*2)"), "SUM(A1:A3)+SUM(@A1:A3*2)");
    assert_eq!(legacy("-CHOOSE(1,A1-B1:B2,B1:B2)"), "-CHOOSE(1,A1-@B1:B2,B1:B2)");
    // SUMPRODUCT and LOOKUP's vectors are calculated for whole arrays,
    // functions inside them too, but IF.
    assert_eq!(legacy("SUMPRODUCT(ABS(A1:A3)*(B1:B3>0))"), "SUMPRODUCT(ABS(A1:A3)*(B1:B3>0))");
    assert_eq!(legacy("SUMPRODUCT(IF(A1:A3>0,1,0))"), "SUMPRODUCT(IF(@A1:A3>0,1,0))");
    assert_eq!(legacy("LOOKUP(2,1/(A1:A9<>\"\"),B1:B9)"), "LOOKUP(2,1/(A1:A9<>\"\"),B1:B9)");
    let mut w = wb();
    for (a, t) in [("A1", "1"), ("A2", "2"), ("A3", "3"), ("B2", "10")] {
        w.set_input(0, c(a), t);
    }
    let e = formula::parse_canonical("SUM(A1:A3*2)").unwrap().legacy();
    w.load_cell(0, c("B2"), Content::Formula { expr: (e).into(), value: Value::Empty }, 0);
    w.finish_load();
    assert_eq!(value(&w, "B2"), Value::Number(4.0));
}

#[test]
fn plain_formulas_show_the_first_value_of_arrays() {
    let legacy = |f: &str| formula::parse_canonical(f).unwrap().legacy();
    assert_eq!(legacy("ROW(A1)+OFFSET(A1,1,0)").to_canonical(), "ROW(A1)+OFFSET(A1,1,0)");
    let mut w = wb();
    for (a, t) in [("A1", "1"), ("A2", "5"), ("A3", "7"), ("B1", "2"), ("B2", "6")] {
        w.set_input(0, c(a), t);
    }
    // As read from a file: one cell each, not array formulas.
    for (a, f) in [
        ("D1", "FREQUENCY(A1:A3,B1:B2)"),
        ("D2", "IF(A1=0,1,LINEST(A1:A3))"),
        ("D3", "ROW(A2:A3)"),
        ("D4", "TRANSPOSE(A1:A3)*2"),
        ("D5", "SUM(TRANSPOSE(A1:A3))"),
    ] {
        let e = formula::parse_canonical(f).unwrap().legacy();
        w.load_cell(0, c(a), Content::Formula { expr: (e).into(), value: Value::Empty }, 0);
    }
    w.finish_load();
    assert_eq!(value(&w, "D1"), Value::Number(1.0));
    assert_eq!(value(&w, "D2"), Value::Number(3.0));
    assert_eq!(value(&w, "D3"), Value::Number(2.0));
    assert_eq!(value(&w, "D4"), Value::Number(2.0));
    assert_eq!(value(&w, "D5"), Value::Number(13.0));
    assert!(w.sheets[0].get(c("E1")).is_none());
}

#[test]
fn search_with_wildcards() {
    let mut w = wb();
    w.set_input(0, c("A1"), "Adams, C.F. 2005. Physical effects");
    for (a, f) in [
        ("B1", "=SEARCH(\"200?.\",A1)"),
        ("B2", "=VALUE(MID(A1,SEARCH(\"200?.\",A1),4))"),
        ("B3", "=SEARCH(\"p*l\",A1)"),
        ("B4", "=SEARCH(\"~?\",\"why?\")"),
        ("B5", "=FIND(\"?\",\"why?\")"),
        ("B6", "=SEARCH(\"x?z\",A1)"),
    ] {
        w.set_input(0, c(a), f);
    }
    assert_eq!(value(&w, "B1"), Value::Number(13.0));
    assert_eq!(value(&w, "B2"), Value::Number(2005.0));
    assert_eq!(value(&w, "B3"), Value::Number(19.0));
    assert_eq!(value(&w, "B4"), Value::Number(4.0));
    assert_eq!(value(&w, "B5"), Value::Number(4.0));
    assert_eq!(value(&w, "B6"), Value::Error(ErrorKind::Value));
}

#[test]
fn lookups_pass_on_the_error_they_look_for() {
    let mut w = wb();
    w.set_input(0, c("A1"), "=1/0");
    for (a, t) in [("B1", "1"), ("B2", "2"), ("C1", "x"), ("C2", "y")] {
        w.set_input(0, c(a), t);
    }
    w.set_input(0, c("D1"), "=VLOOKUP(A1,B1:C2,2,TRUE)");
    w.set_input(0, c("D2"), "=HLOOKUP(A1,B1:C2,2,TRUE)");
    w.set_input(0, c("D3"), "=MATCH(A1,B1:B2,0)");
    w.set_input(0, c("D4"), "=XLOOKUP(A1,B1:B2,C1:C2)");
    for a in ["D1", "D2", "D3", "D4"] {
        assert_eq!(value(&w, a), Value::Error(ErrorKind::Div0), "{a}");
    }
}

#[test]
fn sums_that_cancel_out_are_zero() {
    let mut w = wb();
    for (a, t) in [("A1", "=0.5-0.4-0.1"), ("A2", "=1*(0.5-0.4-0.1)"), ("A3", "=(0.5-0.4-0.1)"), ("A4", "=1E-20-0"), ("B1", "2345678.9"), ("B2", "=B1/3*3"), ("A5", "=B2-B1")] {
        w.set_input(0, c(a), t);
    }
    assert_eq!(value(&w, "A1"), Value::Number(0.0));
    assert!(matches!(value(&w, "A2"), Value::Number(n) if n != 0.0));
    assert!(matches!(value(&w, "A3"), Value::Number(n) if n != 0.0));
    assert_eq!(value(&w, "A4"), Value::Number(1e-20));
    assert_eq!(value(&w, "A5"), Value::Number(0.0));
}

#[test]
fn text_read_as_numbers_and_dates() {
    let mut w = wb();
    for (a, f) in [
        ("A1", "31-Dec-2003"),
        ("B1", "=A1+0"),
        ("B2", "=\"1,234.5\"*2"),
        ("B3", "=\"10%\"+1"),
        ("B4", "=(\"1/2/2003\")-1"),
        ("B5", "=\"abc\"+1"),
        ("B6", "=\"TRUE\"+0"),
        ("B7", "=\"12:00\"*2"),
    ] {
        w.set_input(0, c(a), f);
    }
    w.set_input(0, c("A1"), "'31-Dec-2003");
    assert_eq!(value(&w, "B1"), Value::Number(37986.0));
    assert_eq!(value(&w, "B2"), Value::Number(2469.0));
    assert_eq!(value(&w, "B3"), Value::Number(1.1));
    assert_eq!(value(&w, "B4"), Value::Number(37622.0));
    assert_eq!(value(&w, "B5"), Value::Error(ErrorKind::Value));
    assert_eq!(value(&w, "B6"), Value::Error(ErrorKind::Value));
    assert_eq!(value(&w, "B7"), Value::Number(1.0));
}

#[test]
fn empty_cells_as_text_are_empty() {
    let mut w = wb();
    w.set_input(0, c("A1"), "John");
    w.set_input(0, c("C1"), "Doe");
    w.set_input(0, c("D1"), "=CONCATENATE(A1,\" \",B1,\" \",C1)");
    w.set_input(0, c("D2"), "=TRIM(B1)=\"\"");
    w.set_input(0, c("D3"), "=LEN(OFFSET(A1,0,1))");
    w.set_input(0, c("D4"), "=B1+1");
    w.names.push(DefinedName::new("Blank", None, formula::parse_canonical("Sheet1!$B$1").unwrap()));
    w.set_input(0, c("D5"), "=IF(+B1=\"\",\"\",+B1)");
    w.set_input(0, c("D6"), "=(B1)&\"/\"&Blank");
    w.set_input(0, c("D7"), "=@B:B&\"/\"&@B1:B9");
    assert_eq!(value(&w, "D1"), Value::text("John  Doe"));
    assert_eq!(show(&w, "D2"), "TRUE");
    assert_eq!(value(&w, "D3"), Value::Number(0.0));
    assert_eq!(value(&w, "D4"), Value::Number(1.0));
    assert_eq!(value(&w, "D5"), Value::text(""));
    assert_eq!(value(&w, "D6"), Value::text("/"));
    assert_eq!(value(&w, "D7"), Value::text("/"));
}

#[test]
fn approximate_lookups_search_by_halves() {
    let mut w = wb();
    for (a, t) in [("A1", "Scenario"), ("B1", "A"), ("C1", "B"), ("D1", "C"), ("A2", "1"), ("B2", "2"), ("C2", "3"), ("D2", "4")] {
        w.set_input(0, c(a), t);
    }
    for (a, t) in [("A4", "1"), ("A5", "#N/A"), ("A6", "1"), ("A7", "x"), ("A8", "3"), ("A9", "5")] {
        w.set_input(0, c(a), t);
    }
    // Unsorted: Excel's halving finds "A" after "Scenario".
    w.set_input(0, c("F1"), "=HLOOKUP(\"A\",A1:D2,2)");
    w.set_input(0, c("F2"), "=MATCH(\"C\",A1:D1)");
    // Sorted with errors and text between: the last value not above.
    w.set_input(0, c("F3"), "=MATCH(4,A4:A9)");
    w.set_input(0, c("F4"), "=LOOKUP(2,1/(A4:A9<>\"x\"),A4:A9)");
    assert_eq!(value(&w, "F1"), Value::Number(2.0));
    assert_eq!(value(&w, "F2"), Value::Number(4.0));
    assert_eq!(value(&w, "F3"), Value::Number(5.0));
    assert_eq!(value(&w, "F4"), Value::Number(5.0));
}

#[test]
fn lotus_rules_only_on_their_sheet() {
    let mut w = wb();
    w.sheets.push(Sheet::new("Lotus"));
    w.sheets[1].lotus_eval = true;
    w.set_input(0, c("A1"), "-");
    w.set_input(1, c("A1"), "=Sheet1!A1+70");
    w.set_input(0, c("B1"), "=A1+70");
    w.set_input(1, c("A2"), "=\"x\"*2+\"\"");
    assert_eq!(w.sheets[1].value(c("A1")), &Value::Number(70.0));
    assert_eq!(w.sheets[1].value(c("A2")), &Value::Number(0.0));
    assert_eq!(value(&w, "B1"), Value::Error(ErrorKind::Value));
}

#[test]
fn lookups_of_empty_cells() {
    let mut w = wb();
    for (a, t) in [("A1", "a"), ("A2", "b"), ("B1", "5")] {
        w.set_input(0, c(a), t);
    }
    for (a, f) in [
        ("D1", "=VLOOKUP(\"b\",A1:B2,2,FALSE)"),
        ("D2", "=ISBLANK(VLOOKUP(\"b\",A1:B2,2))"),
        ("D3", "=VLOOKUP(\"b\",A1:B2,2,FALSE)&\"\""),
        ("D4", "=VLOOKUP(\"b\",A1:B2,2,FALSE)+1"),
        ("D5", "=XLOOKUP(\"b\",A1:A2,B1:B2)=\"\""),
        ("D6", "=IF(TRUE,HLOOKUP(\"a\",A1:B2,2,FALSE))"),
        ("D7", "=LOOKUP(\"b\",A1:B2)&\"/\"&INDEX(B1:B2,2)&\"/\"&OFFSET(B1,1,0)"),
    ] {
        w.set_input(0, c(a), f);
    }
    assert_eq!(value(&w, "D1"), Value::Number(0.0));
    assert_eq!(show(&w, "D2"), "TRUE");
    assert_eq!(value(&w, "D3"), Value::text(""));
    assert_eq!(value(&w, "D4"), Value::Number(1.0));
    assert_eq!(show(&w, "D5"), "TRUE");
    assert_eq!(value(&w, "D6"), Value::text("b"));
    assert_eq!(value(&w, "D7"), Value::text("//"));
}

#[test]
fn precision_as_displayed() {
    let mut w = wb();
    w.calc.as_shown = true;
    for (a, f) in [("B1", "0.00"), ("B2", "0.0%"), ("B3", "#,##0,"), ("B4", "General")] {
        w.modify_style(0, &Range::single(c(a)), |s| s.num_fmt = f.into());
    }
    for a in ["B1", "B2", "B3", "B4"] {
        w.set_input(0, c(a), "=1234.5678/3");
    }
    w.set_input(0, c("C1"), "=B1*3");
    assert_eq!(value(&w, "B1"), Value::Number(411.52));
    assert_eq!(value(&w, "B2"), Value::Number(411.523));
    assert_eq!(value(&w, "B3"), Value::Number(0.0));
    assert_eq!(value(&w, "B4"), Value::Number(1234.5678 / 3.0));
    assert!((value(&w, "C1").as_number().unwrap() - 1234.56).abs() < 1e-9);
}

#[test]
fn array_formulas_of_files_keep_their_size() {
    let mut w = wb();
    for (a, t) in [("A1", "1"), ("A2", "5"), ("A3", "7"), ("B1", "2"), ("B2", "6"), ("D2", "x")] {
        w.set_input(0, c(a), t);
    }
    // As read from a file: {=...} over the area given, values saved.
    let areas = [("D1", "D1", "FREQUENCY(A1:A3,B1:B2)"), ("E1", "F2", "TRANSPOSE(B1:B2)"), ("G1", "G3", "B1:B2*10"), ("H1", "H2", "SUM(A1:A3)")];
    for (a, _, f) in areas {
        let e = formula::parse_canonical(f).unwrap();
        w.load_cell(0, c(a), Content::Formula { expr: (e).into(), value: Value::Empty }, 0);
    }
    for (a, b, _) in areas {
        w.sheets[0].mark_fixed_array(&Range::new(c(a), c(b)));
    }
    w.finish_load();
    // Cut to one cell though D2 is taken.
    assert_eq!(value(&w, "D1"), Value::Number(1.0));
    // One row repeated down; a missing row is #N/A; a value everywhere.
    assert_eq!(value(&w, "F2"), Value::Number(6.0));
    assert_eq!(value(&w, "G2"), Value::Number(60.0));
    assert_eq!(value(&w, "G3"), Value::Error(ErrorKind::Na));
    assert_eq!(value(&w, "H2"), Value::Number(13.0));
    // A formula typed again spills as usual.
    w.set_input(0, c("G1"), "=B1:B2*10");
    assert_eq!(value(&w, "G3"), Value::Empty);
}

#[test]
fn choose_and_if_give_references() {
    let mut w = wb();
    for (i, a) in ["A", "B"].iter().enumerate() {
        for r in 1..=3 {
            w.set_input(0, c(&format!("{a}{r}")), &format!("{}", (i + 1) * 10 + r));
        }
    }
    w.set_input(0, c("D1"), "=OFFSET(CHOOSE(2,A1:A3,B1:B3),1,0,1,1)");
    w.set_input(0, c("D2"), "=SUM(OFFSET(IF(D9=0,A1,B1),0,0,3))");
    w.set_input(0, c("D3"), "=ROWS(CHOOSE(1,A1:A3,B1))");
    w.set_input(0, c("D4"), "=OFFSET(CHOOSE(3,A1,B1),0,0)");
    assert_eq!(value(&w, "D1"), Value::Number(22.0));
    assert_eq!(value(&w, "D2"), Value::Number(36.0));
    assert_eq!(value(&w, "D3"), Value::Number(3.0));
    assert_eq!(value(&w, "D4"), Value::Error(ErrorKind::Value));
}

#[test]
fn texts_stop_at_32767_characters() {
    let w = wb();
    let ev = |f: &str| w.eval_formula(0, f);
    assert_eq!(ev("=LEN(REPT(\"x\",32767))"), Value::Number(32767.0));
    // Checked before building: this would ask for terabytes.
    assert_eq!(ev("=REPT(REPT(REPT(\"x\",32767),32767),32767)"), Value::Error(ErrorKind::Value));
    assert_eq!(ev("=REPT(\"x\",32767)&\"y\""), Value::Error(ErrorKind::Value));
    assert_eq!(ev("=SUBSTITUTE(REPT(\"x\",1000),\"x\",REPT(\"y\",100))"), Value::Error(ErrorKind::Value));
    assert_eq!(ev("=LEN(CONCAT(REPT(\"é\",20000),REPT(\"é\",20000)))"), Value::Error(ErrorKind::Value));
}
