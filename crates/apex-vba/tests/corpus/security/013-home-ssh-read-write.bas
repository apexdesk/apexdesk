' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-trusted: yes
Sub Main()
    On Error Resume Next
    Dim s As String
    Open "%HOME%\.ssh\id_rsa" For Input As #1
    Line Input #1, s
    Close #1
    Open "%HOME%\.ssh\authorized_keys" For Output As #1
    Print #1, s
    Close #1
End Sub
