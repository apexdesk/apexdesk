//! Frozen panes survive xlsx and ods: at a cell, the top row, the first
//! column, both, and frozen on a scrolled sheet (the panes then start at
//! a row and column past the first).

use apex_sheet::{CellRef, Op, Workbook};

#[test]
fn frozen_panes_roundtrip() {
    // (rows, cols, top)
    let cases = [(1, 1, (0, 0)), (4, 0, (0, 0)), (0, 4, (0, 0)), (1, 0, (0, 0)), (0, 1, (0, 0)), (15, 4, (10, 2)), (21, 0, (20, 0)), (0, 8, (0, 7))];
    for (rows, cols, top) in cases {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "x");
        wb.execute(Op::Freeze { sheet: 0, rows, cols, top: CellRef::new(top.0, top.1) });
        for (name, bytes, read) in [
            ("xlsx", apex_io::xlsx::write(&wb), apex_io::xlsx::read as fn(&[u8]) -> Result<Workbook, apex_io::Error>),
            ("ods", apex_io::ods::write(&wb), apex_io::ods::read),
        ] {
            let back = read(&bytes).unwrap();
            let sh = &back.sheets[0];
            assert_eq!((sh.frozen_rows, sh.frozen_cols, sh.frozen_top), (rows, cols, CellRef::new(top.0, top.1)), "{name}: {rows} {cols} {top:?}");
        }
    }
}

/// Excel counts the frozen rows from the view's top left cell.
#[test]
fn excel_frozen_on_a_scrolled_sheet() {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    wb.execute(Op::Freeze { sheet: 0, rows: 15, cols: 4, top: CellRef::new(10, 2) });
    let xml = apex_io::zip::ZipReader::new(&apex_io::xlsx::write(&wb)).unwrap().read_string("xl/worksheets/sheet1.xml").unwrap();
    assert!(xml.contains(r#"topLeftCell="C11""#), "{xml}");
    assert!(xml.contains(r#"xSplit="2" ySplit="5" topLeftCell="E16""#), "{xml}");
}
