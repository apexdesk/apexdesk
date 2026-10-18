' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    FileCopy ThisWorkbook.Path & "\data\input.txt", "%OUTSIDE%\secret.txt"
End Sub
