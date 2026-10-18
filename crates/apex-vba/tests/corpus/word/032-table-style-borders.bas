' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Table.Borders is not supported
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim t As Table
    Set t = ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(3).Range, 2, 2)
    t.Borders.Enable = True
    t.Style = "Table Grid"
    Debug.Print ActiveDocument.Tables.Count
End Sub
