' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: Name
Sub Main()
    Dim cc As ContentControl
    Set cc = ActiveDocument.ContentControls.Add(wdContentControlText, ActiveDocument.Paragraphs(3).Range)
    cc.Title = "Name"
    cc.SetPlaceholderText Text:="Enter name"
    Debug.Print ActiveDocument.ContentControls.Count
    Debug.Print cc.Title
End Sub
