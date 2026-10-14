' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 60
' corpus-prints: 3
' corpus-prints: 0
Sub Main()
    Dim a(1 To 3) As Long, i As Long, s As Long
    a(1) = 10: a(2) = 20: a(3) = 30
    For i = LBound(a) To UBound(a)
        s = s + a(i)
    Next i
    Debug.Print s
    Debug.Print UBound(a)
    Dim b(0 To 2) As Long
    Debug.Print b(1)
End Sub
