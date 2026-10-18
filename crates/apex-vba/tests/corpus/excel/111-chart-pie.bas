' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim co As ChartObject
    Range("A1").Value = "a": Range("B1").Value = 1
    Range("A2").Value = "b": Range("B2").Value = 2
    Set co = ActiveSheet.ChartObjects.Add(0, 0, 250, 200)
    With co.Chart
        .ChartType = xlPie
        .SetSourceData Range("A1:B2")
        .HasLegend = True
    End With
    co.Name = "PieChart"
    Debug.Print ActiveSheet.ChartObjects.Count
End Sub
