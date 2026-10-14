' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Jumps over the sleepy dog.
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(2).Range
    r.SetRange r.Start + 15, r.Start + 19
    ActiveDocument.Bookmarks.Add "Adj", r
    ActiveDocument.Bookmarks("Adj").Range.Text = "sleepy"
    Debug.Print Replace(ActiveDocument.Paragraphs(2).Range.Text, vbCr, "")
End Sub
