' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-trusted: yes
Sub Main()
    On Error Resume Next
    Open "%HOME%\.config\autostart\x.desktop" For Output As #1
    Print #1, "[Desktop Entry]"
    Close #1
End Sub
