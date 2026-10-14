' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: done
Sub Main()
    Dim n As Long
    On Error GoTo fix
    n = n + 1
    Dim v As Long
    v = CLng("abc")
    n = n + 1
    Debug.Print n
    Debug.Print "done"
    Exit Sub
fix:
    n = n + 1
    Resume Next
End Sub
