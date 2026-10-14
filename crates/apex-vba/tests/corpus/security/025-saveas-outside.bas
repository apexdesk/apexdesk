' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    ThisWorkbook.SaveAs "%OUTSIDE%\secret.txt"
    ThisWorkbook.SaveCopyAs "%OUTSIDE%\copy.xlsm"
End Sub
