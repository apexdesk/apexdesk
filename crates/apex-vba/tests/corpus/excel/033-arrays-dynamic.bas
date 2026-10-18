' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 5
' corpus-prints: 16
Sub Main()
    Dim sq() As Long, i As Long
    ReDim sq(1 To 3)
    For i = 1 To 3: sq(i) = i * i: Next
    ReDim Preserve sq(1 To 5)
    sq(4) = 16: sq(5) = 25
    Debug.Print UBound(sq)
    Debug.Print sq(4)
End Sub
