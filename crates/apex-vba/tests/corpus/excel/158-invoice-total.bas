' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 118.5
Sub Main()
    Dim r As Long, total As Double
    Range("A1").Value = "Item": Range("B1").Value = "Qty": Range("C1").Value = "Price"
    Range("A2").Value = "Pen": Range("B2").Value = 10: Range("C2").Value = 1.5
    Range("A3").Value = "Book": Range("B3").Value = 3: Range("C3").Value = 12.5
    Range("A4").Value = "Bag": Range("B4").Value = 1: Range("C4").Value = 66
    For r = 2 To 4
        Cells(r, 4).Value = Cells(r, 2).Value * Cells(r, 3).Value
        total = total + Cells(r, 4).Value
    Next r
    Cells(5, 4).Value = total
    Debug.Print Cells(5, 4).Value
End Sub
