' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 60
Sub Main()
    Dim t As Table, r As Long, total As Long, s As String
    Set t = ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(3).Range, 3, 1)
    t.Cell(1, 1).Range.Text = "10"
    t.Cell(2, 1).Range.Text = "20"
    t.Cell(3, 1).Range.Text = "30"
    For r = 1 To t.Rows.Count
        s = t.Cell(r, 1).Range.Text
        s = Left(s, Len(s) - 2)
        total = total + Val(s)
    Next r
    Debug.Print total
End Sub
