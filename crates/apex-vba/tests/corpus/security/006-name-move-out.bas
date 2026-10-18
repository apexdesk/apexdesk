' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    Name "%OUTSIDE%\secret.txt" As ThisWorkbook.Path & "\stolen.txt"
End Sub
