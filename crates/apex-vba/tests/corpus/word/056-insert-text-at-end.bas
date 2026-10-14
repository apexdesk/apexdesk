' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The end. Thanks!
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(3).Range
    r.MoveEnd wdCharacter, -1
    r.InsertAfter " Thanks!"
    Debug.Print Replace(ActiveDocument.Paragraphs(3).Range.Text, vbCr, "")
End Sub
