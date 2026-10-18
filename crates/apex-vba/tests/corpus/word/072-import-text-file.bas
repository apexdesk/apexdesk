' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 6
' corpus-prints: gamma
Sub Main()
    Dim f As Integer, s As String
    f = FreeFile
    Open ActiveDocument.Path & "\data\input.txt" For Input As #f
    Do While Not EOF(f)
        Line Input #f, s
        ActiveDocument.Content.InsertAfter vbCr & s
    Loop
    Close #f
    Debug.Print ActiveDocument.Paragraphs.Count
    Debug.Print Replace(ActiveDocument.Paragraphs(6).Range.Text, vbCr, "")
End Sub
