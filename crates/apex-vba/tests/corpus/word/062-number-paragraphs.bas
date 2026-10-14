' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1. The quick brown fox.
' corpus-prints: 3. The end.
Sub Main()
    Dim i As Long
    For i = ActiveDocument.Paragraphs.Count To 1 Step -1
        ActiveDocument.Paragraphs(i).Range.InsertBefore i & ". "
    Next i
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
    Debug.Print Replace(ActiveDocument.Paragraphs(3).Range.Text, vbCr, "")
End Sub
