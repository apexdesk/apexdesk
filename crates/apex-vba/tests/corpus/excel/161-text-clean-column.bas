' corpus-source: generated
' corpus-flagged: no
' corpus-prints: a b c
' corpus-prints: 3
Sub Main()
    Dim s As String
    s = "  a   b    c  "
    Do While InStr(s, "  ") > 0
        s = Replace(s, "  ", " ")
    Loop
    s = Trim(s)
    Debug.Print s
    Debug.Print Len(s) - 2
End Sub
