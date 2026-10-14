' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Dim avg As Double, c As Range, n As Long
    Range("A1").Value = 1: Range("A2").Value = 2: Range("A3").Value = 9: Range("A4").Value = 8
    avg = WorksheetFunction.Average(Range("A1:A4"))
    For Each c In Range("A1:A4")
        If c.Value > avg Then c.Interior.Color = vbYellow: n = n + 1
    Next c
    Debug.Print n
End Sub
