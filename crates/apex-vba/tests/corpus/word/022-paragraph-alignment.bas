' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: 2
Sub Main()
    ActiveDocument.Paragraphs(1).Alignment = wdAlignParagraphCenter
    ActiveDocument.Paragraphs(2).Alignment = wdAlignParagraphRight
    Debug.Print ActiveDocument.Paragraphs(1).Alignment
    Debug.Print ActiveDocument.Paragraphs(2).Alignment
End Sub
