' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Document.Variables is not supported
' corpus-flagged: no
' corpus-prints: 42
Sub Main()
    ActiveDocument.Variables.Add "answer", 42
    Debug.Print ActiveDocument.Variables("answer").Value
End Sub
