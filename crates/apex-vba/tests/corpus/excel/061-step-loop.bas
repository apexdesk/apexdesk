' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 25
' corpus-prints: 24
Sub Main()
    Dim i As Long, s As Long
    For i = 1 To 9 Step 2
        s = s + i
    Next i
    Debug.Print s
    s = 0
    For i = 10 To 1 Step -3
        s = s + i
    Next i
    Debug.Print s + 2
End Sub
