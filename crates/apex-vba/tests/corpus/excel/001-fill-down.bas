' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 55
Sub Main()
    Dim i As Long
    For i = 1 To 10
        Cells(i, 1).Value = i
    Next i
    Debug.Print WorksheetFunction.Sum(Range("A1:A10"))
End Sub
