' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 5
Sub Main()
    Dim c As Long
    For c = 1 To 5
        Cells(1, c).Value = "H" & c
    Next c
    Debug.Print Cells(1, Columns.Count).End(xlToLeft).Column
End Sub
