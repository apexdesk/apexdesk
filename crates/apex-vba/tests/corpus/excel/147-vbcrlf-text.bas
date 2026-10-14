' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Dim s As String
    s = "line1" & vbCrLf & "line2"
    Debug.Print UBound(Split(s, vbCrLf)) + 1
End Sub
