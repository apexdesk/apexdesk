' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    Open ThisWorkbook.Path & "\run.cmd" For Output As #1
    Print #1, "echo"
    Close #1
    Open ThisWorkbook.Path & "\x.vbs" For Output As #1
    Close #1
End Sub
