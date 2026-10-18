' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 15
Sub Main()
    Dim ws As Worksheet, i As Long
    Set ws = ThisWorkbook.Worksheets("Sheet1")
    For i = 1 To 5
        ws.Cells(i, 1).Value = i
    Next i
    ws.Range("A6").Formula = "=SUM(A1:A5)"
    Debug.Print ws.Range("A6").Value
End Sub
