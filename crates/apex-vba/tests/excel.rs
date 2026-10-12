//! Excel's object model: ranges, cells, sheets, formats, events and undo.

mod common;

use apex_sheet::{CellRef, Range, Workbook};
use apex_vba::{ModuleKind, Options, Project, Source};
use common::*;

fn run(wb: &mut Workbook, code: &str) -> TestUi {
    let mut ui = TestUi::default();
    let out = run_on(wb, code, &mut ui);
    if let Some(e) = out.error {
        panic!("{} line {}: {e}", e.module, e.line);
    }
    ui
}

fn main(wb: &mut Workbook, body: &str) -> TestUi {
    run(wb, &format!("Sub Main()\n{body}\nEnd Sub\n"))
}

#[test]
fn values_and_formulas() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "Range(\"A1\").Value = 10
Range(\"A2\") = 20
Cells(3, 1).Value = \"=A1+A2\"
Range(\"B1:B3\").Formula = \"=A1*2\"
Cells(1, \"C\") = \"hello\"
Range(\"D1\").Value = \"12.5\"
Range(\"E1\").Value = True
Debug.Print Range(\"A3\").Value, Range(\"B3\").Formula, Range(\"B2\").Value, TypeName(Range(\"D1\").Value)
Debug.Print Range(\"A3\").HasFormula, Range(\"A1\").HasFormula, Range(\"A1:A3\").HasFormula
Debug.Print Range(\"A1\").Formula, Cells(1, 3).Text, Range(\"E1\").Value",
    );
    assert_eq!(ui.printed, ["30 =A3*2 40 Double", "True False Null", "10 hello True"]);
    assert_eq!(text(&wb, 0, "B2"), "40");
    assert_eq!(text(&wb, 0, "D1"), "12.5");
}

#[test]
fn range_navigation() {
    let mut wb = workbook();
    for (i, v) in ["a", "b", "c", "d"].iter().enumerate() {
        wb.set_input(0, CellRef::new(i as u32, 0), v);
    }
    wb.set_input(0, CellRef::new(0, 1), "1");
    let ui = main(
        &mut wb,
        "Dim last As Long
last = Cells(Rows.Count, 1).End(xlUp).Row
Debug.Print last, Rows.Count, Columns.Count
Debug.Print Range(\"A1\").End(xlDown).Address, Range(\"A1\").End(xlToRight).Address(False, False)
Debug.Print Range(\"B2\").Offset(1, 2).Address, Range(\"A1\").Resize(2, 3).Address, Range(\"A1:C4\").Rows.Count, Range(\"A1:C4\").Columns.Count, Range(\"A1:C4\").Count
Debug.Print Range(\"B2:D5\").Cells(2, 2).Address, Range(\"B2\").Row, Range(\"B2\").Column, Range(\"C:C\").Address, Range(\"2:3\").Address
Debug.Print Range(\"A1\").CurrentRegion.Address, ActiveSheet.UsedRange.Address
Debug.Print Range(\"A1:A4\").Find(\"c\").Address, Range(\"A1:A4\").Find(\"zz\") Is Nothing
Dim r As Range
Set r = Range(\"B2\")
Debug.Print r.EntireRow.Address, r.EntireColumn.Address, Intersect(r, Range(\"A1:C3\")).Address, Intersect(r, Range(\"D4\")) Is Nothing",
    );
    assert_eq!(
        ui.printed,
        [
            "4 1048576 16384",
            "$A$4 B1",
            "$D$3 $A$1:$C$2 4 3 12",
            "$C$3 2 2 $C:$C $2:$3",
            "$A$1:$B$4 $A$1:$B$4",
            "$A$3 True",
            "$2:$2 $B:$B $B$2 True",
        ]
    );
}

#[test]
fn loops_over_ranges_and_arrays() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "Dim i As Long, c As Range, total As Double
For i = 1 To 5
  Cells(i, 1).Value = i * 10
Next i
For Each c In Range(\"A1:A5\")
  total = total + c.Value
  c.Offset(0, 1).Value = c.Value / 10
Next c
Debug.Print total
Dim data
data = Range(\"A1:B2\").Value
Debug.Print UBound(data, 1), UBound(data, 2), data(2, 1), data(2, 2)
Dim out(1 To 2, 1 To 2)
out(1, 1) = \"x\": out(2, 2) = 5
Range(\"D1:E2\").Value = out
Range(\"G1:I1\").Value = Array(1, 2, 3)
Dim n As Long, row As Range
For Each row In Range(\"A1:B3\").Rows
  n = n + 1
Next
Debug.Print n, Range(\"E2\").Value, Range(\"I1\").Value",
    );
    assert_eq!(ui.printed, ["150", "2 2 20 2", "3 5 3"]);
    assert_eq!(text(&wb, 0, "D1"), "x");
}

#[test]
fn with_and_formats() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "With Range(\"A1:B2\")
  .Value = 1.5
  .Font.Bold = True
  .Font.Color = RGB(255, 0, 0)
  .Interior.Color = vbYellow
  .NumberFormat = \"0.00\"
  .HorizontalAlignment = xlCenter
End With
Range(\"C1\").Font.Size = 14
Range(\"C1\").Interior.ColorIndex = 3
Debug.Print Range(\"A1\").Font.Bold, Range(\"C1\").Font.Bold, Range(\"B2\").Font.Color = vbRed, Range(\"A1\").Interior.Color = vbYellow
Debug.Print Range(\"A1\").Text, Range(\"A1\").NumberFormat, Range(\"C1\").Font.Size, Range(\"A1\").HorizontalAlignment = xlCenter",
    );
    assert_eq!(ui.printed, ["True False True True", "1.50 0.00 14 True"]);
    let st = wb.styles.get(wb.cell(0, CellRef::new(1, 1)).unwrap().style);
    assert!(st.bold);
    assert_eq!(st.color, Some(0xFF0000));
    assert_eq!(st.fill, Some(0xFFFF00));
    let c1 = wb.styles.get(wb.cell(0, CellRef::new(0, 2)).unwrap().style);
    assert_eq!((c1.font_size, c1.fill), (140, Some(0xFF0000)));
}

#[test]
fn clear_delete_insert_copy() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "Range(\"A1:A3\").Value = 7
Range(\"A1\").Font.Bold = True
Range(\"A1:A2\").ClearContents
Debug.Print IsEmpty(Range(\"A1\").Value), Range(\"A1\").Font.Bold, Range(\"A3\").Value
Range(\"A3\").Clear
Debug.Print IsEmpty(Range(\"A3\").Value)
Range(\"B1\").Value = 1: Range(\"B2\").Value = 2: Range(\"B3\").Value = 3
Rows(2).Delete
Debug.Print Range(\"B2\").Value
Range(\"B1\").EntireRow.Insert
Debug.Print Range(\"B2\").Value, IsEmpty(Range(\"B1\"))
Range(\"C1\").Formula = \"=B2*2\"
Range(\"C1\").Copy Destination:=Range(\"D1\")
Debug.Print Range(\"D1\").Formula
Columns(\"C\").Delete
Debug.Print Range(\"C1\").Formula",
    );
    assert_eq!(ui.printed, ["True True 7", "True", "3", "1 True", "=C2*2", "=#REF!*2"]);
}

#[test]
fn sheets() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "Dim ws As Worksheet
Set ws = Worksheets.Add(After:=Worksheets(1))
ws.Name = \"Data\"
ws.Range(\"A1\").Value = \"on data\"
Debug.Print Worksheets.Count, ActiveSheet.Name, Worksheets(\"Data\").Index, Sheets(2).Range(\"A1\").Value
Worksheets(\"Sheet1\").Activate
Debug.Print ActiveSheet.Name
Range(\"B1\").Value = \"on sheet1\"
Dim s, names As String
For Each s In ThisWorkbook.Worksheets
  names = names & s.Name & \";\"
Next
Debug.Print names, ActiveWorkbook.Sheets.Count
Worksheets(\"Data\").Delete
Debug.Print Worksheets.Count
On Error Resume Next
Set ws = Worksheets(\"Nope\")
Debug.Print Err.Number",
    );
    assert_eq!(ui.printed, ["2 Data 2 on data", "Sheet1", "Sheet1;Data; 2", "1", "9"]);
    assert_eq!(text(&wb, 0, "B1"), "on sheet1");
}

#[test]
fn worksheet_function_and_evaluate() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "Range(\"A1\") = 3: Range(\"A2\") = 4: Range(\"A3\") = 5
Debug.Print WorksheetFunction.Sum(Range(\"A1:A3\")), Application.WorksheetFunction.Max(Range(\"A1:A3\"), 10), Application.Average(Range(\"A1:A3\"))
Debug.Print WorksheetFunction.VLookup(4, Range(\"A1:A3\"), 1, False), Evaluate(\"SUM(A1:A2)\"), [A3]
On Error Resume Next
Dim x
x = WorksheetFunction.VLookup(99, Range(\"A1:A3\"), 1, False)
Debug.Print Err.Number",
    );
    assert_eq!(ui.printed, ["12 10 4", "4 7 5", "1004"]);
}

#[test]
fn selection_and_active_cell() {
    let mut wb = workbook();
    wb.sheets.push(apex_sheet::Sheet::new("Two"));
    let p = project("Sub Main()\n  Range(\"B2:C3\").Select\n  Selection.Value = 1\n  ActiveCell.Value = 2\n  Worksheets(2).Activate\n  Range(\"A1\").Value = \"x\"\nEnd Sub");
    let mut ui = TestUi::default();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Main");
    assert!(out.error.is_none());
    assert_eq!(out.active_sheet, 1);
    assert_eq!(text(&wb, 0, "B2"), "2");
    assert_eq!(text(&wb, 0, "C3"), "1");
    assert_eq!(text(&wb, 1, "A1"), "x");
}

#[test]
fn a_macro_is_one_undo_step() {
    let mut wb = workbook();
    wb.set_input(0, CellRef::new(0, 0), "keep");
    let steps = wb.undo_len();
    let mut ui = TestUi::default();
    let out = run_on(&mut wb, "Sub Main()\n  Dim i\n  For i = 1 To 50\n    Cells(i, 2) = i\n  Next\n  Range(\"A1\").Font.Italic = True\n  Worksheets.Add\nEnd Sub", &mut ui);
    assert!(out.changed && out.error.is_none());
    assert_eq!(wb.sheets.len(), 2);
    assert_eq!(wb.undo_len(), steps + 1);
    assert_eq!(wb.undo_history()[0], apex_vba::UNDO_LABEL);
    assert!(wb.undo());
    assert_eq!(wb.sheets.len(), 1);
    assert!(wb.cell(0, CellRef::new(10, 1)).is_none());
    assert_eq!(text(&wb, 0, "A1"), "keep");
    assert!(!wb.styles.get(wb.cell(0, CellRef::new(0, 0)).unwrap().style).italic);
    // A macro that fails keeps what it did before the error, still undoable.
    let out = run_on(&mut wb, "Sub Main()\n  Range(\"C1\") = 1\n  Err.Raise 5\nEnd Sub", &mut ui);
    assert!(out.error.is_some() && out.changed);
    assert!(wb.undo());
    assert!(wb.cell(0, CellRef::new(0, 2)).is_none());
}

#[test]
fn protection_and_read_only_stop_changes() {
    let mut wb = workbook();
    wb.read_only = true;
    let mut ui = TestUi::default();
    let out = run_on(&mut wb, "Sub Main()\n  Range(\"A1\") = 1\nEnd Sub", &mut ui);
    assert_eq!(out.error.unwrap().number, 1004);
    // Protect and Unprotect with a password.
    let mut wb = workbook();
    wb.enforce_protection = true;
    let ui = main(
        &mut wb,
        "ActiveSheet.Protect Password:=\"pw\"
On Error Resume Next
Range(\"A1\") = 1
Debug.Print Err.Number, ActiveSheet.ProtectContents
Err.Clear
ActiveSheet.Unprotect \"bad\"
Debug.Print Err.Number
On Error GoTo 0
ActiveSheet.Unprotect \"pw\"
Range(\"A1\") = 2
Debug.Print ActiveSheet.ProtectContents, Range(\"A1\").Value",
    );
    assert_eq!(ui.printed, ["1004 True", "1004", "False 2"]);
}

fn events_project() -> Project {
    Project::parse(&[
        Source { name: "ThisWorkbook".into(), kind: ModuleKind::Document, code: "Private Sub Workbook_Open()\n  Worksheets(1).Range(\"A1\").Value = \"opened\"\n  MsgBox \"Welcome\"\nEnd Sub\n".into() },
        Source { name: "Sheet1".into(), kind: ModuleKind::Document, code: "Private Sub Worksheet_Change(ByVal Target As Range)\n  If Not Intersect(Target, Range(\"B:B\")) Is Nothing Then\n    Target.Offset(0, 1).Value = Target.Value * 2\n  End If\n  Me.Range(\"Z1\").Value = Target.Address\nEnd Sub\n".into() },
        Source { name: "Module1".into(), kind: ModuleKind::Standard, code: "Sub Hello()\n  MsgBox \"hi\"\nEnd Sub\n".into() },
    ])
    .unwrap()
}

#[test]
fn workbook_open_and_worksheet_change() {
    let p = events_project();
    assert!(p.has_open_handler() && p.has_change_handler());
    assert_eq!(p.macros(), ["Hello"]);
    let mut wb = workbook();
    wb.sheets.push(apex_sheet::Sheet::new("Other"));
    let mut ui = TestUi::default();
    let out = apex_vba::workbook_open(&p, &mut wb, &mut ui, &Options::default()).unwrap();
    assert!(out.error.is_none());
    assert_eq!(text(&wb, 0, "A1"), "opened");
    assert_eq!(ui.shown, ["Welcome"]);
    // The user typed 21 in B3 of the first sheet.
    wb.set_input(0, CellRef::new(2, 1), "21");
    let (sheet, target) = wb.last_edit().unwrap();
    assert_eq!((sheet, target), (0, Range::single(CellRef::new(2, 1))));
    let out = apex_vba::worksheet_change(&p, &mut wb, &mut ui, &Options { active_sheet: 1, ..Default::default() }, sheet, target).unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(text(&wb, 0, "C3"), "42");
    // Unqualified Range in a sheet's module is that sheet, even when another is active.
    assert_eq!(text(&wb, 0, "Z1"), "$B$3");
    assert!(wb.cell(1, CellRef::new(0, 25)).is_none());
    // Other sheets have no handler.
    assert!(apex_vba::worksheet_change(&p, &mut wb, &mut ui, &Options::default(), 1, target).is_none());
}

#[test]
fn application_settings_and_status() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        "Application.ScreenUpdating = False
Application.DisplayAlerts = False
Application.EnableEvents = False
Application.Calculation = xlCalculationManual
Debug.Print Application.ScreenUpdating, Application.EnableEvents, Application.Calculation = xlCalculationManual
Application.ScreenUpdating = True
Application.EnableEvents = True
ThisWorkbook.Save
Debug.Print ThisWorkbook.Name, Application.Name",
    );
    assert_eq!(ui.printed, ["False False True", "Book1 ApexDesk"]);
}

#[test]
fn common_real_world_macro() {
    let mut wb = workbook();
    let data = [("Name", "Amount"), ("Ana", "120"), ("Luis", "80"), ("Eva", "200"), ("Ana", "30")];
    for (i, (a, b)) in data.iter().enumerate() {
        wb.set_input(0, CellRef::new(i as u32, 0), a);
        wb.set_input(0, CellRef::new(i as u32, 1), b);
    }
    let code = "Option Explicit

' Totals per name on a new sheet, highlighting big amounts.
Public Sub Summarize()
    Dim src As Worksheet, dst As Worksheet
    Dim lastRow As Long, r As Long
    Dim totals As Object, key As Variant

    Set src = ThisWorkbook.Worksheets(\"Sheet1\")
    Set totals = CreateObject(\"Scripting.Dictionary\")
    lastRow = src.Cells(src.Rows.Count, \"A\").End(xlUp).Row

    For r = 2 To lastRow
        key = src.Cells(r, 1).Value
        totals(key) = totals(key) + src.Cells(r, 2).Value
        If src.Cells(r, 2).Value >= 100 Then src.Cells(r, 2).Interior.Color = RGB(255, 200, 200)
    Next r

    Set dst = Worksheets.Add(After:=src)
    dst.Name = \"Summary\"
    dst.Range(\"A1:B1\").Value = Array(\"Name\", \"Total\")
    dst.Range(\"A1:B1\").Font.Bold = True
    r = 2
    For Each key In totals.Keys
        dst.Cells(r, 1).Value = key
        dst.Cells(r, 2).Value = totals(key)
        r = r + 1
    Next key
    dst.Cells(r, 1).Value = \"Total\"
    dst.Cells(r, 2).Formula = \"=SUM(B2:B\" & r - 1 & \")\"
    MsgBox \"Done: \" & totals.Count & \" names, total \" & dst.Cells(r, 2).Value, vbInformation
End Sub
";
    let p = project(code);
    let mut ui = TestUi::default();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Summarize");
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(wb.sheets[1].name, "Summary");
    assert_eq!(text(&wb, 1, "A2"), "Ana");
    assert_eq!(text(&wb, 1, "B2"), "150");
    assert_eq!(text(&wb, 1, "B5"), "430");
    assert_eq!(ui.shown, ["Done: 3 names, total 430"]);
    assert_eq!(wb.styles.get(wb.cell(0, CellRef::new(1, 1)).unwrap().style).fill, Some(0xFFC8C8));
    assert_eq!(wb.styles.get(wb.cell(0, CellRef::new(2, 1)).unwrap().style).fill, None);
}

#[test]
fn code_names_and_module_members() {
    let mut wb = workbook();
    wb.sheets[0].name = "Datos".into();
    wb.sheets[0].code_name = Some("shData".into());
    wb.sheets.push(apex_sheet::Sheet::new("Other"));
    wb.sheets[1].code_name = Some("Sheet2".into());
    let src = |name: &str, kind, code: &str| Source { name: name.into(), kind, code: code.into() };
    let p = Project::parse(&[
        src("shData", ModuleKind::Document, "Public Password As String\nPublic Sub Refresh()\n  Me.Range(\"A1\").Value = \"refreshed \" & Password\nEnd Sub\nFunction Twice(x)\n  Twice = 2 * x\nEnd Function\n"),
        src("ThisWorkbook", ModuleKind::Document, "Public Function Helper() As String\n  Helper = \"help\"\nEnd Function\n"),
        src("Tools", ModuleKind::Standard, "Public Const Version = 3\nPublic Counter As Long\nSub Bump()\n  Counter = Counter + 1\nEnd Sub\n"),
        src("Module1", ModuleKind::Standard, "Sub Main()\n  shData.Password = \"x\"\n  Worksheets(\"Datos\").Refresh\n  Sheet2.Range(\"B2\").Value = shData.Twice(21)\n  Tools.Bump: Tools.Bump\n  Debug.Print ThisWorkbook.Helper, Tools.Counter, Tools.Version, shData.Name, Sheet2.CodeName, shData.Password\nEnd Sub\nSub Controls()\n  If shData.CheckBoxes(\"x\").Value = 1 Then Beep\nEnd Sub\n"),
    ])
    .unwrap();
    let mut ui = TestUi::default();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Main");
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.printed, ["help 2 3 Datos Sheet2 x"]);
    assert_eq!(text(&wb, 0, "A1"), "refreshed x");
    assert_eq!(text(&wb, 1, "B2"), "42");
    let e = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Controls").error.unwrap();
    assert_eq!(e.number, 445, "{e}");
}
