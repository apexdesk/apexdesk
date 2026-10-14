' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 45
Sub Main()
    Dim m(1 To 3, 1 To 3) As Long, r As Long, c As Long, t As Long
    For r = 1 To 3
        For c = 1 To 3
            m(r, c) = (r - 1) * 3 + c
            t = t + m(r, c)
        Next c
    Next r
    Debug.Print t
End Sub
