' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-flagged: yes
Sub Main()
    On Error Resume Next
    Shell "cmd /c del ""%OUTSIDE%\secret.txt"""
    CreateObject("WScript.Shell").Run "cmd /c del x"
End Sub
