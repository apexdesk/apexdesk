//! User-defined types and fixed-length strings.

mod common;

use apex_vba::{ModuleKind, Options, Project, Source};
use common::*;

const PT: &str = "Type Pt
    x As Long
    y As Long
    tag As String * 3
    hist(1 To 3) As Double
End Type

Type Shape
    name As String
    corner As Pt
    pts() As Pt
End Type
";

fn run(main: &str) -> Vec<String> {
    printed(&format!("{PT}\n{main}"))
}

#[test]
fn fields_are_read_and_written() {
    let p = run("Sub Main()
Dim p As Pt
p.x = 3: p.y = \"4\"
p.tag = \"abcdef\"
p.hist(2) = 1.5
Debug.Print p.x + p.y, \"[\" & p.tag & \"]\", p.hist(2), TypeName(p), VarType(p)
End Sub");
    assert_eq!(p, [" 7            [abc]          1.5          Pt             36 "]);
}

#[test]
fn records_are_copied() {
    let p = run("Sub Main()
Dim a As Pt, b As Pt
a.x = 1
b = a
b.x = 2
Debug.Print a.x, b.x
Dim v
v = a
a.x = 5
Debug.Print v.x
End Sub");
    assert_eq!(p, [" 1             2 ", " 1 "]);
}

#[test]
fn arrays_nesting_and_with() {
    let p = run("Sub Main()
Dim s As Shape, i As Long
s.name = \"tri\"
s.corner.x = 9
ReDim s.pts(1 To 2)
s.pts(2).x = 5
ReDim Preserve s.pts(1 To 3)
s.pts(3).y = 6
Debug.Print s.name, s.corner.x, s.pts(2).x, s.pts(3).y, UBound(s.pts)
End Sub");
    assert_eq!(p, ["tri            9             5             6             3 "]);
    let p = run("Sub Main()
Dim pts(1 To 3) As Pt, i As Long
For i = 1 To 3
    pts(i).x = i * 10
    pts(i).hist(1) = i
Next
With pts(2)
    .y = .x + 1
    Debug.Print .x, .y
End With
Dim s As Shape
s.corner.x = 9
s.corner.hist(3) = 7
With s.corner
    .y = 8
End With
Debug.Print pts(1).x + pts(3).x, pts(2).y, s.corner.x, s.corner.y, s.corner.hist(3)
Dim many() As Pt
ReDim many(1 To 2)
many(2).x = 4
ReDim Preserve many(1 To 3)
many(3).y = 1
Debug.Print many(2).x, many(3).x, many(3).y, UBound(many)
End Sub");
    assert_eq!(p, [" 20            21 ", " 40            21            9             8             7 ", " 4             0             1             3 "]);
}

#[test]
fn passing_and_returning() {
    let p = run("Function Make(x As Long) As Pt
    Make.x = x
    Make.y = x * 2
End Function

Sub Bump(p As Pt)
    p.x = p.x + 1
End Sub

Sub Main()
Dim p As Pt
p = Make(5)
Bump p
Debug.Print p.x, p.y
End Sub");
    assert_eq!(p, [" 6             10 "]);
}

#[test]
fn fixed_length_strings() {
    let p = body("Dim s As String * 5
Debug.Print Len(s), Asc(s)
s = \"ab\"
Debug.Print \"[\" & s & \"]\"
s = \"abcdefg\"
Debug.Print s
Dim a(1 To 2) As String * 2
a(1) = \"xyz\"
Debug.Print a(1) & \"|\"");
    assert_eq!(p, [" 5             0 ", "[ab   ]", "abcde", "xy|"]);
}

#[test]
fn wrong_assignments_fail() {
    let e = error_of("Sub Main()\nDim p As Pt\np = 5\nEnd Sub");
    assert!(e.contains("mismatch"), "{e}");
    let e = error_of("Sub Main()\nDim p As Pt\np.z = 5\nEnd Sub");
    assert!(e.contains("not found"), "{e}");
    let e = error_of("Sub Main()\nDim p As Pt\np.hist(9) = 1\nEnd Sub");
    assert!(e.contains("Subscript"), "{e}");
    let e = error_of("Type Loop1\n  a As Loop1\nEnd Type\nSub Main()\nDim l As Loop1\nEnd Sub");
    assert!(e.contains("contains itself"), "{e}");
}

fn error_of(code: &str) -> String {
    let code = if code.starts_with("Type Loop1") { code.to_string() } else { format!("{PT}\n{code}") };
    let mut wb = workbook();
    let mut ui = TestUi::default();
    run_on(&mut wb, &code, &mut ui).error.expect("an error").to_string()
}

#[test]
fn public_types_of_other_modules() {
    let p = Project::parse(&[
        Source { name: "Types".into(), kind: ModuleKind::Standard, code: "Public Type Item\n  n As Integer\nEnd Type\nPrivate Type Hidden\n  h As Long\nEnd Type".into() },
        Source { name: "Module1".into(), kind: ModuleKind::Standard, code: "Sub Main()\nDim i As Item\ni.n = 2\nDebug.Print i.n\nEnd Sub".into() },
    ])
    .unwrap();
    let mut wb = workbook();
    let mut ui = TestUi::default();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Main");
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.printed, [" 2 "]);
}

#[test]
fn gosub_mid_and_lset() {
    let p = body("Dim i As Long, t As Long
For i = 1 To 3
    If i <> 2 Then GoSub AddIt
Next
Debug.Print t
Dim s As String
s = \"hello world\"
Mid(s, 1, 1) = \"J\"
Mid$(s, 7) = \"WORLDS!!\"
Debug.Print s
Dim f As String
f = \"12345\"
LSet f = \"ab\"
Debug.Print \"[\" & f & \"]\"
RSet f = \"ab\"
Debug.Print \"[\" & f & \"]\"
Exit Sub
AddIt:
    t = t + i
    Return");
    assert_eq!(p, [" 4 ", "Jello WORLD", "[ab   ]", "[   ab]"]);
    let e = error("Return");
    assert_eq!(e.number, 3);
    let e = error("Dim s As String\ns = \"ab\"\nMid(s, 5) = \"x\"");
    assert_eq!(e.number, 5);
}
