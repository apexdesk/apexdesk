' corpus-source: generated
' corpus-flagged: no
' corpus-prints: handled
' corpus-prints: 11
Sub Main()
    Dim x As Long
    On Error GoTo Oops
    x = 5 / 0
    Debug.Print "not here"
    Exit Sub
Oops:
    Debug.Print "handled"
    Debug.Print Err.Number
End Sub
