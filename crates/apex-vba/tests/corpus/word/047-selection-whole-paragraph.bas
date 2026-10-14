' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Jumps over the lazy dog.
Sub Main()
    Selection.HomeKey wdStory
    Selection.MoveDown wdParagraph, 1
    Selection.Expand wdParagraph
    Debug.Print Replace(Selection.Text, vbCr, "")
End Sub
