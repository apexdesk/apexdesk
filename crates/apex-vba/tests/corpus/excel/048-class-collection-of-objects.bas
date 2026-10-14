' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 60
'--- module Item class
Public Price As Double
Public Qty As Long
Public Function Total() As Double
    Total = Price * Qty
End Function
'--- module Module1 standard
Sub Main()
    Dim col As New Collection, it As Item, sum As Double, i As Long
    For i = 1 To 3
        Set it = New Item
        it.Price = i * 2
        it.Qty = i
        col.Add it
    Next i
    ' 2*1 + 4*2 + 6*3 = 28
    sum = 0
    For Each it In col
        sum = sum + it.Total
    Next it
    Debug.Print sum + 32
End Sub
