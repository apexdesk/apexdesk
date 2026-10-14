' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Friday
' corpus-prints: 6
Sub Main()
    Dim d As Date
    d = DateSerial(2024, 3, 15)
    Debug.Print Format(d, "dddd")
    Debug.Print Weekday(d)
End Sub
