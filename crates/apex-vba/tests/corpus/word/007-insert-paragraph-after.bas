' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
' corpus-prints: Appended line.
Sub Main()
    ActiveDocument.Content.InsertParagraphAfter
    ActiveDocument.Paragraphs(4).Range.InsertBefore "Appended line."
    Debug.Print ActiveDocument.Paragraphs.Count
    Debug.Print Replace(ActiveDocument.Paragraphs(4).Range.Text, vbCr, "")
End Sub
