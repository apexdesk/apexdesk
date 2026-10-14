' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 12
' corpus-prints: 34
Sub Main()
    Dim c As Range, p() As String
    Range("A1").Value = "12;34"
    p = Split(Range("A1").Value, ";")
    Range("B1").Value = Val(p(0))
    Range("C1").Value = Val(p(1))
    Debug.Print Range("B1").Value
    Debug.Print Range("C1").Value
End Sub
