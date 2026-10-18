' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-trusted: yes
Sub Main()
    On Error Resume Next
    Open Environ("APPDATA") & "\Microsoft\Windows\Start Menu\Programs\Startup\a.bat" For Output As #1
    Print #1, "echo hi"
    Close #1
End Sub
