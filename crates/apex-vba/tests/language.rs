//! The VBA language: types, operators, control flow, procedures, errors,
//! arrays, collections and the built-in functions.

mod common;

use common::*;

#[test]
fn arithmetic_and_types() {
    assert_eq!(
        body("Debug.Print 1 + 2 * 3\nDebug.Print 7 \\ 2, 7 Mod 3, 2 ^ 10\nDebug.Print 10 / 4\nDebug.Print -2 ^ 2\nDebug.Print \"a\" & 1 & True\nDebug.Print \"2\" + 3\nDebug.Print TypeName(1), TypeName(100000), TypeName(1.5), TypeName(\"x\")"),
        [" 7 ", " 3             1             1024 ", " 2.5 ", "-4 ", "a1True", " 5 ", "Integer       Long          Double        String"]
    );
    assert_eq!(body("Dim i As Integer\ni = 2.5\nDebug.Print i\ni = 3.5\nDebug.Print i\nDim s As String\ns = 12\nDebug.Print TypeName(s)"), [" 2 ", " 4 ", "String"]);
    assert_eq!(error("Dim i As Integer\ni = 40000").number, 6);
    assert_eq!(error("Dim l As Long\nl = \"abc\"").number, 13);
    assert_eq!(error("x = 1 / 0").number, 11);
    assert_eq!(body("Dim b As Boolean\nb = 5\nDebug.Print b, Not b, True And False, True Or False, 6 And 3, 5 Xor 1"), ["True          False         False         True           2             4 "]);
    assert_eq!(body("Debug.Print 1 < 2, \"a\" < \"b\", \"B\" = \"b\", 3 = \"3\""), ["True          True          False         True"]);
    assert_eq!(body("Debug.Print \"hello\" Like \"h*o\", \"x1\" Like \"x#\""), ["True          True"]);
    assert_eq!(body("Debug.Print 0.1 + 0.2, 1E+20, 1 / 3"), [" 0.3           1E+20         0.333333333333333 "]);
}

#[test]
fn option_compare_text_and_explicit() {
    assert_eq!(printed("Option Compare Text\nSub Main()\nDebug.Print \"ABC\" = \"abc\", InStr(\"Hello\", \"LL\")\nEnd Sub"), ["True           3 "]);
    let mut wb = workbook();
    let mut ui = TestUi::default();
    let out = run_on(&mut wb, "Option Explicit\nSub Main()\n  Dim a\n  a = 1\n  b = 2\nEnd Sub", &mut ui);
    let e = out.error.unwrap();
    assert!(e.message.contains("Variable not defined"), "{e}");
    assert_eq!((e.module.as_str(), e.line), ("Module1", 5));
}

#[test]
fn control_flow() {
    let out = body(
        "Dim i As Long, s As String, t As Long
For i = 1 To 5
  If i Mod 2 = 0 Then
    s = s & \"e\"
  ElseIf i = 5 Then
    s = s & \"5\"
  Else
    s = s & \"o\"
  End If
Next i
Debug.Print s
For i = 10 To 1 Step -3: t = t + i: Next
Debug.Print t
i = 0
Do While i < 3
  i = i + 1
Loop
Debug.Print i
Do
  i = i + 1
  If i = 7 Then Exit Do
Loop Until i > 100
Debug.Print i
While i > 0
  i = i - 2
Wend
Debug.Print i
For i = 1 To 10
  If i = 4 Then Exit For
Next
Debug.Print i",
    );
    assert_eq!(out, ["oeoe5", " 22 ", " 3 ", " 7 ", "-1 ", " 4 "]);
}

#[test]
fn select_case() {
    let code = "Function Kind(x)
  Select Case x
    Case 1, 2: Kind = \"small\"
    Case 3 To 5
      Kind = \"mid\"
    Case \"a\"
      Kind = \"letter\"
    Case Is >= 100
      Kind = \"big\"
    Case Else
      Kind = \"other\"
  End Select
End Function
Sub Main()
  Debug.Print Kind(2), Kind(4), Kind(500), Kind(\"a\"), Kind(50)
End Sub";
    assert_eq!(printed(code), ["small         mid           big           letter        other"]);
}

#[test]
fn procedures_byref_byval_optional() {
    let code = "Sub Twice(ByRef n As Long)
  n = n * 2
End Sub
Sub Keep(ByVal n As Long)
  n = n * 2
End Sub
Function Add(a As Long, Optional b As Long = 10, Optional c) As Long
  If IsMissing(c) Then Add = a + b Else Add = a + b + c
End Function
Function Total(ParamArray xs()) As Double
  Dim x
  For Each x In xs: Total = Total + x: Next
End Function
Function Fact(n As Long) As Double
  If n <= 1 Then Fact = 1 Else Fact = n * Fact(n - 1)
End Function
Sub Main()
  Dim v As Long
  v = 3
  Twice v
  Debug.Print v
  Keep v
  Debug.Print v
  Call Twice(v)
  Debug.Print v
  Twice (v)
  Debug.Print v
  Debug.Print Add(1), Add(1, 2), Add(1, 2, 3), Add(1, c:=5)
  Debug.Print Total(1, 2, 3.5), Fact(10)
End Sub";
    assert_eq!(printed(code), [" 6 ", " 6 ", " 12 ", " 12 ", " 11            3             6             16 ", " 6.5           3628800 "]);
}

#[test]
fn exit_and_static_and_module_vars() {
    let code = "Private counter As Long
Public Const GREETING = \"hi\"
Function Count() As Long
  Static n As Long
  n = n + 1
  counter = counter + 10
  Count = n
  Exit Function
  Count = 99
End Function
Sub Main()
  Count
  Count
  Debug.Print Count(), counter, GREETING
End Sub";
    assert_eq!(printed(code), [" 3             30           hi"]);
}

#[test]
fn on_error() {
    let code = "Sub Main()
  Dim x
  On Error Resume Next
  x = 1 / 0
  Debug.Print Err.Number, Err.Description
  Err.Clear
  Debug.Print Err.Number
  On Error GoTo Handler
  x = CLng(\"nope\")
  Debug.Print \"not here\"
Done:
  Debug.Print \"done\"
  Exit Sub
Handler:
  Debug.Print \"caught\", Err.Number
  Resume Done
End Sub";
    assert_eq!(printed(code), [" 11           Division by zero", " 0 ", "caught         13 ", "done"]);
    let code = "Sub Main()
  On Error GoTo H
  Err.Raise 1234, , \"custom\"
  Debug.Print \"after\"
  Exit Sub
H:
  Debug.Print Err.Number, Err.Description
  Resume Next
End Sub";
    assert_eq!(printed(code), [" 1234         custom", "after"]);
    // An error in a called procedure reaches the caller's handler.
    let code = "Sub Fails()
  Dim a(2)
  a(5) = 1
End Sub
Sub Main()
  On Error Resume Next
  Fails
  Debug.Print Err.Number
End Sub";
    assert_eq!(printed(code), [" 9 "]);
}

#[test]
fn errors_name_module_and_line() {
    let e = error("Dim a\n\na = 1\nFoo 1, 2");
    assert_eq!(e.number, 35);
    assert_eq!(e.line, 5);
    assert_eq!(e.module, "Module1");
    let e = error("Dim s\nSet s = 5");
    assert_eq!(e.number, 424);
    let e = apex_vba::Project::parse(&[apex_vba::Source { name: "Mod".into(), kind: apex_vba::ModuleKind::Standard, code: "Sub A()\n  For i = 1 To 3\n  x = 1\nEnd Sub\n".into() }]).unwrap_err();
    assert_eq!(e.module, "Mod");
    assert!(e.message.contains("Next"), "{e}");
}

#[test]
fn with_blocks_and_objects() {
    let code = "Sub Main()
  Dim c As New Collection
  With c
    .Add \"a\"
    .Add \"b\", \"kb\"
    .Add \"z\", Before:=1
    Debug.Print .Count, .Item(1), .Item(\"KB\"), c(2)
  End With
  Dim d As Object
  Set d = CreateObject(\"Scripting.Dictionary\")
  d(\"x\") = 1
  d.Add \"y\", 2
  d(\"x\") = d(\"x\") + 10
  Debug.Print d.Count, d(\"x\"), d.Exists(\"y\"), d.Exists(\"q\")
  Dim k
  For Each k In d.Keys
    Debug.Print k & \"=\" & d(k)
  Next
  Dim e As Object
  Debug.Print e Is Nothing, TypeName(d)
End Sub";
    assert_eq!(printed(code), [" 3            z             b             a", " 2             11           True          False", "x=11", "y=2", "True          Dictionary"]);
}

#[test]
fn arrays() {
    let code = "Sub Main()
  Dim a(1 To 3) As Long, i As Long
  For i = 1 To 3: a(i) = i * i: Next
  Debug.Print LBound(a), UBound(a), a(3)
  Dim b() As String
  ReDim b(2)
  b(0) = \"x\": b(2) = \"z\"
  ReDim Preserve b(4)
  Debug.Print UBound(b), b(0) & b(2)
  Dim m(2, 1 To 2)
  m(2, 2) = 7
  Debug.Print UBound(m, 1), LBound(m, 2), m(2, 2)
  Dim v
  v = Array(1, \"two\", 3)
  Debug.Print UBound(v), v(1), IsArray(v)
  Dim w
  w = v
  w(0) = 100
  Debug.Print v(0), w(0)
  Dim parts
  parts = Split(\"a,b,,c\", \",\")
  Debug.Print UBound(parts), Join(parts, \"-\")
End Sub";
    assert_eq!(printed(code), [" 1             3             9 ", " 4            xz", " 2             1             7 ", " 2            two           True", " 1             100 ", " 3            a-b--c"]);
    assert_eq!(error("Dim a(2)\na(3) = 1").number, 9);
}

#[test]
fn text_functions() {
    assert_eq!(
        body("Debug.Print Len(\"hello\"), Left(\"hello\", 2), Right(\"hello\", 3), Mid(\"hello\", 2, 3), Mid(\"hello\", 4)
Debug.Print InStr(\"hello\", \"l\"), InStr(4, \"hello\", \"l\"), InStr(\"hello\", \"z\"), InStrRev(\"hello\", \"l\")
Debug.Print UCase(\"ab\") & LCase(\"CD\") & \"[\" & Trim(\"  x  \") & \"]\" & LTrim(\" y\") & RTrim(\"z \")
Debug.Print Replace(\"aXbXc\", \"X\", \"--\"), StrReverse(\"abc\"), Space(2) & \"|\", String(3, \"*\")
Debug.Print Chr(65), Asc(\"a\"), StrComp(\"a\", \"B\", vbTextCompare), Val(\"12.5kg\"), Hex(255)"),
        [" 5            he            llo           ell           lo", " 3             4             0             4 ", "ABcd[x]yz", "a--b--c       cba             |           ***", "A              97           -1             12.5         FF"]
    );
}

#[test]
fn conversion_and_information() {
    assert_eq!(
        body("Debug.Print CInt(\"12\") + 1, CDbl(\"1.5\") * 2, CStr(3) & \"x\", CBool(\"True\"), CLng(2.5), CLng(3.5)
Debug.Print IsNumeric(\"12\"), IsNumeric(\"a\"), IsEmpty(x), IsNull(Null), IsDate(\"2024-03-01\"), IsNumeric(Empty)
Debug.Print VarType(1), VarType(\"s\"), VarType(1.5), IIf(1 > 2, \"y\", \"n\"), Choose(2, \"a\", \"b\", \"c\")
Debug.Print Format(1234.5, \"#,##0.00\"), Format(0.25, \"0%\"), Format(3, \"000\"), Format(\"abc\", \">\")
Debug.Print Round(2.5), Round(3.14159, 2), Int(-2.5), Fix(-2.5), Abs(-3), Sgn(-4), Sqr(16)"),
        [" 13            3            3x            True           2             4 ", "True          False         True          True          True          True", " 2             8             5            n             b", "1,234.50      25%           003           ABC", " 2             3.14         -3            -2             3            -1             4 "]
    );
    assert_eq!(error("x = CInt(\"abc\")").number, 13);
}

#[test]
fn dates() {
    assert_eq!(
        body("Dim d As Date
d = DateSerial(2024, 1, 31)
Debug.Print Year(d), Month(d), Day(d), Weekday(d)
Debug.Print Format(DateAdd(\"m\", 1, d), \"yyyy-mm-dd\"), DateDiff(\"d\", d, #3/1/2024#)
Debug.Print Format(d + TimeSerial(13, 5, 0), \"dd/mm/yyyy hh:nn\"), Hour(TimeSerial(13, 5, 9)), Minute(#1:05:09 PM#)
Debug.Print MonthName(2), WeekdayName(1), Format(#12/25/2023#, \"mmm d\")"),
        [" 2024          1             31            4 ", "2024-02-29     30 ", "31/01/2024 13:05             13            5 ", "February      Sunday        Dec 25"]
    );
    // Now is a date; Timer counts seconds.
    assert_eq!(body("Debug.Print TypeName(Now), Now > DateSerial(2020, 1, 1), Timer >= 0"), ["Date          True          True"]);
}

#[test]
fn msgbox_and_inputbox() {
    let mut wb = workbook();
    let mut ui = TestUi::default();
    ui.answers.push_back(6);
    ui.inputs.push_back(Some("Ada".into()));
    ui.inputs.push_back(None);
    let code = "Sub Main()
  Dim r, n
  r = MsgBox(\"Continue?\", vbYesNo + vbQuestion, \"Q\")
  If r = vbYes Then n = InputBox(\"Name?\", \"Who\", \"x\")
  Debug.Print n, \"[\" & InputBox(\"Again\") & \"]\"
  MsgBox \"Bye \" & n
End Sub";
    let out = run_on(&mut wb, code, &mut ui);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.shown, ["Continue?", "? Name?", "? Again", "Bye Ada"]);
    assert_eq!(ui.printed, ["Ada           []"]);
}

#[test]
fn end_statement_and_goto() {
    assert_eq!(body("Dim i\ni = 0\nAgain:\ni = i + 1\nIf i < 3 Then GoTo Again\nDebug.Print i\nEnd\nDebug.Print \"never\""), [" 3 "]);
}

#[test]
fn limits_stop_runaway_macros() {
    let mut wb = workbook();
    let mut ui = TestUi::default();
    let p = project("Sub Main()\n  Do\n  Loop\nEnd Sub");
    let opts = apex_vba::Options { max_steps: 10_000, ..Default::default() };
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    let e = out.error.unwrap();
    assert!(e.message.contains("stopped"), "{e}");
    // Not even On Error Resume Next keeps it going.
    let p = project("Sub Main()\n  On Error Resume Next\n  Dim i\n  For i = 1 To 1E+12\n  Next\nEnd Sub");
    let opts = apex_vba::Options { max_time: std::time::Duration::from_millis(200), ..Default::default() };
    let t = std::time::Instant::now();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    assert!(out.error.is_some());
    assert!(t.elapsed() < std::time::Duration::from_secs(10));
    // Endless recursion.
    let p = project("Sub R()\n  R\nEnd Sub\nSub Main()\n  R\nEnd Sub");
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &apex_vba::Options::default(), "Main");
    assert_eq!(out.error.unwrap().number, 28);
}

#[test]
fn unsupported_features_say_so() {
    let p = apex_vba::Project::parse(&[
        apex_vba::Source { name: "Module1".into(), kind: apex_vba::ModuleKind::Standard, code: "#If VBA7 Then\nPrivate Declare PtrSafe Function FindWindow Lib \"user32\" Alias \"FindWindowA\" () As LongPtr\n#Else\nPrivate Declare Function FindWindow Lib \"user32\" Alias \"FindWindowA\" () As Long\n#End If\nSub A()\n  x = FindWindow()\nEnd Sub\nSub B()\n  UserForm1.Show\nEnd Sub\nSub C()\n  Set o = CreateObject(\"Outlook.Application\")\nEnd Sub\nSub D()\n  Shell \"calc.exe\"\nEnd Sub\n".into() },
        apex_vba::Source { name: "UserForm1".into(), kind: apex_vba::ModuleKind::Form, code: String::new() },
    ])
    .unwrap();
    for (name, number) in [("A", 445), ("C", 429), ("D", 445)] {
        let mut wb = workbook();
        let mut ui = TestUi::default();
        let e = apex_vba::run_macro(&p, &mut wb, &mut ui, &apex_vba::Options::default(), name).error.unwrap();
        assert_eq!(e.number, number, "{name}: {e}");
        assert!(e.message.contains("not supported") || e.message.contains("Not supported"), "{e}");
    }
}

#[test]
fn harmless_windows_functions() {
    let code = "Private Declare PtrSafe Sub Sleep Lib \"kernel32\" (ByVal ms As Long)
Private Declare PtrSafe Function GetTickCount Lib \"kernel32.dll\" () As Long
Private Declare PtrSafe Function Ticks Lib \"winmm\" Alias \"timeGetTime\" () As Long
Private Declare PtrSafe Function QueryPerformanceCounter Lib \"kernel32\" (c As Currency) As Long
Private Declare PtrSafe Function QueryPerformanceFrequency Lib \"kernel32\" (f As Currency) As Long
Private Declare PtrSafe Function Fake Lib \"C:\\x\\kernel32.dll\" Alias \"Sleep\" (ByVal ms As Long) As Long
Sub Main()
  Dim t As Long, c1 As Currency, c2 As Currency, f As Currency
  t = GetTickCount()
  Sleep 10
  Debug.Print GetTickCount() >= t And Ticks() >= 0
  QueryPerformanceFrequency f
  QueryPerformanceCounter c1
  QueryPerformanceCounter c2
  Debug.Print f > 0 And c2 >= c1
End Sub
Sub Other()
  Fake 1
End Sub
";
    assert_eq!(printed(code), ["True", "True"]);
    let mut wb = workbook();
    let e = apex_vba::run_macro(&project(code), &mut wb, &mut TestUi::default(), &apex_vba::Options::default(), "Other").error.unwrap();
    assert_eq!(e.number, 445, "{e}");
}

#[test]
fn keywords_before_dots_and_parentheses() {
    let out = body(
        "Dim c As New Collection, i As Long
c.Add \"a\"
With c
  With .Item(1) & \"b\"
  End With
  If .Count = 1 Then Debug.Print \"one\"
End With
i = 0
While (i < 3)
  i = i + 1
Wend
Do While (i > 0)
  i = i - 2
Loop
Debug.Print i",
    );
    assert_eq!(out, ["one", "-1 "]);
}
