' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Ann Lee
' corpus-prints: Bob Ray
Sub Main()
    Dim r As Long
    Range("A1").Value = "Ann": Range("B1").Value = "Lee"
    Range("A2").Value = "Bob": Range("B2").Value = "Ray"
    For r = 1 To 2
        Cells(r, 3).Value = Cells(r, 1).Value & " " & Cells(r, 2).Value
        Debug.Print Cells(r, 3).Value
    Next r
End Sub
