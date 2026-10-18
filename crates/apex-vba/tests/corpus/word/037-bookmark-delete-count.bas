' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
' corpus-prints: 1
Sub Main()
    ActiveDocument.Bookmarks.Add "A", ActiveDocument.Paragraphs(1).Range
    ActiveDocument.Bookmarks.Add "B", ActiveDocument.Paragraphs(2).Range
    Debug.Print ActiveDocument.Bookmarks.Count
    ActiveDocument.Bookmarks("A").Delete
    Debug.Print ActiveDocument.Bookmarks.Count
End Sub
