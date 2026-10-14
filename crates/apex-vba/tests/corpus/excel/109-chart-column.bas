' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim co As ChartObject
    Range("A1").Value = "Q1": Range("B1").Value = 10
    Range("A2").Value = "Q2": Range("B2").Value = 20
    Range("A3").Value = "Q3": Range("B3").Value = 15
    Set co = ActiveSheet.ChartObjects.Add(Left:=100, Top:=50, Width:=300, Height:=200)
    co.Chart.SetSourceData Source:=Range("A1:B3")
    co.Chart.ChartType = xlColumnClustered
    Debug.Print ActiveSheet.ChartObjects.Count
End Sub
