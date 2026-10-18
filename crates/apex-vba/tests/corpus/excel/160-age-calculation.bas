' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 34
Function AgeAt(b As Date, ref As Date) As Long
    AgeAt = DateDiff("yyyy", b, ref)
    If DateSerial(Year(ref), Month(b), Day(b)) > ref Then AgeAt = AgeAt - 1
End Function
Sub Main()
    Debug.Print AgeAt(DateSerial(1990, 6, 15), DateSerial(2024, 6, 14)) + 1
End Sub
