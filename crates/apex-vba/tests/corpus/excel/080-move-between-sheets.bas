' corpus-source: generated
' corpus-flagged: no
' corpus-prints: from1
Sub Main()
    Worksheets.Add.Name = "Other"
    Worksheets("Sheet1").Range("A1").Value = "from1"
    Worksheets("Other").Range("A1").Value = Worksheets("Sheet1").Range("A1").Value
    Debug.Print Worksheets("Other").Range("A1").Value
End Sub
