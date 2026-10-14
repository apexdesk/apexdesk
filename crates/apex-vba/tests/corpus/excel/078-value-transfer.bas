' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 5
' corpus-prints: 15
Sub Main()
    Range("A1:A3").Value = 5
    Range("B1:B3").Value = Range("A1:A3").Value
    Debug.Print Range("B2").Value
    Debug.Print WorksheetFunction.Sum(Range("B1:B3"))
End Sub
