' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Title: The quick brown fox.
Sub Main()
    ActiveDocument.Paragraphs(1).Range.InsertBefore "Title: "
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
End Sub
