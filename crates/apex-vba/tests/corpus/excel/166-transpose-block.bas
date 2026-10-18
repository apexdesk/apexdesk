' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: 1
Sub Main()
    Dim r As Long, c As Long
    For r = 1 To 2
        For c = 1 To 3
            Cells(r, c).Value = (r - 1) * 3 + c
        Next c
    Next r
    For r = 1 To 2
        For c = 1 To 3
            Cells(c + 5, r).Value = Cells(r, c).Value
        Next c
    Next r
    Debug.Print Range("A8").Value
    Debug.Print Range("B6").Value - 3
End Sub
