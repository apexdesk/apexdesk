' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Dim i As Long, best As Long, n As Long
    For i = 1 To ActiveDocument.Paragraphs.Count
        If Len(ActiveDocument.Paragraphs(i).Range.Text) > n Then
            n = Len(ActiveDocument.Paragraphs(i).Range.Text)
            best = i
        End If
    Next i
    Debug.Print best
End Sub
