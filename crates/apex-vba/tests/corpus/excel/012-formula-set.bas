' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 7
' corpus-prints: =A1+A2
Sub Main()
    Range("A1").Value = 3
    Range("A2").Value = 4
    Range("A3").Formula = "=A1+A2"
    Debug.Print Range("A3").Value
    Debug.Print Range("A3").Formula
End Sub
