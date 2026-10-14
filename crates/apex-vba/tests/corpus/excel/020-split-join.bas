' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: two
' corpus-prints: one|two|three
Sub Main()
    Dim parts() As String
    parts = Split("one,two,three", ",")
    Debug.Print UBound(parts) + 1
    Debug.Print parts(1)
    Debug.Print Join(parts, "|")
End Sub
