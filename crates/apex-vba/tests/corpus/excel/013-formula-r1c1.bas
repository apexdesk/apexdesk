' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 60
Sub Main()
    Dim i As Long
    For i = 1 To 3
        Cells(i, 1).Value = i * 5
    Next i
    Range("B1:B3").FormulaR1C1 = "=RC[-1]*2"
    Debug.Print Range("B3").Value + Range("B1").Value + Range("B2").Value - 0
End Sub
