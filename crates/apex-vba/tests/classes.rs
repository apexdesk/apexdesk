//! Class modules: objects with their own variables, methods, properties,
//! Me, Implements, Class_Initialize and Class_Terminate.

mod common;

use apex_vba::{ModuleKind, Options, Project, Source};
use common::*;

fn run(classes: &[(&str, &str)], main: &str) -> Result<Vec<String>, String> {
    let mut src: Vec<Source> = classes.iter().map(|(n, c)| Source { name: (*n).into(), kind: ModuleKind::Class, code: (*c).into() }).collect();
    src.push(Source { name: "Module1".into(), kind: ModuleKind::Standard, code: main.into() });
    let p = Project::parse(&src).map_err(|e| format!("{} line {}: {e}", e.module, e.line))?;
    let mut wb = workbook();
    let mut ui = TestUi::default();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Main");
    match out.error {
        Some(e) => Err(format!("{} line {}: {e}", e.module, e.line)),
        None => Ok(ui.printed),
    }
}

const ACCOUNT: &str = "Private mBalance As Double
Public Owner As String
Private Const FEE = 1

Private Sub Class_Initialize()
    mBalance = 10
End Sub

Public Property Get Balance() As Double
    Balance = mBalance
End Property

Public Property Let Balance(ByVal v As Double)
    If v < 0 Then Err.Raise 5, , \"negative\"
    mBalance = v
End Property

Public Sub Deposit(amount As Double)
    mBalance = mBalance + amount - FEE
    Log \"dep\"
End Sub

Private Sub Log(s As String)
    Owner = Owner & \"+\" & s
End Sub

Public Function Twin() As Account
    Dim t As New Account
    t.Balance = Me.Balance
    Set Twin = t
End Function
";

#[test]
fn objects_have_their_own_state() {
    let p = run(
        &[("Account", ACCOUNT)],
        "Sub Main()
Dim a As Account, b As Account
Set a = New Account
Set b = New Account
a.Owner = \"ann\"
a.Deposit 5
b.Balance = 100
Debug.Print a.Balance, b.Balance, a.Owner, TypeName(a), TypeOf a Is Account
Dim c As Account
Set c = a
c.Deposit 1
Debug.Print a.Balance, a Is c, a Is b
Dim t As Account
Set t = b.Twin
Debug.Print t.Balance, t Is b
On Error Resume Next
a.Balance = -1
Debug.Print Err.Number, a.Balance
Err.Clear
a.Log \"x\"
Debug.Print Err.Number
Dim n As New Collection
n.Add a
Debug.Print n(1).Balance
End Sub",
    )
    .unwrap();
    assert_eq!(p, [" 14            100          ann+dep       Account       True", " 14           True          False", " 100          False", " 5             14 ", " 438 ", " 14 "]);
}

#[test]
fn interfaces_and_lifetime() {
    let shape = "Public Function Area() As Double\nEnd Function\n";
    let square = "Implements IShape
Public Side As Double
Private Function IShape_Area() As Double
    IShape_Area = Side * Side
End Function
Private Sub Class_Terminate()
    Debug.Print \"bye\" & Side
End Sub
";
    let p = run(
        &[("IShape", shape), ("Square", square)],
        "Sub Make()
    Dim s As New Square
    s.Side = 1
End Sub

Sub Main()
Dim sq As New Square, sh As IShape
sq.Side = 3
Set sh = sq
Debug.Print sh.Area, TypeOf sh Is IShape, TypeOf sh Is Square, TypeOf sh Is Collection
Make
Set sh = Nothing
Debug.Print \"still\"
Set sq = Nothing
Debug.Print \"end\"
End Sub",
    )
    .unwrap();
    assert_eq!(p, [" 9            True          True          False", "bye1", "still", "bye3", "end"]);
}

#[test]
fn long_chains_and_cycles_are_let_go() {
    let node = "Public NextNode As Node\nPublic Value As Long\n";
    let p = run(
        &[("Node", node)],
        "Sub Main()
Dim head As Node, n As Node, i As Long
For i = 1 To 200000
    Set n = New Node
    n.Value = i
    Set n.NextNode = head
    Set head = n
Next
Debug.Print head.Value, head.NextNode.Value
Dim a As New Node, b As New Node
Set a.NextNode = b
Set b.NextNode = a
Set head = Nothing
Set n = Nothing
Debug.Print \"ok\"
End Sub",
    )
    .unwrap();
    assert_eq!(p, [" 200000        199999 ", "ok"]);
}

#[test]
fn a_class_making_itself_stops() {
    let e = run(&[("Loop1", "Private inner As New Loop1\n")], "Sub Main()\nDim x As New Loop1\nEnd Sub").unwrap_err();
    assert!(e.contains("stack"), "{e}");
    let e = run(&[("Loop2", "Private Sub Class_Initialize()\nDim y As New Loop2\nEnd Sub\n")], "Sub Main()\nDim x As New Loop2\nEnd Sub").unwrap_err();
    assert!(e.contains("stack"), "{e}");
}

#[test]
fn class_events() {
    let counter = "Public Event Changed(ByVal value As Long, Cancel As Boolean)
Public Value As Long
Public Sub Bump()
    Dim c As Boolean
    RaiseEvent Changed(Value + 1, c)
    If Not c Then Value = Value + 1
End Sub
";
    let watcher = "Private WithEvents mC As Counter
Public Seen As String
Public Sub Watch(c As Counter)
    Set mC = c
End Sub
Private Sub mC_Changed(ByVal value As Long, Cancel As Boolean)
    Seen = Seen & value & \";\"
    Cancel = value > 2
End Sub
";
    let p = run(
        &[("Counter", counter), ("Watcher", watcher)],
        "Sub Main()
Dim c As New Counter, w As New Watcher, other As New Counter
c.Bump
w.Watch c
c.Bump
c.Bump
c.Bump
other.Bump
Debug.Print c.Value, w.Seen, other.Value
End Sub",
    )
    .unwrap();
    assert_eq!(p, [" 2            2;3;3;         1 "]);
}
