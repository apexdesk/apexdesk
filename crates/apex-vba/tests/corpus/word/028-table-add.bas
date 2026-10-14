' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: 3
' corpus-prints: 2
Sub Main()
    Dim t As Table
    Set t = ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(3).Range, 3, 2)
    Debug.Print ActiveDocument.Tables.Count
    Debug.Print t.Rows.Count
    Debug.Print t.Columns.Count
End Sub
