' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1 2 3 4 5
Sub Main()
    Dim i As Long, j As Long, t As Variant, s As String
    Range("A1").Value = 4: Range("A2").Value = 2: Range("A3").Value = 5
    Range("A4").Value = 1: Range("A5").Value = 3
    For i = 1 To 4
        For j = 1 To 5 - i
            If Cells(j, 1).Value > Cells(j + 1, 1).Value Then
                t = Cells(j, 1).Value
                Cells(j, 1).Value = Cells(j + 1, 1).Value
                Cells(j + 1, 1).Value = t
            End If
        Next j
    Next i
    For i = 1 To 5: s = s & Cells(i, 1).Value & " ": Next i
    Debug.Print Trim(s)
End Sub
