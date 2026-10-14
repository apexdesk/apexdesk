' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
Sub Main()
    Dim t As Table
    Set t = ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(3).Range, 2, 2)
    t.Rows.Add
    t.Rows.Add
    Debug.Print t.Rows.Count
End Sub
