' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2024-02-29
' corpus-prints: 2024-03-31
Sub Main()
    Dim d As Date
    d = DateSerial(2024, 3, 1) - 1
    Debug.Print Format(d, "yyyy-mm-dd")
    Debug.Print Format(DateSerial(2024, 4, 0), "yyyy-mm-dd")
End Sub
