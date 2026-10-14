' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: THE QUICK BROWN FOX.
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(1).Range
    r.MoveEnd wdCharacter, -1
    r.Text = UCase(r.Text)
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
End Sub
