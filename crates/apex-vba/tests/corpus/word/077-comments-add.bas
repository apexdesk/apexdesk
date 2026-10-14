' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: Check wording
Sub Main()
    ActiveDocument.Comments.Add Range:=ActiveDocument.Paragraphs(1).Range, Text:="Check wording"
    Debug.Print ActiveDocument.Comments.Count
    Debug.Print ActiveDocument.Comments(1).Range.Text
End Sub
