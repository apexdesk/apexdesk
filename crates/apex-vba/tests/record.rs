//! The macro recorder: the code it writes for some edits, run on the
//! workbook as it was, makes the same workbook.

mod common;

use apex_sheet::record::Recorder;
use apex_sheet::sheet::{Line, LineStyle, border};
use apex_sheet::{CellRef, Op, Range, Workbook};

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn r(s: &str) -> Range {
    Range::parse(s).unwrap()
}

fn select(wb: &mut Workbook, sheet: &str, range: &str) {
    let rg = r(range);
    wb.recorder.as_mut().unwrap().select(sheet, rg, rg.start);
}

/// What the test compares of a workbook.
fn summary(wb: &Workbook) -> Vec<String> {
    let mut out = Vec::new();
    for (i, sh) in wb.sheets.iter().enumerate() {
        out.push(format!("sheet {} hidden={} frozen={},{} merges={:?} filter={:?}", sh.name, sh.hidden, sh.frozen_rows, sh.frozen_cols, sh.merges, sh.filter.as_ref().map(|f| f.range)));
        for (at, cell) in sh.iter() {
            let what = match cell.formula() {
                Some(f) => format!("={}", f.expr().to_canonical()),
                None => format!("{:?}", cell.value()),
            };
            out.push(format!("  {} {what} {:?}", at.to_a1(), wb.styles.get(cell.style)));
        }
        for col in 0..8 {
            out.push(format!("  col {col} {}", sh.col_width(col)));
        }
        for row in 0..13 {
            out.push(format!("  row {row} {}", sh.row_height(row)));
        }
        let _ = i;
    }
    out
}

fn replay(start: &Workbook, lines: &[String], relative_at: &str) -> Workbook {
    let code = format!("Sub Main()\n{}\nEnd Sub\n", lines.join("\n"));
    let mut wb = start.clone();
    let p = common::project(&code);
    let mut ui = common::TestUi::default();
    let opts = apex_vba::Options { selection: r(relative_at), ..apex_vba::Options::default() };
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    if let Some(e) = out.error {
        panic!("line {}: {e}\n{code}", e.line);
    }
    wb
}

fn edits(wb: &mut Workbook) {
    select(wb, "Sheet1", "A1");
    wb.set_input(0, c("A1"), "Name");
    select(wb, "Sheet1", "A2");
    wb.set_input(0, c("A2"), "'007");
    wb.set_input(0, c("B1"), "=1+2");
    wb.set_inputs(0, vec![(c("B2"), "=A2&\"x\"".into()), (c("B3"), "=A3&\"x\"".into()), (c("C2"), "2.5".into()), (c("C3"), "TRUE".into())]);
    select(wb, "Sheet1", "B2:C3");
    wb.modify_style(0, &r("B2:C3"), |s| {
        s.bold = true;
        s.fill = Some(0xFFCC00);
        s.num_fmt = "0.00".into();
        s.borders = apex_sheet::sheet::Borders::sides(border::ALL, Line { style: LineStyle::Thin, color: None });
    });
    wb.modify_style(0, &r("A1"), |s| {
        s.italic = true;
        s.color = Some(0x0000FF);
        s.halign = apex_sheet::sheet::HAlign::Center;
        s.font_size = 140;
    });
    wb.execute(Op::Merges { sheet: 0, merges: vec![r("D1:E1")] });
    wb.execute(Op::ColWidths { sheet: 0, sizes: vec![(1, Some((15.0 * 7.0 + 5.0) * 88.0 / 64.0)), (2, Some((15.0 * 7.0 + 5.0) * 88.0 / 64.0))] });
    wb.execute(Op::RowHeights { sheet: 0, sizes: vec![(3, Some(30.0 * 22.0 / 15.0))] });
    wb.execute(Op::InsertRows { sheet: 0, at: 5, count: 2 });
    wb.execute(Op::DeleteCols { sheet: 0, at: 6, count: 1 });
    wb.set_inputs(0, vec![(c("A10"), "b".into()), (c("B10"), "1".into()), (c("A11"), "a".into()), (c("B11"), "2".into()), (c("A12"), "c".into()), (c("B12"), "2".into())]);
    wb.sort_by(0, &r("A10:B12"), &[apex_sheet::SortKey::new(1, false), apex_sheet::SortKey::new(0, true)], false);
    select(wb, "Sheet1", "B2");
    wb.execute(Op::Freeze { sheet: 0, rows: 1, cols: 1, top: CellRef::new(0, 0) });
    wb.execute(Op::Filter { sheet: 0, filter: Some(apex_sheet::data::AutoFilter::new(r("A1:C3"))) });
    wb.execute(Op::AddSheet { index: 1, sheet: Box::new(apex_sheet::Sheet::new("Sheet2")) });
    select(wb, "Sheet2", "A1");
    wb.execute(Op::RenameSheet { index: 1, name: "Data".into() });
    select(wb, "Data", "C4");
    wb.set_input(1, c("C4"), "here");
    wb.duplicate_sheet(0);
    select(wb, "Sheet1 (2)", "A1");
    wb.execute(Op::MoveSheet { from: 2, to: 0 });
    wb.execute(Op::RemoveSheet { index: 2 });
}

#[test]
fn recorded_code_makes_the_same_workbook() {
    let mut start = common::workbook();
    start.set_input(0, c("A3"), "keep");
    for relative in [false, true] {
        let mut wb = start.clone();
        wb.recorder = Some(Box::new(Recorder::new(relative)));
        edits(&mut wb);
        let lines = wb.recorder.take().unwrap().finish();
        assert!(lines.iter().any(|l| l.contains(".Sort Key1:=") && l.contains("Key2:=")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.starts_with('\'')), "{lines:?}");
        let again = replay(&start, &lines, "A1");
        assert_eq!(summary(&again), summary(&wb), "relative={relative}\n{}", lines.join("\n"));
    }
}

#[test]
fn what_it_writes() {
    let mut wb = common::workbook();
    wb.recorder = Some(Box::new(Recorder::new(false)));
    select(&mut wb, "Sheet1", "A1");
    wb.set_input(0, c("A1"), "5");
    select(&mut wb, "Sheet1", "A2");
    wb.set_inputs(0, vec![(c("B1"), "=A1*2".into()), (c("B2"), "=A2*2".into())]);
    select(&mut wb, "Sheet1", "B1:B2");
    wb.modify_style(0, &r("B1:B2"), |s| s.bold = true);
    let lines = wb.recorder.take().unwrap().finish();
    assert_eq!(lines, ["ActiveCell.Value = 5", "Range(\"A2\").Select", "Range(\"B1:B2\").FormulaR1C1 = \"=RC[-1]*2\"", "Range(\"B1:B2\").Select", "Selection.Font.Bold = True"]);
    // Relative: from the active cell.
    let mut wb = common::workbook();
    wb.recorder = Some(Box::new(Recorder::new(true)));
    select(&mut wb, "Sheet1", "A1");
    wb.set_input(0, c("A1"), "5");
    select(&mut wb, "Sheet1", "A2");
    wb.set_input(0, c("A2"), "6");
    wb.set_input(0, c("B3"), "7");
    let lines = wb.recorder.take().unwrap().finish();
    assert_eq!(lines, ["ActiveCell.Value = 5", "ActiveCell.Offset(1, 0).Select", "ActiveCell.Value = 6", "ActiveCell.Offset(1, 1).Value = 7"]);
}
