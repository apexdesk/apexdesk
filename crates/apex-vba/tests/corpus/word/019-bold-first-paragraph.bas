' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: -1
' corpus-prints: 0
Sub Main()
    ActiveDocument.Paragraphs(1).Range.Font.Bold = True
    Debug.Print ActiveDocument.Paragraphs(1).Range.Font.Bold
    Debug.Print ActiveDocument.Paragraphs(2).Range.Font.Bold
End Sub
