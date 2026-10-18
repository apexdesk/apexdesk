' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
Sub Main()
    ActiveDocument.Content.InsertAfter vbCr & "Last paragraph"
    Debug.Print ActiveDocument.Paragraphs.Count
End Sub
