' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Alice
Sub Main()
    Dim cc As ContentControl
    Set cc = ActiveDocument.ContentControls.Add(wdContentControlText)
    cc.Range.Text = "Alice"
    Debug.Print ActiveDocument.ContentControls(1).Range.Text
End Sub
