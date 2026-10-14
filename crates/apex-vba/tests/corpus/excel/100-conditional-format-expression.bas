' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim fc As FormatCondition
    Range("A1:B3").Value = 1
    Set fc = Range("A1:B3").FormatConditions.Add(Type:=xlExpression, Formula1:="=$A1>0")
    fc.Font.Bold = True
    Debug.Print Range("A1:B3").FormatConditions.Count
    Range("A1:B3").FormatConditions.Delete
End Sub
