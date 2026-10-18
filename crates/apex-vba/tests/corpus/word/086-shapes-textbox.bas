' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Document.Shapes / AddTextbox is not supported
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim s As Shape
    Set s = ActiveDocument.Shapes.AddTextbox(msoTextOrientationHorizontal, 50, 50, 200, 40)
    s.TextFrame.TextRange.Text = "Note"
    Debug.Print ActiveDocument.Shapes.Count
End Sub
