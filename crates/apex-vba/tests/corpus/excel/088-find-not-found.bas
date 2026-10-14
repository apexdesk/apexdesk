' corpus-source: generated
' corpus-flagged: no
' corpus-prints: missing
Sub Main()
    Dim f As Range
    Range("A1").Value = "x"
    Set f = Range("A1:A5").Find("zzz")
    If f Is Nothing Then Debug.Print "missing"
End Sub
