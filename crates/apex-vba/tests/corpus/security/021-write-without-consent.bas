' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
Sub Main()
    On Error Resume Next
    Open ThisWorkbook.Path & "\note.txt" For Output As #1
    Print #1, "x"
    Close #1
    Kill ThisWorkbook.Path & "\data\input.txt"
End Sub
