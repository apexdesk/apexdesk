' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Range("A1").Value = 5: Range("A2").Value = 50
    With Range("A1:A2").FormatConditions.Add(Type:=xlCellValue, Operator:=xlGreater, Formula1:="=10")
        .Interior.Color = RGB(255, 199, 206)
    End With
    Debug.Print Range("A1:A2").FormatConditions.Count
End Sub
