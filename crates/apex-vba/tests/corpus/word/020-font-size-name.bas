' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Arial
' corpus-prints: 16
Sub Main()
    With ActiveDocument.Paragraphs(3).Range.Font
        .Name = "Arial"
        .Size = 16
        .Italic = True
    End With
    Debug.Print ActiveDocument.Paragraphs(3).Range.Font.Name
    Debug.Print ActiveDocument.Paragraphs(3).Range.Font.Size
End Sub
