' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 60
Sub Main()
    Range("A1").Value = 10
    Range("A2").Value = 20
    Range("A3").Value = 30
    ThisWorkbook.Names.Add Name:="Amounts", RefersTo:="=Sheet1!$A$1:$A$3"
    Range("C1").Formula = "=SUM(Amounts)"
    Debug.Print Range("C1").Value
End Sub
