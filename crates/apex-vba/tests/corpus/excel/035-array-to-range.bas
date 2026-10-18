' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 9
' corpus-prints: 5
Sub Main()
    Dim arr(1 To 3, 1 To 2) As Long
    Dim r As Long, c As Long
    For r = 1 To 3
        For c = 1 To 2
            arr(r, c) = r + c
        Next c
    Next r
    Range("A1:B3").Value = arr
    Debug.Print Range("B3").Value + 4
    Debug.Print Range("B3").Value
End Sub
