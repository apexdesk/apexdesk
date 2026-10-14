' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Worksheets.Add.Name = "Temp"
    Application.DisplayAlerts = False
    Worksheets("Temp").Delete
    Application.DisplayAlerts = True
    Debug.Print Worksheets.Count
End Sub
