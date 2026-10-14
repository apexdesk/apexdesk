' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Sheet1,Two,Three
Sub Main()
    Dim ws As Worksheet, s As String
    Worksheets.Add(After:=Worksheets(1)).Name = "Two"
    Worksheets.Add(After:=Worksheets(2)).Name = "Three"
    For Each ws In ThisWorkbook.Worksheets
        s = s & ws.Name & ","
    Next ws
    Debug.Print Left(s, Len(s) - 1)
End Sub
