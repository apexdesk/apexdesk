' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: 9
Sub Main()
    Range("A1").Value = 5: Range("A2").Value = 1: Range("A3").Value = 9
    Range("A4").Value = 3
    Range("A1:A4").Sort Key1:=Range("A1"), Order1:=xlAscending, Header:=xlNo
    Debug.Print Range("A1").Value
    Debug.Print Range("A4").Value
End Sub
