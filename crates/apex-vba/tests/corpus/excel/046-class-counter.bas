' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
'--- module Counter class
Private n As Long
Public Sub Increment()
    n = n + 1
End Sub
Public Property Get Value() As Long
    Value = n
End Property
'--- module Module1 standard
Sub Main()
    Dim c As New Counter
    c.Increment: c.Increment: c.Increment
    Debug.Print c.Value
End Sub
