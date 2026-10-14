' corpus-source: generated
' corpus-flagged: no
' corpus-prints: new
' corpus-prints: first
' corpus-prints: right
Sub Main()
    Range("A1").Value = "first"
    Range("B1").Value = "right"
    Rows(1).Insert
    Range("A1").Value = "new"
    Columns(1).Insert
    Range("A2").Value = "x"
    Debug.Print Range("B1").Value
    Debug.Print Range("B2").Value
    Debug.Print Range("C2").Value
End Sub
