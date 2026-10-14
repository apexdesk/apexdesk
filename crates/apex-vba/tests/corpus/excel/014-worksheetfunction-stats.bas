' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 5
' corpus-prints: 3
' corpus-prints: 1
' corpus-prints: 5
' corpus-prints: 3
Sub Main()
    Dim i As Long
    For i = 1 To 5
        Cells(i, 1).Value = i
    Next i
    With WorksheetFunction
        Debug.Print .Max(Range("A1:A5"))
        Debug.Print .Average(Range("A1:A5"))
        Debug.Print .Min(Range("A1:A5"))
        Debug.Print .Count(Range("A1:A5"))
        Debug.Print .Median(Range("A1:A5"))
    End With
End Sub
