' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The quick brown cat.
Sub Main()
    With ActiveDocument.Content.Find
        .Text = "fox"
        .Replacement.Text = "cat"
        .Execute Replace:=wdReplaceAll
    End With
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
End Sub
