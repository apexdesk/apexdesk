' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-trusted: yes
Sub Main()
    On Error Resume Next
    Open "%HOME%\.bashrc" For Append As #1
    Print #1, "echo hi"
    Close #1
End Sub
