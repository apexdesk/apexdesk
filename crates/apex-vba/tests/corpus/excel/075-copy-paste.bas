' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 10
' corpus-prints: 20
Sub Main()
    Range("A1").Value = 10
    Range("A2").Value = 20
    Range("A1:A2").Copy Destination:=Range("C1")
    Debug.Print Range("C1").Value
    Debug.Print Range("C2").Value
End Sub
