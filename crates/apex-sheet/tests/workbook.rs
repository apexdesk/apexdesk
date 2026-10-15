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

#[test]
fn recalc_chain() {
    let mut w = wb();
    w.set_input(0, c("A1"), "2");
    w.set_input(0, c("A2"), "=A1*10");
    w.set_input(0, c("A3"), "=SUM(A1:A2)+1");
    assert_eq!(show(&w, "A3"), "23");
    w.set_input(0, c("A1"), "5");
    assert_eq!(show(&w, "A2"), "50");
    assert_eq!(show(&w, "A3"), "56");
}

#[test]
fn regional_english_date_coercion() {
    for (tag, month, day) in [("en_NZ.UTF-8", 4.0, 1.0), ("en-GB", 4.0, 1.0), ("en_US.UTF-8", 1.0, 4.0)] {
        let mut w = wb();
        w.locale = Locale::from_tag(tag);
        assert_eq!(w.eval_formula(0, "=MONTH(\"1/4/2021\")"), Value::Number(month), "{tag}");
        assert_eq!(w.eval_formula(0, "=DAY(\"1/4/2021\")"), Value::Number(day), "{tag}");
        if month == 1.0 {
            continue;
        }
        for (cell, input) in [
            ("C6", "31"), ("D6", "12"), ("E6", "2020"),
            ("G6", "31"), ("H6", "3"), ("I6", "2021"),
            ("M6", "1"), ("N6", "4"), ("O6", "2021"),
            ("C8", "=C6&\"/\"&D6&\"/\"&E6"),
            ("G8", "=G6&\"/\"&H6&\"/\"&I6"),
            ("M8", "=M6&\"/\"&N6&\"/\"&O6"),
            ("Q6", "=DATE(YEAR(C8)+1,MONTH(C8),DAY(C8)-1)"),
            ("Q8", "=Q6"),
            ("C12", "=G8-C8+1"),
            ("M12", "=Q8-M8+1"),
            ("H16", "=C12+M12"),
        ] {
            w.set_input(0, c(cell), input);
        }
        w.recalc_all();
        for (cell, expected) in [("C12", "91"), ("M12", "274"), ("H16", "365")] {
            assert_eq!(show(&w, cell), expected, "{tag}: {cell}");
        }
        w.set_input(0, c("H6"), "2");
        w.set_input(0, c("G6"), "28");
        assert_eq!(show(&w, "C12"), "60");
        assert_eq!(show(&w, "H16"), "334");
    }
}

#[test]
fn range_dependency_new_cell() {
    let mut w = wb();
    w.set_input(0, c("B1"), "=SUM(A1:A100)");
    w.set_input(0, c("A50"), "7");
    assert_eq!(show(&w, "B1"), "7");
    w.set_input(0, c("C1"), "=SUM(A:A)");
    w.set_input(0, c("A200"), "3");
    assert_eq!(show(&w, "C1"), "10");
}

#[test]
fn cycles() {
    let mut w = wb();
    w.set_input(0, c("A1"), "=B1+1");
    w.set_input(0, c("B1"), "=A1+1");
    assert_eq!(show(&w, "A1"), "#CIRC!");
    w.set_input(0, c("B1"), "1");
    assert_eq!(show(&w, "A1"), "2");
}

#[test]
fn undo_redo() {
    let mut w = wb();
    w.set_input(0, c("A1"), "1");
    w.set_input(0, c("A2"), "=A1+1");
    w.set_input(0, c("A1"), "10");
    assert_eq!(show(&w, "A2"), "11");
    w.undo();
    assert_eq!(show(&w, "A2"), "2");
    w.redo();
    assert_eq!(show(&w, "A2"), "11");
}

#[test]
fn insert_delete_rows() {
    let mut w = wb();
    w.set_input(0, c("A1"), "1");
    w.set_input(0, c("A2"), "2");
    w.set_input(0, c("A3"), "=SUM(A1:A2)");
    w.execute(Op::InsertRows { sheet: 0, at: 1, count: 2 });
    assert_eq!(w.edit_text(0, c("A5")), "=SUM(A1:A4)");
    assert_eq!(show(&w, "A5"), "3");
    w.undo();
    assert_eq!(w.edit_text(0, c("A3")), "=SUM(A1:A2)");
    w.execute(Op::DeleteRows { sheet: 0, at: 0, count: 1 });
    assert_eq!(w.edit_text(0, c("A2")), "=SUM(A1:A1)");
    assert_eq!(show(&w, "A2"), "2");
    w.undo();
    assert_eq!(w.edit_text(0, c("A3")), "=SUM(A1:A2)");
    assert_eq!(show(&w, "A1"), "1");
    assert_eq!(show(&w, "A3"), "3");
}

#[test]
fn delete_referenced_cell() {
    let mut w = wb();
    w.set_input(0, c("A1"), "5");
    w.set_input(0, c("B1"), "=A1*2");
    w.execute(Op::DeleteCols { sheet: 0, at: 0, count: 1 });
    assert_eq!(show(&w, "A1"), "#REF!");
    w.undo();
    assert_eq!(w.edit_text(0, c("B1")), "=A1*2");
    assert_eq!(show(&w, "B1"), "10");
}

#[test]
fn functions_basic() {
    let mut w = wb();
    for (i, v) in ["3", "1", "4", "1", "5"].iter().enumerate() {
        w.set_input(0, CellRef::new(i as u32, 0), v);
    }
    let e = |w: &Workbook, f: &str| {
        let v = w.eval_formula(0, f);
        w.format_value(0, &v).0
    };
    assert_eq!(e(&w, "=AVERAGE(A1:A5)"), "2.8");
    assert_eq!(e(&w, "=MAX(A1:A5)-MIN(A1:A5)"), "4");
    assert_eq!(e(&w, "=COUNTIF(A1:A5,\">2\")"), "3");
    assert_eq!(e(&w, "=IF(SUM(A1:A5)>10,\"big\",\"small\")"), "big");
    assert_eq!(e(&w, "=VLOOKUP(4,A1:A5,1,FALSE)"), "4");
    assert_eq!(e(&w, "=MATCH(5,A1:A5,0)"), "5");
    assert_eq!(e(&w, "=ROUND(2.5,0)"), "3");
    assert_eq!(e(&w, "=CONCAT(\"a\",1,TRUE)"), "a1TRUE");
    assert_eq!(e(&w, "=TEXT(0.5,\"0%\")"), "50%");
    assert_eq!(e(&w, "=1/0"), "#DIV/0!");
    assert_eq!(e(&w, "=IFERROR(1/0,-1)"), "-1");
    assert_eq!(e(&w, "=2^10"), "1024");
    assert_eq!(e(&w, "=-2^2"), "4");
    assert_eq!(e(&w, "=LEN(\"hola\")&\"x\""), "4x");
}

#[test]
fn spanish_input() {
    let mut w = Workbook::new();
    w.locale = Locale::ES;
    w.set_input(0, c("A1"), "1.234,5");
    w.set_input(0, c("A2"), "=SUMA(A1;1)");
    w.set_input(0, c("A3"), "25/12/2024");
    w.set_input(0, c("A4"), "15%");
    assert_eq!(show(&w, "A2"), "1235,5");
    assert_eq!(w.edit_text(0, c("A2")), "=SUM(A1;1)");
    assert_eq!(show(&w, "A3"), "25/12/2024");
    assert_eq!(show(&w, "A4"), "15%");
    assert_eq!(w.edit_text(0, c("A4")), "15%");
}

#[test]
fn paste_offsets_formulas() {
    let mut w = wb();
    w.set_input(0, c("A1"), "1");
    w.set_input(0, c("A2"), "2");
    w.set_input(0, c("B1"), "=A1*2");
    let clip = w.copy(0, &Range::single(c("B1")));
    w.paste(0, &Range::single(c("B2")), &clip);
    assert_eq!(w.edit_text(0, c("B2")), "=A2*2");
    assert_eq!(show(&w, "B2"), "4");
}

#[test]
fn clip_goes_to_another_workbook_with_its_formats() {
    let mut a = wb();
    a.set_input(0, c("A1"), "1");
    a.set_input(0, c("A2"), "=A1*2");
    a.modify_style(0, &Range::single(c("A1")), |s| s.bold = true);
    a.modify_style(0, &Range::single(c("A2")), |s| {
        s.fill = Some(0xFFFF00);
        s.num_fmt = "0.00".into();
    });
    let clip = a.copy(0, &Range::parse("A1:A2").unwrap());
    // The other workbook has its own formats, under the same numbers.
    let mut b = wb();
    b.set_input(0, c("D4"), "x");
    b.modify_style(0, &Range::single(c("D4")), |s| s.italic = true);
    b.modify_style(0, &Range::single(c("D4")), |s| s.underline = true);
    let clip = b.adopt_clip(&clip);
    b.paste(0, &Range::single(c("B1")), &clip);
    let style = |w: &Workbook, a1: &str| w.styles.get(w.cell(0, c(a1)).unwrap().style).clone();
    assert_eq!(style(&b, "B1"), style(&a, "A1"));
    assert_eq!(style(&b, "B2"), style(&a, "A2"));
    assert_eq!(b.edit_text(0, c("B2")), "=B1*2");
    assert_eq!(show(&b, "B2"), "2.00");
    assert!(style(&b, "D4").italic);
    // In its own workbook nothing changes.
    let own = a.copy(0, &Range::parse("A1:A2").unwrap());
    assert_eq!(a.adopt_clip(&own).cells, own.cells);
}

#[test]
fn fill_series_numbers() {
    let mut w = wb();
    w.set_input(0, c("A1"), "1");
    w.set_input(0, c("A2"), "3");
    w.fill_series(0, &Range::parse("A1:A2").unwrap(), &Range::parse("A1:A5").unwrap());
    assert_eq!(show(&w, "A5"), "9");
}

#[test]
fn sort_rows() {
    let mut w = wb();
    for (i, (a, b)) in [("c", "3"), ("a", "1"), ("b", "2")].iter().enumerate() {
        w.set_input(0, CellRef::new(i as u32, 0), a);
        w.set_input(0, CellRef::new(i as u32, 1), b);
    }
    w.sort(0, &Range::parse("A1:B3").unwrap(), 0, true);
    assert_eq!(show(&w, "A1"), "a");
    assert_eq!(show(&w, "B3"), "3");
}

/// Sorting rows of a sheet used up to its last column moves the cells
/// there are, not every empty place of those rows; holes stay holes.
#[test]
fn sort_wide_rows_moves_only_cells() {
    let mut w = wb();
    for (i, v) in ["c", "a", "b"].iter().enumerate() {
        w.set_input(0, CellRef::new(i as u32, 0), v);
    }
    w.set_input(0, CellRef::new(0, 16_383), "far");
    w.set_input(0, CellRef::new(2, 5), "x");
    w.sort(0, &Range::new(CellRef::new(0, 0), CellRef::new(2, 16_383)), 0, true);
    assert_eq!(show(&w, "A1"), "a");
    assert_eq!(show(&w, "A3"), "c");
    assert_eq!(w.display(0, CellRef::new(2, 16_383)).0, "far");
    assert_eq!(show(&w, "F2"), "x");
    assert_eq!(show(&w, "F3"), "");
    assert_eq!(w.sheets[0].cell_count(), 5);
    w.undo();
    assert_eq!(show(&w, "A1"), "c");
    assert_eq!(w.display(0, CellRef::new(0, 16_383)).0, "far");
    assert_eq!(show(&w, "F3"), "x");
}

#[test]
fn cross_sheet_and_rename() {
    let mut w = wb();
    w.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("Datos")) });
    w.set_input(1, c("A1"), "42");
    w.set_input(0, c("A1"), "=Datos!A1+1");
    assert_eq!(show(&w, "A1"), "43");
    w.execute(Op::RenameSheet { index: 1, name: "Mis datos".into() });
    assert_eq!(w.edit_text(0, c("A1")), "='Mis datos'!A1+1");
    w.set_input(1, c("A1"), "1");
    assert_eq!(show(&w, "A1"), "2");
}

#[test]
fn big_sheet_is_fast() {
    let mut w = wb();
    let n = 100_000u32;
    for r in 0..n {
        w.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Number(r as f64)), 0);
        let e = apex_sheet::formula::parse_canonical(&format!("A{}*2", r + 1)).unwrap();
        w.load_cell(0, CellRef::new(r, 1), Content::Formula { expr: e.into(), value: Value::Empty }, 0);
    }
    let e = apex_sheet::formula::parse_canonical("SUM(B:B)").unwrap();
    w.load_cell(0, CellRef::new(0, 2), Content::Formula { expr: e.into(), value: Value::Empty }, 0);
    let t = std::time::Instant::now();
    w.finish_load();
    let load = t.elapsed();
    let t = std::time::Instant::now();
    w.set_input(0, c("A1"), "1000");
    let edit = t.elapsed();
    assert_eq!(show(&w, "C1"), format!("{}", (n as u64 * (n as u64 - 1)) + 2000));
    eprintln!("load+recalc {:?}, single edit {:?}", load, edit);
}

#[test]
fn dynamic_array_spill() {
    let mut w = wb();
    w.set_input(0, c("C1"), "=A3*100");
    w.set_input(0, c("A1"), "=SEQUENCE(3)");
    assert_eq!((show(&w, "A1"), show(&w, "A2"), show(&w, "A3")), ("1".into(), "2".into(), "3".into()));
    // A formula reading a spilled cell follows the anchor.
    assert_eq!(show(&w, "C1"), "300");
    assert_eq!(w.spill_range(0, c("A1")), Some(Range::new(c("A1"), c("A3"))));
    assert_eq!(w.spill_anchor(0, c("A3")), Some(c("A1")));
    w.set_input(0, c("D1"), "=SUM(A1#)");
    assert_eq!(show(&w, "D1"), "6");
    w.set_input(0, c("A1"), "=SEQUENCE(4)");
    assert_eq!(show(&w, "D1"), "10");
    assert_eq!(show(&w, "A4"), "4");
    // Something in the way blocks the spill; clearing it frees it again.
    w.set_input(0, c("A3"), "x");
    assert_eq!(show(&w, "A1"), "#SPILL!");
    assert_eq!(show(&w, "A2"), "");
    assert_eq!(show(&w, "A4"), "");
    assert_eq!(show(&w, "D1"), "#REF!");
    w.undo();
    assert_eq!(show(&w, "A1"), "1");
    assert_eq!(show(&w, "A3"), "3");
    assert_eq!(show(&w, "C1"), "300");
    // Shrinking and deleting the anchor clear the old cells.
    w.set_input(0, c("A1"), "=SEQUENCE(2)");
    assert_eq!(show(&w, "A3"), "");
    assert_eq!(show(&w, "C1"), "0");
    w.set_input(0, c("A1"), "");
    assert_eq!(show(&w, "A2"), "");
    w.undo();
    assert_eq!(show(&w, "A2"), "2");
    // Rows inserted above move the whole array.
    w.execute(Op::InsertRows { sheet: 0, at: 0, count: 1 });
    assert_eq!((show(&w, "A2"), show(&w, "A3")), ("1".into(), "2".into()));
    assert_eq!(w.edit_text(0, c("D2")), "=SUM(A2#)");
    // Two arrays can't overlap.
    w.set_input(0, c("B1"), "={1;2;3}");
    w.set_input(0, c("A10"), "=TRANSPOSE(B1:B3)");
    assert_eq!((show(&w, "A10"), show(&w, "C10")), ("1".into(), "3".into()));
    w.set_input(0, c("B9"), "=SEQUENCE(2)");
    assert_eq!(show(&w, "B9"), "#SPILL!");
    assert_eq!(show(&w, "B10"), "2");
}

#[test]
fn array_formula_text() {
    let w = wb();
    let e = formula::parse_canonical("SUM({1,2;3,4})+@A1:A3+SORT(A1#)").unwrap();
    assert_eq!(e.to_canonical(), "SUM({1,2;3,4})+@A1:A3+SORT(A1#)");
    assert_eq!(e.to_display(&Locale::ES), "=SUM({1\\2;3\\4})+@A1:A3+SORT(A1#)");
    assert_eq!(e.to_xlsx(), "SUM({1,2;3,4})+_xlfn.SINGLE(A1:A3)+_xlfn._xlws.SORT(_xlfn.ANCHORARRAY(A1))");
    assert_eq!(formula::parse_canonical(&e.to_xlsx()).unwrap(), e);
    assert_eq!(formula::parse(&e.to_display(&Locale::ES), &Locale::ES).unwrap(), e);
    let e = formula::parse_canonical("_xlfn.LET(_xlpm.x,2,_xlpm.x*3)").unwrap();
    assert_eq!(e.to_canonical(), "LET(x,2,x*3)");
    assert_eq!(e.to_xlsx(), "_xlfn.LET(_xlpm.x,2,_xlpm.x*3)");
    assert_eq!(e.to_odf(), "of:=COM.MICROSOFT.LET(_xlpm.x;2;_xlpm.x*3)");
    assert_eq!(w.eval_formula(0, "=LET(x,2,x*3)"), Value::Number(6.0));
}

#[test]
fn auditing() {
    let mut w = wb();
    w.sheets.push(Sheet::new("Other"));
    for (a, t) in [("A1", "1"), ("A2", "2"), ("B1", "=A1*2"), ("B2", "=A2*2"), ("B3", "=A3+5"), ("B4", "=A4*2"), ("C1", "=SUM(A1:A2)+Other!A1")] {
        w.set_input(0, c(a), t);
    }
    let p = w.precedents(0, c("C1"));
    assert!(p.contains(&(0, Range::new(c("A1"), c("A2")))));
    assert!(p.contains(&(1, Range::new(c("A1"), c("A1")))));
    assert_eq!(w.dependents(0, c("A1")), vec![(0, c("B1")), (0, c("C1"))]);
    assert!(w.inconsistent_formula(0, c("B3")));
    assert!(!w.inconsistent_formula(0, c("B2")));
    // Whole columns shrink to the used area.
    w.set_input(0, c("D1"), "=SUM(A:A)");
    assert_eq!(w.precedents(0, c("D1")), vec![(0, Range::new(c("A1"), c("A4")))]);
    w.set_input(0, c("E1"), "=WEEKDAY(45000,11)&WEEKDAY(45000,17)&WEEKDAY(45000,1)");
    assert_eq!(show(&w, "E1"), "344");
    w.set_input(0, c("E2"), "=WEEKDAY(45000,9)");
    assert_eq!(show(&w, "E2"), "#NUM!");
}

#[test]
fn legacy_implicit_intersection() {
    let e = formula::parse_canonical("T(J1:J3)&VLOOKUP(A1:A3,B1:C3,2,FALSE)+SUM(A1:A3)+A1:A3").unwrap().legacy();
    assert_eq!(e.to_canonical(), "T(@J1:J3)&VLOOKUP(@A1:A3,B1:C3,2,FALSE)+SUM(A1:A3)+@A1:A3");
    let mut w = wb();
    for (a, t) in [("J1", "x"), ("J2", "y"), ("J3", "z")] {
        w.set_input(0, c(a), t);
    }
    w.set_input(0, c("K2"), "=T(@J1:J3)");
    assert_eq!(show(&w, "K2"), "y");
}

#[test]
fn empty_cell_comparisons() {
    let mut w = wb();
    for (a1, f) in [("A1", "=B1=\"\""), ("A2", "=B1=0"), ("A3", "=B1=FALSE"), ("A4", "=IF(B1=\"\",\"-\",B1*2)"), ("A5", "=B1&\"x\""), ("A6", "=B1+1"), ("A7", "=B1<>\"\"")] {
        w.set_input(0, c(a1), f);
    }
    assert_eq!(show(&w, "A1"), "TRUE");
    assert_eq!(show(&w, "A2"), "TRUE");
    assert_eq!(show(&w, "A3"), "TRUE");
    assert_eq!(show(&w, "A4"), "-");
    assert_eq!(show(&w, "A5"), "x");
    assert_eq!(show(&w, "A6"), "1");
    assert_eq!(show(&w, "A7"), "FALSE");
    w.set_input(0, c("B1"), "3");
    assert_eq!(show(&w, "A1"), "FALSE");
    assert_eq!(show(&w, "A4"), "6");
}

#[test]
fn many_formulas_reading_one_range_of_formulas() {
    // Like a sheet of VLOOKUPs over a table of formulas: the range is one
    // node of the evaluation order (not an edge per pair), and every
    // reader still comes after the formulas it reads.
    let mut w = wb();
    let n = 2000;
    w.set_input(0, CellRef::new(0, 0), "1");
    for r in 1..n {
        w.set_input(0, CellRef::new(r, 0), &format!("=A{r}+1"));
    }
    for r in 0..n {
        w.set_input(0, CellRef::new(r, 1), &format!("=VLOOKUP({},$A$1:$A${n},1,FALSE)+SUM($A$1:$A$3)", r + 1));
    }
    w.set_input(0, c("C1"), &format!("=SUM(B1:B{n})"));
    w.recalc_all();
    assert_eq!(show(&w, &format!("B{n}")), format!("{}", n + 6));
    let want: u32 = (1..=n).map(|r| r + 6).sum();
    assert_eq!(show(&w, "C1"), want.to_string());
}

#[test]
fn many_lookups_into_a_linked_book() {
    // Thousands of VLOOKUPs into the saved values of a linked table (one
    // array for all of them while recalculating, not one per formula).
    let mut w = wb();
    let rows = 5000u32;
    let cells = (0..rows).flat_map(|r| (0..20).map(move |c| (CellRef::new(r, c), Value::Number(f64::from(r * 100 + c))))).collect();
    w.externals.push(ExternalBook { target: "Other.xlsx".into(), sheets: vec!["Data".into()], names: Vec::new(), cells: vec![cells] });
    for r in 0..2000 {
        w.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Number(f64::from(r * 200))), 0);
        let expr = formula::parse(&format!("VLOOKUP(A{},[1]Data!$A$1:$T${rows},20,FALSE)", r + 1), &Locale::EN).unwrap();
        w.load_cell(0, CellRef::new(r, 1), Content::Formula { expr: (expr).into(), value: Value::Empty }, 0);
    }
    let t = std::time::Instant::now();
    w.finish_load();
    assert!(t.elapsed().as_secs() < 30);
    assert_eq!(show(&w, "B2"), "219");
    assert_eq!(show(&w, "B2000"), "399819");
}

#[test]
fn calculation_options_undo() {
    let mut wb = Workbook::new();
    let c = |a: &str| CellRef::parse(a).unwrap();
    wb.set_input(0, c("A1"), "1");
    wb.set_input(0, c("B1"), "=A1*2");
    wb.execute(apex_sheet::Op::Calc(apex_sheet::CalcOptions { manual: true, ..wb.calc }));
    wb.set_input(0, c("A1"), "5");
    assert_eq!(*wb.sheets[0].value(c("B1")), apex_sheet::Value::Number(2.0));
    // Back to automatic: the formulas catch up; undone, manual again.
    wb.execute(apex_sheet::Op::Calc(apex_sheet::CalcOptions { manual: false, ..wb.calc }));
    assert_eq!(*wb.sheets[0].value(c("B1")), apex_sheet::Value::Number(10.0));
    wb.undo();
    assert!(wb.calc.manual);
    wb.undo();
    wb.undo();
    assert!(!wb.calc.manual);
}

/// A book with many formulas is calculated on several threads, the
/// formulas ready at once together; its graph is built meanwhile.
#[test]
fn many_formulas_load_calculated_and_linked() {
    const N: u32 = 30_000;
    let mut w = Workbook::new();
    let formula = |w: &mut Workbook, c: CellRef, text: &str| {
        let e = apex_sheet::formula::parse_canonical(text).unwrap();
        w.load_cell(0, c, Content::Formula { expr: e.into(), value: Value::Empty }, 0);
    };
    for r in 0..N {
        w.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Number(r as f64 + 1.0)), 0);
        let n = r + 1;
        formula(&mut w, CellRef::new(r, 1), &format!("A{n}*2"));
        formula(&mut w, CellRef::new(r, 2), &format!("B{n}+A{n}"));
        formula(&mut w, CellRef::new(r, 3), &format!("IF(C{n}>100,\"big\",\"small\")"));
    }
    formula(&mut w, CellRef::new(0, 4), "SUMIF(A:A,\">10\",C:C)");
    formula(&mut w, CellRef::new(1, 4), "COUNTIF(D:D,\"big\")");
    formula(&mut w, CellRef::new(2, 4), "SUMIF(D:D,\"small\",A:A)");
    w.finish_load();
    let v = |w: &Workbook, r, c| w.sheets[0].value(CellRef::new(r, c)).clone();
    for r in 0..N {
        let n = r as f64 + 1.0;
        assert_eq!(v(&w, r, 2), Value::Number(3.0 * n), "C{}", r + 1);
        assert_eq!(v(&w, r, 3), Value::Text((if 3.0 * n > 100.0 { "big" } else { "small" }).into()), "D{}", r + 1);
    }
    let total: f64 = (11..=N).map(|n| 3.0 * n as f64).sum();
    assert_eq!(v(&w, 0, 4), Value::Number(total));
    assert_eq!(v(&w, 1, 4), Value::Number((N - 33) as f64));
    assert_eq!(v(&w, 2, 4), Value::Number(561.0));
    // What depends on a cell is known: an edit recalculates it.
    w.set_input(0, CellRef::new(0, 0), "100");
    assert_eq!(v(&w, 0, 2), Value::Number(300.0));
    assert_eq!(v(&w, 1, 4), Value::Number((N - 32) as f64));
    assert_eq!(v(&w, 2, 4), Value::Number(560.0));
}
