' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
' corpus-prints: 3
Sub Main()
    Range("A1").Value = 1
    Range("C4").Value = 2
    Debug.Print ActiveSheet.UsedRange.Rows.Count
    Debug.Print ActiveSheet.UsedRange.Columns.Count
End Sub
