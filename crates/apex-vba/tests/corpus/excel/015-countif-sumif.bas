' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
' corpus-prints: 70
Sub Main()
    Range("A1").Value = "red": Range("B1").Value = 10
    Range("A2").Value = "blue": Range("B2").Value = 20
    Range("A3").Value = "red": Range("B3").Value = 60
    Debug.Print WorksheetFunction.CountIf(Range("A1:A3"), "red")
    Debug.Print WorksheetFunction.SumIf(Range("A1:A3"), "red", Range("B1:B3"))
End Sub
