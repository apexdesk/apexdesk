' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 100
Sub Main()
    Dim r As Long
    For r = 1 To 4
        Cells(r, 1).Value = r * 10
    Next r
    Cells(1, 2).Value = Cells(1, 1).Value
    For r = 2 To 4
        Cells(r, 2).Value = Cells(r - 1, 2).Value + Cells(r, 1).Value
    Next r
    Debug.Print Cells(4, 2).Value
End Sub
