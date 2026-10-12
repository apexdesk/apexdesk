//! Formulas of .xls files calculate as in Excel (workbooks built here
//! record by record; compare with the values Excel saved in real files).

use std::io::{Cursor, Write};
use std::path::PathBuf;

use apex_io::Loaded;
use apex_sheet::{CellRef, Value, Workbook};

fn ole(stream: &[u8]) -> Vec<u8> {
    let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    c.create_stream("Workbook").unwrap().write_all(stream).unwrap();
    c.flush().unwrap();
    c.into_inner().into_inner()
}

fn rec(ty: u16, data: &[u8]) -> Vec<u8> {
    [&ty.to_le_bytes()[..], &(data.len() as u16).to_le_bytes(), data].concat()
}

fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A NAME record (BIFF8) with its formula tokens and extra data.
fn name(n: &str, rgce: &[u8], rgcb: &[u8]) -> Vec<u8> {
    rec(0x0018, &[&[0, 0, 0, n.len() as u8][..], &u16s(&[rgce.len() as u16, 0, 0]), &[0, 0, 0, 0, 0], n.as_bytes(), rgce, rgcb].concat())
}

/// A FORMULA record at (row, col) with a cached number.
fn formula(row: u16, col: u16, v: f64, rgce: &[u8]) -> Vec<u8> {
    rec(0x0006, &[&u16s(&[row, col, 0])[..], &v.to_le_bytes(), &[0; 6], &u16s(&[rgce.len() as u16]), rgce].concat())
}

/// A NUMBER record.
fn number(row: u16, col: u16, v: f64) -> Vec<u8> {
    rec(0x0203, &[&u16s(&[row, col, 0])[..], &v.to_le_bytes()].concat())
}

/// `XLUnicodeString` of 8-bit characters.
fn xl_string(s: &str) -> Vec<u8> {
    [&u16s(&[s.len() as u16])[..], &[0], s.as_bytes()].concat()
}

/// A workbook of one sheet "S": the global records after the sheet list
/// (`EXTERNSHEET`, names...) and the sheet's cell records.
fn book(file: &str, globals: &[Vec<u8>], cells: &[Vec<u8>]) -> Workbook {
    let bof = |dt: u16| rec(0x0809, &[&u16s(&[0x0600, dt])[..], &[0; 12]].concat());
    let head = bof(0x0005);
    let tail = [globals.concat(), rec(0x000A, &[])].concat();
    let bound = |pos: u32| rec(0x0085, &[&pos.to_le_bytes()[..], &[0, 0, 1, 0], b"S"].concat());
    let bound = bound((head.len() + bound(0).len() + tail.len()) as u32);
    let sheet = [bof(0x0010), cells.concat(), rec(0x000A, &[])].concat();
    let stream = [head, bound, tail, sheet].concat();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(file);
    std::fs::write(&path, ole(&stream)).unwrap();
    let (loaded, _) = apex_io::open(&path).unwrap();
    let Loaded::Sheet(wb) = loaded else { panic!("not a workbook") };
    *wb
}

fn value(wb: &Workbook, cell: &str) -> Value {
    wb.sheets[0].get(CellRef::parse(cell).unwrap()).map(|c| c.value().clone()).unwrap_or(Value::Empty)
}

#[test]
fn names_of_array_constants() {
    // Arr = {1,2;3,4}: PtgArray, then the values after the tokens.
    let mut rgcb = vec![1, 1, 0];
    for x in [1.0f64, 2.0, 3.0, 4.0] {
        rgcb.push(1);
        rgcb.extend_from_slice(&x.to_le_bytes());
    }
    let arr = name("Arr", &[0x60, 0, 0, 0, 0, 0, 0, 0], &rgcb);
    // A1: =SUM(Arr) (PtgName 1), B1: =INDEX(Arr,2,2).
    let a1 = formula(0, 0, 10.0, &[0x43, 1, 0, 0, 0, 0x22, 1, 4, 0]);
    let b1 = formula(0, 1, 4.0, &[0x43, 1, 0, 0, 0, 0x1E, 2, 0, 0x1E, 2, 0, 0x42, 3, 29, 0]);
    let wb = book("array_name.xls", &[rec(0x0017, &u16s(&[1, 0, 0, 0])), arr], &[a1, b1]);
    assert_eq!(value(&wb, "A1"), Value::Number(10.0));
    assert_eq!(value(&wb, "B1"), Value::Number(4.0));
}

#[test]
fn relative_names_point_back() {
    // Left = S!IV1 and Up = S!A65536, relative: from A1 they wrap round to
    // the column on the left and the row above.
    let own = rec(0x01AE, &u16s(&[1, 0x0401]));
    let xti = rec(0x0017, &u16s(&[1, 0, 0, 0]));
    let left = name("Left", &[&[0x3A][..], &u16s(&[0, 0, 0xC0FF])].concat(), &[]);
    let up = name("Up", &[&[0x3A][..], &u16s(&[0, 0xFFFF, 0xC000])].concat(), &[]);
    // B1: =Left, A2: =Up (PtgName 1 and 2).
    let b1 = formula(0, 1, 5.0, &[0x43, 1, 0, 0, 0]);
    let a2 = formula(1, 0, 5.0, &[0x43, 2, 0, 0, 0]);
    let wb = book("relative_names.xls", &[own, xti, left, up], &[number(0, 0, 5.0), b1, a2]);
    assert_eq!(value(&wb, "B1"), Value::Number(5.0));
    assert_eq!(value(&wb, "A2"), Value::Number(5.0));
}

#[test]
fn cells_of_other_workbooks() {
    // Book 0 is this one, book 1 another file with sheet "Data", of which
    // the file keeps Data!A5 = 7.5.
    let own = rec(0x01AE, &u16s(&[1, 0x0401]));
    let path = "\u{1}\u{2}dir\u{3}other.xls";
    let other = rec(0x01AE, &[&u16s(&[1])[..], &xl_string(path), &xl_string("Data")].concat());
    let xct = rec(0x0059, &u16s(&[1, 0]));
    let mut crn = vec![0, 0, 4, 0, 1];
    crn.extend_from_slice(&7.5f64.to_le_bytes());
    let crn = rec(0x005A, &crn);
    let xti = rec(0x0017, &u16s(&[2, 0, 0, 0, 1, 0, 0]));
    // A1: ='[1]Data'!$A$5*2 (PtgRef3d, ixti 1).
    let a1 = formula(0, 0, 15.0, &[0x5A, 1, 0, 4, 0, 0, 0, 0x1E, 2, 0, 0x05]);
    let wb = book("external.xls", &[own, other, xct, crn, xti], &[a1]);
    assert_eq!(value(&wb, "A1"), Value::Number(15.0));
    assert_eq!(wb.externals[0].sheets, ["Data"]);
    assert_eq!(wb.externals[0].file_name(), "other.xls");
    let f = wb.sheets[0].get(CellRef::parse("A1").unwrap()).and_then(|c| c.formula()).unwrap().expr().to_canonical();
    assert_eq!(f, "[1]Data!$A$5*2");
}

#[test]
fn shared_formulas_found_from_any_cell() {
    // Excel 5 can point the cells of a shared formula at the first one
    // written (B1) rather than at the corner of its area (A1).
    let exp = [0x01, 0, 0, 1, 0];
    // SHRFMLA A1:B1 = the cell below (PtgRefN: row +1, relative).
    let shr = rec(0x04BC, &[&u16s(&[0, 0])[..], &[0, 1, 0, 2], &u16s(&[5]), &[0x4C], &u16s(&[1, 0xC000])].concat());
    let wb = book("shared.xls", &[rec(0x0017, &u16s(&[1, 0, 0, 0]))], &[number(1, 0, 3.0), number(1, 1, 4.0), formula(0, 1, 4.0, &exp), shr, formula(0, 0, 3.0, &exp)]);
    assert_eq!(value(&wb, "A1"), Value::Number(3.0));
    assert_eq!(value(&wb, "B1"), Value::Number(4.0));
}

#[test]
fn lotus_rules_count_text_as_zero() {
    // WSBOOL with alternate expression evaluation; A1 = "-", B1 = A1+70.
    let label = rec(0x0204, &[&u16s(&[0, 0, 0, 1])[..], &[0], b"-"].concat());
    let b1 = formula(0, 1, 70.0, &[0x44, 0, 0, 0, 0xC0, 0x1E, 70, 0, 0x03]);
    let wb = book("lotus.xls", &[rec(0x0017, &u16s(&[1, 0, 0, 0]))], &[rec(0x0081, &u16s(&[0x44C1])), label, b1]);
    assert!(wb.sheets[0].lotus_eval);
    assert_eq!(value(&wb, "B1"), Value::Number(70.0));
}

#[test]
fn iteration_settings() {
    // ITERATION on, 100 steps: A1 = $A$1/2+1 settles at 2 instead of #CIRC!.
    let a1 = formula(0, 0, 2.0, &[0x44, 0, 0, 0, 0, 0x1E, 2, 0, 0x06, 0x1E, 1, 0, 0x03]);
    let settings = [rec(0x000D, &u16s(&[1])), rec(0x000C, &u16s(&[100])), rec(0x0011, &u16s(&[1]))];
    let wb = book("iterate.xls", &[rec(0x0017, &u16s(&[1, 0, 0, 0]))], &[&settings[..], &[a1]].concat());
    assert!(wb.calc.iterate && !wb.calc.manual);
    let Value::Number(n) = value(&wb, "A1") else { panic!("{:?}", value(&wb, "A1")) };
    assert!((n - 2.0).abs() < 0.01, "{n}");
}
