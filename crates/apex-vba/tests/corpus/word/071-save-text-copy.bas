' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-allow-files: yes
' corpus-prints: The quick brown fox.
Sub Main()
    Dim f As Integer, s As String
    f = FreeFile
    Open ActiveDocument.Path & "\data\copy.txt" For Output As #f
    Print #f, Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
    Close #f
    f = FreeFile
    Open ActiveDocument.Path & "\data\copy.txt" For Input As #f
    Line Input #f, s
    Close #f
    Debug.Print s
End Sub
