' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: -1
Sub Main()
    Dim t As Table
    Set t = ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(3).Range, 2, 2)
    t.Cell(1, 1).Range.Text = "Name"
    t.Cell(1, 2).Range.Text = "Age"
    t.Rows(1).Range.Font.Bold = True
    Debug.Print t.Rows(1).Range.Font.Bold
End Sub
