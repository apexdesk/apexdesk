' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Range("A1").Value = "Type": Range("B1").Value = "Qty"
    Range("A2").Value = "x": Range("B2").Value = 1
    Range("A3").Value = "y": Range("B3").Value = 2
    Range("A4").Value = "x": Range("B4").Value = 3
    Range("A1:B4").AutoFilter Field:=1, Criteria1:="x"
    Dim r As Long, n As Long
    For r = 2 To 4
        If Not Rows(r).Hidden Then n = n + 1
    Next r
    Debug.Print n
    ActiveSheet.AutoFilterMode = False
End Sub
