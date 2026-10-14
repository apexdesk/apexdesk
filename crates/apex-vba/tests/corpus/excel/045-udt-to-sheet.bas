' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Widget
' corpus-prints: 19.5
Type Product
    Name As String
    Price As Double
End Type
Sub Main()
    Dim p As Product
    p.Name = "Widget"
    p.Price = 19.5
    Range("A1").Value = p.Name
    Range("B1").Value = p.Price
    Debug.Print Range("A1").Value
    Debug.Print Range("B1").Value
End Sub
