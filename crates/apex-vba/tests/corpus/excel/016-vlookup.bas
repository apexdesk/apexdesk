' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 25
' corpus-prints: 2
Sub Main()
    Range("A1").Value = "a": Range("B1").Value = 10
    Range("A2").Value = "b": Range("B2").Value = 25
    Range("A3").Value = "c": Range("B3").Value = 40
    Debug.Print WorksheetFunction.VLookup("b", Range("A1:B3"), 2, False)
    Debug.Print WorksheetFunction.Match("b", Range("A1:A3"), 0)
End Sub
