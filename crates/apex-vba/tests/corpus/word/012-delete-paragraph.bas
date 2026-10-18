' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
' corpus-prints: The end.
Sub Main()
    ActiveDocument.Paragraphs(2).Range.Delete
    Debug.Print ActiveDocument.Paragraphs.Count
    Debug.Print Replace(ActiveDocument.Paragraphs(2).Range.Text, vbCr, "")
End Sub
