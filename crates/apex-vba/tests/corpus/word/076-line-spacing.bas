' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
Sub Main()
    ActiveDocument.Content.ParagraphFormat.LineSpacingRule = wdLineSpace1pt5
    Debug.Print ActiveDocument.Paragraphs(1).LineSpacingRule = wdLineSpace1pt5
End Sub
