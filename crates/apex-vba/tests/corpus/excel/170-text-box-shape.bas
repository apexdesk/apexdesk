' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim shp As Shape
    Set shp = ActiveSheet.Shapes.AddShape(msoShapeRectangle, 10, 10, 100, 50)
    shp.Name = "Box1"
    shp.TextFrame2.TextRange.Text = "Hi"
    Debug.Print ActiveSheet.Shapes.Count
End Sub
