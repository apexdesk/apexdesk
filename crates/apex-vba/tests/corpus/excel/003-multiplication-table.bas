' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 42
' corpus-prints: 81
Sub Main()
    Dim r As Long, c As Long
    For r = 1 To 9
        For c = 1 To 9
            Cells(r, c).Value = r * c
        Next c
    Next r
    Debug.Print Range("F7").Value
    Debug.Print Range("I9").Value
End Sub
