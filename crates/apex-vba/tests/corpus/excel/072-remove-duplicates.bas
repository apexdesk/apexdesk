' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim i As Long, v As Variant
    v = Array("a", "b", "a", "c", "b")
    For i = 0 To 4
        Cells(i + 1, 1).Value = v(i)
    Next i
    Range("A1:A5").RemoveDuplicates Columns:=1, Header:=xlNo
    Debug.Print WorksheetFunction.CountA(Range("A1:A5"))
End Sub
