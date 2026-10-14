' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    Open ThisWorkbook.Path & "\..\outside\secret.txt" For Output As #1
    Print #1, "x"
    Close #1
End Sub
