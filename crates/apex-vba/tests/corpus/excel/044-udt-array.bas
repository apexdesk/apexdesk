' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: Cy
Private Type Item
    Label As String
    Qty As Long
End Type
Sub Main()
    Dim items(1 To 3) As Item, i As Long
    Dim names As Variant
    names = Array("Al", "Bo", "Cy")
    For i = 1 To 3
        items(i).Label = names(i - 1)
        items(i).Qty = i
    Next i
    Debug.Print items(3).Qty
    Debug.Print items(3).Label
End Sub
