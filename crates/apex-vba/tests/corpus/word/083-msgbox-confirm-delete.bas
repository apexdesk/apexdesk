' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-answer: 6
' corpus-prints: 2
Sub Main()
    If MsgBox("Delete the last paragraph?", vbYesNo + vbExclamation) = vbYes Then
        ActiveDocument.Paragraphs(2).Range.Delete
    End If
    Debug.Print ActiveDocument.Paragraphs.Count
End Sub
