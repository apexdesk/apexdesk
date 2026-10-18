' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: done
Sub Main()
    ActiveDocument.Paragraphs(1).Range.InsertBefore "x"
    ActiveDocument.Undo
    Debug.Print "done"
End Sub
