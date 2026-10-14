' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 255
Sub Main()
    ActiveDocument.Paragraphs(2).Range.Font.Color = RGB(255, 0, 0)
    Debug.Print ActiveDocument.Paragraphs(2).Range.Font.Color
End Sub
