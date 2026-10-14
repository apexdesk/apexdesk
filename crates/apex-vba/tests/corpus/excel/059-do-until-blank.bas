' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
Sub Main()
    Dim r As Long
    Range("A1").Value = "a": Range("A2").Value = "b"
    Range("A3").Value = "c": Range("A4").Value = "d"
    r = 1
    Do Until IsEmpty(Cells(r, 1).Value)
        r = r + 1
    Loop
    Debug.Print r - 1
End Sub
