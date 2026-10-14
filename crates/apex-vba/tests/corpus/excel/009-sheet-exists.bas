' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: False
Function SheetExists(nm As String) As Boolean
    Dim ws As Worksheet
    For Each ws In ThisWorkbook.Worksheets
        If ws.Name = nm Then SheetExists = True: Exit Function
    Next ws
End Function
Sub Main()
    Debug.Print SheetExists("Sheet1")
    Debug.Print SheetExists("Nope")
End Sub
