' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 0.00
' corpus-prints: $1,234.50
Sub Main()
    Range("A1").Value = 1234.5
    Range("A1").NumberFormat = "0.00"
    Debug.Print Range("A1").NumberFormat
    Range("A1").NumberFormat = "$#,##0.00"
    Debug.Print Range("A1").Text
End Sub
