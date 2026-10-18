' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1,2,5,8,9
Sub Main()
    Dim a As Variant, i As Long, j As Long, t As Variant
    a = Array(5, 2, 9, 1, 8)
    For i = LBound(a) To UBound(a) - 1
        For j = i + 1 To UBound(a)
            If a(j) < a(i) Then t = a(i): a(i) = a(j): a(j) = t
        Next j
    Next i
    Debug.Print Join(a, ",")
End Sub
