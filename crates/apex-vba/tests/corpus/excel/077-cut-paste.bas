' corpus-source: generated
' corpus-flagged: no
' corpus-prints: moved
' corpus-prints: 0
Sub Main()
    Range("A1").Value = "moved"
    Range("A1").Cut Destination:=Range("D4")
    Debug.Print Range("D4").Value
    Debug.Print WorksheetFunction.CountA(Range("A1"))
End Sub
