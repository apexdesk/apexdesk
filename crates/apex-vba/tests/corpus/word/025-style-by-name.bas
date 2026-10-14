' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Heading 2
Sub Main()
    ActiveDocument.Paragraphs(2).Style = "Heading 2"
    Debug.Print ActiveDocument.Paragraphs(2).Style
End Sub
