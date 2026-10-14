' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The Quick Brown Fox.
Sub Main()
    ActiveDocument.Paragraphs(1).Range.Case = wdTitleWord
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
End Sub
