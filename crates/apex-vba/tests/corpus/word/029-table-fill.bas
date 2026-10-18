' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: R2C2
Sub Main()
    Dim t As Table, r As Long, c As Long
    Set t = ActiveDocument.Tables.Add(ActiveDocument.Range(ActiveDocument.Content.End - 1, ActiveDocument.Content.End - 1), 3, 3)
    For r = 1 To 3
        For c = 1 To 3
            t.Cell(r, c).Range.Text = "R" & r & "C" & c
        Next c
    Next r
    Debug.Print Replace(Replace(t.Cell(2, 2).Range.Text, vbCr, ""), Chr(7), "")
End Sub
