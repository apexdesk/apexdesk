' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    Open ThisWorkbook.Path & "\a.txt" For Output As #1
    Close #1
    Name ThisWorkbook.Path & "\a.txt" As "%OUTSIDE%\a.txt"
End Sub
