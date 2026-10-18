' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim co As ChartObject, s As Series
    Range("A1:A4").Formula = "=ROW()"
    Set co = ActiveSheet.ChartObjects.Add(0, 0, 300, 200)
    co.Chart.ChartType = xlXYScatter
    Set s = co.Chart.SeriesCollection.NewSeries
    s.Values = Range("A1:A4")
    s.Name = "Rows"
    Debug.Print co.Chart.SeriesCollection.Count
End Sub
