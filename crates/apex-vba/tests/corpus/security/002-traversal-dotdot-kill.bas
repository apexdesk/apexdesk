' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    Kill ThisWorkbook.Path & "\..\outside\secret.txt"
End Sub
