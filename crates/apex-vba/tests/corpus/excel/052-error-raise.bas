' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1000
' corpus-prints: custom failure
Sub Main()
    On Error GoTo h
    Err.Raise 1000, "Main", "custom failure"
    Exit Sub
h:
    Debug.Print Err.Number
    Debug.Print Err.Description
End Sub
