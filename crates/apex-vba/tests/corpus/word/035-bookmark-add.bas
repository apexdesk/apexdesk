' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: Jumps over the lazy dog.
Sub Main()
    ActiveDocument.Bookmarks.Add Name:="Second", Range:=ActiveDocument.Paragraphs(2).Range
    Debug.Print ActiveDocument.Bookmarks.Exists("Second")
    Debug.Print Replace(ActiveDocument.Bookmarks("Second").Range.Text, vbCr, "")
End Sub
