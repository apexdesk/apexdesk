' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Heading 1
' corpus-prints: Normal
Sub Main()
    ActiveDocument.Paragraphs(1).Style = ActiveDocument.Styles(wdStyleHeading1)
    Debug.Print ActiveDocument.Paragraphs(1).Style
    Debug.Print ActiveDocument.Paragraphs(2).Style
End Sub
