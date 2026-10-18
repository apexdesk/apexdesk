//! Parts of the workbook reached from its objects: Names, a sheet's Tab,
//! Hyperlinks, notes as Comment objects and data Validation.

mod common;

use apex_sheet::data::{CfKind, Cfvo, CmpOp, ErrorStyle, ValKind};
use apex_sheet::{CellRef, Range, Workbook};
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
fn names() {
    let mut wb = workbook();
    for i in 1..=3 {
        wb.set_input(0, c(&format!("A{i}")), &i.to_string());
    }
    let ui = main(
        &mut wb,
        r#"ThisWorkbook.Names.Add Name:="Total", RefersTo:=Range("A1:A3")
ActiveWorkbook.Names.Add "Rate", "=0.25"
Names.Add "Greeting", "hello"
Range("B1").Formula = "=SUM(Total)*Rate"
Debug.Print Range("B1").Value, Names.Count, Names("Total").RefersTo
Debug.Print Names(1).Name, Names("rate").Value, Names("Greeting").RefersTo
Debug.Print Names("Total").RefersToRange.Address, Range("Total").Count
Dim n As Name, s As String
For Each n In ThisWorkbook.Names
    s = s & n.Name & ";"
Next
Debug.Print s
Names("Rate").RefersTo = "=0.5"
Debug.Print Range("B1").Value
Worksheets(1).Names.Add "Local", "=Sheet1!$A$2"
Debug.Print Worksheets(1).Names.Count, Names("Sheet1!Local").Name, Names("Sheet1!Local").RefersToRange.Value
Names("Greeting").Delete
Debug.Print Names.Count
Names("Total").Name = "Amount"
Debug.Print Range("B1").Formula
On Error Resume Next
Names.Add "1bad", "=1"
Debug.Print Err.Number <> 0
Err.Clear
Debug.Print Names("Nope").Name
Debug.Print Err.Number"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        [" 1.5           3            =Sheet1!$A$1:$A$3", "Greeting      =0.25         =\"hello\"", "$A$1:$A$3      3 ", "Greeting;Rate;Total;", " 3 ", " 1            Sheet1!Local   2 ", " 3 ", "=SUM(Amount)*Rate", "True", " 9 "]
    );
}

#[test]
fn tab_colors() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        r#"Debug.Print ActiveSheet.Tab.Color, ActiveSheet.Tab.ColorIndex
ActiveSheet.Tab.Color = RGB(255, 0, 0)
Debug.Print ActiveSheet.Tab.Color = vbRed, ActiveSheet.Tab.ColorIndex"#,
    )
    .unwrap();
    assert_eq!(ui.printed, ["False         -4142 ", "True           3 "]);
    assert_eq!(wb.sheets[0].tab_color, Some(0xFF0000));
    main(&mut wb, "Sheets(1).Tab.ColorIndex = xlNone").unwrap();
    assert_eq!(wb.sheets[0].tab_color, None);
    main(&mut wb, "Sheets(1).Tab.ColorIndex = 5").unwrap();
    assert_eq!(wb.sheets[0].tab_color, Some(0x0000FF));
}

#[test]
fn hyperlinks() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        r#"ActiveSheet.Hyperlinks.Add Anchor:=Range("A1"), Address:="https://example.com", ScreenTip:="Site", TextToDisplay:="Example"
ActiveSheet.Hyperlinks.Add Range("A3"), "", "Sheet1!C5"
Debug.Print Range("A1").Value, Range("A3").Value, ActiveSheet.Hyperlinks.Count
Dim h As Hyperlink
For Each h In ActiveSheet.Hyperlinks
    Debug.Print h.Address & "|" & h.SubAddress & "|" & h.ScreenTip & "|" & h.Range.Address
Next
Range("A1").Hyperlinks(1).Address = "https://example.org"
Debug.Print Range("A1").Hyperlinks(1).Address, Range("A1:A2").Hyperlinks.Count
Range("A3").Hyperlinks.Delete
Debug.Print ActiveSheet.Hyperlinks.Count, Range("A3").Value
On Error Resume Next
ActiveSheet.Hyperlinks(1).Follow
Debug.Print Err.Number <> 0
Err.Clear
ActiveSheet.Hyperlinks.Add Range("B1"), "C:\Windows\System32\calc.exe"
Debug.Print Err.Number; ActiveSheet.Hyperlinks.Count
Err.Clear
Range("A1").Hyperlinks(1).Address = "file:///etc/passwd"
Debug.Print Err.Number; Range("A1").Hyperlinks(1).Address"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        ["Example       Sheet1!C5      2 ", "https://example.com||Site|$A$1", "|Sheet1!C5||$A$3", "https://example.org          1 ", " 1            Sheet1!C5", "True", " 1004  1 ", " 1004 https://example.org"]
    );
    let l = wb.cell(0, c("A1")).unwrap().link().unwrap();
    assert_eq!((l.target.as_str(), l.tip.as_str()), ("https://example.org", "Site"));
}

#[test]
fn comments() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        r#"Dim k As Comment
Set k = Range("B2").AddComment("Check this")
Range("A1").AddComment "First"
Debug.Print k.Text, TypeName(k), ActiveSheet.Comments.Count, Range("C3").Comment Is Nothing
Debug.Print ActiveSheet.Comments(1).Text, ActiveSheet.Comments(1).Next.Text, ActiveSheet.Comments(1).Previous Is Nothing
k.Text " please", 11
Debug.Print Range("B2").Comment.Text
k.Text "CHECK", 1, True
Debug.Print k.Text, k.Parent.Address
k.Visible = True
Debug.Print Range("B2").Comment.Visible
Dim x As Comment, s As String
For Each x In ActiveSheet.Comments
    s = s & x.Parent.Address(0, 0) & ";"
Next
Debug.Print s
Range("A1").Comment.Delete
Debug.Print ActiveSheet.Comments.Count"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        ["Check this    Comment        2            True", "First         Check this    True", "Check this please", "CHECK this please           $B$2", "True", "A1;B2;", " 1 "]
    );
    assert!(wb.cell(0, c("B2")).unwrap().note().unwrap().visible);
}

#[test]
fn validation() {
    let mut wb = workbook();
    wb.set_input(0, c("D1"), "x");
    let ui = main(
        &mut wb,
        r#"With Range("A1:A5").Validation
    .Delete
    .Add Type:=xlValidateList, AlertStyle:=xlValidAlertStop, Formula1:="Yes,No"
    .InputTitle = "Answer"
    .ErrorMessage = "Pick one"
End With
With Range("B1").Validation
    .Add xlValidateWholeNumber, xlValidAlertWarning, xlBetween, "1", "10"
    Debug.Print .Type, .Operator, .Formula1, .Formula2, .AlertStyle
End With
Range("C1").Validation.Add xlValidateDecimal, , xlGreater, "=$D$2"
Debug.Print Range("A3").Validation.Formula1, Range("A3").Validation.Type, Range("A3").Validation.InputTitle
Debug.Print Range("C1").Validation.Formula1
Range("B1").Value = 50
Debug.Print Range("B1").Validation.Value
Range("A2:A3").Validation.Delete
On Error Resume Next
Debug.Print Range("A2").Validation.Type
Debug.Print Err.Number
Err.Clear
Range("A1").Validation.Add xlValidateList, , , "a"
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(ui.printed, [" 1             1            1             10             2 ", "Yes,No         3            Answer", "=$D$2", "False", " 1004 ", "True"]);
    let list = &wb.sheets[0].validations;
    let a = list.iter().find(|v| v.contains(c("A1"))).unwrap();
    assert_eq!((a.kind, a.f1.as_str(), a.input_title.as_str(), a.error_msg.as_str()), (ValKind::List, "\"Yes,No\"", "Answer", "Pick one"));
    assert!(!a.contains(c("A2")) && !a.contains(c("A3")) && a.contains(c("A5")));
    let b = list.iter().find(|v| v.contains(c("B1"))).unwrap();
    assert_eq!((b.kind, b.op, b.error_style), (ValKind::Whole, CmpOp::Between, ErrorStyle::Warning));
    let k = list.iter().find(|v| v.contains(c("C1"))).unwrap();
    assert_eq!((k.op, k.f1.as_str()), (CmpOp::Greater, "$D$2"));
}

#[test]
fn formula2_is_formula() {
    let mut wb = workbook();
    let ui = main(&mut wb, "Range(\"A1\").Formula2 = \"=1+2\"\nDebug.Print Range(\"A1\").Formula2, Range(\"A1\").Value").unwrap();
    assert_eq!(ui.printed, ["=1+2           3 "]);
}

#[test]
fn conditional_formats() {
    let mut wb = workbook();
    for i in 1..=5 {
        wb.set_input(0, c(&format!("A{i}")), &(i * 10).to_string());
    }
    let ui = main(
        &mut wb,
        r#"Range("B3").Select
With Range("A1:A5").FormatConditions.Add(Type:=xlCellValue, Operator:=xlGreater, Formula1:="=25")
    .Font.Bold = True
    .Font.Color = -16383844
    .Interior.Color = 13551615
    .Interior.TintAndShade = 0
    .StopIfTrue = False
End With
Range("A1:A5").FormatConditions.Add xlExpression, , "=$A3>$B3"
Range("A1:A5").FormatConditions.Add Type:=xlTextString, String:="x", TextOperator:=xlBeginsWith
Dim cs As Object
Set cs = Range("A1:A5").FormatConditions.AddColorScale(ColorScaleType:=3)
cs.ColorScaleCriteria(1).FormatColor.Color = RGB(255, 0, 0)
cs.ColorScaleCriteria(2).Type = xlConditionValuePercent
cs.ColorScaleCriteria(2).Value = 40
With Range("A1:A5").FormatConditions.AddIconSetCondition
    .IconSet = ActiveWorkbook.IconSets(xl3Flags)
    .ReverseOrder = True
End With
Range("A1:A5").FormatConditions.AddDatabar.BarColor.Color = RGB(0, 0, 255)
Range("A1:A5").FormatConditions(Range("A1:A5").FormatConditions.Count).SetFirstPriority
With Range("A1:A5").FormatConditions
    Debug.Print .Count, .Item(1).Type, .Item(2).Formula1, .Item(2).Operator, .Item(2).Font.Bold, .Item(2).Priority
    Debug.Print .Item(3).Formula1, .Item(4).Text, .Item(4).TextOperator, .Item(5).ColorScaleCriteria(2).Value
    Debug.Print .Item(2).Interior.Color = 13551615, .Item(2).AppliesTo.Address, TypeName(.Item(2))
End With
Range("A1:A5").FormatConditions(1).Delete
Range("A3").FormatConditions.Delete
Debug.Print Range("A1:A5").FormatConditions.Count, Range("A3").FormatConditions.Count
On Error Resume Next
Range("A1").FormatConditions.Add 99
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(ui.printed, [" 6             4            =25            5            True           2 ", "=$A1>$B1      x              2             40 ", "True          $A$1:$A$5     FormatCondition", " 5             0 ", "True"]);
    let list = &wb.sheets[0].cond_formats;
    assert_eq!(list.len(), 5);
    let first = &list[0];
    assert_eq!(first.kind, CfKind::CellIs { op: CmpOp::Greater, f1: "25".into(), f2: String::new() });
    assert_eq!((first.style.bold, first.style.color, first.style.fill), (Some(true), Some(0x9C0006), Some(0xFFC7CE)));
    assert_eq!(first.ranges, [Range::parse("A1:A2").unwrap(), Range::parse("A4:A5").unwrap()]);
    let CfKind::ColorScale(stops) = &list[3].kind else { panic!("{:?}", list[3].kind) };
    assert_eq!((stops[0].1, &stops[1].0), (0xFF0000, &Cfvo::Percent(40.0)));
    let CfKind::IconSet { set, reverse, .. } = &list[4].kind else { panic!() };
    assert_eq!((set.as_str(), *reverse), ("3Flags", true));
}

#[test]
fn page_setup() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        r#"With ActiveSheet.PageSetup
    .Orientation = xlLandscape
    .PaperSize = xlPaperLetter
    .Zoom = False
    .FitToPagesWide = 1
    .FitToPagesTall = False
    .LeftMargin = Application.InchesToPoints(0.5)
    .TopMargin = Application.CentimetersToPoints(2.54)
    .PrintArea = "$A$1:$F$40"
    .PrintTitleRows = "$1:$2"
    .PrintTitleColumns = "$A:$B"
    .CenterHeader = "&A"
    .RightFooter = "Page &P of &N"
    .PrintGridlines = True
    .CenterHorizontally = True
    .BlackAndWhite = False
    .PrintComments = xlPrintNoComments
    .FirstPageNumber = xlAutomatic
    .DifferentFirstPageHeaderFooter = True
    .FirstPage.CenterHeader.Text = "Cover"
    Debug.Print .Orientation, .PaperSize, .Zoom, .FitToPagesWide, .FitToPagesTall, .LeftMargin, .TopMargin
    Debug.Print .PrintArea, .PrintTitleRows, .PrintTitleColumns, .CenterHeader, .FirstPage.CenterHeader.Text
End With
ActiveSheet.HPageBreaks.Add Before:=Range("A20")
Range("A30").PageBreak = xlPageBreakManual
Debug.Print ActiveSheet.HPageBreaks.Count, ActiveSheet.HPageBreaks(2).Location.Row, Range("A20").PageBreak, Range("A21").PageBreak
ActiveSheet.HPageBreaks(1).Delete
Debug.Print ActiveSheet.HPageBreaks.Count
ActiveSheet.ResetAllPageBreaks
Debug.Print ActiveSheet.HPageBreaks.Count
ActiveSheet.PageSetup.Zoom = 80
Debug.Print ActiveSheet.PageSetup.Zoom
On Error Resume Next
ActiveSheet.PageSetup.Zoom = 5
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        [" 2             1            False          1            False          36            72 ", "$A$1:$F$40    $1:$2         $A:$B         &A            Cover", " 2             30           -4135         -4142 ", " 1 ", " 0 ", " 80 ", "True"]
    );
    let p = &wb.sheets[0].page;
    assert!(p.landscape && p.gridlines && p.center_h);
    assert_eq!((p.paper_width, p.title_rows, p.title_cols), (612.0, Some((0, 1)), Some((0, 1))));
    assert_eq!((p.hf.footer.right.as_str(), p.hf.first_header.center.as_str()), ("Page &P of &N", "Cover"));
    assert_eq!(p.print_areas, [Range::parse("A1:F40").unwrap()]);
}

#[test]
fn tables() {
    let mut wb = workbook();
    for (i, row) in [["Item", "Qty", "Price"], ["Pen", "2", "1.5"], ["Ink", "4", "3"]].iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            wb.set_input(0, CellRef::new(i as u32, j as u32), v);
        }
    }
    wb.set_input(0, c("A10"), "below");
    let ui = main(
        &mut wb,
        r#"Dim t As ListObject
Set t = ActiveSheet.ListObjects.Add(xlSrcRange, Range("A1:C3"), , xlYes)
t.Name = "Sales"
Debug.Print t.Name, t.Range.Address, t.DataBodyRange.Address, t.HeaderRowRange.Address, t.ListColumns.Count, t.ListRows.Count
Debug.Print t.ListColumns("Qty").Index, t.ListColumns(3).Name, t.ListColumns(2).DataBodyRange.Address, Range("B2").ListObject.Name
t.ListRows.Add
t.ListRows(3).Range.Value = Array("Cap", 1, 2)
t.ListRows.Add 1
Debug.Print t.Range.Address, Range("A3").Value, Range("A5").Value, Range("A12").Value
t.ListRows(1).Delete
t.ListColumns.Add.Name = "Total"
Debug.Print t.Range.Address, Range("D1").Value, t.ListColumns(4).Name
t.ShowTotals = True
t.ListColumns("Qty").TotalsCalculation = xlTotalsCalculationSum
Debug.Print t.TotalsRowRange.Address, Range("B5").Value
t.TableStyle = "TableStyleLight9"
t.ShowTableStyleRowStripes = False
Dim n As String, lc As ListColumn
For Each lc In t.ListColumns
    n = n & lc.Name & ";"
Next
Debug.Print n, ActiveSheet.ListObjects.Count, ActiveSheet.ListObjects("sales").Name
t.ListColumns(4).Delete
Debug.Print t.Range.Address
t.Unlist
Debug.Print ActiveSheet.ListObjects.Count, Range("B2").ListObject Is Nothing
On Error Resume Next
ActiveSheet.ListObjects.Add xlSrcRange, Range("A1:A1"), , xlYes
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        ["Sales         $A$1:$C$3     $A$2:$C$3     $A$1:$C$1      3             2 ", " 2            Price         $B$2:$B$3     Sales", "$A$1:$C$5     Pen           Cap           below", "$A$1:$D$4     Total         Total", "$A$5:$D$5      7 ", "Item;Qty;Price;Total;        1            Sales", "$A$1:$C$5", " 0            True", "True"]
    );
    assert_eq!(text(&wb, 0, "A11"), "below");
}

#[test]
fn input_box_reference() {
    let mut wb = workbook();
    let mut ui = TestUi::default();
    ui.inputs.extend([Some("Q0".to_string()), Some("=b2:c3".to_string()), None]);
    let out = run_on(
        &mut wb,
        r#"Sub Main()
Dim r As Range, v As Variant
Set r = Application.InputBox("Pick", Type:=8)
Debug.Print r.Address
v = Application.InputBox("Again", Type:=8)
Debug.Print v
End Sub
"#,
        &mut ui,
    );
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.printed, ["$B$2:$C$3", "False"]);
    assert_eq!(ui.shown, ["? Pick", "The reference you typed isn't valid.", "? Pick", "? Again"]);
}

#[test]
fn text_to_columns() {
    let mut wb = workbook();
    wb.set_inputs(0, vec![(c("A1"), "a,b;c".into()), (c("A2"), "\"x,y\",2".into()), (c("A3"), "1,,3".into()), (c("E1"), "abcdef".into())]);
    main(
        &mut wb,
        r#"Range("A1:A3").TextToColumns Destination:=Range("B1"), DataType:=xlDelimited, Comma:=True, Semicolon:=True, FieldInfo:=Array(Array(2, xlSkipColumn))
Range("E1").TextToColumns DataType:=xlFixedWidth, FieldInfo:=Array(Array(0, 1), Array(2, 1), Array(4, 1))
On Error Resume Next
Range("A1:B1").TextToColumns
Debug.Print Err.Number"#,
    )
    .unwrap();
    for (cell, want) in [("B1", "a"), ("C1", "c"), ("B2", "x,y"), ("C2", ""), ("B3", "1"), ("C3", "3"), ("E1", "ab"), ("F1", "cd"), ("G1", "ef")] {
        assert_eq!(text(&wb, 0, cell), want, "{cell}");
    }
}

#[test]
fn advanced_filter() {
    let mut wb = workbook();
    let rows = [("Item", "Qty"), ("pen", "5"), ("cap", "1"), ("pen", "5"), ("ink", "9")];
    for (i, (a, b)) in rows.iter().enumerate() {
        wb.set_input(0, c(&format!("A{}", i + 1)), a);
        wb.set_input(0, c(&format!("B{}", i + 1)), b);
    }
    wb.set_input(0, c("D1"), "Qty");
    wb.set_input(0, c("D2"), ">2");
    let ui = main(
        &mut wb,
        r#"Range("A1").AdvancedFilter xlFilterCopy, Range("D1:D2"), Range("F1"), True
Range("A1:B5").AdvancedFilter Action:=xlFilterInPlace, CriteriaRange:=Range("D1:D2")
Debug.Print ActiveSheet.FilterMode, Rows(3).Hidden, Rows(4).Hidden
ActiveSheet.ShowAllData
Debug.Print ActiveSheet.FilterMode, Rows(3).Hidden
On Error Resume Next
ActiveSheet.ShowAllData
Debug.Print Err.Number <> 0
Err.Clear
Range("A1:B5").AdvancedFilter xlFilterCopy
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(ui.printed, ["True          True          False", "False         False", "True", "True"]);
    for (cell, want) in [("F1", "Item"), ("F2", "pen"), ("G2", "5"), ("F3", "ink"), ("F4", "")] {
        assert_eq!(text(&wb, 0, cell), want, "{cell}");
    }
}

#[test]
fn characters() {
    let mut wb = workbook();
    wb.set_input(0, c("A1"), "Hello world");
    wb.set_input(0, c("A2"), "42");
    let ui = main(
        &mut wb,
        r#"With Range("A1").Characters(7, 5).Font
    .Bold = True
    .Color = RGB(255, 0, 0)
End With
Range("A1").Characters(1, 1).Font.Superscript = True
Debug.Print Range("A1").Characters(7, 5).Font.Bold, Range("A1").Characters(1, 5).Font.Bold, IsNull(Range("A1").Characters.Font.Bold)
Debug.Print Range("A1").Characters(7).Text, Range("A1").Characters(3, 2).Count, Range("A1").Characters(1, 1).Font.Superscript
Range("A1").Characters(1, 5).Text = "Hi"
Debug.Print Range("A1").Value, Range("A1").Characters(4, 5).Font.Bold, Range("A1").Characters(4, 5).Font.Color = RGB(255, 0, 0)
Range("A1").Characters(3, 1).Delete
Range("A1").Characters(1, 0).Insert ">"
Debug.Print Range("A1").Value, Range("A1").Characters(4).Font.Bold
Range("A2").Characters(1, 1).Font.Italic = True
Debug.Print Range("A2").Font.Italic, Range("A2").Characters(2).Text
On Error Resume Next
Range("A1").Characters(0, 1).Font.Bold = True
Debug.Print Err.Number"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        ["True          False         True", "world          2            True", "Hi world      True          True", ">Hiworld      True", "True          2", " 5 "]
    );
    assert!(wb.cell(0, c("A1")).and_then(|x| x.rich()).is_some());
}

#[test]
fn shapes() {
    let mut wb = workbook();
    let ui = main(
        &mut wb,
        r#"Dim s As Shape
Set s = ActiveSheet.Shapes.AddShape(msoShapeRectangle, 72, 36, 144, 72)
s.Name = "Box"
s.TextFrame.Characters.Text = "Hello" & vbCr & "there"
s.TextFrame2.TextRange.Font.Bold = True
s.Fill.ForeColor.RGB = RGB(255, 0, 0)
s.Line.Visible = msoFalse
s.OnAction = "Macro1"
Debug.Print s.Left, s.Top, s.Width, s.Type, s.AutoShapeType, s.TopLeftCell.Address
s.Left = 100
s.IncrementTop 10
Debug.Print s.Left, s.Top, s.TextFrame.Characters.Text = "Hello" & vbLf & "there", s.Fill.ForeColor.RGB = RGB(255, 0, 0), s.Line.Visible
ActiveSheet.Shapes.AddTextbox msoTextOrientationHorizontal, 0, 0, 50, 20
ActiveSheet.Shapes.AddLine 10, 0, 0, 50
Dim n As String, x As Shape
For Each x In ActiveSheet.Shapes
    n = n & x.Type & ";"
Next
Debug.Print ActiveSheet.Shapes.Count, n, ActiveSheet.Shapes("box").OnAction
ActiveSheet.Shapes(1).Delete
Debug.Print ActiveSheet.Shapes.Count
On Error Resume Next
Debug.Print s.Name
Debug.Print Err.Number"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        [" 72            36            144           1             1            $B$3", " 100           46           True          True           0 ", " 3            1;17;9;       Macro1", " 2 ", " 1004 "]
    );
    let sh = &wb.sheets[0];
    assert_eq!(sh.shapes.len(), 2);
    assert!(sh.shapes[1].shape.flip_h && !sh.shapes[1].shape.flip_v);
}

#[test]
fn charts() {
    let mut wb = workbook();
    let rows = [("Month", "Sales"), ("Jan", "10"), ("Feb", "30"), ("Mar", "20")];
    for (i, (a, b)) in rows.iter().enumerate() {
        wb.set_input(0, c(&format!("A{}", i + 1)), a);
        wb.set_input(0, c(&format!("B{}", i + 1)), b);
    }
    let ui = main(
        &mut wb,
        r#"Dim co As ChartObject
Set co = ActiveSheet.ChartObjects.Add(Left:=200, Top:=10, Width:=300, Height:=200)
With co.Chart
    .SetSourceData Source:=Range("A1:B4")
    .ChartType = xlLineMarkers
    .HasTitle = True
    .ChartTitle.Text = "Sales by month"
    .Axes(xlValue).MaximumScale = 50
    .Axes(xlValue).HasTitle = True
    .Axes(xlValue).AxisTitle.Text = "Units"
    .Axes(xlCategory).HasMajorGridlines = True
    .HasLegend = True
    .Legend.Position = xlLegendPositionTop
End With
co.Name = "Trend"
Debug.Print co.Chart.ChartType, co.Chart.SeriesCollection.Count, co.Chart.SeriesCollection(1).Name, co.Left, co.Width
Debug.Print co.Chart.Axes(xlValue).MaximumScale, co.Chart.Axes(xlValue).MinimumScaleIsAuto, co.Chart.Legend.Position
With co.Chart.SeriesCollection.NewSeries
    .Name = "Target"
    .Values = Array(15, 15, 15)
    .XValues = Range("A2:A4")
End With
Debug.Print co.Chart.SeriesCollection(2).Values(3), co.Chart.SeriesCollection("Target").Name
ActiveSheet.Shapes.AddChart2(201, xlColumnClustered).Select
ActiveChart.SetSourceData Range("A1:B4")
Debug.Print ActiveSheet.ChartObjects.Count, ActiveChart.ChartType, ActiveChart.SeriesCollection.Count, ActiveSheet.ChartObjects("trend").Index
ActiveSheet.ChartObjects(2).Delete
Debug.Print ActiveChart Is Nothing, ActiveSheet.ChartObjects.Count
On Error Resume Next
co.Chart.ChartType = 9999
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        [" 65            1            Sales          200           300 ", " 50           True          -4160 ", " 15           Target", " 2             51            1             1 ", "True           1 ", "True"]
    );
    let ch = &wb.sheets[0].charts[0];
    assert_eq!(ch.name, "Trend");
    assert_eq!(ch.title, "Sales by month");
    assert_eq!(ch.y_axis.title, "Units");
    assert!(ch.markers && ch.x_axis.major_grid);
}

#[test]
fn styles() {
    let mut wb = workbook();
    wb.set_input(0, c("A1"), "1");
    wb.set_input(0, c("A2"), "2");
    let ui = main(
        &mut wb,
        r#"Debug.Print Range("A1").Style, Range("A1").Style.Name, ActiveWorkbook.Styles("Good").BuiltIn
Range("A1").Style = "Good"
Debug.Print Range("A1").Style.Name
Range("A3").Font.Bold = True
Dim st As Style
Set st = ActiveWorkbook.Styles.Add("Totals", Range("A3"))
Debug.Print st.Name, st.BuiltIn, st.Font.Bold
st.NumberFormat = "0.00"
st.Interior.Color = RGB(255, 0, 0)
Range("A2").Style = "Totals"
Debug.Print Range("A2").NumberFormat, Range("A2").Font.Bold, Range("A2").Interior.Color = RGB(255, 0, 0)
st.Font.Italic = True
Debug.Print Range("A2").Font.Italic
Dim n As Long, x As Style
For Each x In ActiveWorkbook.Styles
    n = n + 1
Next
Debug.Print n = ActiveWorkbook.Styles.Count
st.Delete
Debug.Print Range("A2").NumberFormat
On Error Resume Next
Range("A1").Style = "Nope"
Debug.Print Err.Number <> 0
Err.Clear
ActiveWorkbook.Styles.Add "Good"
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(
        ui.printed,
        [
            "Normal        Normal        True",
            "Good",
            "Totals        False         True",
            "0.00          True          True",
            "True",
            "True",
            "General",
            "True",
            "True",
        ]
    );
}

#[test]
fn print_out_opens_the_print_dialog_once() {
    let mut wb = workbook();
    let mut ui = TestUi::default();
    let out = run_on(&mut wb, "Sub Main()\nDim i As Long\nFor i = 1 To 50\n    Worksheets(1).PrintOut\nNext\nActiveWorkbook.PrintOut\nEnd Sub\n", &mut ui);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.requests, [apex_vba::Request::Print { sheet: Some(0) }]);
}

#[test]
fn pivot_tables() {
    use apex_sheet::analysis::pivot::{Field, PivotSpec, ValueField};
    let mut wb = workbook();
    for (i, (r, v)) in [("Region", "Sales"), ("East", "10"), ("West", "5"), ("East", "7")].iter().enumerate() {
        wb.set_input(0, c(&format!("A{}", i + 1)), r);
        wb.set_input(0, c(&format!("B{}", i + 1)), v);
    }
    let spec = PivotSpec { source_sheet: "Sheet1".into(), source: Range::new(c("A1"), c("B4")), rows: vec![Field::new(0)], values: vec![ValueField { col: 1, ..Default::default() }], ..Default::default() };
    wb.create_pivot(spec, Some((0, c("D1")))).unwrap();
    let ui = main(
        &mut wb,
        r#"Dim pt As PivotTable
Debug.Print ActiveSheet.PivotTables.Count, ActiveSheet.PivotTables(1).Name, Range("E2").PivotTable.Name
Range("B3").Value = 50
Set pt = ActiveSheet.PivotTables("PivotTable1")
pt.PivotCache.Refresh
Debug.Print Range("E3").Value, pt.TableRange1.Address, pt.SourceData
Range("B3").Value = 1
ThisWorkbook.RefreshAll
Debug.Print Range("E3").Value
For Each pt In ActiveSheet.PivotTables
    pt.RefreshTable
Next
On Error Resume Next
Debug.Print Range("A1").PivotTable.Name
Debug.Print Err.Number <> 0"#,
    )
    .unwrap();
    assert_eq!(ui.printed, [" 1            PivotTable1   PivotTable1", " 50           $D$1:$E$4     'Sheet1'!$A$1:$B$4", " 1 ", "True"]);
}
