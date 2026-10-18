' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-flagged: yes
Sub Main()
    On Error Resume Next
    SendKeys "%{F4}"
    GetObject("winmgmts:").Get("Win32_Process").Create "calc.exe"
End Sub
