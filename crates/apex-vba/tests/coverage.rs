//! Macros as people write them: the idioms of forums, books and
//! templates, each run on a small workbook. What fails here is what a
//! real macro would trip over first.

mod common;

use apex_sheet::{CellRef, Workbook};
use common::*;

/// A sheet "Data" with a header and rows: name, region, amount, date.
fn data_book() -> Workbook {
    let mut wb = workbook();
    wb.sheets[0].name = "Data".into();
    let rows = [
        ["Name", "Region", "Amount", "Date"],
        ["Ann", "North", "120", "2024-01-05"],
        ["Bob", "South", "80", "2024-01-09"],
        ["", "", "", ""],
        ["Cid", "North", "200", "2024-02-01"],
        ["Ann", "East", "50", "2024-02-11"],
        ["Dee", "South", "", "2024-03-03"],
        ["Bob", "South", "80", "2024-01-09"],
    ];
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            if !v.is_empty() {
                wb.set_input(0, CellRef::new(r as u32, c as u32), v);
            }
        }
    }
    wb
}

fn run(code: &str) -> (Workbook, Vec<String>) {
    let mut wb = data_book();
    let mut ui = TestUi::default();
    let out = run_on(&mut wb, code, &mut ui);
    if let Some(e) = out.error {
        panic!("{} line {}: {e}", e.module, e.line);
    }
    (wb, ui.printed)
}

fn cell(wb: &Workbook, sheet: usize, a1: &str) -> String {
    text(wb, sheet, a1)
}

#[test]
fn last_row_and_column_idioms() {
    let (_, p) = run(r##"Sub Main()
    Dim ws As Worksheet, lastRow As Long, lastCol As Long
    Set ws = ThisWorkbook.Worksheets("Data")
    lastRow = ws.Cells(ws.Rows.Count, "A").End(xlUp).Row
    lastCol = ws.Cells(1, ws.Columns.Count).End(xlToLeft).Column
    Debug.Print lastRow; lastCol
    Debug.Print ws.UsedRange.Rows.Count; ws.Range("A1").CurrentRegion.Address
    Debug.Print ws.Cells.Find(What:="*", SearchOrder:=xlByRows, SearchDirection:=xlPrevious).Row
    Debug.Print Range("A1").End(xlDown).Row; ActiveSheet.Name
End Sub"##);
    assert_eq!(p, [" 8  4 ", " 8 $A$1:$D$3", " 8 ", " 3 Data"].map(String::from));
}

#[test]
fn delete_blank_rows_from_the_bottom() {
    let (wb, _) = run(r##"Sub Main()
    Dim i As Long
    Application.ScreenUpdating = False
    For i = Cells(Rows.Count, 1).End(xlUp).Row To 2 Step -1
        If WorksheetFunction.CountA(Rows(i)) = 0 Then Rows(i).Delete
    Next i
    Application.ScreenUpdating = True
End Sub"##);
    assert_eq!(cell(&wb, 0, "A4"), "Cid");
    assert_eq!(cell(&wb, 0, "A8"), "");
}

#[test]
fn remove_duplicates_and_sort() {
    let (wb, _) = run(r##"Sub Main()
    With Worksheets("Data")
        .Range("A1:D8").RemoveDuplicates Columns:=Array(1, 2, 3), Header:=xlYes
        .Range("A1").CurrentRegion.Sort Key1:=.Range("C1"), Order1:=xlDescending, Header:=xlYes
    End With
End Sub"##);
    assert_eq!(cell(&wb, 0, "A2"), "Ann");
    assert_eq!(cell(&wb, 0, "C2"), "120");
}

#[test]
fn loop_cells_and_color_them() {
    let (wb, p) = run(r##"Sub Main()
    Dim c As Range, n As Long
    For Each c In Range("C2:C8")
        If IsNumeric(c.Value) And Not IsEmpty(c.Value) Then
            If c.Value > 100 Then
                c.Interior.Color = RGB(255, 199, 206)
                c.Font.Bold = True
                n = n + 1
            End If
        End If
    Next c
    Debug.Print n & " big"
    Debug.Print Range("C2").Interior.Color = vbRed, Range("C2").Font.Bold
End Sub"##);
    assert_eq!(p, ["2 big", "False         True"]);
    let _ = wb;
}

#[test]
fn split_data_into_sheets_by_region() {
    let (wb, p) = run(r##"Sub Main()
    Dim src As Worksheet, ws As Worksheet, dict As Object, k, r As Long, last As Long
    Set src = Worksheets("Data")
    Set dict = CreateObject("Scripting.Dictionary")
    last = src.Cells(src.Rows.Count, 1).End(xlUp).Row
    For r = 2 To last
        k = src.Cells(r, 2).Value
        If k <> "" Then
            If Not dict.Exists(k) Then
                Set ws = Worksheets.Add(After:=Worksheets(Worksheets.Count))
                ws.Name = k
                src.Rows(1).Copy ws.Rows(1)
                dict.Add k, 2
            End If
            Set ws = Worksheets(k)
            src.Rows(r).Copy Destination:=ws.Rows(dict(k))
            dict(k) = dict(k) + 1
        End If
    Next r
    For Each k In dict.Keys
        Debug.Print k & ":" & (dict(k) - 2)
    Next
    src.Activate
End Sub"##);
    assert_eq!(p, ["North:2", "South:3", "East:1"]);
    assert_eq!(wb.sheets.len(), 4);
    assert_eq!(cell(&wb, 2, "A4"), "Bob");
}

#[test]
fn consolidate_sheets_into_one() {
    let (wb, _) = run(r##"Sub Main()
    Dim ws As Worksheet, dest As Worksheet, nextRow As Long, rng As Range
    Worksheets.Add(After:=Sheets(Sheets.Count)).Name = "Other"
    Sheets("Other").Range("A1:D1").Value = Array("Name", "Region", "Amount", "Date")
    Sheets("Other").Range("A2:C2").Value = Array("Eve", "West", 10)
    Set dest = Worksheets.Add(Before:=Sheets(1))
    dest.Name = "All"
    nextRow = 1
    For Each ws In ThisWorkbook.Worksheets
        If ws.Name <> dest.Name Then
            Set rng = ws.Range("A1").CurrentRegion
            If nextRow > 1 Then Set rng = rng.Offset(1, 0).Resize(rng.Rows.Count - 1)
            rng.Copy dest.Cells(nextRow, 1)
            nextRow = dest.Cells(dest.Rows.Count, 1).End(xlUp).Row + 1
        End If
    Next ws
End Sub"##);
    assert_eq!(wb.sheets[0].name, "All");
    assert_eq!(cell(&wb, 0, "A1"), "Name");
    assert_eq!(cell(&wb, 0, "A4"), "Eve");
}

#[test]
fn arrays_in_and_out_of_ranges() {
    let (wb, p) = run(r##"Sub Main()
    Dim v As Variant, i As Long, total As Double, out() As Variant
    v = Range("A1:D8").Value
    ReDim out(1 To UBound(v, 1), 1 To 1)
    For i = 2 To UBound(v, 1)
        If Not IsEmpty(v(i, 3)) Then total = total + v(i, 3)
        out(i, 1) = UCase(v(i, 1))
    Next i
    out(1, 1) = "UPPER"
    Range("F1").Resize(UBound(out, 1), 1).Value = out
    Debug.Print total, LBound(v, 2), UBound(v, 2)
    Range("G1:G3").Value = Application.Transpose(Array(1, 2, 3))
End Sub"##);
    assert_eq!(p, [" 530           1             4 "]);
    assert_eq!(cell(&wb, 0, "F2"), "ANN");
    assert_eq!(cell(&wb, 0, "G3"), "3");
}

#[test]
fn formulas_written_from_code() {
    let (wb, _) = run(r##"Sub Main()
    Dim last As Long
    last = Cells(Rows.Count, "A").End(xlUp).Row
    Range("E2:E" & last).Formula = "=IF(C2="""","""",C2*1.21)"
    Range("E1").Value = "With VAT"
    Range("C" & last + 1).FormulaR1C1 = "=SUM(R2C:R[-1]C)"
    Range("E2:E" & last).Value = Range("E2:E" & last).Value
    Columns("A:E").AutoFit
    Range("C2:C" & last + 1).NumberFormat = "#,##0.00"
End Sub"##);
    assert_eq!(cell(&wb, 0, "E2"), "145.2");
    assert_eq!(cell(&wb, 0, "C9"), "530.00");
}

#[test]
fn autofilter_and_copy_visible() {
    let (wb, _) = run(r##"Sub Main()
    Dim ws As Worksheet
    Set ws = Worksheets.Add(After:=Sheets(1))
    ws.Name = "South"
    With Sheets("Data")
        .Range("A1:D8").AutoFilter Field:=2, Criteria1:="South"
        .Range("A1:D8").SpecialCells(xlCellTypeVisible).Copy ws.Range("A1")
        .AutoFilterMode = False
    End With
End Sub"##);
    assert_eq!(cell(&wb, 1, "A2"), "Bob");
    assert_eq!(cell(&wb, 1, "A3"), "Dee");
    assert_eq!(cell(&wb, 1, "A4"), "Bob");
    assert_eq!(cell(&wb, 1, "A5"), "");
}

#[test]
fn find_and_find_next() {
    let (_, p) = run(r##"Sub Main()
    Dim f As Range, first As String, n As Long
    With Worksheets("Data").Range("A:A")
        Set f = .Find(What:="Bob", LookIn:=xlValues, LookAt:=xlWhole)
        If Not f Is Nothing Then
            first = f.Address
            Do
                n = n + 1
                Set f = .FindNext(f)
            Loop While Not f Is Nothing And f.Address <> first
        End If
    End With
    Debug.Print n; first
    Debug.Print Application.WorksheetFunction.VLookup("Cid", Range("A:C"), 3, False)
    Debug.Print Application.Match("Dee", Range("A:A"), 0)
    Debug.Print IsError(Application.Match("Zed", Range("A:A"), 0))
End Sub"##);
    assert_eq!(p, [" 2 $A$3", " 200 ", " 7 ", "True"]);
}

#[test]
fn error_handling_patterns() {
    let (_, p) = run(r##"Function SheetExists(n As String) As Boolean
    Dim ws As Worksheet
    On Error Resume Next
    Set ws = ThisWorkbook.Sheets(n)
    On Error GoTo 0
    SheetExists = Not ws Is Nothing
End Function

Sub Main()
    On Error GoTo Fail
    Debug.Print SheetExists("Data"), SheetExists("Nope")
    Dim x As Long
    x = 1 / 0
    Debug.Print "not here"
    Exit Sub
Fail:
    Debug.Print "Error " & Err.Number & ": " & Err.Description
    Resume Next
End Sub"##);
    assert_eq!(p, ["True          False", "Error 11: Division by zero", "not here"]);
}

#[test]
fn strings_and_dates() {
    let (_, p) = run(r##"Sub Main()
    Dim s As String, parts() As String, d As Date
    s = "  Ann;Bob;Cid  "
    parts = Split(Trim(s), ";")
    Debug.Print UBound(parts), Join(parts, "-"), InStr(1, s, "bob", vbTextCompare)
    Debug.Print Left("ApexDesk", 4) & "|" & Mid("ApexDesk", 5) & "|" & Right("ApexDesk", 4) & "|" & Replace("a.b.c", ".", "/")
    d = DateSerial(2024, 1, 31)
    Debug.Print Format(DateAdd("m", 1, d), "yyyy-mm-dd"), Format(d, "dddd"), Weekday(d, vbMonday), DateDiff("d", d, #3/1/2024#)
    Debug.Print Format(1234.5, "#,##0.00"), Format(0.256, "0.0%"), CStr(CLng("42") + 1)
    Debug.Print Year(Range("D2").Value), Month(CDate("2024-05-06"))
End Sub"##);
    assert_eq!(
        p,
        [" 2            Ann-Bob-Cid    7 ", "Apex|Desk|Desk|a/b/c", "2024-02-29    Wednesday      3             30 ", "1,234.50      25.6%         43", " 2024          5 "]
    );
}

#[test]
fn classes_collections_and_types() {
    let mut wb = data_book();
    let mut ui = TestUi::default();
    let p = apex_vba::Project::parse(&[
        apex_vba::Source {
            name: "Module1".into(),
            kind: apex_vba::ModuleKind::Standard,
            code: r##"Type Sale
    Name As String
    Amount As Double
End Type

Sub Main()
    Dim c As New Collection, p As Person, s As Sale, all() As Sale, i As Long
    For i = 2 To 3
        Set p = New Person
        p.Name = Cells(i, 1).Value
        c.Add p, p.Name
    Next
    Debug.Print c.Count, c("Bob").Greeting
    ReDim all(1 To 2)
    all(1).Name = "x": all(1).Amount = 2
    s = all(1)
    s.Amount = 5
    Debug.Print all(1).Amount, s.Amount
End Sub"##
                .into(),
        },
        apex_vba::Source {
            name: "Person".into(),
            kind: apex_vba::ModuleKind::Class,
            code: r##"Private mName As String
Public Property Let Name(v As String)
    mName = v
End Property
Public Property Get Name() As String
    Name = mName
End Property
Public Function Greeting() As String
    Greeting = "Hi " & mName
End Function"##
                .into(),
        },
    ])
    .unwrap();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &apex_vba::Options::default(), "Main");
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.printed, [" 2            Hi Bob", " 2             5 "]);
}

#[test]
fn select_case_with_and_loops() {
    let (wb, p) = run(r##"Sub Main()
    Dim r As Long, grade As String, n As Long
    r = 2
    Do Until IsEmpty(Cells(r, 1)) And IsEmpty(Cells(r + 1, 1))
        Select Case Cells(r, 3).Value
            Case Is >= 150: grade = "A"
            Case 100 To 149: grade = "B"
            Case "", Empty: grade = "-"
            Case Else: grade = "C"
        End Select
        Cells(r, 5).Value = grade
        r = r + 1
        n = n + 1
        If n > 100 Then Exit Do
    Loop
    With Range("E1")
        .Value = "Grade"
        .Font.Bold = True
        .HorizontalAlignment = xlCenter
    End With
    Debug.Print n
End Sub"##);
    assert_eq!(p, [" 7 "]);
    assert_eq!(cell(&wb, 0, "E2"), "B");
    assert_eq!(cell(&wb, 0, "E5"), "A");
    assert_eq!(cell(&wb, 0, "E7"), "-");
}

#[test]
fn names_tables_and_validation() {
    let (wb, p) = run(r##"Sub Main()
    Dim lo As ListObject
    ThisWorkbook.Names.Add Name:="Amounts", RefersTo:="=Data!$C$2:$C$8"
    Debug.Print Application.Sum(Range("Amounts"))
    Set lo = ActiveSheet.ListObjects.Add(xlSrcRange, Range("A1:D3"), , xlYes)
    lo.Name = "Sales"
    Debug.Print lo.ListRows.Count, lo.ListColumns(3).Name
    With Range("F2").Validation
        .Delete
        .Add Type:=xlValidateList, Formula1:="North,South,East"
    End With
    Range("F2").Value = "North"
End Sub"##);
    assert_eq!(p, [" 530 ", " 2            Amount"]);
    assert_eq!(cell(&wb, 0, "F2"), "North");
}

#[test]
fn sheets_added_renamed_hidden_deleted() {
    let (wb, p) = run(r##"Sub Main()
    Dim ws As Worksheet
    Application.DisplayAlerts = False
    Set ws = Sheets.Add(After:=Sheets(Sheets.Count))
    ws.Name = "Report " & Format(DateSerial(2024, 3, 1), "mmm yy")
    ws.Tab.Color = RGB(0, 176, 80)
    Sheets("Data").Copy After:=Sheets(Sheets.Count)
    ActiveSheet.Name = "Data copy"
    Sheets("Data copy").Visible = xlSheetHidden
    For Each ws In Worksheets
        Debug.Print ws.Index & " " & ws.Name & " " & ws.Visible
    Next
    Sheets("Report Mar 24").Delete
    Application.DisplayAlerts = True
    Debug.Print Sheets.Count
End Sub"##);
    assert_eq!(p, ["1 Data -1", "2 Report Mar 24 -1", "3 Data copy 0", " 2 "]);
    assert_eq!(wb.sheets.len(), 2);
}

#[test]
fn message_and_input_boxes() {
    let mut wb = data_book();
    let mut ui = TestUi::default();
    ui.answers.push_back(vbyes());
    ui.inputs.push_back(Some("Zed".into()));
    let out = run_on(
        &mut wb,
        r##"Sub Main()
    Dim ans As VbMsgBoxResult, name As String
    ans = MsgBox("Add a row?", vbYesNo + vbQuestion, "Rows")
    If ans = vbYes Then
        name = InputBox("Name?", "New row", "Someone")
        If name <> "" Then Cells(Rows.Count, 1).End(xlUp).Offset(1, 0).Value = name
    End If
End Sub"##,
        &mut ui,
    );
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(cell(&wb, 0, "A9"), "Zed");
}

fn vbyes() -> i32 {
    6
}

#[test]
fn timer_status_and_speed_settings() {
    let (_, p) = run(r##"Sub Main()
    Dim t As Single, calc As XlCalculation
    t = Timer
    calc = Application.Calculation
    With Application
        .ScreenUpdating = False
        .EnableEvents = False
        .Calculation = xlCalculationManual
        .StatusBar = "Working..."
    End With
    DoEvents
    With Application
        .Calculation = calc
        .EnableEvents = True
        .ScreenUpdating = True
        .StatusBar = False
    End With
    Debug.Print Timer - t < 5, Application.Calculation = xlCalculationAutomatic
End Sub"##);
    assert_eq!(p, ["True          True"]);
}

#[test]
fn pivot_like_summary_with_dictionary() {
    let (wb, _) = run(r##"Sub Main()
    Dim d As Object, r As Long, k As Variant, i As Long
    Set d = CreateObject("Scripting.Dictionary")
    For r = 2 To 8
        If Cells(r, 2).Value <> "" Then d(Cells(r, 2).Value) = d(Cells(r, 2).Value) + Val(Cells(r, 3).Value)
    Next
    i = 1
    For Each k In d.Keys
        Cells(i, 8).Value = k
        Cells(i, 9).Value = d(k)
        i = i + 1
    Next
    Range("H1:I" & d.Count).Sort Key1:=Range("I1"), Order1:=xlDescending, Header:=xlNo
End Sub"##);
    assert_eq!(cell(&wb, 0, "H1"), "North");
    assert_eq!(cell(&wb, 0, "I1"), "320");
    assert_eq!(cell(&wb, 0, "H3"), "East");
}

#[test]
fn conditional_formatting_and_borders() {
    let (wb, p) = run(r##"Sub Main()
    With Range("C2:C8")
        .FormatConditions.Delete
        .FormatConditions.Add Type:=xlCellValue, Operator:=xlGreater, Formula1:="=100"
        .FormatConditions(1).Interior.Color = RGB(198, 239, 206)
    End With
    With Range("A1:D8").Borders
        .LineStyle = xlContinuous
        .Weight = xlThin
    End With
    Range("A1:D1").Borders(xlEdgeBottom).Weight = xlMedium
    Debug.Print Range("C2:C8").FormatConditions.Count
    Range("A1:D1").Merge
    Debug.Print Range("B1").MergeCells
End Sub"##);
    assert_eq!(p, [" 1 ", "True"]);
    let _ = wb;
}

#[test]
fn enums_as_types() {
    let (_, p) = run(r##"Enum Level
    Low = 1
    Mid
    High
End Enum

Function Rate(l As Level) As Double
    Select Case l
        Case Low: Rate = 0.1
        Case High: Rate = 0.3
        Case Else: Rate = 0.2
    End Select
End Function

Sub Main()
    Dim l As Level, c As XlCalculation, m As VbMsgBoxResult
    l = High
    c = xlCalculationManual
    m = vbNo
    Debug.Print l; Rate(Mid); c; m; TypeName(l)
End Sub"##);
    assert_eq!(p, [" 3  0.2 -4135  7 Long"]);
}

#[test]
fn autofilter_criteria() {
    let (wb, p) = run(r##"Sub Main()
    With Worksheets("Data").Range("A1:D8")
        .AutoFilter Field:=3, Criteria1:=">=100"
        Debug.Print .SpecialCells(xlCellTypeVisible).Count
        .AutoFilter Field:=3
        .AutoFilter Field:=1, Criteria1:=Array("Ann", "Dee"), Operator:=xlFilterValues
        Debug.Print Rows(3).Hidden, Rows(6).Hidden, Rows(7).Hidden
        .AutoFilter Field:=1, Criteria1:="B*", Operator:=xlOr, Criteria2:="C*"
        Debug.Print Rows(2).Hidden, Rows(3).Hidden
    End With
End Sub"##);
    assert_eq!(p, [" 12 ", "True          False         False", "True          False"]);
    assert!(wb.sheets[0].filter.is_some());
}

#[test]
fn find_previous_and_wildcards() {
    let (_, p) = run(r##"Sub Main()
    Dim f As Range
    Set f = Columns("A").Find("B?b", LookIn:=xlValues, LookAt:=xlWhole)
    Debug.Print f.Address
    Set f = Columns("A").FindNext(f)
    Debug.Print f.Address
    Set f = Columns("A").FindPrevious(f)
    Debug.Print f.Address
    Debug.Print Columns("A").Find("zz") Is Nothing
    Set f = Range("A1:D8").Find("*", SearchOrder:=xlByColumns, SearchDirection:=xlPrevious)
    Debug.Print f.Address
End Sub"##);
    assert_eq!(p, ["$A$3", "$A$8", "$A$3", "True", "$D$8"]);
}

#[test]
fn procedures_and_arguments() {
    let (_, p) = run(r##"Function Total(ParamArray xs() As Variant) As Double
    Dim x
    For Each x In xs
        Total = Total + x
    Next
End Function

Function Greet(Optional ByVal who As Variant) As String
    If IsMissing(who) Then who = "world"
    Greet = "Hello " & who
End Function

Sub Bump(ByRef n As Long, Optional ByVal by As Long = 1)
    n = n + by
End Sub

Function Counter() As Long
    Static n As Long
    n = n + 1
    Counter = n
End Function

Sub Main()
    Dim n As Long, a() As Long
    Bump n
    Bump n, 5
    Call Bump(n)
    Debug.Print Total(1, 2, 3.5); Greet(); Greet("Ann"); n
    Counter
    Counter
    Debug.Print Counter(); IIf(n > 5, "big", "small"); Choose(2, "a", "b", "c")
    ReDim a(2)
    a(1) = 7
    ReDim Preserve a(4)
    Debug.Print a(1); UBound(a); 17 Mod 5; 17 \ 5; 2 ^ 10
    Erase a
End Sub"##);
    assert_eq!(p, [" 6.5 Hello worldHello Ann 7 ", " 3 bigb", " 7  4  2  3  1024 "]);
}

#[test]
fn union_intersect_and_offsets() {
    let (wb, p) = run(r##"Sub Main()
    Dim u As Range, i As Range, c As Range
    Set u = Union(Range("A2"), Range("A5"), Range("C2:C3"))
    Debug.Print u.Areas.Count; u.Count; u.Address
    Set i = Intersect(Range("A1:C5"), Range("B3:D8"))
    Debug.Print i.Address; Intersect(Range("A1"), Range("D8")) Is Nothing
    For Each c In Range("A2:A3")
        c.Offset(0, 4).Value = c.Value & "-" & c.Offset(0, 1).Value
    Next
    Debug.Print Range("A1").Offset(2, 1).Resize(2, 2).Address(False, False)
    Debug.Print Cells(2, 3).Address(RowAbsolute:=False), Range("B2").Address(ReferenceStyle:=xlR1C1)
End Sub"##);
    assert_eq!(p, [" 3  4 $A$2,$A$5,$C$2:$C$3", "$B$3:$C$5True", "B3:C4", "$C2           R2C2"]);
    assert_eq!(cell(&wb, 0, "E3"), "Bob-South");
}

#[test]
fn worksheet_functions_from_code() {
    let (_, p) = run(r##"Sub Main()
    With Application.WorksheetFunction
        Debug.Print .SumIfs(Range("C:C"), Range("B:B"), "South"); .CountIf(Range("A:A"), "Ann"); .Max(Range("C2:C8")); .Round(2.567, 1)
        Debug.Print .Index(Range("A1:D8"), 5, 1); .Text(0.5, "0%"); .Trim("  a   b "); .Average(Range("C2:C3"))
    End With
    Debug.Print Evaluate("SUM(C2:C3)*2"); [C2] + 1; Application.Evaluate("=COUNTA(A:A)")
End Sub"##);
    assert_eq!(p, [" 160  2  200  2.6 ", "Cid50%a b 100 ", " 400  121  7 "]);
}

#[test]
fn comments_hyperlinks_and_shapes() {
    let (wb, p) = run(r##"Sub Main()
    Dim s As Shape
    Range("A2").AddComment "First row"
    Debug.Print Range("A2").Comment.Text
    ActiveSheet.Hyperlinks.Add Anchor:=Range("F1"), Address:="https://example.com", TextToDisplay:="Site"
    Set s = ActiveSheet.Shapes.AddShape(msoShapeRectangle, 10, 10, 100, 30)
    s.Name = "Box"
    s.TextFrame.Characters.Text = "Hi"
    Debug.Print ActiveSheet.Shapes.Count; ActiveSheet.Shapes("Box").Width
    ActiveSheet.Shapes("Box").Delete
    Debug.Print ActiveSheet.Shapes.Count; Range("F1").Text
End Sub"##);
    assert_eq!(p, ["First row", " 1  100 ", " 0 Site"]);
    let _ = wb;
}

#[test]
fn charts_from_code() {
    let (_, p) = run(r##"Sub Main()
    Dim co As ChartObject
    Set co = ActiveSheet.ChartObjects.Add(Left:=300, Top:=10, Width:=300, Height:=200)
    With co.Chart
        .SetSourceData Source:=Range("A1:A3,C1:C3")
        .ChartType = xlColumnClustered
        .HasTitle = True
        .ChartTitle.Text = "Sales"
    End With
    Debug.Print ActiveSheet.ChartObjects.Count; co.Chart.ChartTitle.Text
End Sub"##);
    assert_eq!(p, [" 1 Sales"]);
}

#[test]
fn files_text_and_fso() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("apex-vba-coverage");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("book")).unwrap();
    let code = String::from(
        r##"Sub Main()
    Dim f As Integer, line As String, fso As Object, p As String, n As Long
    p = "out.txt"
    f = FreeFile
    Open p For Output As #f
    Print #f, "a;1"
    Print #f, "b;2"
    Close #f
    f = FreeFile
    Open p For Input As #f
    Do While Not EOF(f)
        Line Input #f, line
        n = n + Val(Split(line, ";")(1))
    Loop
    Close #f
    Set fso = CreateObject("Scripting.FileSystemObject")
    Debug.Print n; fso.FileExists(p); Dir(p) <> ""; fso.GetFileName(p)
    Kill p
    Debug.Print Dir(p) = ""
End Sub"##
    );
    let mut wb = data_book();
    let mut ui = TestUi::default();
    let p = apex_vba::Project::parse(&[apex_vba::Source { name: "Module1".into(), kind: apex_vba::ModuleKind::Standard, code }]).unwrap();
    let opts = apex_vba::Options { file_path: dir.join("book").display().to_string(), file_name: "Book1.xlsm".into(), files: apex_vba::files::FileAccess { trusted: false, write: Some(true) }, ..apex_vba::Options::default() };
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.printed, [" 3 TrueTrueout.txt", "True"]);
}
