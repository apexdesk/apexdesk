//! Calculation options survive xlsx and ods, and iteration applies on load.

use apex_sheet::{CalcOptions, CellRef, Workbook};

#[test]
fn calc_options_roundtrip() {
    let mut wb = Workbook::new();
    let c = |a: &str| CellRef::parse(a).unwrap();
    wb.calc = CalcOptions { manual: true, iterate: true, max_iter: 50, max_change: 0.01, date1904: false, r1c1: true, as_shown: true };
    wb.set_input(0, c("A1"), "=A2/2+10");
    wb.set_input(0, c("A2"), "=A1");
    for (name, bytes, read) in [
        ("xlsx", apex_io::xlsx::write(&wb), apex_io::xlsx::read as fn(&[u8]) -> Result<Workbook, apex_io::Error>),
        ("ods", apex_io::ods::write(&wb), apex_io::ods::read),
    ] {
        let back = read(&bytes).unwrap();
        // The reference style is an Excel setting.
        let want = CalcOptions { r1c1: name == "xlsx", ..wb.calc };
        assert_eq!(back.calc, want, "{name}");
        let v = back.sheets[0].value(c("A1")).as_number().unwrap_or(0.0);
        assert!((v - 20.0).abs() < 0.1, "{name}: {v}");
    }
    let wb = Workbook::new();
    let back = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
    assert_eq!(back.calc, CalcOptions::default());
}

#[test]
fn cell_watches_roundtrip() {
    let mut wb = Workbook::new();
    let c = |a: &str| CellRef::parse(a).unwrap();
    wb.set_input(0, c("A1"), "5");
    wb.sheets[0].cell_watches = vec![c("B2"), c("A1")];
    let bytes = apex_io::xlsx::write(&wb);
    let back = apex_io::xlsx::read(&bytes).unwrap();
    assert_eq!(back.sheets[0].cell_watches, vec![c("B2"), c("A1")]);
    // Watches move with inserted rows.
    let mut w = back;
    w.execute(apex_sheet::Op::InsertRows { sheet: 0, at: 0, count: 2 });
    assert_eq!(w.sheets[0].cell_watches, vec![c("B4"), c("A3")]);
}

#[test]
fn dates_1904_roundtrip() {
    let mut wb = Workbook::new();
    let c = |a: &str| CellRef::parse(a).unwrap();
    wb.locale = apex_sheet::Locale::EN;
    wb.set_input(0, c("A1"), "2024-03-01");
    wb.set_input(0, c("A2"), "7");
    wb.calc.date1904 = true;
    let date = wb.sheets[0].value(c("A1")).as_number().unwrap();
    let bytes = apex_io::xlsx::write(&wb);
    if let Ok(dir) = std::env::var("APEX_DUMP") {
        std::fs::write(format!("{dir}/d1904.xlsx"), &bytes).unwrap();
    }
    let back = apex_io::xlsx::read(&bytes).unwrap();
    assert!(back.calc.date1904);
    assert_eq!(back.sheets[0].value(c("A1")).as_number(), Some(date));
    assert_eq!(back.sheets[0].value(c("A2")).as_number(), Some(7.0));
    let back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
    assert!(back.calc.date1904);
    assert_eq!(back.sheets[0].value(c("A1")).as_number(), Some(date));
}

#[test]
fn custom_views_roundtrip() {
    use apex_sheet::views::Shown;
    use apex_sheet::Range;
    let mut wb = Workbook::new();
    wb.execute(apex_sheet::Op::AddSheet { index: 1, sheet: Box::new(apex_sheet::Sheet::new("Two")) });
    wb.execute(apex_sheet::Op::Freeze { sheet: 1, rows: 1, cols: 0, top: apex_sheet::CellRef::new(0, 0) });
    let shown = Shown { sheet: 1, zoom: 1.25, top_left: CellRef::new(5, 1), cursor: CellRef::new(6, 2), selection: Range::parse("C7:D9").unwrap() };
    wb.sheets[1].page.landscape = true;
    wb.add_custom_view("Review", shown, true, true);
    wb.sheets[1].page.landscape = false;
    let bytes = apex_io::xlsx::write(&wb);
    let back = apex_io::xlsx::read(&bytes).unwrap();
    assert_eq!(back.custom_views, wb.custom_views);
    let v = &back.sheets[1].custom_views[0];
    assert_eq!((v.zoom, v.top_left, v.cursor, v.frozen), (125, CellRef::new(5, 1), CellRef::new(6, 2), (1, 0)));
    assert_eq!(v.selection, Range::parse("C7:D9").unwrap());
    assert!(v.page.as_ref().unwrap().landscape);
    // The views' page setup is not taken for the sheet's.
    assert!(!back.sheets[1].page.landscape);
}

// A cell at 10 pt (one step under the default) and check boxes come back
// from ods.
#[test]
fn ods_keeps_ten_points_and_checkboxes() {
    use apex_sheet::Range;
    let mut wb = Workbook::new();
    let c = |a: &str| CellRef::parse(a).unwrap();
    wb.set_input(0, c("A1"), "small");
    wb.modify_style(0, &Range::new(c("A1"), c("A1")), |s| s.font_size = 100);
    wb.insert_checkboxes(0, &Range::new(c("B1"), c("B2")));
    let back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
    let st = |wb: &Workbook, a: &str| wb.styles.get(wb.sheets[0].get(c(a)).map_or(0, |x| x.style)).clone();
    assert_eq!(st(&back, "A1").font_size, 100);
    assert!(back.is_checkbox(0, c("B1")) && back.is_checkbox(0, c("B2")));
    // Cells at the default stay at it.
    wb.set_input(0, c("C1"), "plain");
    assert_eq!(st(&apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap(), "C1").font_size, 0);
}
