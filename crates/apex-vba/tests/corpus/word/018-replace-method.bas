' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The quick brown wolf.
Sub Main()
    Dim s As String
    s = ActiveDocument.Paragraphs(1).Range.Text
    s = Replace(s, "fox", "wolf")
    s = Replace(s, vbCr, "")
    Debug.Print s
End Sub
