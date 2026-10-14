' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Document.TablesOfContents is not supported
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    ActiveDocument.Paragraphs(1).Style = "Heading 1"
    ActiveDocument.TablesOfContents.Add Range:=ActiveDocument.Range(0, 0), UseHeadingStyles:=True, UpperHeadingLevel:=1, LowerHeadingLevel:=2
    Debug.Print ActiveDocument.TablesOfContents.Count
End Sub
