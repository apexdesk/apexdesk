' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
Sub Main()
    Dim i As Long
    ActiveDocument.Content.InsertAfter vbCr & vbCr
    For i = ActiveDocument.Paragraphs.Count To 1 Step -1
        If Len(ActiveDocument.Paragraphs(i).Range.Text) <= 1 Then ActiveDocument.Paragraphs(i).Range.Delete
    Next i
    Debug.Print ActiveDocument.Paragraphs.Count
End Sub
