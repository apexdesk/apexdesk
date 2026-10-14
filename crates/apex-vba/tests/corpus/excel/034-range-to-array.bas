' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 6
' corpus-prints: 20
Sub Main()
    Dim v As Variant, r As Long, t As Long
    Range("A1").Value = 1: Range("A2").Value = 2: Range("A3").Value = 3
    Range("B1").Value = 10: Range("B2").Value = 20: Range("B3").Value = 30
    v = Range("A1:B3").Value
    For r = 1 To UBound(v, 1)
        t = t + v(r, 1)
    Next r
    Debug.Print t
    Debug.Print v(2, 2)
End Sub
