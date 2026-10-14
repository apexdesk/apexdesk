' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: Sales
Sub Main()
    Dim lo As ListObject
    Range("A1").Value = "Item": Range("B1").Value = "Qty"
    Range("A2").Value = "x": Range("B2").Value = 1
    Range("A3").Value = "y": Range("B3").Value = 2
    Set lo = ActiveSheet.ListObjects.Add(xlSrcRange, Range("A1:B3"), , xlYes)
    lo.Name = "Sales"
    Debug.Print lo.Range.Rows.Count
    Debug.Print lo.Name
End Sub
