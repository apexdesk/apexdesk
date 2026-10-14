' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Dear Ann, welcome to Acme.
Sub Main()
    Dim t As String
    t = "Dear <<Name>>, welcome to <<Company>>."
    t = Replace(t, "<<Name>>", "Ann")
    t = Replace(t, "<<Company>>", "Acme")
    ActiveDocument.Content.InsertAfter vbCr & t
    Debug.Print Replace(ActiveDocument.Paragraphs(4).Range.Text, vbCr, "")
End Sub
