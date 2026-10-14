' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim r As Long, n As Long
    For r = 1 To 6
        If r Mod 2 = 0 Then
            Rows(r).Interior.Color = RGB(230, 230, 230)
            n = n + 1
        End If
    Next r
    Debug.Print n
End Sub
