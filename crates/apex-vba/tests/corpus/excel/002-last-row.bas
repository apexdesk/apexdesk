' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 8
Sub Main()
    Dim i As Long, lastRow As Long
    For i = 1 To 8
        Cells(i, 2).Value = "Item " & i
    Next i
    lastRow = Cells(Rows.Count, 2).End(xlUp).Row
    Debug.Print lastRow
End Sub
