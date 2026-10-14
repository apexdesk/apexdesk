' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim d As Object, c As Range, k As Variant
    Set d = CreateObject("Scripting.Dictionary")
    Range("A1").Value = "N": Range("B1").Value = 5
    Range("A2").Value = "S": Range("B2").Value = 3
    Range("A3").Value = "N": Range("B3").Value = 2
    Range("A4").Value = "S": Range("B4").Value = 0
    Dim r As Long
    For r = 1 To 4
        d(Cells(r, 1).Value) = d(Cells(r, 1).Value) + Cells(r, 2).Value
    Next r
    Debug.Print d("S")
End Sub
