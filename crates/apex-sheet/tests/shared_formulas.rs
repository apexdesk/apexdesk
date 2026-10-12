//! Formulas filled down share one tree; editing keeps each cell's own
//! formula and value exactly as without sharing.

use apex_sheet::workbook::SortKey;
use apex_sheet::{CellRef, Content, Op, Range, Value, Workbook};

const N: u32 = 1000;

/// A1:A1000 numbers, B1 `=A1*2+$C$1`, filled down to B1000.
fn filled() -> Workbook {
    let mut w = Workbook::new();
    for r in 0..N {
        w.set_input(0, CellRef::new(r, 0), &(r + 1).to_string());
    }
    w.set_input(0, CellRef::new(0, 2), "5");
    w.set_input(0, CellRef::new(0, 1), "=A1*2+$C$1");
    w.fill(0, &Range::new(CellRef::new(0, 1), CellRef::new(N - 1, 1)), true);
    w
}

fn texts(w: &Workbook, col: u32) -> Vec<String> {
    (0..MAXR).map(|r| w.edit_text(0, CellRef::new(r, col))).collect()
}

fn values(w: &Workbook, col: u32) -> Vec<Value> {
    (0..MAXR).map(|r| w.sheets[0].value(CellRef::new(r, col)).clone()).collect()
}

/// How many formula cells of column `col` share the tree of the first.
fn sharing(w: &Workbook, col: u32) -> usize {
    let sh = &w.sheets[0];
    let formulas: Vec<_> = (0..MAXR).filter_map(|r| sh.get(CellRef::new(r, col))?.formula().cloned()).collect();
    formulas.iter().filter(|f| f.shares_with(&formulas[0])).count()
}
const MAXR: u32 = N + 10;

#[test]
fn a_filled_column_shares_one_formula() {
    let w = filled();
    assert_eq!(sharing(&w, 1), N as usize);
    assert_eq!(w.edit_text(0, CellRef::new(499, 1)), "=A500*2+$C$1");
    assert_eq!(*w.sheets[0].value(CellRef::new(499, 1)), Value::Number(1005.0));
}

#[test]
fn loading_shares_what_fill_would() {
    let mut w = Workbook::new();
    for r in 0..N {
        let e = apex_sheet::formula::parse_canonical(&format!("A{}*2+$C$1", r + 1)).unwrap();
        w.load_cell(0, CellRef::new(r, 1), Content::Formula { expr: e.into(), value: Value::Empty }, 0);
    }
    // One cell differs: it keeps its own tree, the next ones start again.
    let e = apex_sheet::formula::parse_canonical("A1*3").unwrap();
    w.load_cell(0, CellRef::new(N, 1), Content::Formula { expr: e.into(), value: Value::Empty }, 0);
    assert_eq!(sharing(&w, 1), N as usize);
    assert_eq!(w.edit_text(0, CellRef::new(N, 1)), "=A1*3");
}

#[test]
fn inserting_and_deleting_rows_keeps_formulas_and_sharing() {
    let mut w = filled();
    let (t0, v0) = (texts(&w, 1), values(&w, 1));
    w.execute(Op::InsertRows { sheet: 0, at: 0, count: 3 });
    assert_eq!(w.edit_text(0, CellRef::new(3, 1)), "=A4*2+$C$4");
    assert_eq!(w.edit_text(0, CellRef::new(N + 2, 1)), format!("=A{}*2+$C$4", N + 3));
    assert_eq!(sharing(&w, 1), N as usize);
    assert_eq!(values(&w, 1)[3..3 + N as usize], v0[..N as usize]);
    w.execute(Op::DeleteRows { sheet: 0, at: 10, count: 5 });
    assert_eq!(w.edit_text(0, CellRef::new(10, 1)), "=A11*2+$C$4");
    assert_eq!(w.sheets[0].value(CellRef::new(10, 1)), &Value::Number(2.0 * 13.0 + 5.0));
    w.undo();
    w.undo();
    assert_eq!(texts(&w, 1), t0);
    assert_eq!(values(&w, 1), v0);
}

#[test]
fn editing_one_copy_leaves_the_others() {
    let mut w = filled();
    w.set_input(0, CellRef::new(10, 1), "=A11*100");
    assert_eq!(w.edit_text(0, CellRef::new(10, 1)), "=A11*100");
    assert_eq!(w.edit_text(0, CellRef::new(9, 1)), "=A10*2+$C$1");
    assert_eq!(w.edit_text(0, CellRef::new(11, 1)), "=A12*2+$C$1");
    assert_eq!(*w.sheets[0].value(CellRef::new(10, 1)), Value::Number(1100.0));
    // Changing what they all read updates them all.
    w.set_input(0, CellRef::new(0, 2), "7");
    assert_eq!(*w.sheets[0].value(CellRef::new(999, 1)), Value::Number(2007.0));
}

#[test]
fn sorting_keeps_each_cells_formula_as_without_sharing() {
    let mut w = filled();
    let r = Range::new(CellRef::new(0, 0), CellRef::new(N - 1, 1));
    w.sort_range(0, &r, &[SortKey::new(0, false)], false, false);
    // What sorting did to each formula matches doing it cell by cell.
    let mut plain = Workbook::new();
    for r in 0..N {
        plain.set_input(0, CellRef::new(r, 0), &(r + 1).to_string());
        plain.set_input(0, CellRef::new(r, 1), &format!("=A{}*2+$C$1", r + 1));
    }
    plain.set_input(0, CellRef::new(0, 2), "5");
    plain.sort_range(0, &r, &[SortKey::new(0, false)], false, false);
    assert_eq!(texts(&w, 1), texts(&plain, 1));
    assert_eq!(values(&w, 1), values(&plain, 1));
}
