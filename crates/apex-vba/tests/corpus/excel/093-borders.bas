' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Range("A1:C3").Borders.LineStyle = xlContinuous
    Range("A1:C3").Borders.Weight = xlThin
    Range("A1:C1").Borders(xlEdgeBottom).Weight = xlMedium
    Debug.Print Range("B2").Borders(xlEdgeTop).LineStyle * -1 * -1
End Sub
