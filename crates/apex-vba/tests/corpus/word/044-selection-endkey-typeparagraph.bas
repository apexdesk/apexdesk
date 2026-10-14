' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
' corpus-prints: New last
Sub Main()
    Selection.EndKey Unit:=wdStory
    Selection.TypeParagraph
    Selection.TypeText "New last"
    Debug.Print ActiveDocument.Paragraphs.Count
    Debug.Print Replace(ActiveDocument.Paragraphs(4).Range.Text, vbCr, "")
End Sub
