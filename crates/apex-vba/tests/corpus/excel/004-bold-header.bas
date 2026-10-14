' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: False
Sub Main()
    Range("A1").Value = "Name"
    Range("B1").Value = "Qty"
    Range("A2").Value = "Apples"
    With Range("A1:B1")
        .Font.Bold = True
        .Interior.Color = RGB(200, 200, 200)
    End With
    Debug.Print Range("A1").Font.Bold
    Debug.Print Range("A2").Font.Bold
End Sub
