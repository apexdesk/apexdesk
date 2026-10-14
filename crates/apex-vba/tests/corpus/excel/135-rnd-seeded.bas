' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
Sub Main()
    Dim i As Long, ok As Boolean
    ok = True
    Randomize
    For i = 1 To 20
        If Int(Rnd * 10) + 1 > 10 Then ok = False
    Next i
    Debug.Print ok
End Sub
