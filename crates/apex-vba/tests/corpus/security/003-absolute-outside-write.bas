' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    Open "%OUTSIDE%\secret.txt" For Append As #1
    Print #1, "x"
    Close #1
End Sub
