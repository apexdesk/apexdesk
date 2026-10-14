' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Sales
Sub Main()
    Dim ch As Chart
    Range("A1:B3").Value = 3
    Set ch = ActiveSheet.ChartObjects.Add(10, 10, 300, 200).Chart
    ch.SetSourceData Range("A1:B3")
    ch.ChartType = xlLine
    ch.HasTitle = True
    ch.ChartTitle.Text = "Sales"
    Debug.Print ch.ChartTitle.Text
End Sub
