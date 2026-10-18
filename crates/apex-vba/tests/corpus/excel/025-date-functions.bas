' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2024
' corpus-prints: 3
' corpus-prints: 15
' corpus-prints: 2024-03-20
' corpus-prints: 2024-04-15
Sub Main()
    Dim d As Date
    d = DateSerial(2024, 3, 15)
    Debug.Print Year(d)
    Debug.Print Month(d)
    Debug.Print Day(d)
    Debug.Print Format(d + 5, "yyyy-mm-dd")
    Debug.Print Format(DateAdd("m", 1, d), "yyyy-mm-dd")
End Sub
