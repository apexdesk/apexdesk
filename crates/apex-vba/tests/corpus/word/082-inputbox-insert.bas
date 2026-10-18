' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-input: Hola mundo
' corpus-prints: Hola mundo
Sub Main()
    Dim s As String
    s = InputBox("Text to add")
    ActiveDocument.Content.InsertAfter vbCr & s
    Debug.Print Replace(ActiveDocument.Paragraphs(4).Range.Text, vbCr, "")
End Sub
