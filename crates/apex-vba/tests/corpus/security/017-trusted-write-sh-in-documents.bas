' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-trusted: yes
Sub Main()
    On Error Resume Next
    Open "%HOME%\Documents\run.sh" For Output As #1
    Print #1, "echo hi"
    Close #1
End Sub
