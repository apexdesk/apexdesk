' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The quick brown fox.
' corpus-prints: The end.
Sub Main()
    With ActiveDocument.Paragraphs
        Debug.Print Replace(.First.Range.Text, vbCr, "")
        Debug.Print Replace(.Last.Range.Text, vbCr, "")
    End With
End Sub
