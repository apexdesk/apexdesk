' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: quick
' corpus-prints: brown
Sub Main()
    Dim w As Range
    Set w = ActiveDocument.Paragraphs(1).Range.Words(2)
    Debug.Print Trim(w.Text)
    Debug.Print Trim(ActiveDocument.Paragraphs(1).Range.Words(3).Text)
End Sub
