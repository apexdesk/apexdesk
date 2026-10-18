' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 6
Sub Main()
    Dim i As Long, found As Long
    For i = 1 To 100
        If i * i > 30 Then found = i: Exit For
    Next i
    Debug.Print found
End Sub
