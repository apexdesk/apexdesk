' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 36
' corpus-prints: 12
Sub Main()
    With ActiveDocument.Paragraphs(1).Format
        .LeftIndent = 36
        .SpaceAfter = 12
        .LineSpacingRule = wdLineSpaceSingle
    End With
    Debug.Print ActiveDocument.Paragraphs(1).LeftIndent
    Debug.Print ActiveDocument.Paragraphs(1).SpaceAfter
End Sub
