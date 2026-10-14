' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The slow brown fox.
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Range(4, 9)
    r.Text = "slow"
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
End Sub
