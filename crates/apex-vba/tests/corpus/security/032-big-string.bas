' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
Sub Main()
    On Error Resume Next
    Dim s As String
    s = "x"
    Do
    s = s & s
    Loop
End Sub
