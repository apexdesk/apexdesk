' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Cy
' corpus-prints: Al
Sub Main()
    Range("A1").Value = "Name": Range("B1").Value = "Score"
    Range("A2").Value = "Al": Range("B2").Value = 10
    Range("A3").Value = "Bo": Range("B3").Value = 50
    Range("A4").Value = "Cy": Range("B4").Value = 90
    Range("A1:B4").Sort Key1:=Range("B1"), Order1:=xlDescending, Header:=xlYes
    Debug.Print Range("A2").Value
    Debug.Print Range("A4").Value
End Sub
