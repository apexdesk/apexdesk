' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: A quick brown fox.
' corpus-prints: Jumps over a lazy dog.
' corpus-prints: A end.
Sub Main()
    Dim p As Paragraph
    With ActiveDocument.Content.Find
        .ClearFormatting
        .Text = "The"
        .Replacement.Text = "A"
        .MatchCase = True
        .Forward = True
        .Wrap = wdFindStop
        .Execute Replace:=wdReplaceAll
    End With
    With ActiveDocument.Content.Find
        .Text = "the"
        .Replacement.Text = "a"
        .MatchCase = True
        .Execute Replace:=wdReplaceAll
    End With
    For Each p In ActiveDocument.Paragraphs
        Debug.Print Replace(p.Range.Text, vbCr, "")
    Next p
End Sub
