' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 100
' corpus-prints: 25
Sub Main()
    Range("A1").Value = 10
    Range("B1").Formula = "=A1^2"
    Range("C1").Formula = "=B1/4"
    Application.Calculate
    Debug.Print Range("B1").Value
    Debug.Print Range("C1").Value
End Sub
