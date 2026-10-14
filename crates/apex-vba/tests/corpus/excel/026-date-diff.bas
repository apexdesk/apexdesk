' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 30
' corpus-prints: 2
Sub Main()
    Dim a As Date, b As Date
    a = DateSerial(2024, 1, 1)
    b = DateSerial(2024, 1, 31)
    Debug.Print DateDiff("d", a, b)
    Debug.Print DateDiff("m", a, DateSerial(2024, 3, 1))
End Sub
