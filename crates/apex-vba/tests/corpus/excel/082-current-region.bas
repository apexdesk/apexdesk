' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: 2
Sub Main()
    Range("A1").Value = "h1": Range("B1").Value = "h2"
    Range("A2").Value = 1: Range("B2").Value = 2
    Range("A3").Value = 3: Range("B3").Value = 4
    Debug.Print Range("A1").CurrentRegion.Rows.Count
    Debug.Print Range("A1").CurrentRegion.Columns.Count
End Sub
