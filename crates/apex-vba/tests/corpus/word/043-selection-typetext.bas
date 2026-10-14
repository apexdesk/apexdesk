' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Hello The quick brown fox.
Sub Main()
    Selection.HomeKey Unit:=wdStory
    Selection.TypeText "Hello "
    Debug.Print Replace(ActiveDocument.Paragraphs(1).Range.Text, vbCr, "")
End Sub
