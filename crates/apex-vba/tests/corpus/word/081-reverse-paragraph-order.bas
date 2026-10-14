' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The end.
' corpus-prints: The quick brown fox.
Sub Main()
    Dim a(1 To 3) As String, i As Long
    For i = 1 To 3
        a(i) = Replace(ActiveDocument.Paragraphs(i).Range.Text, vbCr, "")
    Next i
    ActiveDocument.Content.Text = a(3) & vbCr & a(2) & vbCr & a(1)
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
    Debug.Print Replace(ActiveDocument.Paragraphs(3).Range.Text, vbCr, "")
End Sub
