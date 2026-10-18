//! Ranges of several areas and Range methods made with the workbook's
//! edits (SpecialCells, FillDown, AutoFill, RemoveDuplicates, Replace,
//! notes, Copy and PasteSpecial).

mod common;

use apex_sheet::{CellRef, Workbook};
use common::*;

fn main(wb: &mut Workbook, body: &str) -> Result<TestUi, String> {
    let mut ui = TestUi::default();
    let out = run_on(wb, &format!("Sub Main()\n{body}\nEnd Sub\n"), &mut ui);
    match out.error {
        Some(e) => Err(format!("line {}: {e}", e.line)),
        None => Ok(ui),
    }
}

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

#[test]
fn several_areas() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "Dim r As Range
Set r = Range(\"A1:B2,D4\")
r.Value = 7
Debug.Print r.Address, r.Areas.Count, r.Count, r.Areas(2).Address(False, False)
Dim c As Range, n As Long
For Each c In r
    n = n + c.Value
Next
Debug.Print n
Set r = Union(Range(\"A1\"), Range(\"C3\"))
Debug.Print r.Address, TypeName(r), Union(Range(\"A1\"), Range(\"A2\")).Address
r.Font.Bold = True
Debug.Print Intersect(Range(\"A1:B2,D4\"), Range(\"B2:D4\")).Address
Debug.Print Intersect(Range(\"A1\"), Range(\"C3\")) Is Nothing
Dim a As Range
Dim s As String
For Each a In Range(\"A1:B2,D4\").Areas
    s = s & a.Address(0, 0) & \";\"
Next
Debug.Print s",
    )
    .unwrap();
    assert_eq!(ui.printed, ["$A$1:$B$2,$D$4               2             5            D4", " 35 ", "$A$1,$C$3     Range         $A$1:$A$2", "$B$2,$D$4", "True", "A1:B2;D4;"]);
    assert!(wb.styles.get(wb.cell(0, c("C3")).unwrap().style).bold);
}

#[test]
fn deleting_rows_of_several_areas() {
    let mut wb = workbook();
    for i in 1..=8 {
        wb.set_input(0, c(&format!("A{i}")), &i.to_string());
    }
    main(&mut wb, "Range(\"A2,A3,A6\").EntireRow.Delete").unwrap();
    let left: Vec<String> = (1..=5).map(|i| wb.display(0, c(&format!("A{i}"))).0).collect();
    assert_eq!(left, ["1", "4", "5", "7", "8"]);
}

#[test]
fn special_cells() {
    let mut wb = workbook();
    wb.set_inputs(0, vec![(c("A1"), "1".into()), (c("A3"), "x".into()), (c("A4"), "=A1*2".into()), (c("B1"), "TRUE".into())]);
    let ui = main(
        &mut wb,
        "Debug.Print Range(\"A1:A4\").SpecialCells(xlCellTypeBlanks).Address
Debug.Print Range(\"A1:B4\").SpecialCells(xlCellTypeConstants, xlTextValues).Address
Debug.Print Range(\"A1:B4\").SpecialCells(xlCellTypeConstants).Address
Debug.Print Range(\"A1\").SpecialCells(xlCellTypeFormulas).Address
Debug.Print Range(\"A1\").SpecialCells(xlCellTypeLastCell).Address
Range(\"A1:A4\").SpecialCells(xlCellTypeBlanks).Value = 0
On Error Resume Next
Range(\"C1:C4\").SpecialCells(xlCellTypeConstants).Select
Debug.Print Err.Number, Err.Description",
    )
    .unwrap();
    assert_eq!(ui.printed, ["$A$2", "$A$3", "$A$1:$B$1,$A$3", "$A$4", "$B$4", " 1004         No cells were found."]);
    assert_eq!(wb.display(0, c("A2")).0, "0");
}

#[test]
fn fills_duplicates_and_replace() {
    let mut wb = workbook();
    wb.set_inputs(0, vec![(c("A1"), "1".into()), (c("B1"), "=A1*10".into()), (c("A2"), "2".into())]);
    main(
        &mut wb,
        "Range(\"B1:B3\").FillDown
Range(\"A1:A2\").AutoFill Destination:=Range(\"A1:A5\")
Range(\"D1:F1\").Value = Array(\"k\", \"v\", \"n\")
Range(\"D2:F2\").Value = Array(\"a\", \"1\", \"x\")
Range(\"D3:F3\").Value = Array(\"A\", \"1\", \"y\")
Range(\"D4:F4\").Value = Array(\"b\", \"1\", \"z\")
Range(\"D1:F4\").RemoveDuplicates Columns:=Array(1, 2), Header:=xlYes
Range(\"H1:H2\").Value = Application.Transpose(Array(\"red car\", \"Red\"))
Range(\"H1:H2\").Replace What:=\"red\", Replacement:=\"blue\", LookAt:=xlPart, MatchCase:=False
Range(\"B1:B3\").Replace \"A\", \"C\"",
    )
    .unwrap();
    let t = |s: &str| wb.display(0, c(s)).0;
    // Filled down, then A replaced by C in the formulas.
    assert_eq!([t("B2"), t("B3")], ["0", "0"]);
    assert_eq!(wb.edit_text(0, c("B3")), "=C3*10");
    assert_eq!([t("A3"), t("A5")], ["3", "5"]);
    assert_eq!([t("D2"), t("D3"), t("F3"), t("D4")], ["a", "b", "z", ""]);
    assert_eq!([t("H1"), t("H2")], ["blue car", "blue"]);
}

#[test]
fn notes_copy_and_paste() {
    let mut wb = workbook();
    wb.set_inputs(0, vec![(c("A1"), "5".into()), (c("A2"), "=A1+1".into())]);
    let ui = main(
        &mut wb,
        "Range(\"A1\").AddComment \"check this\"
Debug.Print Range(\"A1\").NoteText
Range(\"A1:A2\").Copy
Debug.Print Application.CutCopyMode
Range(\"C1\").PasteSpecial Paste:=xlPasteValues
Range(\"A1:A2\").Copy
ActiveSheet.Paste Destination:=Range(\"D1\")
Range(\"D1\").Copy
Range(\"E1\").PasteSpecial Paste:=xlPasteAll, Operation:=xlAdd
Application.CutCopyMode = False
Debug.Print Application.CutCopyMode
Range(\"A1:A2\").Cut Destination:=Range(\"F5\")
Range(\"F5:F6\").ClearComments",
    )
    .unwrap();
    assert_eq!(ui.printed, ["check this", " 1 ", "False"]);
    let t = |s: &str| wb.display(0, c(s)).0;
    assert_eq!(wb.edit_text(0, c("C2")), "6");
    assert_eq!(wb.edit_text(0, c("D2")), "=D1+1");
    assert_eq!(t("E1"), "5");
    assert_eq!([t("A1"), t("F5"), t("F6")], ["", "5", "6"]);
    assert!(wb.cell(0, c("F5")).and_then(|x| x.note()).is_none());
    // One undo step for the whole macro.
    assert!(wb.undo());
    let t = |s: &str| wb.display(0, c(s)).0;
    assert_eq!([t("A1"), t("C1"), t("F5")], ["5", "", ""]);
    assert!(wb.cell(0, c("A1")).and_then(|x| x.note()).is_none());
}

#[test]
fn protection_holds() {
    let mut wb = workbook();
    wb.set_inputs(0, vec![(c("A1"), "1".into()), (c("A2"), "2".into())]);
    wb.enforce_protection = true;
    let Err(e) = main(&mut wb, "ActiveSheet.Protect\nRange(\"A1:A2\").FillDown") else { panic!("it filled") };
    assert!(e.contains("protected"), "{e}");
    assert_eq!(wb.display(0, c("A2")).0, "2");
}

