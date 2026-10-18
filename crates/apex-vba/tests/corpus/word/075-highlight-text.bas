' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 7
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(1).Range
    With r.Find
        .Text = "brown"
        If .Execute Then r.HighlightColorIndex = wdYellow
    End With
    Debug.Print r.HighlightColorIndex
End Sub
