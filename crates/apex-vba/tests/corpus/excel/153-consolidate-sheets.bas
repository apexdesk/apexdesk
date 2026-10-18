' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 6
Sub Main()
    Dim ws As Worksheet, t As Double
    Worksheets.Add.Name = "B"
    Worksheets.Add.Name = "C"
    For Each ws In ThisWorkbook.Worksheets
        ws.Range("A1").Value = 2
        t = t + ws.Range("A1").Value
    Next ws
    Debug.Print t
End Sub
