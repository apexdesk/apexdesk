' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 0
Sub Main()
    ActiveDocument.Tables.Add ActiveDocument.Paragraphs(3).Range, 2, 2
    ActiveDocument.Tables(1).Delete
    Debug.Print ActiveDocument.Tables.Count
End Sub
