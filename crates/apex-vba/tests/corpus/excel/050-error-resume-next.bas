' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 11
' corpus-prints: 0
Sub Main()
    Dim x As Long
    On Error Resume Next
    x = 1 / 0
    Debug.Print Err.Number
    Err.Clear
    Debug.Print Err.Number
End Sub
