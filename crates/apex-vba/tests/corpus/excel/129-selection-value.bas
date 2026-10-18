' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Range("A1:A3").Select
    Selection.Value = 7
    Debug.Print Selection.Cells.Count
End Sub
