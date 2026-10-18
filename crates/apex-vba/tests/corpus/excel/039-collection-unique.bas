' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim u As New Collection, v As Variant, c As Range
    Range("A1").Value = "x": Range("A2").Value = "y": Range("A3").Value = "x"
    Range("A4").Value = "z": Range("A5").Value = "y"
    On Error Resume Next
    For Each c In Range("A1:A5")
        u.Add c.Value, CStr(c.Value)
    Next c
    On Error GoTo 0
    Debug.Print u.Count
End Sub
