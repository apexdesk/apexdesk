' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 20
Sub Main()
    Debug.Print Len(Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, ""))
End Sub
