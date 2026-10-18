' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim r As Long, lastRow As Long
    Range("A1").Value = "a": Range("A3").Value = "b": Range("A6").Value = "c"
    lastRow = 6
    For r = lastRow To 1 Step -1
        If WorksheetFunction.CountA(Rows(r)) = 0 Then Rows(r).Delete
    Next r
    Debug.Print Cells(Rows.Count, 1).End(xlUp).Row
End Sub
