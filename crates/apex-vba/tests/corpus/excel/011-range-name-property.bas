' corpus-source: generated
' corpus-expect: ok
' corpus-note: Range.Name (setting a name on a range) is not supported
' corpus-flagged: no
' corpus-prints: 12
Sub Main()
    Range("B1:B3").Name = "Prices"
    Range("Prices").Value = 4
    Debug.Print WorksheetFunction.Sum(Range("Prices"))
End Sub
