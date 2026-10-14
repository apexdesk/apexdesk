' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 99
' corpus-prints: 6
Sub Main()
    Range("A1").Offset(2, 1).Value = 99
    Debug.Print Range("B3").Value
    Debug.Print Range("A1").Resize(3, 2).Cells.Count
End Sub
