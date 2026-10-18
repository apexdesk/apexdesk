' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Dim c As Range, n As Long
    Range("A1").Value = -5
    Range("A2").Value = 10
    Range("A3").Value = -2
    Range("A4").Value = 7
    For Each c In Range("A1:A4")
        If c.Value < 0 Then
            c.Font.Color = RGB(255, 0, 0)
            n = n + 1
        End If
    Next c
    Debug.Print n
End Sub
